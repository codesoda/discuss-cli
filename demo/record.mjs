// Records browser scenes 2-9 of docs/demo-script.md with Playwright.
// Each scene: fresh discuss server + fresh browser context -> demo/clips/sceneNN.webm
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BIN = path.join(ROOT, 'target', 'release', 'discuss');
const CLIPS = path.join(ROOT, 'demo', 'clips');
const CAPS = path.join(ROOT, 'demo', 'captures');
const FIX = path.join(ROOT, 'docs', 'demo-fixtures');
fs.mkdirSync(CLIPS, { recursive: true });
fs.mkdirSync(CAPS, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------- discuss server ----------
const activeServers = [];
function startServer(args, { cwd = ROOT, captureFile = null } = {}) {
  return new Promise((resolve, reject) => {
    const proc = spawn(BIN, ['--no-open', '--no-save', ...args], { cwd });
    activeServers.push(proc);
    const events = [];
    const waiters = [];
    let buf = '';
    const cap = captureFile ? fs.createWriteStream(captureFile) : null;
    proc.stdout.on('data', (d) => {
      if (cap) cap.write(d);
      buf += d.toString();
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        if (!line.trim()) continue;
        try {
          const ev = JSON.parse(line);
          events.push(ev);
          waiters.forEach((w) => w(ev));
        } catch {}
      }
    });
    let stderr = '';
    proc.stderr.on('data', (d) => (stderr += d.toString()));
    proc.on('exit', (code) => {
      if (!events.find((e) => e.kind === 'session.started')) {
        reject(new Error(`discuss exited (${code}) before session.started: ${stderr}`));
      }
    });
    const server = {
      proc,
      events,
      exited: new Promise((r) => proc.on('exit', r)),
      waitFor(kind, timeoutMs = 15000) {
        const seen = events.filter((e) => e.kind === kind);
        if (server._cursors === undefined) server._cursors = {};
        const cursor = server._cursors[kind] || 0;
        if (seen.length > cursor) {
          server._cursors[kind] = cursor + 1;
          return Promise.resolve(seen[cursor]);
        }
        return new Promise((res, rej) => {
          const t = setTimeout(() => rej(new Error(`timeout waiting for ${kind}`)), timeoutMs);
          const w = (ev) => {
            if (ev.kind === kind) {
              clearTimeout(t);
              waiters.splice(waiters.indexOf(w), 1);
              server._cursors[kind] = (server._cursors[kind] || 0) + 1;
              res(ev);
            }
          };
          waiters.push(w);
        });
      },
      stop() {
        try { proc.kill('SIGTERM'); } catch {}
      },
    };
    server
      .waitFor('session.started', 15000)
      .then((ev) => {
        server.url = ev.payload.url;
        server.endpoints = ev.payload.endpoints;
        resolve(server);
      })
      .catch(reject);
  });
}

async function api(url, method = 'GET', body = null) {
  const res = await fetch(url, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : null,
  });
  if (!res.ok) throw new Error(`${method} ${url} -> ${res.status}: ${await res.text()}`);
  return res.json().catch(() => ({}));
}

