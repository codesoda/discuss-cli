#!/bin/bash
# Scene 10: session.done with verdict arrives in the terminal. Run from the repo root.
CAP=demo/captures/scene9.ndjsonl
clear

printf '\033[1;36m⏺ agent\033[0m  Review finished — reading the transcript…\n\n'
sleep 0.6
printf '\033[2m'
grep '"session.done"' "$CAP" | head -c 700
printf '\033[0m…\n\n'
sleep 1.2
printf '\033[1;36m⏺ agent\033[0m  Verdict: \033[1;31mDecline\033[0m — "Ship after the risks section is fixed."\n'
sleep 0.5
printf '\033[1;36m⏺ agent\033[0m  I will fix the risks section and rerun the review. Same terminal session.\n'
sleep 2.5
