---
name: release
description: Cut a BinariGit release — version bump and CHANGELOG on dev, dev into master with a squash merge, annotated v<version> tag that starts package.yml, then check the GitHub Release. Use when the user asks for a release, a new version or a tag.
---

# Release

The rules are in `AGENTS.md` ("Releases"): `dev` goes into `master` with a **squash
merge** (the repository allows only squash; never delete `dev` afterwards), the release is an annotated `v<version>` tag on `master`, and
`.github/workflows/package.yml` builds AppImage, zip and MSI and creates the GitHub Release with
the notes taken from the `## [<version>]` section of `CHANGELOG.md`.

Every step below is outward-facing (PRs, merges, a tag that publishes a Release): the user's
request for a release covers them, but stop and ask if something does not match (CI red,
`master` has commits that `dev` lacks, the version is already tagged).

## 0. Where things stand

```sh
git fetch -q origin --tags
git tag --sort=-creatordate | head -3              # last version
git diff --stat origin/master <last tag>          # empty: master is the last release
prep=$(git log --format=%h -1 --grep='^Release .* preparation' origin/dev)
git log --oneline $prep..origin/dev                # what goes in the release (the tag is on
                                                   # master's squash, not an ancestor of dev)
gh pr list --state merged --base master --limit 1 --json number,title,body   # last release PR
```

Pick the version: 0.x, so a new feature or a visible change bumps the minor (`0.5.0` → `0.6.0`),
fixes only bump the patch. Use the user's version if they gave one.

## 1. Preparation PR on dev (squash)

```sh
git checkout -b release-<version> origin/dev
```

- `Cargo.toml`: `[workspace.package] version = "<version>"`, then `cargo update -w` (Cargo.lock).
- `CHANGELOG.md`:
  - a new `## [<version>] - <YYYY-MM-DD>` right under `## [Unreleased]` (which stays, empty), with
    a one-paragraph summary, then the `### Added` / `### Changed` / `### Fixed` / `### Packages`
    / `### Known limitations` sections of the previous releases (copy their wording style:
    user-visible behavior, command ids in backticks, no internal refactors);
  - check that every user-visible PR since the last tag is there: the `Unreleased` section is
    often incomplete (quality and UI steps are usually missing);
  - `### Packages` names the files with the new version (AppImage + `.zsync`, `.msi`, Windows
    zip, `SHA256SUMS.txt`);
  - the links at the bottom: `[Unreleased]` compares from `v<version>`, plus a new
    `[<version>]: …/compare/v<previous>...v<version>` line.
- `cargo xtask ci --quiet` must be green. Check the exit code itself
  (`cargo xtask ci --quiet > target/ci.log 2>&1; echo $?`), not the output of a `| tail`.
- Commit `Release <version> preparation: version and CHANGELOG`, push, PR to `dev`, wait for
  the checks (`gh pr checks <n> --watch`), squash-merge.

## 2. dev into master (squash)

```sh
gh pr create --base master --head dev --title "Release <version>" --body "…"
gh pr checks <n> --watch
gh pr merge <n> --squash         # squash, and NOT --delete-branch: dev stays
git fetch -q origin && git diff --stat origin/master origin/dev   # must be empty
```

Body as the previous ones: "Brings `dev` into `master` for <version> (squash: one commit
on `master` per release): #a, #b, … After the merge, the `v<version>` tag on
`master` starts `package.yml`." plus the PR attribution line.

## 3. Tag

```sh
git fetch -q origin master
git tag -a v<version> -m "BinariGit <version>" origin/master   # annotated, as the past tags
git push origin v<version>
```

## 4. Check the Release

```sh
gh run list --workflow package.yml --limit 3
gh run watch <id> --exit-status
gh release view v<version> --json name,assets --jq '.name, (.assets[].name)'
```

The assets: `BinariGit-<version>-x86_64.AppImage`, `.AppImage.zsync`,
`BinariGit-<version>-x86_64-windows.zip`, `BinariGit-<version>-x86_64.msi`, `SHA256SUMS.txt`.
If the job fails on "no section for <version> in CHANGELOG.md", the heading is wrong.

Finally update local `dev` (`git checkout dev && git pull --ff-only`) and delete the
`release-<version>` branch, local and remote.

## Push from the devcontainer

If `git push` fails with "could not read Username" (stale VS Code credential helper):

```sh
git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push …
```
