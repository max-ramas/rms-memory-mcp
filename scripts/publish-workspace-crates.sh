#!/usr/bin/env bash
# Publish workspace crates to crates.io in dependency order.
set -euo pipefail

publish_once() {
  local pkg="$1"
  cargo publish -p "${pkg}" --locked 2>&1
}

publish_or_skip() {
  local pkg="$1"
  echo "==> Publishing ${pkg}..."
  set +e
  local output
  output="$(publish_once "${pkg}")"
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
  printf '%s\n' "${output}" >&2
  return "${status}"
}

publish_with_index_retries() {
  local pkg="$1"
  local attempts="${2:-12}"
  local delay="${3:-30}"
  local attempt=1
  while [ "${attempt}" -le "${attempts}" ]; do
    if publish_or_skip "${pkg}"; then
      return 0
    fi
    echo "==> ${pkg} publish attempt ${attempt}/${attempts} failed; waiting ${delay}s for crates.io index..." >&2
    sleep "${delay}"
    attempt=$((attempt + 1))
  done
  echo "==> Failed to publish ${pkg} after ${attempts} attempts." >&2
  return 1
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
# Umbrella depends on exact versions of the workspace members; the registry
# index can lag several minutes after member publishes.
publish_with_index_retries rms-memory-mcp

echo "==> All workspace crates published."
