import { test } from 'node:test';
import assert from 'node:assert/strict';
import { stats, assertCompatible } from './stats.mjs';
test('nearest-rank p50/p95 and arithmetic median use raw samples without mutating them', () => {
  const input = Array.from({ length: 20 }, (_, i) => 20 - i);
  assert.deepEqual(stats(input), { count: 20, p50: 10, p95: 19, median: 10.5 });
  assert.equal(input[0], 20);
  assert.equal(stats([-2, 1]).median, -0.5);
});
test('reject incompatible fixtures, scenarios, options, machines and sample sizes', () => {
  const base = { version: 1, fixtures: { 'a.md': 'abc' }, options: { iterations: 20 }, scenarioNames: ['small'], environment: { profile: 'desk', browser: '1', node: '2', os: 'linux', cpu: 'cpu' }, scenarios: { small: { samples: { roundTripMs: Array(20).fill(1) } } } };
  assert.doesNotThrow(() => assertCompatible(base, structuredClone(base)));
  for (const change of [b => b.fixtures['a.md'] = 'def', b => b.options.iterations = 21, b => b.options.schedules = { overlapSerial: { handling: 'serial', events: [{ offsetMs: 500, delayMs: 6000 }] } }, b => b.scenarioNames = ['large'], b => b.environment.cpu = 'other', b => b.scenarios.small.samples.roundTripMs.pop(), b => b.scenarios.small.samples.extra = Array(20).fill(1)]) {
    const other = structuredClone(base); change(other);
    assert.throws(() => assertCompatible(base, other), /incompatible|missing/);
  }
});
