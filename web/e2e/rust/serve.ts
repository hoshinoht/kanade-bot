import { access, appendFile, chmod, mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, type ChildProcess } from 'node:child_process';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const web = join(repo, 'web');
const results = join(web, 'e2e/.results-rust');
const adminPort = requiredPort('KANADE_RUST_E2E_ADMIN_PORT');
const publicPort = requiredPort('KANADE_RUST_E2E_PUBLIC_PORT');
const adminToken = 'e2e-break-glass-token-0123456789abcdef';

let root = '';
let fixture: ChildProcess | undefined;
let server: ChildProcess | undefined;
let stopping = false;
let finished = false;
let stderr = '';
let stdout = '';

function requiredPort(name: string): number {
  const port = Number(process.env[name]);
  if (!Number.isInteger(port) || port < 1 || port > 65_535) throw new Error(`${name} must be a valid port`);
  return port;
}

async function secureDir(path: string): Promise<void> {
  await mkdir(path, { recursive: true, mode: 0o700 });
  await chmod(path, 0o700);
}

async function secret(path: string, value: string): Promise<void> {
  await writeFile(path, `${value}\n`, { mode: 0o600 });
  await chmod(path, 0o600);
}

function toolEnv(): NodeJS.ProcessEnv {
  // Cargo needs only its toolchain locations; Kanade configuration is never inherited.
  return Object.fromEntries(['PATH', 'HOME', 'CARGO_HOME', 'RUSTUP_HOME'].flatMap((name) => (process.env[name] ? [[name, process.env[name]!]] : [])));
}

function wait(child: ChildProcess, name: string): Promise<void> {
  return new Promise((resolveWait, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => {
      if (code === 0) resolveWait();
      else reject(new Error(`${name} exited ${code ?? `from ${signal ?? 'an unknown signal'}`}`));
    });
  });
}

async function seed(): Promise<void> {
  fixture = spawn(
    'cargo',
    [
      'run',
      '--quiet',
      '--locked',
      '--offline',
      '--example',
      'rust_e2e_fixture',
      '--features',
      'test-support',
      '--',
      '--db',
      join(root, 'db/kanade.sqlite3'),
      '--lock-dir',
      join(root, 'locks'),
      '--timezone',
      'Asia/Kuala_Lumpur',
      '--reset-weekday',
      'wed',
      '--reset-time',
      '08:00',
    ],
    { cwd: repo, env: toolEnv(), stdio: 'inherit' },
  );
  await wait(fixture, 'rust E2E fixture');
  fixture = undefined;
}

function serverEnv(): NodeJS.ProcessEnv {
  return {
    KANADE_TIMEZONE: 'Asia/Kuala_Lumpur',
    KANADE_ADMIN_BIND: `127.0.0.1:${adminPort}`,
    KANADE_PUBLIC_BIND: `127.0.0.1:${publicPort}`,
    KANADE_PUBLIC_HOST: `127.0.0.1:${publicPort}`,
    KANADE_DISCORD_GATEWAY: '0',
    KANADE_DISCORD_TOKEN_FILE: join(root, 'discord_token'),
    KANADE_ADMIN_TOKEN_FILE: join(root, 'admin_token'),
    KANADE_GUILD_ID: '900',
    KANADE_BOSSING_ROLE_ID: '10',
    KANADE_ADMIN_ROLE_ID: '20',
    KANADE_DB_PATH: join(root, 'db/kanade.sqlite3'),
    KANADE_OWNER_LOCK_DIR: join(root, 'locks'),
    KANADE_WEB_DIR: web,
    KANADE_CATALOG_FILE: join(repo, 'boss/bosses.yaml'),
    KANADE_KNOWLEDGE_DIR: join(repo, 'boss/knowledge'),
    KANADE_BOSS_DIR: join(web, 'e2e/fixtures/boss'),
    KANADE_PERSONA_DIR: join(root, 'Personas'),
    KANADE_WATCH_CATEGORY_IDS: '41,42',
    KANADE_CHAT_CATEGORY_IDS: '43',
    KANADE_BOSS_WEEK_RESET_WEEKDAY: 'wed',
    KANADE_BOSS_WEEK_RESET_TIME: '08:00',
  };
}

