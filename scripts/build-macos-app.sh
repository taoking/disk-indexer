#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
derived_data="$repo_root/build/DerivedData"
project="$repo_root/apps/macos/DiskIndexerApp.xcodeproj"

"$repo_root/scripts/build-rust-release.sh"
xcodebuild \
  -project "$project" \
  -scheme DiskIndexerApp \
  -configuration Debug \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$derived_data" \
  CODE_SIGNING_ALLOWED=NO \
  build
"$repo_root/scripts/copy-rust-tool-to-app.sh" "$derived_data/Build/Products/Debug/DiskIndexer.app"
echo "$derived_data/Build/Products/Debug/DiskIndexer.app"
