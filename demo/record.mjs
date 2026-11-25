// Records browser scenes 2-9 from docs/demo-script.md with Playwright.
// Every scene starts the real bundled `discuss demo` launcher; no external app,
// GitHub access, authentication, or model is involved.
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BIN = path.join(ROOT, 'target', 'release', 'discuss');
const CLIPS = path.join(ROOT, 'demo', 'clips');
const CAPS = path.join(ROOT, 'demo', 'captures');
const SYSTEM_CHROME = process.env.CHROME_BIN
  || (process.platform === 'darwin' && fs.existsSync('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
    ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
    : null);
fs.mkdirSync(CLIPS, { recursive: true });
fs.mkdirSync(CAPS, { recursive: true });
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const activeServers = [];
function startServer({ captureFile = null } = {}) {
  return new Promise((resolve, reject) => {
    const proc = spawn(BIN, ['--no-open', '--no-save', 'demo'], { cwd: ROOT });
    activeServers.push(proc);
    const events = [];
    const waiters = [];
    let buffer = '';
    let stderr = '';
    const capture = captureFile ? fs.createWriteStream(captureFile) : null;
    proc.stdout.on('data', data => {
      capture?.write(data);
      buffer += data.toString();
      let newline;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
        if (!line.trim()) continue;
        try {
          const event = JSON.parse(line); events.push(event); waiters.forEach(waiter => waiter(event));
        } catch {}
      }
    });
    proc.stderr.on('data', data => { stderr += data.toString(); });
    proc.on('exit', code => {
      capture?.end();
      if (!events.some(event => event.kind === 'session.started')) reject(new Error(`discuss exited (${code}) before readiness: ${stderr}`));
    });
    const server = {
      proc, events,
      waitFor(kind, timeoutMs = 15000) {
        const existing = events.find(event => event.kind === kind);
        if (existing) return Promise.resolve(existing);
        return new Promise((res, rej) => {
          const timer = setTimeout(() => rej(new Error(`timeout waiting for ${kind}`)), timeoutMs);
          const waiter = event => {
            if (event.kind !== kind) return;
            clearTimeout(timer); waiters.splice(waiters.indexOf(waiter), 1); res(event);
          };
          waiters.push(waiter);
        });
      },
      stop() { try { proc.kill('SIGTERM'); } catch {} },
    };
    server.waitFor('session.started').then(event => {
      server.url = event.payload.url;
      server.payload = event.payload;
      resolve(server);
    }).catch(reject);
  });
}
async function waitForPr(server) {
  for (let attempt = 0; attempt < 120; attempt++) {
    const response = await fetch(`${server.payload.examplePrUrl}/api/state`);
    if (response.ok && (await response.json()).prSession?.phase === 'reviewing') return;
    await sleep(50);
  }
  throw new Error('demo PR did not import');
}

const CURSOR = `(() => {
  const add = () => {
    if (document.getElementById('pw-cursor') || !document.body) return;
    const cursor = document.createElement('div'); cursor.id = 'pw-cursor';
    cursor.style.cssText = 'position:fixed;width:20px;height:20px;border-radius:50%;background:rgba(37,99,235,.30);border:2px solid rgba(37,99,235,.95);box-shadow:0 1px 4px rgba(0,0,0,.3);z-index:2147483647;pointer-events:none;left:-60px;top:-60px;transform:translate(-50%,-50%);transition:transform .08s;';
    document.body.appendChild(cursor);
    addEventListener('mousemove', event => { cursor.style.left=event.clientX+'px'; cursor.style.top=event.clientY+'px'; }, true);
    addEventListener('mousedown', () => cursor.style.transform='translate(-50%,-50%) scale(.7)', true);
    addEventListener('mouseup', () => cursor.style.transform='translate(-50%,-50%) scale(1)', true);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', add); else add();
})();`;

let browser;
async function newScene() {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    recordVideo: { dir: CLIPS, size: { width: 1280, height: 800 } },
  });
  await context.addInitScript(CURSOR);
  const page = await context.newPage();
  return {
    page,
    async finish(name) {
      const video = page.video(); await context.close();
      fs.renameSync(await video.path(), path.join(CLIPS, name));
      console.log('saved', name);
    },
  };
}
async function moveClick(page, locator) {
  const box = await locator.boundingBox();
  if (!box) throw new Error('target has no bounding box');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 22 });
  await sleep(220); await page.mouse.down(); await sleep(60); await page.mouse.up();
}
async function openScenario(page, server, key) {
  const url = key === 'tour' ? server.url : key === 'pr' ? server.payload.examplePrUrl : server.payload.localAppReviewUrl;
  if (key === 'pr') await waitForPr(server);
  await page.goto(url);
  await page.waitForSelector('#demo-scenarios a', { timeout: 15000 });
  await sleep(1000);
}

