import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const extract = (start, end) => {
  const from = page.indexOf(start);
  const to = page.indexOf(end, from + start.length);
  assert.ok(from >= 0 && to > from, `missing source delimiters: ${start}, ${end}`);
  return page.slice(from, to);
};

test('switching files clears proposal choices before rendering the new file', () => {
  const calls = [];
  const context = vm.createContext({
    activeFileId: 'a', renderedFileHtml: new Map([['b', 'html']]),
    document: { getElementById: () => ({}), querySelector: () => null },
    newThreadEditor: null, cancelImageSelection: null, setInspectMode() {}, stashBlockEditorForRerender() {},
    resetProposalReview: () => calls.push('reset'),
    mountFile: () => calls.push('mount'), activateFileUi() {}, isHtmlFile: () => true,
    renderAllThreads() {}, syncEditReadonlyClass() {}, restoreBlockEditorAfterRerender() {},
    blockEditorStashes: new Map(), renderProposalUi: () => calls.push('render'),
    updateFileSidebar() {}, scheduleReposition() {}, setTimeout() {},
  });
  vm.runInContext(extract('  function switchToFile(', '  function prepareActiveFileDocument('), context);
  vm.runInContext("switchToFile('b')", context);
  assert.deepEqual(calls, ['reset', 'mount', 'render']);
  assert.equal(context.activeFileId, 'b');
});

test('proposal resync retries a snapshot overtaken by an SSE event', async () => {
  const map = new Map();
  let fetches = 0;
  let resets = 0;
  const context = vm.createContext({
    proposalLoadId: 0, proposalRevision: 0, proposalsByFile: map,
    activeProposal: () => map.get('a'), resetProposalReview: () => resets++,
    setHoldUi() {}, renderProposalUi() {}, console,
    fetch: async () => ({ ok: true, json: async () => {
      fetches++;
      if (fetches === 1) context.proposalRevision++;
      return { proposals: [{ fileId: 'a', proposalId: fetches === 1 ? 'old' : 'new' }] };
    } }),
  });
  vm.runInContext(extract('  async function loadProposals()', '  function applyProposalCreated('), context);
  await vm.runInContext('loadProposals()', context);
  assert.equal(fetches, 2);
  assert.equal(map.get('a').proposalId, 'new');
  assert.equal(resets, 1);
  assert.match(page, /source\.onopen = .*loadProposals\(\)/);
});

test('agent status expires on the activity timer without another server event', () => {
  const refreshed = [];
  const el = { replaceChildren() {} };
  const meta = { hidden: false };
  let timer;
  const now = Date.now();
  const context = vm.createContext({
    agentStatuses: [{ label: 'Working', threadId: 't1', startedAt: new Date(now - 600001).toISOString() }],
    AGENT_STATUS_TTL_MS: 600000, AGENT_REPLY_WAIT_MS: 300000,
    agentActivityTimer: null, reviewComplete: false, threadActivity: new Map(),
    loadState: () => ({ userThreads: [], resolutions: {} }),
    refreshThreadFromState: id => refreshed.push(id),
    document: { getElementById: id => id === 'agent-status' ? el : meta, querySelectorAll: () => [], querySelector: () => null },
    clearTimeout() {}, setTimeout: fn => { timer = fn; },
  });
  vm.runInContext(extract('  function renderAgentActivity()', '  function applyElementDetachment('), context);
  vm.runInContext('renderAgentActivity()', context);
  assert.equal(context.agentStatuses.length, 0);
  assert.deepEqual(refreshed, ['t1']);
  assert.equal(el.hidden, true);
  assert.equal(meta.hidden, false);
  assert.equal(timer, undefined, 'expired status must not keep scheduling activity ticks');
});

test('document width resolves sidebar variables on body rather than root', () => {
  const root = extract('  :root {', '  html[data-theme');
  assert.doesNotMatch(root, /--left-w:/);
  assert.match(page, /body \{\s*--available-pane-w: max\(0px, calc\(100% - var\(--files-w\) - var\(--divider-w\)\)\);/);
  assert.match(page, /--thread-w: min\(var\(--side-w\), max\(0px, calc\(var\(--available-pane-w\) - 320px\)\)\);/);
  assert.match(page, /--left-w: calc\(var\(--available-pane-w\) - var\(--thread-w\)\);/);
  assert.match(page, /th\.cell-nowrap, #doc-content td\.cell-nowrap \{ white-space: nowrap; \}/);
});
