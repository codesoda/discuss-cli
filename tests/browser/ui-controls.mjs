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

test('image anchors retain legacy points and normalize only valid positive rectangles', () => {
  const source = between('  function normalizeImageAnchor(raw)', '  function normalizeElementAnchor(raw)');
  const context = vm.createContext({});
  vm.runInContext(source, context);
  const normalize = value => JSON.parse(JSON.stringify(vm.runInContext(`normalizeImageAnchor(${JSON.stringify(value)})`, context)));
  assert.deepEqual(normalize({ xPct: 0, yPct: 10000 }), { xPct: 0, yPct: 10000 });
  assert.deepEqual(normalize({ xPct: 100, yPct: 200, xEndPct: 900, yEndPct: 800 }),
    { xPct: 100, yPct: 200, xEndPct: 900, yEndPct: 800 });
  for (const anchor of [
    { xPct: 1, yPct: 2, xEndPct: 3 },
    { xPct: 1, yPct: 2, yEndPct: 3 },
    { xPct: 1, yPct: 2, xEndPct: 1, yEndPct: 3 },
    { xPct: 1, yPct: 2, xEndPct: 3, yEndPct: 2 },
    { xPct: 1, yPct: 2, xEndPct: 10001, yEndPct: 3 },
  ]) assert.equal(normalize(anchor), null);
});

test('image drag normalizes reverse direction and clamps to visible image', () => {
  const source = between('  function imagePoint(', '  function installImageInteractions()');
  const context = vm.createContext({});
  vm.runInContext(source, context);
  const result = vm.runInContext(`imageRectangle(imagePoint(150, 80, {left: 10, top: 20, width: 200, height: 100}), imagePoint(-10, 130, {left: 10, top: 20, width: 200, height: 100}))`, context);
  assert.deepEqual(JSON.parse(JSON.stringify(result)), { xPct: 0, yPct: 6000, xEndPct: 7000, yEndPct: 10000 });
});

function imageHarness() {
  const listeners = new Map();
  const docListeners = new Map();
  const layer = {
    children: [],
    appendChild(child) { this.children.push(child); child.parent = this; },
  };
  const parent = { querySelector: () => layer, appendChild: child => { child.parent = parent; } };
  const image = {
    dataset: {}, parentElement: parent, setAttribute() {},
    addEventListener: (name, callback) => listeners.set(name, callback),
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 200, height: 100 }),
    setPointerCapture(id) { this.captured = id; },
    hasPointerCapture(id) { return this.captured === id; },
    releasePointerCapture() { this.captured = null; },
  };
  const document = {
    querySelector: selector => selector.includes('img') ? image : layer,
    createElement: () => ({ style: {}, dataset: {}, setAttribute() {}, remove() { this.parent.children = this.parent.children.filter(child => child !== this); } }),
    addEventListener: (name, callback) => docListeners.set(name, callback),
  };
  const opened = [];
  const context = vm.createContext({ document, reviewComplete: false, isImageFile: () => true,
    openImagePinEditor: anchor => { opened.push({ ...anchor }); vm.runInContext('showImageDraftRegion(' + JSON.stringify(anchor) + ')', context); },
    setTimeout: () => {},
  });
  vm.runInContext(between('  let suppressImageClick = false;', '  function formatImagePercent('), context);
  vm.runInContext('installImageInteractions()', context);
  const key = (name, target = listeners.get('keydown')) => {
    const event = { key: name, preventDefault() { this.prevented = true; }, stopImmediatePropagation() {} };
    target(event);
    return event;
  };
  return { context, listeners, docListeners, layer, opened, key };
}

test('keyboard can choose arbitrary region corners and keeps the draft visible in the editor', () => {
  const h = imageHarness();
  h.key('r');
  assert.equal(h.layer.children.length, 1);
  assert.match(h.layer.children[0].className, /preview/);
  for (let i = 0; i < 8; i++) h.key('ArrowLeft');
  h.key('Enter');
  for (let i = 0; i < 3; i++) h.key('ArrowRight');
  for (let i = 0; i < 5; i++) h.key('ArrowDown');
  h.key('Enter');
  assert.deepEqual(h.opened, [{ xPct: 4200, yPct: 5000, xEndPct: 4500, yEndPct: 5500 }]);
  assert.equal(h.layer.children.length, 1, 'draft overlay remains until the editor closes');
  assert.equal(h.layer.children[0].style.width, '3%');
  vm.runInContext(between('  function closeNewThreadEditor()', '  let suppressImageClick = false;'), h.context);
  Object.assign(h.context, {
    newThreadEditor: { remove() {} }, activeNewThreadDraft: null,
    htmlThreadStateNeedsRender: false, renderMarkers() {},
  });
  h.context.document.querySelectorAll = () => [];
  vm.runInContext('closeNewThreadEditor()', h.context);
  assert.equal(h.layer.children.length, 0, 'cancel/save/file switch closes the draft overlay');
});

