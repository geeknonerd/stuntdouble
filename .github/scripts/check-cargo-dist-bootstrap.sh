#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "cargo-dist bootstrap check: $*" >&2
  exit 1
}

root=$(git rev-parse --show-toplevel)
cd "$root"

config="dist-workspace.toml"
manifest=".github/dist-bootstrap/cargo-dist-installer.sha256"
shell_wrapper=".github/dist-bootstrap/cargo-dist-installer.sh"
powershell_wrapper=".github/dist-bootstrap/cargo-dist-installer.ps1"

read_toml_value() {
  local key=$1
  awk -F'"' -v key="$key" '
    $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      count++
      value=$2
    }
    END {
      if (count != 1) {
        exit 1
      }
      print value
    }
  ' "$config"
}

read_optional_toml_value() {
  local key=$1
  awk -F'"' -v key="$key" '
    $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      count++
      value=$2
    }
    END {
      if (count == 0) {
        exit 2
      }
      if (count != 1) {
        exit 1
      }
      print value
    }
  ' "$config"
}

extract_assignment() {
  local file=$1
  local name=$2
  awk -F'"' -v name="$name" '
    {
      line=$0
      sub(/^[[:space:]]*/, "", line)
      if (index(line, name) == 1 && substr(line, length(name) + 1) ~ /^[[:space:]]*=/) {
        count++
        value=$2
      }
    }
    END {
      if (count != 1) {
        exit 1
      }
      print value
    }
  ' "$file"
}

is_sha256() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]]
}

config_version=$(read_toml_value "cargo-dist-version") ||
  die "$config must contain exactly one cargo-dist-version"
pr_run_mode=$(read_toml_value "pr-run-mode") ||
  die "$config must contain exactly one pr-run-mode"

[[ "$pr_run_mode" == "skip" ]] ||
  die "$config must set pr-run-mode = \"skip\""

[[ -f "$manifest" ]] || die "missing $manifest"
[[ -f "$shell_wrapper" ]] || die "missing $shell_wrapper"
[[ -f "$powershell_wrapper" ]] || die "missing $powershell_wrapper"

shell_version=$(extract_assignment "$shell_wrapper" "DIST_VERSION") ||
  die "$shell_wrapper must contain exactly one DIST_VERSION assignment"
shell_sha256=$(extract_assignment "$shell_wrapper" "INSTALLER_SHA256") ||
  die "$shell_wrapper must contain exactly one INSTALLER_SHA256 assignment"
powershell_version=$(extract_assignment "$powershell_wrapper" "\$DistVersion") ||
  die "$powershell_wrapper must contain exactly one DistVersion assignment"
powershell_sha256=$(extract_assignment "$powershell_wrapper" "\$InstallerSha256") ||
  die "$powershell_wrapper must contain exactly one InstallerSha256 assignment"

[[ "$config_version" == "$shell_version" ]] ||
  die "$config declares cargo-dist $config_version, but the shell wrapper pins $shell_version"
[[ "$config_version" == "$powershell_version" ]] ||
  die "$config declares cargo-dist $config_version, but the PowerShell wrapper pins $powershell_version"
is_sha256 "$shell_sha256" ||
  die "invalid shell installer SHA-256 in $shell_wrapper"
is_sha256 "$powershell_sha256" ||
  die "invalid PowerShell installer SHA-256 in $powershell_wrapper"

manifest_count=0
manifest_shell_sha256=""
manifest_powershell_sha256=""
seen_versions=" "
line_number=0

while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
  line_number=$((line_number + 1))
  line=${raw_line%$'\r'}
  if [[ -z "${line//[[:space:]]/}" || "$line" =~ ^[[:space:]]*# ]]; then
    continue
  fi

  read -r -a fields <<< "$line"
  [[ "${#fields[@]}" -eq 3 ]] ||
    die "malformed checksum entry at $manifest:$line_number"

  version=${fields[0]}
  shell_sha256=${fields[1]}
  powershell_sha256=${fields[2]}
  [[ "$version" =~ ^[0-9A-Za-z.+-]+$ ]] ||
    die "invalid cargo-dist version at $manifest:$line_number"
  is_sha256 "$shell_sha256" ||
    die "invalid shell SHA-256 at $manifest:$line_number"
  is_sha256 "$powershell_sha256" ||
    die "invalid PowerShell SHA-256 at $manifest:$line_number"
  [[ "$seen_versions" != *" $version "* ]] ||
    die "duplicate checksum entry for cargo-dist $version"
  seen_versions+="$version "

  if [[ "$version" == "$config_version" ]]; then
    manifest_count=$((manifest_count + 1))
    manifest_shell_sha256=$shell_sha256
    manifest_powershell_sha256=$powershell_sha256
  fi
done < "$manifest"

[[ "$manifest_count" -eq 1 ]] ||
  die "$manifest must contain exactly one checksum entry for cargo-dist $config_version"
[[ "$manifest_shell_sha256" == "$shell_sha256" ]] ||
  die "shell wrapper SHA-256 does not match $manifest"
[[ "$manifest_powershell_sha256" == "$powershell_sha256" ]] ||
  die "PowerShell wrapper SHA-256 does not match $manifest"

release_workflow=.github/workflows/release.yml
[[ -f "$release_workflow" ]] || die "missing $release_workflow"
if grep -Eq '^[[:space:]]*pull_request[[:space:]]*:' "$release_workflow"; then
  die "release.yml must not contain a pull_request trigger"
fi

override=""
if override=$(read_optional_toml_value "cargo-dist-url-override" 2>/dev/null); then
  [[ "$override" =~ ^https://raw\.githubusercontent\.com/geeknonerd/stuntdouble/([0-9a-f]{40})/\.github/dist-bootstrap$ ]] ||
    die "cargo-dist-url-override must be an exact 40-character commit URL"
  pin=${BASH_REMATCH[1]}

  git cat-file -e "${pin}^{commit}" 2>/dev/null ||
    die "cargo-dist-url-override commit $pin does not exist"

  if git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null; then
    base=origin/main
  else
    base=HEAD
  fi
  git merge-base --is-ancestor "$pin" "$base" ||
    die "cargo-dist-url-override commit $pin is not reachable from $base"

  for path in "$shell_wrapper" "$powershell_wrapper" "$manifest"; do
    git show "${pin}:${path}" | cmp -s - "$path" ||
      die "pinned bootstrap file $path differs from the working tree"
  done

  grep -Fq "${override}/cargo-dist-installer.sh" "$release_workflow" ||
    die "release.yml does not use the pinned shell bootstrap wrapper"

  plan=$(mktemp)
  trap 'rm -f "$plan"' EXIT
  dist plan --output-format=json > "$plan"
  jq -e \
    --arg shell "${override}/cargo-dist-installer.sh" \
    --arg powershell "${override}/cargo-dist-installer.ps1" '
      [.ci.github.artifacts_matrix.include[].install_dist.run] as $runs
      | (.ci.github.pr_run_mode == "skip")
        and ($runs | length > 0)
        and (all($runs[]; contains($shell) or contains($powershell)))
        and (any($runs[]; contains($shell)))
        and (any($runs[]; contains($powershell)))
    ' "$plan" >/dev/null ||
    die "dist plan does not route every installer through the pinned wrappers"
else
  status=$?
  [[ "$status" -eq 2 ]] ||
    die "$config must contain at most one cargo-dist-url-override"
  echo "warning: release installer pin is still awaiting Phase 2" >&2
fi

echo "cargo-dist bootstrap checks passed"
