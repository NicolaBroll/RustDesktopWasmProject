#!/usr/bin/env bash
# Builds git <version> from the sources on github.com/git/git and installs it in <prefix>.
# Only what the tests need: no curl (http remotes), no gettext, perl, python or tcl/tk.
#
#   .github/scripts/build-git.sh 2.30.9 "$HOME/git-2.30.9"
set -euo pipefail

version="$1"
prefix="$2"
src="$(mktemp -d)"

git clone --quiet --depth 1 --branch "v$version" https://github.com/git/git "$src"
make -C "$src" -j"$(nproc)" prefix="$prefix" \
  NO_CURL=1 NO_EXPAT=1 NO_OPENSSL=1 NO_GETTEXT=1 NO_PERL=1 NO_PYTHON=1 NO_TCLTK=1 NO_INSTALL_HARDLINKS=1 \
  install >/dev/null
rm -rf "$src"
"$prefix/bin/git" --version
