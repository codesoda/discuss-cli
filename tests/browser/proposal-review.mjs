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
    newThreadEditor: null, setInspectMode() {}, stashBlockEditorForRerender() {},
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

test('document width resolves sidebar variables on body rather than root', () => {
  const root = extract('  :root {', '  html[data-theme');
  assert.doesNotMatch(root, /--left-w:/);
  assert.match(page, /body \{ --left-w: calc\(100% - var\(--files-w\) - var\(--divider-w\) - var\(--side-w\)\); \}/);
});
