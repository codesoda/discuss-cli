import crypto from 'node:crypto';
import http from 'node:http';

const binaryFixture = Buffer.from([0, 255, 17, 34, 128, 64, 10]);
let serviceWorkerRequests = 0;

function page(title = 'Live fixture') {
  return `<!doctype html>
<html><head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; frame-ancestors 'none'">
<link rel="stylesheet" href="styles.css">
<script src="/app.js" defer></script>
</head><body>
<h1>${title}</h1>
<img id="fixture-image" src="/pixel.svg" alt="fixture pixel">
<button id="select-me" aria-label="Review target">Review this element</button>
<button id="spa-push">Push route</button>
<button id="spa-replace">Replace route</button>
<button id="spa-back">Back route</button>
<button id="hash-route">Hash route</button>
<div id="api-result">pending</div><div id="ws-result">pending</div><div id="sw-result">pending</div><div id="csrf-result">pending</div>
</body></html>`;
}

const appJs = `
window.fixtureJsLoaded = true;
const discussApiOrigin = document.querySelector('script[data-discuss-parent-origin]')?.dataset.discussParentOrigin;
if (discussApiOrigin) {
  fetch(discussApiOrigin + '/api/done', { method: 'POST', mode: 'no-cors' }).then(
    () => { document.querySelector('#csrf-result').textContent = 'attempted'; },
    () => { document.querySelector('#csrf-result').textContent = 'blocked'; }
  );
}
fetch('/api/example').then(r => r.json()).then(v => {
  document.querySelector('#api-result').textContent = v.source + '|' + v.host + '|' + v.origin;
});
const ws = new WebSocket('ws://' + location.host + '/ws');
ws.addEventListener('open', () => ws.send('through-proxy'));
ws.addEventListener('message', event => { document.querySelector('#ws-result').textContent = event.data; });
ws.addEventListener('close', event => { window.wsClose = event.code + '|' + event.reason; });
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').then(
    () => { document.querySelector('#sw-result').textContent = 'registered'; },
    error => { document.querySelector('#sw-result').textContent = 'blocked:' + error.name; }
  );
} else document.querySelector('#sw-result').textContent = 'unsupported';
document.querySelector('#spa-push').addEventListener('click', () => history.pushState({}, '', '/route-two?step=push#target'));
document.querySelector('#spa-replace').addEventListener('click', () => history.replaceState({}, '', '/route-three?step=replace'));
document.querySelector('#spa-back').addEventListener('click', () => history.back());
document.querySelector('#hash-route').addEventListener('click', () => { location.hash = 'changed'; });
`;

