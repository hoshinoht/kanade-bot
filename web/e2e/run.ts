import { spawnSync } from 'node:child_process';

// `bun run e2e [playwright args]`: the whole browser suite while it is split
// between two servers. The mock config runs first, then the harness config
// (`harness/` plus the specs in `ported.ts`), each with the same arguments, and
// the run fails if either does. Both always run, so one failure never hides
// the other's. `--pass-with-no-tests`: a filter (`--project`, a file name) may
// select nothing on one side. An `--output` dir gets a `harness/` child for
// the second run, which would otherwise clear the first run's traces.
const args = process.argv.slice(2);
const harnessArgs = args.map((arg, i) => (args[i - 1] === '--output' || arg.startsWith('--output=') ? `${arg}/harness` : arg));

let failed = false;
for (const [config, rest] of [
  ['playwright.config.ts', args],
  ['playwright.harness.config.ts', harnessArgs],
] as const) {
  const run = spawnSync('bunx', ['playwright', 'test', '-c', config, '--pass-with-no-tests', ...rest], { stdio: 'inherit' });
  if (run.status !== 0) {
    console.error(`e2e: ${config} failed (exit ${run.status ?? run.signal})`);
    failed = true;
  }
}
process.exit(failed ? 1 : 0);
