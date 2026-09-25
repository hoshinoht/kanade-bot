import { describe, expect, it } from 'vitest';
import { ApiRequestError } from '@kanade/client';
import type { RescanJob } from '@kanade/api-types';
import { reread, type RescanTransport } from '../src/week/reread';

const job = (over: Partial<RescanJob> = {}): RescanJob => ({
  id: 'j1',
  state: 'running',
  window: 'week',
  channels: [{ id: 'hstar-party', name: '#hstar-party', state: 'reading', messages: 0 }],
  proposals: 0,
  ...over,
});

const done = job({ state: 'done', channels: [{ id: 'hstar-party', name: '#hstar-party', state: 'done', messages: 42 }], proposals: 2 });
const wait = () => Promise.resolve();

describe('reread', () => {
  it('reports messages and proposed changes on success', async () => {
    const transport: RescanTransport = {
      post: async () => job(),
      get: async () => done,
    };
    await expect(reread('hstar-party', '#hstar-party', { wait, transport })).resolves.toEqual({
      ok: true,
      message: 'Re-read #hstar-party: 42 messages, 2 changes proposed in the Inbox.',
    });
  });

  it('maps a refused start (unwatched channel) without polling', async () => {
    let gets = 0;
    const transport: RescanTransport = {
      post: async () => {
        throw new ApiRequestError('http', '#bm-trio is not watched, so there is nothing to re-read.', 422);
      },
      get: async () => {
        gets += 1;
        return done;
      },
    };
    await expect(reread('bm-trio', '#bm-trio', { wait, transport })).resolves.toEqual({
      ok: false,
      message: "Couldn't re-read #bm-trio: #bm-trio is not watched, so there is nothing to re-read.",
    });
    expect(gets).toBe(0);
  });

  it('rides out transient poll errors while the job continues', async () => {
    let gets = 0;
    const transport: RescanTransport = {
      post: async () => job(),
      get: async () => {
        gets += 1;
        if (gets < 3) throw new ApiRequestError('network', 'Network unavailable');
        return done;
      },
    };
    const outcome = await reread('hstar-party', '#hstar-party', { wait, transport });
    expect(outcome.ok).toBe(true);
    expect(gets).toBe(3);
  });

  it('gives up tracking after consecutive poll errors, without blaming the job', async () => {
    const transport: RescanTransport = {
      post: async () => job(),
      get: async () => {
        throw new ApiRequestError('timeout', 'No answer within 10 s');
      },
    };
    await expect(reread('hstar-party', '#hstar-party', { wait, transport })).resolves.toEqual({
      ok: false,
      message: 'Lost track of the re-read of #hstar-party; it continues on Extractions.',
    });
  });

  it('treats a vanished job as ended', async () => {
    const transport: RescanTransport = {
      post: async () => job(),
      get: async () => {
        throw new ApiRequestError('http', 'That run no longer exists.', 404);
      },
    };
    const outcome = await reread('hstar-party', '#hstar-party', { wait, transport });
    expect(outcome.message).toContain('disappeared');
  });
});
