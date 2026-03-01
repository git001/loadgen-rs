import type {
  ScriptCheckCounter,
  ScriptCheckSummary,
  ScriptExtractor,
  ScriptRedirectPolicy,
  ScriptRunResult,
  ScriptScenarioConfig,
  ScriptState,
  ScriptStep,
  ScriptStepErrorMap,
  ScriptStepStats,
  StepResponse,
} from "./types.ts";
import { LoadgenStepFFI } from "./step_ffi.ts";
import { DOMParser } from "jsr:@b-fuze/deno-dom";

interface StepExecutionResult {
  checksTotal: number;
  checksPassed: number;
  checksFailed: number;
}

interface StepEvaluationContext {
  status: number;
  bodyText: string;
  getHeader: (name: string) => string | null;
}

interface CookieEntry {
  name: string;
  value: string;
  domain: string;
  path: string;
  secure: boolean;
  expiresAtMs?: number;
}

function renderTemplate(input: string, state: ScriptState): string {
  return input.replace(
    /\{\{\s*([a-zA-Z0-9_.-]+)\s*\}\}/g,
    (_full, key: string) => {
      const value = state[key];
      if (value === undefined) {
        throw new Error(`missing template variable '{{${key}}}'`);
      }
      return value;
    },
  );
}

function resolveHeaders(
  headers: Record<string, string> | undefined,
  state: ScriptState,
): Record<string, string> {
  if (!headers) {
    return {};
  }
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(headers)) {
    out[k] = renderTemplate(v, state);
  }
  return out;
}

function readJsonPath(value: unknown, path: string): unknown {
  const parts = path.split(".").filter((p) => p.length > 0);
  let current: unknown = value;
  for (const part of parts) {
    if (current === null || current === undefined) {
      return undefined;
    }
    if (Array.isArray(current) && /^\d+$/.test(part)) {
      current = current[Number(part)];
      continue;
    }
    if (typeof current === "object") {
      current = (current as Record<string, unknown>)[part];
      continue;
    }
    return undefined;
  }
  return current;
}

function normalizeExpectedStatus(
  expected: number | number[] | undefined,
): number[] {
  if (expected === undefined) {
    return [200];
  }
  return Array.isArray(expected) ? expected : [expected];
}

function defaultCookiePath(pathname: string): string {
  if (!pathname || !pathname.startsWith("/")) {
    return "/";
  }
  if (pathname === "/") {
    return "/";
  }
  if (pathname.endsWith("/")) {
    return pathname;
  }
  const idx = pathname.lastIndexOf("/");
  if (idx <= 0) {
    return "/";
  }
  return pathname.slice(0, idx + 1);
}

function domainMatches(host: string, cookieDomain: string): boolean {
  const h = host.toLowerCase();
  const d = cookieDomain.toLowerCase();
  return h === d || h.endsWith(`.${d}`);
}

function pathMatches(pathname: string, cookiePath: string): boolean {
  if (cookiePath === "/") {
    return true;
  }
  return pathname.startsWith(cookiePath);
}

const HTTP_DATE_MONTHS: Record<string, number> = {
  jan: 1, feb: 2, mar: 3, apr: 4, may: 5, jun: 6,
  jul: 7, aug: 8, sep: 9, oct: 10, nov: 11, dec: 12,
};

/** Parse an HTTP date (RFC 1123) or ISO 8601 string to epoch milliseconds. */
function parseDateToMs(value: string): number | undefined {
  // Try ISO 8601 first (Temporal native)
  try {
    return Temporal.Instant.from(value).epochMilliseconds;
  } catch { /* not ISO 8601 */ }

  // Try RFC 1123: "Thu, 01 Dec 2025 00:00:00 GMT"
  const m = value.match(
    /(\d{1,2})\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+(\d{4})\s+(\d{2}):(\d{2}):(\d{2})\s+GMT/i,
  );
  if (m) {
    const month = HTTP_DATE_MONTHS[m[2].toLowerCase()];
    if (month !== undefined) {
      const pad = (n: number) => String(n).padStart(2, "0");
      const iso = `${m[3]}-${pad(month)}-${pad(Number(m[1]))}T${m[4]}:${m[5]}:${m[6]}Z`;
      try {
        return Temporal.Instant.from(iso).epochMilliseconds;
      } catch { /* malformed */ }
    }
  }
  return undefined;
}

