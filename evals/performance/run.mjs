import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { stats } from './stats.mjs';
import { stopChild } from './child-lifecycle.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(dir, '../..');
const script = JSON.parse(fs.readFileSync(path.join(dir, 'fixtures/script.json')));
if (script.iterations < 20) throw new Error('at least 20 measured iterations required');
const binary = process.env.DISCUSS_BIN;
if (!binary || !fs.existsSync(binary)) throw new Error('set DISCUSS_BIN to an existing executable');
const modulePath = process.env.PLAYWRIGHT_MODULE || path.join(dir, 'node_modules/playwright/index.mjs');
const { chromium } = await import(pathToFileURL(path.resolve(modulePath)).href);
const sha = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
const fixtures = Object.fromEntries(['large.md', 'script.json', 'small.md'].map(name => [name, sha(path.join(dir, 'fixtures', name))]));
const now = () => process.hrtime.bigint();
const ms = (start, end) => Number(end - start) / 1e6;
const timeout = (promise, label, duration = 15000) => new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error(`timeout: ${label}`)), duration);
  promise.then(value => { clearTimeout(timer); resolve(value); }, error => { clearTimeout(timer); reject(error); });
});
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'discuss-perf-'));
let browser, child;
async function post(url, body) {
  const response = await timeout(fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }), `POST ${url}`);
  if (!response.ok) throw new Error(`POST ${url}: ${response.status} ${await response.text()}`);
  return response.json();
}
async function scenario(page, base, name, config, waitEvent) {
  const file = path.join(dir, 'fixtures', config.document);
  const seeded = [];
  for (let i = 0; i < config.seedThreads; i++) {
    seeded.push(await post(`${base}/api/threads`, { anchorStart: 2 + i * 7, anchorEnd: 2 + i * 7, snippet: `Section ${String(i).padStart(4, '0')}`, text: `Seeded review point ${i}` }));
  }
  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__perfSseOpen === true);
  const id = seeded[0].id;
  await page.locator(`.thread-marker[data-thread-id="${id}"]`).first().click();
  const thread = page.locator(`.thread[data-thread-id="${id}"]`);
  await thread.locator('.followup textarea').waitFor();
  const samples = { submitToStdoutMs: [], postToInsertionMs: [], postToFrameMs: [], roundTripMs: [] };
  for (let i = 0; i < script.warmup + script.iterations; i++) {
    const baseline = await thread.locator('.user-comment[data-kind="take"]').count();
    // Arm DOM observation before the user action; no polling/sleep in timed intervals.
    await page.evaluate(({ id, baseline }) => { window.__perfDomPromise = new Promise((resolve, reject) => {
      const target = document.querySelector(`.thread[data-thread-id="${id}"]`);
      let postStart;
      const observer = new MutationObserver(() => {
        if (!postStart || target.querySelectorAll('.user-comment[data-kind="take"]').length <= baseline) return;
        const insertion = performance.now();
        observer.disconnect();
        requestAnimationFrame(() => resolve({ insertionMs: insertion - postStart, frameMs: performance.now() - postStart }));
      });
      observer.observe(target, { childList: true, subtree: true });
      window.__perfRespond = async () => {
        postStart = performance.now();
        const response = await fetch(`/api/threads/${encodeURIComponent(id)}/takes`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ text: window.__perfResponse }) });
        if (!response.ok) { observer.disconnect(); reject(new Error(`take POST: ${response.status}`)); }
      };
    }); }, { id, baseline });
    const receipt = waitEvent('reply.added', event => event.payload.threadId === id && event.payload.text === script.question);
    await thread.locator('.followup textarea').fill(script.question);
    const start = now();
    await thread.locator('.followup .save').click();
    const received = await timeout(receipt, 'reply.added');
    await page.evaluate(response => { window.__perfResponse = response; window.__perfRespond(); }, script.response);
    const timing = await timeout(page.evaluate(() => window.__perfDomPromise), 'DOM take and frame');
    const end = now();
    if (i >= script.warmup) {
      samples.submitToStdoutMs.push(ms(start, received.at));
      samples.postToInsertionMs.push(timing.insertionMs);
      samples.postToFrameMs.push(timing.frameMs);
      samples.roundTripMs.push(ms(start, end));
    }
  }
  return { seedThreads: config.seedThreads, warmup: script.warmup, samples, summary: Object.fromEntries(Object.entries(samples).map(([k, v]) => [k, stats(v)])) };
}
try {
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  await context.route('**/*', route => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1' || url.hostname === 'localhost') return route.continue();
    return route.abort();
  });
  await context.addInitScript(() => {
    const Native = window.EventSource;
    window.EventSource = class extends Native {
      constructor(...args) { super(...args); this.addEventListener('open', () => { window.__perfSseOpen = true; }); }
    };
  });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  const scenarios = {};
  for (const [name, config] of Object.entries(script.scenarios)) {
    let stdoutBuffer = '', stderr = '';
    const waiters = new Set();
    const waitEvent = (kind, match = () => true) => new Promise((resolve, reject) => waiters.add({ kind, match, resolve, reject }));
    const emit = event => {
      for (const waiter of [...waiters]) if (waiter.kind === event.kind && waiter.match(event)) {
        waiters.delete(waiter); waiter.resolve({ event, at: now() });
      }
    };
    const home = path.join(temp, name);
    fs.mkdirSync(home);
    const document = path.join(dir, 'fixtures', config.document);
    const started = waitEvent('session.started');
    child = spawn(path.resolve(binary), ['--no-open', '--no-save', document], { cwd: home, env: { ...process.env, HOME: home, XDG_CONFIG_HOME: home, XDG_DATA_HOME: home }, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.on('data', chunk => {
      stdoutBuffer += chunk;
      let n;
      while ((n = stdoutBuffer.indexOf('\n')) !== -1) {
        const line = stdoutBuffer.slice(0, n); stdoutBuffer = stdoutBuffer.slice(n + 1);
        try { emit(JSON.parse(line)); } catch { /* stderr is diagnostic; stdout may contain a partial line */ }
      }
    });
    child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-8000); });
    child.on('error', error => { for (const waiter of waiters) waiter.reject(error); waiters.clear(); });
    child.on('exit', code => { for (const waiter of waiters) waiter.reject(new Error(`discuss exited ${code}: ${stderr}`)); waiters.clear(); });
    try {
      const { event } = await timeout(started, 'session.started');
      scenarios[name] = await scenario(page, event.payload.apiBaseUrl || event.payload.url, name, config, waitEvent);
    } finally {
      await stopChild(child);
      for (const waiter of waiters) waiter.reject(new Error(`scenario ${name} ended`));
      waiters.clear();
      child = null;
    }
  }
  const output = {
    version: 1, fixtures, options: { warmup: script.warmup, iterations: script.iterations, network: 'block non-loopback', flags: ['--no-open', '--no-save'] },
    scenarioNames: Object.keys(script.scenarios), scenarios,
    binary: { path: path.resolve(binary), sha256: sha(binary) },
    source: { head: git('rev-parse', 'HEAD'), dirty: !!git('status', '--porcelain'), diffSha256: createHash('sha256').update(git('diff', 'HEAD', '--binary') + git('status', '--porcelain')).digest('hex') },
    environment: { profile: process.env.PERF_PROFILE || 'local', node: process.version, browser: browser.version(), os: `${os.platform()} ${os.release()} ${os.arch()}`, cpu: os.cpus()[0].model },
    recordedAt: new Date().toISOString()
  };
  const destination = process.env.PERF_OUTPUT || path.join(dir, 'results', `baseline-${Date.now()}.json`);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, JSON.stringify(output, null, 2) + '\n');
  console.log(destination);
  for (const [name, result] of Object.entries(scenarios)) console.log(name, result.summary);
} finally {
  if (child) await stopChild(child, 0);
  if (browser) await browser.close();
  fs.rmSync(temp, { recursive: true, force: true });
}
