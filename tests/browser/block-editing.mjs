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
  const source = between('  // Capture-phase so edit-mode clicks', '  async function openBlockEditor');
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
      target: { closest: selector => selector === '.block-editor' ? null : anchor },
      preventDefault: () => { prevented = true; },
      stopImmediatePropagation: () => { stopped = true; },
    });
    assert.equal(prevented, editable);
    assert.equal(stopped, editable);
    assert.equal(opened, editable);
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

test('out-of-order source events update each file without lowering the session version', () => {
  const source = between('  const appliedSourceVersions = new Map();', '  // ---------- Version badge');
  let state = { sourceVersion: 0, userThreads: [
    { id: 'a', anchorStart: 1, anchorEnd: 1 },
    { id: 'b', anchorStart: 1, anchorEnd: 1 },
  ] };
  const html = new Map();
  const context = vm.createContext({
    document: { getElementById: () => ({}) },
    activeFileId: 'other', DEFAULT_FILE_ID: 'a', renderedFileHtml: html,
    loadState: () => state,
    cloneState: value => structuredClone(value),
    setCurrentState: value => { state = value; }, renderMetaStatus() {},
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
