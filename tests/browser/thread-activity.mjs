// Run with: node --test tests/browser/thread-activity.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
function between(start, end) {
  const first = page.indexOf(start);
  const last = page.indexOf(end, first + start.length);
  assert.ok(first >= 0 && last > first, `missing source span: ${start}`);
  return page.slice(first, last);
}
const source = [
  between('  // Ephemeral activity is never written', '  function commentsForThread(state, threadId)'),
  between('  function applyThreadResolvedEvent(payload)', '  function applyThreadUnresolvedEvent(payload)'),
  between('  function renderAllThreads() {', '  // ---------- Markers ----------'),
].join('\n');

function harness() {
  let now = 1000;
  let nextTimer = 0;
  let activeFileId = 'a';
  const timers = new Map();
  const mounted = [];
  const state = { threads: [{ id: 't1' }], userThreads: [{ id: 't1', fileId: 'a' }],
    followups: {}, takes: {}, resolutions: {} };
  const prepopulated = [{ id: 'p1', fileId: 'b', anchorIdx: 1 }];
  const threadsHost = {
    querySelector() { return null; },
    set innerHTML(value) { assert.equal(value, ''); mounted.length = 0; },
    appendChild(thread) { mounted.push(thread); },
  };
  function createThread(t) {
    const children = [];
    const list = { appendChild(node) { children.push(node); }, set innerHTML(_) { children.length = 0; } };
    const thread = {
      children,
      getAttribute(name) { return name === 'data-thread-id' ? String(t.id) : null; },
      querySelector(selector) {
        if (selector === '.user-comment-list') return list;
        if (selector === '.thread-activity') return children.at(-1) ?? null;
        return null;
      },
    };
    sandbox.renderFollowups(thread, String(t.id));
    return thread;
  }
  const sandbox = {
    Date: class extends Date { static now() { return now; } static parse(value) { return Date.parse(value); } },
    document: {
      activeElement: null,
      querySelectorAll(selector) { assert.equal(selector, '.thread'); return mounted; },
      createElement() {
        return { setAttribute(key, value) { this[key] = value; }, remove() {
          for (const thread of mounted) {
            const index = thread.children.indexOf(this);
            if (index >= 0) thread.children.splice(index, 1);
          }
        } };
      },
    },
    loadState: () => state,
    cloneState: value => structuredClone(value),
    setCurrentState(next) { Object.assign(state, next); },
    refreshThreadFromState(id) {
      const thread = mounted.find(item => item.getAttribute('data-thread-id') === id);
      if (thread) sandbox.renderFollowups(thread, id);
    },
    threadsHost, prepopulated, DEFAULT_FILE_ID: 'a',
    createPrepopulatedThread: createThread, createUserThread: createThread,
    threadFileId: thread => thread.fileId,
    commentsForThread: () => [],
    escapeHtml: value => value,
    renderMarkers() {}, updateEmptyState() {}, renderMetaStatus() {},
    setTimeout(fn, ms) { const id = ++nextTimer; timers.set(id, { fn, ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  Object.defineProperty(sandbox, 'activeFileId', { get: () => activeFileId });
  vm.createContext(sandbox);
  vm.runInContext(source + '\nthis.renderFollowups = renderFollowups; this.activity = { applyThreadActivityEvent, applyThreadResolvedEvent, clearThreadActivity, clearAllThreadActivity, renderAllThreads };', sandbox);
  function render(fileId) { activeFileId = fileId; sandbox.activity.renderAllThreads(); }
  render('a');
  return { sandbox, state, mounted, timers, render, setNow(value) { now = value; } };
}

const active = (threadId, status = 'thinking', expiry = 3000) =>
  ({ threadId, status, expiresAt: new Date(expiry).toISOString() });

test('unknown and expired events do not create phantom threads; renewal replaces timer', () => {
  const h = harness();
  h.sandbox.activity.applyThreadActivityEvent(active('missing'));
  h.sandbox.activity.applyThreadActivityEvent(active('t1', 'thinking', 999));
  assert.equal(h.mounted.length, 1);
  assert.equal(h.mounted[0].children.length, 0);
  assert.equal(h.timers.size, 0);
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  assert.equal(h.mounted[0].children[0]['aria-label'], 'Agent thinking');
  assert.equal(h.mounted[0].children[0].role, 'status');
  const oldId = [...h.timers.keys()][0];
  h.sandbox.activity.applyThreadActivityEvent(active('t1', 'replying', 4000));
  assert.equal(h.mounted[0].children.length, 1);
  assert.equal(h.mounted[0].children[0]['aria-label'], 'Agent replying');
  assert.equal(h.timers.size, 1);
  assert.equal(h.timers.has(oldId), false);
  assert.equal([...h.timers.values()][0].ms, 3000);
});

test('off-file prepopulated activity survives file switch and full rerender until idle or expiry', () => {
  const h = harness();
  h.sandbox.activity.applyThreadActivityEvent(active('p1'));
  assert.equal(h.mounted.length, 1);
  assert.equal(h.mounted[0].children.length, 0);
  assert.equal(h.timers.size, 1);
  h.render('b');
  assert.equal(h.mounted[0].getAttribute('data-thread-id'), 'p1');
  assert.equal(h.mounted[0].children[0]['aria-label'], 'Agent thinking');
  h.render('b');
  assert.equal(h.mounted[0].children.length, 1);
  h.sandbox.activity.applyThreadActivityEvent({ threadId: 'p1', status: 'idle', expiresAt: null });
  assert.equal(h.mounted[0].children.length, 0);
  h.sandbox.activity.applyThreadActivityEvent(active('p1'));
  const timer = [...h.timers.values()][0];
  h.setNow(3000);
  timer.fn();
  h.render('b');
  assert.equal(h.mounted[0].children.length, 0);
  assert.equal(h.timers.size, 0);
});

test('off-file user thread activity renders on return; resolution then late renewal cannot revive it', () => {
  const h = harness();
  h.render('b');
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  h.render('a');
  assert.equal(h.mounted[0].children[0]['aria-label'], 'Agent thinking');
  h.sandbox.activity.applyThreadResolvedEvent({ threadId: 't1', resolution: {
    decision: 'Done', resolvedAt: new Date(1200).toISOString(), resolvedBy: 'agent',
  } });
  assert.equal(h.mounted[0].children.length, 0);
  assert.equal(h.timers.size, 0);
  h.sandbox.activity.applyThreadActivityEvent(active('t1', 'replying', 4000));
  h.render('a');
  assert.equal(h.mounted[0].children.length, 0);
  assert.equal(h.timers.size, 0);
  assert.equal(h.state.resolutions.t1.resolvedBy, 'agent');
});
