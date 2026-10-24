#!/bin/sh
# Regenerates the diff-*.bin files (used by diff_tests.rs) in a temporary repo.
# Usage: sh diff.sh <output-folder>
#
# Each file is the output of `git -c core.quotePath=false diff` with the DIFF_ARGS arguments
# (repeated below in `d`: if they change in diff.rs they must change here too), plus the
# revisions or `--cached`. Exceptions, noted case by case: `diff-names-quotepath.bin` leaves
# `core.quotePath` at its default value (true, non-ASCII names in octal) and `diff-config.bin`
# uses a hostile configuration that the arguments must neutralize.
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

git init -q -b main repo
cd repo

# --- Text: several hunks with a context header, omitted counts, end of file without a newline,
# CRLF, non-UTF-8 content, lines that look like headers. ----------------------------------
printf 'int main() {\n' >code.c
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    printf '    line(%s);\n' "$i" >>code.c
done
printf '}\n' >>code.c
printf 'only\n' >one-line.txt
printf 'stays\nno newline' >adds-newline.txt
printf 'stays\nwith newline\n' >removes-newline.txt
printf 'old no newline' >both-without.txt
printf 'one\r\ntwo\r\n' >crlf.txt
printf 'caf\351\nsecond\n' >latin1.txt
printf 'head\n-- dashes\n++ na\303\257ve\ntail\n' >fake-headers.txt
printf 'first\n\nthird\n' >line-empty.txt
git add -A
git commit -q -m 'Text'

sed -e 's/line(2);/line(2); \/\/ changed/' -e 's/line(18);/line(18);\n    new();/' code.c >tmp
mv tmp code.c
printf 'single\n' >one-line.txt
printf 'stays\nno newline\n' >adds-newline.txt
printf 'stays\nwith newline' >removes-newline.txt
printf 'new no newline' >both-without.txt
printf 'one\r\nTWO\r\n' >crlf.txt
printf 'caf\351\nSECOND\n' >latin1.txt
printf 'head\n--- a/fake\n+++ b/fake\ndiff --git a/x b/x\ntail\n' >fake-headers.txt
printf 'first\n\nTHIRD\n' >line-empty.txt
d >"$out/diff-text.bin"

# Same changes with a hostile configuration: the DIFF_ARGS arguments neutralize it
# (except `diff.suppressBlankEmpty`, which writes empty context lines without the space).
cat >"$dir/ostile.gitconfig" <<'CFG'
[diff]
	noprefix = true
	mnemonicPrefix = true
	external = false
	renames = false
	relative = true
	submodule = log
	suppressBlankEmpty = true
	context = 0
[color]
	ui = always
	diff = always
[core]
	quotePath = true
CFG
GIT_CONFIG_GLOBAL="$dir/ostile.gitconfig" d >"$out/diff-config.bin"
git add -A
git commit -q -m 'Text changed'

# --- Files: new, deleted, empty, mode changes, renames, type change, binaries. ----------
printf 'content long enough to recognize the rename\n' >rename-exact.txt
printf 'content slightly modified, long enough\nline two\nline three\nline four\n' >rename-modified.txt
printf 'becomes executable and changes name\n' >rename-mode.sh
printf 'to delete\nsecond\n' >deleted.txt
: >empty-deleted.txt
printf '#!/bin/sh\n' >executable.sh
printf 'becomes a link\n' >kind.txt
printf 'GIF89a\000\001\002\003' >image.gif
printf 'BIN\000\001' >binary-deleted.bin
git add -A
git commit -q -m 'File'

git mv rename-exact.txt renamed-exact.txt
git mv rename-modified.txt renamed-modified.txt
printf 'line five\n' >>renamed-modified.txt
git mv rename-mode.sh renamed-mode.sh
chmod +x renamed-mode.sh
git rm -q deleted.txt empty-deleted.txt binary-deleted.bin
: >empty-new.txt
printf 'new\n' >new.txt
chmod +x executable.sh
rm kind.txt
ln -s new.txt kind.txt
printf 'GIF89a\000\001\002\004' >image.gif
printf 'NEW\000\002' >binary-new.bin
git add -A
d --cached >"$out/diff-file.bin"
git commit -q -m 'File changed'

# --- Copies: recognized only with -C. ----------------------------------------------------
cp new.txt copy.txt
git add copy.txt
d --cached -C --find-copies-harder >"$out/diff-copies.bin"
git commit -q -m 'Copy'

# --- Names: spaces, non-UTF-8, newline, tab, quotes, backslash. --------------------------
printf 'space\n' >'with space.txt'
printf 'latin\n' >"$(printf 'caf\377.txt')"
printf 'utf8\n' >"$(printf 'na\303\257ve.txt')"
printf 'newline\n' >"$(printf 'a\nbreak.txt')"
printf 'tab\n' >"$(printf 'with\ttab.txt')"
printf 'quotes\n' >'with "quotes".txt'
printf 'backslash\n' >'with\back.txt'
printf 'to rename with a strange name\n' >simple.txt
git add -A
git commit -q -m 'Names'
for f in 'with space.txt' "$(printf 'caf\377.txt')" "$(printf 'na\303\257ve.txt')" \
    "$(printf 'a\nbreak.txt')" "$(printf 'with\ttab.txt')" 'with "quotes".txt' 'with\back.txt'; do
    printf 'modified\n' >>"$f"
done
git mv simple.txt "$(printf 'strange "name"\twith space.txt')"
git add -A
d --cached >"$out/diff-names.bin"
# core.quotePath at its default (true): non-ASCII bytes become octal escapes.
git diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3 --cached >"$out/diff-names-quotepath.bin"
git commit -q -m 'Names changed'

# --- Submodule: added, then moved to another commit (with `diff.submodule=log`, which
# `--submodule=short` neutralizes). -------------------------------------------------
git init -q -b main "$dir/sub"
git -C "$dir/sub" commit -q --allow-empty -m 'One'
git -c protocol.file.allow=always submodule --quiet add "$dir/sub" sub
# Relative URL instead of the temporary folder, for reproducible output.
git config -f .gitmodules submodule.sub.url ../sub
git add .gitmodules
d --cached >"$out/diff-submodule-new.bin"
git commit -q -m 'Submodule'
git -C "$dir/sub" commit -q --allow-empty -m 'Two'
git -C sub -c protocol.file.allow=always pull -q
git -c diff.submodule=log diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3 >"$out/diff-submodule.bin"
git add sub
git commit -q -m 'Submodule updated'

# --- No changes: empty output. -----------------------------------------------------------
d >"$out/diff-empty.bin"

# --- Conflict: in the worktree a combined diff (`diff --cc`), in the index `* Unmerged path`.
git checkout -q -b side
printf 'from the branch\n' >conflict.txt
printf 'side\n' >"$(printf 'conflict\tstrange.txt')"
git add -A
git commit -q -m 'Side'
git checkout -q main
printf 'from main\n' >conflict.txt
printf 'main\n' >"$(printf 'conflict\tstrange.txt')"
printf 'clean\n' >>new.txt
git add -A
git commit -q -m 'Primary'
git merge -q side >/dev/null 2>&1 || true
printf 'in the worktree\n' >>new.txt
d >"$out/diff-conflict.bin"
d --cached >"$out/diff-conflict-index.bin"
