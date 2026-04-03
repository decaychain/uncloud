#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

BUILD_MODE="${1:-debug}"

echo "==> Building web frontend..."
cd crates/uncloud-web
dx build --release
cd ../..

echo "==> Copying build output to src-frontend..."
rm -rf crates/uncloud-desktop/src-frontend/assets/
cp -r target/dx/uncloud-web/release/web/public/* crates/uncloud-desktop/src-frontend/

echo "==> Building Android ($BUILD_MODE)..."
cd crates/uncloud-desktop
if [ "$BUILD_MODE" = "release" ]; then
    cargo tauri android build
else
    cargo tauri android build --debug
fi

echo "==> Done. APK is at:"
find gen/android -name '*.apk' -type f 2>/dev/null
echo "==> AAB is at:"
find gen/android -name '*.aab' -type f 2>/dev/null
