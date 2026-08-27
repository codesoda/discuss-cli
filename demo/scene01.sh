#!/bin/bash
# Scene 1: agent session launches discuss. Run from the repo root.
BIN=./target/release/discuss
clear
type_out() {
  local s="$1"
  local i
  for ((i = 0; i < ${#s}; i++)); do
    printf '%s' "${s:i:1}"
    sleep 0.028
  done
  printf '\n'
}

printf '\033[1;35m❯ you\033[0m  '
type_out "Can you discuss docs/demo-fixtures/plan.md with me?"
sleep 0.7
printf '\n\033[1;36m⏺ agent\033[0m  Launching a discuss session…\n\n'
sleep 0.5
printf '\033[2m$ discuss docs/demo-fixtures/plan.md\033[0m\n'
sleep 0.4
TMP=$(mktemp)
$BIN docs/demo-fixtures/plan.md --no-open --no-save >"$TMP" 2>/dev/null &
PID=$!
sleep 1.5
kill $PID 2>/dev/null
head -c 700 "$TMP"
printf '…\n\n'
rm -f "$TMP"
sleep 0.8
printf '\033[1;36m⏺ agent\033[0m  Session open — drop a comment in the browser and I will reply with a take.\n'
sleep 2
