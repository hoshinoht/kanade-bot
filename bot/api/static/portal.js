/* Dialogs, and nothing else.
 *
 * Every control on the portal is a real form or link, so this file only
 * upgrades one interaction: a link that points at a <dialog> opens it as a
 * modal instead of scrolling to it. With the script blocked the same link is
 * a fragment link and `dialog:target` (portal.css) reveals the form in place,
 * so the page still edits a fixed timing without JavaScript.
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
})();
