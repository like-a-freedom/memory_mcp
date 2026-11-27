#!/usr/bin/env bash
# Publication glue; correctness policies and artifact verification live in xtask.
set -euo pipefail
XTASK="$1"
DIST="$2"
IMAGE_DIR="$3"
: "${TAG:?}" "${VERSION:?}" "${GITHUB_SHA:?}" "${ARTIFACT_RUN_ID:?}"
: "${GITHUB_RUN_ID:?}" "${GITHUB_RUN_ATTEMPT:?}" "${GITHUB_REPOSITORY:?}" "${RUNNER_TEMP:?}"
export GH_REPO="$GITHUB_REPOSITORY"
fail() { printf '%s\n' "$*" >&2; exit 1; }
test "$TAG" = "v$VERSION" || fail 'Tag and qualified version differ'
WORK="$(mktemp -d "$RUNNER_TEMP/publication.XXXXXXXX")"
LEGACY_CONTAINER="memory-release-version-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
cleanup() {
  if test "${LEGACY_STARTED:-false}" = true; then docker rm -f "$LEGACY_CONTAINER" >/dev/null 2>&1 || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# Verify the downloaded bytes before any registry or release mutation.
"$XTASK" release-bundle verify "$DIST" --source-sha "$GITHUB_SHA" --run-id "$ARTIFACT_RUN_ID" --version "$VERSION"
"$XTASK" image-handoff verify "$IMAGE_DIR" --source-sha "$GITHUB_SHA" --run-id "$ARTIFACT_RUN_ID"
docker load --input "$IMAGE_DIR/image.tar"
IMAGE_ID="$(jq -er .image_id "$IMAGE_DIR/image.json")"
test "$(docker image inspect --format '{{.Id}}' memory-mcp-http:local)" = "$IMAGE_ID" || fail 'Loaded image differs from qualified image'
test "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.version"}}' "$IMAGE_ID")" = "$VERSION" || fail 'Candidate OCI version differs from qualified version'
test "$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$IMAGE_ID")" = "$GITHUB_SHA" || fail 'Candidate OCI revision differs from qualified commit'
IMAGE="ghcr.io/$(printf '%s' "$GITHUB_REPOSITORY" | tr '[:upper:]' '[:lower:]')"

