import { test } from 'node:test';
import assert from 'node:assert/strict';
import { stats, assertCompatible } from './stats.mjs';
import { timingSamples } from './delayed-timing.mjs';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { activity } from './activity-probe.mjs';
import { stopChild } from './child-lifecycle.mjs';
test('compare prints n/a for zero baseline median without losing absolute delta', () => {
  const dir = mkdtempSync(join(tmpdir(), 'discuss-compare-'));
  try {
    const base = { version: 1, fixtures: {}, options: {}, scenarioNames: ['small'], environment: { profile: 'test', os: 'test', cpu: 'test', browser: 'test', node: 'test' }, scenarios: { small: { samples: { indicatorMs: Array(20).fill(0) } } } };
    const before = join(dir, 'before.json'), after = join(dir, 'after.json');
    writeFileSync(before, JSON.stringify(base));
    base.scenarios.small.samples.indicatorMs.fill(2);
    writeFileSync(after, JSON.stringify(base));
    const result = spawnSync(process.execPath, [new URL('./compare.mjs', import.meta.url).pathname, before, after], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /indicatorMs: 0\.000 -> 2\.000 ms; n\/a \(2\.000 ms\)/);
    base.scenarios.small.samples.indicatorMs.fill(0);
    writeFileSync(after, JSON.stringify(base));
    const bothZero = spawnSync(process.execPath, [new URL('./compare.mjs', import.meta.url).pathname, before, after], { encoding: 'utf8' });
    assert.equal(bothZero.status, 0, bothZero.stderr);
    assert.match(bothZero.stdout, /indicatorMs: 0\.000 -> 0\.000 ms; n\/a \(0\.000 ms\)/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('activity probe distinguishes advertised missing route and known missing thread from legacy unsupported route', async () => {
  let status = 404, body = 'Not Found';
  const server = createServer((_request, response) => { response.writeHead(status); response.end(body); });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    assert.equal(await activity(base, 'id', 'idle', false), false);
    await assert.rejects(activity(base, 'id', 'idle', true), /activity: 404/);
    body = JSON.stringify({ error: { code: 'thread_not_found' } });
    await assert.rejects(activity(base, 'id', 'idle', false), /thread_not_found/);
    status = 405; body = 'Method Not Allowed';
    assert.equal(await activity(base, 'id', 'idle', false), false);
    status = 200; body = '{}';
    assert.equal(await activity(base, 'id', 'idle', false), true);
  } finally { server.close(); await once(server, 'close'); }
});
test('scenario child ignoring SIGTERM is reaped after SIGKILL before next scenario', async () => {
  const child = spawn(process.execPath, ['-e', "process.on('SIGTERM', () => {}); process.stdout.write('ready\\n'); setInterval(() => {}, 1000)"], { stdio: ['ignore', 'pipe', 'ignore'] });
  try {
    await once(child.stdout, 'data');
    await stopChild(child, 20);
    assert.equal(child.signalCode, 'SIGKILL');
    assert.ok(child.exitCode !== null || child.signalCode !== null);
  } finally { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); }
});
test('nearest-rank p50/p95 and arithmetic median use raw samples without mutating them', () => {
  const input = Array.from({ length: 20 }, (_, i) => 20 - i);
  assert.deepEqual(stats(input), { count: 20, p50: 10, p95: 19, median: 10.5 });
  assert.equal(input[0], 20);
  assert.equal(stats([-2, 1]).median, -0.5);
});
test('slow activity endpoint contributes to overhead, while artificial wait and serial queue do not', async () => {
  const server = createServer((_request, response) => setTimeout(() => { response.writeHead(200); response.end('{}'); }, 70));
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    const clock = () => process.hrtime.bigint();
    const sentAt = clock();
    const receivedAt = clock();
    await new Promise(resolve => setTimeout(resolve, 25)); // serial queue
    const handleStart = clock();
    const indicatorStart = clock();
    await fetch(`http://127.0.0.1:${server.address().port}/activity`, { method: 'POST' });
    const indicatorNs = clock() - indicatorStart;
    const waitStart = clock();
    await new Promise(resolve => setTimeout(resolve, 35)); // artificial response delay
    const waitEnd = clock();
    const postAt = clock();
    const visibleAt = clock();
    const measured = timingSamples({ sentAt, receivedAt, handleStart, indicatorNs, waitStart, waitEnd, postAt, visibleAt });
    assert.ok(measured.indicatorMs >= 65);
    assert.ok(measured.queueWaitMs >= 20);
    assert.ok(measured.overheadMs >= measured.indicatorMs - 2, JSON.stringify(measured));
    assert.ok(measured.overheadMs < measured.indicatorMs + 25, JSON.stringify(measured));
  } finally {
    server.close();
    await once(server, 'close');
  }
});
test('reject incompatible fixtures, scenarios, options, machines and sample sizes', () => {
  const base = { version: 1, fixtures: { 'a.md': 'abc' }, options: { iterations: 20 }, scenarioNames: ['small'], environment: { profile: 'desk', browser: '1', node: '2', os: 'linux', cpu: 'cpu' }, scenarios: { small: { samples: { roundTripMs: Array(20).fill(1) } } } };
  assert.doesNotThrow(() => assertCompatible(base, structuredClone(base)));
  const oldDelayed = { ...base, version: 2 };
  const revisedDelayed = { ...base, version: 3 };
  assert.throws(() => assertCompatible(oldDelayed, revisedDelayed), /incompatible version/);
  for (const change of [b => b.fixtures['a.md'] = 'def', b => b.options.iterations = 21, b => b.options.schedules = { overlapSerial: { handling: 'serial', events: [{ offsetMs: 500, delayMs: 6000 }] } }, b => b.scenarioNames = ['large'], b => b.environment.cpu = 'other', b => b.scenarios.small.samples.roundTripMs.pop(), b => b.scenarios.small.samples.extra = Array(20).fill(1)]) {
    const other = structuredClone(base); change(other);
    assert.throws(() => assertCompatible(base, other), /incompatible|missing/);
  }
});
