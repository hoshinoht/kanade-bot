/* Dialogs, the phone menu, how the portal looks, and the Limits page's live
 * updates.
 *
 * Every control on the portal is a real form or link, so this file only
 * upgrades interactions: dialog links open as modals, the phone nav closes when
 * tapped past, long behaviour-plugin lists paginate in place, and the Limits
 * page listens for server-sent events instead of asking on a timer. With the
 * script blocked the same dialog link is
 * a fragment link and `dialog:target` (portal.css) reveals the form in place,
 * so the page still edits a fixed timing without JavaScript; the Limits page
 * falls back to its slow htmx poll, and then to its Refresh link.
 *
 * The two appearance controls -- the colourway, and light/dark/system -- are
 * the one exception to that rule, and a deliberate one. They are preferences of
 * this browser and nothing else: kept in localStorage, never told to the server,
 * so there is no state a POST could write and nothing to redirect back to.
 * Making them forms would mean inventing cookies, a route and a round trip for
 * choices that have to apply before the next paint anyway. With JavaScript off
 * the radios on the Config page do nothing, the portal stays on the default
 * palette and the device keeps deciding the hour -- a page that looks slightly
 * different rather than a page that cannot be used, which is the test every
 * other control has to pass and these two do not need to. The Config card says
 * so in a <noscript>.
 *
 * Served from the bot itself, like every other asset here. The SSE wiring is a
 * dozen lines of `EventSource` rather than htmx's SSE extension because the
 * extension would be a third-party file to vendor and keep pinned, and this
 * needs one event to mean one refetch.
 */
