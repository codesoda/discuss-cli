#!/bin/bash
# Scene 10: the local PR simulation completes without publishing. Run from repo root.
set -e
CAP=demo/captures/scene6.ndjsonl
clear

printf '\033[1;36m⏺ demo\033[0m  The exact confirmed GFM was simulated locally.\n\n'
sleep 0.7
printf '\033[2m'
grep '"session.done"' "$CAP" | head -c 760
printf '\033[0m…\n\n'
sleep 1.0
printf '\033[1;32m✓ Demo publication simulated locally\033[0m\n'
printf '  No gh command ran. Nothing was sent to GitHub. No history was saved.\n'
sleep 3
