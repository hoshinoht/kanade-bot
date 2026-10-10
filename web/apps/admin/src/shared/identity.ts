import type { Identity } from '@kanade/api-types';

/** An art URL tagged with the identity's version, so a new avatar or banner is not served from cache. */
export function artUrl(url: string, identity: Pick<Identity, 'version'>): string {
  const v = identity.version;
  if (v === undefined || v === null || v === '') return url;
  return `${url}${url.includes('?') ? '&' : '?'}v=${encodeURIComponent(String(v))}`;
}