test('pointer release keeps the selected rectangle visible until the editor closes', () => {
  const h = imageHarness();
  h.listeners.get('pointerdown')({ pointerId: 1, button: 0, clientX: 150, clientY: 70, preventDefault() {} });
  h.listeners.get('pointermove')({ pointerId: 1, clientX: 50, clientY: 20 });
  h.listeners.get('pointerup')({ pointerId: 1, clientX: 50, clientY: 20 });
  assert.deepEqual(h.opened, [{ xPct: 2500, yPct: 2000, xEndPct: 7500, yEndPct: 7000 }]);
  assert.equal(h.layer.children.length, 1);
  assert.equal(h.layer.children[0].style.width, '50%');
});

test('Escape cancels both pointer and keyboard selection and removes the preview', () => {
  const h = imageHarness();
  h.listeners.get('pointerdown')({ pointerId: 1, button: 0, clientX: 10, clientY: 10, preventDefault() {} });
  h.listeners.get('pointermove')({ pointerId: 1, clientX: 100, clientY: 70 });
  assert.equal(h.layer.children.length, 1);
  h.key('Escape', h.docListeners.get('keydown'));
  assert.equal(h.layer.children.length, 0);
  assert.equal(h.opened.length, 0);
  h.key('r');
  h.key('Enter');
  h.key('ArrowRight');
  assert.equal(h.layer.children.length, 1);
  h.key('Escape', h.docListeners.get('keydown'));
  assert.equal(h.layer.children.length, 0);
  assert.equal(h.opened.length, 0);
});

test('Escape suppression survives a delayed pointer release and resets for the next gesture', () => {
  const h = imageHarness();
  const timers = [];
  h.context.setTimeout = callback => timers.push(callback);
  const down = { pointerId: 1, button: 0, clientX: 10, clientY: 10, preventDefault() {} };
  h.listeners.get('pointerdown')(down);
  h.listeners.get('pointermove')({ pointerId: 1, clientX: 100, clientY: 70 });
  h.key('Escape', h.docListeners.get('keydown'));
  timers.forEach(callback => callback()); // Time passes before the user releases.
  h.listeners.get('pointerup')({ pointerId: 1, clientX: 100, clientY: 70 });
  assert.equal(vm.runInContext('suppressImageClick', h.context), true);
  assert.equal(h.opened.length, 0);
  h.listeners.get('pointerdown')({ ...down, pointerId: 2 });
  assert.equal(vm.runInContext('suppressImageClick', h.context), false);
});

