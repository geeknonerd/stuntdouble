#!/usr/bin/env bash
# Assert that a GitHub Release carries every asset the dist manifest declares,
# plus the assets the release-extras job owns.
#
# Usage:
#   check-release-assets.sh <plan.json> <actual-assets.txt> <version>
#   check-release-assets.sh --self-test
#
# <plan.json> is the cargo-dist manifest for the release (the `plan` input of
# the Release extras workflow). <actual-assets.txt> lists the release's asset
# names, one per line:
#   gh release view <tag> --json assets --jq '.assets[].name'
set -euo pipefail

usage() {
  echo "usage: $0 <plan.json> <actual-assets.txt> <version> | --self-test" >&2
}

required_assets() {
  local plan_file="$1" version="$2"
  if [[ "$(jq -r '.artifacts | length' "$plan_file")" -eq 0 ]]; then
    echo "the dist manifest declares no artifacts to verify" >&2
    return 1
  fi
  # Every artifact the manifest declares must be attached, so a new
  # apiVersion type definition or artifact class needs no edit here.
  jq -r '.artifacts | keys[]' "$plan_file"
  # Assets owned by the release-extras job itself.
  printf '%s\n' \
    "SHA256SUMS" \
    "stuntdouble-${version}.cdx.json" \
    "stuntdouble-${version}-image.txt"
}

check() {
  local plan_file="$1" actual_file="$2" version="$3"
  local required missing=0 asset
  if ! required="$(required_assets "$plan_file" "$version")"; then
    return 1
  fi
  while IFS= read -r asset; do
    [[ -n "$asset" ]] || continue
    if ! grep -Fxq -- "$asset" "$actual_file"; then
      echo "missing release asset: $asset" >&2
      missing=1
    fi
  done <<<"$required"
  return "$missing"
}

self_test() {
  local tmp plan actual
  tmp="$(mktemp -d)"
  plan="$tmp/plan.json"
  actual="$tmp/actual.txt"

  cat > "$plan" <<'JSON'
{
  "artifacts": {
    "ctx-api-v1.d.ts": {},
    "ctx-api-v2.d.ts": {},
    "stuntdouble-x86_64-unknown-linux-gnu.tar.gz": {}
  }
}
JSON

  cat > "$actual" <<'EOF'
ctx-api-v1.d.ts
ctx-api-v2.d.ts
stuntdouble-x86_64-unknown-linux-gnu.tar.gz
SHA256SUMS
stuntdouble-0.5.2.cdx.json
stuntdouble-0.5.2-image.txt
EOF

  if ! check "$plan" "$actual" 0.5.2; then
    echo "self-test: a complete asset set must pass" >&2
    rm -rf "$tmp"
    return 1
  fi

  # The newly declared ctx-api-v2.d.ts is missing from the release.
  cat > "$actual" <<'EOF'
ctx-api-v1.d.ts
stuntdouble-x86_64-unknown-linux-gnu.tar.gz
SHA256SUMS
stuntdouble-0.5.2.cdx.json
stuntdouble-0.5.2-image.txt
EOF

  if check "$plan" "$actual" 0.5.2; then
    echo "self-test: a declared artifact missing from the release must fail" >&2
    rm -rf "$tmp"
    return 1
  fi

  rm -rf "$tmp"
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
    if [[ $# -ne 3 ]]; then
      usage
      exit 2
    fi
    check "$1" "$2" "$3"
    ;;
esac
