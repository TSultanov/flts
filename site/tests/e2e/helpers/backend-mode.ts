/** True under playwright.real.config.ts (which pins the env var). */
export function isRealMode(): boolean {
  return !!process.env.PLAYWRIGHT_REAL;
}

/** Uniform rejection for mock-only helpers reached from a real-tier spec. */
export function realModeUnsupported(what: string): never {
  const err = new Error(`not supported in real mode: ${what}`);
  err.name = 'RealModeUnsupported';
  throw err;
}
