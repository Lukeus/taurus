// The docs viewer. It renders the repository's own Markdown (README.md and
// docs/*.md) in the browser, so the site can never drift from the docs: there
// is no second copy to forget to update.
//
// Pages are addressed as docs.html?p=<id>#<heading-slug>. Heading ids follow
// GitHub's slug rules, so every anchor the Markdown already links to (and
// every link someone copied from github.com) lands in the same place here.

const REPO = "https://github.com/Lukeus/taurus";

const PAGES = [
  { id: "overview", file: "README.md", title: "Overview" },
  { id: "capabilities", file: "docs/capabilities.md", title: "Capabilities" },
  { id: "safety", file: "docs/safety.md", title: "Permission and undo" },
  { id: "working-with-it", file: "docs/working-with-it.md", title: "Working with it" },
  { id: "configuration", file: "docs/configuration.md", title: "Configuration" },
  { id: "known-gaps", file: "docs/known-gaps.md", title: "Known gaps" },
  { id: "development", file: "docs/development.md", title: "Development" },
];
const BY_FILE = new Map(PAGES.map((p) => [p.file, p]));

const params = new URLSearchParams(location.search);
const current = PAGES.find((p) => p.id === params.get("p")) || PAGES[0];

// ---------- Fetching ----------

const cache = new Map();
function fetchMarkdown(page) {
  if (!cache.has(page.file)) {
    cache.set(
      page.file,
      fetch(page.file).then((r) => {
        if (!r.ok) throw new Error(`${page.file}: ${r.status}`);
        return r.text();
      }),
    );
  }
  return cache.get(page.file);
}

// ---------- Slugs (github-slugger's rules) ----------

function slugger() {
  const seen = new Map();
  return (text) => {
    const base = text
      .trim()
      .toLowerCase()
      .replace(/[^\p{L}\p{M}\p{N}\p{Pc} -]/gu, "")
      .replace(/ /g, "-");
    let slug = base;
    let n = seen.get(base) || 0;
    while (seen.has(slug)) slug = `${base}-${++n}`;
    seen.set(base, n);
    seen.set(slug, 0);
    return slug;
  };
}

// ---------- Links ----------

// Resolve `href` the way GitHub would, relative to the Markdown file it's in.
function resolve(fromFile, href) {
  const parts = fromFile.split("/").slice(0, -1);
  for (const seg of href.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg && seg !== ".") parts.push(seg);
  }
  return parts.join("/");
}

function rewriteLink(a) {
  const href = a.getAttribute("href");
  if (!href || href.startsWith("#")) return;
  if (/^[a-z]+:/i.test(href)) {
    if (!href.startsWith(location.origin)) {
      a.target = "_blank";
      a.rel = "noopener";
    }
    return;
  }
  const [path, hash = ""] = href.split("#");
  const target = resolve(current.file, path);
  const page = BY_FILE.get(target);
  if (page) {
    a.href = `docs.html?p=${page.id}${hash ? "#" + hash : ""}`;
  } else {
    // Source files and folders: send those to GitHub.
    const kind = path.endsWith("/") ? "tree" : "blob";
    a.href = `${REPO}/${kind}/main/${target}${hash ? "#" + hash : ""}`;
    a.target = "_blank";
    a.rel = "noopener";
  }
}

function rewriteImage(img) {
  const src = img.getAttribute("src");
  if (!src || /^[a-z]+:/i.test(src)) return;
  const target = resolve(current.file, src);
  // Only the screenshots are published beside the docs; anything else comes
  // from the repository itself.
  img.src = target.startsWith("docs/screenshots/") ? target : `${REPO}/raw/main/${target}`;
  img.loading = "lazy";
}

// ---------- Rendering ----------

