// Layout fidelity against the M3E boards: bun run fidelity [pair ...] [--keep]
// Builds with KANADE_FIDELITY=1 (keeps the data-fid tags), runs
// e2e/fidelity.spec.ts, then rebuilds clean so no tagged dist is left behind
// (e2e/bundle.spec.ts fails on one). --keep skips the clean rebuild while
// iterating; run once without it, or `bun run build`, before anything else.
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';

const cwd = join(import.meta.dir, '..');
const args = process.argv.slice(2);
const keep = args.includes('--keep');
const only = args.filter((a) => a !== '--keep');

const clean = { ...process.env };
delete clean.KANADE_FIDELITY;
const tagged = { ...clean, KANADE_FIDELITY: '1', ...(only.length ? { KANADE_FIDELITY_ONLY: only.join(',') } : {}) };
const run = (cmd: string[], env: NodeJS.ProcessEnv) => spawnSync(cmd[0]!, cmd.slice(1), { cwd, env, stdio: 'inherit' }).status ?? 1;

let status = run(['bun', 'run', 'build'], tagged);
if (status === 0) status = run(['bunx', 'playwright', 'test', 'e2e/fidelity.spec.ts'], tagged);
if (!keep) {
  const rebuilt = run(['bun', 'run', 'build'], clean);
  if (rebuilt !== 0) console.error('fidelity: the clean rebuild failed; dist may still hold data-fid tags');
  status ||= rebuilt;
}
console.log(`\nReports: ${join(cwd, 'e2e', '.captures', 'fidelity')}/<pair>/report.md and composite.png`);
process.exit(status);