function parseSetCookie(
  headerValue: string,
  requestUrl: URL,
): CookieEntry | null {
  const parts = headerValue.split(";").map((p) => p.trim());
  if (parts.length === 0) {
    return null;
  }

  const [nameValue, ...attrs] = parts;
  const eqIdx = nameValue.indexOf("=");
  if (eqIdx <= 0) {
    return null;
  }

  const name = nameValue.slice(0, eqIdx).trim();
  const value = nameValue.slice(eqIdx + 1).trim();
  if (!name) {
    return null;
  }

  const cookie: CookieEntry = {
    name,
    value,
    domain: requestUrl.hostname.toLowerCase(),
    path: defaultCookiePath(requestUrl.pathname),
    secure: false,
  };

  for (const attr of attrs) {
    const idx = attr.indexOf("=");
    const key = (idx >= 0 ? attr.slice(0, idx) : attr).trim().toLowerCase();
    const rawVal = idx >= 0 ? attr.slice(idx + 1).trim() : "";

    switch (key) {
      case "domain": {
        if (rawVal) {
          cookie.domain = rawVal.replace(/^\./, "").toLowerCase();
        }
        break;
      }
      case "path": {
        if (rawVal.startsWith("/")) {
          cookie.path = rawVal;
        }
        break;
      }
      case "secure": {
        cookie.secure = true;
        break;
      }
      case "max-age": {
        const secs = Number(rawVal);
        if (Number.isFinite(secs)) {
          cookie.expiresAtMs = Temporal.Now.instant().epochMilliseconds + secs * 1000;
        }
        break;
      }
      case "expires": {
        const ts = parseDateToMs(rawVal);
        if (ts !== undefined) {
          cookie.expiresAtMs = ts;
        }
        break;
      }
      default:
        break;
    }
  }

  return cookie;
}

function getSetCookieValues(headers: Headers): string[] {
  const ext = headers as Headers & { getSetCookie?: () => string[] };
  if (typeof ext.getSetCookie === "function") {
    return ext.getSetCookie();
  }
  const single = headers.get("set-cookie");
  return single ? [single] : [];
}

class CookieJar {
  private entries: CookieEntry[] = [];

  private removeExpired(now: number): void {
    this.entries = this.entries.filter((c) =>
      c.expiresAtMs === undefined || c.expiresAtMs > now
    );
  }

  storeFromResponse(url: URL, headers: Headers): void {
    const now = Temporal.Now.instant().epochMilliseconds;
    this.removeExpired(now);

    for (const raw of getSetCookieValues(headers)) {
      const parsed = parseSetCookie(raw, url);
      if (!parsed) {
        continue;
      }

      if (parsed.expiresAtMs !== undefined && parsed.expiresAtMs <= now) {
        this.entries = this.entries.filter(
          (c) =>
            !(c.name === parsed.name && c.domain === parsed.domain &&
              c.path === parsed.path),
        );
        continue;
      }

      const existingIdx = this.entries.findIndex(
        (c) =>
          c.name === parsed.name && c.domain === parsed.domain &&
          c.path === parsed.path,
      );
      if (existingIdx >= 0) {
        this.entries[existingIdx] = parsed;
      } else {
        this.entries.push(parsed);
      }
    }
  }

  cookieHeaderFor(url: URL): string | null {
    const now = Temporal.Now.instant().epochMilliseconds;
    this.removeExpired(now);

    const cookies = this.entries
      .filter((c) => domainMatches(url.hostname, c.domain))
      .filter((c) => pathMatches(url.pathname, c.path))
      .filter((c) => !c.secure || url.protocol === "https:")
      .sort((a, b) => b.path.length - a.path.length)
      .map((c) => `${c.name}=${c.value}`);

    if (cookies.length === 0) {
      return null;
    }
    return cookies.join("; ");
  }

