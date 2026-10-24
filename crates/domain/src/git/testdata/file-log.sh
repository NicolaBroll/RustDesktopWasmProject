#!/bin/sh
# Regenerates file-log-*.bin (used by file_log_tests.rs) in a temporary repo, with the
# arguments of FILE_LOG_ARGS (LOG_ARGS + --name-status --diff-merges=first-parent --root --no-relative).
# Usage: sh file-log.sh <output-folder>
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
    export GIT_AUTHOR_DATE="$n +0200" GIT_COMMITTER_DATE="$n +0200"
}

git init -q -b main .
tick; printf 'a\nb\nc\nd\ne\n' >old.txt; git add old.txt; git commit -q -m 'Add old'
tick; printf 'other\n' >other.txt; git add other.txt; git commit -q -m 'Unrelated'
tick; printf 'a\nb\nc\nd\ne\nf\n' >old.txt; git commit -q -am 'Edit old' -m 'Body line.'
tick; git mv old.txt new.txt; git commit -q -m 'Rename'
# Side branch and main both change new.txt: the merge resolves the conflict with a third version,
# so it differs from both parents and git log shows it.
git checkout -q -b side
tick; printf 'A\nb\nc\nd\ne\nG\n' >new.txt; git commit -q -am 'Side edit'
git checkout -q main
tick; printf 'a\nb\nc\nd\ne\nF\n' >new.txt; git commit -q -am 'Main edit'
tick; git merge -q --no-edit side >/dev/null 2>&1 || true
printf 'A\nb\nc\nd\ne\nF\nmerged\n' >new.txt; git add new.txt; git commit -q --no-edit
# Non-UTF-8 name (Latin-1), added then deleted.
lat=$(printf 'caf\351.txt')
tick; printf 'x\n' >"$lat"; git add "$lat"; git commit -q -m 'Add latin1'
tick; git rm -q "$lat"; git commit -q -m 'Remove latin1'

F='%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b'
log() {
    git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color \
        --format="tformat:$F" --name-status --diff-merges=first-parent --root --no-relative "$@"
}
log --follow -- new.txt >"$out/file-log-follow.bin"
log -- new.txt >"$out/file-log-nofollow.bin"
log -- "$lat" >"$out/file-log-latin1.bin"
log -- missing.txt >"$out/file-log-empty.bin"
