#!/usr/bin/env bash
# poller.sh — blocking discuss event poller (fallback for when no Monitor-type
# background monitoring tool is available)
#
# Usage: poller.sh <url> [baseline_json]
#
# Polls the discuss /api/state endpoint every 5 seconds. Blocks until a new
# thread appears or an existing thread gains a new reply or take. When
# something changes, prints one JSON object per line for EVERY change detected
# in that poll, followed by a final snapshot line:
#
#   {"event": "thread.created", "thread": {...}}
#   {"event": "thread.updated", "thread": {...}, "prev_count": 1, "current_count": 2}
#   {"event": "snapshot", "baseline": {"<thread-id>": <count>, ...}}
#
# Pass the "baseline" value from the snapshot line as $2 on the next
# invocation so no events are dropped between invocations. If you post a reply
# or take yourself, bump that thread's count in the baseline first:
#   BASELINE=$(echo "$BASELINE" | jq -c --arg id "$THREAD_ID" '.[$id] += 1')
#
# Exit codes:
#   0  — change detected; event lines + snapshot line printed to stdout
#   1  — error (API unreachable at startup, or invalid args)
#   2  — session ended (discuss exited); prints {"event": "session.done"}

set -euo pipefail

URL="${1:-}"
BASELINE_JSON="${2:-}"
TMPFILE=$(mktemp /tmp/discuss-state.XXXXXX)
trap 'rm -f "$TMPFILE"' EXIT

if [ -z "$URL" ]; then
  echo "Usage: poller.sh <discuss-url> [baseline_json]" >&2
  exit 1
fi

# Fetch /api/state into TMPFILE. Returns 0 if we got a 200 with a valid JSON
# body containing .threads, non-zero otherwise. Never exits the script.
fetch_state() {
  local http_code
  http_code=$(curl -s -o "$TMPFILE" -w "%{http_code}" "$URL/api/state" 2>/dev/null) || return 1
  [ "$http_code" = "200" ] || return 1
  jq -e '.threads' "$TMPFILE" > /dev/null 2>&1 || return 1
  return 0
}

# Build snapshot from TMPFILE: JSON object mapping thread_id -> (reply_count +
# take_count). replies and takes are top-level objects keyed by thread ID, not
# nested in threads.
snapshot() {
  jq -c '
    . as $s |
    [.threads[].id] |
    map({
      key: .,
      value: (
        (($s.replies[.] // []) | length) +
        (($s.takes[.] // []) | length)
      )
    }) | from_entries
  ' "$TMPFILE"
}

# Establish baseline if not provided
if [ -z "$BASELINE_JSON" ]; then
  ATTEMPTS=0
  until fetch_state; do
    ATTEMPTS=$((ATTEMPTS + 1))
    if [ "$ATTEMPTS" -ge 3 ]; then
      echo "discuss API unreachable at $URL" >&2
      exit 1
    fi
    sleep 2
  done
  BASELINE_JSON=$(snapshot)
fi

FAIL_COUNT=0

while true; do
  sleep 5

  if ! fetch_state; then
    FAIL_COUNT=$((FAIL_COUNT + 1))
    if [ "$FAIL_COUNT" -ge 3 ]; then
      echo '{"event": "session.done"}'
      exit 2
    fi
    continue
  fi
  FAIL_COUNT=0

  # Emit every new thread and every thread whose reply+take count grew.
  EVENTS=$(jq --argjson base "$BASELINE_JSON" -c '
    . as $s |
    .threads[] |
    . as $t |
    (
      (($s.replies[$t.id] // []) | length) +
      (($s.takes[$t.id] // []) | length)
    ) as $current |
    if ($base | has($t.id) | not) then
      {event: "thread.created", thread: $t}
    elif $current > ($base[$t.id] // 0) then
      {event: "thread.updated", thread: $t, prev_count: ($base[$t.id] // 0), current_count: $current}
    else
      empty
    end
  ' "$TMPFILE" 2>/dev/null) || EVENTS=""

  if [ -n "$EVENTS" ]; then
    echo "$EVENTS"
    printf '{"event": "snapshot", "baseline": %s}\n' "$(snapshot)"
    exit 0
  fi
done
