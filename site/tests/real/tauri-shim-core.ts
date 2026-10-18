// Real-backend stand-in for @tauri-apps/api/core: every invoke goes over the
// WS bridge. Export surface mirrors tests/mocks/tauri-api.ts.
import { bridgeInvoke } from "./bridge-transport";

export type InvokeArgs = Record<string, unknown>;

export type PermissionState =
  "granted" | "denied" | "prompt" | "prompt-with-rationale";

// The barcode-scanner plugin imports these from the core package even in the
// real tier; camera scanning has no headless equivalent.
export async function checkPermissions<T>(_plugin: string): Promise<T> {
  return { camera: "granted" } as unknown as T;
}

export async function requestPermissions<T>(_plugin: string): Promise<T> {
  return { camera: "granted" } as unknown as T;
}

export function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  return bridgeInvoke<T>(cmd, args ?? {});
}

export { invoke as default };
