#!/bin/bash
# The real-app smoke set (scripts/smoke-apk.sh on each): apps R8 shrank in different ways.
#   gretio          Gretio 10804000 (Play build, R8 full mode)
#   pokedex-ctf     Pokédex Compose CTF release: R8 on, but androidx/kotlin kept by -keep rules
#   pokedex-r8      the same app built from source with R8 fully on (scripts/build-pokedex-r8.sh)
# The two Pokédex builds are an A/B: one app, mostly-kept vs fully shrunk libraries.
#
#   [APKS=dir] [PACKS=dir] scripts/smoke-suite.sh [NAME...]
# APKS (default ~/Downloads) holds 10804000.apk, app-release-ctf.apk, pokedex-r8.apk. PACKS: a
# directory of LibDB packs (`8r-forge build APK`), passed to 8R as LIBDB (only matching packs are
# used). Screenshots land in target/smoke/<name>.png.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APKS=${APKS:-$HOME/Downloads}
declare -A APK=([gretio]=10804000.apk [pokedex-ctf]=app-release-ctf.apk [pokedex-r8]=pokedex-r8.apk)
declare -A COMPONENT=([gretio]=com.surrealdev.max/.SplashActivity [pokedex-ctf]=com.skydoves.pokedex.compose/.MainActivity [pokedex-r8]=com.skydoves.pokedex.compose/.MainActivity)
# A tap that moves past the first screen: Gretio's "Continue as guest", the first Pokémon.
declare -A TAP=([gretio]="160 266" [pokedex-ctf]="80 190" [pokedex-r8]="80 190")
names=("$@"); [ ${#names[@]} -eq 0 ] && names=(gretio pokedex-ctf pokedex-r8)
mkdir -p "$ROOT/target/smoke"
status=0; summary=""
for n in "${names[@]}"; do
    apk=$APKS/${APK[$n]:?unknown app $n}
    echo "=== $n"
    if [ ! -f "$apk" ]; then summary+="$n: missing $apk"$'\n'; status=1; continue; fi
    # shellcheck disable=SC2086
    if LIBDB=${PACKS:-} SCREENSHOT="$ROOT/target/smoke/$n.png" "$ROOT/scripts/smoke-apk.sh" "$apk" "${COMPONENT[$n]}" ${TAP[$n]}; then
        summary+="$n: ok"$'\n'
    else
        summary+="$n: FAILED"$'\n'; status=1
    fi
done
echo "=== summary"; printf '%s' "$summary"
exit $status