test('demo strip stays off-black with one consistently sized dropdown and no empty spacing', () => {
  assert.match(page, /--demo-bg: #1b1b1b;/);
  assert.match(page, /\.demo-strip \{[^}]*gap: 10px;[^}]*background: var\(--demo-bg\);/);
  assert.match(page, /#demo-notice:empty \{ display: none; \}/);
  assert.match(page, /\.demo-scenarios \{[^}]*height: 28px;/);
  assert.doesNotMatch(page, /<label for="demo-scenarios">Scenario<\/label>/);
  assert.match(page, /id="demo-scenarios" aria-label="Demo scenario"/);
});

test('settings theme options stay inside the dialog; Tab skips disabled and hidden controls', () => {
  const dropdown = between('  header .theme-menu-list {', '  header .theme-menu-list[hidden]');
  assert.match(dropdown, /position: absolute/);
  assert.doesNotMatch(dropdown, /settings-dialog/);
  assert.match(page, /\.settings-dialog \.theme-menu-list \{ position: static;/);
  let handler;
  const first = { getClientRects: () => [1], focus() { focused = this; } };
  const disabled = { disabled: true, getClientRects: () => [1] };
  const hidden = { getClientRects: () => [] };
  const last = { getClientRects: () => [1], focus() { focused = this; } };
  let focused;
  const context = vm.createContext({
    settingsModal: { addEventListener: (_, fn) => { handler = fn; },
      querySelectorAll: () => [first, hidden, last] },
    settingsToggle: { focus() {} },
    document: { activeElement: last },
    closeSettings() {},
  });
  vm.runInContext(between("  settingsModal.addEventListener('keydown'", "  settingsThemeList.addEventListener('keydown'"), context);
  handler({ key: 'Tab', shiftKey: false, preventDefault() {} });
  assert.equal(focused, first);
  assert.match(page, /querySelectorAll\('button:not\(:disabled\)'\)/);
  assert.equal(disabled.disabled, true);
});

test('pane controls exist before saved Edit mode restores and tool clicks bypass capture editing', () => {
  const restored = page.indexOf("  setEditMode(window.discussReadPref('editMode', false) === true");
  for (const [name, id] of [
    ['modeMenu', 'mode-menu'], ['modeToggle', 'mode-menu-toggle'], ['modeList', 'mode-menu-list'],
    ['documentMenu', 'document-menu'], ['documentToggle', 'document-menu-toggle'], ['documentList', 'document-menu-list'],
  ]) {
    const declaration = page.indexOf(`const ${name} = document.getElementById('${id}')`);
    assert.ok(declaration >= 0 && declaration < restored, `${id} initialized before restore`);
  }
  const source = between('  const documentControlSelector', '  async function openBlockEditor');
  let handler;
  vm.runInNewContext(source, {
    editMode: true, reviewComplete: false, activeFileIsEditable: () => true,
    document: { addEventListener: (_, callback) => { handler = callback; } },
    openBlockEditor: () => assert.fail('toolbar click opened block editor'),
  });
  handler({ target: { closest: selector => selector === '#pane-left' || selector.includes('#pane-tools') ? {} : null },
    preventDefault: () => assert.fail('toolbar click blocked'),
    stopImmediatePropagation: () => assert.fail('toolbar click stopped'),
  });
});

test('mode trigger names its SVG-only current mode and hover menu survives popup transit', () => {
  function element() {
    const listeners = {};
    return {
      hidden: false, listeners, children: [], attrs: {},
      addEventListener(name, handler) { listeners[name] = handler; },
      setAttribute(name, value) { this.attrs[name] = value; },
      replaceChildren() { this.children = []; },
      appendChild(child) { this.children.push(child); },
      querySelector(selector) { return selector === 'button' ? this.children.find(child => child.type === 'button') : null; },
      querySelectorAll() { return this.children.filter(child => child.type === 'button'); },
      focus() { document.activeElement = this; },
      contains(node) { return node === this || this.children.includes(node) || node === modeToggle || node === modeList || modeList.children.includes(node); },
      matches() { return pointerInside; },
    };
  }
  let pointerInside = false;
  const modeMenu = element();
  const modeToggle = element();
  const modeList = element();
  modeList.hidden = true;
  const documentMenu = element();
  const documentToggle = element();
  const documentList = element();
  documentList.hidden = true;
  let kind = 'markdown';
  let editMode = false;
  let inspectOn = false;
  const document = { activeElement: null, createElement: element };
  const context = vm.createContext({ modeMenu, modeToggle, modeList, documentMenu, documentToggle, documentList,
    document, fileKind: () => kind, reviewComplete: false, setEditMode: on => { editMode = on; },
    setInspectMode: on => { inspectOn = on; }, setRewriteOpen() {},
  });
  Object.defineProperties(context, { editMode: { get: () => editMode }, inspectOn: { get: () => inspectOn } });
  vm.runInContext(between('  let modePinned = false;', "  document.getElementById('fold-sections-toggle')"), context);
  vm.runInContext('syncPaneMenus()', context);
  assert.equal(modeToggle.attrs['aria-label'], 'Mode: Comment');
  assert.equal(modeToggle.title, 'Mode: Comment');
  assert.match(modeToggle.innerHTML, /<svg[^>]*aria-hidden="true"/);
  assert.doesNotMatch(modeToggle.innerHTML, /Comment|▾|♧/);
  pointerInside = true;
  modeMenu.listeners.pointerenter();
  assert.equal(modeList.hidden, false);
  assert.equal(modeToggle.attrs['aria-expanded'], 'true');
  assert.equal(document.activeElement, null, 'hover does not take keyboard focus');
  assert.match(modeList.children[0].innerHTML, /<svg[^>]*>[\s\S]*<span>Comment<\/span>/);
  assert.equal(modeList.children[0].attrs['aria-checked'], 'true');
  modeMenu.listeners.pointerleave();
  pointerInside = false;
  assert.equal(modeList.hidden, true);
  modeMenu.listeners.pointerenter();
  modeToggle.listeners.click();
  modeMenu.listeners.pointerleave();
  assert.equal(modeList.hidden, false, 'click pins the menu open');
  const escape = { key: 'Escape', preventDefault() { this.prevented = true; } };
  modeList.listeners.keydown(escape);
  assert.equal(escape.prevented, true);
  assert.equal(modeList.hidden, true);
  assert.equal(document.activeElement, modeToggle);
  modeMenu.listeners.pointerenter();
  assert.equal(modeList.hidden, true, 'Escape does not immediately reopen on focus/pointer');
  modeMenu.listeners.pointerleave();
  modeMenu.listeners.pointerenter();
  assert.equal(modeList.hidden, false, 'a fresh pointer entry can reopen');
  modeList.children[0].focus();
  modeMenu.listeners.pointerleave();
  assert.equal(modeList.hidden, false, 'keyboard focus within keeps the menu open');
  modeMenu.listeners.focusout({ relatedTarget: null });
  assert.equal(modeList.hidden, true);
  modeMenu.listeners.pointerenter();
  modeList.children[1].listeners.click();
  assert.equal(editMode, true, 'menu choice selects Edit');
  assert.equal(modeList.hidden, true);
  assert.equal(document.activeElement, modeToggle);
  vm.runInContext('syncPaneMenus()', context);
  assert.equal(modeToggle.attrs['aria-label'], 'Mode: Edit');
  assert.equal(modeList.children[1].attrs['aria-checked'], 'true');
  assert.match(modeToggle.innerHTML, /<svg/);
  kind = 'html'; inspectOn = false;
  vm.runInContext('syncPaneMenus()', context);
  assert.equal(modeToggle.attrs['aria-label'], 'Mode: Interact');
  assert.deepEqual(modeList.children.map(item => item.attrs.role).filter(Boolean), ['menuitemradio', 'menuitemradio']);
});

test('Escape from a hover-open mode menu restores focus to Mode', () => {
  const modeToggle = { focus() { document.activeElement = this; } };
  const documentToggle = { focus() { document.activeElement = this; } };
  const modeList = { hidden: false };
  const documentList = { hidden: true };
  const document = { activeElement: null, addEventListener(_, handler) { this.onKeydown = handler; } };
  let closed = false;
  vm.runInNewContext(between("  document.addEventListener('keydown', event => {\n    if (event.key === 'Escape' && (!modeList.hidden", "  const rewriteToggle = document.getElementById('rewrite-toggle')"), {
    document, modeToggle, documentToggle, modeList, documentList,
    modeMenu: { contains: () => false },
    closePaneMenus() { closed = true; modeList.hidden = true; documentList.hidden = true; },
  });
  const event = { key: 'Escape', preventDefault() { this.prevented = true; } };
  document.onKeydown(event);
  assert.equal(event.prevented, true);
  assert.equal(closed, true);
  assert.equal(document.activeElement, modeToggle);
});

test('thread expansion stays in the summary dropdown when its list refreshes', () => {
  const header = between('<header>', '</header>');
  assert.match(header, /id="thread-summary-popover"[\s\S]*id="show-all"[\s\S]*id="thread-summary-entries"/);
  assert.equal((page.match(/id="show-all"/g) || []).length, 1);
  assert.ok(!page.includes('thread-panel-head'));
  assert.ok(!header.includes('class="header-sep"'));
  assert.ok(page.includes("document.getElementById('thread-summary-entries').innerHTML = threadSummaryEntries(state)"));
  assert.ok(!page.includes('summaryPopover.innerHTML ='));
  assert.ok(page.includes('if (!popover.hidden) showAllBtn.focus();'));
});

test('summary expand action toggles open threads, label, and pressed state', () => {
  let handler;
  let on = false;
  let opened = 0;
  let collapsed = 0;
  const button = {
    classList: { toggle() { on = !on; return on; } },
    addEventListener(_event, fn) { handler = fn; },
    setAttribute(name, value) { this[name] = value; },
  };
  vm.runInNewContext(between('  // Show-all toggle', '  // File list width'), {
    showAllBtn: button,
    document: { querySelectorAll() { return [{ classList: { add() { opened++; } } }]; } },
    syncThreadActiveAnchors() {}, updateEmptyState() {}, reposition() {},
    closeAllThreads() { collapsed++; },
  });
  handler();
  assert.equal(opened, 1);
  assert.equal(button.textContent, 'Collapse all threads');
  assert.equal(button['aria-pressed'], 'true');
  handler();
  assert.equal(collapsed, 1);
  assert.equal(button.textContent, 'Expand all threads');
  assert.equal(button['aria-pressed'], 'false');
});

test('controls are localized, keyboard operable and do not discard the existing edit workflow', () => {
  assert.match(page, /id="settings-modal"[^>]*hidden/);
  assert.match(page, /const documentControlSelector = '#pane-tools,/);
  assert.match(page, /id="mode-menu-list" role="menu" hidden/);
  assert.match(page, /id="fold-sections-toggle" role="menuitemcheckbox"/);
  assert.match(page, /id="show-all"[^>]*>Expand all threads/);
  assert.match(page, /id="demo-strip"[^>]*hidden/);
  assert.match(page, /image\.addEventListener\('pointercancel', clearSelection\)/);
  assert.match(page, /image\.addEventListener\('keydown', event/);
  assert.match(page, /button\.addEventListener\('click', event => \{[\s\S]*?openBlockEditor\(Number\(block\.dataset\.anchorIdx\), block\)/);
  assert.match(page, /editModeToggle\.addEventListener\('click', \(\) => setEditMode\(!editMode\)\)/);
});
