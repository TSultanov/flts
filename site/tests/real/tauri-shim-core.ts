// Real-backend stand-in for @tauri-apps/api/core: every invoke goes over the
// WS bridge. Export surface mirrors tests/mocks/tauri-api.ts.
import { bridgeInvoke } from "./bridge-transport";

export type InvokeArgs = Record<string, unknown>;

export function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  return bridgeInvoke<T>(cmd, args ?? {});
}

export { invoke as default };
