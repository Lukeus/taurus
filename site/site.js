// Behaviour shared by both pages: the screenshot lightbox and the quick-start
// tabs. No framework; the pages work without this script, just less nicely.

(function () {
  // ---- Lightbox: any <img data-zoom> or .prose img opens full size. ----
  const dialog = document.createElement("dialog");
  dialog.className = "lightbox";
  dialog.innerHTML = '<img alt=""><p></p>';
  dialog.addEventListener("click", () => dialog.close());
  document.addEventListener("DOMContentLoaded", () => document.body.appendChild(dialog));

  document.addEventListener("click", (event) => {
    const img = event.target.closest("img[data-zoom], .prose img");
    if (!img || img.closest("a:not(.shot)")) return;
    event.preventDefault();
    const full = dialog.querySelector("img");
    full.src = img.currentSrc || img.src;
    full.alt = img.alt;
    dialog.querySelector("p").textContent = img.alt;
    dialog.showModal();
  });

  // ---- Tabs (WAI-ARIA tabs pattern, arrow keys move between them). ----
  document.addEventListener("DOMContentLoaded", () => {
    document.querySelectorAll(".tabs").forEach((tabs) => {
      const buttons = [...tabs.querySelectorAll('[role="tab"]')];
      const select = (btn) => {
        buttons.forEach((b) => {
          const on = b === btn;
          b.setAttribute("aria-selected", String(on));
          b.tabIndex = on ? 0 : -1;
          document.getElementById(b.getAttribute("aria-controls")).hidden = !on;
        });
      };
      buttons.forEach((btn, i) => {
        btn.addEventListener("click", () => select(btn));
        btn.addEventListener("keydown", (e) => {
          const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
          if (!step) return;
          const next = buttons[(i + step + buttons.length) % buttons.length];
          select(next);
          next.focus();
        });
      });
    });
  });
})();
