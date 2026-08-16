import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../../..',
);

export default function globalSetup(): void {
  if (process.env.FLTS_E2E_SKIP_BUILD === '1') return;
  // Per-package features: `--features app/e2e-bridge` is the only form that is
  // unambiguous when several -p packages are selected.
  const args = [
    'build',
    '-p',
    'app',
    '-p',
    'e2e-sims',
    '--features',
    'app/e2e-bridge',
  ];
  const res = spawnSync('cargo', args, {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (res.error) throw res.error;
  if (res.status !== 0) {
    throw new Error(
      `cargo ${args.join(' ')} failed (exit ${res.status}). Fix the build, or set FLTS_E2E_SKIP_BUILD=1 to reuse target/debug binaries.`,
    );
  }
}
