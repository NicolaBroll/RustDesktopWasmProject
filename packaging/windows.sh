#!/usr/bin/env bash
# Cross-compiles the desktop app for Windows (x86_64-pc-windows-gnu, mingw-w64) and builds
# target/windows/BinariGit-<version>-x86_64-windows.zip and SHA256SUMS.txt.
# Usage: packaging/windows.sh   (or: make windows)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/target/windows"
TARGET="x86_64-pc-windows-gnu"
STAGE="$OUT/BinariGit"

for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-windres zip; do
    command -v "$tool" >/dev/null || {
        echo "missing $tool: sudo apt-get install gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64 zip" >&2
        exit 1
    }
done
rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"

VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml")"
[ -n "$VERSION" ] || { echo "version not found in Cargo.toml" >&2; exit 1; }
NAME="BinariGit-${VERSION}-x86_64-windows.zip"

cargo build --release --locked -p project --target "$TARGET" --manifest-path "$ROOT/Cargo.toml"

rm -rf "$STAGE"
mkdir -p "$STAGE"
install -m 644 "$ROOT/target/$TARGET/release/project.exe" "$STAGE/BinariGit.exe"
sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/windows-README.txt" | sed 's/$/\r/' > "$STAGE/README.txt"
for doc in LICENSE-MIT LICENSE-APACHE ATTRIBUTION.md; do
    sed 's/$/\r/' "$ROOT/$doc" > "$STAGE/$doc.txt"
done

rm -f "$OUT/$NAME"
(cd "$OUT" && zip -q -X -r "$NAME" BinariGit)
(cd "$OUT" && sha256sum "$NAME" > SHA256SUMS.txt)
echo "Windows zip: $OUT/$NAME"
cat "$OUT/SHA256SUMS.txt"
