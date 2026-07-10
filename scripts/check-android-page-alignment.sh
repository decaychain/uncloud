#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <apk> <aab>" >&2
  exit 2
fi

APK=$1
AAB=$2

for artifact in "$APK" "$AAB"; do
  if [[ ! -f "$artifact" ]]; then
    echo "Android artifact not found: $artifact" >&2
    exit 1
  fi
done

if [[ -z "${NDK_HOME:-}" ]]; then
  echo "NDK_HOME must point to the pinned Android NDK" >&2
  exit 1
fi

READELF=$(find "$NDK_HOME/toolchains/llvm/prebuilt" -name llvm-readelf -print -quit)
if [[ -z "$READELF" ]]; then
  echo "llvm-readelf not found under NDK_HOME=$NDK_HOME" >&2
  exit 1
fi

if command -v zipalign >/dev/null 2>&1; then
  ZIPALIGN=$(command -v zipalign)
elif [[ -n "${ANDROID_HOME:-}" ]]; then
  ZIPALIGN=$(find "$ANDROID_HOME/build-tools" -name zipalign -print | sort -V | tail -1)
else
  ZIPALIGN=""
fi

if [[ -z "$ZIPALIGN" ]]; then
  echo "zipalign not found" >&2
  exit 1
fi

if [[ -z "${BUNDLETOOL_JAR:-}" || ! -f "$BUNDLETOOL_JAR" ]]; then
  echo "BUNDLETOOL_JAR must point to the verified bundletool-all JAR" >&2
  exit 1
fi

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

mapfile -t LIBRARIES < <(
  unzip -Z1 "$APK" 'lib/arm64-v8a/*.so' 'lib/x86_64/*.so'
)

if [[ ${#LIBRARIES[@]} -eq 0 ]]; then
  echo "No 64-bit native libraries found in $APK" >&2
  exit 1
fi

for entry in "${LIBRARIES[@]}"; do
  library="$TEMP_DIR/${entry//\//_}"
  unzip -p "$APK" "$entry" > "$library"
  mapfile -t alignments < <(
    "$READELF" -lW "$library" | awk '$1 == "LOAD" { print $NF }'
  )

  if [[ ${#alignments[@]} -eq 0 ]]; then
    echo "No ELF LOAD segments found in $entry" >&2
    exit 1
  fi

  for alignment in "${alignments[@]}"; do
    if (( alignment < 0x4000 )); then
      echo "$entry has LOAD segment alignment $alignment; expected at least 0x4000" >&2
      exit 1
    fi
  done

  echo "ELF alignment OK: $entry"
done

"$ZIPALIGN" -c -P 16 4 "$APK"
echo "APK ZIP alignment OK: $APK"

BUNDLE_CONFIG=$(java -jar "$BUNDLETOOL_JAR" dump config --bundle="$AAB")
if ! grep -Fq '"alignment": "PAGE_ALIGNMENT_16K"' <<< "$BUNDLE_CONFIG"; then
  echo "AAB does not request PAGE_ALIGNMENT_16K:" >&2
  echo "$BUNDLE_CONFIG" >&2
  exit 1
fi

echo "AAB page alignment OK: $AAB"
