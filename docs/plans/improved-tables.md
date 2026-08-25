# Better tables in discuss

Branch: `feat/improved-table-ui`
Worktree: `discuss-cli-worktrees/feat-improved-table-ui`

## What's wrong today

All three problems come from one root cause: **`discuss.html` has no table CSS and no table anchoring at all.** Tables fall through to browser defaults and to nothing, respectively.

**1. Tables clash and reflow.**
There is not a single `table`, `th`, or `td` rule in the stylesheet. So a table gets browser-default layout: no borders, no padding, font size inherited from body, and columns auto-sized to whatever content happens to be in them. One long cell blows a column out and squeezes the rest. A wide table just pushes past the left pane instead of scrolling inside it.

**2. You can't comment on a table.**
Comments bind to elements that carry a `data-anchor-idx` attribute. That list lives in one line — `COMMENTABLE_SELECTOR` at [discuss.html:2686](../../discuss.html:2686) — and it covers headings, paragraphs, list items, blockquotes, code blocks, and the stylized boxes. No table element is in it. So a table is invisible to both ways of starting a comment: clicking a block, and selecting text then hitting the popup. Selecting inside a table finds no anchor above it, so the popup never appears.

**3. Column titles are centred.**
`<th>` centres its text by default in every browser. Nothing in discuss overrides it.

## The fix

### Part 1 — Compact, self-contained tables

Add table CSS scoped to `#doc-content`, matching the density of the surrounding text:

- `border-collapse: collapse`, ~13px font, tight cell padding (roughly `4px 10px`)
- Hairline row separators using the existing `--line` variable, header row separated with `--line-strong`
- Header background from `--pre-bg` so it reads as a header without shouting
- Light/dark both handled for free, since every colour comes from an existing CSS variable

**Cap runaway columns.** Looking at the screenshot, the real clash isn't only missing CSS — one prose column ("The feeling") eats the available width and squeezes everything else, so the last column ends up jammed against it. Browsers size columns by content with no ceiling. Give cells a max-width plus `overflow-wrap: break-word` so a long prose column wraps instead of dominating. For a table that only slightly overflows, this is more of the actual fix than the scroll box is.

Then stop wide tables from pushing the layout around. Wrap each table client-side, the same way `<pre>` already gets wrapped:

```
.table-wrap      → position: relative, no overflow  (markers live here, unclipped)
  .table-scroll  → overflow-x: auto                 (the table scrolls sideways in here)
    <table>
```

The two-layer wrapper exists for a specific reason. Gutter markers are absolutely positioned and hang off the right edge of their anchor. `overflow-x: auto` would clip them — that's exactly why `.pre-wrap` exists for code blocks today ([discuss.html:2684](../../discuss.html:2684)). Keeping the scroll on an inner element means the table scrolls but the comment dots don't get cut off. Horizontal scroll doesn't move markers, since they only need a vertical position.

Result: a wide table scrolls inside its own box instead of shoving the page.

**Show that there's more to the right.** Once a table sits in a scroll box, nothing tells you it's scrollable — you only find out by trying. Add a fade on the right edge of `.table-scroll`, shown only while the table actually overflows and hidden once you've scrolled to the end.

### Part 2 — Comment on a row

Add `tr` to `COMMENTABLE_SELECTOR`. Every row — including the header row — becomes a comment anchor, exactly like a paragraph or a list item.

Three things then work with no further code:

- Click a row → comment editor opens on it
- Drag-select across several rows → popup appears → one comment covering that range (the multi-element range mechanism already handles this)
- The dot in the right gutter lines up with the row, and the thread card on the right lines up with it too

Marker placement needs one adjustment: for a row anchor, the marker gets appended to the row's `.table-wrap` and offset vertically to the row, instead of being appended to the anchor itself. This reuses the existing `aligned-to-line` path that already positions markers against individual code lines ([discuss.html:3636](../../discuss.html:3636)).

Two small styling details:

- `position: relative` on `<tr>` is fine in every current browser, but the row's highlight (hover, focused, has-thread, thread-active) reads better painted on the cells than on the row, so the highlight rules get a `tr > td, tr > th` variant.
- `outline-offset: 2px` on a row would draw outside the table edge. Rows use a background tint plus a left edge marker instead of an outline.

### Part 3 — Left-align the column titles

```css
#doc-content th:not([align]) { text-align: left; }
```

The `:not([align])` matters. When markdown says `|---:|`, comrak writes `align="right"` onto the cell (confirmed in comrak 0.52 `html.rs:1161`). A blanket `th { text-align: left }` would silently throw that away. This rule only changes headers where the author didn't ask for anything.

## The one decision: row or cell?

You asked "a row (or cell?)". They're mutually exclusive under the current anchoring model, so this needs a call.

Anchors are assigned to the *outermost* commentable element — a deliberate rule at [discuss.html:2702](../../discuss.html:2702) so nested elements don't fight over clicks. A row contains its cells. So if both rows and cells were commentable, rows would win and cells would get nothing.

**Recommendation: rows for v1.** It's the granularity people actually want ("this line is wrong"), it keeps the anchor count sane on a big table, and it needs zero changes to the Rust side — no new state fields, no protocol change, no server work. Everything above is CSS and JavaScript in `discuss.html`.

**Cells are a clean v2, not a dead end.** There's already a pattern for narrowing inside an anchor: `lineRange` picks specific lines inside a code block. A `cellRange` inside a row anchor would be the same shape — select across two cells, the comment binds to those columns. But it's a real change: new field in `src/state/types.rs`, server validation, transcript output, and the browser side. Worth doing on its own if row-level turns out to be too blunt.

Say the word if you want cells in v1 instead and I'll re-plan around the protocol change.

## Steps

1. Table CSS, column max-width, `th:not([align])` left-align → verify: a test doc with a wide table, a narrow table, and one using `:---:` alignment renders compact, no column dominates, headers left unless the markdown says otherwise
2. `.table-wrap` / `.table-scroll` wrapping in `assignAnchorIndices`, plus the right-edge fade → verify: table scrolls inside its box, page doesn't scroll sideways, fade shows only while there's more to the right
3. `tr` into `COMMENTABLE_SELECTOR` + row marker placement → verify: click a row, comment saves, dot sits next to the right row, thread card lines up; reload and the comment is still on the right row
4. Multi-row selection → verify: drag across three rows, popup fires, one comment covers the range
5. Dark mode + `cargo fmt`, `clippy`, `test` → verify: nothing Rust-side broke (no Rust files change, so this is a regression check only)

Verification is by eye in a running `discuss` session — there's no browser test harness in this repo. I'll build a scratch markdown file with the awkward cases (the domain-name table from your screenshot is a good one) and walk it.

## Not doing

**No zebra striping.** Tempting on a wide table for tracking your eye across a row, but it fights the comment states directly. `has-thread`, hover, `focused`, and `thread-active` are all background tints. Stripe the rows and every one of those signals gets muddier. In a review tool the comment state has to win.

**No sticky header row.** Sounds right for a long table, but the left pane scrolls as one document — a sticky `th` would pin to the top of the pane and float over unrelated content once the table scrolls past. Making it behave needs a vertical scroll container per table, which reintroduces the marker-clipping problem the two-layer wrapper exists to avoid. Worth revisiting only if a genuinely long table makes it hurt.

- No column sorting or resizing — not asked for
- No change to markdown rendering (`src/render.rs` stays as is)
- No cell-level anchoring in v1, see decision above
