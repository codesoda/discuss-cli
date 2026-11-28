import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
function between(start, end) {
  const from = page.indexOf(start);
  const to = page.indexOf(end, from + start.length);
  assert.ok(from >= 0 && to > from, `${start} → ${end}`);
  return page.slice(from, to);
}

function harness() {
  const agent = { hidden: true, children: [], replaceChildren(...children) { this.children = children; } };
  const meta = { hidden: false, innerHTML: '' };
  const elements = { 'agent-status': agent, 'meta-status': meta };
  const state = { userThreads: [], resolutions: {} };
  const context = vm.createContext({
    document: {
      getElementById: id => elements[id] || null,
      createElement: () => ({ setAttribute() {}, append(...children) { this.children = children; } }),
      querySelectorAll: () => [], querySelector: () => null,
    },
    agentStatuses: [], agentActivityTimer: null, threadActivity: new Map(), reviewComplete: false,
    AGENT_STATUS_TTL_MS: 600000, AGENT_REPLY_WAIT_MS: 300000,
    loadState: () => state, refreshThreadFromState() {},
    awaitingAgentSince: () => Date.now(), activeThreadActivity: () => null,
    agentStatusForThread: id => context.agentStatuses.find(status => status.threadId === id) || null,
    clearTimeout() {}, setTimeout() {},
    prepopulated: [], isImageFile: () => false, countLiveDrafts: () => 2,
    updateFileSidebar() {},
  });
  vm.runInContext(between('  function renderAgentActivity()', '  function applyElementDetachment('), context);
  vm.runInContext(between('  function renderMetaStatus()', '  function threadSummaryEntries('), context);
  return { agent, meta, state, context, renderAgent: () => vm.runInContext('renderAgentActivity()', context),
    renderMeta: () => vm.runInContext('renderMetaStatus()', context) };
}

test('pending copy stays provider-neutral, including stale and explicit status labels', () => {
  const context = vm.createContext({
    agentStatuses: [], AGENT_REPLY_WAIT_MS: 300000, escapeHtml: value => value,
    agentStatusForThread: () => null,
  });
  vm.runInContext(between('  function agentPendingHtml(', '  function setAgentStatuses('), context);
  assert.match(vm.runInContext('agentPendingHtml("t", Date.now())', context), /Waiting for agent…/);
  assert.match(vm.runInContext('agentPendingHtml("t", Date.now() - 300001)', context), /No reply yet/);
  context.agentStatusForThread = () => ({ label: 'Reviewing changes' });
  assert.match(vm.runInContext('agentPendingHtml("t", Date.now())', context), /Reviewing changes/);
  assert.doesNotMatch(between('  // ---------- Agent working indicator ----------', '  function applyElementDetachment('), /Claude/);
});

test('agent activity substitutes for counts, and idle, expiry, and completion restore them', () => {
  const h = harness();
  h.state.userThreads = [{ id: 't1' }];
  h.renderMeta();
  assert.equal(h.meta.hidden, false);
  assert.match(h.meta.innerHTML, /2 drafts/);

  h.renderAgent(); // Waiting for a take, even without a posted status.
  assert.equal(h.agent.hidden, false);
  assert.equal(h.meta.hidden, true);
  assert.equal(h.agent.children[0].title, 'Waiting for agent…');
  assert.equal(h.agent.children[0].children[1].textContent, 'Waiting for agent…');
  h.renderMeta(); // A count refresh must not re-show the duplicate status.
  assert.equal(h.meta.hidden, true);
  assert.match(h.meta.innerHTML, /2 drafts/);

  h.state.userThreads = [];
  h.context.agentStatuses = [{ label: 'Working on a long task', startedAt: new Date().toISOString() }];
  h.renderAgent();
  assert.equal(h.agent.children[0].title, 'Working on a long task');
  assert.equal(h.meta.hidden, true);
  h.context.agentStatuses = [{ label: 'Expired', startedAt: new Date(Date.now() - 600001).toISOString() }];
  h.renderAgent();
  assert.equal(h.agent.hidden, true);
  assert.equal(h.meta.hidden, false);
  assert.match(h.meta.innerHTML, /2 drafts/);

  h.context.agentStatuses = [{ label: 'Working', startedAt: new Date().toISOString() }];
  h.renderAgent();
  h.context.reviewComplete = true;
  h.renderAgent();
  assert.equal(h.agent.hidden, true);
  assert.equal(h.meta.hidden, false);
});

test('header status shares the right slot before resolved and finish, with bounded right-aligned chips', () => {
  const header = between('<header>\n  <h1>Discuss</h1>', '</header>');
  const positions = ['id="meta-status"', 'id="agent-status"', 'id="thread-summary-wrap"', 'id="done-control"']
    .map(id => header.indexOf(id));
  assert.ok(positions.every((value, i) => value >= 0 && (!i || value > positions[i - 1])));
  assert.ok(header.indexOf('id="agent-status"') > header.indexOf('class="spacer"'));
  assert.match(page, /\.agent-status \{[^}]*justify-content: flex-end;[^}]*max-width: min\(40vw, 400px\);[^}]*overflow: hidden;/);
  assert.match(page, /\.agent-status-label \{[^}]*text-overflow: ellipsis;/);
  assert.match(page, /chip\.title = label;/);
  assert.match(header, /id="agent-status" role="status" aria-live="polite" hidden/);
});
