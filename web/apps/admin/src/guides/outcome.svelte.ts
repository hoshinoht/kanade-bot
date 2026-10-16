/**
 * The publishing admin's one toast when a run they started ends (approved
 * default 9): "Limbo party roles published · 7 added", wherever they are in
 * the admin app. Runs this tab started are followed until they finish; the
 * list's chip and time say it for everyone else.
 */
import type { GuideRun } from '@kanade/api-types';
import type { Toaster } from '@kanade/ui';
import { send } from '../resource.svelte';
import { guidePath, runSummary, stopWords } from './guides';

/** How often a followed run is read while it is live. */
const EVERY_MS = 1_500;

type Followed = { guide: string; run: string; title: string; channel: string };

export const followed = $state<{ runs: Followed[] }>({ runs: [] });

/** Follow a run this tab started, until it ends. */
export function follow(run: Followed): void {
  if (!followed.runs.some((r) => r.run === run.run)) followed.runs.push(run);
}

/** The words of an ended run's toast. */
export function outcomeToast(run: GuideRun, title: string, channel: string): { message: string; tone: 'ok' | 'error' } {
  const verb = run.kind === 'unpublish' ? 'unpublished' : run.kind === 'search' ? 'searched' : 'published';
  if (run.state === 'done') return { message: run.kind === 'search' ? `${title}: search finished.` : `${title} ${verb} · ${runSummary(run)}`, tone: 'ok' };
  if (run.state === 'cancelled') return { message: `${title}: publishing cancelled · ${runSummary(run)}`, tone: 'error' };
  return { message: `${title}: publishing stopped. ${stopWords(run.stop_reason, run, channel)}`, tone: 'error' };
}

/** App-wide: read followed runs while any is live and toast each once it ends. */
export function watchOutcomes(toaster: Toaster): () => void {
  const timer = setInterval(() => {
    for (const f of [...followed.runs]) {
      void send((c) => c.get<GuideRun>(guidePath(f.guide, `/publishes/${encodeURIComponent(f.run)}`))).then((read) => {
        // An earlier read may have ended it already: one toast per run.
        if (!followed.runs.some((r) => r.run === f.run)) return;
        if (read.ok && read.value.state !== 'queued' && read.value.state !== 'running') {
          followed.runs = followed.runs.filter((r) => r.run !== f.run);
          toaster.show(outcomeToast(read.value, f.title, f.channel));
        } else if (!read.ok && read.status === 404) followed.runs = followed.runs.filter((r) => r.run !== f.run);
      });
    }
  }, EVERY_MS);
  return () => clearInterval(timer);
}
