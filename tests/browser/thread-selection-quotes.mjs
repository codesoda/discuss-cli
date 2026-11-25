// Run with: node --test tests/browser/thread-selection-quotes.mjs
import assert from 'node:assert/strict';
import { quoteFunction as source, selectionSource } from './fixtures/thread-selection-source.mjs';
import test from 'node:test';
import vm from 'node:vm';
const context = { Event };
vm.createContext(context);
vm.runInContext(source, context);

test('popup selects user/agent messages, copies raw text, quotes and dismisses', async () => {
  const listeners = {};
  const buttons = [0, 1].map(() => ({ addEventListener(type, fn) { this[type] = fn; } }));
  const status = {};
  const popup = { style: {}, children: buttons, offsetWidth: 180, offsetHeight: 40,
    setAttribute() {}, addEventListener() {}, querySelector() { return status; }, contains() { return false; } };
  const textarea = { value: 'Draft', dispatchEvent() {}, focus() {}, setSelectionRange() {}, closest() { return null; } };
  const thread = { isConnected: true, classList: { contains: () => true }, querySelector: () => textarea };
  const message = { closest: () => thread, querySelectorAll: () => [] };
  const element = { nodeType: 1, closest(selector) {
    if (selector === '.meta') return null;
    assert.ok(selector.includes('.user-comment'));
    assert.ok(selector.includes('.thread-take'));
    return message;
  } };
  let cleared = false;
  let copied;
  const selection = { isCollapsed: false, rangeCount: 1, toString: () => 'selected\ntext',
    getRangeAt: () => ({ commonAncestorContainer: element, getBoundingClientRect: () => ({ left: 100, top: 100 }) }),
    removeAllRanges() { cleared = true; } };
  let pending;
  const sandbox = { Event, reviewComplete: false, setTimeout(fn) { pending = fn; }, clearTimeout() { pending = null; }, async copyTextToClipboard(text) { copied = text; },
    document: { createElement: () => popup, body: { appendChild() {} }, addEventListener(type, fn) { listeners[type] = fn; } },
    window: { getSelection: () => selection, innerWidth: 800, innerHeight: 600, addEventListener() {} } };
  vm.runInNewContext(selectionSource, sandbox);
  const changed = listeners.selectionchange;
  listeners.selectionchange = () => { changed(); pending?.(); };
  listeners.selectionchange();
  assert.equal(popup.hidden, false);
  await buttons[0].click();
  assert.equal(copied, 'selected\ntext');
  assert.equal(popup.hidden, true);
  listeners.selectionchange();
  buttons[1].click();
  assert.equal(textarea.value, 'Draft\n\n> selected\n> text\n\n');
  assert.equal(cleared, true);
  listeners.selectionchange();
  listeners.keydown({ key: 'Escape' });
  assert.equal(popup.hidden, true);
  selection.isCollapsed = true;
  listeners.selectionchange();
  assert.equal(popup.hidden, true);
  selection.isCollapsed = false;
  sandbox.reviewComplete = true;
  listeners.selectionchange();
  assert.equal(buttons[1].disabled, true);
});

for (const [draft, separator] of [['', ''], ['Existing draft', '\n\n'], ['Draft\n', '\n'], ['Draft\n\n', '']]) {
  test(`appends multiline quote preserving draft ${JSON.stringify(draft)}`, () => {
    let focused = false;
    let selection;
    let inputs = 0;
    const textarea = {
      value: draft, scrollHeight: 123,
      dispatchEvent(event) { assert.equal(event.type, 'input'); assert.equal(event.bubbles, true); inputs++; },
      focus() { focused = true; },
      setSelectionRange(...args) { selection = args; },
    };
    context.appendQuotedSelection(textarea, 'first\r\nsecond\n\nlast');
    assert.equal(textarea.value, draft + separator + '> first\n> second\n> \n> last\n\n');
    const previous = textarea.value;
    context.appendQuotedSelection(textarea, 'another');
    assert.equal(textarea.value, previous + '> another\n\n');
    assert.deepEqual(selection, [textarea.value.length, textarea.value.length]);
    assert.equal(inputs, 2);
    assert.equal(focused, true);
    assert.equal(textarea.scrollTop, 123);
  });
}
