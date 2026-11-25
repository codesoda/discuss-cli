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
ws.onmessage = ({data}) => { const m = JSON.parse(data); pending.get(m.id)?.(m); };
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
  throw new Error(`Timeout: ${expression}`);
}
try {
  await send('Page.navigate', {url});
  await until(`document.querySelector('#md-tools')?.hidden === false`);
  const post = async (path, body) => { const r = await fetch(url + path, {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}); assert.ok(r.ok, await r.clone().text()); return r.json(); };
  const thread = await post('/api/threads',{fileId:'f-1',anchorStart:1,anchorEnd:1,snippet:'Fixture',text:'Fixture question?'});
  await post(`/api/threads/${thread.id}/takes`,{text:'Fixture answer.'});
  await js(`document.querySelector('#md-tools').click(); document.querySelector('#md-source').value='# Revised fixture'; document.querySelector('#md-apply').click()`);
  await until(`document.querySelector('#doc-content').textContent.includes('Revised fixture')`);
  await js(`document.querySelector('#md-tools').click(); HTMLAnchorElement.prototype.click=function(){window.saved=fetch(this.href).then(r=>r.text())}; document.querySelector('#md-download').click()`);
  assert.equal(await js('window.saved'), '# Revised fixture');
  await js(`document.querySelector('#md-qa').checked=true; document.querySelector('#md-download').click()`);
  assert.match(await js('window.saved'), /Fixture question\?[\s\S]*Agent: Fixture answer\./);
  await js(`window.print=()=>{window.printed=document.querySelector('#md-print').textContent}; document.querySelector('#md-pdf').click()`);
  assert.match(await js('window.printed'), /Revised fixture[\s\S]*Fixture question\?/);
  console.log('Markdown edit, discussion, download and print content passed.');
} finally { ws.close(); await fetch(`${cdp}/json/close/${page.id}`); }