// ---------- browser ----------
const CURSOR = `
(() => {
  const add = () => {
    if (document.getElementById('pw-cursor') || !document.body) return;
    const c = document.createElement('div');
    c.id = 'pw-cursor';
    c.style.cssText = 'position:fixed;width:20px;height:20px;border-radius:50%;background:rgba(37,99,235,.30);border:2px solid rgba(37,99,235,.95);box-shadow:0 1px 4px rgba(0,0,0,.3);z-index:2147483647;pointer-events:none;left:-60px;top:-60px;transform:translate(-50%,-50%);transition:transform .08s;';
    document.body.appendChild(c);
    window.addEventListener('mousemove', (e) => { c.style.left = e.clientX + 'px'; c.style.top = e.clientY + 'px'; }, true);
    window.addEventListener('mousedown', () => { c.style.transform = 'translate(-50%,-50%) scale(.7)'; }, true);
    window.addEventListener('mouseup', () => { c.style.transform = 'translate(-50%,-50%) scale(1)'; }, true);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', add);
  else add();
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
      const video = page.video();
      await context.close();
      const p = await video.path();
      fs.renameSync(p, path.join(CLIPS, name));
      console.log('saved', name);
    },
  };
}

async function moveClick(page, locator, opts = {}) {
  const box = await locator.boundingBox();
  if (!box) throw new Error('no bounding box');
  const x = box.x + (opts.rx ?? 0.5) * box.width;
  const y = box.y + (opts.ry ?? 0.5) * box.height;
  await page.mouse.move(x, y, { steps: 22 });
  await sleep(250);
  await page.mouse.down();
  await sleep(60);
  await page.mouse.up();
}

async function typeInto(page, locator, text) {
  await locator.pressSequentially(text, { delay: 34 });
}

async function openDoc(page, url) {
  await page.goto(url);
  await page.waitForSelector('#doc-content [data-anchor-idx]', { state: 'attached', timeout: 15000 });
  await page.waitForSelector('#doc-content p', { timeout: 15000 });
  await sleep(1500); // Prism + reposition settle
}

// ---------- scenes ----------

// Scene 2: single doc, comment, agent take appears
async function scene02() {
  const s = await startServer([path.join(FIX, 'plan.md'), '--port', '5761']);
  const { page, finish } = await newScene();
  await openDoc(page, s.url);
  const para = page.locator('#doc-content p', { hasText: '2.1% failure rate' }).first();
  await moveClick(page, para);
  const ta = page.locator('.new-thread-editor textarea');
  await ta.waitFor();
  await typeInto(page, ta, 'This claim needs a source.');
  await sleep(300);
  await moveClick(page, page.locator('.new-thread-editor button.primary.save'));
  const ev = await s.waitFor('thread.created');
  await sleep(900);
  await api(s.endpoints.addTakeTemplate.replace('{threadId}', ev.payload.id), 'POST', {
    text: 'Source is the March incident review (PAY-1042): 2.1% of charge attempts failed during the brownout window. I will add the citation to this paragraph.',
  });
  await page.waitForSelector('.thread.open .user-comment[data-kind="take"]', { timeout: 8000 });
  await sleep(2600);
  s.stop();
  await finish('scene02.webm');
}

// Scene 3: multi-file, sidebar badges
async function scene03() {
  const s = await startServer([
    path.join(FIX, 'plan.md'), path.join(FIX, 'notes.md'), path.join(FIX, 'todo.md'),
    '--port', '5762',
  ]);
  const { page, finish } = await newScene();
  await openDoc(page, s.url);
  await sleep(600);
  await moveClick(page, page.locator('.file-item[data-file-id="f-2"]'));
  await sleep(900);
  const para = page.locator('#doc-content p', { hasText: 'rate-limits' }).first();
  await moveClick(page, para);
  const ta = page.locator('.new-thread-editor textarea');
  await ta.waitFor();
  await typeInto(page, ta, 'Do we throttle retries per merchant?');
  await sleep(250);
  await moveClick(page, page.locator('.new-thread-editor button.primary.save'));
  await s.waitFor('thread.created');
  await sleep(800);
  // hover the sidebar so the badge is in focus, then back to plan.md
  await moveClick(page, page.locator('.file-item[data-file-id="f-1"]'));
  await sleep(1500);
  s.stop();
  await finish('scene03.webm');
}

// Scene 4: discuss diff, line-anchored comment
async function scene04() {
  const s = await startServer(['--port', '5763', 'diff'], { cwd: path.join(FIX, 'demo-repo') });
  const { page, finish } = await newScene();
  await page.goto(s.url);
  await page.waitForSelector('#doc-content .pre-wrap pre.line-numbers .line-numbers-rows > span', { timeout: 20000 });
  await sleep(1500);
  // find the row index of the added jitter line inside the first hunk
  const lineIdx = await page.evaluate(() => {
    const pre = document.querySelector('#doc-content .pre-wrap pre.line-numbers');
    const code = pre.querySelector('code');
    const lines = code.textContent.split('\n');
    let i = lines.findIndex((l) => l.includes('base + jitter_ms(base)'));
    if (i < 0) i = lines.findIndex((l) => l.startsWith('+'));
    return i + 1; // nth-child is 1-based
  });
  const row = page.locator(`#doc-content .pre-wrap pre.line-numbers .line-numbers-rows > span:nth-child(${lineIdx})`).first();
  await row.scrollIntoViewIfNeeded();
  await sleep(400);
  await moveClick(page, row);
  const ta = page.locator('.new-thread-editor textarea');
  await ta.waitFor();
  await typeInto(page, ta, 'Cap total backoff. Three retries at full jitter can exceed the provider timeout.');
  await sleep(250);
  await moveClick(page, page.locator('.new-thread-editor button.primary.save'));
  await s.waitFor('thread.created');
  await page.waitForSelector('.line-numbers-rows > span.has-thread-line', { timeout: 8000 });
  await sleep(2000);
  s.stop();
  await finish('scene04.webm');
}

