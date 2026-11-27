---
name: github-release
description: Prepare and publish a Memory MCP release with a version bump, release-mode CI qualification, annotated tag, reviewed draft, explicit Release dispatch and verification of published assets and image. Use for authorized release preparation or publication; respect requests limited to notes, audits or CI changes.
---

# Release this project

One sequence: **bump → commit/push master → release-mode CI → annotated tag/push → reviewed draft → dispatch Release → verify published assets and image.** The preferred path reuses the exact tested bytes. Read [the CI/release runbook](../../../docs/operations/CI.md) for cache, publication and recovery contracts. A request to inspect or modify CI does not authorize publishing a release; respect explicit no-push constraints.

## Why the shape is what it is

- **The version has one owner.** It lives in `crates/memory-mcp/Cargo.toml` (`version = "X.Y.Z"`), not the workspace manifest. `Cargo.lock` mirrors it under `name = "memory_mcp"`, and a correct bump moves exactly one line there — no more.
- **The tag must be annotated.** This is the project convention. `Release` is manually dispatched on the exact tag and checks that its version, SHA and qualified artifacts agree. A tag push or manual draft publication does not trigger the workflow.
- **The notes are the release body, and they are for a reader deciding whether to upgrade.** Brief bullets beat prose: what changed for a user, what breaks, what behaves differently, what was fixed.
- **CI gates publication.** Release-mode CI validates every platform and seals binary/image provenance. The publisher downloads and verifies assets before publishing the draft, then promotes stable latest monotonically. Local checks do not establish hosted completion.

## GitHub goes through `gh`, git goes through `git`

Every GitHub-facing step uses the `gh` CLI. It already holds the credentials, resolves the repository's slug, and speaks the API natively — so no step hardcodes `like-a-freedom/memory_mcp` or re-implements auth, and none reaches for `curl https://api.github.com/…`. `git` stays for the three operations it owns — `commit`, `tag`, `push` — which `gh` cannot perform. Everything else (auth, repo coordinates, the release itself, any API question) is a `gh` command: ask `gh repo view --json nameWithOwner -q .nameWithOwner` for the slug, `gh release …` for the release, `gh api …` for anything the porcelain does not cover.

## Preconditions — check these before touching anything

1. On the release branch (`master`) and the tree is clean apart from the version bump you are about to make. `git status --short` should show nothing but `Cargo.lock` if a build moved it; if it shows other edits, stop and ask — releasing uncommitted work is a mistake.
2. `gh auth status` succeeds (the skill publishes via `gh`).
3. The local preflight passes (below). Do not tag a revision whose gate is red.

## Step 0 — mirror CI locally

`release.yml` shells out to `.github/workflows/ci.yml`. Run what CI runs, so a green local run predicts a green release. Read `ci.yml` and run its steps; at minimum:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked
```

`ci.yml` also runs UI-WASM clippy, the `xtask` toolchain/observability checks and the evaluation gate — run those too when the change could affect them (UI, observability, evals). Report which commands were run; a release whose evidence is "clippy was green" when CI also builds the WASM bundle is not evidence.

## Step 1 — decide the version

Read the current version:

```bash
grep -m1 '^version' crates/memory-mcp/Cargo.toml
```

Choose the next version from the **public surface**, following semver, and say why in one sentence:

- **major** — a breaking change to a wire-visible contract (MCP tool schemas, HTTP API shapes, on-disk schema) or a documented behaviour users depend on.
- **minor** — new capability that is additive; the previous release's clients keep working.
- **patch** — fixes only, no new surface.

When it is a genuine judgement call (a change that is technically breaking but affects nobody in practice), state the call and its reason rather than asking — the pattern in this repo is a minor bump for anything that changes what the server accepts or produces, patch for pure fixes.

## Step 2 — collect and categorise the changes

```bash
git describe --tags --abbrev=0            # previous release tag
git log --oneline <prev-tag>..HEAD
git diff <prev-tag>..HEAD --stat
```

Read the commits and the diff — not just the subjects — and sort each change into the note categories below. A commit subject can undersell or mislabel; the diff is the source of truth for what a user will see.

## Step 3 — bump the version

Edit `crates/memory-mcp/Cargo.toml` so `version = "X.Y.Z"` names the new version, then move the lockfile the way this repo does:

```bash
cargo update -p memory_mcp --workspace
```

Confirm the bump is exactly two files and one lock line:

```bash
git diff --stat -- crates/memory-mcp/Cargo.toml Cargo.lock
git diff -- Cargo.lock
```

If `Cargo.lock` moved more than the single `version = "..."` line under `name = "memory_mcp"`, something else re-resolved — revert the extra churn (`git checkout Cargo.lock` then re-run the update) rather than shipping a lockfile with unrelated moves.

## Step 4 — commit

```bash
git add crates/memory-mcp/Cargo.toml Cargo.lock
git -c user.name="like-a-freedom" \
    -c user.email="like-a-freedom@users.noreply.github.com" \
    commit -F - <<'EOF'
