# UI settings survive across sessions

## The bug

Every discuss run binds a fresh ephemeral port (`SocketAddr::from((Ipv4Addr::LOCALHOST, 0))`). The browser treats `http://127.0.0.1:54321` and `http://127.0.0.1:54322` as two different origins, and `localStorage` is scoped per origin. So every UI preference stored in `localStorage` starts empty on every run.

Three settings are affected, and one mechanism fixes all three:

| Setting | Pref key |
| --- | --- |
| ⌘ Enter to send | `cmdEnterToSend` |
| Theme (light/dark/system) | `theme` |
| File sidebar collapsed | `filesCollapsed` |

## What changed

The CLI owns UI preferences in `~/.discuss/prefs.json`, the same home directory as `discuss.config.toml` and `history/`. The server is the only store; the page keeps nothing in `localStorage`.

### Server

**`src/prefs.rs`**

- `UiPrefs { theme: Option<Theme>, cmd_enter_to_send: Option<bool>, files_collapsed: Option<bool> }` with `Theme::{Light, Dark, System}` serialized lowercase. `deny_unknown_fields`, so a bad key or value fails at deserialization.
- `DISCUSS_DIR_NAME` constant; `default_prefs_path()` resolves the home directory on each call. `AppState::with_prefs_path` overrides it, so tests stay isolated.
- `load(path)` — a missing, unreadable, or corrupt file returns defaults. A bad prefs file must never stop a review from opening.
- `merge_and_save(path, patch)` — a process-wide mutex guards the whole load → merge → save, and the write goes to a temp file that is renamed into place, so readers never see a partial file.

**`POST /api/prefs`** (`src/server/prefs.rs`)

- Body is a partial patch: `{"cmdEnterToSend": false}`. Unknown keys, bad values, and empty patches return 400.
- Responds with the merged prefs.
- Offline demo sessions merge in memory and write nothing.

**Injection** (`src/template.rs`, `src/server/pages.rs`)

- One `render_page(.., prefs_json: Option<&str>)`. `Some` injects `window.__DISCUSS_PREFS__` in `<script id="discuss-prefs">` before `#discuss-theme-bootstrap`, so the saved theme is on the page before the first paint. `None` (demo sessions) injects nothing.

### Client (`discuss.html`)

- `discussReadPref(key, fallback)` — the injected value, or the fallback.
- `discussWritePref(key, value)` — updates the in-page copy at once and POSTs the change, fire-and-forget. A failed POST loses only the cross-session memory, not the click.

Defaults do not change: ⌘ Enter required, theme `system`, sidebar expanded.

## Design decisions

1. **A separate JSON file, not `discuss.config.toml`** — config is hand-written by the user; prefs are written by the app.
2. **Server is the only store** — no `localStorage` fallback or migration. These are three resettable settings; a one-time reset is cheaper than a second code path.
3. **Demo sessions never write** — a demo must not reach into the user's home directory, and it renders with defaults so recordings do not pick up the developer's settings.
