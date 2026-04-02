#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

echo "==> Building web frontend..."
cd crates/uncloud-web
dx build --release
cd ../..

echo "==> Copying build output to src-frontend..."
rm -rf crates/uncloud-desktop/src-frontend/assets/
cp -r target/dx/uncloud-web/release/web/public/* crates/uncloud-desktop/src-frontend/

echo "==> Building Android APK..."
cd crates/uncloud-desktop
cargo tauri android build --debug

echo "==> Done. APK is at:"
find gen/android -name '*.apk' -type f 2>/dev/null