  applyToHeaders(url: URL, headers: Record<string, string>): void {
    const jarCookie = this.cookieHeaderFor(url);
    if (!jarCookie) {
      return;
    }

    const existing = Object.entries(headers).find(([k]) =>
      k.toLowerCase() === "cookie"
    );
    if (existing) {
      headers[existing[0]] = `${existing[1]}; ${jarCookie}`;
    } else {
      headers["cookie"] = jarCookie;
    }
  }
}

function initStepMaps(steps: ScriptStep[]): {
  stats: ScriptStepStats;
  errors: ScriptStepErrorMap;
} {
  const stats: ScriptStepStats = {};
  const errors: ScriptStepErrorMap = {};
  for (const step of steps) {
    stats[step.name] = 0;
    errors[step.name] = 0;
  }
  return { stats, errors };
}

function ensureCheckCounter(
  summary: ScriptCheckSummary,
  key: string,
): ScriptCheckCounter {
  if (!summary[key]) {
    summary[key] = { total: 0, passed: 0, failed: 0 };
  }
  return summary[key];
}

function recordCheck(
  summary: ScriptCheckSummary,
  key: string,
  ok: boolean,
): void {
  const counter = ensureCheckCounter(summary, key);
  counter.total += 1;
  if (ok) {
    counter.passed += 1;
  } else {
    counter.failed += 1;
  }
}

function assertConfig(config: ScriptScenarioConfig): void {
  if (config.vus < 1) {
    throw new Error("script scenario: vus must be >= 1");
  }
  if (!(config.duration_s > 0)) {
    throw new Error("script scenario: duration_s must be > 0");
  }
  if (config.steps.length === 0) {
    throw new Error("script scenario: steps must not be empty");
  }
  for (const step of config.steps) {
    if (!step.name || step.name.trim().length === 0) {
      throw new Error("script scenario: each step needs a non-empty name");
    }
    if (!step.url || step.url.trim().length === 0) {
      throw new Error(`script scenario: step '${step.name}' needs a url`);
    }
  }
}

function stepNeedsBodyCapture(step: ScriptStep): boolean {
  const checks = step.checks;

  const extractorNeedsBody = (step.extract ?? []).some((extractor) =>
    extractor.type === "json" || extractor.type === "regex" || extractor.type === "dom"
  );

  if (extractorNeedsBody) {
    return true;
  }
  if ((checks?.body_includes?.length ?? 0) > 0) {
    return true;
  }
  if ((checks?.json_path_exists?.length ?? 0) > 0) {
    return true;
  }
  if (Object.keys(checks?.json_path_equals ?? {}).length > 0) {
    return true;
  }
  if ((checks?.regex_match?.length ?? 0) > 0) {
    return true;
  }

  return false;
}

function getHeaderFromRecord(
  headers: Record<string, string>,
  name: string,
): string | null {
  const target = name.toLowerCase();
  for (const [k, v] of Object.entries(headers)) {
    if (k.toLowerCase() === target) {
      return v;
    }
  }
  return null;
}

function resolveExtractorValue(
  extractor: ScriptExtractor,
  context: StepEvaluationContext,
  state: ScriptState,
  parseJson: () => unknown,
  parseDom: () => ReturnType<DOMParser["parseFromString"]>,
): string {
  switch (extractor.type) {
    case "header": {
      const headerName = renderTemplate(extractor.name, state);
      const value = context.getHeader(headerName);
      if (!value) {
        throw new Error(
          `header extractor '${extractor.as}' missing: ${headerName}`,
        );
      }
      return value;
    }

    case "regex": {
      const pattern = renderTemplate(extractor.pattern, state);
      const flags = extractor.flags ?? "";
      const re = new RegExp(pattern, flags);
      const match = context.bodyText.match(re);
      const group = extractor.group ?? 1;
      const value = match?.[group];
      if (!value) {
        throw new Error(
          `regex extractor '${extractor.as}' found no group ${group}`,
        );
      }
      return value;
    }

    case "json": {
      const jsonData = parseJson();
      const raw = readJsonPath(jsonData, extractor.path);
      if (raw === undefined || raw === null) {
        throw new Error(
          `json extractor '${extractor.as}' path not found: ${extractor.path}`,
        );
      }
      return String(raw);
    }

    case "dom": {
      const selector = renderTemplate(extractor.selector, state);
      const doc = parseDom();
      const el = doc.querySelector(selector);
      if (!el) {
        throw new Error(
          `dom extractor '${extractor.as}' no element matches: ${selector}`,
        );
      }
      if (extractor.attribute) {
        const attr = el.getAttribute(extractor.attribute);
        if (attr === null) {
          throw new Error(
            `dom extractor '${extractor.as}' attribute '${extractor.attribute}' not found on: ${selector}`,
          );
        }
        return attr;
      }
      const text = el.textContent?.trim() ?? "";
      if (!text) {
        throw new Error(
          `dom extractor '${extractor.as}' empty textContent on: ${selector}`,
        );
      }
      return text;
    }
  }
}