async function render() {
  document.title = `${current.title} · Taurus Docs`;
  document.getElementById("crumb").textContent = current.title;
  document.getElementById("edit").href = `${REPO}/edit/main/${current.file}`;

  const content = document.getElementById("content");
  let md;
  try {
    md = await fetchMarkdown(current);
  } catch (err) {
    content.innerHTML = `<p>Couldn't load this page (${err.message}). It's also
      <a href="${REPO}/blob/main/${current.file}">on GitHub</a>.</p>`;
    return;
  }

  content.innerHTML = marked.parse(md, { gfm: true });

  const slug = slugger();
  content.querySelectorAll("h1, h2, h3, h4, h5, h6").forEach((h) => {
    h.id = slug(h.textContent);
    if (h.tagName !== "H1") {
      const a = document.createElement("a");
      a.className = "anchor";
      a.href = `#${h.id}`;
      a.setAttribute("aria-label", `Link to ${h.textContent}`);
      a.textContent = "#";
      h.prepend(a);
    }
  });
  content.querySelectorAll("a[href]").forEach(rewriteLink);
  content.querySelectorAll("img[src]").forEach(rewriteImage);

  renderNav(content);
  renderPager();
  renderMermaid(content);

  // The browser tried to jump to the hash before the content existed.
  if (location.hash) {
    const el = document.getElementById(decodeURIComponent(location.hash.slice(1)));
    if (el) el.scrollIntoView();
  }
}