# Only a definite missing manifest is absence. Network/auth failures must stop.
inspect_or_absent() {
  local ref="$1" result
  if result="$(docker buildx imagetools inspect "$ref" --format '{{.Manifest.Digest}}' 2>"$WORK/inspect.error")"; then
    [[ "$result" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'Registry returned an invalid image digest'
    printf '%s' "$result"
  elif ! grep -Fxq "ERROR: $ref: not found" "$WORK/inspect.error"; then
    cat "$WORK/inspect.error" >&2
    fail "Cannot determine registry state: $ref"
  fi
}
channel_decision() {
  CURRENT_DIGEST="$(inspect_or_absent "$IMAGE:latest")"
  CURRENT_TAG=''
  local version='' output
  set --
  if test -n "$CURRENT_DIGEST"; then
    docker buildx imagetools inspect "$IMAGE@$CURRENT_DIGEST" --format '{{json .Image}}' > "$WORK/current-image.json"
    version="$(jq -r '.config.Labels["org.opencontainers.image.version"] // empty' "$WORK/current-image.json")"
    if test -z "$version"; then
      # Old published images have no OCI version. Read their own CLI by exact
      # digest, with no credentials/mounts/network and a bounded lifetime.
      docker pull "$IMAGE@$CURRENT_DIGEST" >&2
      LEGACY_STARTED=true
      output="$(timeout 60 docker run --rm --name "$LEGACY_CONTAINER" --network none --read-only \
        --cap-drop ALL --security-opt no-new-privileges --pids-limit 32 --memory 128m \
        --entrypoint /usr/local/bin/memory_mcp "$IMAGE@$CURRENT_DIGEST" --version)"
      LEGACY_STARTED=false
      [[ "$output" =~ ^memory_mcp[[:space:]]([^[:space:]]+)$ ]] || fail 'Cannot identify the current stable image version'
      version="${BASH_REMATCH[1]}"
    fi
    CURRENT_TAG="v${version#v}"
    set -- --current-tag "$CURRENT_TAG" --current-digest "$CURRENT_DIGEST"
  fi
  "$XTASK" release-channel --candidate-tag "$TAG" --candidate-digest "$CANDIDATE_DIGEST" \
    "$@" --output "$WORK/promote"
  PROMOTE="$(cat "$WORK/promote")"
}
github_channel_decision() {
  set --
  if gh api "repos/$GITHUB_REPOSITORY/releases/latest" > "$WORK/github-latest.json" 2> "$WORK/github.error"; then
    jq -e '.draft == false and .prerelease == false' "$WORK/github-latest.json" > /dev/null || fail 'GitHub latest is not a stable published release'
    # This channel compares versions only; image identity is checked separately.
    set -- --current-tag "$(jq -er .tag_name "$WORK/github-latest.json")" --current-digest "$CANDIDATE_DIGEST"
  elif ! grep -Fxq 'gh: Not Found (HTTP 404)' "$WORK/github.error"; then
    cat "$WORK/github.error" >&2
    fail 'Cannot determine GitHub latest release'
  fi
  "$XTASK" release-channel --candidate-tag "$TAG" --candidate-digest "$CANDIDATE_DIGEST" \
    "$@" --output "$WORK/github-promote"
  GITHUB_PROMOTE="$(cat "$WORK/github-promote")"
}
release_state() {
  gh release view "$TAG" --json tagName,isDraft,isPrerelease,body,assets > "$WORK/release.json"
  test "$(jq -r .tagName "$WORK/release.json")" = "$TAG" || fail 'Release tag changed'
  test "$(jq -r '.body | length > 0' "$WORK/release.json")" = true || fail 'Reviewed release notes are required'
}
release_state
IS_DRAFT="$(jq -r .isDraft "$WORK/release.json")"
if test "$IS_DRAFT" != true && test "${ALLOW_PUBLISHED_RECOVERY:-false}" != true; then
  fail 'Published release recovery requires allow_published_recovery=true'
fi

# Stage qualified bytes under a run-specific reference to learn the registry
# manifest digest. Never overwrite an existing version or SHA alias with new bytes.
CANDIDATE="$IMAGE:candidate-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
# Refuse an unavailable registry before creating even a candidate reference.
inspect_or_absent "$IMAGE:latest" > /dev/null
docker tag "$IMAGE_ID" "$CANDIDATE"
docker push "$CANDIDATE"
CANDIDATE_DIGEST="$(inspect_or_absent "$CANDIDATE")"
test -n "$CANDIDATE_DIGEST" || fail 'Pushed candidate is absent from registry'
# Validate version and current-channel policy before publishing any release.
channel_decision
github_channel_decision
for alias in "${TAG//+/_}" "sha-${GITHUB_SHA:0:7}"; do
  existing="$(inspect_or_absent "$IMAGE:$alias")"
  if test -n "$existing"; then
    test "$existing" = "$CANDIDATE_DIGEST" || fail "Immutable image alias conflicts: $alias"
  else
    docker tag "$IMAGE_ID" "$IMAGE:$alias"
    docker push "$IMAGE:$alias"
    test "$(inspect_or_absent "$IMAGE:$alias")" = "$CANDIDATE_DIGEST" || fail "Image readback differs: $alias"
  fi
done

if test "$IS_DRAFT" = true; then
  gh release upload "$TAG" "$DIST"/memory_mcp* --clobber
else
  # Recovery can fill missing assets, but never replaces already public bytes.
  mkdir "$WORK/existing"
  if test "$(jq '.assets | length' "$WORK/release.json")" -gt 0; then
    gh release download "$TAG" --dir "$WORK/existing"
  fi
  for asset in "$WORK/existing"/*; do
    test -e "$asset" || continue
    name="$(basename "$asset")"
    test -f "$DIST/$name" && test "$name" != release.json || fail "Unexpected public asset: $name"
    cmp -s "$asset" "$DIST/$name" || fail "Public asset bytes differ: $name"
  done
  for asset in "$DIST"/memory_mcp*; do
    if ! test -e "$WORK/existing/$(basename "$asset")"; then gh release upload "$TAG" "$asset"; fi
  done
fi
mkdir "$WORK/download"
gh release download "$TAG" --dir "$WORK/download"
test ! -e "$WORK/download/release.json" || fail 'Unexpected public qualification receipt'
cp "$DIST/release.json" "$WORK/download/release.json"
"$XTASK" release-bundle verify "$WORK/download" --source-sha "$GITHUB_SHA" --run-id "$ARTIFACT_RUN_ID" --version "$VERSION"
PRERELEASE=false
if [[ "${VERSION%%+*}" = *-* ]]; then PRERELEASE=true; fi
if test "$IS_DRAFT" = true; then
  gh release edit "$TAG" --draft=false --prerelease="$PRERELEASE" --latest=false
else
  # Do not clear a public release's existing Latest marker during recovery.
  gh release edit "$TAG" --draft=false --prerelease="$PRERELEASE"
fi
release_state
test "$(jq -r .isDraft "$WORK/release.json")" = false || fail 'Release is still a draft'
test "$(jq -r .isPrerelease "$WORK/release.json")" = "$PRERELEASE" || fail 'Release prerelease status differs'

# Re-read inside the serialized publication job: a stale run must not roll the
# stable channel back. External publishers must respect this same lock/policy.
channel_decision
github_channel_decision
if test "$PROMOTE" = true; then
  docker tag "$IMAGE_ID" "$IMAGE:latest"
  docker push "$IMAGE:latest"
  test "$(inspect_or_absent "$IMAGE:latest")" = "$CANDIDATE_DIGEST" || fail 'Latest image readback differs'
fi
if test "$GITHUB_PROMOTE" = true; then
  gh release edit "$TAG" --latest=true
  gh api "repos/$GITHUB_REPOSITORY/releases/latest" > "$WORK/github-published.json"
  jq -e --arg tag "$TAG" '.tag_name == $tag and .draft == false and .prerelease == false' \
    "$WORK/github-published.json" > /dev/null || fail 'GitHub latest readback differs'
fi
