#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

BUILD_MODE="${1:-debug}"

java_major_version() {
    local java_bin version
    if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/java" ]; then
        java_bin="$JAVA_HOME/bin/java"
    else
        java_bin="$(command -v java || true)"
    fi

    if [ -z "$java_bin" ]; then
        return
    fi

    version="$($java_bin -version 2>&1)"
    version="${version#*\"}"
    version="${version%%\"*}"
    if [[ "$version" == 1.* ]]; then
        version="${version#1.}"
    fi
    echo "${version%%.*}"
}

ensure_android_java() {
    local major java_home
    major="$(java_major_version)"

    if [[ "$major" =~ ^[0-9]+$ ]] && [ "$major" -le 21 ]; then
        return
    fi

    for java_home in /usr/lib/jvm/temurin-21-jdk /usr/lib/jvm/java-21-temurin-jdk; do
        if [ -x "$java_home/bin/java" ]; then
            export JAVA_HOME="$java_home"
            export PATH="$JAVA_HOME/bin:$PATH"
            echo "==> Using JAVA_HOME=$JAVA_HOME for Android Gradle build"
            return
        fi
    done

    if [ -n "$major" ]; then
        echo "Android Gradle build requires JDK 21 or older; current Java major version is $major." >&2
    else
        echo "Android Gradle build requires JDK 21 or older, but no usable Java runtime was found." >&2
    fi
    echo "Install JDK 21 or set JAVA_HOME to a supported JDK before running this script." >&2
    exit 1
}

echo "==> Building web frontend..."
cd crates/uncloud-web
dx build --release
cd ../..

echo "==> Copying build output to src-frontend..."
rm -rf crates/uncloud-desktop/src-frontend/assets/
cp -r target/dx/uncloud-web/release/web/public/* crates/uncloud-desktop/src-frontend/

echo "==> Building Android ($BUILD_MODE)..."
ensure_android_java
cd crates/uncloud-desktop
if [ "$BUILD_MODE" = "release" ]; then
    cargo tauri android build
else
    cargo tauri android build --debug --apk
fi

echo "==> Done. APK is at:"
find gen/android -name '*.apk' -type f 2>/dev/null
if [ "$BUILD_MODE" = "release" ]; then
    echo "==> AAB is at:"
    find gen/android -name '*.aab' -type f 2>/dev/null
else
    echo "==> AAB skipped for debug builds"
fi