function evaluateStepResult(
  step: ScriptStep,
  state: ScriptState,
  context: StepEvaluationContext,
  checkSummary: ScriptCheckSummary,
): StepExecutionResult {
  const responseText = context.bodyText;
  let parsedJson: unknown | undefined;
  let parsedJsonDone = false;
  const parseJson = () => {
    if (!parsedJsonDone) {
      parsedJsonDone = true;
      parsedJson = JSON.parse(responseText);
    }
    return parsedJson;
  };

  let parsedDom: ReturnType<DOMParser["parseFromString"]> | undefined;
  let parsedDomDone = false;
  const parseDom = () => {
    if (!parsedDomDone) {
      parsedDomDone = true;
      parsedDom = new DOMParser().parseFromString(responseText, "text/html");
    }
    return parsedDom!;
  };

  if (step.extract && step.extract.length > 0) {
    for (const extractor of step.extract) {
      const value = resolveExtractorValue(extractor, context, state, parseJson, parseDom);
      state[extractor.as] = value;
    }
  }

  let checksTotal = 0;
  let checksPassed = 0;
  let checksFailed = 0;
  const failures: string[] = [];

  const registerCheck = (key: string, ok: boolean, message: string) => {
    checksTotal += 1;
    if (ok) {
      checksPassed += 1;
    } else {
      checksFailed += 1;
      failures.push(message);
    }
    recordCheck(checkSummary, key, ok);
  };

  const checks = step.checks;
  const expectedStatuses = checks?.status_in ??
    normalizeExpectedStatus(step.expected_status);
  registerCheck(
    "status",
    expectedStatuses.includes(context.status),
    `step '${step.name}' status mismatch: got ${context.status}, expected one of [${
      expectedStatuses.join(", ")
    }]. body=${responseText.slice(0, 200)}`,
  );

  for (const pattern of checks?.body_includes ?? []) {
    const renderedPattern = renderTemplate(pattern, state);
    registerCheck(
      "body_includes",
      responseText.includes(renderedPattern),
      `step '${step.name}' body does not include '${renderedPattern}'`,
    );
  }

  for (const headerNameRaw of checks?.header_exists ?? []) {
    const headerName = renderTemplate(headerNameRaw, state);
    registerCheck(
      "header_exists",
      context.getHeader(headerName) !== null,
      `step '${step.name}' missing header '${headerName}'`,
    );
  }

  for (
    const [headerNameRaw, expectedRaw] of Object.entries(
      checks?.header_equals ?? {},
    )
  ) {
    const headerName = renderTemplate(headerNameRaw, state);
    const expectedValue = renderTemplate(expectedRaw, state);
    const actual = context.getHeader(headerName);
    registerCheck(
      "header_equals",
      actual === expectedValue,
      `step '${step.name}' header '${headerName}' mismatch: got '${actual}', expected '${expectedValue}'`,
    );
  }

  for (
    const [headerNameRaw, substringRaw] of Object.entries(
      checks?.header_includes ?? {},
    )
  ) {
    const headerName = renderTemplate(headerNameRaw, state);
    const substring = renderTemplate(substringRaw, state);
    const actual = context.getHeader(headerName);
    registerCheck(
      "header_includes",
      actual !== null && actual.includes(substring),
      `step '${step.name}' header '${headerName}' does not include '${substring}' (got '${actual}')`,
    );
  }

  for (const pathRaw of checks?.json_path_exists ?? []) {
    const path = renderTemplate(pathRaw, state);
    let exists = false;
    try {
      const val = readJsonPath(parseJson(), path);
      exists = val !== undefined && val !== null;
    } catch {
      exists = false;
    }
    registerCheck(
      "json_path_exists",
      exists,
      `step '${step.name}' json path missing: '${path}'`,
    );
  }

  for (
    const [pathRaw, expectedValueRaw] of Object.entries(
      checks?.json_path_equals ?? {},
    )
  ) {
    const path = renderTemplate(pathRaw, state);
    const expectedValue = typeof expectedValueRaw === "string"
      ? renderTemplate(expectedValueRaw, state)
      : expectedValueRaw;

    let actualValue: unknown = undefined;
    let ok = false;
    try {
      actualValue = readJsonPath(parseJson(), path);
      ok = actualValue === expectedValue;
    } catch {
      ok = false;
    }

    registerCheck(
      "json_path_equals",
      ok,
      `step '${step.name}' json path '${path}' mismatch: got '${
        String(actualValue)
      }', expected '${String(expectedValue)}'`,
    );
  }

  for (const regexPatternRaw of checks?.regex_match ?? []) {
    const regexPattern = renderTemplate(regexPatternRaw, state);
    const re = new RegExp(regexPattern);
    registerCheck(
      "regex_match",
      re.test(responseText),
      `step '${step.name}' regex did not match: /${regexPattern}/`,
    );
  }

  if (failures.length > 0) {
    throw new Error(failures.join(" | "));
  }

  return { checksTotal, checksPassed, checksFailed };
}

