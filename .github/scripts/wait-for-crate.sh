#!/usr/bin/env bash
set -euo pipefail

crate="${1:?crate name is required}"
version="${2:?crate version is required}"
url="https://crates.io/api/v1/crates/${crate}/${version}"

for attempt in {1..30}; do
  status="$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --user-agent 'rspyts-release-workflow/1.0 (https://github.com/2xf-org/rspyts)' \
    "$url" || true)"
  if [[ "$status" == "200" ]]; then
    echo "${crate} ${version} is available from crates.io"
    exit 0
  fi

  echo "Waiting for ${crate} ${version} in the crates.io index (attempt ${attempt}/30, HTTP ${status})"
  sleep 10
done

echo "Timed out waiting for ${crate} ${version} to become available from crates.io" >&2
exit 1
