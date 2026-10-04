#!/bin/bash
# Builds the smoke set's `pokedex-r8.apk`: github.com/Snipesy/pokedex-ctf with R8 fully on. The CTF
# release keeps androidx/kotlin whole (`-keep class androidx.** {*;}`, `kotlin.**`) and drops the
# `META-INF/*.version` files; this build removes those keeps (R8 shrinks and renames the libraries),
# keeps the version files (LibDB reads the app's libraries from them), and signs with the debug key.
#
#   scripts/build-pokedex-r8.sh [OUT_DIR]   (default ~/Downloads; also writes pokedex-r8-mapping.txt)
# Needs: git, JDK 17 (JAVA_HOME; AGP 8.4 doesn't run on newer JDKs), ANDROID_HOME.
set -euo pipefail
OUT=${1:-$HOME/Downloads}
REV=0fba720
WORK=$(cd "$(dirname "$0")/.." && pwd)/target/pokedex-ctf
[ -d "$WORK" ] || git clone -q https://github.com/Snipesy/pokedex-ctf "$WORK"
cd "$WORK"
git checkout -q -f "$REV"
sed -i '/^-keep class androidx\.\*\* {\*;}/d; /^-keep class kotlin\.\*\* {\*;}/d' app/proguard-rules.pro
sed -i '/"META-INF\/\*\.version"/d' app/build.gradle.kts
sed -i 's/signingConfig = signingConfigs.getByName("release")/signingConfig = signingConfigs.getByName("debug")/' app/build.gradle.kts
# sed changes nothing silently when a pattern doesn't match: check every edit took.
! grep -qE '^-keep class (androidx|kotlin)\.\*\* \{\*;\}' app/proguard-rules.pro || { echo "keep rules still present" >&2; exit 1; }
! grep -q '"META-INF/\*\.version"' app/build.gradle.kts || { echo "version files still excluded" >&2; exit 1; }
grep -q 'signingConfig = signingConfigs.getByName("debug")' app/build.gradle.kts || { echo "signing not switched to debug" >&2; exit 1; }
echo "sdk.dir=${ANDROID_HOME:-$HOME/Android/Sdk}" > local.properties
./gradlew --no-daemon -q :app:assembleRelease
cp app/build/outputs/apk/release/app-release.apk "$OUT/pokedex-r8.apk"
cp app/build/outputs/mapping/release/mapping.txt "$OUT/pokedex-r8-mapping.txt"
echo "$OUT/pokedex-r8.apk"
