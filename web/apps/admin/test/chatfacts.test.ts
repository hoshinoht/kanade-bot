import { describe, expect, it } from 'vitest';
import { guardrailFlags, messageParts, modelViewState, profileText, routeLabel, took } from '../src/chat/facts';

describe('chat turn facts', () => {
  it('tells an unknown time from a real zero', () => {
    expect(took(null)).toBe('unknown');
    expect(took(0)).toBe('0 ms');
    expect(took(41)).toBe('41 ms');
  });

  it('names routes for people', () => {
    expect(routeLabel('homelab')).toBe('Homelab');
    expect(routeLabel('external_masked')).toBe('External (masked)');
    expect(routeLabel('external_unmasked')).toBe('External (unmasked)');
    expect(routeLabel(null)).toBe('—');
  });

  it('says where a reply profile came from', () => {
    expect(profileText({ profile: 'gentle', profile_source: 'saved' })).toBe('gentle (saved)');
    expect(profileText({ profile: 'terse', profile_source: 'role' })).toBe('terse (from role)');
    expect(profileText({ profile: null, profile_source: 'default' })).toBe('default voice (default)');
    expect(profileText({ profile: null, profile_source: null })).toBe('—');
  });

  it('lists only the guardrails that fired', () => {
    expect(guardrailFlags({ pseudonymized: true, content_filter: false, identity_leak_blocked: { count: 1 } })).toEqual(['pseudonymized', 'identity leak blocked']);
    expect(guardrailFlags({})).toEqual([]);
  });

  it('shows a Model view only for masked turns, and explains a withheld one', () => {
    const view = { rounds: [], reply: '', mapping: [] };
    expect(modelViewState({ masked: false, model_view: null })).toBe('none');
    expect(modelViewState({ masked: true, model_view: null })).toBe('withheld');
    expect(modelViewState({ masked: true, model_view: view })).toBe('shown');
  });

  it('keeps a request message as sent: text, then its other fields', () => {
    expect(messageParts({ role: 'user', content: 'Haruka: hi' })).toEqual({ role: 'user', content: 'Haruka: hi', extra: null });
    const call = messageParts({ role: 'assistant', content: null, tool_calls: [{ id: 'call_1', name: 'schedule.read' }] });
    expect(call.content).toBeNull();
    expect(JSON.parse(call.extra!)).toEqual({ tool_calls: [{ id: 'call_1', name: 'schedule.read' }] });
  });
});
