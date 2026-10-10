import { afterEach, describe, expect, it, vi } from "vitest";
import { linkAttrs, toDeeplink } from "../src/shared/discordLink.svelte";

const MSG = "https://discord.com/channels/111/222/333";

describe("Discord deeplinks", () => {
  it("turns a Discord channel/message URL into the app scheme", () => {
    expect(toDeeplink(MSG)).toBe("discord://-/channels/111/222/333");
    expect(toDeeplink("https://discordapp.com/channels/1/2/3")).toBe(
      "discord://-/channels/1/2/3",
    );
    expect(toDeeplink("https://ptb.discord.com/channels/1/2")).toBe(
      "discord://-/channels/1/2",
    );
    expect(toDeeplink("https://canary.discord.com/channels/1/2/3")).toBe(
      "discord://-/channels/1/2/3",
    );
  });

  it("leaves every other URL alone", () => {
    for (const url of [
      "http://discord.com/channels/1/2/3",
      "https://discord.com/invite/abc",
      "https://evil.example/discord.com/channels/1/2/3",
      "https://discord.com.evil.example/channels/1/2/3",
      "https://www.discord.com/channels/1/2/3",
      "https://discord.com/channels/",
      "/history",
    ]) {
      expect(toDeeplink(url)).toBeNull();
      expect(linkAttrs(url, true)).toEqual({
        href: url,
        target: "_blank",
        rel: "noopener noreferrer",
      });
    }
  });

  it("a deeplink stays in this tab; off keeps the web link in a new tab", () => {
    expect(linkAttrs(MSG, true)).toEqual({
      href: "discord://-/channels/111/222/333",
    });
    expect(linkAttrs(MSG, false)).toEqual({
      href: MSG,
      target: "_blank",
      rel: "noopener noreferrer",
    });
  });
});

describe("the per-device choice", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  function storage(initial: Record<string, string> = {}) {
    const map = new Map(Object.entries(initial));
    const fake = {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
    };
    vi.stubGlobal("localStorage", fake);
    return map;
  }

  it("is on by default, also with storage blocked", async () => {
    storage();
    const { discordLink, discordLinks } =
      await import("../src/shared/discordLink.svelte");
    expect(discordLinks.app).toBe(true);
    expect(discordLink(MSG).href).toBe("discord://-/channels/111/222/333");

    vi.resetModules();
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
    });
    const blocked = await import("../src/shared/discordLink.svelte");
    expect(blocked.discordLinks.app).toBe(true);
  });

  it("turning it off is remembered; on again clears the key", async () => {
    const map = storage();
    const { discordLink, discordLinks } =
      await import("../src/shared/discordLink.svelte");
    discordLinks.set(false);
    expect(map.get("discord-links")).toBe("web");
    expect(discordLink(MSG)).toEqual({
      href: MSG,
      target: "_blank",
      rel: "noopener noreferrer",
    });

    vi.resetModules();
    const reloaded = await import("../src/shared/discordLink.svelte");
    expect(reloaded.discordLinks.app).toBe(false);
    reloaded.discordLinks.set(true);
    expect(map.has("discord-links")).toBe(false);
  });
});
