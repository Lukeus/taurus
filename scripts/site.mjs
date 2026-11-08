// Assembles the project site into `_site/`, the folder GitHub Pages publishes.
//
//   node scripts/site.mjs           build
//   node scripts/site.mjs --serve   build, then serve it at http://localhost:4173
//
// Nothing is generated: the site is `site/` plus the Markdown it renders in
// the browser (README.md and docs/*.md) and the screenshots those files show.
// Copying rather than converting is the point. The docs stay the one source,
// and a docs change reaches the site without anyone touching `site/`.

import { cp, mkdir, readdir, rm, readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(import.meta.url), "../..");
const out = join(root, "_site");

await rm(out, { recursive: true, force: true });
await cp(join(root, "site"), out, { recursive: true });
await mkdir(join(out, "docs"), { recursive: true });

for (const name of await readdir(join(root, "docs"))) {
  if (name.endsWith(".md")) await cp(join(root, "docs", name), join(out, "docs", name));
}
await cp(join(root, "docs/screenshots"), join(out, "docs/screenshots"), { recursive: true });
await cp(join(root, "README.md"), join(out, "README.md"));

console.log(`Built ${out}`);

if (process.argv.includes("--serve")) {
  const types = {
    ".html": "text/html; charset=utf-8",
    ".css": "text/css",
    ".js": "text/javascript",
    ".md": "text/markdown; charset=utf-8",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".webp": "image/webp",
    ".jpg": "image/jpeg",
  };
  const port = Number(process.env.PORT) || 4173;
  createServer(async (req, res) => {
    const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^(\.\.[/\\])+/, "");
    let file = join(out, path);
    try {
      if ((await stat(file)).isDirectory()) file = join(file, "index.html");
      res.writeHead(200, { "content-type": types[extname(file)] || "application/octet-stream" });
      res.end(await readFile(file));
    } catch {
      res.writeHead(404).end("Not found");
    }
  }).listen(port, () => console.log(`Serving at http://localhost:${port}`));
}
