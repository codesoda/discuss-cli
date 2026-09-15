# UI settings survive across sessions

## The bug

Every discuss run binds a fresh ephemeral port (`SocketAddr::from((Ipv4Addr::LOCALHOST, 0))`, `src/server/mod.rs:133`). The browser treats `http://127.0.0.1:54321` and `http://127.0.0.1:54322` as two different origins, and `localStorage` is scoped per origin. So every UI preference we store in `localStorage` is born fresh on every run and thrown away when the session ends.

Three settings are affected today:

| Setting | Key | Where |
| --- | --- | --- |
| ⌘ Enter to send | `discuss-cmd-enter-to-send` | `discuss.html:4593` |
| Theme (light/dark/system) | `discuss-theme` | `discuss.html:64`, `discuss.html:7627` |
| File sidebar collapsed | `discuss-files-collapsed` | `discuss.html:3723` |

The one Keith hit is ⌘ Enter, but all three have the same amnesia, and they all get fixed by the same mechanism. Fixing only one would leave the other two broken for no saved effort.

## What changed

Move the source of truth for UI preferences out of the browser and into a file the CLI owns: `~/.discuss/prefs.json`. Same home directory `discuss` already uses for `~/.discuss/discuss.config.toml` and `~/.discuss/history/`.

The server reads the file when it renders the page and injects the values; the page writes changes back over a small API. `localStorage` stays as a same-session mirror so toggles feel instant and a failed write never loses the click.

### Server

**New `src/prefs.rs`**

- `UiPrefs { theme: Option<String>, cmd_enter_to_send: Option<bool>, files_collapsed: Option<bool> }` — every field optional so "never set" is distinct from "set to the default", and unknown keys in the file are ignored rather than rejected (an older binary shouldn't choke on a newer file).
- `path()` → `~/.discuss/prefs.json`, falling back to `.discuss/prefs.json` when there's no home dir, overridable with `DISCUSS_PREFS_PATH` so tests never touch the real one.
- `load()` — a missing, unreadable, or corrupt file returns defaults. A bad prefs file must never stop a review from opening.
- `merge_and_save(patch)` — read, apply only the fields present in the patch, write to a temp file and rename into place. Read-modify-write means two concurrent discuss sessions can't clobber each other's unrelated settings.
- Validation lives here: `theme` must be one of `light` / `dark` / `system`, the rest are plain bools. Anything else is rejected with a 400 rather than written.

**`POST /api/prefs`** (`src/server/mod.rs`, new handler in a `src/server/prefs.rs` alongside the other route modules)

- Body is a partial patch: `{"cmd_enter_to_send": false}`.
- Responds with the merged prefs so the page can't drift from disk.
- Demo sessions (`is_offline_demo`) accept the call and return the merged value but skip the disk write — the demo must not reach into the user's home directory.

**Injecting the values** (`src/template.rs`, `src/server/pages.rs:66`)

- `render_page` takes a fourth argument, `prefs_json`.
- It is injected as `window.__DISCUSS_PREFS__ = {...}` in a `<script id="discuss-prefs">` placed **before** the existing `#discuss-theme-bootstrap` script in `<head>`. That ordering matters: the theme bootstrap runs pre-paint to stop the flash of the wrong theme, so the saved theme has to be on the page before it. The existing initial-state injection point (before the main script) is too late for theme.
- Same `js_safe_json` escaping as the other two injections.

### Client (`discuss.html`)

A small prefs layer, defined early enough that the theme bootstrap can use it:

- `readPref(key, fallback)` — `window.__DISCUSS_PREFS__[key]` when present, otherwise `localStorage`, otherwise the fallback. The `localStorage` step is what carries an existing user's current choice over on first run with the new build, instead of resetting them to defaults.
- `writePref(key, value)` — write `localStorage` (instant, survives a reload of this same session) **and** `POST /api/prefs` fire-and-forget, wrapped in catch. If the POST fails the session still behaves correctly; only the cross-session memory is lost.

Then swap the three consumers onto it:

- `isCmdEnterRequired()` / `setCmdEnterRequired()` (`discuss.html:4594`)
- `isFilesCollapsed()` / `setFilesCollapsedPref()` (`discuss.html:3720`)
- theme bootstrap (`discuss.html:62`) and the theme menu click handler (`discuss.html:7627`)

No behaviour changes beyond persistence — defaults stay exactly as they are (⌘ Enter required by default, theme `system`, sidebar expanded).

## Tests

- `src/prefs.rs` unit tests: missing file → defaults; corrupt JSON → defaults, no error; unknown keys preserved through a merge; patch touches only the fields it names; invalid `theme` rejected.
- `src/template.rs`: prefs land before the theme bootstrap; `</script>` in a value can't break out.
- `tests/server.rs`: `POST /api/prefs` round-trips, rejects a bad theme with 400, and writes nothing under demo mode (`DISCUSS_PREFS_PATH` pointed at a temp dir).
- `tests/theme.rs` already covers the theme bootstrap — extend it for the injected-value path.
- Manual: open a doc, untick ⌘ Enter, close, reopen on a new port, box is still unticked.

## Design decisions

1. **A separate JSON file, not `discuss.config.toml`** — config is hand-written by the user; prefs are written by the app. Machine writes into a user's TOML would lose their comments and formatting.
2. **A JSON object rather than a typed struct on disk** — a file written by a newer build keeps its extra keys when an older build merges a patch into it.
3. **Read-modify-write on every save** — two sessions open at once can each change a different setting without clobbering the other.
4. **Browser storage kept as a mirror** — the toggle stays instant and a failed POST costs the cross-session memory, not the click. It is also the migration path: an existing choice still in `localStorage` is the fallback on first run of the new build.
5. **Demo sessions never write** — a demo must not reach into the user's home directory, and it renders with defaults so recordings do not pick up whatever the developer has saved.
