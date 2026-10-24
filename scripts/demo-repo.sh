#!/usr/bin/env bash
# Creates a demo repository for exploring the desktop app (`make demo-repo`, then
# `make desktop-start REPO=<dir>`): a history with a merge, a tag, a remote with a branch
# ahead, a stash, the branch `experiment` that conflicts with `main` on `src/lib.rs`, and
# local changes (staged, unstaged and untracked). It replaces the old web demo repo.
#
# usage: scripts/demo-repo.sh <dir>   (the folder must not exist)
set -euo pipefail

dir=${1:?usage: $0 <dir>}
if [ -e "$dir" ]; then
  echo "$dir already exists" >&2
  exit 1
fi

export GIT_AUTHOR_NAME="Ada Lovelace" GIT_AUTHOR_EMAIL="ada@example.com"
export GIT_COMMITTER_NAME="Ada Lovelace" GIT_COMMITTER_EMAIL="ada@example.com"
# Fixed dates one hour apart: the same history every time.
t=1767225600
commit() {
  t=$((t + 3600))
  GIT_AUTHOR_DATE="@$t +0100" GIT_COMMITTER_DATE="@$t +0100" git commit -q "$@"
}

mkdir -p "$dir"
cd "$dir"
git init -q -b main
git config commit.gpgsign false

mkdir -p src docs
printf '# Demo\n\nA small demo repository.\n' > README.md
printf 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n' > src/lib.rs
printf 'fn main() {\n    println!("{}", demo::add(1, 2));\n}\n' > src/main.rs
git add -A
commit -m "Initial commit"

printf '/target\n' > .gitignore
git add .gitignore
commit -m "Ignore target"

git switch -q -c feature/docs
printf '# Usage\n\nRun `cargo run`.\n' > docs/usage.md
git add docs
commit -m "Add usage docs"
printf '\nSee docs/usage.md.\n' >> README.md
commit -am "Link the docs from the README"

git switch -q main
printf 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn sub(a: i32, b: i32) -> i32 {\n    a - b\n}\n' > src/lib.rs
commit -am "Add sub"
t=$((t + 3600))
GIT_AUTHOR_DATE="@$t +0100" GIT_COMMITTER_DATE="@$t +0100" \
  git merge -q --no-ff -m "Merge branch 'feature/docs'" feature/docs
git tag -a v0.1.0 -m "First release"

# `experiment` starts before "Add sub" and changes the same lines of src/lib.rs.
git switch -q -c experiment HEAD~1^1
printf 'pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n' > src/lib.rs
mkdir -p docs
printf '# Layout\n\nOne crate, two files.\n' > docs/layout.md
git add -A
commit -m "Use i64 in add, document the layout"
git switch -q main

# A bare "origin" next to the repo, one commit ahead of the local main.
git clone -q --bare . ../"$(basename "$dir")-origin.git"
git remote add origin ../"$(basename "$dir")-origin.git"
git fetch -q origin
git branch -q -u origin/main
other=$(mktemp -d)
git clone -q ../"$(basename "$dir")-origin.git" "$other/clone"
(
  cd "$other/clone"
  printf '\nLicense: MIT.\n' >> README.md
  commit -am "Mention the license"
  git push -q origin main
)
rm -rf "$other"
git fetch -q origin

# A stash and the local changes.
printf 'fn main() {\n    println!("{}", demo::add(2, 2));\n}\n' > src/main.rs
git stash push -q -m "Try other numbers"
printf '\n## Build\n\n`cargo build`\n' >> README.md
git add README.md
printf '\npub fn mul(a: i32, b: i32) -> i32 {\n    a * b\n}\n' >> src/lib.rs
printf 'notes\n' > notes.txt

echo "demo repo: $PWD"
