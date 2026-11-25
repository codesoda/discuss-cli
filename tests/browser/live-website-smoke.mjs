// Browser E2E for live website review. Builds on Chrome's dependency-free CDP API.
// Usage: cargo build && node tests/browser/live-website-smoke.mjs

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

const root = path.resolve(import.meta.dirname, '../..');
const children = [];
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'discuss-live-e2e-'));
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
async function waitFor(check, label, attempts = 100) {
  let last;
  for (let i = 0; i < attempts; i++) {
    try { last = await check(); if (last) return last; } catch (error) { last = error; }
    await delay(100);
  }
  throw new Error(`Timed out waiting for ${label}${last instanceof Error ? `: ${last.message}` : ''}`);
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
      const resolved = (process.env.PATH || '')
        .split(path.delimiter)
        .map(directory => path.join(directory, candidate))
        .find(file => fs.existsSync(file));
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
      const { resolve, reject } = pending.get(message.id); pending.delete(message.id);
      if (message.error) reject(new Error(JSON.stringify(message.error))); else resolve(message.result);
    }
    if (message.method === 'Runtime.executionContextCreated') {
      const context = message.params.context;
      if (context.auxData?.isDefault && context.auxData?.frameId) contexts.set(context.auxData.frameId, context.id);
    }
    if (message.method === 'Runtime.executionContextDestroyed') {
      for (const [frameId, id] of contexts) if (id === message.params.executionContextId) contexts.delete(frameId);
    }
    if (message.method === 'Runtime.executionContextsCleared') contexts.clear();
    if (message.method === 'Runtime.exceptionThrown') {
      runtimeErrors.push(message.params.exceptionDetails.exception?.description || message.params.exceptionDetails.text);
    }
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