chore: release X.Y.Z
EOF
```

The subject is the whole message, exactly as the repo's own release history does it (`chore: release X.Y.Z`, no body). Commits land under the project's GitHub identity `like-a-freedom <like-a-freedom@users.noreply.github.com>`, not the local `git config` name — pass it per-command as above (or set `user.name`/`user.email` for the repo) so the release commit is attributed correctly. Never add co-author or bot-attribution trailers.

If the release itself reverses or changes a standing decision, the reason belongs in the tag message (Step 5) and the release notes (Step 7), not a commit body — those are what a reader sees.

## Step 5 — qualify the release commit, then annotate the tag

Push the release commit to `master`, dispatch release-mode CI and record the
resulting run ID (select the run for this exact SHA, not merely the newest run):

```bash
git push origin master
gh workflow run ci.yml --ref master -f release=true
gh run list --workflow ci.yml --commit "$(git rev-parse HEAD)" --event workflow_dispatch
# Select the dispatched run ID, then wait for successful completion:
gh run watch <candidate-run-id> --exit-status
```

Require successful completion and the `qualified-binaries` and `candidate-image`
artifacts. An ordinary development CI run is not a release candidate. Artifacts
expire after seven days; if unavailable, omit the candidate ID and let Release
rebuild and qualify the exact tag.

### Annotated tag

The tag message is `vX.Y.Z — <short title>` followed by a blank line and a short paragraph naming the release:

```bash
git tag -a vX.Y.Z -m "vX.Y.Z — <short title>

<one short paragraph: what this release is and why it matters>"
```

Annotated (`-a`), not lightweight: an annotated tag carries the message and is what the existing releases use (`git cat-file -t v1.27.0` → `tag`). The tag name follows the repo's current convention `vX.Y.Z` — recent releases have no dot after the `v` (`v1.27.0`), older ones did (`v.1.9.0`); use the current form.

## Step 6 — push the qualified tag

```bash
git push origin vX.Y.Z
```

The master commit was pushed and qualified in Step 5. **Verify the remote tag points at that exact commit**, reading it through `gh`:

```bash
slug=$(gh repo view --json nameWithOwner -q .nameWithOwner)
gh api "repos/$slug/git/ref/tags/vX.Y.Z" --jq '.object.type, .object.sha'
```

An annotated tag reports `type = tag` and the Sha of the *tag object*, not the commit — dereference it once more, and the result must equal `git rev-parse HEAD`:

```bash
gh api "repos/$slug/git/tags/$(gh api "repos/$slug/git/ref/tags/vX.Y.Z" --jq .object.sha)" --jq .object.sha
```

(`git ls-remote origin 'refs/tags/vX.Y.Z^{}'` is the same lookup in a single call, if you prefer it — but the `gh` form is what this skill reaches for, and it works the same from a host with no git ref cache.)

If it does not match, the tag was created on the wrong commit; delete it locally and remotely (`git push origin :refs/tags/vX.Y.Z`), re-tag, and re-push before publishing the release.

## Step 7 — create a draft and dispatch publication

Write brief English notes to a file, create a reviewed draft, then dispatch the
publisher on the existing tag:

```bash
gh release create vX.Y.Z --draft --verify-tag \
  --title "vX.Y.Z" --notes-file <path-to-notes.md>
