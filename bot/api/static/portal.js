/* Dialogs, the phone menu, the colourway, and the Limits page's live updates.
 *
 * Every control on the portal is a real form or link, so this file only
 * upgrades three interactions: a link that points at a <dialog> opens it as a
 * modal instead of scrolling to it, the phone nav's <details> closes when you
 * tap outside it, and the Limits page listens for server-sent
 * events instead of asking on a timer. With the script blocked the same link is
 * a fragment link and `dialog:target` (portal.css) reveals the form in place,
 * so the page still edits a fixed timing without JavaScript; the Limits page
 * falls back to its slow htmx poll, and then to its Refresh link.
 *
 * The colourway is the one exception to that rule, and a deliberate one. It is
 * a preference of this browser and nothing else: it is kept in localStorage,
 * the server is never told, and there is therefore no state a POST could write
 * and nothing to redirect back to. Making it a form would mean inventing a
 * cookie, a route and a round trip for a choice that has to apply before the
 * next paint anyway. With JavaScript off the radios on the Config page do
 * nothing and the portal stays on the default palette, which is a page that
 * looks slightly different rather than a page that cannot be used -- the test
 * every other control has to pass and this one does not need to. The Config
 * card says so in a <noscript>.
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
  var COLORWAYS = ["otonose", "nazuna", "sumire", "coral", "hinano"];
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
  window.setInterval(tickHeld, 1000);
})();
