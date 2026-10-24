#!/bin/sh
# Regenerates the diff-tree-*.bin files (used by diff_tree_tests.rs) in a temporary repo.
# Usage: sh diff-tree.sh <output-folder>
#
# Each file is the output of `git diff-tree` with the DIFF_TREE_ARGS arguments (repeated
# below in `dt`: if they change in diff_tree.rs they must change here too), plus the revisions:
# `--root <commit>` for the root commit, `<first parent> <commit>` for the others.
# `diff-tree-copies.bin` adds `-C --find-copies-harder`.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

dt() {
    git diff-tree -z -r --raw --no-abbrev -M --no-commit-id "$@"
}

git init -q -b main repo
cd repo

# --- Root: files in folders, with spaces, newlines and non-UTF-8 bytes in the name. -------
mkdir -p src/nested
printf 'one\ntwo\nthree\nfour\nfive\nsix\n' >modified.txt
printf 'content long enough to recognize the rename\n' >old.txt
printf 'content slightly modified, long enough\nline two\nline three\nline four\n' >move.txt
printf 'to delete\n' >deleted.txt
printf '#!/bin/sh\n' >script.sh
printf 'becomes a link\n' >kind.txt
printf 'space\n' >'with space.txt'
printf 'newline\n' >"$(printf 'a\nbreak.txt')"
printf 'latin\n' >"$(printf 'caf\377.txt')"
printf 'deep\n' >src/nested/deep.rs
git add -A
git commit -q -m 'Root'
dt --root HEAD >"$out/diff-tree-root.bin"

# --- Every kind of change relative to the parent. ----------------------------------------
printf 'one\ntwo\nTHREE\nfour\nfive\nsix\n' >modified.txt
git mv old.txt new.txt
git mv move.txt moved.txt
printf 'line five\n' >>moved.txt
git rm -q deleted.txt
chmod +x script.sh
rm kind.txt
ln -s modified.txt kind.txt
printf 'added\n' >src/added.rs
git add -A
git commit -q -m 'Every kind of change'
dt HEAD~1 HEAD >"$out/diff-tree-mixed.bin"

# --- Copy: recognized only with -C. ------------------------------------------------------
cp modified.txt copy.txt
git add copy.txt
git commit -q -m 'Copy'
dt -C --find-copies-harder HEAD~1 HEAD >"$out/diff-tree-copies.bin"

# --- Empty commit: no files. -------------------------------------------------------------
git commit -q --allow-empty -m 'Empty'
dt HEAD~1 HEAD >"$out/diff-tree-empty.bin"

# --- Merge: comparison with the first parent (the files brought by the merged branch). ---
git checkout -q -b side
printf 'from the branch\n' >side.txt
git add side.txt
git commit -q -m 'Side'
git checkout -q main
printf 'from main\n' >primary.txt
git add primary.txt
git commit -q -m 'Primary'
git merge -q --no-ff --no-edit side
dt HEAD^1 HEAD >"$out/diff-tree-merge.bin"
