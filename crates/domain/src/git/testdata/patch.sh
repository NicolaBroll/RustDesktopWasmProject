#!/bin/sh
# Regenerates the patch-*.bin files (input of the patch_tests.rs tests) in a temporary repo.
# Usage: sh patch.sh <output-folder>
#
# Each file is the output of `git -c core.quotePath=false diff` with the DIFF_ARGS arguments
# (repeated below in `d`), plus `--cached` for the index:
# - patch-worktree.bin: `d`, the worktree → index diff, input of line staging;
# - patch-index.bin: `d --cached`, the HEAD → index diff, input of line unstaging.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

d() {
    git -c core.quotePath=false diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3 "$@"
}

lines() {
    i=1
    while [ "$i" -le "$1" ]; do
        printf 'line %s\n' "$i"
        i=$((i + 1))
    done
}

git init -q -b main repo
cd repo

# --- Common base -----------------------------------------------------------------------
lines 20 >near.txt
printf 'one\nno newline' >adds-newline.txt
printf 'one\nwith newline\n' >removes-newline.txt
printf 'old' >both-without.txt
printf 'a\nb' >tail.txt
printf 'one\r\ntwo\r\nthree\r\n' >crlf.txt
printf 'caf\351\nsecond\n' >latin1.txt
printf 'a\nb\nc\n' >"with	tab.txt"
printf 'a\nb\nc\n' >"$(printf 'name\377.txt')"
printf 'gone one\ngone two\ngone three\n' >deleted.txt
printf 'stays\n' >script.sh
lines 8 >old-name.txt
git add -A
git commit -q -m base

# --- Index: staged changes (input of unstaging) -------------------------------------------
# near.txt: two hunks separated by unchanged lines, and two nearby changes in the same hunk.
sed -e 's/^line 2$/line 2 staged/' -e 's/^line 4$/line 4 staged/' \
    -e 's/^line 15$/line 15 staged\nline 15 extra/' near.txt >tmp && mv tmp near.txt
printf 'one\nno newline\nmore\n' >adds-newline.txt
printf 'new' >both-without.txt
printf 'ONE\r\ntwo\r\nTHREE\r\n' >crlf.txt
printf 'new one\nnew two\nnew three\n' >new.txt
git rm -q deleted.txt
git mv old-name.txt "new name.txt"
sed -e 's/^line 7$/line 7 renamed/' "new name.txt" >tmp && mv tmp "new name.txt"
printf 'a\nB\nc\nd\n' >"with	tab.txt"
git add -A
d --cached >"$out/patch-index.bin"
git commit -q -m index

# --- Worktree: unstaged changes (input of staging) ----------------------------------------
lines 20 >near.txt
sed -e 's/^line 1$/line 1 in the worktree/' -e 's/^line 9$/line 9 in the worktree/' \
    -e 's/^line 17$/line 17 in the worktree/' near.txt >tmp && mv tmp near.txt
printf 'one\nno newline\n' >adds-newline.txt
printf 'one\nwith newline' >removes-newline.txt
printf 'other\nnew' >both-without.txt
printf 'a\nb\nc' >tail.txt
printf 'ONE\r\nTWO\r\nTHREE\r\nfour\r\n' >crlf.txt
printf 'caf\351\nSECOND\nthird\n' >latin1.txt
printf 'a\nb\nc\nd\ne\n' >"$(printf 'name\377.txt')"
rm "new name.txt"
printf 'stays\nadded\n' >script.sh
chmod +x script.sh
printf 'ready one\nready two\nready three\n' >intent.txt
git add -N intent.txt
d >"$out/patch-worktree.bin"
