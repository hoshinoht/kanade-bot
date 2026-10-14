// Classic, synchronous and external: it must run before first paint, and
// script-src 'self' forbids the inline bootstrap v4 used. Keys match v4's
// theme_boot.html so a browser's stored choice means the same thing.
(function () {
  try {
    // Keep in step with COLORWAYS in @kanade/tokens/colorways.
    var ways = [
      "marigold", "blossom", "periwinkle", "twilight",
      "hoshino", "mika", "seia", "hina", "aris",
      "catppuccin", "tokyonight", "github", "dynamic",
    ];
    var root = document.documentElement;
    var c = localStorage.getItem("colorway");
    var t = localStorage.getItem("theme");
    if (ways.indexOf(c) > -1) root.dataset.colorway = c;
    // A retired colourway (coral) or junk: forget it, so the page wears marigold.
    else if (c !== null) localStorage.removeItem("colorway");
    if (t === "light" || t === "dark") root.dataset.theme = t;
    // This browser's "Reduce motion" switch (motion/preference.svelte.ts), set
    // before first paint so no animation starts and then stops.
    if (localStorage.getItem("kanade.motion") === "reduce") root.dataset.motion = "reduce";
    // The Dynamic palette last derived from the avatar (theme.ts refreshes it),
    // applied through the CSSOM so the first paint already wears it.
    var cached = JSON.parse(localStorage.getItem("colorway-dynamic") || "null");
    if (cached && typeof cached === "object") {
      for (var name in cached) {
        if (/^--dyn-[a-z-]+-[ld]$/.test(name) && /^#[0-9a-f]{6}$/.test(cached[name])) {
          root.style.setProperty(name, cached[name]);
        }
      }
    }
  } catch {
    // Private windows may throw; the page then wears marigold and follows the device.
  }
})();
