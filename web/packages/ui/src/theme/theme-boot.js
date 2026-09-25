// Classic, synchronous and external: it must run before first paint, and
// script-src 'self' forbids the inline bootstrap v4 used. Keys match v4's
// theme_boot.html so a browser's stored choice means the same thing.
(function () {
  try {
    var ways = ["marigold", "blossom", "periwinkle", "coral", "twilight"];
    var c = localStorage.getItem("colorway");
    var t = localStorage.getItem("theme");
    if (ways.indexOf(c) > -1) document.documentElement.dataset.colorway = c;
    if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  } catch {
    // Private windows may throw; the page then wears marigold and follows the device.
  }
})();
