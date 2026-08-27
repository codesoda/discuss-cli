#!/bin/bash
# Normalizes clips, burns captions, concatenates, and produces docs/demo.mp4 + docs/demo.gif.
# Run from the repo root.
set -euo pipefail

CLIPS=demo/clips
NORM=demo/clips/norm
FRAMES=demo/frames
CAPTIONS=demo/captions
mkdir -p "$NORM" "$FRAMES"
rm -f "$NORM"/*.mp4 "$FRAMES"/*.png

# caption strips are pre-rendered PNGs (see demo/captions.mjs)
(cd demo && node captions.mjs)

LIST=$NORM/concat.txt
: > "$LIST"

for name in scene01 scene02 scene03 scene04 scene05 scene06 scene07 scene08 scene09 scene10; do
  src=""
  [ -f "$CLIPS/$name.mp4" ] && src="$CLIPS/$name.mp4"
  [ -f "$CLIPS/$name.webm" ] && src="$CLIPS/$name.webm"
  if [ -z "$src" ]; then
    echo "skip: $name (no clip)"
    continue
  fi
  ffmpeg -y -loglevel error -i "$src" -i "$CAPTIONS/$name.png" -filter_complex "\
[0:v]scale=1280:800:force_original_aspect_ratio=decrease,\
pad=1280:800:(ow-iw)/2:(oh-ih)/2:color=0x0d1117,fps=30[v];\
[v][1:v]overlay=0:0" \
    -an -c:v libx264 -preset medium -crf 19 -pix_fmt yuv420p "$NORM/$name.mp4"
  echo "file '$name.mp4'" >> "$LIST"
  echo "normalized: $name"
done

ffmpeg -y -loglevel error -f concat -safe 0 -i "$LIST" -c copy demo/main-cut.mp4
ffmpeg -y -loglevel error -i demo/main-cut.mp4 -c:v libx264 -preset slow -crf 22 -pix_fmt yuv420p -r 30 -an docs/demo.mp4
echo "wrote docs/demo.mp4 ($(du -h docs/demo.mp4 | cut -f1))"

ffmpeg -y -loglevel error -i demo/main-cut.mp4 -vf "fps=10,scale=960:-1" "$FRAMES/frame%04d.png"
gifski --fps 10 --width 960 --quality 70 -o docs/demo.gif "$FRAMES"/frame*.png
echo "wrote docs/demo.gif ($(du -h docs/demo.gif | cut -f1))"
