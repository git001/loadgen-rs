import { BenchConfig, RunReport } from "./types.ts";

const SYMBOLS = {
  loadgen_abi_version: { parameters: [], result: "u32" },
  loadgen_create: { parameters: ["pointer"], result: "pointer" },
  loadgen_run: { parameters: ["pointer"], result: "pointer", nonblocking: true },
  loadgen_metrics_snapshot: { parameters: ["pointer"], result: "pointer" },
  loadgen_last_error: { parameters: [], result: "pointer" },
  loadgen_free_string: { parameters: ["pointer"], result: "void" },
  loadgen_destroy: { parameters: ["pointer"], result: "void" },
} as const;

type LoadgenLibrary = Deno.DynamicLibrary<typeof SYMBOLS>;

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

export class LoadgenFFI {
  private lib: LoadgenLibrary;
  private handle: Deno.PointerValue | null;

  constructor(config: BenchConfig, libraryPath = DEFAULT_LIBRARY_PATH) {
    this.lib = Deno.dlopen(libraryPath, SYMBOLS);

    const configBytes = toCStringBytes(JSON.stringify(config));
    const configPtr = Deno.UnsafePointer.of(configBytes);
    if (configPtr === null) {
      throw new Error("failed to get pointer for config bytes");
    }

    this.handle = this.lib.symbols.loadgen_create(configPtr);
    if (this.handle === null) {
      const error = this.lastError();
      throw new Error(error ?? "loadgen_create failed");
    }
  }

  abiVersion(): number {
    return this.lib.symbols.loadgen_abi_version();
  }

  async run(): Promise<RunReport> {
    const handle = this.requireHandle();
    const resultPtr = await this.lib.symbols.loadgen_run(handle);
    if (resultPtr === null) {
      throw new Error(this.lastError() ?? "loadgen_run failed");
    }
    const json = readCString(resultPtr);
    this.lib.symbols.loadgen_free_string(resultPtr);
    return JSON.parse(json) as RunReport;
  }

  snapshot(): Record<string, unknown> {
    const handle = this.requireHandle();
    const snapshotPtr = this.lib.symbols.loadgen_metrics_snapshot(handle);
    if (snapshotPtr === null) {
      throw new Error(this.lastError() ?? "loadgen_metrics_snapshot failed");
    }
    const json = readCString(snapshotPtr);
    this.lib.symbols.loadgen_free_string(snapshotPtr);
    return JSON.parse(json) as Record<string, unknown>;
  }

  close(): void {
    if (this.handle !== null) {
      this.lib.symbols.loadgen_destroy(this.handle);
      this.handle = null;
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

  private requireHandle(): Deno.PointerValue {
    if (this.handle === null) {
      throw new Error("loadgen handle is closed");
    }
    return this.handle;
  }
}
