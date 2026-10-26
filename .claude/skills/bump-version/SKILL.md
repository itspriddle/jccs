---
name: bump-version
description: 'Cut a jccs release end to end: bump the version in Cargo.toml and Cargo.lock, open a PR and watch CI, merge it, create and push a signed annotated `vX.Y.Z` tag, watch the Release workflow, then watch the Homebrew tap''s bump-formula run and verify the formula. Use whenever the user says "bump the version", "bump to 0.7.0", "cut a release", "release a patch/minor/major", "ship vX.Y.Z", or runs /bump-version, even if they only name the version.'
disable-model-invocation: true
---

# Bump version

Releases a new version of jccs. Every step after the PR is opened is outward-facing (merge to `main`, a pushed tag, a GitHub release, a commit to the Homebrew tap), and a pushed tag triggers a public release. So the rule throughout is: **if anything fails, stop and report. Never merge red CI, never tag an unmerged commit, never retry a release by deleting or moving a tag** without the user asking.

## 1. Decide the version

The argument may be an exact version (`0.7.0` or `v0.7.0`) or a bump level (`patch`, `minor`, `major`). Read the current version from `Cargo.toml` (`grep -m1 '^version' Cargo.toml`) and compute the new one. If no argument was given, ask which level to bump; don't guess.

Below, `VER` is the bare version (`0.7.0`), `TAG` is `v$VER`, and `SLUG` is `VER` with the dots removed (`070`).

Sanity checks before touching anything:

- The working tree is clean and you're on `main`, up to date with `origin/main` (`git fetch origin && git status -sb`).
- `TAG` doesn't already exist locally or on the remote (`git tag -l "$TAG"`, `git ls-remote --tags origin "$TAG"`).
- `VER` is greater than the current version.

## 2. Bump and open the PR

```sh
git switch -c "feature/bump-version-$SLUG"
```

Edit the `version = "…"` line in the `[package]` section of `Cargo.toml` (only that line; dependency versions must not change). Then sync the lockfile without touching dependencies:

```sh
cargo update -p jccs --offline
git diff --stat   # expect exactly Cargo.toml and Cargo.lock, 1 line each
make check
```

If the diff touches anything other than the jccs version lines, stop and investigate. Commit and push:

```sh
git commit -am "chore: bump to $TAG"
git push -u origin HEAD
gh pr create --base main --title "chore: bump to $TAG" --body ""
```

## 3. Watch CI and merge

Checks can take a few seconds to register after the PR is opened, and `gh pr checks` errors with "no checks reported" until then. Retry a few times before watching:

```sh
gh pr checks <PR> --watch --fail-fast
```

If any check fails, stop. Show the failing check and its log (`gh run view <run-id> --log-failed`) and don't merge.

When everything passes, merge with a merge commit (the repo's convention) and return to an up-to-date `main`:

```sh
gh pr merge <PR> --merge --delete-branch
git switch main && git pull --ff-only
```

Confirm `HEAD` is the merge commit of the PR (`gh pr view <PR> --json mergeCommit -q .mergeCommit.oid` matches `git rev-parse HEAD`) and that `Cargo.toml` on `main` has `VER`.

## 4. Tag and push

The user has `tag.gpgsign` set, so tags must be annotated. Pass `-m` so git doesn't open an editor:

```sh
git tag -m "$TAG" "$TAG"
git push origin "$TAG"
```

The tag goes on the merge commit on `main`, matching earlier releases.

## 5. Watch the Release workflow

Pushing the tag triggers `.github/workflows/release.yml` (build matrix, then publish release, then bump-formula dispatch). Find the run. It may take a few seconds to appear, so retry briefly:

```sh
gh run list --workflow release.yml --branch "$TAG" -L 1 --json databaseId,status,url
gh run watch <run-id> --exit-status
```

On failure, stop and show `gh run view <run-id> --log-failed`. Don't delete the tag or the release.

On success, confirm the release exists with all assets: `gh release view "$TAG" --json assets -q '.assets[].name'` should list a `.tar.gz` and a `.sha256` for each of `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, and `aarch64-unknown-linux-musl`.

## 6. Watch the Homebrew tap

The `bump-formula` job dispatches `bump-formula.yml` in `itspriddle/homebrew-brews`, which commits the updated formula straight to `master`. Dispatched runs are all titled "Bump formula" and don't carry the version, so find the right one by time: take the first `workflow_dispatch` run created at or after the `bump-formula` job's start time.

```sh
gh run view <release-run-id> --json jobs -q '.jobs[] | select(.name=="Bump Homebrew formula") | .startedAt'
gh run list --repo itspriddle/homebrew-brews --workflow bump-formula.yml --event workflow_dispatch -L 5 \
  --json databaseId,createdAt,status,conclusion,url
gh run watch <tap-run-id> --repo itspriddle/homebrew-brews --exit-status
```

Then verify the formula on `master` actually has the new version:

```sh
gh api repos/itspriddle/homebrew-brews/contents/Formula/jccs.rb -q .content | base64 -d | grep -m1 'version "'
```

## 7. Report

Finish with a short summary: the PR link, the tag, the release URL (`gh release view "$TAG" --json url -q .url`), the tap run URL, and the formula version. If anything stopped early, say exactly which step and what's left to do by hand.
