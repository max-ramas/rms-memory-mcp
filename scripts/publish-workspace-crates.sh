#!/usr/bin/env bash
# Publish workspace crates to crates.io in dependency order.
set -euo pipefail

publish_or_skip() {
  local pkg="$1"
  echo "==> Publishing ${pkg}..."
  set +e
  local output
  output="$(cargo publish -p "${pkg}" --locked 2>&1)"
  local status=$?
  set -e
  printf '%s\n' "${output}"
  if [ "${status}" -eq 0 ]; then
    return 0
  fi
  if printf '%s\n' "${output}" | grep -q 'already exists on crates.io'; then
    echo "==> ${pkg} already on crates.io; continuing."
    return 0
  fi
  return "${status}"
}

wait_for_index() {
  echo "==> Waiting for crates.io index propagation..."
  sleep 20
}

publish_or_skip rms-memory-core
wait_for_index
publish_or_skip rms-memory-index
wait_for_index
publish_or_skip rms-memory-vault
wait_for_index
publish_or_skip rms-memory-mcp

echo "==> All workspace crates published."
