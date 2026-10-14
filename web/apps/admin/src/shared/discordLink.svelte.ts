// Discord links open in the Discord app (`discord://`) unless this device turned
// that off on Account; the choice is kept in this browser like the rail's.
const KEY = "discord-links";

const WEB =
  /^https:\/\/(?:(?:ptb|canary)\.)?discord(?:app)?\.com\/channels\/(.+)$/;

/** The app deeplink for a Discord channel/message URL, or `null` for any other URL. */
export function toDeeplink(url: string): string | null {
  const m = WEB.exec(url);
  return m ? `discord://-/channels/${m[1]}` : null;
}

export interface LinkAttrs {
  href: string;
  target?: "_blank";
  rel?: string;
}

/**
 * A deeplink stays in this tab (a blank tab for a protocol handler would be
 * left empty); a web link opens a new tab as before.
 */
export function linkAttrs(url: string, app: boolean): LinkAttrs {
  const deep = app ? toDeeplink(url) : null;
  return deep
    ? { href: deep }
    : { href: url, target: "_blank", rel: "noopener noreferrer" };
}

function stored(): boolean {
  try {
    return localStorage.getItem(KEY) !== "web";
  } catch {
    return true;
  }
}

class DiscordLinks {
  /** Open Discord links in the app; on unless this device turned it off. */
  app = $state(stored());

  set(app: boolean): void {
    this.app = app;
    try {
      if (app) localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, "web");
    } catch {
      /* storage blocked: the choice lasts for this page only */
    }
  }
}

export const discordLinks = new DiscordLinks();

/** Attributes for an `<a>` to Discord under this device's choice; reactive in templates. */
export const discordLink = (url: string): LinkAttrs =>
  linkAttrs(url, discordLinks.app);