function renderNav(content) {
  const list = document.querySelector(".doc-nav");
  list.innerHTML = "";
  for (const page of PAGES) {
    const li = document.createElement("li");
    const a = document.createElement("a");
    a.href = `docs.html?p=${page.id}`;
    a.textContent = page.title;
    li.append(a);
    if (page === current) {
      a.setAttribute("aria-current", "page");
      const toc = document.createElement("ul");
      toc.className = "toc";
      content.querySelectorAll("h2").forEach((h) => {
        const item = document.createElement("li");
        const link = document.createElement("a");
        link.href = `#${h.id}`;
        link.textContent = h.textContent.replace(/^#/, "");
        link.dataset.target = h.id;
        item.append(link);
        toc.append(item);
      });
      if (toc.children.length) li.append(toc);
    }
    list.append(li);
  }
  scrollSpy(content);
}

function scrollSpy(content) {
  const links = new Map([...document.querySelectorAll(".toc a")].map((a) => [a.dataset.target, a]));
  if (!links.size) return;
  const headings = [...content.querySelectorAll("h2")];
  const update = () => {
    let active = headings[0];
    for (const h of headings) {
      if (h.getBoundingClientRect().top < 120) active = h;
      else break;
    }
    links.forEach((a) => a.classList.remove("active"));
    links.get(active.id)?.classList.add("active");
  };
  let raf = 0;
  addEventListener("scroll", () => {
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(update);
  });
  update();
}

function renderPager() {
  const i = PAGES.indexOf(current);
  const prev = PAGES[i - 1];
  const next = PAGES[i + 1];
  const pager = document.getElementById("pager");
  pager.innerHTML =
    (prev ? `<a href="docs.html?p=${prev.id}"><small>Previous</small>${prev.title}</a>` : "<span></span>") +
    (next ? `<a class="next" href="docs.html?p=${next.id}"><small>Next</small>${next.title}</a>` : "");
}

// Mermaid is large, so it loads only on a page that has a diagram.
async function renderMermaid(content) {
  const blocks = content.querySelectorAll("pre > code.language-mermaid");
  if (!blocks.length) return;
  blocks.forEach((code) => {
    const div = document.createElement("div");
    div.className = "mermaid";
    div.textContent = code.textContent;
    code.parentElement.replaceWith(div);
  });
  await new Promise((ok, fail) => {
    const s = document.createElement("script");
    s.src = "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js";
    s.onload = ok;
    s.onerror = fail;
    document.head.append(s);
  }).catch(() => null);
  if (!window.mermaid) return;
  const dark = !matchMedia("(prefers-color-scheme: light)").matches;
  window.mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
  window.mermaid.run({ nodes: content.querySelectorAll(".mermaid") }).catch(() => null);
}

// ---------- Search ----------

// Split each Markdown file into sections at its headings, skipping anything
// inside a code fence, and give each section the same slug render() would.
function sections(page, md) {
  const out = [];
  const slug = slugger();
  let fence = null;
  let cur = { page, title: page.title, slug: "", text: [] };
  for (const line of md.split("\n")) {
    const f = line.match(/^\s*(`{3,}|~{3,})/);
    if (f) {
      if (!fence) fence = f[1][0];
      else if (f[1][0] === fence) fence = null;
      cur.text.push(line);
      continue;
    }
    const h = !fence && line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/);
    if (h) {
      out.push(cur);
      const title = plain(h[2]);
      cur = { page, title, slug: slug(title), text: [] };
    } else {
      cur.text.push(line);
    }
  }
  out.push(cur);
  return out.map((s) => ({ ...s, text: plain(s.text.join(" ")) })).filter((s) => s.text || s.slug);
}

function plain(s) {
  return s
    .replace(/<[^>]+>/g, "")
    // One pass can leave a tag behind ("<scr<b>ipt>"), and this text is meant
    // to be plain, so no angle bracket survives at all.
    .replace(/[<>]/g, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[`*]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

let indexPromise = null;
function buildIndex() {
  indexPromise ??= Promise.all(
    PAGES.map((p) => fetchMarkdown(p).then((md) => sections(p, md)).catch(() => [])),
  ).then((all) => all.flat());
  return indexPromise;
}

const escapeHtml = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

function highlight(text, terms) {
  let html = escapeHtml(text);
  for (const t of terms) {
    html = html.replace(new RegExp(`(${t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "gi"), "<mark>$1</mark>");
  }
  return html;
}

async function search(query) {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const box = document.getElementById("results");
  const nav = document.getElementById("nav");
  if (!terms.length) {
    box.hidden = true;
    nav.hidden = false;
    return;
  }
  const index = await buildIndex();
  const hits = [];
  for (const s of index) {
    const title = s.title.toLowerCase();
    const text = s.text.toLowerCase();
    if (!terms.every((t) => title.includes(t) || text.includes(t))) continue;
    let score = 0;
    for (const t of terms) {
      if (title.includes(t)) score += 20;
      score += Math.min(text.split(t).length - 1, 10);
    }
    hits.push({ s, score });
  }
  hits.sort((a, b) => b.score - a.score);

  const ul = box.querySelector("ul");
  ul.innerHTML = "";
  for (const { s } of hits.slice(0, 25)) {
    const lower = s.text.toLowerCase();
    const at = Math.max(0, Math.min(...terms.map((t) => lower.indexOf(t)).filter((i) => i >= 0), lower.length) - 50);
    const snippet = (at ? "…" : "") + s.text.slice(at, at + 150) + (s.text.length > at + 150 ? "…" : "");
    const li = document.createElement("li");
    li.innerHTML = `<a href="docs.html?p=${s.page.id}${s.slug ? "#" + s.slug : ""}">
      <span class="where">${escapeHtml(s.page.title)}</span>
      ${highlight(s.title, terms)}
      <span class="snip">${highlight(snippet, terms)}</span></a>`;
    ul.append(li);
  }
  if (!hits.length) ul.innerHTML = `<li class="empty">Nothing matches “${escapeHtml(query)}”.</li>`;
  box.hidden = false;
  nav.hidden = true;
}

// ---------- Wiring ----------

document.addEventListener("DOMContentLoaded", () => {
  render();

  const q = document.getElementById("q");
  let timer = 0;
  q.addEventListener("focus", buildIndex, { once: true });
  q.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(() => search(q.value), 120);
  });
  q.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      q.value = "";
      search("");
      q.blur();
    } else if (e.key === "Enter") {
      document.querySelector("#results a")?.click();
    }
  });
  addEventListener("keydown", (e) => {
    if (e.key === "/" && document.activeElement !== q && !/input|textarea/i.test(document.activeElement.tagName)) {
      e.preventDefault();
      q.focus();
    }
  });

  // Same-page anchors from search results or the TOC: let the browser scroll,
  // but close the mobile menu.
  const sidebar = document.getElementById("sidebar");
  const toggle = document.querySelector(".nav-toggle");
  toggle.addEventListener("click", () => {
    const open = sidebar.classList.toggle("open");
    toggle.setAttribute("aria-expanded", String(open));
  });
  sidebar.addEventListener("click", (e) => {
    if (e.target.closest("a")) {
      sidebar.classList.remove("open");
      toggle.setAttribute("aria-expanded", "false");
    }
  });
});
