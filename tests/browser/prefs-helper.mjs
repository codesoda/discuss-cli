// Run with: node --test tests/browser/prefs-helper.mjs
//
// The preference helper is what makes a setting outlive one review: the CLI
// injects the saved values, the helper reads them, and every change is posted
// back. Browser storage plays no part, because each session binds a fresh
// port and gets a fresh origin.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const html = fs.readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const open = '<script id="discuss-prefs-client">';
const start = html.indexOf(open) + open.length;
const source = html.slice(start, html.indexOf('</script>', start));

function setup({ saved = null, failingFetch = false } = {}) {
  const posts = [];
  const window = {};
  if (saved !== null) window.__DISCUSS_PREFS__ = saved;

  vm.runInNewContext(source, {
    window,
    fetch: (url, options) => {
      if (failingFetch) throw new Error('network down');
      posts.push({ url, body: JSON.parse(options.body), method: options.method });
      return { catch: () => {} };
    },
  });

  return { window, posts };
}

test('a value saved by the CLI is what the page reads', () => {
  const ui = setup({ saved: { cmdEnterToSend: false, theme: 'dark' } });

  assert.equal(ui.window.discussReadPref('cmdEnterToSend', true), false);
  assert.equal(ui.window.discussReadPref('theme', null), 'dark');
});

test('an unset preference falls back to its first-run default', () => {
  const ui = setup();

  assert.equal(ui.window.discussReadPref('cmdEnterToSend', true), true);
  assert.equal(ui.window.discussReadPref('filesCollapsed', false), false);
  assert.equal(ui.window.discussReadPref('theme', null), null);
});

test('a change is posted to the CLI and visible to the page at once', () => {
  const ui = setup({ saved: {} });

  ui.window.discussWritePref('cmdEnterToSend', false);

  assert.deepEqual(ui.posts, [{ url: '/api/prefs', method: 'POST', body: { cmdEnterToSend: false } }]);
  assert.equal(ui.window.discussReadPref('cmdEnterToSend', true), false);
});

test('a failed post still keeps the change for this page', () => {
  const ui = setup({ failingFetch: true });

  ui.window.discussWritePref('theme', 'light');

  assert.equal(ui.window.discussReadPref('theme', null), 'light');
});

test('the helper never touches browser storage', () => {
  assert.equal(source.includes('localStorage'), false);
});
