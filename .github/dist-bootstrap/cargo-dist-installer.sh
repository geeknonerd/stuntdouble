#!/bin/sh
# Download and verify the pinned cargo-dist installer.
set -eu

DIST_VERSION="0.33.0"
INSTALLER_SHA256="c5b53ac2559eb26e3b3dc28755eb5d011ed6ec9ac97fa4fc2a2459872e766b5e"

usage() {
  echo "usage: $0 [--check | --self-test]" >&2
}

die() {
  echo "cargo-dist bootstrap: $*" >&2
  exit 1
}

repo_root() {
  if [ -f "$0" ] && [ "$(basename "$0")" = "cargo-dist-installer.sh" ]; then
    script_dir=$(CDPATH='' cd "$(dirname "$0")" && pwd)
    CDPATH='' cd "$script_dir/../.." && pwd
  else
    # cargo-dist invokes this script through `curl ... | sh`, where $0 is
    # just "sh" and the workflow's working directory is the repository root.
    pwd
  fi
}

read_version() {
  config_path=$1
  if [ ! -r "$config_path" ]; then
    die "cannot read $config_path"
  fi
  if ! version=$(awk -F'"' '
    /^[[:space:]]*cargo-dist-version[[:space:]]*=/ {
      count++
      value=$2
    }
    END {
      if (count != 1) {
        exit 1
      }
      print value
    }
  ' "$config_path"); then
    die "$config_path must contain exactly one cargo-dist-version"
  fi
  if [ -z "$version" ]; then
    die "$config_path is missing cargo-dist-version"
  fi
  case "$version" in
    *[!0-9A-Za-z.+-]*)
      die "invalid cargo-dist-version: $version"
      ;;
  esac
  printf '%s\n' "$version"
}

assert_version() {
  config_path=$1
  expected_version=$2
  actual_version=$(read_version "$config_path")
  if [ "$actual_version" != "$expected_version" ]; then
    die "$config_path declares cargo-dist $actual_version, but the bootstrap wrapper pins $expected_version"
  fi
}

validate_sha256() {
  value=$1
  case "$value" in
    "" | *[!0-9a-f]*)
      return 1
      ;;
  esac
  [ "${#value}" -eq 64 ]
}

sha256_file() {
  file_path=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file_path" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file_path" | awk '{ print $1 }'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file_path" | awk '{ print $NF }'
  else
    die "sha256sum, shasum, and openssl are all unavailable"
  fi
}

verify_sha256() {
  file_path=$1
  expected=$2
  actual=$(sha256_file "$file_path")
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 mismatch for $file_path" >&2
    echo "expected: $expected" >&2
    echo "actual:   $actual" >&2
    return 1
  fi
}

self_test() {
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/cargo-dist-bootstrap.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT

  config="$tmp/dist-workspace.toml"
  duplicate_config="$tmp/duplicate-dist-workspace.toml"
  mismatch_config="$tmp/mismatch-dist-workspace.toml"
  fixture="$tmp/installer-fixture"
  known_sha256=b4bde33f55012026d69eb6c73ae6b1b06951bf9558f1b2e5568176adda4090ce

  printf 'cargo-dist-version = "%s"\r\n' "$DIST_VERSION" > "$config"
  if [ "$(read_version "$config")" != "$DIST_VERSION" ]; then
    die "self-test: cargo-dist-version parsing failed"
  fi
  assert_version "$config" "$DIST_VERSION"

  printf '%s\n%s\n' \
    'cargo-dist-version = "0.33.0"' \
    'cargo-dist-version = "0.34.0"' > "$duplicate_config"
  if (read_version "$duplicate_config") >/dev/null 2>&1; then
    die "self-test: duplicate cargo-dist-version was accepted"
  fi

  printf '%s\n' 'cargo-dist-version = "9.8.7"' > "$mismatch_config"
  if (assert_version "$mismatch_config" "$DIST_VERSION") >/dev/null 2>&1; then
    die "self-test: bootstrap version mismatch was accepted"
  fi

  if ! validate_sha256 "$INSTALLER_SHA256"; then
    die "self-test: embedded installer SHA-256 is invalid"
  fi

  printf 'cargo-dist-bootstrap\n' > "$fixture"
  if [ "$(sha256_file "$fixture")" != "$known_sha256" ]; then
    die "self-test: SHA-256 calculation failed"
  fi
  if ! verify_sha256 "$fixture" "$known_sha256"; then
    die "self-test: matching SHA-256 was rejected"
  fi
  if verify_sha256 "$fixture" "0000000000000000000000000000000000000000000000000000000000000000" 2>/dev/null; then
    die "self-test: mismatched SHA-256 was accepted"
  fi

  echo "self-test passed"
}

check_only=0
case "${1:-}" in
  "")
    [ "$#" -eq 0 ] || {
      usage
      exit 2
    }
    ;;
  --check)
    [ "$#" -eq 1 ] || {
      usage
      exit 2
    }
    check_only=1
    ;;
  --self-test)
    [ "$#" -eq 1 ] || {
      usage
      exit 2
    }
    self_test
    exit 0
    ;;
  *)
    usage
    exit 2
    ;;
esac

root=$(repo_root)
config_path="$root/dist-workspace.toml"
assert_version "$config_path" "$DIST_VERSION"
if ! validate_sha256 "$INSTALLER_SHA256"; then
  die "invalid embedded shell installer SHA-256 for cargo-dist $DIST_VERSION"
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/cargo-dist-bootstrap.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
installer_path="$tmp/cargo-dist-installer.sh"
installer_url="https://github.com/axodotdev/cargo-dist/releases/download/v${DIST_VERSION}/cargo-dist-installer.sh"

echo "Downloading cargo-dist $DIST_VERSION installer..." >&2
curl --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 120 -LsSf \
  "$installer_url" -o "$installer_path"
verify_sha256 "$installer_path" "$INSTALLER_SHA256"

if [ "$check_only" -eq 1 ]; then
  echo "verified cargo-dist $DIST_VERSION shell installer"
  exit 0
fi

sh "$installer_path"