async function scene02() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'tour');
  await moveClick(page, page.locator('.file-item[title="plan.md"]'));
  await sleep(700); await moveClick(page, page.locator('.thread-marker.kind-pending').first());
  await sleep(2300); server.stop(); await finish('scene02.webm');
}
async function scene03() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'pr');
  await page.waitForSelector('.file-item[title="src/payments/retry.ts"]');
  await sleep(1300); await moveClick(page, page.locator('.file-item[title="src/payments/retry.ts"]'));
  await sleep(700); await moveClick(page, page.locator('.thread-marker[data-thread-id="gh-review-thread-900003"]'));
  await sleep(2300); server.stop(); await finish('scene03.webm');
}
async function scene04() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'pr');
  const file = page.locator('.file-item[title="docs/operations/retry-runbook.md"]');
  await moveClick(page, file); await page.waitForSelector('.pr-file-viewed-control input');
  await sleep(900); await moveClick(page, page.locator('.pr-file-viewed-control input'));
  await sleep(2600); server.stop(); await finish('scene04.webm');
}
async function scene05() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'pr');
  await moveClick(page, page.locator('#finish-review'));
  await page.waitForSelector('#pr-review-summary');
  await sleep(900); await moveClick(page, page.locator('.pr-item-include-checkbox:not(:disabled)').first());
  const summary = page.locator('#pr-review-summary');
  await summary.fill(`${await summary.inputValue()} Ready after the Stage 2 capacity check.`);
  await sleep(2500); server.stop(); await finish('scene05.webm');
}
async function scene06() {
  const server = await startServer({ captureFile: path.join(CAPS, 'scene6.ndjsonl') });
  const { page, finish } = await newScene(); await openScenario(page, server, 'pr');
  await page.locator('#finish-review').click(); await page.waitForSelector('#pr-review-summary');
  await page.locator('.pr-item-include-checkbox:not(:disabled)').first().check();
  const summary = page.locator('#pr-review-summary');
  await summary.fill(`${await summary.inputValue()} Recording-verified edit.`);
  await moveClick(page, page.locator('#pr-dialog-actions .pr-button.primary'));
  await page.waitForSelector('#pr-preview-gfm'); await sleep(2400);
  await moveClick(page, page.locator('#pr-dialog-actions .pr-button.primary'));
  await page.waitForSelector('#done-banner.shown'); await sleep(2200);
  await finish('scene06.webm');
}
async function scene07() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'local-app');
  const frame = page.frameLocator('.prototype-frame');
  await frame.locator('body[data-demo-api-loaded="true"]').waitFor({ timeout: 15000 });
  await sleep(3200); server.stop(); await finish('scene07.webm');
}
async function scene08() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'local-app');
  const frame = page.frameLocator('.prototype-frame');
  await frame.locator('body[data-demo-api-loaded="true"]').waitFor({ timeout: 15000 });
  await page.locator('#inspect-toggle').click();
  await moveClick(page, frame.locator('a[data-route="/payments"]'));
  await page.waitForFunction(() => document.querySelector('.live-route-value')?.textContent === '/payments');
  await sleep(1200); await frame.locator('body').evaluate(() => history.back());
  await page.waitForFunction(() => document.querySelector('.live-route-value')?.textContent === '/');
  await sleep(2200); server.stop(); await finish('scene08.webm');
}
async function scene09() {
  const server = await startServer(); const { page, finish } = await newScene();
  await openScenario(page, server, 'local-app');
  const frame = page.frameLocator('.prototype-frame');
  await frame.locator('body[data-demo-api-loaded="true"]').waitFor({ timeout: 15000 });
  await page.locator('#inspect-toggle').click();
  await frame.locator('a[data-route="/payments"]').click();
  await page.waitForFunction(() => document.querySelector('.live-route-value')?.textContent === '/payments');
  await page.locator('#inspect-toggle').click();
  await moveClick(page, frame.locator('#deploy-card'));
  const textarea = page.locator('.html-thread-editor textarea'); await textarea.waitFor();
  await textarea.pressSequentially('Keep the rollback control visible here.', { delay: 32 });
  await moveClick(page, page.locator('.html-thread-editor .save'));
  await page.waitForSelector('.element-thread .user-comment[data-kind="take"]', { timeout: 7000 });
  await sleep(2400); server.stop(); await finish('scene09.webm');
}

const wanted = process.argv.slice(2);
const scenes = { scene02, scene03, scene04, scene05, scene06, scene07, scene08, scene09 };
const run = wanted.length ? wanted : Object.keys(scenes);
browser = await chromium.launch(SYSTEM_CHROME ? { executablePath: SYSTEM_CHROME } : {});
let failed = false;
for (const name of run) {
  try { console.log('recording', name, '...'); await scenes[name](); }
  catch (error) { failed = true; console.error(`FAILED ${name}:`, error.message); }
  finally { for (const process of activeServers.splice(0)) try { process.kill('SIGKILL'); } catch {} }
}
await browser.close();
process.exit(failed ? 1 : 0);
