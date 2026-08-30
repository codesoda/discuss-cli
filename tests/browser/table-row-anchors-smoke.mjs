// Dependency-free Chrome DevTools Protocol smoke test for Markdown table rows.
// Usage: DISCUSS_URL=http://127.0.0.1:7792 CDP_URL=http://127.0.0.1:9227 node tests/browser/table-row-anchors-smoke.mjs

const discussUrl = process.env.DISCUSS_URL || 'http://127.0.0.1:7792';
const cdpUrl = process.env.CDP_URL || 'http://127.0.0.1:9227';

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

await send('Runtime.enable');
await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', {
  width: 900, height: 700, deviceScaleFactor: 1, mobile: false,
});
await send('Page.navigate', { url: discussUrl });
await waitFor(`document.querySelectorAll('.table-wrap tr[data-anchor-idx]').length === 3`, 'stamped table rows');

const blocks = await (await fetch(`${discussUrl}/api/files/f-1/blocks`)).json();
const tableBlocks = blocks.blocks.filter(block => block.index >= 2 && block.index <= 5);
if (
  tableBlocks.length !== 4
  || tableBlocks[0].breadcrumb !== 'Plan'
  || tableBlocks[1].snippet !== 'Item Estimate Notes'
  || tableBlocks[1].breadcrumb !== 'Plan › Table header'
  || tableBlocks[2].snippet !== 'Alpha 1 day short'
  || tableBlocks[2].breadcrumb !== 'Plan › Table row 1'
  || !tableBlocks[3].snippet.startsWith('Beta 20 days wide value')
  || tableBlocks[3].breadcrumb !== 'Plan › Table row 2'
) {
  throw new Error(`Unexpected table block metadata: ${JSON.stringify(tableBlocks)}`);
}

const anchors = await evaluate(`Array.from(document.querySelectorAll('.table-wrap [data-anchor-idx]')).map(el => ({
  tag: el.tagName,
  index: Number(el.dataset.anchorIdx),
  text: el.textContent.trim().replace(/\\s+/g, ' '),
}))`);
if (JSON.stringify(anchors.map(anchor => [anchor.tag, anchor.index])) !== JSON.stringify([
  ['TR', 3], ['TR', 4], ['TR', 5],
])) {
  throw new Error(`Rows were not stamped in DOM order: ${JSON.stringify(anchors)}`);
}

await click(await center('.table-wrap tbody tr:nth-child(2) td:nth-child(2)'));
const editorRef = await waitFor(`document.querySelector('.new-thread-editor .anchor-ref')?.textContent`, 'row editor');
if (!editorRef.startsWith('#5 · Beta 20 days wide value')) {
  throw new Error(`Click did not target the precise row: ${editorRef}`);
}
const focusState = await evaluate(`({
  row: document.querySelector('.table-wrap tbody tr:nth-child(2)').classList.contains('focused'),
  table: document.querySelector('.table-wrap').classList.contains('focused'),
})`);
if (!focusState.row || focusState.table) {
  throw new Error(`Wrong table focus hierarchy: ${JSON.stringify(focusState)}`);
}

await evaluate(`(() => {
  const textarea = document.querySelector('.new-thread-editor textarea');
  textarea.value = 'Row estimate is too high';
  textarea.dispatchEvent(new Event('input', { bubbles: true }));
  return true;
})()`);
await click(await center('.new-thread-editor .save'));
await waitFor(`document.querySelector('.thread[data-thread-id="u-1"]')`, 'saved row thread');

let state;
for (let attempt = 0; attempt < 50; attempt++) {
  state = await (await fetch(`${discussUrl}/api/state`)).json();
  if (state.threads?.[0]?.id === 'u-1') break;
  await new Promise(resolve => setTimeout(resolve, 100));
}
const saved = state.threads?.[0];
if (!saved || saved.anchorStart !== 5 || saved.anchorEnd !== 5 || !saved.snippet.startsWith('Beta 20 days wide value')) {
  throw new Error(`Server did not persist the row anchor: ${JSON.stringify(saved)}`);
}

const markerPosition = await waitFor(`(() => {
  const row = document.querySelector('.table-wrap tbody tr:nth-child(2)');
  const wrapper = document.querySelector('.table-wrap');
  const stack = wrapper.querySelector(':scope > .thread-marker-stack[data-row-anchor-idx="5"]');
  if (!stack) return null;
  const rowRect = row.getBoundingClientRect();
  const markerRect = stack.getBoundingClientRect();
  return { rowY: rowRect.y, markerY: markerRect.y, markerX: markerRect.x, wrapperRight: wrapper.getBoundingClientRect().right };
})()`, 'row gutter marker');
if (Math.abs(markerPosition.rowY - markerPosition.markerY) > 3) {
  throw new Error(`Marker is not vertically aligned to its row: ${JSON.stringify(markerPosition)}`);
}

const markerHost = await evaluate(`(() => {
  const marker = document.querySelector('.table-wrap > .thread-marker-stack[data-row-anchor-idx="5"]');
  return marker ? {
    directChildOfWrapper: marker.parentElement.classList.contains('table-wrap'),
    outsideRow: !document.querySelector('.table-wrap tbody tr:nth-child(2)').contains(marker),
  } : null;
})()`);
if (!markerHost?.directChildOfWrapper || !markerHost.outsideRow) {
  throw new Error(`Row marker was placed inside table content: ${JSON.stringify(markerHost)}`);
}
if (runtimeErrors.length) throw new Error(`Browser runtime errors: ${runtimeErrors.join('; ')}`);

console.log(JSON.stringify({
  ok: true,
  rowAnchors: anchors.map(anchor => anchor.index),
  editorRef,
  savedAnchor: saved.anchorStart,
  markerAnchoredToTableGutter: true,
}));
socket.close();
