// Run with: PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs CHROME_BIN=/path/to/chrome node tests/browser/file-tree-guides.mjs
// Checks the production file-tree CSS against rendered folder geometry.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');

const html = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const cssStart = html.indexOf('.file-tree { display: grid; gap: 1px; }');
const cssEndMarker = 'body.files-collapsed .file-folder-children > *::before { display: none; }';
const cssEnd = html.indexOf(cssEndMarker, cssStart);
assert.ok(cssStart >= 0 && cssEnd > cssStart, 'file-tree CSS is present');
const treeCss = html.slice(cssStart, cssEnd + cssEndMarker.length);
function cssRule(selector) {
  const start = html.indexOf(selector);
  assert.ok(start >= 0, `production CSS rule is present: ${selector}`);
  if (selector.includes('}')) return selector;
  const end = html.indexOf('\n  }', start);
  assert.ok(end > start, `production CSS rule closes: ${selector}`);
  return html.slice(start, end + 4);
}
const diffCss = [
  cssRule('#doc-content .pre-wrap {'),
  cssRule('body.diff-file #doc-content .pre-wrap {'),
  cssRule('body.diff-file #doc-content .pre-wrap pre { border-radius: 5px; }'),
  cssRule('.thread-marker {'),
].join('\n');

const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN, args: ['--no-sandbox'], headless: true });
try {
  const page = await browser.newPage();
  await page.setContent(`<style>:root { --line: #000; --left-gutter: 44px; } body { margin: 0; } ${treeCss}${diffCss}</style>
    <body class="diff-file">
    <div class="file-folder-children">
      <details class="file-folder"><summary>First folder</summary></details>
      <details class="file-folder" open>
        <summary id="last-summary">Last folder</summary>
        <div class="file-folder-children"><div class="file-item">Nested file</div></div>
      </details>
    </div>
    <div id="doc-content" style="width: 200px; margin-top: 100px;">
      <div class="pre-wrap" id="diff-wrapper" style="width: 200px; height: 60px;">
        <pre><code>diff line</code></pre><span class="thread-marker kind-user" id="marker">1</span>
      </div>
    </div>
    </body>`);

  const initial = await page.locator('#last-summary').evaluate(summary => ({
    rowHeight: summary.getBoundingClientRect().height,
    guideHeight: parseFloat(getComputedStyle(summary, '::before').height),
  }));
  assert.ok(initial.guideHeight > 0, 'the last-folder guide is rendered');
  assert.ok(Math.abs(initial.guideHeight - initial.rowHeight / 2) < 1, `guide follows half of the summary: ${JSON.stringify(initial)}`);

  const scaled = await page.locator('#last-summary').evaluate(summary => {
    summary.style.fontSize = '24px';
    summary.style.lineHeight = '1.2';
    summary.style.paddingBlock = '12px';
    return {
      rowHeight: summary.getBoundingClientRect().height,
      guideHeight: parseFloat(getComputedStyle(summary, '::before').height),
    };
  });
  assert.ok(scaled.rowHeight > initial.rowHeight, 'font scaling grows the folder summary');
  assert.ok(Math.abs(scaled.guideHeight - scaled.rowHeight / 2) < 1, `guide follows the scaled summary: ${JSON.stringify(scaled)}`);

  const markerVisibleOutsideWrapper = await page.evaluate(() => {
    const wrapper = document.getElementById('diff-wrapper');
    const marker = document.getElementById('marker');
    const box = wrapper.getBoundingClientRect();
    const mark = marker.getBoundingClientRect();
    const x = mark.left + mark.width / 2;
    const y = mark.top + mark.height / 2;
    return {
      overflow: getComputedStyle(wrapper).overflow,
      extendsOutside: mark.left >= box.right,
      hitTested: document.elementFromPoint(x, y) === marker,
    };
  });
  assert.equal(markerVisibleOutsideWrapper.overflow, 'visible');
  assert.ok(markerVisibleOutsideWrapper.extendsOutside, 'gutter marker extends beyond the diff wrapper');
  assert.ok(markerVisibleOutsideWrapper.hitTested, `gutter marker is not clipped: ${JSON.stringify(markerVisibleOutsideWrapper)}`);
  console.log(`PASS: folder guide tracks summary midpoint (${initial.rowHeight}px -> ${scaled.rowHeight}px); diff gutter marker remains visible outside its wrapper`);
} finally {
  await browser.close();
}
