// Run: PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs CHROME_BIN=/path/to/chrome node tests/browser/thread-selection-quotes-e2e.mjs
// Isolates the production selection code in a real browser; no server required.
import assert from 'node:assert/strict';
import { selectionSource as source, clipboardSource, clickSource, completedSource } from './fixtures/thread-selection-source.mjs';
import http from 'node:http';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');

const server = http.createServer((req, res) => {
  res.setHeader('Content-Type', 'text/html');
  res.end(`<style>.thread{margin:80px}.user-comment{white-space:pre-wrap}.meta{font-size:10px}</style>
    <div class="thread open"><div class="user-comment" data-kind="reply">User first\nUser second<div class="meta">timestamp</div></div>
    <div class="user-comment" data-kind="take">Agent response<div class="meta">timestamp</div></div>
    <div class="thread-take">Prepopulated agent take</div>
    <div class="followup"><textarea></textarea></div></div>
    <script>let reviewComplete = false; window.inputs = []; const showAllBtn = { classList: { contains: () => false } }; function closeNewThreadEditor() {} function closeAllThreads() { document.querySelector('.thread').classList.remove('open'); } document.querySelector('textarea').addEventListener('input', e => inputs.push(e.target.value));${clipboardSource}${source}${clickSource}${completedSource} document.addEventListener('click', blockCompletedInteraction, true);</script>`);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN, args: ['--no-sandbox'], headless: true });
try {
  const context = await browser.newContext({ permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  const menu = page.locator('#thread-selection-actions');
  async function select(selector) {
    await page.locator(selector).evaluate(el => {
      const range = document.createRange();
      range.selectNodeContents(el.firstChild);
      const selection = getSelection();
      selection.removeAllRanges(); selection.addRange(range);
    });
    await menu.waitFor({ state: 'visible' });
  }
  await select('[data-kind="reply"]');
  await menu.getByRole('button', { name: 'Copy', exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'User first\nUser second');
  await menu.waitFor({ state: 'hidden' });
  await page.locator('textarea').fill('Existing draft');
  await select('[data-kind="reply"]');
  await menu.getByRole('button', { name: 'Add to chat' }).click();
  let expected = 'Existing draft\n\n> User first\n> User second\n\n';
  assert.equal(await page.locator('textarea').inputValue(), expected);
  await select('[data-kind="take"]');
  await menu.getByRole('button', { name: 'Add to chat' }).click();
  expected += '> Agent response\n\n';
  assert.deepEqual(await page.locator('textarea').evaluate(el => [el.value, el.selectionStart, el.selectionEnd, document.activeElement === el]), [expected, expected.length, expected.length, true]);
  assert.equal(await page.evaluate(() => inputs.at(-1)), expected);
  await page.keyboard.type('My reply');
  assert.equal(await page.locator('textarea').inputValue(), expected + 'My reply');
  await select('.thread-take');
  await page.keyboard.press('Escape');
  await menu.waitFor({ state: 'hidden' });
  await select('[data-kind="reply"]');
  await page.evaluate(() => getSelection().removeAllRanges());
  await menu.waitFor({ state: 'hidden' });
  // A range ending exactly at metadata start must remain quotable.
  await page.locator('[data-kind="reply"]').evaluate(el => {
    const range = document.createRange(); range.setStart(el.firstChild, 0); range.setEnd(el.querySelector('.meta'), 0);
    getSelection().removeAllRanges(); getSelection().addRange(range);
  });
  await menu.waitFor({ state: 'visible' });
  // A later Escape listener must not collapse the thread.
  await page.evaluate(() => document.addEventListener('keydown', e => { if (e.key === 'Escape') closeAllThreads(); }));
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.thread').evaluate(el => el.classList.contains('open')), true);
  // Late clipboard completion must not dismiss a newer selection.
  await page.evaluate(() => { window.originalCopy = navigator.clipboard.writeText.bind(navigator.clipboard); navigator.clipboard.writeText = () => new Promise(resolve => window.finishCopy = resolve); });
  await select('[data-kind="reply"]');
  await menu.getByRole('button', { name: 'Copy', exact: true }).click();
  await select('[data-kind="take"]');
  await page.evaluate(() => finishCopy());
  await page.waitForTimeout(100);
  assert.equal(await menu.isVisible(), true);
  await page.evaluate(() => navigator.clipboard.writeText = window.originalCopy);
  // Replaced/detached reply controls cannot receive a stale quote.
  await page.evaluate(() => { const ta = document.querySelector('textarea'); ta.replaceWith(ta.cloneNode()); });
  const before = await page.locator('textarea').inputValue();
  await menu.getByRole('button', { name: 'Add to chat' }).click();
  assert.equal(await page.locator('textarea').inputValue(), before);
  await page.evaluate(() => { reviewComplete = true; document.querySelectorAll('button, textarea').forEach(el => el.disabled = true); });
  await select('[data-kind="reply"]');
  assert.equal(await menu.getByRole('button', { name: 'Add to chat' }).isDisabled(), true);
  await menu.getByRole('button', { name: 'Copy', exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'User first\nUser second');
  await page.evaluate(() => Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true }));
  await select('[data-kind="take"]');
  await menu.getByRole('button', { name: 'Copy', exact: true }).click();
  await menu.waitFor({ state: 'hidden' });
  await page.evaluate(() => { window.originalExec = document.execCommand.bind(document); document.execCommand = () => false; });
  await select('[data-kind="reply"]');
  await menu.getByRole('button', { name: 'Copy', exact: true }).click();
  await page.waitForTimeout(150);
  assert.equal(await menu.isVisible(), true);
  assert.match(await menu.getByRole('status').textContent(), /Copy failed/);
  await page.evaluate(() => document.execCommand = window.originalExec);
  await menu.getByRole('button', { name: 'Copy', exact: true }).click();
  await menu.waitFor({ state: 'hidden' });
  assert.deepEqual(errors, []);
  console.log('PASS: real-browser selection, raw clipboard, user/agent quotes, append, blank lines, focus/caret, input event, dismissal');
} finally {
  await browser.close();
  server.close();
}