async function writePersona(): Promise<void> {
  const personas = join(root, 'Personas');
  const bundles = join(personas, 'bundles');
  await secureDir(personas);
  await secureDir(bundles);
  await secret(
    join(personas, 'catalog.yaml'),
    'schema_version: 1\ndefault: kanade\npersonas:\n  - id: kanade\n    label: Kanade\n    aliases: []',
  );
  await secret(
    join(bundles, 'kanade.yaml'),
    "schema_version: 1\nid: kanade\nidentity: |\n  # Persona: Kanade\n\n  Synthetic Rust E2E persona.\nbehaviour:\n  voice: Synthetic.\n  prompt: |\n    Synthetic behaviour.\nstaging:\n  schedule: Synthetic schedule\n  guide: Synthetic guide\n  guide_named: '{boss} synthetic guide'\n  write: Synthetic write\n  generic: Synthetic reply",
  );
}

async function finish(code: number | null, signal: NodeJS.Signals | null): Promise<void> {
  if (finished) return;
  finished = true;
  await secureDir(results);
  const log = join(results, 'server.log');
  await writeFile(log, `${stdout}${stderr}`, { mode: 0o600 });
  const closed = `${stdout}${stderr}`.includes('"event":"store_closed"');
  if ((closed || !server) && root) {
    await rm(root, { recursive: true, force: true });
    await appendFile(log, `rust_e2e_cleanup=${closed ? 'removed_after_store_closed' : 'removed_after_fixture_exit'}\n`);
  } else {
    await appendFile(log, `rust_e2e_cleanup=retained_without_store_closed root=${root}\n`);
  }
  process.exitCode = stopping && code === 0 && closed ? 0 : 1;
  if (process.exitCode !== 0) console.error(`Rust E2E server exited ${code ?? signal ?? 'unexpectedly'}; see ${log}`);
}

async function stop(): Promise<void> {
  if (stopping) return;
  stopping = true;
  if (fixture && !fixture.killed) {
    fixture.kill('SIGTERM');
    return;
  }
  if (server && !server.killed) server.kill('SIGTERM');
  else await finish(1, 'SIGTERM');
}

async function main(): Promise<void> {
  // macOS reports /var/... here, but the store correctly refuses that symlink.
  root = await mkdtemp(join(await realpath(tmpdir()), 'kanade-rust-e2e-'));
  await chmod(root, 0o700);
  await secureDir(join(root, 'db'));
  await secureDir(join(root, 'locks'));
  await secret(join(root, 'discord_token'), 'e2e-discord-token-not-a-real-secret');
  await secret(join(root, 'admin_token'), adminToken);
  await writePersona();
  await seed();
  if (stopping) return;

  const binary = process.env.KANADE_RUST_E2E_BINARY ?? join(repo, 'target/release/kanade');
  await access(binary);
  server = spawn(binary, ['serve'], { cwd: repo, env: serverEnv(), stdio: ['ignore', 'pipe', 'pipe'] });
  server.stdout?.on('data', (chunk: Buffer) => {
    stdout += chunk.toString();
  });
  server.stderr?.on('data', (chunk: Buffer) => {
    const text = chunk.toString();
    stderr += text;
    process.stderr.write(text);
  });
  server.once('error', (error) => {
    stderr += `${error.message}\n`;
    void finish(1, null);
  });
  server.once('close', (code, signal) => void finish(code, signal));
}

process.once('SIGTERM', () => void stop());
process.once('SIGINT', () => void stop());

void main().catch(async (error: unknown) => {
  await secureDir(results);
  const log = join(results, 'server.log');
  const detail = error instanceof Error ? error.stack ?? error.message : String(error);
  if (root && !server) await rm(root, { recursive: true, force: true });
  await writeFile(log, `${detail}\nrust_e2e_cleanup=${server ? 'retained_without_store_closed' : 'removed_after_fixture_exit'} root=${root}\n`, { mode: 0o600 });
  console.error(error);
  process.exitCode = 1;
});
