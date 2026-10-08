// Loaded before everything else, and written in the oldest JavaScript there is, so it also runs in an engine
// that cannot parse the scripts after it. If one of them fails to load or to parse before the page has
// started (a browser or web view below the supported versions, a host that does not serve a bundle file),
// the page says so instead of sitting at "Loading the engine..." for ever.
(function () {
  "use strict";
  var NEEDS = "It needs Chrome or Android System WebView 97, Safari 16 or Firefox 79, or newer.";
  window.mbNeeds = NEEDS;
  function say(text) {
    if (window.mbBooted) return;
    var status = document.getElementById("status");
    if (!status) return;
    status.className = "error";
    status.textContent = text;
  }
  // Script errors reach the window; a script file that could not be fetched only fires on its own element,
  // which the capturing listener sees.
  window.addEventListener(
    "error",
    function (event) {
      var target = event && event.target;
      if (target && target.tagName === "SCRIPT") {
        say("A part of this page could not be loaded (" + (target.getAttribute("src") || "script") + "). Check that all files of the site are in place.");
      } else if (event && event.message) {
        say("This browser cannot run the game. " + NEEDS + " (" + event.message + ")");
      }
    },
    true
  );
})();
