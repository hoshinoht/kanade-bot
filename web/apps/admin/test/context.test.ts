import { describe, expect, it } from "vitest";
import type { EffectiveContext, ModelInfo } from "@kanade/api-types";
import {
  MAX_CONTEXT_TOKENS,
  MAX_RESERVE,
  clampNotes,
  reserveHelp,
  isLocal,
  overrideMax,
  stopIndex,
  tokenStops,
} from "../src/config/context";

const model = (fields: Partial<ModelInfo>): ModelInfo => fields as ModelInfo;

const effective = (fields: Partial<EffectiveContext>): EffectiveContext =>
  ({
    window: 8192,
    reserve: 1024,
    prompt_budget: 7168,
    source: "local_default",
    clamped_by_published: false,
    clamped_by_hard_cap: false,
    clamped_by_role_cap: false,
    local_warning: false,
    ...fields,
  }) as EffectiveContext;

describe("tokenStops", () => {
  it("uses quarter steps, keeps 16k as a stop and ends at the maximum", () => {
    const stops = tokenStops(MAX_CONTEXT_TOKENS);
    expect(stops[0]).toBe(64);
    expect(stops).toContain(16_384);
    expect(stops.slice(stops.indexOf(8192), stops.indexOf(8192) + 5)).toEqual([
      8192, 10_240, 12_288, 14_336, 16_384,
    ]);
    expect(stops.at(-1)).toBe(MAX_CONTEXT_TOKENS);
    expect(new Set(stops).size).toBe(stops.length);
  });

  it("ends at an unround published maximum", () => {
    const stops = tokenStops(20_000);
    expect(stops.at(-1)).toBe(20_000);
    expect(stops.every((stop) => stop <= 20_000)).toBe(true);
  });
});

describe("stopIndex", () => {
  const stops = tokenStops(MAX_CONTEXT_TOKENS);
  it("picks the last stop at or below the value", () => {
    expect(stops[stopIndex(stops, 16_384)]).toBe(16_384);
    expect(stops[stopIndex(stops, 16_000)]).toBe(14_336);
    expect(stopIndex(stops, null)).toBe(0);
  });
});

describe("isLocal and overrideMax", () => {
  it("treats only listed homelab routes as local", () => {
    expect(isLocal(model({ leaves_homelab: false }))).toBe(true);
    expect(isLocal(model({ leaves_homelab: true }))).toBe(false);
    expect(isLocal(undefined)).toBe(false);
  });

  it("caps an override at the published window and the hard limit", () => {
    expect(overrideMax(model({ context_tokens: 65_536 }))).toBe(65_536);
    expect(overrideMax(model({ context_tokens: 200_000 }))).toBe(
      MAX_CONTEXT_TOKENS,
    );
    expect(overrideMax(model({}))).toBe(MAX_CONTEXT_TOKENS);
    expect(overrideMax(undefined)).toBe(MAX_CONTEXT_TOKENS);
  });
});

describe("clampNotes", () => {
  it("reports only the server flags and a reserve held below its saved value", () => {
    expect(clampNotes(effective({}), 1024)).toEqual([]);
    expect(
      clampNotes(
        effective({
          clamped_by_published: true,
          clamped_by_role_cap: true,
          reserve: 512,
        }),
        1024,
      ),
    ).toEqual([
      "held to the published window",
      "held to the role cap",
      "reserve held to the max output",
    ]);
    expect(clampNotes(effective({ clamped_by_hard_cap: true }), 1024)).toEqual([
      "held to the 131,072 limit",
    ]);
  });
});

describe('reserve limit', () => {
  it('names the effective limit and the call budget behind it', () => {
    expect(MAX_RESERVE).toBe(15_359);
    expect(reserveHelp()).toBe("At most 15,359: each call's token budget is 16,384, and at least 1,024 of it stays for the prompt.");
    expect(tokenStops(MAX_RESERVE).at(-1)).toBe(MAX_RESERVE);
  });
});
