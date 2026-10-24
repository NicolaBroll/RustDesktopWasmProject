#!/usr/bin/env bash
# Builds target/appimage/BinariGit-<version>-x86_64.AppImage and SHA256SUMS.txt.
# Usage: packaging/appimage.sh   (or: make appimage)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/target/appimage"
CACHE="$ROOT/target/packaging-cache"
APPDIR="$OUT/BinariGit.AppDir"

# Pinned tools: the sha256 values are the digests GitHub publishes for these release assets.
APPIMAGETOOL_VERSION="1.9.1"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/${APPIMAGETOOL_VERSION}/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
# Passed with --runtime-file so appimagetool does not fetch the unpinned "continuous" runtime.
RUNTIME_VERSION="20251108"
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/${RUNTIME_VERSION}/runtime-x86_64"
RUNTIME_SHA256="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"

# fetch <url> <sha256> <dest>: downloads once into the cache and always verifies the checksum.
fetch() {
    local url="$1" sum="$2" dest="$3"
    if [ ! -f "$dest" ] || ! echo "$sum  $dest" | sha256sum --check --status; then
        rm -f "$dest" "$dest.part"
        curl -fL --retry 3 -o "$dest.part" "$url"
        if ! echo "$sum  $dest.part" | sha256sum --check --status; then
            echo "checksum mismatch for $url" >&2
            rm -f "$dest.part"
            exit 1
        fi
        mv "$dest.part" "$dest"
    fi
}

VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml")"
[ -n "$VERSION" ] || { echo "version not found in Cargo.toml" >&2; exit 1; }
NAME="BinariGit-${VERSION}-x86_64.AppImage"

mkdir -p "$CACHE" "$OUT"
fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" "$CACHE/appimagetool-${APPIMAGETOOL_VERSION}-x86_64.AppImage"
fetch "$RUNTIME_URL" "$RUNTIME_SHA256" "$CACHE/runtime-${RUNTIME_VERSION}-x86_64"
chmod +x "$CACHE/appimagetool-${APPIMAGETOOL_VERSION}-x86_64.AppImage"

cargo build --release --locked -p project --manifest-path "$ROOT/Cargo.toml"

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
install -m 755 "$ROOT/target/release/project" "$APPDIR/usr/bin/binarigit"
install -m 644 "$ROOT/assets/icon-256.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/binarigit.png"
install -m 644 "$ROOT/assets/icon.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/binarigit.svg"
install -m 644 "$ROOT/assets/icon-256.png" "$APPDIR/binarigit.png"
ln -s binarigit.png "$APPDIR/.DirIcon"
install -m 644 "$ROOT/packaging/binarigit.desktop" "$APPDIR/binarigit.desktop"
install -m 644 "$ROOT/packaging/binarigit.desktop" "$APPDIR/usr/share/applications/binarigit.desktop"
install -m 755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"
for doc in LICENSE-MIT LICENSE-APACHE ATTRIBUTION.md; do
    install -D -m 644 "$ROOT/$doc" "$APPDIR/usr/share/doc/binarigit/$doc"
done

rm -f "$OUT/$NAME"
# No FUSE in containers: the tool extracts itself instead of mounting.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$CACHE/appimagetool-${APPIMAGETOOL_VERSION}-x86_64.AppImage" \
    --no-appstream --runtime-file "$CACHE/runtime-${RUNTIME_VERSION}-x86_64" "$APPDIR" "$OUT/$NAME"

(cd "$OUT" && sha256sum "$NAME" > SHA256SUMS.txt)
echo "AppImage: $OUT/$NAME"
cat "$OUT/SHA256SUMS.txt"
