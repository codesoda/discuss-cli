#!/usr/bin/env bash
# wait-event.sh — wait for the next discuss event that needs the agent
#
# Usage: wait-event.sh <events.jsonl> <handled-line-count> <state-endpoint-url>
#
# Use this when discuss runs as a background job that writes its stdout events
# to <events.jsonl>, and no persistent monitor-type tool exists. Run it as a
# background job too. It checks the file every 2 seconds and costs no tokens
# while it waits. It exits when:
#
#   - complete event lines appear after <handled-line-count>, other than
#     prompt.suggest_done. It prints "LINES=<new total>" and then those lines.
#   - the state endpoint stops answering (discuss exited). It prints
#     {"event": "session.done"}.
#
# Pass the LINES value from the previous run as <handled-line-count> next time.
#
# Exit codes:
#   0  — new events or session ended; see stdout
#   1  — invalid args

set -uo pipefail

EVENTS_FILE="${1:-}"
HANDLED="${2:-}"
STATE_URL="${3:-}"

if [ -z "$EVENTS_FILE" ] || [ -z "$HANDLED" ] || [ -z "$STATE_URL" ]; then
  echo "Usage: wait-event.sh <events.jsonl> <handled-line-count> <state-endpoint-url>" >&2
  exit 1
fi

if ! [[ "$HANDLED" =~ ^[0-9]+$ ]]; then
  echo "handled-line-count must be a nonnegative integer" >&2
  exit 1
fi

# Bound each scan to the file size observed before reading. read succeeds only
# for newline-terminated records; a partly written JSON event stays pending.
new_events() {
  local bytes line
  [ -f "$EVENTS_FILE" ] || return
  bytes=$(wc -c < "$EVENTS_FILE" 2>/dev/null) || return
  bytes=${bytes//[[:space:]]/}
  LINES=$HANDLED
  EVENTS=()
  while IFS= read -r line; do
    LINES=$((LINES + 1))
    [[ "$line" == *'"kind":"prompt.suggest_done"'* ]] || EVENTS+=("$line")
  done < <(head -c "$bytes" "$EVENTS_FILE" 2>/dev/null | tail -n +"$((HANDLED + 1))")
}

FAIL_COUNT=0
TICKS=0
LINES=$HANDLED
EVENTS=()

while true; do
  new_events
  if [ "${#EVENTS[@]}" -gt 0 ]; then
    echo "LINES=$LINES"
    printf '%s\n' "${EVENTS[@]}"
    exit 0
  fi

  # Check that discuss is still up about every 10 seconds.
  TICKS=$((TICKS + 1))
  if [ $((TICKS % 5)) -eq 0 ]; then
    if curl -s -o /dev/null -f --max-time 5 "$STATE_URL"; then
      FAIL_COUNT=0
    else
      FAIL_COUNT=$((FAIL_COUNT + 1))
      if [ "$FAIL_COUNT" -ge 3 ]; then
        echo '{"event": "session.done"}'
        exit 0
      fi
    fi
  fi

  sleep 2
done
