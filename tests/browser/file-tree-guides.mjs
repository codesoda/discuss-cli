// Run with: PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs CHROME_BIN=/path/to/chrome node tests/browser/file-tree-guides.mjs
// Checks the production file-tree CSS against rendered folder geometry.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');

const html = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
// Load the whole stylesheet so selector suffixes cannot select the wrong rule,
// and the assertions exercise the production cascade rather than isolated rules.
const styles = [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/g)].map(match => match[1]).join('\n');
assert.ok(styles.length > 0, 'production stylesheet is present');

const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN, args: ['--no-sandbox'], headless: true });
try {
  const page = await browser.newPage();
  await page.setContent(`<style>:root { --line: #000; --left-gutter: 44px; } body { margin: 0; } ${styles}</style>
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
  await page.evaluate(() => document.body.classList.add('files-collapsed'));
  const collapsedGuides = await page.locator('.file-folder-children > *').evaluateAll(children =>
    children.every(child => getComputedStyle(child, '::before').display === 'none')
  );
  assert.ok(collapsedGuides, 'collapsed sidebar hides direct-child guides');
  assert.equal(await page.locator('#last-summary').evaluate(summary =>
    getComputedStyle(summary).display), 'none', 'collapsed sidebar hides summaries and their guides');
  console.log(`PASS: folder guide tracks summary midpoint (${initial.rowHeight}px -> ${scaled.rowHeight}px); diff gutter marker remains visible outside its wrapper`);
} finally {
  await browser.close();
}
