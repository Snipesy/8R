#!/bin/bash
# Real-app smoke test: replace an APK's dex with 8R's output, verify every class with ART,
# install on the attached device/emulator, launch, and show crashes.
#
#   [LIBDB=pack.8rpack|dir] scripts/smoke-apk.sh APK COMPONENT [TAP_X TAP_Y]
#   e.g. scripts/smoke-apk.sh ~/Downloads/app.apk com.example/.MainActivity 160 266
# LIBDB: LibDB packs (`8r-forge build APK`) for 8R to use. SCREENSHOT=file.png: the screen at the end.
# Exits non-zero when a class fails to verify, the app crashes, it isn't in front at the end, or a
# second 8R run on the output changes it.
# scripts/smoke-suite.sh runs it over the real-app set.
#
# Needs: target/release/8r (cargo build --release), Android SDK build-tools, adb with a device.
set -euo pipefail
APK=$1; COMPONENT=$2
SDK=${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}
BT=$(ls -d "$SDK"/build-tools/* | sort -V | tail -1); ADB=$SDK/platform-tools/adb
ROOT=$(cd "$(dirname "$0")/.." && pwd); WORK=$ROOT/target/smoke; mkdir -p "$WORK"
KS=$WORK/debug.jks
[ -f "$KS" ] || keytool -genkeypair -keystore "$KS" -storepass android -keypass android -alias debug \
    -keyalg RSA -validity 10000 -dname CN=8r >/dev/null 2>&1

rm -rf "$WORK/out" && "$ROOT/target/release/8r" undo -o "$WORK/out" ${LIBDB:+--libdb "$LIBDB"} "$APK" >/dev/null
# Idempotence: 8R on its own output (same packs) gives the same bytes.
rm -rf "$WORK/again" && "$ROOT/target/release/8r" undo -o "$WORK/again" ${LIBDB:+--libdb "$LIBDB"} "$WORK/out" >/dev/null
idem=""
for f in "$WORK"/out/classes*.dex; do cmp -s "$f" "$WORK/again/$(basename "$f")" || idem="$idem $(basename "$f")"; done
echo "--- idempotence"; echo "${idem:+changed on a second run:}${idem:-second run identical}"
cp "$APK" "$WORK/unsigned.apk" && (cd "$WORK/out" && zip -q -0 ../unsigned.apk classes*.dex)
"$BT/zipalign" -f -p 4 "$WORK/unsigned.apk" "$WORK/aligned.apk"
"$BT/apksigner" sign --ks "$KS" --ks-pass pass:android --ks-key-alias debug --out "$WORK/8r.apk" "$WORK/aligned.apk" 2>/dev/null

echo "--- ART verification (whole program)"
args=""
for f in "$WORK"/out/classes*.dex; do
    "$ADB" push -q "$f" "/data/local/tmp/8r-smoke-$(basename "$f")" >/dev/null
    args="$args --dex-file=/data/local/tmp/8r-smoke-$(basename "$f")"
done
verify=$("$ADB" shell "dex2oat64$args --oat-file=/data/local/tmp/8r-smoke.odex --compiler-filter=verify & p=\$!; wait \$p; logcat -d --pid=\$p" \
    | grep -E 'failed to verify|Rejecting class' | head -5 || true)
echo "${verify:-all classes verify}"

PKG=${COMPONENT%%/*}
"$ADB" uninstall "$PKG" >/dev/null 2>&1 || true
"$ADB" install -r "$WORK/8r.apk" >/dev/null
"$ADB" logcat -c
"$ADB" shell am start -W -n "$COMPONENT" >/dev/null
sleep 8
if [ $# -ge 4 ]; then "$ADB" shell input tap "$3" "$4"; sleep 8; fi
crash=$("$ADB" logcat -d -b crash | head -20)
top=$("$ADB" shell dumpsys activity activities | grep -E 'topResumedActivity' | head -1)
echo "--- crash buffer"; [ -n "$crash" ] && echo "$crash"
echo "--- top activity"; echo "$top"
[ -n "${SCREENSHOT:-}" ] && "$ADB" exec-out screencap -p > "$SCREENSHOT"
[ -z "$verify" ] && [ -z "$crash" ] && [ -z "$idem" ] && [[ "$top" == *" $PKG/"* ]]
