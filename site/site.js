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

  // ---- The drive: scroll position becomes the buggy's trip into the garage. ----
  // p is how far down the page you are, 0 to 1. The car covers the road by
  // p = 0.9, the door goes up ahead of it (0.55 to 0.72) and comes down behind it
  // (0.93 to 1). Nothing runs with reduced motion; the CSS hides the strip too.
  document.addEventListener("DOMContentLoaded", () => {
    const drive = document.querySelector(".drive");
    const motion = matchMedia("(prefers-reduced-motion: reduce)");
    if (!drive || motion.matches) return;

    const root = document.documentElement;
    const car = drive.querySelector(".drive-car");
    const garage = drive.querySelector(".drive-garage");
    const span = (p, from, to) => Math.min(1, Math.max(0, (p - from) / (to - from)));
    let queued = false;

    const draw = () => {
      queued = false;
      const max = root.scrollHeight - innerHeight;
      const p = max > 0 ? Math.min(1, Math.max(0, scrollY / max)) : 0;
      const carW = car.getBoundingClientRect().width;
      const g = garage.getBoundingClientRect();
      // From just off the left edge to centred in the garage's doorway.
      const start = -carW - 8;
      const end = g.left + g.width / 2 - carW / 2;
      const t = span(p, 0.02, 0.9);
      const x = start + (end - start) * t;
      const wheel = (Math.PI * carW * 26) / 120; // wheel circumference in px
      const door = span(p, 0.55, 0.72) - span(p, 0.93, 1);

      root.style.setProperty("--p", p.toFixed(4));
      drive.style.setProperty("--car-x", `${x.toFixed(1)}px`);
      drive.style.setProperty("--spin", `${(((x - start) / wheel) * 360).toFixed(1)}deg`);
      drive.style.setProperty("--door", door.toFixed(3));
      drive.classList.toggle("on", scrollY > 120);
      drive.classList.toggle("home", p > 0.98);
    };
    const queue = () => {
      if (!queued) requestAnimationFrame(draw);
      queued = true;
    };
    addEventListener("scroll", queue, { passive: true });
    addEventListener("resize", queue);
    draw();
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
