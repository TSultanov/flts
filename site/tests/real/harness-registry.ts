// Bridge between the worker `harness` fixture and helpers that only receive a
// `page`. One worker process runs one harness at a time, so a module-level
// slot is unambiguous.
import type { RealHarness } from "./fixtures";

let current: RealHarness | undefined;

export function setHarness(h: RealHarness | undefined): void {
  current = h;
}

export function getHarness(): RealHarness {
  if (!current) {
    throw new Error(
      "real harness unavailable: the spec must import { test } from helpers/test",
    );
  }
  return current;
}
