#!/bin/sh
# Regenerates blame-*.bin (used by blame_tests.rs) in a temporary repo.
# Usage: sh blame.sh <output-folder>
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
n=1700000000
tick() {
    n=$((n + 100))
    export GIT_AUTHOR_DATE="$n +0200" GIT_COMMITTER_DATE="$n -0530"
}
blame() {
    git -c core.fsmonitor=false -c blame.ignoreRevsFile= blame --porcelain "$@"
}

git init -q -b main .
# Root commit: its lines are `boundary` (blame.showRoot is false by default).
tick; printf 'one\ntwo\n\nfour\n' >old.txt; git add old.txt; git commit -q -m 'Root'
# Rename, then edits: `filename` differs between commits, `previous` names the old path.
tick; git mv old.txt new.txt; git commit -q -m 'Rename'
tick; printf 'one\nTWO\n\nfour\nfive\n' >new.txt; git commit -q -am 'Edit two, add five'
# Latin-1 line and no final newline.
tick; printf 'one\nTWO\n\nfour\nfive\ncaf\351\n\nlast' >new.txt
GIT_AUTHOR_NAME="$(printf 'Ren\351e')" git commit -q -am 'Latin-1 line, no final newline'
blame -- new.txt >"$out/blame-head.bin"
# Explicit revision.
blame "$(git rev-parse HEAD~1)" -- new.txt >"$out/blame-rev.bin"
# Uncommitted worktree lines (all-zero id).
printf 'zero\none\nTWO\n\nfour\nfive\ncaf\351\n\nlast\n' >new.txt
blame -- new.txt >"$out/blame-worktree.bin"
# Name with a tab and a non-UTF-8 byte.
odd=$(printf 'odd\tcaf\351.txt')
tick; printf 'x\n' >"$odd"; git add "$odd"; git commit -q -m 'Odd name'
blame -- "$odd" >"$out/blame-odd-name.bin"
# Empty file: no lines.
tick; : >empty.txt; git add empty.txt; git commit -q -m 'Empty'
blame -- empty.txt >"$out/blame-empty.bin"
