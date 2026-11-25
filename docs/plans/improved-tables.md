# Table presentation polish

PR #25 originally introduced browser-assigned row anchors. That implementation
is superseded by #44, #47 and #48 on main: the server now stamps whole-table
and row anchors, with matching blocks API metadata and browser smoke coverage.

## Retained scope

- Compact table typography, padding, separators and header backgrounds.
- Left-aligned headers unless Markdown explicitly specifies alignment.
- An inner `.table-scroll` inside the existing server-generated `.table-wrap`.
  The wrapper retains its whole-table anchor; row anchors remain unchanged.
- A right-edge fade only while more content can be scrolled into view.
- Existing row marker stacks stay outside the scroll clip. Resize observation
  of both the scroll container and table keeps their offsets current, including
  when row heights change without changing the container width.
- Disconnect observers before reattaching to the active document on rendering.
- Explicit additional scope: readable document links in both themes, including
  visited links, using `--link`.

## Boundaries

No new anchor generation, snippets, protocol fields, or Rust rendering changes.
Preserve whole-table and row comment behavior, source mappings, and blocks API
indices. Nested tables retain the existing server anchoring policy.

## Verification

Run Rust formatting, clippy, and tests. Extend the existing table-row browser
smoke test to check presentation wrapping does not renumber anchors and that
scroll overflow, fades and marker placement remain correct. Browser smoke
requires a running review of its fixture and a Chrome CDP endpoint.
