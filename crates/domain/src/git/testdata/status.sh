#!/bin/sh
# Regenerates the status-*.bin files (used by status_tests.rs) in temporary repos.
# Usage: sh status.sh <output-folder>
#
# Each file is the output of `git status --porcelain=v2 -z --branch --untracked-files=all`
# (the STATUS_ARGS arguments); `status-ignored.bin` adds `--ignored` and
# `status-copies.bin` uses `-c status.renames=copies`.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Author' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

status() {
    git status --porcelain=v2 -z --branch --untracked-files=all "$@"
}

# --- Empty repo: no commits, then a staged file and an untracked one. ------------------
git init -q -b main empty
(
    cd empty
    status >"$out/status-empty.bin"
    echo new >new.txt
    git add new.txt
    echo loose >loose.txt
    status >"$out/status-empty-file.bin"
)

# --- Submodule used by the mixed repo. ---------------------------------------------------
git init -q -b main sub
(
    cd sub
    echo sub >sub.txt
    git add sub.txt
    git commit -q -m 'Submodule'
)

# --- Mixed repo: upstream with ahead/behind and every kind of change. --------------------
git init -q -b main mixed
(
    cd mixed
    for f in modified.txt in-stage.txt both.txt removed.txt removed-wt.txt \
        old-name.txt kind.txt executable.sh 'with space.txt'; do
        printf 'line 1 di %s\nline 2\nline 3\nline 4\n' "$f" >"$f"
    done
    printf 'latin1 content\n' >"$(printf 'old-\351.txt')"
    git add .
    git -c protocol.file.allow=always submodule add -q ../sub module
    git commit -q -m 'Base'
    base=$(git rev-parse HEAD)

    # Upstream: origin/main has a commit main does not have (behind 1); main has two (ahead 2).
    git config remote.origin.url ../origin.git
    git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    git config branch.main.remote origin
    git config branch.main.merge refs/heads/main
    git checkout -q -b remote
    git commit -q --allow-empty -m 'Only on the remote'
    git update-ref refs/remotes/origin/main "$(git rev-parse HEAD)"
    git checkout -q main
    git branch -q -D remote
    git commit -q --allow-empty -m 'Local 1'
    git commit -q --allow-empty -m 'Local 2'

    echo change >>modified.txt
    echo change >>in-stage.txt
    git add in-stage.txt
    echo change >>both.txt
    git add both.txt
    echo more >>both.txt
    git rm -q removed.txt
    rm removed-wt.txt
    git mv old-name.txt new-name.txt
    git mv "$(printf 'old-\351.txt')" "$(printf 'new-\351.txt')"
    echo change >>'with space.txt'
    rm kind.txt
    ln -s modified.txt kind.txt
    chmod +x executable.sh
    echo added >added.txt
    git add added.txt
    echo intent >intent.txt
    git add -N intent.txt
    mkdir -p folder/inner
    echo a >folder/inner/deep.txt
    echo b >"$(printf 'non-utf8-\377.txt')"
    printf 'name\nwith newline' >"$(printf 'a\nbreak.txt')"
    # Submodule with a modified file and an untracked one.
    echo change >>module/sub.txt
    echo x >module/loose.txt

    status >"$out/status-mixed.bin"

)

# --- Upstream gone: configured but without the remote ref, so no `branch.ab`. ----------
git init -q -b main gone
(
    cd gone
    git commit -q --allow-empty -m 'Base'
    git config remote.origin.url ../origin.git
    git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    git config branch.main.remote origin
    git config branch.main.merge refs/heads/main
    status >"$out/status-upstream-gone.bin"
)

# --- Ignored and copies. -----------------------------------------------------------------
git init -q -b main ignored
(
    cd ignored
    printf '*.log\nbuild/\n' >.gitignore
    printf 'a line long enough to be recognized as a copy\n' >originale.txt
    git add .
    git commit -q -m 'Base'
    echo log >debug.log
    mkdir build
    echo o >build/out.o
    echo loose >loose.txt
    status --ignored >"$out/status-ignored.bin"
    cp originale.txt copy.txt
    echo change >>originale.txt
    git add .
    git -c status.renames=copies status --porcelain=v2 -z --branch --untracked-files=all \
        >"$out/status-copies.bin"
)

# --- Merge conflicts on main without upstream. --------------------------------------------
git init -q -b main conflict
(
    cd conflict
    for f in both-modified.txt removed-by-us.txt removed-by-them.txt; do
        printf 'base di %s\n' "$f" >"$f"
    done
    git add .
    git commit -q -m 'Base'
    git checkout -q -b side
    echo side >both-modified.txt
    echo side >removed-by-us.txt
    git rm -q removed-by-them.txt
    echo side >both-added.txt
    git add .
    git commit -q -m 'Side'
    git checkout -q main
    echo main >both-modified.txt
    git rm -q removed-by-us.txt
    echo main >removed-by-them.txt
    echo main >both-added.txt
    git add .
    git commit -q -m 'Main'
    git merge -q side >/dev/null 2>&1 || true
    echo clean >clean.txt
    status >"$out/status-conflict.bin"
)

# --- Detached HEAD with a modified file. -------------------------------------------------
git init -q -b main detached
(
    cd detached
    echo one >file.txt
    git add file.txt
    git commit -q -m 'One'
    echo two >file.txt
    git commit -q -am 'Two'
    git checkout -q --detach HEAD~1
    echo change >file.txt
    status >"$out/status-detached.bin"
)
