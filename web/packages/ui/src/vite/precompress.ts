import { readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { brotliCompressSync, constants, gzipSync } from 'node:zlib';
import type { Plugin } from 'vite';

/** The types the Rust static fallback negotiates (`compressible` in `src/api/assets.rs`). */
const COMPRESSIBLE = /\.(?:js|mjs|css|html|svg|webmanifest|json)$/;
/** Below this, framing overhead and an extra file are not worth it. */
const THRESHOLD = 1024;

export interface PrecompressStats {
  br: number;
  gz: number;
  /** Raw bytes of the files that got at least one sibling. */
  raw: number;
  /** Bytes saved by the best sibling of each of those files. */
  saved: number;
}

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

/**
 * Writes `.br` (quality 11) and `.gz` (level 9) siblings for each compressible
 * file of at least `threshold` bytes, keeping a sibling only when it is smaller.
 * Stale siblings are removed first, so reruns over the same dir are idempotent.
 */
export function precompressDir(dir: string, threshold = THRESHOLD): PrecompressStats {
  const stats: PrecompressStats = { br: 0, gz: 0, raw: 0, saved: 0 };
  const files = walk(dir);
  for (const stale of files.filter((f) => /\.(?:br|gz)$/.test(f))) rmSync(stale);
  for (const file of files.filter((f) => COMPRESSIBLE.test(f))) {
    const source = readFileSync(file);
    if (source.length < threshold) continue;
    const variants = [
      ['br', brotliCompressSync(source, { params: { [constants.BROTLI_PARAM_QUALITY]: 11, [constants.BROTLI_PARAM_SIZE_HINT]: source.length } })],
      ['gz', gzipSync(source, { level: 9 })],
    ] as const;
    let best = source.length;
    for (const [suffix, bytes] of variants) {
      if (bytes.length >= source.length) continue;
      writeFileSync(`${file}.${suffix}`, bytes);
      stats[suffix] += 1;
      best = Math.min(best, bytes.length);
    }
    if (best < source.length) {
      stats.raw += source.length;
      stats.saved += source.length - best;
    }
  }
  return stats;
}

/**
 * Precompresses the client build for the Rust server. Runs as the last
 * `closeBundle` handler, after vite-plugin-pwa has written `sw.js` there, so the
 * worker gets siblings too; its precache globs list extensions, so `.br`/`.gz`
 * never enter the manifest. List it after `VitePWA()`.
 */
export function precompress(): Plugin {
  let outDir = '';
  return {
    name: 'kanade-precompress',
    apply: 'build',
    enforce: 'post',
    configResolved(config) {
      outDir = resolve(config.root, config.build.outDir);
    },
    closeBundle: {
      order: 'post',
      sequential: true,
      handler(error?: Error) {
        if (error || !outDir) return;
        const { br, gz, raw, saved } = precompressDir(outDir);
        this.info?.(`precompressed ${br} br + ${gz} gz, ${(saved / 1024).toFixed(1)} of ${(raw / 1024).toFixed(1)} KB saved`);
      },
    },
  };
}