function websocketUpgrade(request, socket) {
  if (request.url !== '/ws') return socket.destroy();
  const key = request.headers['sec-websocket-key'];
  const accept = crypto
    .createHash('sha1')
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest('base64');
  socket.write(
    'HTTP/1.1 101 Switching Protocols\r\n' +
    'Upgrade: websocket\r\n' +
    'Connection: Upgrade\r\n' +
    `Sec-WebSocket-Accept: ${accept}\r\n\r\n`
  );
  let pending = Buffer.alloc(0);
  socket.on('error', () => {});
  socket.on('data', chunk => {
    pending = Buffer.concat([pending, chunk]);
    while (pending.length >= 2) {
      const first = pending[0];
      const opcode = first & 0x0f;
      const masked = (pending[1] & 0x80) !== 0;
      let length = pending[1] & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (pending.length < 4) return;
        length = pending.readUInt16BE(2); offset = 4;
      }
      const needed = offset + (masked ? 4 : 0) + length;
      if (pending.length < needed) return;
      let payload;
      if (masked) {
        const mask = pending.subarray(offset, offset + 4); offset += 4;
        payload = Buffer.from(pending.subarray(offset, offset + length));
        for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
      } else payload = pending.subarray(offset, offset + length);
      pending = pending.subarray(needed);
      if (opcode === 8) {
        socket.end(Buffer.from([0x88, 0]));
        return;
      }
      if (opcode === 1) {
        const header = payload.length < 126
          ? Buffer.from([0x81, payload.length])
          : Buffer.from([0x81, 126, payload.length >> 8, payload.length & 255]);
        socket.write(Buffer.concat([header, payload]));
        const reason = Buffer.from('fixture-close');
        const closePayload = Buffer.alloc(2 + reason.length);
        closePayload.writeUInt16BE(4001, 0);
        reason.copy(closePayload, 2);
        socket.write(Buffer.concat([Buffer.from([0x88, closePayload.length]), closePayload]));
      }
    }
  });
}

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://fixture.invalid');
  if (url.pathname === '/start' || url.pathname === '/route-two' || url.pathname === '/route-three') {
    response.writeHead(200, {
      'Content-Type': 'text/html; charset=utf-8',
      'Content-Security-Policy': "default-src 'self'; frame-ancestors 'none'",
      'Content-Security-Policy-Report-Only': "frame-ancestors 'none'",
      'X-Frame-Options': 'DENY',
      'Content-Length': Buffer.byteLength(page()),
      ETag: 'stale-html-validator',
    });
    return response.end(page());
  }
  if (url.pathname === '/redirected') {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'X-Frame-Options': 'DENY' });
    return response.end(page('Redirect stayed proxied'));
  }
  if (url.pathname === '/styles.css') {
    response.writeHead(200, { 'Content-Type': 'text/css; charset=utf-8' });
    return response.end('h1 { color: rgb(12, 34, 56); }');
  }
  if (url.pathname === '/app.js') {
    response.writeHead(200, { 'Content-Type': 'application/javascript' });
    return response.end(appJs);
  }
  if (url.pathname === '/pixel.svg') {
    response.writeHead(200, { 'Content-Type': 'image/svg+xml' });
    return response.end('<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="green"/></svg>');
  }
  if (url.pathname === '/api/example') {
    response.writeHead(200, { 'Content-Type': 'application/json' });
    return response.end(JSON.stringify({
      source: 'upstream-api',
      host: request.headers.host || '',
      origin: request.headers.origin || '',
      referer: request.headers.referer || '',
    }));
  }
  if (url.pathname === '/echo') {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    response.writeHead(200, { 'Content-Type': 'application/json' });
    return response.end(JSON.stringify({ method: request.method, query: url.search, body: Buffer.concat(chunks).toString(), host: request.headers.host || '', origin: request.headers.origin || '', referer: request.headers.referer || '' }));
  }
  if (url.pathname === '/binary') {
    response.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Length': binaryFixture.length });
    return response.end(binaryFixture);
  }
  if (url.pathname === '/same-redirect') {
    response.writeHead(302, { Location: `http://127.0.0.1:${server.address().port}/redirected?ok=1` });
    return response.end();
  }
  if (url.pathname === '/cross-redirect') {
    response.writeHead(302, { Location: 'https://example.invalid/outside' });
    return response.end();
  }
  if (url.pathname === '/backslash-redirect') {
    response.writeHead(302, { Location: '\\\\example.invalid/backslash' });
    return response.end();
  }
  if (url.pathname === '/sw.js') {
    serviceWorkerRequests++;
    response.writeHead(200, { 'Content-Type': 'application/javascript', 'Service-Worker-Allowed': '/' });
    return response.end("self.addEventListener('fetch', event => event.respondWith(new Response('controlled')))");
  }
  if (url.pathname === '/stats') {
    response.writeHead(200, { 'Content-Type': 'application/json' });
    return response.end(JSON.stringify({ serviceWorkerRequests }));
  }
  response.writeHead(404, { 'Content-Type': 'text/plain' });
  response.end('fixture not found');
});
server.on('upgrade', websocketUpgrade);
server.listen(0, '127.0.0.1', () => {
  console.log(JSON.stringify({ port: server.address().port }));
});
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => server.close(() => process.exit(0)));
