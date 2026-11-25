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

test('hydration syncs Edit visibility after selecting the initial file', () => {
  const source = between('    discussFiles = loadState().files || [];', '    if (isHtmlFile()) stashHtmlFrame(activeFileId);');
  for (const kind of ['markdown', 'diff', 'image', 'html']) {
    let syncedFile;
    const context = vm.createContext({
      loadState: () => ({ files: [{ id: kind }] }),
      discussFiles: [], activeFileId: null,
      syncEditReadonlyClass: () => { syncedFile = context.activeFileId; },
    });
    vm.runInContext(source, context);
    assert.equal(syncedFile, kind);
  }
});