gh workflow run release.yml --ref vX.Y.Z \
  -f tag=vX.Y.Z -f candidate_run_id=<candidate-run-id>
```

Do not publish the draft manually. `Release` attaches qualified bytes, reads
them back and verifies checksums before publishing; only then can it advance
stable Docker/GitHub latest. Prereleases and older versions do not advance the
stable channel. Version/SHA image aliases cannot be replaced with different
bytes.

## Step 8 — verify hosted completion

Find the exact dispatched Release run and wait for it:

```bash
gh run list --workflow release.yml --commit "$(git rev-parse HEAD)" --event workflow_dispatch
gh run watch <release-run-id> --exit-status
gh release view vX.Y.Z --json tagName,isDraft,isPrerelease,body,assets
```

Require a successful workflow, a public release and all platform archives and
checksum sidecars. Verify the registry version tag's OCI version/revision and
manifest digest; for a newly highest stable version, verify both latest markers.
Report actual hosted completion, not just local gates or draft existence.

Rerun failed jobs of the original Release run to finish its partial publication,
even if the draft already became public. This preserves the same artifacts. For
a new recovery dispatch, use the same tag, the original master CI candidate run
ID and `-f allow_published_recovery=true`. A tag-build Release run ID is not a
master CI candidate; for that path prefer rerunning the original failed jobs. Recovery permits missing assets but refuses
to change existing public bytes. Do not rebuild or overwrite an already public
release to make a mismatch pass; investigate the provenance conflict.

## Release notes format

**Language: English, always** — even when the working conversation is Russian. The release is read by GitHub visitors, not by us.

**Shape: short themed groups of bullets.** The categories, in this order, **omitting any group with nothing in it** (an empty heading is noise):

```markdown
<optional single framing line: the blast radius in one sentence>

**Breaking changes**
- …

**Customer-facing changes**
- …

**Behaviour changes**
- …

**Deprecations**
- …

**Fixed issues**
- …

**Other**
- …
```

Rules that make the notes useful:

- **Brevity is the point.** One line per item; no preamble, no "in this release we…". A reader scans for the item that names their problem.
- **Lead with the blast radius when it is non-trivial.** A single line such as "No API, tool-surface or configuration changes — upgrade with no operator action" tells the reader whether they need to read further. Omit it when the answer is obvious from the bullets.
- **Symptom before mechanism.** For a fix, name what the user saw (and the status/error if it was externally visible), then what changed. "A valid ID-token algorithm could fail sign-in with a 503; the allow-lists had drifted" beats "refactored the algorithm list".
- **A behaviour change and a breaking change are different claims.** A behaviour change is what now happens differently; a breaking change is something a caller must react to. Put each where it belongs; do not file behaviour under Fixed.
- **Be honest about what is unsupported.** A case that is deliberately still not handled is a note, not an omission.
- **Only what ships.** Internal refactors that change nothing a user can observe do not get a bullet ("Other" is for the valuable ones — an operator-visible metric, a new log field, a documented limit).

The notes equal the release body, so emit them in the reply inside a fenced ```markdown block as well — the clickable record and the text the user can copy should be the same bytes.

See `references/notes-examples.md` for concrete good/bad examples drawn from this repo's history.

## Pitfalls

- **Don't create the GitHub Release before the tag is pushed.** `--verify-tag` exists to stop this; without the tag on the remote the release has nothing to point at and the workflow's `ref` job fails.
- **Don't `--generate-notes`.** GitHub's generated changelog adds a "## What's Changed" commit/PR list that buries the brief bullets. Write the notes; if you want a compare link as a footer, build it from the slug `gh` gives you rather than a hardcoded URL: `**Full Changelog**: https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/compare/<prev>...vX.Y.Z`.
- **Don't tag `master` if the release commit is not `HEAD`.** Tag exactly the release commit (`git rev-parse HEAD` after Step 4).
- **Don't bump the workspace manifest.** The version is owned by `crates/memory-mcp/Cargo.toml`; changing root `Cargo.toml` does nothing and a drifting duplicate is a future bug.
- **Don't leave the tree dirty.** After Step 4, `git status --short` should be clean; anything else (a stray `target/` note, an editor file) is not part of the release.
