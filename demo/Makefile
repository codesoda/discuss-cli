# Run from the repo root: make -f demo/Makefile demo
.PHONY: demo release dependencies browser terminal stitch clean

demo: browser terminal stitch

release:
	cargo build --release

dependencies:
	npm ci --prefix demo
	npx --prefix demo playwright install chromium ffmpeg

browser: release dependencies
	cd demo && node record.mjs

terminal: release
	vhs demo/scene01.tape
	vhs demo/scene10.tape

stitch: dependencies
	bash demo/stitch.sh

clean:
	rm -rf demo/node_modules demo/clips demo/frames demo/captions demo/captures demo/main-cut.mp4
