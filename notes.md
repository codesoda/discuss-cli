We are going to build a discuss-cli rust cli

follow the generall approach followed by bugatti-cli, dispatch-cli

install.sh script (local and via github)

and all the other CLI features described in the notes for the rust-cli talk at "/Users/chrisraethke/Documents/ai-talk-research/talks", check the slides.html, outline.md frist, check in research for reference once you have the concepts identified

currently we have been using a agent session to say use this template and then make a concrete html plan file from it. When claude code is in plan mode it can't write other files, but if it can call a "discuss {filepath.md}" that then renders the html in memory and serves it over a local http endpoint, and the user can then interact with the model via a stdin/stdout from the cli being piped via api requests/results back to the page, we could create a really rich plan (or anything else that coud be written to markdown) discussion experience between user and agent. An example of the current "agent tcopies the template and makes a custom html file that can be used in a manual review process can be found at /tmp/plan-review.html" read the source for this file to get an idea of how the current manual process works.

# Below is a bit of an outline of what an ideal result would look like.

❯ in 3-5 paragraphs write me an outline of how this review process is working, assume that instead of writing a html file each time, you can just call a CLI, e.g. discuss
path/to/markdown/file.md and that renders and serves the html, opening it in the browser. As the user adds comments or replies, that is sent via http api inside the cli,
back to the cli, which echos the users request to stdout, the model which launch discuss cli should be able to see that user request and also respond, that should be
returned to the html page via the cli as a further update to the rendered review page. Finally, instead of a copy all button, it should be a button to say that's all the
feedback/discussion for now, and the CLI then prints ALL comments/replies/resolution to stdout, so te model can then cinlude that back into the main conversation. The
browser page can then be closed and the discuss cli exits with a 0 status code.

⏺ Launch and serve. discuss path/to/file.md starts a local HTTP server on an ephemeral port, renders the markdown through the same two-pane template plan-review.html uses
today, opens the default browser at http://localhost:PORT/, and begins streaming events to stdout. The server is the single source of truth for review state — persisted
in-memory plus a sidecar path/to/file.md.discuss.json so a crash or accidental tab-close doesn't lose work. Optional flags seed the session: --threads prepopulated.json for
agent-generated initial takes, --reset to clear prior state. No zipping, no copying a bespoke HTML to ~/Downloads, no per-review file generation — the tool ships once and
every review is one command.

User-event stream to stdout. Every browser action POSTs back to the server: starting a comment, editing a draft, saving, resolving with or without a decision, replying,
unresolving, deleting. Endpoints look like POST /api/threads, POST /api/threads/:id/replies, POST /api/threads/:id/resolve, POST /api/drafts. On each write, the CLI appends
one JSON line to stdout — something like {"kind":"thread.created","threadId":"u-abc","anchor":[45,48],"snippet":"…","text":"user's comment body"}. The model that launched
discuss has it running via a backgrounded Bash or Monitor, so each new stdout line arrives as a notification: "the user just said something, here it is."

Model replies loop back via HTTP. When the model wants to respond, there's no practical stdin channel into a backgrounded process — so it posts back the same way the
browser does. curl -s http://localhost:PORT/api/threads/{id}/takes -d '{"take":"…"}' attaches a model take to that thread. The CLI records it, pushes the update over a
Server-Sent Events stream (GET /api/events) that the browser subscribed to on load, and the page re-renders the thread in place to show the model's reply below the user's
comment. The user replies, which lands as a new stdout event; the model posts another take; the loop continues. The UI distinguishes model takes from user replies visually
(blue vs pink, same vocabulary as today's pre-populated vs user threads). Drafts stay exactly as we just wired them — but the draft store is server-side now, so the teal
marker appears via SSE on every tab open, surviving reloads without relying on localStorage.

"Done" replaces Copy All, CLI exits 0. The Copy-All clipboard dance goes away. The button becomes "Done — send to chat." Clicking it POSTs /api/done, the CLI writes one
consolidated transcript to stdout — every thread in document order, with anchor/snippet, original prepopulated comment if any, every take, every user reply, resolution
status and decision — returns 200 with a small confirmation payload the browser uses to show a "You can close this tab" banner, then shuts the HTTP server and exits with
status 0. The backgrounded task completing is itself the signal to the model: it reads the final stdout chunk, sees the full machine-parseable transcript, and folds it into
the main conversation. The transcript format is the CLI's decision, not a human-readable copy-paste — so it can be structured JSON the model consumes directly. End result:
one command in, a live bidirectional review, a clean structured dump out, no paste step, no guessing whether the right version of state got copied.
