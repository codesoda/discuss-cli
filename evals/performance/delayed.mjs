import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { stats } from './stats.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(dir, '../..');
const workloadFile = path.resolve(process.env.DELAY_FIXTURE || path.join(dir, 'fixtures/delayed.json'));
const workload = JSON.parse(fs.readFileSync(workloadFile));
if (workload.iterations < 20 || workload.warmup < 1) throw new Error('delayed workload requires >=20 measured batches and >=1 warmup');
const document = path.join(dir, 'fixtures/small.md');
const bin = process.env.DISCUSS_BIN;
if (!bin || !fs.existsSync(bin)) throw new Error('set DISCUSS_BIN to a real executable');
const { chromium } = await import(pathToFileURL(path.resolve(process.env.PLAYWRIGHT_MODULE || path.join(dir, 'node_modules/playwright/index.mjs'))).href);
const sha = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const hr = () => process.hrtime.bigint();
const elapsed = (a, b) => Number(b - a) / 1e6;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const bounded = (promise, label, duration = 20000) => new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error(`timeout: ${label}`)), duration);
  promise.then(x => { clearTimeout(timer); resolve(x); }, e => { clearTimeout(timer); reject(e); });
});
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'discuss-delayed-'));
let browser, child, buffer = '', stderr = '';
const waiters = new Set();
function receipt(id) { return new Promise((resolve, reject) => waiters.add({ id, resolve, reject })); }
async function post(url, body) {
  const response = await bounded(fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }), url);
  if (!response.ok) throw new Error(`${url}: ${response.status} ${await response.text()}`);
  return response.json();
}
async function activity(base, id, status) {
  const response = await bounded(fetch(`${base}/api/threads/${id}/activity`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ status }) }), 'activity');
  if (response.status === 404 || response.status === 405) return false;
  if (!response.ok) throw new Error(`activity: ${response.status}`);
  return true;
}
async function profile(context, name, config) {
  const started = new Promise((resolve, reject) => waiters.add({ id: 'session.started', resolve, reject }));
  const home = path.join(temp, name); fs.mkdirSync(home);
  child = spawn(path.resolve(bin), ['--no-open', '--no-save', document], { cwd: home, env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home, XDG_DATA_HOME: home }, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.on('data', chunk => {
    buffer += chunk;
    let n;
    while ((n = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, n); buffer = buffer.slice(n + 1);
      let event;
      try { event = JSON.parse(line); } catch { continue; }
      for (const waiter of [...waiters]) if (waiter.id === event.kind || (event.kind === 'reply.added' && waiter.id === event.payload.threadId)) {
        waiters.delete(waiter); waiter.resolve({ event, at: hr() });
      }
    }
  });
  child.stderr.on('data', data => { stderr = (stderr + data).slice(-8000); });
  const fail = error => { for (const waiter of waiters) waiter.reject(error); waiters.clear(); };
  child.on('error', fail);
  child.on('exit', code => fail(new Error(`discuss exited ${code}: ${stderr}`)));
  const pages = [];
  try {
    const base = (await bounded(started, 'session.started')).event.payload.apiBaseUrl;
    const threads = [];
    for (let i = 0; i < config.events.length; i++) threads.push(await post(`${base}/api/threads`, { anchorStart: 2 + i * 7, anchorEnd: 2 + i * 7, snippet: `Section ${String(i).padStart(4, '0')}`, text: `Seeded review point ${i}` }));
    const supported = await activity(base, threads[0].id, 'idle');
    for (const thread of threads) {
      const page = await context.newPage(); pages.push(page); page.setDefaultTimeout(20000);
      await page.goto(base, { waitUntil: 'domcontentloaded' });
      await page.waitForFunction(() => window.__perfSseOpen === true);
      await page.locator(`.thread-marker[data-thread-id="${thread.id}"]`).first().click();
      await page.locator(`.thread[data-thread-id="${thread.id}"] .followup textarea`).waitFor();
    }
    const samples = { submissionToVisibleMs: [], observedWaitMs: [], actualTimerMs: [], intendedWaitMs: [], overheadMs: [], batchMakespanMs: [], scheduleDriftMs: [] };
    const correctness = { missing: 0, duplicate: 0, misrouted: 0, indicatorFailures: 0, checked: 0, indicatorSupported: supported };
    for (let batch = 0; batch < workload.warmup + workload.iterations; batch++) {
      const start = hr();
      let serial = Promise.resolve();
      const tasks = config.events.map((event, index) => (async () => {
        const thread = threads[event.thread];
        const page = pages[event.thread];
        const id = thread.id;
        const responseText = `${workload.response} [${name}:${batch}:${index}]`;
        const countBefore = await page.locator(`.thread[data-thread-id="${id}"] .user-comment[data-kind="take"]`).count();
        // Install before submission; DOM observer uses browser clock only for insertion, harness clock for receipt.
        await page.evaluate(({ id, countBefore, responseText }) => {
          window.__delayedTake = new Promise(resolve => {
            const target = document.querySelector(`.thread[data-thread-id="${id}"]`);
            const observer = new MutationObserver(() => {
              const items = [...target.querySelectorAll('.user-comment[data-kind="take"]')];
              if (items.length <= countBefore || !items.some(item => item.textContent.includes(responseText))) return;
              observer.disconnect(); resolve({ at: performance.now(), count: items.length });
            });
            observer.observe(target, { childList: true, subtree: true });
          });
        }, { id, countBefore, responseText });
        await sleep(Math.max(0, event.offsetMs - elapsed(start, hr())));
        const sentAt = hr();
        const pending = receipt(id);
        await page.locator(`.thread[data-thread-id="${id}"] .followup textarea`).fill(workload.question);
        await page.locator(`.thread[data-thread-id="${id}"] .followup .save`).click();
        const received = await bounded(pending, 'reply.added');
        if (received.event.payload.text !== workload.question) throw new Error(`unexpected question on ${id}`);
        const handle = async () => {
          if (supported) {
            await activity(base, id, 'thinking');
            try { await page.locator(`.thread[data-thread-id="${id}"] .thread-activity`).getByText('Agent thinking').waitFor({ timeout: 3000 }); }
            catch { correctness.indicatorFailures++; }
          }
          const waitStart = hr();
          await sleep(event.delayMs);
          const waitEnd = hr();
          if (supported) {
            await activity(base, id, 'replying');
            try { await page.locator(`.thread[data-thread-id="${id}"] .thread-activity`).getByText('Agent replying').waitFor({ timeout: 3000 }); }
            catch { correctness.indicatorFailures++; }
          }
          const postAt = hr();
          await post(`${base}/api/threads/${id}/takes`, { text: responseText });
          const visible = await bounded(page.evaluate(() => window.__delayedTake), 'visible take');
          const visibleAt = hr();
          if (supported) {
            await activity(base, id, 'idle');
            try { await page.locator(`.thread[data-thread-id="${id}"] .thread-activity`).waitFor({ state: 'detached', timeout: 3000 }); }
            catch { correctness.indicatorFailures++; }
          }
          return { id, responseText, sentAt, receivedAt: received.at, waitStart, waitEnd, postAt, visibleAt, browserInsertion: visible.at, expectedCount: countBefore + 1 };
        };
        if (config.handling === 'serial') {
          const queued = serial.then(handle);
          serial = queued.then(() => {});
          return queued;
        }
        return handle();
      })());
      const results = await bounded(Promise.all(tasks), `batch ${batch}`, 45000);
      const snapshot = await (await fetch(`${base}/api/state`)).json();
      for (const result of results) {
        const takes = (snapshot.takes[result.id] || []).filter(t => t.text === result.responseText);
        const elsewhere = Object.entries(snapshot.takes).flatMap(([id, items]) => id === result.id ? [] : items.filter(t => t.text === result.responseText));
        if (takes.length === 0) correctness.missing++;
        if (takes.length > 1) correctness.duplicate += takes.length - 1;
        correctness.misrouted += elsewhere.length;
        const ownPage = pages[threads.findIndex(thread => thread.id === result.id)];
        const ownDom = await ownPage.locator(`.thread[data-thread-id="${result.id}"] .user-comment[data-kind="take"]`).filter({ hasText: result.responseText }).count();
        if (ownDom === 0) correctness.missing++;
        if (ownDom > 1) correctness.duplicate += ownDom - 1;
        const wrongDom = await ownPage.locator(`.thread:not([data-thread-id="${result.id}"]) .user-comment[data-kind="take"]`).filter({ hasText: result.responseText }).count();
        correctness.misrouted += wrongDom;
        correctness.checked++;
        if (batch >= workload.warmup) {
          const observed = elapsed(result.receivedAt, result.postAt);
          samples.submissionToVisibleMs.push(elapsed(result.sentAt, result.visibleAt));
          samples.observedWaitMs.push(observed);
          samples.actualTimerMs.push(elapsed(result.waitStart, result.waitEnd));
          samples.intendedWaitMs.push(config.events[results.indexOf(result)].delayMs);
          samples.overheadMs.push(elapsed(result.sentAt, result.visibleAt) - observed);
          samples.scheduleDriftMs.push(elapsed(start, result.sentAt) - config.events[results.indexOf(result)].offsetMs);
        }
      }
      if (batch >= workload.warmup) samples.batchMakespanMs.push(elapsed(start, results.reduce((latest, result) => result.visibleAt > latest ? result.visibleAt : latest, start)));
    }
    if (correctness.missing || correctness.duplicate || correctness.misrouted || correctness.indicatorFailures) throw new Error(`correctness failed ${name}: ${JSON.stringify(correctness)}`);
    return { schedule: config, warmup: workload.warmup, iterations: workload.iterations, correctness, samples, summary: Object.fromEntries(Object.entries(samples).map(([key, values]) => [key, stats(values)])) };
  } finally {
    await Promise.all(pages.map(page => page.close()));
    if (child && child.exitCode === null) {
      const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGTERM');
      await Promise.race([exited, sleep(2000)]);
      if (child.exitCode === null) child.kill('SIGKILL');
    }
    child = null; buffer = ''; stderr = '';
  }
}
try {
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  await context.route('**/*', route => ['127.0.0.1', 'localhost'].includes(new URL(route.request().url()).hostname) ? route.continue() : route.abort());
  await context.addInitScript(() => {
    const Native = window.EventSource;
    window.EventSource = class extends Native {
      constructor(...args) { super(...args); this.addEventListener('open', () => { window.__perfSseOpen = true; }); }
    };
  });
  const requested = process.env.DELAY_PROFILES?.split(',') || Object.keys(workload.profiles);
  const scenarios = {};
  for (const name of requested) {
    if (!workload.profiles[name]) throw new Error(`unknown profile: ${name}`);
    scenarios[name] = await profile(context, name, workload.profiles[name]);
    console.log(name, scenarios[name].summary);
  }
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  const output = {
    version: 2, fixtures: { 'small.md': sha(document), [path.basename(workloadFile)]: sha(workloadFile) },
    options: { warmup: workload.warmup, iterations: workload.iterations, network: 'block non-loopback', flags: ['--no-open', '--no-save'], schedules: Object.fromEntries(requested.map(name => [name, workload.profiles[name]])) },
    scenarioNames: requested, scenarios, binary: { path: path.resolve(bin), sha256: sha(bin) },
    source: { head: git('rev-parse', 'HEAD'), dirty: !!git('status', '--porcelain'), diffSha256: createHash('sha256').update(git('diff', 'HEAD', '--binary') + git('status', '--porcelain')).digest('hex') },
    environment: { profile: process.env.PERF_PROFILE || 'local', node: process.version, browser: browser.version(), os: `${os.platform()} ${os.release()} ${os.arch()}`, cpu: os.cpus()[0].model },
    recordedAt: new Date().toISOString()
  };
  const destination = process.env.PERF_OUTPUT || path.join(dir, 'results', `delayed-${Date.now()}.json`);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, JSON.stringify(output, null, 2) + '\n'); console.log(destination);
} finally {
  if (child && child.exitCode === null) child.kill('SIGKILL');
  if (browser) await browser.close();
  fs.rmSync(temp, { recursive: true, force: true });
}
