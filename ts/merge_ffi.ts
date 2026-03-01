import type { RunReport } from "./types.ts";

const MERGE_SYMBOLS = {
  loadgen_merge_reports: { parameters: ["pointer"], result: "pointer" },
  loadgen_last_error: { parameters: [], result: "pointer" },
  loadgen_free_string: { parameters: ["pointer"], result: "void" },
} as const;

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

/**
 * Merge multiple RunReports (with histogram b64 fields) into a single
 * statistically correct merged report via the Rust FFI.
 */
export function mergeReports(
  reports: RunReport[],
  libraryPath = DEFAULT_LIBRARY_PATH,
): RunReport {
  const lib = Deno.dlopen(libraryPath, MERGE_SYMBOLS);
  try {
    const jsonBytes = toCStringBytes(JSON.stringify(reports));
    const jsonPtr = Deno.UnsafePointer.of(jsonBytes);
    if (jsonPtr === null) {
      throw new Error("failed to get pointer for reports JSON");
    }

    const resultPtr = lib.symbols.loadgen_merge_reports(jsonPtr);
    if (resultPtr === null) {
      const errPtr = lib.symbols.loadgen_last_error();
      const errMsg = errPtr !== null ? readCString(errPtr) : "unknown error";
      if (errPtr !== null) lib.symbols.loadgen_free_string(errPtr);
      throw new Error(`loadgen_merge_reports failed: ${errMsg}`);
    }

    const json = readCString(resultPtr);
    lib.symbols.loadgen_free_string(resultPtr);
    return JSON.parse(json) as RunReport;
  } finally {
    lib.close();
  }
}
