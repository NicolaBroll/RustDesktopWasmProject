#!/bin/sh
# Regenerates the conflict-*.bin files (input of the conflict_tests.rs tests) in a temporary repo.
# Usage: sh conflict.sh <output-folder>
#
# A merge of `other` into `main` with conflicts in several files; each .bin file is the worktree
# file after `git merge other` (merge style), or after `git checkout --conflict=diff3` / `zdiff3`:
# - conflict-merge.bin: two blocks, merge style;
# - conflict-diff3.bin, conflict-zdiff3.bin: the same file with the base;
# - conflict-crlf.bin: lines with CRLF;
# - conflict-tail.bin: last line without a newline on both sides;
# - conflict-empty.bin: `main` removes the lines that `other` changes (our side empty);
# - conflict-size.bin: `conflict-marker-size=10` from `.gitattributes`;
# - conflict-latin1.bin: non-UTF-8 content.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

git init -q -b main repo
cd repo
printf 'size.txt conflict-marker-size=10\n' >.gitattributes
printf 'one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n' >merge.txt
printf 'one\r\ntwo\r\nthree\r\n' >crlf.txt
printf 'one\ntwo' >tail.txt
printf 'first\nx\ny\nafter\n' >empty.txt
printf 'one\ntwo\nthree\n' >size.txt
printf 'caf\351\ntwo\n' >latin1.txt
git add -A && git commit -q -m base

git checkout -q -b other
printf 'one\nTWO other\nthree\nfour\nfive\nsix\nseven\nEIGHT other\nnine\n' >merge.txt
printf 'one\r\nTWO other\r\nthree\r\n' >crlf.txt
printf 'one\ntwo other' >tail.txt
printf 'first\nX\nY\nafter\n' >empty.txt
printf 'one\nTWO other\nthree\n' >size.txt
printf 'caf\351 other\ntwo\n' >latin1.txt
git commit -q -am other

git checkout -q main
printf 'one\nTWO main\nthree\nfour\nfive\nsix\nseven\nEIGHT main\nEIGHT extra\nnine\n' >merge.txt
printf 'one\r\nTWO main\r\nthree\r\n' >crlf.txt
printf 'one\ntwo main' >tail.txt
printf 'first\nafter\n' >empty.txt
printf 'one\nTWO main\nthree\n' >size.txt
printf 'caf\351 main\ntwo\n' >latin1.txt
git commit -q -am main

git merge -q other >/dev/null 2>&1 || true
for f in merge crlf tail empty size latin1; do cp "$f.txt" "$out/conflict-$f.bin"; done
git checkout -q --conflict=diff3 -- merge.txt
cp merge.txt "$out/conflict-diff3.bin"
git checkout -q --conflict=zdiff3 -- merge.txt
cp merge.txt "$out/conflict-zdiff3.bin"
