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
#   - new event lines appear after <handled-line-count>, other than
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

new_events() {
  tail -n +"$((HANDLED + 1))" "$EVENTS_FILE" 2>/dev/null \
    | grep -v '"kind":"prompt.suggest_done"'
}

FAIL_COUNT=0
TICKS=0

while true; do
  if [ -n "$(new_events)" ]; then
    # Let a burst of events land together.
    sleep 1
    echo "LINES=$(wc -l < "$EVENTS_FILE" | tr -d ' ')"
    new_events
    exit 0
  fi

  # Check that discuss is still up about every 10 seconds.
  TICKS=$((TICKS + 1))
  if [ $((TICKS % 5)) -eq 0 ]; then
    if curl -s -o /dev/null -f "$STATE_URL"; then
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
