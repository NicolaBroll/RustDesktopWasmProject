#!/bin/sh
# Rebuilds crates/ui/assets/icons.ttf: the toolbar glyphs taken from Phosphor Regular
# (https://phosphoricons.com, MIT; the same file as `res/Phosphor.ttf` of the egui-phosphor crate).
# Needs `pyftsubset` (pip install fonttools). The codepoints are those of `crates/ui/src/icons.rs`:
# add a glyph there and here.
#
#   scripts/icon-font.sh path/to/Phosphor.ttf
set -eu
src=${1:?"usage: $0 path/to/Phosphor.ttf"}
out="$(dirname "$0")/../crates/ui/assets/icons.ttf"
pyftsubset "$src" --output-file="$out" --no-hinting --layout-features='' \
    --unicodes='U+E00E,U+E036,U+E038,U+E05C,U+E066,U+E094,U+E136,U+E1AC,U+E46E'
echo "$out: $(wc -c <"$out") bytes"
