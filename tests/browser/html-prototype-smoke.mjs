// Dependency-free Chrome DevTools Protocol smoke test for HTML prototype review.
// Usage: DISCUSS_URL=http://127.0.0.1:7791 CDP_URL=http://127.0.0.1:9226 node tests/browser/html-prototype-smoke.mjs

import fs from 'node:fs';

const discussUrl = process.env.DISCUSS_URL || 'http://127.0.0.1:7791';
const cdpUrl = process.env.CDP_URL || 'http://127.0.0.1:9226';
const screenshotPath = process.env.SCREENSHOT_PATH || '/tmp/discuss-html-hover.png';

const targets = await (await fetch(`${cdpUrl}/json`)).json();
const target = targets.find(item => item.type === 'page');
if (!target) throw new Error('Chrome exposes no page target');

const socket = new WebSocket(target.webSocketDebuggerUrl);
let nextId = 0;
const pending = new Map();
const runtimeErrors = [];
socket.onmessage = event => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const handlers = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) handlers.reject(new Error(JSON.stringify(message.error)));
    else handlers.resolve(message.result);
  }
  if (message.method === 'Runtime.exceptionThrown') {
    runtimeErrors.push(message.params.exceptionDetails.exception?.description || message.params.exceptionDetails.text);
  }
};
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});

