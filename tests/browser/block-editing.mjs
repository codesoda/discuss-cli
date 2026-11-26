import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const between = (start, end) => {
  const from = page.indexOf(start);
  assert.ok(from >= 0, start);
  const to = page.indexOf(end, from + start.length);
  assert.ok(to > from, end);
  return page.slice(from, to);
};

test('edit clicks prevent link navigation, while read-only panes keep normal clicks', () => {
  const source = between('  const documentControlSelector', '  async function openBlockEditor');
  for (const editable of [true, false]) {
    let handler;
    let prevented = false;
    let stopped = false;
    let opened = false;
    const anchor = { getAttribute: () => '2' };
    vm.runInNewContext(source, {
      editMode: true, reviewComplete: false,
      activeFileIsEditable: () => editable,
      document: { addEventListener: (_, fn, capture) => { assert.equal(capture, true); handler = fn; } },
      openBlockEditor: () => { opened = true; },
    });
    handler({
      target: { closest: selector => ['#pane-left', '#doc-content [data-anchor-idx]'].includes(selector) ? anchor : null },
      preventDefault: () => { prevented = true; },
      stopImmediatePropagation: () => { stopped = true; },
    });
    assert.equal(prevented, editable);
    assert.equal(stopped, editable);
    assert.equal(opened, editable);
  }
});

test('edit mode leaves document controls clickable and samples prose typography', () => {
  const source = between('  const documentControlSelector', '  async function openBlockEditor');
  let handler;
  vm.runInNewContext(source, {
    editMode: true, reviewComplete: false, activeFileIsEditable: () => true,
    document: { addEventListener: (_, fn) => { handler = fn; } },
    openBlockEditor: () => assert.fail('control opened editor'),
  });
  for (const control of ['.thread-marker', '.thread-marker-stack', '.section-toggle']) {
    handler({
      target: { closest: selector => selector === '#pane-left' || selector.split(',').some(part => part.trim() === control) ? {} : null },
      preventDefault: () => assert.fail('control default blocked'),
      stopImmediatePropagation: () => assert.fail('control propagation blocked'),
    });
  }
  const typography = between('    const sample =', '\n    targetEl.style.display');
  for (const hasPre of [true, false]) {
    const prose = { fontFamily: 'serif' };
    const pre = { fontFamily: 'monospace' };
    const inlineCode = { fontFamily: 'inline-monospace' };
    const textarea = { style: {} };
    const targetEl = { querySelector: selector => selector === 'pre' ? (hasPre ? pre : null) : inlineCode };
    vm.runInNewContext(typography, {
      targetEl, textarea, getComputedStyle: element => element === targetEl ? prose : element,
    });
    assert.equal(textarea.style.fontFamily, hasPre ? 'monospace' : 'serif');
  }
});

test('bubble dispatcher ignores marker-stack gaps and section controls', () => {
  const declarations = between('  const documentControlSelector', '  // Capture-phase');
  const guards = between('  // ---------- Click dispatch ----------', '\n    const sel = window.getSelection();');
  let handler;
  vm.runInNewContext(declarations + guards + '\n throw new Error("comment dispatch reached");\n});', {
    document: { addEventListener: (_, fn) => { handler = fn; } },
  });
  for (const control of ['.thread-marker-stack', '.section-toggle']) {
    handler({ target: { closest: selector => selector.split(',').some(part => part.trim() === control) ? {} : null } });
  }
});

test('unchanged CRLF textarea content closes without a PATCH', async () => {
  const source = between('  async function commitBlockEditor', '  // The document moved under an open editor');
  let closed = false;
  let calls = 0;
  const context = vm.createContext({
    blockEditor: { textarea: { value: 'first\nsecond' }, original: 'first\r\nsecond', fileId: 'f-1' },
    blockEditorStashes: new Map(),
    closeBlockEditor: restore => { closed = restore; },
    apiJson: () => { calls++; throw new Error('unexpected PATCH'); },
    setBlockEditorError: () => {},
  });
  vm.runInContext(source, context);
  await vm.runInContext("commitBlockEditor('blur')", context);
  assert.equal(calls, 0);
  assert.equal(closed, true);
});

test('stale saves retry once, retain the draft, and offer an explicit Retry', async () => {
  const source = between('  async function commitBlockEditor', '  // A re-render or file switch');
  let patches = 0;
  let gets = 0;
  let retry;
  const editor = {
    textarea: { value: 'draft', disabled: false, focus() {} },
    original: 'original', sourceVersion: 0, anchorIdx: 2, fileId: 'f-1', el: {},
  };
  const context = vm.createContext({
    blockEditor: editor, blockEditorStashes: new Map(),
    setBlockEditorError() {},
    showMutationError: (_, message, action) => { retry = action; },
    apiJson: async (_, options) => {
      if (options.method === 'GET') {
        gets++;
        return { markdown: 'original', sourceVersion: gets };
      }
      patches++;
      throw Object.assign(new Error('stale'), { code: 'stale_source_version' });
    },
  });
  vm.runInContext(source, context);
  await vm.runInContext("commitBlockEditor('shortcut')", context);
  assert.equal(patches, 2);
  assert.equal(gets, 1);
  assert.equal(editor.textarea.value, 'draft');
  assert.equal(editor.textarea.disabled, false);
  assert.equal(editor.committing, false);
  assert.equal(typeof retry, 'function');
});

