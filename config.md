# Configuration

Discuss loads defaults, `~/.discuss/discuss.config.toml`, then
`./discuss.config.toml`, environment overrides, and CLI overrides (highest
priority). Omitted keys preserve lower-priority values. Unknown TOML keys fail.

## Voice transcription

For recording instructions, screenshots, and copyable engine setup prompts, see
[Voice notes](voice.md).

Voice notes are opt-in: without a nonempty transcription command, Discuss does
not offer hold-to-talk or the V recording shortcut, and audio uploads are rejected.
Existing voice threads still retain their playable audio, `audioId`, and absolute
`audioPath`. Models with native audio input can use those recordings if their
agent harness supports audio attachments; a path alone is not an attachment.

```toml
transcription_command = ["/home/me/bin/transcribe", "{filepath}"]
```

This is an argument array: the first element is an executable (absolute path or
found on PATH); remaining elements are its arguments. Discuss replaces every
`{filepath}` in arguments with the saved recording's absolute path. Spaces and
shell metacharacters remain literal. There is no shell expansion, quoting,
pipe processing, or automatic `~` expansion. Use `[]` to disable a command
inherited from another config file. There is no transcription CLI flag or
environment override.

The command must return UTF-8 transcript text on stdout and exit successfully.
Send diagnostics to stderr. Discuss waits up to 120 seconds during upload;
on failure, timeout, empty output, or invalid UTF-8 it logs a warning and retains
the audio-only workflow. Successful text becomes the voice thread's message;
the original playable recording and agent audio path remain available during the session.
The Saving indicator stays visible while the command runs; large models and cold
starts can take the full timeout. Choose a faster model for interactive feedback.
Clips live in a private, randomly named session directory removed when server
state is dropped on normal shutdown (not guaranteed after a crash or SIGKILL).
Deleted threads and failed thread submissions retain their clips until then.
Done/history transcripts omit audio fields; consume audio before ending the session.

### Parakeet, Whisper, and other CLIs

Discuss does not know engine-specific options. Point the command at your CLI if
it accepts the recording and prints plain text; otherwise use a wrapper that:

1. Takes the recording path as its first argument.
2. Converts WebM/Opus or other browser audio if the engine requires WAV.
3. Runs your installed Parakeet CLI, mlx-whisper, whisper.cpp, or another engine.
4. Prints only the resulting transcript to stdout (reading its output file if needed).

CLI names, flags, and output formats vary by implementation/version, so adapt
the wrapper to your installed engine. No engine is installed by Discuss.

**Trust boundary:** commands run locally with Discuss's permissions. Review
project `discuss.config.toml` files before starting Discuss in untrusted checkouts.
Do not place secrets in command arguments.

## Other settings

| TOML key | Default | Environment override |
| --- | --- | --- |
| `port` | OS-assigned | `DISCUSS_PORT` |
| `auto_open` | `true` | `DISCUSS_AUTO_OPEN` |
| `idle_timeout_secs` | `600` (`0` disables) | `DISCUSS_IDLE_TIMEOUT_SECS` |
| `history_dir` | standard history directory | `DISCUSS_HISTORY_DIR` |
| `no_save` | `false` | `DISCUSS_NO_SAVE` |
| `log_level` | default logging filter | `DISCUSS_LOG` |
| `max_diff_bytes` | built-in diff limit | `DISCUSS_MAX_DIFF_BYTES` |

Explicit nonzero ports bind exactly; occupied ports fail rather than scanning.
