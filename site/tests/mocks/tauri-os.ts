/**
 * Stand-in for @tauri-apps/plugin-os. `platform()` re-reads
 * `window.__mockPlatform` on every call so `page.addInitScript` can set it
 * before module-level app code runs. Defaults to `'macos'`.
 */

export function platform(): string {
  if (typeof window === "undefined") return "macos";
  return ((window as any).__mockPlatform as string | undefined) ?? "macos";
}
