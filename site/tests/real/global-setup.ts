import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);

const BUILD_CMD = "cargo build -p app -p e2e-sims --features app/e2e-bridge";

const BINARIES = ["target/debug/app", "target/debug/flts-e2e-sims"];

export default function globalSetup(): void {
  if (process.env.FLTS_E2E_SKIP_BUILD === "1") {
    // Fail here rather than as a spawn ENOENT inside every worker fixture.
    const missing = BINARIES.filter(
      (b) => !fs.existsSync(path.join(repoRoot, b)),
    );
    if (missing.length) {
      throw new Error(
        `FLTS_E2E_SKIP_BUILD=1 but missing: ${missing.join(", ")}. Run \`${BUILD_CMD}\` from ${repoRoot}, or unset FLTS_E2E_SKIP_BUILD.`,
      );
    }
    return;
  }
  // Per-package features: `--features app/e2e-bridge` is the only form that is
  // unambiguous when several -p packages are selected.
  const args = [
    "build",
    "-p",
    "app",
    "-p",
    "e2e-sims",
    "--features",
    "app/e2e-bridge",
  ];
  const res = spawnSync("cargo", args, {
    cwd: repoRoot,
    stdio: "inherit",
    env: process.env,
  });
  if (res.error) throw res.error;
  if (res.status !== 0) {
    throw new Error(
      `cargo ${args.join(" ")} failed (exit ${res.status}). Fix the build, or set FLTS_E2E_SKIP_BUILD=1 to reuse target/debug binaries.`,
    );
  }
}