// Scene 5: image pin
async function scene05() {
  const s = await startServer([path.join(FIX, 'mockup.png'), '--port', '5764']);
  const { page, finish } = await newScene();
  await page.goto(s.url);
  const img = page.locator('#doc-content .image-review img');
  await img.waitFor();
  await page.waitForFunction(() => {
    const i = document.querySelector('#doc-content .image-review img');
    return i && i.complete && i.naturalWidth > 0;
  });
  await sleep(1200);
  await moveClick(page, img, { rx: 0.045, ry: 0.04 }); // logo area, top-left
  const ta = page.locator('.new-thread-editor textarea');
  await ta.waitFor();
  await typeInto(page, ta, 'Logo is too small at this size.');
  await sleep(250);
  await moveClick(page, page.locator('.new-thread-editor button.primary.save'));
  await s.waitFor('thread.created');
  await page.waitForSelector('button.image-pin-marker', { timeout: 8000 });
  await sleep(1800);
  s.stop();
  await finish('scene05.webm');
}

// Scene 6: HTML prototype inspect
async function scene06() {
  const s = await startServer([path.join(ROOT, 'examples', 'prototype.html'), '--port', '5765']);
  const { page, finish } = await newScene();
  await page.goto(s.url);
  const frame = page.frameLocator('.html-review:not([hidden]) iframe.prototype-frame');
  const btn = frame.locator('button[data-test="buy-team"]');
  await btn.waitFor({ timeout: 15000 });
  await sleep(1500);
  // hover a couple of elements so the outline is visible, then click the target
  const other = frame.locator('button[data-test="buy-starter"]');
  const ob = await other.boundingBox();
  if (ob) { await page.mouse.move(ob.x + ob.width / 2, ob.y + ob.height / 2, { steps: 20 }); await sleep(700); }
  await moveClick(page, btn);
  const ta = page.locator('.new-thread-editor textarea');
  await ta.waitFor();
  await typeInto(page, ta, 'Make this the primary action.');
  await sleep(250);
  await moveClick(page, page.locator('.new-thread-editor button.primary.save'));
  await s.waitFor('thread.created');
  await sleep(2200);
  s.stop();
  await finish('scene06.webm');
}

// Scene 7: agent pre-annotations visible on load
async function scene07() {
  const s = await startServer([path.join(FIX, 'plan.md'), '--port', '5766']);
  const state = await api(s.endpoints.state);
  const fileId = state.files[0].id;
  const blocksUrl = s.endpoints.blocksTemplate.replace('{fileId}', fileId);
  const { sourceVersion, blocks } = await api(blocksUrl);
  const pick = (needle) => blocks.find((b) => b.snippet.includes(needle));
  const notes = [
    [pick('charge_with_retry'), 'I added the retry wrapper here. Verify the idempotency key is cloned before the first attempt, not per retry.'],
    [pick('Stage'), 'I tightened the rollout gates. Stage 2 now requires a failure rate under 0.5% before expansion.'],
    [pick('Rollback'), 'I simplified rollback to a flag flip. Confirm no stage writes data that a rollback would strand.'],
  ];
  for (const [block, text] of notes) {
    if (!block) continue;
    await api(s.endpoints.createThread, 'POST', {
      kind: 'agent',
      anchorStart: block.index,
      anchorEnd: block.index,
      snippet: block.snippet,
      sourceVersion,
      text,
    });
  }
  const { page, finish } = await newScene();
  await openDoc(page, s.url);
  await page.waitForSelector('.thread-marker.kind-pending', { timeout: 8000 });
  await sleep(1200);
  const marker = page.locator('.thread-marker.kind-pending').first();
  await moveClick(page, marker);
  await page.waitForSelector('.thread.pending.open', { timeout: 8000 });
  await sleep(2600);
  s.stop();
  await finish('scene07.webm');
}

