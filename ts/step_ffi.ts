import type { StepRequest, StepResponse, StepSessionConfig } from "./types.ts";

const SYMBOLS = {
  loadgen_step_abi_version: { parameters: [], result: "u32" },
  loadgen_step_session_create: { parameters: ["pointer"], result: "pointer" },
  loadgen_step_execute: { parameters: ["pointer", "pointer"], result: "pointer", nonblocking: true },
  loadgen_step_snapshot: { parameters: ["pointer"], result: "pointer" },
  loadgen_step_session_reset: { parameters: ["pointer"], result: "void" },
  loadgen_step_session_destroy: { parameters: ["pointer"], result: "void" },
  loadgen_last_error: { parameters: [], result: "pointer" },
  loadgen_free_string: { parameters: ["pointer"], result: "void" },
} as const;

type StepLibrary = Deno.DynamicLibrary<typeof SYMBOLS>;

const DEFAULT_LIBRARY_PATH = "./target/release/libloadgen_ffi.so";

function toCStringBytes(value: string): Uint8Array<ArrayBuffer> {
  const body = new TextEncoder().encode(value);
  const out = new Uint8Array(new ArrayBuffer(body.length + 1));
  out.set(body, 0);
  out[body.length] = 0;
  return out;
}

function readCString(ptr: Deno.PointerValue): string {
  if (ptr === null) {
    throw new Error("null string pointer");
  }
  return new Deno.UnsafePointerView(ptr).getCString();
}

export class LoadgenStepFFI {
  private lib: StepLibrary;
  private sessionHandle: Deno.PointerValue | null;

  constructor(sessionConfig: StepSessionConfig, libraryPath = DEFAULT_LIBRARY_PATH) {
    this.lib = Deno.dlopen(libraryPath, SYMBOLS);

    const configBytes = toCStringBytes(JSON.stringify(sessionConfig));
    const configPtr = Deno.UnsafePointer.of(configBytes);
    if (configPtr === null) {
      throw new Error("failed to get pointer for step session config");
    }

    this.sessionHandle = this.lib.symbols.loadgen_step_session_create(configPtr);
    if (this.sessionHandle === null) {
      throw new Error(this.lastError() ?? "loadgen_step_session_create failed");
    }
  }

  abiVersion(): number {
    return this.lib.symbols.loadgen_step_abi_version();
  }

  async execute(stepRequest: StepRequest): Promise<StepResponse> {
    const session = this.requireSessionHandle();
    const reqBytes = toCStringBytes(JSON.stringify(stepRequest));
    const reqPtr = Deno.UnsafePointer.of(reqBytes);
    if (reqPtr === null) {
      throw new Error("failed to get pointer for step request");
    }

    const responsePtr = await this.lib.symbols.loadgen_step_execute(session, reqPtr);
    if (responsePtr === null) {
      throw new Error(this.lastError() ?? "loadgen_step_execute failed");
    }
    const json = readCString(responsePtr);
    this.lib.symbols.loadgen_free_string(responsePtr);
    return JSON.parse(json) as StepResponse;
  }

  snapshot(): Record<string, unknown> {
    const session = this.requireSessionHandle();
    const ptr = this.lib.symbols.loadgen_step_snapshot(session);
    if (ptr === null) {
      throw new Error(this.lastError() ?? "loadgen_step_snapshot failed");
    }
    const json = readCString(ptr);
    this.lib.symbols.loadgen_free_string(ptr);
    return JSON.parse(json) as Record<string, unknown>;
  }

  reset(): void {
    const session = this.requireSessionHandle();
    this.lib.symbols.loadgen_step_session_reset(session);
  }

  close(): void {
    if (this.sessionHandle !== null) {
      this.lib.symbols.loadgen_step_session_destroy(this.sessionHandle);
      this.sessionHandle = null;
    }
    this.lib.close();
  }

  private lastError(): string | null {
    const ptr = this.lib.symbols.loadgen_last_error();
    if (ptr === null) {
      return null;
    }
    const msg = readCString(ptr);
    this.lib.symbols.loadgen_free_string(ptr);
    return msg;
  }

  private requireSessionHandle(): Deno.PointerValue {
    if (this.sessionHandle === null) {
      throw new Error("step session is closed");
    }
    return this.sessionHandle;
  }
}
