# Enter to Send — discuss.html change

## What changed

Added an "Enter to send" checkbox to the discuss review UI so users can submit comments with bare Enter instead of Ctrl/Cmd+Enter.

## Behaviour

- **Checkbox location**: Bottom-right of every comment action bar (followup textareas and new-thread editor), next to the existing buttons.
- **Default state**: ON (checked). First-time visitors get Enter-to-send immediately.
- **Persistence**: Stored in `localStorage` under `discuss-enter-to-send`. Survives page reloads and applies to all future discuss sessions on the same browser.
- **Global sync**: Toggling any checkbox updates all visible checkboxes instantly via `setEnterToSend()`.
- **Shift+Enter**: Always inserts a newline, regardless of setting.
- **Ctrl/Cmd+Enter**: Always sends, regardless of setting (backwards-compatible).
- **Hint text**: The "⌘/Ctrl+Enter" hint auto-hides when checkbox is checked (CSS `:has()` selector), reappears when unchecked.

## Files touched

- `discuss.html` — all changes in this single file:
  - **CSS** (line ~557): `.enter-to-send` label styles, checkbox styles, `:has()` rule to hide hint
  - **JS** (line ~1704): `isEnterToSend()`, `setEnterToSend()`, `enterSendCheckboxHtml()`, `wireEnterSendCheckbox()`, `shouldSendOnKey()`
  - **Templates**: Three action bars updated to include `${enterSendCheckboxHtml()}`
  - **Keydown handlers**: Two handlers updated from inline `(e.metaKey || e.ctrlKey) && e.key === 'Enter'` to `shouldSendOnKey(e)`
  - **Placeholders**: Removed `(⌘/Ctrl+Enter to save)` from textarea placeholders since the checkbox communicates this

## Design decisions

1. **ON by default** — The whole point is reducing friction. Users who want the old behaviour can uncheck once.
2. **localStorage over cookie** — No server-side state needed. discuss is a localhost tool, so localStorage is the simplest persistent storage.
3. **CSS `:has()` for hint toggle** — Avoids JS DOM manipulation to show/hide the hint. The hint is only useful when Enter-to-send is off.
4. **No multiline concern** — Comments in discuss are typically short. Shift+Enter covers the rare multiline case. This matches Slack, Discord, and most chat UIs.
