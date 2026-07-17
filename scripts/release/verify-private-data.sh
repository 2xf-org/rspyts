#!/usr/bin/env bash

set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "usage: $0 PATH..." >&2
  exit 2
fi

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
secret_pattern='AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|pypi-[A-Za-z0-9_-]{20,}|npm_[A-Za-z0-9]{20,}|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY'
credential_url_pattern='(https?|ssh)://[^[:space:]/:@]+:[^[:space:]@/]+@'
mac_users_root="/""Users/"
windows_users_root='C:\''Users\'
private_patterns_file=${RSPYTS_PRIVATE_PATTERNS_FILE:-}
failed=false
scanned=0
private_patterns=()

if [[ -n "$private_patterns_file" && ! -f "$private_patterns_file" ]]; then
  echo "configured private-pattern file is missing" >&2
  exit 2
fi

if [[ -n "$private_patterns_file" ]]; then
  while IFS= read -r pattern || [[ -n "$pattern" ]]; do
    [[ -n "$pattern" && ${pattern:0:1} != "#" ]] || continue
    if printf '' | LC_ALL=C grep -Eq -- "$pattern" 2>/dev/null; then
      :
    else
      status=$?
      if [[ "$status" -ne 1 ]]; then
        echo "configured private-pattern file contains an invalid expression" >&2
        exit 2
      fi
    fi
    private_patterns+=("$pattern")
  done < "$private_patterns_file"

  if [[ "${#private_patterns[@]}" -eq 0 ]]; then
    echo "configured private-pattern file contains no expressions" >&2
    exit 2
  fi
fi

matches_private_pattern() {
  local file=$1
  [[ "${#private_patterns[@]}" -gt 0 ]] || return 1

  local pattern
  for pattern in "${private_patterns[@]}"; do
    if LC_ALL=C grep -aEiq -- "$pattern" "$file"; then
      return 0
    fi
  done
  return 1
}

name_matches_private_pattern() {
  local name=$1
  [[ "${#private_patterns[@]}" -gt 0 ]] || return 1

  local pattern
  for pattern in "${private_patterns[@]}"; do
    if printf '%s' "$name" | LC_ALL=C grep -Eiq -- "$pattern"; then
      return 0
    fi
  done
  return 1
}

scan_name() {
  local name=$1
  if printf '%s' "$name" | LC_ALL=C grep -Eiq -- \
    "$secret_pattern|$credential_url_pattern"; then
    echo "sensitive filename found in scanned input" >&2
    failed=true
  fi
  if name_matches_private_pattern "$name"; then
    echo "private filename found in scanned input" >&2
    failed=true
  fi
}

scan_file() {
  local file=$1
  if [[ ! -f "$file" ]]; then
    return 0
  fi
  scanned=$((scanned + 1))

  if LC_ALL=C grep -aEiq -- "$secret_pattern|$credential_url_pattern" "$file"; then
    echo "secret-like content found in scanned file" >&2
    failed=true
  fi

  if matches_private_pattern "$file"; then
    echo "private content found in scanned file" >&2
    failed=true
  fi

  local path
  for path in \
    "$repository_root" \
    "${HOME:-}" \
    "$mac_users_root" \
    "$windows_users_root"
  do
    [[ -n "$path" ]] || continue
    if LC_ALL=C grep -aFq -- "$path" "$file"; then
      echo "local build path found in scanned file" >&2
      failed=true
    fi
  done
}

for input in "$@"; do
  if [[ -d "$input" ]]; then
    while IFS= read -r -d '' file; do
      scan_name "${file#"$input"/}"
      scan_file "$file"
    done < <(find "$input" -type f -print0)
  else
    case "$input" in
      "$repository_root"/*) scan_name "${input#"$repository_root"/}" ;;
      /*) scan_name "$(basename "$input")" ;;
      *) scan_name "${input#./}" ;;
    esac
    scan_file "$input"
  fi
done

if [[ "$failed" == true ]]; then
  exit 1
fi

if [[ "$scanned" -eq 0 ]]; then
  echo "no files were available to scan" >&2
  exit 2
fi
