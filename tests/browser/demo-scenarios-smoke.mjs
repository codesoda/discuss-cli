// Browser E2E for the three self-contained `discuss demo` scenarios.
// Usage: cargo build && node tests/browser/demo-scenarios-smoke.mjs
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

const root = path.resolve(import.meta.dirname, '../..');
const children = [];
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'discuss-demo-e2e-'));
const evidenceDir = process.env.SCREENSHOT_DIR || path.join(temp, 'evidence');
fs.mkdirSync(evidenceDir, { recursive: true });
let cdpSocket;
let phase = 'setup';

function start(command, args, options = {}) {
  const child = spawn(command, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'], ...options });
  children.push(child);
  return child;
}
function firstLine(stream, label, timeoutMs = 15000) {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: stream });
    const timer = setTimeout(() => reject(new Error(`Timed out waiting for ${label}`)), timeoutMs);
    lines.once('line', line => { clearTimeout(timer); lines.close(); resolve(line); });
  });
}
function delay(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }
async function waitFor(check, label, attempts = 120) {
  let last;
  for (let i = 0; i < attempts; i++) {
    try { last = await check(); if (last) return last; } catch (error) { last = error; }
    await delay(100);
  }
  throw new Error(`Timed out waiting for ${label}${last instanceof Error ? `: ${last.message}` : ''}`);
}
function freeLoopbackPort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      server.close(error => error ? reject(error) : resolve(port));
    });
  });
}
function chromeExecutable() {
  const candidates = [
    process.env.CHROME_BIN,
    process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : null,
    'google-chrome', 'chromium', 'chromium-browser',
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (candidate.includes('/') && fs.existsSync(candidate)) return candidate;
    if (!candidate.includes('/')) {
      const resolved = (process.env.PATH || '').split(path.delimiter)
        .map(directory => path.join(directory, candidate)).find(file => fs.existsSync(file));
      if (resolved) return resolved;
    }
  }
  throw new Error('Chrome executable not found; set CHROME_BIN');
}
async function connectCdp(port) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const target = targets.find(item => item.type === 'page');
  if (!target) throw new Error('Chrome exposes no page target');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let nextId = 0;
  const pending = new Map();
  const contexts = new Map();
  const runtimeErrors = [];
  socket.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const request = pending.get(message.id); pending.delete(message.id);
      if (message.error) request.reject(new Error(JSON.stringify(message.error))); else request.resolve(message.result);
    }
    if (message.method === 'Runtime.executionContextCreated') {
      const context = message.params.context;
      if (context.auxData?.isDefault && context.auxData?.frameId) contexts.set(context.auxData.frameId, context.id);
    }
    if (message.method === 'Runtime.executionContextDestroyed') {
      for (const [frameId, id] of contexts) if (id === message.params.executionContextId) contexts.delete(frameId);
    }
    if (message.method === 'Runtime.executionContextsCleared') contexts.clear();
    if (message.method === 'Runtime.exceptionThrown') runtimeErrors.push(message.params.exceptionDetails.exception?.description || message.params.exceptionDetails.text);
  };
  function send(method, params = {}) {
    const id = ++nextId;
    socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
  }
  async function evaluate(expression, contextId) {
    const result = await send('Runtime.evaluate', { expression, contextId, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
    return result.result.value;
  }
  return { socket, send, evaluate, contexts, runtimeErrors };
}
function flattenFrames(tree, output = []) {
  output.push(tree.frame);
  for (const child of tree.childFrames || []) flattenFrames(child, output);
  return output;
}
async function capture(cdp, name) {
  const shot = await cdp.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  const file = path.join(evidenceDir, name);
  fs.writeFileSync(file, Buffer.from(shot.data, 'base64'));
  return file;
}

async function run() {
  phase = 'start demo';
  const binary = process.env.DISCUSS_BIN || path.join(root, 'target/debug/discuss');
  if (!fs.existsSync(binary)) throw new Error(`Discuss binary not found at ${binary}; run cargo build first`);
  const home = path.join(temp, 'home'); fs.mkdirSync(home);
  const discuss = start(binary, ['--no-open', 'demo'], {
    env: { ...process.env, HOME: home, DISCUSS_LOG: '', PATH: '' },
  });
  const discussExited = new Promise(resolve => discuss.once('exit', resolve));
  let discussErrors = '';
  discuss.stderr.on('data', chunk => { discussErrors += chunk; });
  const started = JSON.parse(await firstLine(discuss.stdout, 'session.started'));
  const payload = started.payload;
  if (started.kind !== 'session.started' || payload.mode !== 'demo' || payload.scenarios?.length !== 3) {
    throw new Error(`Bad demo startup: ${JSON.stringify(started)}`);
  }
  for (const key of ['apiBaseUrl', 'examplePrUrl', 'localAppReviewUrl', 'localAppUpstreamUrl', 'localAppProxyUrl']) {
    if (new URL(payload[key]).hostname !== '127.0.0.1') throw new Error(`${key} is not loopback`);
  }
  await waitFor(async () => (await (await fetch(`${payload.examplePrUrl}/api/state`)).json()).prSession?.phase === 'reviewing', 'demo PR import');

  phase = 'launch Chrome';
  const profile = path.join(temp, 'chrome'); fs.mkdirSync(profile);
  const cdpPort = await freeLoopbackPort();
  const chrome = start(chromeExecutable(), [
    '--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', '--disable-background-networking',
    '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost',
    `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, 'about:blank',
  ]);
  let chromeErrors = '';
  chrome.stderr.on('data', chunk => { chromeErrors += chunk; });
  await waitFor(async () => {
    if (chrome.exitCode != null) throw new Error(`Chrome exited ${chrome.exitCode}: ${chromeErrors}`);
    try { return (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok; } catch (_) { return false; }
  }, 'Chrome DevTools endpoint');
  const cdp = await connectCdp(cdpPort); cdpSocket = cdp.socket;
  await cdp.send('Runtime.enable'); await cdp.send('Page.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
  const topEval = expression => cdp.evaluate(expression);

  phase = 'tour discoverability';
  await cdp.send('Page.navigate', { url: payload.apiBaseUrl });
  await waitFor(() => topEval(`document.readyState === 'complete' && document.querySelectorAll('#demo-scenarios a').length === 3`), 'tour scenario navigation');
  const nav = await topEval(`[...document.querySelectorAll('#demo-scenarios a')].map(a => ({id:a.dataset.demoScenario,label:a.textContent,current:a.getAttribute('aria-current')}))`);
  if (nav.map(item => item.id).join(',') !== 'tour,pr,local-app' || nav[0].current !== 'page') throw new Error(`Bad scenario navigation: ${JSON.stringify(nav)}`);

  phase = 'local app review';
  await topEval(`document.querySelector('[data-demo-scenario="local-app"]').click()`);
  await waitFor(() => topEval(`location.origin === ${JSON.stringify(new URL(payload.localAppReviewUrl).origin)} && !!document.querySelector('.prototype-frame')`), 'local app review UI');
  const frameId = await waitFor(async () => {
    const tree = await cdp.send('Page.getFrameTree');
    return flattenFrames(tree.frameTree).find(frame => frame.url.startsWith(payload.localAppProxyUrl))?.id;
  }, 'local app proxy iframe');
  const frameEval = expression => waitFor(() => cdp.contexts.get(frameId), 'iframe context').then(id => cdp.evaluate(expression, id));
  await waitFor(() => frameEval(`document.readyState === 'complete' && document.body.dataset.demoApiLoaded === 'true' && !!document.querySelector('[data-discuss-inspector]')`), 'local app assets, API, and inspector');
  const app = await frameEval(`({
    origin: location.origin,
    css: getComputedStyle(document.querySelector('.app-header')).backgroundColor,
    api: document.querySelector('#api-status').textContent,
    script: [...document.scripts].some(script => script.src.endsWith('/demo-app.js')),
  })`);
  if (app.origin !== new URL(payload.localAppProxyUrl).origin || app.css !== 'rgb(20, 33, 61)' || app.api !== 'local API connected' || !app.script) {
    throw new Error(`Bundled app contract failed: ${JSON.stringify(app)}`);
  }
  await topEval(`document.querySelector('#inspect-toggle').click()`);
  await waitFor(() => topEval(`!document.body.classList.contains('inspecting')`), 'pause inspect mode for app navigation');
  await frameEval(`document.querySelector('a[data-route="/payments"]').click()`);
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/payments'`), 'pushState route report');
  await frameEval(`history.back()`);
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/'`), 'popstate route report');
  await frameEval(`document.querySelector('a[data-route="/payments"]').click()`);
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/payments'`), 'return to payments route');
  await topEval(`document.querySelector('#inspect-toggle').click()`);

  async function framePoint(selector) {
    const frameRect = await topEval(`(() => { const r=document.querySelector('.prototype-frame').getBoundingClientRect(); return {x:r.x,y:r.y}; })()`);
    const rect = await frameEval(`(() => { const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height}; })()`);
    return { x: frameRect.x + rect.x + rect.width / 2, y: frameRect.y + rect.y + rect.height / 2 };
  }
  async function pointerClick(point) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
  }
  await waitFor(() => topEval(`document.body.classList.contains('inspecting')`), 'automatic inspect mode');
  await pointerClick(await framePoint('#deploy-card'));
  await waitFor(() => topEval(`!!document.querySelector('.html-thread-editor textarea')`), 'element comment editor');
  await topEval(`(() => { const textarea=document.querySelector('.html-thread-editor textarea'); textarea.value='Keep the rollback control visible here.'; textarea.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('.html-thread-editor .save').click(); return true; })()`);
  const localState = await waitFor(async () => {
    const state = await (await fetch(`${payload.localAppReviewUrl}/api/state`)).json();
    return state.threads?.[0]?.elementAnchor?.route === '/payments' && state.takes?.[state.threads[0].id]?.length ? state : null;
  }, 'route-scoped element anchor and canned response');
  if (!localState.threads[0].elementAnchor.selector.startsWith('#deploy-card') || !localState.takes[localState.threads[0].id][0].text.startsWith('Demo agent — ')) {
    throw new Error(`Incomplete local element thread: ${JSON.stringify(localState)}`);
  }
  await waitFor(() => frameEval(`document.querySelector('[data-discuss-inspector]').getAttribute('data-discuss-marker-count') === '1'`), 'in-frame marker');
  const localShot = await capture(cdp, 'demo-local-app.png');

  phase = 'PR review';
  await topEval(`document.querySelector('[data-demo-scenario="pr"]').click()`);
  await waitFor(() => topEval(`location.origin === ${JSON.stringify(new URL(payload.examplePrUrl).origin)} && document.querySelectorAll('.file-item').length === 5`), 'PR file tree');
  const prUi = await topEval(`({
    demo: document.querySelector('#pr-header')?.textContent.includes('SYNTHETIC DEMO DATA'),
    folders: [...document.querySelectorAll('.file-folder-name')].map(node => node.textContent),
    imported: !!document.querySelector('[data-thread-id="gh-review-thread-900003"]'),
    diffFiles: [...document.querySelectorAll('.file-item')].filter(item => item.querySelector('.file-kind')?.textContent === 'diff').length,
  })`);
  if (!prUi.demo || !prUi.imported || prUi.diffFiles !== 4 || !['assets','docs','src','payments','__tests__'].every(folder => prUi.folders.includes(folder))) {
    throw new Error(`PR fixture/UI contract failed: ${JSON.stringify(prUi)}`);
  }
  const firstDiffId = await topEval(`document.querySelector('.file-item .file-kind')?.closest('.file-item').dataset.fileId`);
  await topEval(`document.querySelector('.file-item[data-file-id=${JSON.stringify(firstDiffId)}]').click()`);
  await waitFor(() => topEval(`document.querySelector('.pr-file-viewed-control input') !== null`), 'Viewed control');
  await topEval(`document.querySelector('.pr-file-viewed-control input').click()`);
  await waitFor(() => topEval(`document.querySelector('.file-item[data-file-id=${JSON.stringify(firstDiffId)}]').classList.contains('viewed') && document.querySelector('.file-item.active').dataset.fileId !== ${JSON.stringify(firstDiffId)}`), 'Viewed eye and next-unviewed progression');

  await topEval(`document.querySelector('#finish-review').click()`);
  await waitFor(() => topEval(`!!document.querySelector('#pr-review-summary') && document.querySelector('#pr-dialog-title').textContent === 'Simulate demo PR review'`), 'PR Finish Review editor');
  await topEval(`(() => {
    const summary=document.querySelector('#pr-review-summary'); summary.value += ' Browser-verified edit.'; summary.dispatchEvent(new Event('input',{bubbles:true}));
    const include=document.querySelector('.pr-item-include-checkbox:not(:disabled)'); if (include && !include.checked) include.click();
    document.querySelector('#pr-dialog-actions .pr-button.primary').click(); return true;
  })()`);
  await waitFor(() => topEval(`document.querySelector('#pr-preview-gfm')?.textContent.includes('Browser-verified edit.') && document.querySelector('#pr-dialog-actions .pr-button.primary')?.textContent === 'OK — Simulate locally'`), 'exact GFM confirmation');
  const gfm = await topEval(`document.querySelector('#pr-preview-gfm').textContent`);
  if (!gfm.includes('Action: COMMENT') || !gfm.includes('Reply to review thread 900003')) throw new Error(`Unexpected exact GFM: ${gfm}`);
  const prShot = await capture(cdp, 'demo-pr-confirmation.png');
  await topEval(`document.querySelector('#pr-dialog-actions .pr-button.primary').click()`);
  await waitFor(() => topEval(`document.querySelector('#done-banner').textContent.includes('Demo publication simulated locally') && document.querySelector('#done-banner').textContent.includes('Nothing was sent to GitHub')`), 'demo-only publication result');
  const resultShot = await capture(cdp, 'demo-pr-result.png');

  if (cdp.runtimeErrors.length) throw new Error(`Browser runtime errors: ${cdp.runtimeErrors.join('; ')}`);
  const exitCode = discuss.exitCode ?? await Promise.race([
    discussExited,
    delay(8000).then(() => { throw new Error('Timed out waiting for Discuss shutdown'); }),
  ]);
  if (exitCode !== 0) throw new Error(`Discuss exited ${exitCode}: ${discussErrors}`);
  console.log(JSON.stringify({
    ok: true,
    scenariosDiscoverable: true,
    prNestedFiles: true,
    prViewedProgression: true,
    prExactGfm: true,
    prLocalSimulation: true,
    localAssetsAndApi: true,
    localSpaRoutes: true,
    localElementAnchorMarkerAndTake: true,
    evidence: [localShot, prShot, resultShot],
  }));
}

try {
  await run();
} catch (error) {
  console.error(`Demo browser E2E failed during: ${phase}`);
  throw error;
} finally {
  try { cdpSocket?.close(); } catch (_) {}
  for (const child of children.reverse()) if (child.exitCode == null) child.kill('SIGTERM');
  await delay(300);
  if (!process.env.SCREENSHOT_DIR) {
    try { fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); } catch (_) {}
  }
}
