#!/usr/bin/env bash
# Ensure the GitHub Release for a pending cargo-dist release is a draft.
#
# Usage:
#   ensure-draft-release.sh <tag> <plan.json>
#   ensure-draft-release.sh --self-test
#
# The script creates the draft Release when it does not exist yet, accepts an
# existing draft, and refuses to touch an already published Release. It runs as
# a release-extras step before assets are attached and verified (#41).
#
# Requires GITHUB_REPOSITORY; GH_BIN overrides the gh executable for tests.
set -euo pipefail

GH_BIN="${GH_BIN:-gh}"

usage() {
  echo "usage: $0 <tag> <plan.json> | --self-test" >&2
}

ensure_draft_release() {
  local tag="$1" plan_file="$2"
  local repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"
  local is_draft title body notes

  if "$GH_BIN" release view "$tag" --repo "$repo" >/dev/null 2>&1; then
    is_draft="$("$GH_BIN" release view "$tag" --repo "$repo" --json isDraft --jq '.isDraft')"
    if [[ "$is_draft" != "true" ]]; then
      echo "release $tag already exists and is not a draft; refusing to touch it" >&2
      return 1
    fi
    echo "draft release $tag already exists; continuing."
    return 0
  fi

  title="$(jq -r '.announcement_title // empty' "$plan_file")"
  body="$(jq -r '.announcement_github_body // empty' "$plan_file")"
  if [[ -z "$title" || -z "$body" ]]; then
    echo "the dist plan is missing announcement_title or announcement_github_body" >&2
    return 1
  fi

  notes="$(mktemp)"
  printf '%s' "$body" > "$notes"
  if ! "$GH_BIN" release create "$tag" --repo "$repo" --draft --verify-tag \
    --title "$title" --notes-file "$notes"; then
    rm -f "$notes"
    return 1
  fi
  rm -f "$notes"
  echo "created draft release $tag."
}

self_test() {
  local tmp fake_gh plan_complete plan_incomplete
  tmp="$(mktemp -d)"
  # Expand tmp now: the local binding is gone when the EXIT trap runs.
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  fake_gh="$tmp/gh"
  plan_complete="$tmp/plan-complete.json"
  plan_incomplete="$tmp/plan-incomplete.json"

  cat > "$fake_gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$FAKE_GH_LOG"
if [[ "${1:-}" == "release" && "${2:-}" == "view" ]]; then
  case "${FAKE_GH_MODE:?}" in
    absent)
      exit 1
      ;;
    draft)
      if [[ "$*" == *"--json isDraft"* ]]; then
        echo true
      fi
      exit 0
      ;;
    published)
      if [[ "$*" == *"--json isDraft"* ]]; then
        echo false
      fi
      exit 0
      ;;
    *)
      exit 2
      ;;
  esac
fi
exit 0
FAKE_GH
  chmod +x "$fake_gh"

  cat > "$plan_complete" <<'JSON'
{
  "announcement_title": "v0.0.0",
  "announcement_github_body": "Release notes."
}
JSON
  cat > "$plan_incomplete" <<'JSON'
{
  "announcement_title": "v0.0.0"
}
JSON

  # 1. A missing Release is created as a draft with the plan's title/body.
  : > "$tmp/log"
  if ! GH_BIN="$fake_gh" GITHUB_REPOSITORY="owner/repo" FAKE_GH_MODE=absent FAKE_GH_LOG="$tmp/log" \
    ensure_draft_release v0.0.0 "$plan_complete"; then
    echo "self-test: a missing draft release must be created" >&2
    return 1
  fi
  if ! grep -q 'release create v0.0.0 --repo owner/repo --draft --verify-tag --title v0.0.0' "$tmp/log"; then
    echo "self-test: the create command must target a draft release" >&2
    return 1
  fi

  # 2. An existing draft is accepted without recreating it.
  : > "$tmp/log"
  if ! GH_BIN="$fake_gh" GITHUB_REPOSITORY="owner/repo" FAKE_GH_MODE=draft FAKE_GH_LOG="$tmp/log" \
    ensure_draft_release v0.0.0 "$plan_complete"; then
    echo "self-test: an existing draft release must be accepted" >&2
    return 1
  fi
  if grep -q 'release create' "$tmp/log"; then
    echo "self-test: an existing draft release must not be recreated" >&2
    return 1
  fi

  # 3. A published Release is refused.
  : > "$tmp/log"
  if GH_BIN="$fake_gh" GITHUB_REPOSITORY="owner/repo" FAKE_GH_MODE=published FAKE_GH_LOG="$tmp/log" \
    ensure_draft_release v0.0.0 "$plan_complete"; then
    echo "self-test: a published release must be refused" >&2
    return 1
  fi
  if grep -q 'release create' "$tmp/log"; then
    echo "self-test: a published release must not be recreated" >&2
    return 1
  fi

  # 4. A plan without announcement fields is refused before any create call.
  : > "$tmp/log"
  if GH_BIN="$fake_gh" GITHUB_REPOSITORY="owner/repo" FAKE_GH_MODE=absent FAKE_GH_LOG="$tmp/log" \
    ensure_draft_release v0.0.0 "$plan_incomplete"; then
    echo "self-test: a plan without announcement fields must be refused" >&2
    return 1
  fi
  if grep -q 'release create' "$tmp/log"; then
    echo "self-test: a plan without announcement fields must not create a release" >&2
    return 1
  fi

  echo "self-test passed"
}

case "${1:-}" in
  --self-test)
    self_test
    ;;
  "")
    usage
    exit 2
    ;;
  *)
    if [[ $# -ne 2 ]]; then
      usage
      exit 2
    fi
    ensure_draft_release "$1" "$2"
    ;;
esac
