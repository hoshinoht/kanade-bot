import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import type { Plugin } from 'vite';

/**
 * Emits the pre-paint theme bootstrap as a hashed classic script in <head>
 * (parser-blocking, so it still runs before first paint). External because script-src 'self' has no 'unsafe-inline'.
 */
export function themeBoot(): Plugin {
  const source = readFileSync(new URL('../theme/theme-boot.js', import.meta.url), 'utf8');
  const hash = createHash('sha256').update(source).digest('hex').slice(0, 10);
  const fileName = `assets/theme-boot-${hash}.js`;
  let base = '/';

  return {
    name: 'kanade-theme-boot',
    configResolved(config) {
      base = config.base;
    },
    configureServer(server) {
      server.middlewares.use(`${base}${fileName}`, (_req, res) => {
        res.setHeader('Content-Type', 'text/javascript');
        res.end(source);
      });
    },
    buildStart() {
      if (this.environment?.config.consumer === 'client' || !this.environment) {
        this.emitFile({ type: 'asset', fileName, source });
      }
    },
    transformIndexHtml: {
      order: 'post',
      handler: () => [{ tag: 'script', attrs: { src: `${base}${fileName}` }, injectTo: 'head' }],
    },
  };
}

export interface ForbidOptions {
  /** Module id fragments (paths or package names) the bundle must not contain. */
  patterns: string[];
}

/**
 * Fails the build if any listed module reaches the output graph. The public
 * app uses it so admin code can never ride along through a shared import.
 */
export function forbidModules({ patterns }: ForbidOptions): Plugin {
  return {
    name: 'kanade-forbid-modules',
    apply: 'build',
    generateBundle(_options, bundle) {
      const hits: string[] = [];
      for (const output of Object.values(bundle)) {
        if (output.type !== 'chunk') continue;
        for (const id of output.moduleIds) {
          const normalised = id.replaceAll('\\', '/');
          const pattern = patterns.find((p) => normalised.includes(p));
          if (pattern) hits.push(`${output.fileName} <- ${normalised} (${pattern})`);
        }
      }
      if (hits.length > 0) this.error(`Forbidden modules in bundle:\n${hits.join('\n')}`);
    },
  };
}

/** Literal `data-fid="…"` attributes only; dynamic values are not supported (and not stripped). */
const FID_ATTR = /\s+data-fid="[^"{}]*"/g;

/**
 * Removes the layout-fidelity tags (`data-fid`, matched against the M3E boards
 * by `e2e/fidelity.spec.ts`) from Svelte sources before they compile, so no
 * normal build ships them. Only `KANADE_FIDELITY=1` keeps them, and
 * `e2e/bundle.spec.ts` fails if a kept build is left in `dist`.
 */
export function stripFidelityTags(keep = process.env.KANADE_FIDELITY === '1'): Plugin {
  return {
    name: 'kanade-strip-fidelity-tags',
    enforce: 'pre',
    transform(code, id) {
      if (keep || !id.endsWith('.svelte') || !code.includes('data-fid')) return null;
      return { code: code.replace(FID_ATTR, ''), map: null };
    },
  };
}

export { precompress, precompressDir } from './precompress.ts';