function send(method, params = {}) {
  const id = ++nextId;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

async function evaluate(expression) {
  const response = await send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (response.exceptionDetails) {
    throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
  }
  return response.result.value;
}

async function waitFor(expression, description, attempts = 60) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const value = await evaluate(expression);
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${description}`);
}

async function click(point) {
  await send('Input.dispatchMouseEvent', {
    type: 'mousePressed', x: point.x, y: point.y, button: 'left', clickCount: 1,
  });
  await send('Input.dispatchMouseEvent', {
    type: 'mouseReleased', x: point.x, y: point.y, button: 'left', clickCount: 1,
  });
}

async function center(selector) {
  return evaluate(`(() => {
    const rect = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
    return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
  })()`);
}

async function framePoint(selector, placement = 'center') {
  return evaluate(`(() => {
    const frame = document.querySelector('.prototype-frame');
    const frameRect = frame.getBoundingClientRect();
    const rect = frame.contentDocument.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
    return placement === 'marker'
      ? { x: frameRect.x + rect.right, y: frameRect.y + rect.top }
      : { x: frameRect.x + rect.x + rect.width / 2, y: frameRect.y + rect.y + rect.height / 2 };
  })()`.replace('placement ===', `${JSON.stringify(placement)} ===`));
}

await send('Runtime.enable');
await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', {
  width: 1000, height: 760, deviceScaleFactor: 1, mobile: false,
});
await send('Page.navigate', { url: discussUrl });
await waitFor(
  `!!document.querySelector('.prototype-frame')?.contentDocument?.querySelector('h1')`,
  'prototype iframe load',
);
await waitFor(
  `!!document.querySelector('.prototype-frame')?.contentDocument?.querySelector('[data-discuss-inspector]')`,
  'injected inspector readiness',
);

const injectedScript = await evaluate(
  `document.querySelector('.prototype-frame').contentDocument.querySelector('script[src*="discuss-inspect"]')?.getAttribute('src')`,
);
if (injectedScript !== '/assets/discuss-inspect.js?v=4') {
  throw new Error(`Inspector URL is not cache-busted: ${injectedScript}`);
}

const inspectButton = await center('#inspect-toggle');
await waitFor(`document.body.classList.contains('inspecting')`, 'automatic Inspect mode');
const inspectCursor = await waitFor(
  `document.querySelector('.prototype-frame').contentDocument.documentElement.style.cursor === 'crosshair' && 'crosshair'`,
  'inspector crosshair cursor',
);
if (inspectCursor !== 'crosshair') {
  throw new Error(`Inspector did not expose its active crosshair cursor: ${inspectCursor}`);
}

const headingPoint = await framePoint('h1');
await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...headingPoint });
await new Promise(resolve => setTimeout(resolve, 150));
const hoverActive = await evaluate(
  `document.querySelector('.prototype-frame').contentDocument.querySelector('[data-discuss-inspector]')?.hasAttribute('data-discuss-hovering')`,
);
if (!hoverActive) throw new Error('Real pointer movement did not activate the inspector hover overlay');
const screenshot = await send('Page.captureScreenshot', { format: 'png' });
fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, 'base64'));
await click(headingPoint);
const headingAnchor = await waitFor(
  `document.querySelector('.html-thread-editor .anchor-ref')?.textContent`,
  'heading element editor',
);
if (!headingAnchor.includes('h1') || !headingAnchor.includes('Ship reliable software')) {
  throw new Error(`Wrong heading selected: ${headingAnchor}`);
}

// Reproduce the resolver race that previously erased a freshly-opened editor:
// an SSE echo with unchanged detachment state must leave the draft mounted.
await fetch(`${discussUrl}/api/anchors/resolve`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ fileId: 'f-1', detachedThreadIds: [] }),
});
await new Promise(resolve => setTimeout(resolve, 300));
const editorSurvivedResolverEcho = await evaluate(`(() => {
  const editor = document.querySelector('.html-thread-editor');
  if (!editor) return false;
  const rect = editor.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 && rect.bottom > 0 && rect.top < innerHeight;
})()`);
if (!editorSurvivedResolverEcho) {
  throw new Error('Element comment editor was removed by an anchors.resolved SSE echo');
}
await click(await center('.html-thread-editor .cancel'));
await waitFor(`!document.querySelector('.html-thread-editor')`, 'heading editor close');
await waitFor(`document.body.classList.contains('inspecting')`, 'commenting mode persists after cancel');

// Scroll through the actual iframe viewport, then select a lower CTA with real pointer input.
const frameCenter = await center('.prototype-frame');
await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...frameCenter });
await send('Input.dispatchMouseEvent', {
  type: 'mouseWheel', ...frameCenter, deltaX: 0, deltaY: 620,
});
await new Promise(resolve => setTimeout(resolve, 250));
await waitFor(`document.body.classList.contains('inspecting')`, 'commenting mode persists after iframe scroll');
const ctaPoint = await framePoint('[data-test="buy-team"]');
if (ctaPoint.y < 54 || ctaPoint.y > 760) throw new Error(`CTA did not scroll into view: ${JSON.stringify(ctaPoint)}`);
await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...ctaPoint });
await click(ctaPoint);
const ctaAnchor = await waitFor(
  `document.querySelector('.html-thread-editor .anchor-ref')?.textContent`,
  'CTA element editor',
);
if (!ctaAnchor.includes('button') || !ctaAnchor.includes('Start trial')) {
  throw new Error(`Wrong CTA selected: ${ctaAnchor}`);
}

await evaluate(`(() => {
  const textarea = document.querySelector('.html-thread-editor textarea');
  textarea.value = 'Browser smoke test';
  textarea.dispatchEvent(new Event('input', { bubbles: true }));
  return true;
})()`);
await click(await center('.html-thread-editor .save'));
await waitFor(
  `document.querySelector('.element-thread[data-thread-id="u-1"]')`,
  'saved element thread',
);
let apiState;
for (let attempt = 0; attempt < 50; attempt++) {
  apiState = await (await fetch(`${discussUrl}/api/state`)).json();
  if (apiState.threads?.[0]?.elementAnchor) break;
  await new Promise(resolve => setTimeout(resolve, 100));
}
const saved = apiState.threads?.[0];
if (!saved || saved.elementAnchor?.selector !== '#pricing [data-test="buy-team"]') {
  throw new Error(`Server did not persist the expected anchor: ${JSON.stringify(saved)}`);
}
await waitFor(`document.body.classList.contains('inspecting')`, 'commenting mode persists after save');

// The Interact toggle is the special mode: it hands clicks to the prototype
// and restores its cursor, and a second press returns to commenting.
await click(inspectButton);
await waitFor(`!document.body.classList.contains('inspecting')`, 'Interact mode on toggle press');
await waitFor(
  `document.querySelector('.prototype-frame').contentDocument.documentElement.style.cursor !== 'crosshair'`,
  'inspector crosshair cursor released in Interact mode',
);
await click(inspectButton);
await waitFor(`document.body.classList.contains('inspecting')`, 'commenting mode after leaving Interact');

// Close the card, then use the in-frame marker's real hit target to reopen it.
await click(await center('.element-thread .thread-close'));
await waitFor(`!document.querySelector('.element-thread').classList.contains('open')`, 'thread close');
await new Promise(resolve => setTimeout(resolve, 200));
await click(await framePoint('[data-test="buy-team"]', 'marker'));
await waitFor(`document.querySelector('.element-thread').classList.contains('open')`, 'in-frame marker click');
await waitFor(
  `document.querySelector('.prototype-frame').contentDocument.querySelector('[data-discuss-inspector]')?.getAttribute('data-discuss-focused-thread') === 'u-1'`,
  'persistent element focus for open thread',
);
await new Promise(resolve => setTimeout(resolve, 1200));
const focusRemainedAfterPulse = await evaluate(
  `document.querySelector('.prototype-frame').contentDocument.querySelector('[data-discuss-inspector]')?.getAttribute('data-discuss-focused-thread') === 'u-1'`,
);
if (!focusRemainedAfterPulse) throw new Error('Open thread highlight disappeared after its pulse animation');
const focusScreenshot = await send('Page.captureScreenshot', { format: 'png' });
const focusScreenshotPath = screenshotPath.replace(/\.png$/i, '-focused.png');
fs.writeFileSync(focusScreenshotPath, Buffer.from(focusScreenshot.data, 'base64'));

// Removing the selected element must detach the thread through the resolver endpoint.
await evaluate(`(() => {
  document.querySelector('.prototype-frame').contentDocument.querySelector('[data-test="buy-team"]').remove();
  return true;
})()`);
for (let attempt = 0; attempt < 50; attempt++) {
  apiState = await (await fetch(`${discussUrl}/api/state`)).json();
  if (apiState.threads?.[0]?.orphaned === true) break;
  await new Promise(resolve => setTimeout(resolve, 100));
}
if (apiState.threads?.[0]?.orphaned !== true) {
  throw new Error('Removed element was not reported as detached');
}
if (runtimeErrors.length) throw new Error(`Browser runtime errors: ${runtimeErrors.join('; ')}`);

console.log(JSON.stringify({
  ok: true,
  injectedScript,
  headingAnchor,
  ctaAnchor,
  savedSelector: saved.elementAnchor.selector,
  editorSurvivedResolverEcho,
  markerReopenedThread: true,
  focusRemainedAfterPulse,
  detachedAfterRemoval: true,
  screenshotPath,
  focusScreenshotPath,
}));
socket.close();
