# Run from the repo root: make -f demo/Makefile demo
.PHONY: demo browser terminal stitch clean

demo: browser terminal stitch

browser:
	cd demo && node record.mjs

terminal:
	vhs demo/scene01.tape
	vhs demo/scene10.tape

stitch:
	bash demo/stitch.sh

clean:
	rm -rf demo/clips demo/frames demo/captures demo/main-cut.mp4
