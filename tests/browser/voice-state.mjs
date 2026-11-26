// Focused state regressions; no microphone or installed transcription engine needed.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../../discuss.html', import.meta.url), 'utf8');
const source = html.slice(html.indexOf('  function showVoiceBadge('), html.indexOf('  async function getVoiceStream('));
let pending = new Map(), next = 0, shown = false;
const context = vm.createContext({
  voice: { lastPointer: { x: 0, y: 0 } },
  voiceBadge: { querySelector: () => ({}), style: {}, classList: {
    toggle() {}, add() { shown = true; }, remove() { shown = false; },
  } },
  setTimeout: fn => { pending.set(++next, fn); return next; },
  clearTimeout: id => pending.delete(id),
});
vm.runInContext(source + "showVoiceBadge('Too short'); hideVoiceBadge(800); showVoiceBadge('Listening');", context);
for (const fn of pending.values()) fn();
assert.equal(shown, true, 'an old timer must not hide a new recording');
const create = html.slice(html.indexOf('  async function createVoiceThread('), html.indexOf("  document.addEventListener('mousemove'", html.indexOf('  async function createVoiceThread(')));
assert.ok(create.indexOf('const snippet =') < create.indexOf('await fetch'));
assert.ok(create.indexOf('const sourceVersion =') < create.indexOf('await fetch'));
assert.match(create, /if \(!created\) throw new Error/);
const blur = html.slice(html.indexOf("  window.addEventListener('blur', () => {", html.indexOf('// ---------- Voice notes:')), html.indexOf("  document.addEventListener('keydown'", html.indexOf('// ---------- Voice notes:')));
assert.match(blur, /voice.keyHeld = false/);
assert.match(blur, /voice.suppressClick = false/);
console.log('Voice state regressions passed');