async function executeStepFetch(
  step: ScriptStep,
  state: ScriptState,
  requestTimeoutMs: number,
  cookieJar: CookieJar,
  scenarioUseCookies: boolean,
  scenarioRedirectPolicy: ScriptRedirectPolicy,
  checkSummary: ScriptCheckSummary,
): Promise<StepExecutionResult> {
  const renderedUrl = renderTemplate(step.url, state);
  const urlObj = new URL(renderedUrl);
  const method = step.method ?? (step.body ? "POST" : "GET");
  const headers = resolveHeaders(step.headers, state);
  const body = step.body === undefined
    ? undefined
    : renderTemplate(step.body, state);
  const useCookies = step.use_cookies ?? scenarioUseCookies;
  const redirectPolicy = step.redirect_policy ?? scenarioRedirectPolicy;

  if (useCookies) {
    cookieJar.applyToHeaders(urlObj, headers);
  }

  const signal = AbortSignal.timeout(requestTimeoutMs);
  const response = await fetch(renderedUrl, {
    method,
    headers,
    body,
    redirect: redirectPolicy,
    signal,
  });

  if (useCookies) {
    cookieJar.storeFromResponse(urlObj, response.headers);
  }

  const responseText = await response.text();
  return evaluateStepResult(
    step,
    state,
    {
      status: response.status,
      bodyText: responseText,
      getHeader: (name: string) => response.headers.get(name),
    },
    checkSummary,
  );
}

function extractNativeErrorMessage(response: StepResponse): string {
  if (response.error) {
    return `${response.error.code}: ${response.error.message}`;
  }
  return "unknown native step error";
}

async function executeStepFfi(
  step: ScriptStep,
  state: ScriptState,
  stepSession: LoadgenStepFFI,
  scenarioUseCookies: boolean,
  scenarioRedirectPolicy: ScriptRedirectPolicy,
  checkSummary: ScriptCheckSummary,
): Promise<StepExecutionResult> {
  const renderedUrl = renderTemplate(step.url, state);
  const method = step.method ?? (step.body ? "POST" : "GET");
  const headers = resolveHeaders(step.headers, state);
  const body = step.body === undefined
    ? undefined
    : renderTemplate(step.body, state);
  const useCookies = step.use_cookies ?? scenarioUseCookies;
  const redirectPolicy = step.redirect_policy ?? scenarioRedirectPolicy;
  const captureBody = stepNeedsBodyCapture(step);

  const response = await stepSession.execute({
    name: step.name,
    method,
    url: renderedUrl,
    headers,
    body,
    redirect_policy: redirectPolicy,
    capture_body: captureBody,
    use_cookies: useCookies,
  });

  if (!response.ok) {
    throw new Error(
      `native step failed: ${extractNativeErrorMessage(response)}`,
    );
  }
  if (response.status === null) {
    throw new Error(`native step returned null status for '${step.name}'`);
  }

  return evaluateStepResult(
    step,
    state,
    {
      status: response.status,
      bodyText: response.body ?? "",
      getHeader: (name: string) => getHeaderFromRecord(response.headers, name),
    },
    checkSummary,
  );
}

