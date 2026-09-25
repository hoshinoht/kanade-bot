import { describe, expect, it } from 'vitest';
import { forbidModules } from '../src/vite';

type Hook = (this: { error: (m: string) => never }, options: unknown, bundle: Record<string, unknown>) => void;

function run(moduleIds: string[]) {
  const plugin = forbidModules({ patterns: ['/apps/admin/', '/@dnd-kit/'] });
  const hook = plugin.generateBundle as unknown as Hook;
  const ctx = {
    error: (message: string): never => {
      throw new Error(message);
    },
  };
  return () => hook.call(ctx, {}, { 'main.js': { type: 'chunk', fileName: 'main.js', moduleIds } });
}

describe('forbidModules', () => {
  it('passes a clean graph', () => {
    expect(run(['/w/apps/public/src/main.ts', '/w/packages/ui/src/index.ts'])).not.toThrow();
  });

  it('fails the build naming the offending module', () => {
    expect(run(['/w/apps/public/src/main.ts', '/w/node_modules/.bun/x/node_modules/@dnd-kit/dom/index.js'])).toThrow(
      /@dnd-kit\/dom\/index\.js/,
    );
    expect(run(['C:\\w\\apps\\admin\\src\\App.svelte'])).toThrow(/apps\/admin/);
  });
});
