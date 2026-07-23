#!/usr/bin/env bash
# Large-vault / large-code perf smoke for rms-memory.
# Generates a temporary fixture under a private HOME, times vault sync + search + code reindex.
# Usage: ./scripts/bench_large_vault.sh [note_count=200] [code_files=40]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NOTES="${1:-200}"
CODE_FILES="${2:-40}"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/rms-bench.XXXXXX")"
export HOME="$WORKDIR/home"
KEY="benchproj"
mkdir -p "$HOME/.rms-memory" "$WORKDIR/code/src" "$WORKDIR/vault" "$WORKDIR/vaults"

echo "== fixture: $NOTES notes, $CODE_FILES rust files =="
for i in $(seq 1 "$NOTES"); do
  id="00000000-0000-4000-8000-$(printf '%012d' "$i")"
  printf '---\nid: %s\ntitle: Note %s\n---\n\n# Note %s\n\nBody about topic-%s with keyword compute.\n' \
    "$id" "$i" "$i" "$((i % 17))" >"$WORKDIR/vault/note-$i.md"
done
for i in $(seq 1 "$CODE_FILES"); do
  cat >"$WORKDIR/code/src/mod_$i.rs" <<EOF
//! bench module $i
pub fn compute_$i(x: u64) -> u64 {
    x.wrapping_mul($i).wrapping_add($i)
}
pub struct Item$i { pub value: u64 }
impl Item$i {
    pub fn new(value: u64) -> Self { Self { value } }
    pub fn bump(&mut self) { self.value = compute_$i(self.value); }
}
EOF
done

BIN="${RMS_MEMORY_BIN:-$ROOT/target/release/rms-memory}"
if [[ ! -x "$BIN" ]]; then
  echo "Building release binary…"
  (cd "$ROOT" && cargo build --release -q)
  BIN="$ROOT/target/release/rms-memory"
fi

cat >"$HOME/.rms-memory/config.toml" <<EOF
revision = 1

[global]
global_vault_path = "$WORKDIR/vaults"

[projects.$KEY]
code_path = "$WORKDIR/code"
vault_path = "$WORKDIR/vault"
include = ["**/*.md"]
exclude = [".git/**"]
code_index_mode = "manual"
code_languages = ["rust"]
EOF

echo "== vault sync =="
START=$(date +%s)
"$BIN" --scope "$KEY" sync
END=$(date +%s)
echo "vault_sync_seconds=$((END - START))"

echo "== search =="
START=$(date +%s)
"$BIN" --scope "$KEY" search "compute" >/dev/null
END=$(date +%s)
echo "search_seconds=$((END - START))"

echo "== code reindex =="
START=$(date +%s)
"$BIN" --scope "$KEY" reindex --code
END=$(date +%s)
echo "code_reindex_seconds=$((END - START))"

rm -rf "$WORKDIR"
echo "bench_ok=1"
