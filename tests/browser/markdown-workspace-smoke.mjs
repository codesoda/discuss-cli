// Run against a fresh Markdown fixture session and a disposable Chromium profile.
// DISCUSS_URL=... CDP_URL=... node tests/browser/markdown-workspace-smoke.mjs
import assert from 'node:assert/strict';
const { DISCUSS_URL: url, CDP_URL: cdp } = process.env;
if (!url || !cdp) throw new Error('Set DISCUSS_URL and CDP_URL for disposable test instances');
const page = await (await fetch(`${cdp}/json/new?about:blank`, {method:'PUT'})).json();
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = fail; });
let id = 0;
const pending = new Map();
const runtimeErrors = [];
ws.onmessage = ({data}) => {
  const m = JSON.parse(data); pending.get(m.id)?.(m);
  if (m.method === 'Runtime.exceptionThrown') runtimeErrors.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
};
async function send(method, params) {
  const n = ++id;
  const answer = new Promise(resolve => pending.set(n, resolve));
  ws.send(JSON.stringify({id:n, method, params}));
  const m = await answer; pending.delete(n); assert.ok(!m.error, JSON.stringify(m.error)); return m.result;
}
async function js(expression) {
  const r = await send('Runtime.evaluate', {expression, awaitPromise:true, returnByValue:true});
  assert.ok(!r.exceptionDetails, r.exceptionDetails?.exception?.description); return r.result.value;
}
async function until(expression) {
  for (let i=0; i<100; i++) { if (await js(expression)) return; await new Promise(r=>setTimeout(r,100)); }
  throw new Error(`Timeout: ${expression}; page: ${await js('JSON.stringify({ready:document.readyState, tools:document.querySelector("#md-tools")?.hidden,events:window.fixtureEvents?.readyState})')}; errors: ${runtimeErrors.join('; ')}`);
}
async function openEditor() {
  await js(`document.querySelector('#md-tools').click()`);
  await until(`document.querySelector('#md-workspace').open && !document.querySelector('#md-source').disabled`);
}
try {
  await send('Page.enable'); await send('Runtime.enable');
  await send('Page.addScriptToEvaluateOnNewDocument', {source: `window.EventSource = class extends EventSource {
    constructor(...args) { super(...args); window.fixtureEvents = this; window.delayedEvents = []; }
    addEventListener(type, listener) {
      super.addEventListener(type, event => {
        if (window.delayedEvents && ['thread.created','take.added','source.updated'].includes(type)) {
          window.delayedEvents.push(() => listener(event));
        } else { listener(event); }
      });
    }
  }`});
  await send('Page.navigate', {url});
  await until(`window.fixtureEvents?.readyState === EventSource.OPEN && document.querySelector('#md-tools')?.hidden === false`);
  const post = async (path, body) => { const r = await fetch(url + path, {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}); assert.ok(r.ok, await r.clone().text()); return r.json(); };
  const thread = await post('/api/threads',{fileId:'f-1',anchorStart:1,anchorEnd:1,snippet:'Fixture',text:'Fixture question?'});
  await post(`/api/threads/${thread.id}/takes`,{text:'Fixture answer.'});
  await openEditor();
  await js(`document.querySelector('#md-source').value='# Revised fixture'; document.querySelector('#md-apply').click()`);
  await until(`document.querySelector('#doc-content').textContent.includes('Revised fixture')`);
  await until(`!document.querySelector('#md-workspace').open`);
  // Deliver creation only after the HTTP Apply response already orphaned its anchor.
  await js(`{ const queued = window.delayedEvents; window.delayedEvents = null; queued.forEach(deliver => deliver()); }`);
  await until(`!!document.querySelector('.thread.orphaned[data-thread-id="${thread.id}"]')`);
  await openEditor();
  await js(`HTMLAnchorElement.prototype.click=function(){window.saved=fetch(this.href).then(r=>r.text())}; document.querySelector('#md-download').click()`);
  assert.equal(await js('window.saved'), '# Revised fixture');
  // Discussion export reflects SSE delivery, not completion of the REST writes.
  await js(`document.querySelector('#md-qa').checked=true`);
  await until(`(async()=>{document.querySelector('#md-download').click();return (await window.saved).includes('Agent: Fixture answer.')})()`);
  assert.match(await js('window.saved'), /Fixture question\?[\s\S]*Agent: Fixture answer\./);
  await js(`window.print=()=>{window.printed=document.querySelector('#md-print').textContent}; document.querySelector('#md-pdf').click()`);
  assert.match(await js('window.printed'), /Revised fixture[\s\S]*Fixture question\?/);
  assert.equal(await js(`document.body.classList.contains('print-review-copy')`), true);
  await js(`window.dispatchEvent(new Event('afterprint'))`);
  assert.equal(await js(`document.querySelector('#md-print').childElementCount`), 0);
  assert.equal(await js(`document.body.classList.contains('print-review-copy')`), false);
  await openEditor();
  await post('/api/source', {fileId:'f-1', markdown:'Concurrent edit', threadAnchors:[{threadId:thread.id, orphaned:true}]});
  await js(`document.querySelector('#md-source').value='Keep my draft'; document.querySelector('#md-apply').click()`);
  await until(`document.querySelector('#md-message').textContent.includes('Source changed')`);
  assert.equal(await js(`document.querySelector('#md-workspace').open && document.querySelector('#md-source').value`), 'Keep my draft');
  await js(`window.confirm=()=>true; document.querySelector('#md-close').click(); document.querySelector('#finish-review').click()`);
  await until(`document.body.classList.contains('review-complete') && document.querySelector('#md-tools').hidden`);
  assert.deepEqual(runtimeErrors, []);
  console.log('Markdown edit, stale draft, discussion export, print lifecycle and completion passed.');
} finally { ws.close(); await fetch(`${cdp}/json/close/${page.id}`); }