export async function runScriptScenario(
  config: ScriptScenarioConfig,
  libraryPath?: string,
): Promise<ScriptRunResult> {
  assertConfig(config);

  const requestTimeoutMs = Math.max(
    1,
    Math.floor((config.request_timeout_s ?? 30) * 1000),
  );
  const continueOnError = config.continue_on_error ?? false;
  const useCookies = config.use_cookies ?? true;
  const redirectPolicy = config.redirect_policy ?? "follow";
  const executionMode = config.execution_mode ?? "fetch";
  const stepSessionConfig = {
    request_timeout_s: config.request_timeout_s ?? 30,
    cookie_jar: useCookies,
    redirect_policy: redirectPolicy,
    response_headers: true,
    ...(config.step_session_config ?? {}),
  };

  let iterations = 0;
  let stepsExecuted = 0;
  let errorsTotal = 0;
  let checksTotal = 0;
  let checksPassed = 0;
  let checksFailed = 0;

  const { stats: stepStats, errors: stepErrors } = initStepMaps(config.steps);
  const checkSummary: ScriptCheckSummary = {};

  const startedAt = Temporal.Now.instant();
  const startedPerf = performance.now();
  const stopAt = startedPerf + config.duration_s * 1000;

  async function runVu(_vu: number): Promise<void> {
    const cookieJar = executionMode === "fetch" ? new CookieJar() : null;
    const stepSession = executionMode === "ffi-step"
      ? new LoadgenStepFFI(stepSessionConfig, libraryPath)
      : null;

    try {
      while (performance.now() < stopAt) {
        iterations += 1;
        const state: ScriptState = {};

        for (const step of config.steps) {
          stepsExecuted += 1;
          stepStats[step.name] = (stepStats[step.name] ?? 0) + 1;

          try {
            const stepChecks = executionMode === "ffi-step"
              ? await executeStepFfi(
                step,
                state,
                stepSession as LoadgenStepFFI,
                useCookies,
                redirectPolicy,
                checkSummary,
              )
              : await executeStepFetch(
                step,
                state,
                requestTimeoutMs,
                cookieJar as CookieJar,
                useCookies,
                redirectPolicy,
                checkSummary,
              );

            checksTotal += stepChecks.checksTotal;
            checksPassed += stepChecks.checksPassed;
            checksFailed += stepChecks.checksFailed;
          } catch (error) {
            errorsTotal += 1;
            checksTotal += 1;
            checksFailed += 1;
            recordCheck(checkSummary, "step_error", false);
            stepErrors[step.name] = (stepErrors[step.name] ?? 0) + 1;

            const msg = error instanceof Error ? error.message : String(error);
            if (continueOnError) {
              console.error(`step '${step.name}' failed: ${msg}`);
            }
            break;
          }
        }
      }
    } finally {
      stepSession?.close();
    }
  }

  await Promise.all(
    Array.from({ length: config.vus }, (_v, i) => runVu(i + 1)),
  );

  const finishedAt = Temporal.Now.instant();
  const elapsedS = (performance.now() - startedPerf) / 1000;
  const stepRate = elapsedS > 0 ? stepsExecuted / elapsedS : 0;
  const iterationRate = elapsedS > 0 ? iterations / elapsedS : 0;

  return {
    vus: config.vus,
    duration_target_s: config.duration_s,
    elapsed_s: elapsedS,
    started_at: startedAt.toString(),
    finished_at: finishedAt.toString(),
    iterations,
    steps_executed: stepsExecuted,
    iteration_rate: iterationRate,
    step_rate: stepRate,
    checks_total: checksTotal,
    checks_passed: checksPassed,
    checks_failed: checksFailed,
    errors_total: errorsTotal,
    step_stats: stepStats,
    step_errors: stepErrors,
    check_summary: checkSummary,
  };
}
