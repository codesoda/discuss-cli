// Renders caption strip PNGs (1280x800, transparent) for ffmpeg overlay.
import { chromium } from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const OUT = path.join(path.dirname(fileURLToPath(import.meta.url)), 'captions');
fs.mkdirSync(OUT, { recursive: true });

const captions = {
  scene01: 'Ask your agent to discuss a doc.',
  scene02: 'Click a paragraph. Drop a comment. The agent replies in the margin.',
  scene03: 'Review many files in one session. Badges track open threads.',
  scene04: 'Run discuss diff. Comment on exact lines of the staged diff.',
  scene05: 'Review images. Pins anchor threads to a spot.',
  scene06: 'Review HTML prototypes. Click an element to anchor a thread.',
  scene07: 'The agent pre-annotates its own edits. A guided review.',
  scene08: 'The doc updates live. Threads survive the rewrite.',
  scene09: 'End with a verdict. Feedback is required when you decline.',
  scene10: 'The agent gets the transcript and the verdict.',
};

const browser = await chromium.launch();
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
