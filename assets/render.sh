#!/bin/sh
# Regenerates the PNGs from the SVGs in this folder with headless Chrome (ImageMagick renders
# gradients and curves poorly). Usage: assets/render.sh [path to chrome]
set -eu
chrome=${1:-$(command -v google-chrome || command -v chromium || echo /opt/google/chrome/chrome)}
here=$(cd "$(dirname "$0")" && pwd)
render() { # <svg> <png> <width> <height>
    tmp=$(mktemp -d)
    printf '<html><body style="margin:0"><img src="file://%s/%s" width="%s" height="%s" style="display:block"></body></html>' \
        "$here" "$1" "$3" "$4" > "$tmp/p.html"
    "$chrome" --headless --no-sandbox --disable-gpu --hide-scrollbars --default-background-color=00000000 \
        --user-data-dir="$tmp/u" --window-size="$3,$4" --screenshot="$here/$2" "file://$tmp/p.html" >/dev/null 2>&1
    rm -rf "$tmp"
}
render icon.svg icon-256.png 256 256
render icon.svg icon-512.png 512 512
render icon.svg favicon-32.png 32 32
render logo.svg logo.png 1500 512
render banner.svg banner.png 1280 640
# Windows icon of the .exe (apps/project/build.rs), from the PNGs rendered above.
convert "$here/icon-512.png" -define icon:auto-resize=256,64,48,32,16 "$here/icon.ico"
