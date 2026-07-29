#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 /path/to/DiskIndexer.app" >&2
  exit 64
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_bundle="$1"
tool_source="$repo_root/target/release/disk-indexer"
resource_dir="$app_bundle/Contents/Resources"

if [[ ! -x "$tool_source" ]]; then
  echo "Rust release binary is missing: $tool_source" >&2
  exit 66
fi
mkdir -p "$resource_dir"
ditto "$tool_source" "$resource_dir/disk-indexer"
chmod 755 "$resource_dir/disk-indexer"
