# discuss

**Stop copy-pasting markdown into Claude. Argue back in the margins.**

`discuss` is a CLI that opens any markdown file in a browser with inline comment threads on every paragraph. Your AI agent (Claude Code, Codex) joins the session, reads what you highlight, and posts its own "takes" right in the margins. Review docs like you review PRs — anchored, threaded, bidirectional. No cloud, no copy-paste.

## Why?

Markdown is how engineers share everything that isn't code — PRDs, design docs, RFCs, incident post-mortems, analysis notes. But review tools assume the thing being reviewed is a diff. Docs either get copy-pasted into a chat window, marked up in Google Docs comments no agent can read, or ignored.

`discuss` makes the doc itself the workspace:

- **Inline anchored threads** — click any paragraph, drop a comment, get a threaded response.
- **Takes vs replies** — the agent posts *takes* (its view), humans post *replies*. Rendered distinctly so you can tell who said what at a glance.
- **Bidirectional** — the browser writes through a local REST API; the agent reads stdout events and writes back through the same API.
- **No cloud.** One Rust binary, one localhost server, one browser tab.

## Install

### Full install (CLI + skill) from a clone

```sh
git clone https://github.com/codesoda/discuss-cli.git
cd discuss-cli
./install.sh
```

Builds from source with `cargo build --release`, installs the binary to `~/.discuss/bin/`, symlinks `~/.local/bin/discuss`, and symlinks the `/discuss` skill into every agent root present (`~/.claude/skills/`, `~/.codex/skills/`, `~/.agents/skills/`).

### Skill-only (lazy binary bootstrap)

```sh
npx skills add codesoda/discuss-cli
```

Uses the [vercel-labs/skills](https://github.com/vercel-labs/skills) CLI to drop the `/discuss` skill into your agent roots. The binary isn't installed yet — the skill bootstraps it on first use (see Quick Start).

## Quick Start

### With an agent (the main use case)

In Claude Code, Codex, or any agent with the `/discuss` skill, just ask:

> Can you discuss ./plan.md with me?

The agent invokes the skill. If `discuss` isn't on your PATH yet, it'll prompt before running the installer:

> `discuss` isn't on your PATH. Install it now? (runs `curl -sSL https://raw.githubusercontent.com/codesoda/discuss-cli/main/install.sh | sh`)

Confirm — the installer self-bootstraps in the background, the server launches on `http://127.0.0.1:7777`, your browser opens with the rendered doc, and the agent starts streaming events. Drop an inline thread anywhere and the agent replies with a take.

### Without an agent

```sh
discuss ./plan.md
```

Browser opens on `http://127.0.0.1:7777`. You get the full review UI — inline threads, replies, resolution — without any agent participation. Useful for solo review.

## CLI

| Command | Description |
|---------|-------------|
| `discuss <file>` | Open a markdown file in a browser-based review session |
| `discuss update --check` | Check GitHub for a newer release |
| `discuss update -y` | Download the latest release, verify checksum, self-replace |

### Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--port <N>` | `7777` | Bind port. No free-port fallback — fails fast if already bound. |
| `--no-open` | off | Don't auto-launch the browser |
| `--history-dir <path>` | `~/.discuss/history` | Where transcripts get written |
| `--no-save` | off | Don't persist transcripts |

## HTTP API

While the server is running:

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/api/state` | Current snapshot: threads, replies, takes, drafts |
| `GET` | `/api/events` | SSE event stream (browser UI) |
| `POST` | `/api/threads` | Create a thread |
| `POST` | `/api/threads/{id}/replies` | Add a **human** reply |
| `POST` | `/api/threads/{id}/takes` | Add an **agent** take |
| `POST` | `/api/threads/{id}/resolve` | Resolve a thread |
| `POST` | `/api/threads/{id}/unresolve` | Unresolve |
| `DELETE` | `/api/threads/{id}` | Soft-delete (`kind = "user"` only) |

## Stdout events

One newline-delimited JSON object per line. Consumed by the `/discuss` skill via Monitor; any line-reader works.

| Kind | When |
|------|------|
| `session.started` | Server bound and listening |
| `thread.created` | User opened a new thread |
| `reply.added` | Human posted a reply |
| `thread.resolved` / `thread.unresolved` | Resolution toggled |
| `thread.deleted` | Soft-delete |
| `prompt.suggest_done` | Idle timeout fired |
| `session.done` | Server exited cleanly |

Draft keystrokes and agent takes broadcast via SSE only — they never surface on stdout.

## Agent integration

The skill lives at [`skills/discuss/SKILL.md`](skills/discuss/SKILL.md) and targets:

- **Claude Code** — `~/.claude/skills/discuss`
- **Codex** — `~/.codex/skills/discuss`
- **Cline / Warp / anything respecting `~/.agents/skills/`**

What the skill handles:

- Launching `discuss <file>` as a background task
- Streaming stdout events via the agent's Monitor primitive
- Posting takes in response to user-opened threads
- Self-bootstrapping the binary if it isn't installed

## License

MIT
