import assert from 'node:assert/strict';
import fs from 'node:fs';
const html = fs.readFileSync(new URL('../../../discuss.html', import.meta.url), 'utf8');
function between(start, end) {
  const first = html.indexOf(start);
  const last = html.indexOf(end, first + start.length);
  assert.ok(first >= 0 && last > first, `Missing source markers: ${start}, ${end}`);
  return html.slice(first, last);
}
export const quoteFunction = between('  function appendQuotedSelection(', '  const selectionActions =');
export const selectionSource = between('  function appendQuotedSelection(', '  function renderFollowups(');
export const clipboardSource = between('  function copyTextToClipboard(', '  // ---------- Heartbeat');
export const clickSource = between('  const documentControlSelector =', '\n\n  // Capture-phase') + '\n' +
  between("  document.addEventListener('click', (e) => {\n    if (e.target.closest('#finish-review'))", '  // Escape closes');
export const completedSource = between('  function blockCompletedInteraction(', '  function updateFinishReviewButtonLabel(');