// Scene 8: live source update, threads survive, one orphan
async function scene08() {
  const s = await startServer([path.join(FIX, 'plan.md'), '--port', '5767']);
  const state = await api(s.endpoints.state);
  const fileId = state.files[0].id;
  const blocksUrl = s.endpoints.blocksTemplate.replace('{fileId}', fileId);
  const { sourceVersion, blocks } = await api(blocksUrl);
  const design = blocks.find((b) => b.snippet.includes('Retries are safe'));
  const rollback = blocks.find((b) => b.snippet.includes('Rollback is a flag flip'));
  const t1 = await api(s.endpoints.createThread, 'POST', {
    kind: 'agent', anchorStart: design.index, anchorEnd: design.index,
    snippet: design.snippet, sourceVersion,
    text: 'Idempotency is the load-bearing claim. Verify the provider dedupe window is longer than our total retry span.',
  });
  const t2 = await api(s.endpoints.createThread, 'POST', {
    anchorStart: rollback.index, anchorEnd: rollback.index,
    snippet: rollback.snippet,
    text: 'Does rollback also stop in-flight retries?',
  });
  const { page, finish } = await newScene();
  await openDoc(page, s.url);
  await sleep(600);
  // pin every thread open so the orphan is visible after the swap
  await moveClick(page, page.locator('#show-all'));
  await sleep(1400);
  // agent pushes a rewritten doc: Design paragraph extended, Rollback paragraph removed
  const md = fs.readFileSync(path.join(FIX, 'plan.md'), 'utf8')
    .replace(
      'The provider deduplicates on that key, so a retry can never double-charge.',
      'The provider deduplicates on that key, so a retry can never double-charge. Keys expire after 24 hours, which is longer than any retry span.'
    )
    .replace('\nRollback is a flag flip. No data migration is needed at any stage.\n', '');
  await api(new URL('/api/source', s.url).href, 'POST', {
    markdown: md,
    threadAnchors: [
      { threadId: t1.id, anchorStart: design.index, anchorEnd: design.index, snippet: design.snippet },
      { threadId: t2.id, orphaned: true },
    ],
  });
  await page.waitForSelector('.thread.orphaned', { state: 'attached', timeout: 8000 });
  await sleep(1400);
  // the swap re-renders threads closed; surface the orphan via the summary popover
  await moveClick(page, page.locator('#thread-summary-toggle'));
  await sleep(800);
  await moveClick(page, page.locator(`.thread-summary-item[data-thread-id="${t2.id}"]`));
  await page.waitForSelector('.thread.orphaned.open', { timeout: 8000 });
  await sleep(2600);
  s.stop();
  await finish('scene08.webm');
}

// Scene 9: verdict modal with required feedback (captures stdout for scene 10)
async function scene09() {
  const s = await startServer([
    '--verdict-options', 'approve:Approve:positive|revise:Revise:neutral!|decline:Decline:negative!',
    path.join(FIX, 'plan.md'), '--port', '5768',
  ], { captureFile: path.join(CAPS, 'scene9.ndjsonl') });
  const { page, finish } = await newScene();
  await openDoc(page, s.url);
  await sleep(500);
  await moveClick(page, page.locator('#finish-review'));
  await page.waitForSelector('#verdict-modal:not([hidden])', { timeout: 8000 });
  await sleep(900);
  const decline = page.locator('.verdict-option-button[data-option-id="decline"]');
  await moveClick(page, decline);
  await page.waitForSelector('#verdict-validation', { timeout: 5000 });
  await sleep(1200);
  await typeInto(page, page.locator('#verdict-feedback'), 'Ship after the risks section is fixed.');
  await sleep(400);
  await moveClick(page, decline);
  await s.waitFor('session.done', 10000);
  await sleep(2200);
  await finish('scene09.webm');
  await s.exited;
}

// ---------- run ----------
const wanted = process.argv.slice(2);
const scenes = { scene02, scene03, scene04, scene05, scene06, scene07, scene08, scene09 };
const run = wanted.length ? wanted : Object.keys(scenes);

browser = await chromium.launch();
let failed = false;
for (const name of run) {
  try {
    console.log('recording', name, '...');
    await scenes[name]();
  } catch (e) {
    failed = true;
    console.error(`FAILED ${name}:`, e.message);
  } finally {
    for (const p of activeServers.splice(0)) {
      try { p.kill('SIGKILL'); } catch {}
    }
  }
}
await browser.close();
process.exit(failed ? 1 : 0);
