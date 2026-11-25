// Renders caption strip PNGs (1280x800, transparent) for ffmpeg overlay.
import { chromium } from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const OUT = path.join(path.dirname(fileURLToPath(import.meta.url)), 'captions');
const SYSTEM_CHROME = process.env.CHROME_BIN
  || (process.platform === 'darwin' && fs.existsSync('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
    ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
    : null);
fs.mkdirSync(OUT, { recursive: true });

const captions = {
  scene01: 'One command. Three safe, self-contained review scenarios.',
  scene02: 'Tour markdown, diffs, images, and prototypes with a canned Demo agent.',
  scene03: 'Review a realistic synthetic PR with imported discussion and nested files.',
  scene04: 'Mark a changed file Viewed and continue to the next unviewed diff.',
  scene05: 'Finish Review exposes editable summaries, destinations, and include choices.',
  scene06: 'Confirm exact GFM, then simulate locally — nothing reaches GitHub.',
  scene07: 'Inspect a bundled running app through the production live proxy.',
  scene08: 'Root assets, an app API, pushState, and popstate stay on the app origin.',
  scene09: 'Anchor a thread to an app element; its marker and canned response appear.',
  scene10: 'No gh, no LLM, no history, and no public network required.',
};

const browser = await chromium.launch(SYSTEM_CHROME ? { executablePath: SYSTEM_CHROME } : {});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
for (const [name, text] of Object.entries(captions)) {
  await page.setContent(`
    <body style="margin:0;width:1280px;height:800px;background:transparent;">
      <div style="position:absolute;left:0;right:0;bottom:30px;display:flex;justify-content:center;">
        <div style="font-family:-apple-system,'Helvetica Neue',sans-serif;font-size:30px;font-weight:600;
                    color:#fff;background:rgba(10,14,22,.72);padding:14px 28px;border-radius:12px;
                    letter-spacing:.2px;text-shadow:0 1px 2px rgba(0,0,0,.5);">${text}</div>
      </div>
    </body>`);
  await page.screenshot({ path: path.join(OUT, `${name}.png`), omitBackground: true });
}
await browser.close();
console.log('captions rendered:', Object.keys(captions).length);
