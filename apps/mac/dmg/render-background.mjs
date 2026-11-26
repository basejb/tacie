// Renders dmg/background.html at 1× and 2× and packs both into one TIFF, so Finder draws the window sharp on
// Retina and on 1× screens alike. Playwright comes from the site's packages (run `pnpm install` in site/ once).
// usage: node dmg/render-background.mjs [out.tiff]   (default: dmg/background.tiff, which make-dmg.sh uses)
import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const here = new URL(".", import.meta.url);
const { chromium } = await import(new URL("../../../site/node_modules/@playwright/test/index.mjs", here).href);
const out = process.argv[2] ?? new URL("background.tiff", here).pathname;

const page = pathToFileURL(new URL("background.html", here).pathname).href;
const dir = mkdtempSync(join(tmpdir(), "tacie-dmg-bg-"));
const browser = await chromium.launch();
for (const scale of [1, 2]) {
  const p = await browser.newPage({ viewport: { width: 640, height: 400 }, deviceScaleFactor: scale });
  await p.goto(page);
  await p.screenshot({ path: join(dir, `bg@${scale}x.png`) });
  await p.close();
}
await browser.close();
execFileSync("tiffutil", ["-cathidpicheck", join(dir, "bg@1x.png"), join(dir, "bg@2x.png"), "-out", join(dir, "bg.tiff")]);
execFileSync("tiffutil", ["-lzw", join(dir, "bg.tiff"), "-out", out]);   // lossless; the raw TIFF is about 5 MB
console.log(out);
