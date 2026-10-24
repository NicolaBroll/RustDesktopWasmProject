#!/bin/sh
# Regenerates the refs-*.bin files (used by refs_tests.rs) in temporary repos.
# Usage: sh refs.sh <output-folder>
#
# Each file is the output of `git for-each-ref` with the REFS_ARGS arguments (format and pattern
# repeated below in `refs`: if they change in refs.rs they must change here too).
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

refs() {
    git for-each-ref \
        '--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00' \
        refs/heads/ refs/remotes/ refs/tags/ refs/stash
}

commit() {
    git commit -q --allow-empty -m "$1"
}

# --- Freshly created repo: no refs, empty output. -----------------------------------------
git init -q -b main empty
(cd empty && refs >"$out/refs-empty.bin")

# --- Repo with a real (bare) remote and every kind of ref. --------------------------------
git init -q --bare -b main origin.git
git init -q -b main repo
(
    cd repo
    git remote add origin ../origin.git
    echo base >file.txt
    git add file.txt
    commit 'Base'
    # main: level with origin/main.
    git push -q -u origin main 2>/dev/null

    # forward: two commits not yet pushed (ahead 2).
    git checkout -q -b forward
    git push -q -u origin forward 2>/dev/null
    commit 'Forward 1'
    commit 'Forward 2'

    # back: origin/back has one more commit (behind 1).
    git checkout -q -b back main
    commit 'Back 1'
    git push -q -u origin back 2>/dev/null
    git reset -q --hard HEAD~1

    # both: diverged (ahead 1, behind 2).
    git checkout -q -b both main
    commit 'Both remote 1'
    commit 'Both remote 2'
    git push -q -u origin both 2>/dev/null
    git reset -q --hard main
    commit 'Both local'

    # gone: upstream configured but deleted from the remote (gone).
    git checkout -q -b gone main
    git push -q -u origin gone 2>/dev/null
    git push -q origin --delete gone 2>/dev/null

    # Only on the remote: refs/remotes/origin/remote-only with no local branch.
    git checkout -q -b only-remote main
    commit 'Solo remote'
    git push -q origin only-remote 2>/dev/null
    git checkout -q main
    git branch -q -D only-remote

    # symbolic origin/HEAD pointing to origin/main.
    git remote set-head origin main >/dev/null

    # Branches without upstream: with `/`, non-ASCII, non-UTF-8 (byte 0xff).
    git branch feature/new-interface
    git branch "$(printf 'caf\303\251')"
    git branch "$(printf 'latin-\377')"
    # Local upstream (remote = `.`): the upstream is refs/heads/main.
    git branch -q --track on-main main
    # Local symbolic ref: discarded by the parser like origin/HEAD.
    git symbolic-ref refs/heads/alias refs/heads/main

    # Tags: lightweight, annotated, annotated tag of an annotated tag, lightweight on a tree.
    git tag v0.1
    git tag -a -m 'Version 1.0' v1.0 forward
    git -c advice.nestedTag=false tag -a -m 'Tag of the tag' v1.0-double v1.0
    git tag tree 'main^{tree}'

    # Notes: refs/notes/commits is not in the patterns.
    git notes add -m 'a note' main

    # Stash.
    echo change >>file.txt
    git stash -q

    refs >"$out/refs.bin"
)

# --- Upstream with a newline in the name (from `branch.<b>.merge` in the config). --------
git init -q -b main newline
(
    cd newline
    commit 'Base'
    git config remote.origin.url ../origin.git
    git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    git config branch.main.remote origin
    git config branch.main.merge "$(printf 'refs/heads/line\nnew')"
    refs >"$out/refs-upstream-newline.bin"
)
