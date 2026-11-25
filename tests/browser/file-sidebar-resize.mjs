// Run with: node --test tests/browser/file-sidebar-resize.mjs
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const html = fs.readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const source = html.slice(html.indexOf('  // File list width is'), html.indexOf('  // Divider drag'));

function setup() {
  const listeners = new Map();
  const attributes = new Map();
  const classes = new Set();
  let width = 236;
  let repositions = 0;
  const classList = { add: name => classes.add(name), remove: name => classes.delete(name) };
  const handle = {
    classList,
    setAttribute: (key, value) => attributes.set(key, value),
    addEventListener: (key, fn) => listeners.set(key, fn),
    removeEventListener: key => listeners.delete(key),
    setPointerCapture: () => {},
  };
  vm.runInNewContext(source, {
    document: {
      body: { classList, style: { setProperty: (key, value) => {
        assert.equal(key, '--files-expanded-w');
        width = parseFloat(value);
      } } },
      getElementById: id => ({
        'file-sidebar-resizer': handle,
        'file-sidebar': { getBoundingClientRect: () => ({ width }) },
        'workspace-grid': { clientWidth: 1400 },
      })[id],
    },
    paneLeft: { getBoundingClientRect: () => ({ width: 840 }) },
    scheduleReposition: () => repositions++,
  });
  return {
    listeners, attributes, classes,
    width: () => width,
    repositions: () => repositions,
    fire: (name, event = {}) => listeners.get(name)({ preventDefault() {}, ...event }),
  };
}

test('keyboard resize clamps to available space and resets to default', () => {
  const ui = setup();
  ui.fire('keydown', { key: 'ArrowRight' });
  assert.equal(ui.width(), 246);
  ui.fire('keydown', { key: 'ArrowLeft', shiftKey: true });
  assert.equal(ui.width(), 206);
  ui.fire('keydown', { key: 'Home' });
  assert.equal(ui.width(), 160);
  ui.fire('keydown', { key: 'End' });
  assert.equal(ui.width(), 320);
  assert.equal(ui.attributes.get('aria-valuemax'), '320');
  assert.equal(ui.attributes.get('aria-valuenow'), '320');
  ui.fire('dblclick');
  assert.equal(ui.width(), 236);
  assert.equal(ui.repositions(), 5);
});

test('pointer resize cleans up after capture loss, including cancellation', () => {
  const ui = setup();
  ui.fire('pointerdown', { button: 2 });
  assert.equal(ui.listeners.has('pointermove'), false);
  ui.fire('pointerdown', { button: 0, clientX: 236, pointerId: 1 });
  ui.fire('pointermove', { clientX: 280 });
  assert.equal(ui.width(), 280);
  ui.fire('pointermove', { clientX: -100 });
  assert.equal(ui.width(), 160);
  ui.fire('lostpointercapture');
  assert.equal(ui.listeners.has('pointermove'), false);
  assert.equal(ui.classes.size, 0);
});
