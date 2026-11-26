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
  between('  function applyReplyAddedEvent(payload)', '  function applyTakeAddedEvent(payload)'),
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
    followups: {}, replies: {}, takes: {}, resolutions: {} };
  const prepopulated = [{ id: 'p1', fileId: 'b', anchorIdx: 1 }];
  const threadsHost = {
    querySelector() { return null; },
    set innerHTML(value) { assert.equal(value, ''); mounted.length = 0; },
    appendChild(thread) { mounted.push(thread); },
  };
  function createThread(t) {
    const children = [];
    const list = {
      pending: null,
      appendChild(node) { children.push(node); },
      insertAdjacentHTML(_position, html) { this.pending = html; },
      set innerHTML(_) { children.length = 0; this.pending = null; },
    };
    const thread = {
      children,
      getAttribute(name) { return name === 'data-thread-id' ? String(t.id) : null; },
      querySelector(selector) {
        if (selector === '.user-comment-list') return list;
        if (selector === '.thread-activity') return children.at(-1) ?? null;
        if (selector === '.agent-pending') return list.pending && { remove() { list.pending = null; } };
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
      getElementById: () => sandbox.header ?? null,
      querySelector: () => null,
      querySelectorAll(selector) {
        if (selector === '.agent-pending[data-since]') return [];
        assert.equal(selector, '.thread'); return mounted;
      },
      createElement() {
        return { append() {}, setAttribute(key, value) { this[key] = value; }, remove() {
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
    awaitingAgentSince: () => sandbox.pendingSince ?? null,
    agentPendingHtml: () => '<div class="agent-pending">Claude is on it</div>',
    renderAgentActivity() {}, agentStatusForThread: () => null, reviewComplete: false,
    normalizeEventComment: payload => payload.reply,
    upsertRawComment: (items, reply) => [...(items || []), reply],
    upsertDisplayComment: (items, reply) => [...(items || []), reply],
    escapeHtml: value => value,
    renderMarkers() {}, updateEmptyState() {}, renderMetaStatus() {},
    setTimeout(fn, ms) { const id = ++nextTimer; timers.set(id, { fn, ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  Object.defineProperty(sandbox, 'activeFileId', { get: () => activeFileId });
  vm.createContext(sandbox);
  vm.runInContext(source + '\nthis.renderFollowups = renderFollowups; this.activity = { applyThreadActivityEvent, applyReplyAddedEvent, applyThreadResolvedEvent, clearThreadActivity, clearAllThreadActivity, renderAllThreads };', sandbox);
  function render(fileId) { activeFileId = fileId; sandbox.activity.renderAllThreads(); }
  render('a');
  return { sandbox, state, mounted, timers, render, setNow(value) { now = value; } };
}

const active = (threadId, status = 'thinking', expiry = 3000) =>
  ({ threadId, status, expiresAt: new Date(expiry).toISOString() });

test('active lease replaces generic pending indicator; expiry restores it without duplication', () => {
  const h = harness();
  h.sandbox.pendingSince = 500;
  h.render('a');
  const thread = h.mounted[0];
  assert.match(thread.querySelector('.user-comment-list').pending, /Claude is on it/);
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  assert.equal(thread.querySelector('.user-comment-list').pending, null);
  assert.equal(thread.children.length, 1);
  h.render('a');
  assert.equal(h.mounted[0].querySelector('.user-comment-list').pending, null);
  const timer = [...h.timers.values()][0];
  h.setNow(3000);
  timer.fn();
  assert.match(h.mounted[0].querySelector('.user-comment-list').pending, /Claude is on it/);
  assert.equal(h.mounted[0].children.length, 0);
});

test('header includes active off-file leases without counting them as pending twice', () => {
  const h = harness();
  h.sandbox.header = { replaceChildren(...children) { this.chips = children; }, hidden: true };
  h.sandbox.pendingSince = 500;
  vm.runInContext('const AGENT_REPLY_WAIT_MS = 300000; const AGENT_STATUS_TTL_MS = 600000; let agentStatuses = []; let agentActivityTimer = null;\n'
    + between('  function renderAgentActivity() {', '  function applyElementDetachment(state, fileId, detached, route = null) {'), h.sandbox);
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  h.sandbox.activity.applyThreadActivityEvent(active('p1', 'replying'));
  assert.deepEqual(h.sandbox.header.chips.map(chip => chip.title), ['Agent thinking…', 'Agent replying…']);
  assert.equal(h.sandbox.header.hidden, false);
  h.sandbox.activity.applyThreadActivityEvent({ threadId: 't1', status: 'idle', expiresAt: null });
  assert.deepEqual(h.sandbox.header.chips.map(chip => chip.title), ['Agent replying…', 'Replying to 1 comment']);
});

test('direct activity handlers ignore late events and rerenders after completion', () => {
  const h = harness();
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  h.sandbox.reviewComplete = true;
  h.sandbox.activity.clearAllThreadActivity();
  h.sandbox.activity.applyThreadActivityEvent(active('t1', 'replying'));
  assert.equal(h.timers.size, 0);
  h.render('a');
  assert.equal(h.mounted[0].children.length, 0);
  // Even an already-present entry cannot render on a completed page.
  h.sandbox.reviewComplete = false;
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  h.sandbox.reviewComplete = true;
  h.render('a');
  assert.equal(h.mounted[0].children.length, 0);
});

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

test('human reply retains agent activity and its existing expiry timer', () => {
  const h = harness();
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  const timer = [...h.timers.keys()][0];
  h.sandbox.activity.applyReplyAddedEvent({ reply: { id: 'r1', threadId: 't1', text: 'Clarification' } });
  assert.equal(h.mounted[0].children[0]['aria-label'], 'Agent thinking');
  assert.equal(h.timers.has(timer), true);
  assert.equal(h.state.replies.t1[0].text, 'Clarification');
});

test('failed optimistic resolve restores the indicator and original lease', async () => {
  const h = harness();
  h.sandbox.activity.applyThreadActivityEvent(active('t1'));
  const timer = [...h.timers.keys()][0];
  const ta = { value: '' };
  const followup = {};
  Object.assign(h.sandbox, {
    clearMutationError() {}, clearFollowupDraft() {}, syncClearVisibility() {},
    renderResolution(thread, id) { h.sandbox.renderFollowups(thread, id); },
    renderMetaStatus() {}, reposition() {}, showMutationError() {},
    threadApiPath: () => '/resolve', apiJson: async () => { throw Error('offline'); },
    console: { error() {} },
  });
  const resolveSource = between('    async function resolve() {', '    saveBtn.addEventListener');
  vm.runInContext(`this.testResolve = async function () {
    const threadId = 't1', threadEl = threadsHost.querySelector() || document.querySelectorAll('.thread')[0];
    const ta = this.testTa, followup = this.testFollowup;
    ${resolveSource}
    await resolve();
  }`, h.sandbox);
  h.sandbox.testTa = ta;
  h.sandbox.testFollowup = followup;
  await h.sandbox.testResolve();
  assert.equal(h.state.resolutions.t1, undefined);
  assert.equal(h.mounted[0].children[0]['aria-label'], 'Agent thinking');
  assert.equal(h.timers.has(timer), true);
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