test('failed saves retain input and provide a working Retry closure', async () => {
  const source = between('  async function commitBlockEditor', '  // The document moved under an open editor');
  let retry;
  let patches = 0;
  const editor = {
    textarea: { value: 'draft', focus() {} }, original: 'original',
    sourceVersion: 0, anchorIdx: 2, fileId: 'f-1', el: {},
  };
  const context = vm.createContext({
    blockEditor: editor, blockEditorStashes: new Map(),
    setBlockEditorError() {},
    showMutationError: (_, message, action) => { retry = action; },
    apiJson: async () => { patches++; throw new Error('offline'); },
  });
  vm.runInContext(source, context);
  await vm.runInContext("commitBlockEditor('shortcut')", context);
  assert.equal(editor.textarea.value, 'draft');
  assert.equal(editor.textarea.disabled, false);
  retry();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(patches, 2);
});

test('changed block identity blocks subsequent blur, shortcut, and retry saves', async () => {
  let patches = 0;
  const editor = {
    textarea: { value: 'my draft', focus() {} }, original: 'original block',
    fileId: 'f-1', anchorIdx: 2, sourceVersion: 0,
  };
  const context = vm.createContext({
    blockEditor: editor, blockEditorStashes: new Map(), setBlockEditorError() {},
    apiJson: async (_, options) => {
      if (options.method === 'GET') return { markdown: 'unrelated block', sourceVersion: 1 };
      patches++;
      throw Object.assign(new Error('stale'), { code: 'stale_source_version' });
    },
  });
  vm.runInContext(between('  async function commitBlockEditor', '  // A re-render or file switch'), context);
  await vm.runInContext("commitBlockEditor('shortcut')", context);
  assert.equal(editor.conflicted, true);
  assert.equal(editor.original, 'original block', 'retain identity across later stashes');
  for (const reason of ['blur', 'shortcut', 'retry', 'mode-exit']) {
    context.reason = reason;
    await vm.runInContext('commitBlockEditor(reason)', context);
  }
  assert.equal(patches, 1);
  assert.equal(editor.textarea.value, 'my draft');
  assert.equal(editor.textarea.disabled, false);
});

test('failed in-flight PATCH retains its draft across rerenders and file switches', async () => {
  for (const activeFileId of ['f-1', 'f-2']) {
    let rejectPatch;
    let reopened;
    const editor = {
      textarea: { value: 'unsaved draft' }, original: 'original',
      fileId: 'f-1', anchorIdx: 2, sourceVersion: 0,
    };
    const stashes = new Map();
    const context = vm.createContext({
      blockEditor: editor, blockEditorStashes: stashes, activeFileId, editMode: true,
      setBlockEditorError() {}, showNoticeToast() {},
      apiJson: () => new Promise((resolve, reject) => { rejectPatch = reject; }),
      closeBlockEditor: () => { context.blockEditor = null; },
      document: { querySelector: () => ({}) },
      openBlockEditor: (idx, el, options) => { reopened = options; },
    });
    vm.runInContext(between('  async function commitBlockEditor', '  function showNoticeToast'), context);
    const request = vm.runInContext("commitBlockEditor('shortcut')", context);
    vm.runInContext('restoreBlockEditorAfterRerender(stashBlockEditorForRerender())', context);
    assert.equal(stashes.get('f-1').draft, 'unsaved draft');
    assert.equal(reopened, undefined, 'pending request must not reopen an editable draft');
    rejectPatch(new Error('offline'));
    await request;
    assert.equal(stashes.get('f-1').draft, 'unsaved draft');
    assert.equal(stashes.get('f-1').pending, null);
    if (activeFileId === 'f-1') assert.equal(reopened.draft, 'unsaved draft');
    else assert.equal(reopened, undefined);
  }
});

test('out-of-order source events update each file without lowering the session version', () => {
  const source = between('  const appliedSourceVersions = new Map();', '  // ---------- Version badge');
  let state = { sourceVersion: 0, userThreads: [
    { id: 'a', anchorStart: 1, anchorEnd: 1 },
    { id: 'b', anchorStart: 1, anchorEnd: 1 },
  ] };
  const html = new Map();
  const context = vm.createContext({
    document: { getElementById: () => ({}) },
    activeFileId: 'other', DEFAULT_FILE_ID: 'a', renderedFileHtml: html, proposalsByFile: new Map(),
    loadState: () => state,
    cloneState: value => structuredClone(value),
    setCurrentState: value => { state = value; }, renderMetaStatus() {}, renderProposalUi() {},
  });
  vm.runInContext(source, context);
  for (const [fileId, version, anchor] of [['b', 2, 4], ['a', 1, 3], ['b', 2, 99], ['b', 1, 99]]) {
    context.payload = {
      fileId, sourceVersion: version, renderedHtml: `${fileId}-${anchor}`,
      threadAnchors: [{ threadId: fileId, anchorStart: anchor, anchorEnd: anchor }],
    };
    vm.runInContext('applySourceUpdatedEvent(payload)', context);
  }
  assert.equal(html.get('a'), 'a-3');
  assert.equal(html.get('b'), 'b-4');
  assert.equal(state.sourceVersion, 2);
  assert.equal(state.userThreads[0].anchorStart, 3);
  assert.equal(state.userThreads[1].anchorStart, 4);
});

test('hydration syncs Edit visibility after selecting the initial file', () => {
  const source = between('    discussFiles = loadState().files || [];', '    if (isHtmlFile()) stashHtmlFrame(activeFileId);');
  for (const kind of ['markdown', 'diff', 'image', 'html']) {
    let syncedFile;
    const context = vm.createContext({
      loadState: () => ({ files: [{ id: kind }] }),
      discussFiles: [], activeFileId: null, appliedSourceVersions: new Map(),
      syncEditReadonlyClass: () => { syncedFile = context.activeFileId; },
    });
    vm.runInContext(source, context);
    assert.equal(syncedFile, kind);
  }
});
