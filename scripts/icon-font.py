#!/usr/bin/env python3
"""Rebuilds crates/ui/assets/icons.ttf: the UI glyphs taken from Phosphor Regular
(https://phosphoricons.com, MIT; the same file as `res/Phosphor.ttf` of the egui-phosphor crate).

Needs fontTools (pip install fonttools). The codepoints are those of `crates/ui/src/icons.rs`:
add a glyph there and here. ALIASES also map a few text symbols the default fonts lack (↑ ↓ ⚠)
onto icons, so they render in any label.

    python3 scripts/icon-font.py path/to/Phosphor.ttf
"""

import pathlib
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

ICONS = [
    0xE00E,  # box-arrow-down: stash
    0xE036,  # arrow-clockwise: redo
    0xE038,  # arrow-counter-clockwise: undo
    0xE03E,  # arrow-down: behind
    0xE05C,  # arrow-line-down: pull
    0xE066,  # arrow-line-up: push
    0xE08E,  # arrow-up: ahead
    0xE094,  # arrows-clockwise: refresh
    0xE136,  # caret-down
    0xE13A,  # caret-right
    0xE182,  # check: current branch
    0xE1AA,  # cloud: remotes
    0xE1AC,  # cloud-arrow-down: fetch
    0xE1CA,  # copy
    0xE1FE,  # dots-three: more
    0xE24A,  # folder
    0xE278,  # git-branch
    0xE30C,  # magnifying-glass: filter
    0xE3D4,  # plus
    0xE46E,  # stop-circle
    0xE478,  # tag
    0xE4E0,  # warning
    0xE4F6,  # x
    0xE546,  # columns: side by side diff
    0xE5A2,  # rows: unified diff
]

ALIASES = {
    0x2191: 0xE08E,  # ↑
    0x2193: 0xE03E,  # ↓
    0x26A0: 0xE4E0,  # ⚠
}


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} path/to/Phosphor.ttf")
    out = pathlib.Path(__file__).resolve().parent.parent / "crates/ui/assets/icons.ttf"
    options = subset.Options()
    options.hinting = False
    options.layout_features = []
    font = TTFont(sys.argv[1])
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=ICONS)
    subsetter.subset(font)
    for table in font["cmap"].tables:
        if table.isUnicode():
            for alias, icon in ALIASES.items():
                table.cmap[alias] = table.cmap[icon]
    font.save(out)
    print(f"{out}: {out.stat().st_size} bytes")


if __name__ == "__main__":
    main()
