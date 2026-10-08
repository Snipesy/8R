## 8R

8R tries to undo R8 and common SDKs within an Android application.

## Motivation

8R's goal is to assist Android APK forensics on modern toolchains (Kotlin, Jetpack Compose, etc.).

Its goal is not to answer "What is malware?" but rather "What is not malware?"

https://surrealdev.com/what-is-not-malware/

## Methodology

Many of R8's transforms are deterministic, so they can be mapped back to the original sources from
the shipped APK alone. 8R never needs a `mapping.txt`. Every name or structure it recovers carries
a label saying how far you can trust it:

| Label | Meaning |
|---|---|
| **S** (solved) | Provably the original. |
| **D** (deterministic) | Not the original, but a canonical, readable form that is the same however R8 happened to name things. |
| **N** (nondeterministic) | Ambiguous. The full candidate set is given, and the original is in it. |

The transforms are designed to be 'perfect': the output still runs. An APK with 8R's dex in place
of its own verifies and behaves the same on ART (ignoring signature validation). Running 8R on its
own output changes nothing.

Library code can be recognized with **LibDB packs**. `8r-forge` rebuilds the app's own libraries
with the app's own R8 version and settings, then fingerprints the result. The methods, classes,
fields and packages of those libraries can then be named, as S where the match is provable.

## Usage

Needs Rust 1.88+.

```sh
cargo build --release

# Summarize an APK: dex files, build markers, detected sources
target/release/8r info app.apk

# Undo: writes classes*.dex, 8r-mapping.txt (loadable by jadx) and report.json
target/release/8r undo -o out app.apk

# Optional: forge a LibDB pack for the app's libraries, then use it
# (needs java, javac and the Android SDK; downloads the libraries and R8)
target/release/8r-forge build app.apk -o app.8rpack
target/release/8r undo -o out --libdb app.8rpack app.apk
```

`report.json` lists every label with the rule that produced it, plus everything 8R refused to do
and why. `8r rules` lists every rule.

## More

- [DESIGN.md](DESIGN.md): the contract (labels, determinism, α-invariance) and the architecture.
- [docs/research](docs/research): experiments and results, e.g. [LibDB](docs/research/libdb.md).
- `scripts/smoke-apk.sh` and `scripts/smoke-suite.sh`: run 8R's output on a device or emulator.

## License

Apache-2.0, see [LICENSE](LICENSE).
