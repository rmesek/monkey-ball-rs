// DEVELOPER TOOL, not part of the game: `xtask serve-web --disc <image>` serves the page at /dev/ with this
// script added and the developer's own disc image at /dev-disc (loopback only). It hands that image to the
// page through the page's own file input, exactly as if it had been picked, so the import and everything
// after it can be exercised where a file dialog cannot be driven. scripts/build-web never copies this file.
"use strict";

(async () => {
  const input = document.getElementById("file");
  const pick = document.getElementById("pick");
  // Wait for the engine (the button is enabled once it is loaded and no stored game data took over).
  for (let i = 0; pick.disabled && i < 100; i++) await new Promise((r) => setTimeout(r, 100));
  if (pick.disabled) return;
  const response = await fetch("/dev-disc");
  if (!response.ok) {
    console.error("dev-disc: the server has no disc image (start it with --disc <path>)");
    return;
  }
  const file = new File([await response.blob()], "dev-disc.rvz");
  const transfer = new DataTransfer();
  transfer.items.add(file);
  input.files = transfer.files;
  input.dispatchEvent(new Event("change"));
})();
