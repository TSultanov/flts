/**
 * Mock implementation of @tauri-apps/plugin-os for Playwright tests.
 *
 * `platform()` reads from `window.__mockPlatform` on every call so tests can
 * set the value via `page.addInitScript(() => { window.__mockPlatform = 'linux' })`
 * before any app code (including App.svelte's module-level
 * `try { isMac = platform() === 'macos' }`) runs. Defaults to `'macos'`.
 */

export function platform(): string {
  if (typeof window === 'undefined') return 'macos';
  return ((window as any).__mockPlatform as string | undefined) ?? 'macos';
}
