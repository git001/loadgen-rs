import { runScriptScenario } from "../ts/mod.ts";

const loginUrl = "https://quickpizza.grafana.com/api/users/token/login";
const vus = Number(Deno.env.get("VUS") ?? "2");
const durationS = Number(Deno.env.get("DURATION_S") ?? "5");
const execModeEnv = (Deno.env.get("EXEC_MODE") ?? "fetch").toLowerCase();
const executionMode = execModeEnv === "ffi-step" ? "ffi-step" : "fetch";

const payload = JSON.stringify({
  username: "default",
  password: "12345678",
});

const result = await runScriptScenario({
  vus,
  duration_s: durationS,
  request_timeout_s: 10,
  continue_on_error: false,
  use_cookies: true,
  redirect_policy: "follow",
  execution_mode: executionMode,
  step_session_config: executionMode === "ffi-step"
    ? {
      protocol: "h2",
      request_timeout_s: 10,
      cookie_jar: true,
      redirect_policy: "follow",
      response_headers: true,
      response_body_limit: 64 * 1024,
    }
    : undefined,
  steps: [
    {
      name: "login",
      method: "POST",
      url: loginUrl,
      headers: {
        "content-type": "application/json",
      },
      body: payload,
      expected_status: 200,
      extract: [
        {
          type: "json",
          path: "token",
          as: "token",
        },
      ],
      checks: {
        body_includes: ['"token":"'],
        header_exists: ["content-type"],
        json_path_exists: ["token"],
      },
    },
    {
      name: "followup_with_token",
      method: "POST",
      url: `${loginUrl}?source=deno-corr&token={{token}}`,
      headers: {
        "content-type": "application/json",
        authorization: "Bearer {{token}}",
      },
      body: payload,
      expected_status: 200,
      extract: [
        {
          type: "regex",
          pattern: '\\"token\\":\\"([^\\"]+)\\"',
          as: "token_2",
        },
      ],
      checks: {
        json_path_exists: ["token"],
        regex_match: ['"token":"[a-zA-Z0-9]+"'],
      },
    },
  ],
});

console.log(JSON.stringify(result, null, 2));
