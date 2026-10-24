#!/bin/sh
# Regenerates log.bin and log-empty.bin (used by tests.rs) in a temporary repo.
# Usage: sh log.sh <output-folder>
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com

# at <seconds> <author zone> <committer zone>
at() {
    export GIT_AUTHOR_DATE="$1 $2" GIT_COMMITTER_DATE="$1 $3"
}

git init -q -b main .

# Root commit, empty body, positive time zone.
at 1700000000 +0200 +0200
git commit -q --allow-empty -m 'Root commit'

# Multi-line body; subject on two lines (same paragraph); different time zones.
at 1700000100 +0530 -0800
git commit -q --allow-empty -F - <<'MSG'
Subject on
two lines

First line of the body.
Second line of the body.

Second paragraph.
MSG

# Side branch with a non-UTF-8 author (Latin-1, no encoding header). `git commit`
# would convert the name to UTF-8, so the object is written by hand with `hash-object`.
git checkout -q -b side
{
    printf 'tree %s\n' "$(git write-tree)"
    printf 'parent %s\n' "$(git rev-parse HEAD)"
    printf 'author Jos\351 Latin1 <jose@example.com> 1700000200 -0330\n'
    printf 'committer Carlo Committer <carlo@example.com> 1700000200 +0000\n'
    printf '\nNon-UTF-8 author\n'
} | git hash-object -t commit -w --stdin >oid
git update-ref refs/heads/side "$(cat oid)"

# Latin-1 message declared with i18n.commitEncoding: git log converts it to UTF-8.
at 1700000300 +0100 +0100
printf 'Caf\351 declared Latin-1\n\nBody in Latin-1: caf\351.\n' |
    GIT_AUTHOR_NAME="$(printf 'Ren\351e Encoding')" \
    git -c i18n.commitEncoding=ISO-8859-1 commit -q --allow-empty -F -

# Merge with two parents, empty body.
git checkout -q main
at 1700000400 -0700 +1245
git merge -q --no-ff --no-edit -m 'Merge side' side

git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color \
    --format='%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b' \
    >"$out/log.bin"

# Empty range: no commits.
git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color \
    --format='%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b' \
    HEAD..HEAD >"$out/log-empty.bin"