async function run() {
  phase = 'start fixture';
  const fixture = start(process.execPath, ['tests/browser/fixtures/live-app.mjs']);
  let fixtureErrors = '';
  fixture.stderr.on('data', chunk => { fixtureErrors += chunk; if (process.env.VERBOSE_BROWSER) process.stderr.write(chunk); });
  const fixtureInfo = JSON.parse(await firstLine(fixture.stdout, 'fixture port'));
  const upstreamUrl = `http://127.0.0.1:${fixtureInfo.port}/start?from=e2e#initial`;

  const binary = process.env.DISCUSS_BIN || path.join(root, 'target/debug/discuss');
  if (!fs.existsSync(binary)) throw new Error(`Discuss binary not found at ${binary}; run cargo build first`);
  fs.mkdirSync(path.join(temp, 'home'), { recursive: true });
  const discuss = start(binary, ['--no-open', '--no-save', upstreamUrl], {
    env: { ...process.env, HOME: path.join(temp, 'home'), DISCUSS_LOG: '' },
  });
  let discussErrors = '';
  discuss.stderr.on('data', chunk => { discussErrors += chunk; });
  const started = JSON.parse(await firstLine(discuss.stdout, 'session.started'));
  if (started.kind !== 'session.started' || started.payload.mode !== 'live') throw new Error(`Bad startup event: ${JSON.stringify(started)}`);
  const { apiBaseUrl, proxyUrl, upstreamUrl: reportedUpstream, endpoints } = started.payload;
  if (reportedUpstream !== upstreamUrl) throw new Error('Startup event changed upstream URL');
  if (new URL(apiBaseUrl).hostname !== '127.0.0.1' || new URL(proxyUrl).hostname !== '127.0.0.1') throw new Error('Listeners are not loopback-only');
  if (new URL(apiBaseUrl).origin === new URL(proxyUrl).origin) throw new Error('Proxy did not receive a second origin');
  if (process.env.VERBOSE_BROWSER) console.error(JSON.stringify({ fixturePort: fixtureInfo.port, apiBaseUrl, proxyUrl }));

  phase = 'proxy HTTP checks';
  const directBinary = new Uint8Array(await (await fetch(`http://127.0.0.1:${fixtureInfo.port}/binary`)).arrayBuffer());
  const proxiedBinary = new Uint8Array(await (await fetch(`${proxyUrl}/binary`)).arrayBuffer());
  if (Buffer.compare(Buffer.from(directBinary), Buffer.from(proxiedBinary)) !== 0) throw new Error('Non-HTML bytes changed in proxy');
  const rewrittenHead = await fetch(`${proxyUrl}/start`, { redirect: 'manual' });
  if (rewrittenHead.headers.has('x-frame-options') || rewrittenHead.headers.has('content-security-policy') || rewrittenHead.headers.has('content-security-policy-report-only')) {
    throw new Error('Frame-blocking headers survived HTML rewriting');
  }
  if (rewrittenHead.headers.get('cache-control') !== 'no-store') throw new Error('Rewritten HTML is cacheable');
  const rewrittenHtml = await rewrittenHead.text();
  if (!rewrittenHtml.includes('data-discuss-service-worker-guard') || !rewrittenHtml.includes('data-discuss-parent-origin')) throw new Error('Live HTML injection missing');

  phase = 'launch Chrome';
  const profile = path.join(temp, 'chrome');
  fs.mkdirSync(profile);
  const cdpPort = await freeLoopbackPort();
  const chrome = start(chromeExecutable(), [
    '--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', '--disable-background-networking',
    `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, 'about:blank',
  ]);
  let chromeErrors = '';
  chrome.stderr.on('data', chunk => { chromeErrors += chunk; });
  await waitFor(async () => {
    if (chrome.exitCode != null) throw new Error(`Chrome exited ${chrome.exitCode}: ${chromeErrors}`);
    try { return (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok; }
    catch (_) { return false; }
  }, 'Chrome DevTools HTTP endpoint');
  const cdp = await connectCdp(cdpPort);
  cdpSocket = cdp.socket;
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1100, height: 800, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Page.navigate', { url: apiBaseUrl });
  await waitFor(() => cdp.evaluate(`document.readyState === 'complete' && !!document.querySelector('.prototype-frame')`), 'Discuss UI');

  const frameId = await waitFor(async () => {
    const tree = await cdp.send('Page.getFrameTree');
    return flattenFrames(tree.frameTree).find(frame => frame.url.startsWith(proxyUrl))?.id;
  }, 'proxied iframe');
  const frameEval = expression => waitFor(() => cdp.contexts.get(frameId), 'iframe execution context').then(id => cdp.evaluate(expression, id));
  const topEval = expression => cdp.evaluate(expression);
  await waitFor(() => frameEval(`document.readyState === 'complete' && !!document.querySelector('[data-discuss-inspector]')`), 'injected inspector');

  phase = 'initial iframe checks';
  const initial = await frameEval(`({
    origin: location.origin,
    route: location.pathname + location.search + location.hash,
    js: window.fixtureJsLoaded === true,
    css: getComputedStyle(document.querySelector('h1')).color,
    image: document.querySelector('#fixture-image').complete && document.querySelector('#fixture-image').naturalWidth > 0,
    api: document.querySelector('#api-result').textContent,
    ws: document.querySelector('#ws-result').textContent,
    sw: document.querySelector('#sw-result').textContent,
    controlled: !!navigator.serviceWorker?.controller,
    guardBeforeApp: [...document.scripts].findIndex(s => s.hasAttribute('data-discuss-service-worker-guard')) < [...document.scripts].findIndex(s => s.src.endsWith('/app.js')),
  })`);
  await waitFor(() => frameEval(`document.querySelector('#api-result').textContent.startsWith('upstream-api|')`), 'proxied root API fetch');
  await waitFor(() => frameEval(`document.querySelector('#ws-result').textContent === 'through-proxy'`), 'proxied WebSocket echo');
  await waitFor(() => frameEval(`window.wsClose === '4001|fixture-close'`), 'proxied WebSocket close frame');
  await waitFor(() => frameEval(`document.querySelector('#sw-result').textContent.startsWith('blocked:')`), 'service-worker block');
  await waitFor(() => frameEval(`document.querySelector('#csrf-result').textContent !== 'pending'`), 'cross-origin API mutation attempt');
  if (!(await fetch(endpoints.state)).ok) throw new Error('Upstream page terminated the Discuss API cross-origin');
  if (initial.origin !== new URL(proxyUrl).origin || initial.route !== '/start?from=e2e#initial') throw new Error(`Wrong live iframe origin/route: ${JSON.stringify(initial)}`);
  if (!initial.js || initial.css !== 'rgb(12, 34, 56)' || !initial.image || !initial.guardBeforeApp) throw new Error(`Relative/root assets failed: ${JSON.stringify(initial)}`);
  if (await frameEval(`!!navigator.serviceWorker?.controller`)) throw new Error('Service worker controls proxy origin');
  const stats = await (await fetch(`http://127.0.0.1:${fixtureInfo.port}/stats`)).json();
  if (stats.serviceWorkerRequests !== 0) throw new Error(`Service worker script reached upstream: ${JSON.stringify(stats)}`);

  phase = 'request rewriting';
  const echo = await frameEval(`fetch('/echo?case=body', { method: 'POST', headers: {'Content-Type':'text/plain'}, body: 'payload-123' }).then(r => r.json())`);
  if (echo.method !== 'POST' || echo.query !== '?case=body' || echo.body !== 'payload-123') throw new Error(`Method/query/body not preserved: ${JSON.stringify(echo)}`);
  if (echo.host !== `127.0.0.1:${fixtureInfo.port}` || echo.origin !== `http://127.0.0.1:${fixtureInfo.port}` || !echo.referer.startsWith(`http://127.0.0.1:${fixtureInfo.port}/start`)) {
    throw new Error(`Host/Origin/Referer not rewritten: ${JSON.stringify(echo)}`);
  }

  async function framePoint(selector, placement = 'center') {
    const frameRect = await topEval(`(() => { const r=document.querySelector('.prototype-frame').getBoundingClientRect(); return {x:r.x,y:r.y}; })()`);
    const rect = await frameEval(`(() => { const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return {x:r.x,y:r.y,right:r.right,top:r.top,width:r.width,height:r.height}; })()`);
    return placement === 'marker'
      ? { x: frameRect.x + rect.right, y: frameRect.y + rect.top }
      : { x: frameRect.x + rect.x + rect.width / 2, y: frameRect.y + rect.y + rect.height / 2 };
  }
  async function pointerClick(point) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
  }

  phase = 'element thread lifecycle';
  await waitFor(() => topEval(`document.body.classList.contains('inspecting')`), 'automatic inspect mode');
  await waitFor(() => frameEval(`document.documentElement.style.cursor === 'crosshair'`), 'live inspector crosshair');
  const selectPoint = await framePoint('#select-me');
  if (process.env.VERBOSE_BROWSER) console.error(JSON.stringify({ selectPoint }));
  await pointerClick(selectPoint);
  await waitFor(() => topEval(`!!document.querySelector('.html-thread-editor')`), 'pointer-created element editor');
  await topEval(`(() => { const t=document.querySelector('.html-thread-editor textarea'); t.value='Live browser thread'; t.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('.html-thread-editor .save').click(); return true; })()`);
  await waitFor(async () => (await (await fetch(endpoints.state)).json()).threads?.[0]?.elementAnchor, 'saved live element anchor');
  let state = await (await fetch(endpoints.state)).json();
  const anchor = state.threads[0].elementAnchor;
  if (anchor.route !== '/start?from=e2e#initial' || anchor.accessibleName !== 'Review target' || anchor.selector !== '#select-me') throw new Error(`Incomplete live anchor context: ${JSON.stringify(anchor)}`);

  phase = 'SPA route tracking';
  await pointerClick(await framePoint('#spa-push'));
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/route-two?step=push#target'`), 'pushState route report');
  await waitFor(() => frameEval(`document.querySelector('[data-discuss-inspector]').getAttribute('data-discuss-marker-count') === '0'`), 'route-scoped marker hidden');
  state = await (await fetch(endpoints.state)).json();
  if (state.threads[0].orphaned) throw new Error('Off-route anchor was incorrectly detached');
  await pointerClick(await framePoint('#spa-replace'));
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/route-three?step=replace'`), 'replaceState route report');
  await pointerClick(await framePoint('#spa-back'));
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/start?from=e2e#initial'`), 'popstate route report');
  await pointerClick(await framePoint('#hash-route'));
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent.endsWith('#changed')`), 'hash route report');
  await frameEval('history.back()');
  await waitFor(() => topEval(`document.querySelector('.live-route-value').textContent === '/start?from=e2e#initial'`), 'hash popstate return');
  await waitFor(() => frameEval(`document.querySelector('[data-discuss-inspector]').getAttribute('data-discuss-marker-count') === '1'`), 'route-scoped marker restored');

  await topEval(`document.querySelector('.element-thread .thread-close').click()`);
  await waitFor(() => topEval(`!document.querySelector('.element-thread').classList.contains('open')`), 'thread close');
  await pointerClick(await framePoint('#select-me', 'marker'));
  await waitFor(() => topEval(`document.querySelector('.element-thread').classList.contains('open')`), 'marker reopen');
  await waitFor(() => frameEval(`document.querySelector('[data-discuss-inspector]').getAttribute('data-discuss-focused-thread') === 'u-1'`), 'persistent highlight');
  await delay(1100);
  if (!(await frameEval(`document.querySelector('[data-discuss-inspector]').getAttribute('data-discuss-focused-thread') === 'u-1'`))) throw new Error('Element highlight was not persistent');
  phase = 'element detachment';
  await frameEval(`document.querySelector('#select-me').remove()`);
  await waitFor(() => frameEval(`document.querySelector('[data-discuss-inspector]').getAttribute('data-discuss-marker-count') === '0'`), 'detached marker removal');
  await waitFor(async () => (await (await fetch(endpoints.state)).json()).threads?.[0]?.orphaned === true, 'element detachment');

  phase = 'redirect handling';
  await frameEval(`location.href='/same-redirect'`);
  await waitFor(() => frameEval(`document.querySelector('h1')?.textContent === 'Redirect stayed proxied'`), 'same-origin redirect');
  if (!(await frameEval(`location.origin === ${JSON.stringify(new URL(proxyUrl).origin)}`))) throw new Error('Same-origin redirect escaped proxy');
  await topEval(`window.confirm = message => { window.__externalPrompt = message; return false; }`);
  await frameEval(`location.href='/cross-redirect'`);
  await waitFor(() => topEval(`document.body.dataset.discussExternalNavigation === 'https://example.invalid/outside'`), 'cross-origin redirect prompt');
  if (!(await frameEval(`location.origin === ${JSON.stringify(new URL(proxyUrl).origin)} && document.body.textContent.includes('External navigation blocked')`))) throw new Error('Cross-origin redirect escaped proxy interstitial');
  await topEval(`delete document.body.dataset.discussExternalNavigation`);
  await frameEval(`location.href='/backslash-redirect'`);
  await waitFor(() => topEval(`document.body.dataset.discussExternalNavigation === 'http://example.invalid/backslash'`), 'backslash cross-origin redirect prompt');
  if (!(await frameEval(`location.origin === ${JSON.stringify(new URL(proxyUrl).origin)} && document.body.textContent.includes('External navigation blocked')`))) throw new Error('Backslash redirect escaped proxy interstitial');

  if (cdp.runtimeErrors.length) throw new Error(`Browser runtime errors: ${cdp.runtimeErrors.join('; ')}`);
  phase = 'joint shutdown';
  const done = await fetch(endpoints.done, { method: 'POST' });
  if (!done.ok) throw new Error(`Done failed: ${done.status}`);
  const exitCode = await new Promise(resolve => discuss.once('exit', resolve));
  if (exitCode !== 0) throw new Error(`Discuss exited ${exitCode}: ${discussErrors}`);
  await waitFor(async () => {
    try { await fetch(endpoints.state); return false; } catch (_) { return true; }
  }, 'API listener shutdown');
  await waitFor(async () => {
    try { await fetch(proxyUrl); return false; } catch (_) { return true; }
  }, 'proxy listener shutdown');

  console.log(JSON.stringify({
    ok: true,
    apiBaseUrl,
    proxyUrl,
    upstreamUrl,
    htmlInjected: true,
    assetsLoaded: true,
    apiFetchProxied: true,
    websocketProxied: true,
    framingHeadersRemoved: true,
    serviceWorkerBlocked: true,
    redirectsSafe: true,
    routesScoped: true,
    pointerThreadLifecycle: true,
    binaryUnchanged: true,
    listenersStoppedTogether: true,
  }));
  if (fixtureErrors) process.stderr.write(fixtureErrors);
  if (chromeErrors && process.env.VERBOSE_BROWSER) process.stderr.write(chromeErrors);
}

try {
  await run();
} catch (error) {
  console.error(`Live browser E2E failed during: ${phase}`);
  throw error;
} finally {
  try { cdpSocket?.close(); } catch (_) {}
  for (const child of children.reverse()) {
    if (child.exitCode == null) child.kill('SIGTERM');
  }
  await delay(300);
  try { fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); } catch (_) {}
}