(function () {
  "use strict";

  function dialogFor(el) {
    var id = el.getAttribute("data-dialog");
    var node = id && document.getElementById(id);
    return node && node.tagName === "DIALOG" ? node : null;
  }

  document.addEventListener("click", function (event) {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey) {
      return;
    }

    var opener = event.target.closest("[data-dialog]");
    if (opener) {
      var dialog = dialogFor(opener);
      if (dialog && typeof dialog.showModal === "function") {
        event.preventDefault();
        dialog.showModal();
      }
      return;
    }

    var closer = event.target.closest("[data-dialog-close]");
    if (closer) {
      event.preventDefault();
      var owner = closer.closest("dialog");
      if (owner) {
        owner.close();
      }
      return;
    }

    // The backdrop is the dialog element itself: the panel inside it covers
    // the whole box, so a click that lands on <dialog> landed outside.
    if (event.target.tagName === "DIALOG") {
      event.target.close();
    }
  });

  // The phone nav is a plain <details>, so it already opens, closes and takes
  // focus without any of this. All that is added here is what a menu is
  // expected to do and markup alone cannot: shut when you tap past it.
  document.addEventListener("click", function (event) {
    var open = document.querySelector(".nav__more[open]");
    if (open && !open.contains(event.target)) {
      open.removeAttribute("open");
    }
  });

  // -- the colourway, which lives in this browser and nowhere else -----------
  //
  // The same five keys as the bootstrap in `templates/partials/theme_boot.html`
  // and as `COLORWAYS` in `bot/api/templating.py`: the snippet has to be
  // inline and tiny, so it repeats them rather than importing them.
  var COLORWAYS = ["marigold", "blossom", "periwinkle", "coral", "twilight"];
  var COLORWAY_KEY = "colorway";

  function storedColorway() {
    try {
      var value = window.localStorage.getItem(COLORWAY_KEY);
      return COLORWAYS.indexOf(value) > -1 ? value : COLORWAYS[0];
    } catch (e) {
      // A private window may refuse to answer at all; that is the default.
      return COLORWAYS[0];
    }
  }

  // The server renders the default ticked, because it does not know any better;
  // this puts the tick on whichever one is actually applied.
  function markColorway() {
    var inputs = document.querySelectorAll('input[name="colorway"]');
    var current = storedColorway();
    for (var i = 0; i < inputs.length; i++) {
      inputs[i].checked = inputs[i].value === current;
    }
  }

  document.addEventListener("change", function (event) {
    var input = event.target;
    if (input.name !== COLORWAY_KEY || COLORWAYS.indexOf(input.value) < 0) {
      return;
    }
    // Applied first and remembered second: a browser that will not store it
    // still changes colour for as long as the tab is open.
    document.documentElement.dataset.colorway = input.value;
    try {
      window.localStorage.setItem(COLORWAY_KEY, input.value);
    } catch (e) {
      // Nothing to do about it, and nothing worth saying to the reader.
    }
  });

  // -- light or dark, when the device's answer is not the wanted one ---------
  //
  // Three states from two stored ones: "system" is the *absence* of a choice,
  // so choosing it removes the key and the attribute and hands the question
  // back to `prefers-color-scheme` in portal.css. Storing the word "system"
  // instead would mean a third value for that stylesheet to know about, and a
  // browser with the script off could never reach it anyway.
  var THEMES = ["light", "dark"];
  var THEME_KEY = "theme";
  var SYSTEM = "system";

  function storedTheme() {
    try {
      var value = window.localStorage.getItem(THEME_KEY);
      return THEMES.indexOf(value) > -1 ? value : SYSTEM;
    } catch (e) {
      return SYSTEM;
    }
  }

  function markTheme() {
    var inputs = document.querySelectorAll('input[name="thememode"]');
    var current = storedTheme();
    for (var i = 0; i < inputs.length; i++) {
      inputs[i].checked = inputs[i].value === current;
    }
  }

  document.addEventListener("change", function (event) {
    var input = event.target;
    if (input.name !== "thememode") {
      return;
    }
    var wanted = input.value;
    if (wanted !== SYSTEM && THEMES.indexOf(wanted) < 0) {
      return;
    }
    // Applied first, remembered second, as above.
    if (wanted === SYSTEM) {
      delete document.documentElement.dataset.theme;
    } else {
      document.documentElement.dataset.theme = wanted;
    }
    try {
      if (wanted === SYSTEM) {
        window.localStorage.removeItem(THEME_KEY);
      } else {
        window.localStorage.setItem(THEME_KEY, wanted);
      }
    } catch (e) {
      // Nothing to do about it, and nothing worth saying to the reader.
    }
  });

  // -- compact pages for the two behaviour-plugin editor lists ----------------
  //
  // Every editor is server-rendered and visible without JavaScript. This is only
  // a space-saving enhancement for a list that has grown past one screen, and
  // sessionStorage returns an editor to the page it was on after its form posts.
  function storedListPage(key) {
    try {
      return Number(window.sessionStorage.getItem("config-page:" + key)) || 0;
    } catch (e) {
      return 0;
    }
  }

  function storeListPage(key, page) {
    try {
      window.sessionStorage.setItem("config-page:" + key, String(page));
    } catch (e) {
      // Pagination still works for this document when storage is unavailable.
    }
  }

  function paginateList(root) {
    var items = root.querySelectorAll("[data-page-item]");
    var controls = root.querySelector("[data-page-controls]");
    var pageSize = Number(root.dataset.pageSize) || 5;
    if (!controls || items.length <= pageSize) {
      return;
    }

    var previous = controls.querySelector("[data-page-previous]");
    var next = controls.querySelector("[data-page-next]");
    var status = controls.querySelector("[data-page-status]");
    var key = root.dataset.paginationKey || "list";
    var pages = Math.ceil(items.length / pageSize);
    var current = Math.min(storedListPage(key), pages - 1);

    function render(page) {
      current = Math.max(0, Math.min(page, pages - 1));
      for (var i = 0; i < items.length; i++) {
        items[i].hidden = Math.floor(i / pageSize) !== current;
      }
      previous.disabled = current === 0;
      next.disabled = current === pages - 1;
      status.textContent =
        "Page " + (current + 1) + " of " + pages + " · " + items.length + " " +
        (root.dataset.pageLabel || "items");
      storeListPage(key, current);
    }

    previous.addEventListener("click", function () {
      render(current - 1);
    });
    next.addEventListener("click", function () {
      render(current + 1);
    });
    controls.hidden = false;
    render(current);
  }

  function paginateLists() {
    var lists = document.querySelectorAll("[data-paginated-list]");
    for (var i = 0; i < lists.length; i++) {
      paginateList(lists[i]);
    }
  }

  // -- the Limits page, updated when something happens rather than on a timer --

  function liveRegion() {
    return document.querySelector("[data-live-src]");
  }

  function watchLive() {
    var region = liveRegion();
    // `htmx.ajax` is what does the refetch, so with htmx blocked there is
    // nothing to drive and the page keeps whatever it was rendered with.
    if (!region || !window.EventSource || !window.htmx) {
      return;
    }
    var source = new EventSource(region.getAttribute("data-live-src"));
    source.addEventListener("limits", function () {
      // Re-read the region fresh each time: a swap replaces its contents, and
      // an element captured up front would be the one still in the document
      // but no longer the one being written to.
      var target = liveRegion();
      if (target) {
        window.htmx.ajax("GET", target.getAttribute("hx-get"), { target: target, swap: "innerHTML" });
      }
    });
    // The browser reconnects a dropped stream on its own; a stream that fails
    // outright leaves the htmx poll in the markup doing the job slowly.
    source.addEventListener("error", function () {
      if (source.readyState === EventSource.CLOSED) {
        window.setTimeout(watchLive, 30000);
      }
    });
    window.addEventListener("pagehide", function () {
      source.close();
    });
  }

  // How long the model has been held, counted forward in the browser between
  // events. The server's figure is right when the fragment is rendered and
  // then stands still -- and a hold that says "0s" for half a minute reads as
  // a broken page rather than a busy one. Only ever counts up from what the
  // server said, so it cannot disagree with it about anything but the seconds.
  function tickHeld() {
    var elements = document.querySelectorAll("[data-held-for]");
    for (var i = 0; i < elements.length; i++) {
      var el = elements[i];
      if (!el.dataset.heldSeenAt) {
        el.dataset.heldSeenAt = String(Date.now());
      }
      var since = (Date.now() - Number(el.dataset.heldSeenAt)) / 1000;
      el.textContent = Math.round(Number(el.dataset.heldFor) + since) + "s";
    }
  }

  function onReady(fn) {
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", fn);
    } else {
      fn();
    }
  }

  onReady(watchLive);
  onReady(markColorway);
  onReady(markTheme);
  onReady(paginateLists);
  window.setInterval(tickHeld, 1000);
})();
