// Run with: node --test tests/browser/prefs-helper.mjs
//
// The preference helper is what makes a setting outlive one review: the CLI
// injects the saved values, the helper reads them, and every change is posted
// back. Browser storage is only a same-session mirror, because each session
// binds a fresh port and gets a fresh origin.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const html = fs.readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const open = '<script id="discuss-prefs-client">';
const start = html.indexOf(open) + open.length;
const source = html.slice(start, html.indexOf('</script>', start));

function setup({ saved = null, storage = {}, throwingStorage = false } = {}) {
  const posts = [];
  const window = {};
  if (saved !== null) window.__DISCUSS_PREFS__ = saved;

  const localStorage = {
    getItem: key => {
      if (throwingStorage) throw new Error('storage disabled');
      return Object.prototype.hasOwnProperty.call(storage, key) ? storage[key] : null;
    },
    setItem: (key, value) => {
      if (throwingStorage) throw new Error('storage disabled');
      storage[key] = value;
    },
  };

  vm.runInNewContext(source, {
    window,
    localStorage,
    fetch: (url, options) => {
      posts.push({ url, body: JSON.parse(options.body), method: options.method });
      return { catch: () => {} };
    },
  });

  return { window, storage, posts };
}

test('a value saved by the CLI wins over whatever this browser remembers', () => {
  const ui = setup({ saved: { cmdEnterToSend: false }, storage: { 'discuss-cmd-enter-to-send': 'true' } });

  assert.equal(ui.window.discussReadPref('cmdEnterToSend', 'discuss-cmd-enter-to-send', true), false);
});

test('an existing browser choice carries over when nothing is saved yet', () => {
  const ui = setup({ saved: {}, storage: { 'discuss-cmd-enter-to-send': 'false', 'discuss-files-collapsed': '1', 'discuss-theme': 'dark' } });

  assert.equal(ui.window.discussReadPref('cmdEnterToSend', 'discuss-cmd-enter-to-send', true), false);
  assert.equal(ui.window.discussReadPref('filesCollapsed', 'discuss-files-collapsed', false), true);
  assert.equal(ui.window.discussReadPref('theme', 'discuss-theme', null), 'dark');
});

test('an unset preference falls back to its first-run default', () => {
  const ui = setup();

  assert.equal(ui.window.discussReadPref('cmdEnterToSend', 'discuss-cmd-enter-to-send', true), true);
  assert.equal(ui.window.discussReadPref('filesCollapsed', 'discuss-files-collapsed', false), false);
  assert.equal(ui.window.discussReadPref('theme', 'discuss-theme', null), null);
});

test('a change is posted to the CLI and mirrored for this session', () => {
  const ui = setup({ saved: {} });

  ui.window.discussWritePref('cmdEnterToSend', 'discuss-cmd-enter-to-send', false, 'false');

  assert.deepEqual(ui.posts, [{ url: '/api/prefs', method: 'POST', body: { cmdEnterToSend: false } }]);
  assert.equal(ui.storage['discuss-cmd-enter-to-send'], 'false');
  // The in-page copy updates too, so anything reading later sees the new value.
  assert.equal(ui.window.discussReadPref('cmdEnterToSend', 'discuss-cmd-enter-to-send', true), false);
});

test('blocked browser storage breaks neither reads nor writes', () => {
  const ui = setup({ saved: { theme: 'dark' }, throwingStorage: true });

  assert.equal(ui.window.discussReadPref('theme', 'discuss-theme', null), 'dark');
  assert.equal(ui.window.discussReadPref('cmdEnterToSend', 'discuss-cmd-enter-to-send', true), true);
  ui.window.discussWritePref('theme', 'discuss-theme', 'light', 'light');
  assert.deepEqual(ui.posts, [{ url: '/api/prefs', method: 'POST', body: { theme: 'light' } }]);
});
