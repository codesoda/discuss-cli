#!/bin/bash
# Scene 1: one command launches all three bundled scenarios. Run from repo root.
set -e
BIN=./target/release/discuss
clear
type_out() {
  local s="$1"
  local i
  for ((i = 0; i < ${#s}; i++)); do printf '%s' "${s:i:1}"; sleep 0.035; done
  printf '\n'
}

printf '\033[1;35m❯ you\033[0m  '
type_out "I want to try Discuss without setup."
sleep 0.6
printf '\n\033[1;36m⏺ demo\033[0m  Everything is bundled — launching the tour, example PR, and local app…\n\n'
sleep 0.5
printf '\033[2m$ discuss demo\033[0m\n'
TMP=$(mktemp)
ERR=$(mktemp)
$BIN --no-open demo >"$TMP" 2>"$ERR" &
PID=$!
for _ in {1..60}; do [ -s "$TMP" ] && break; sleep 0.05; done
python3 - "$TMP" <<'PY'
import json, sys
with open(sys.argv[1]) as handle:
    event = json.loads(handle.readline())
print('session.started  mode=' + event['payload']['mode'])
for scenario in event['payload']['scenarios']:
    print(f"  {scenario['label']:<12} {scenario['url']}")
PY
head -1 "$ERR"
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
rm -f "$TMP" "$ERR"
sleep 0.8
printf '\n\033[1;36m⏺ demo\033[0m  No LLM, GitHub login, app process, or network connection required.\n'
sleep 2
