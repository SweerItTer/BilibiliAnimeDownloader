# Building

The repository pins Rust in `rust-toolchain.toml`; build scripts require Python 3.11.9 or later. Use the normal Cargo dependency cache. FFmpeg 8.0.3 is built from pinned source as shared libraries; the CLI does not use an `ffmpeg` executable.

## Linux x86_64

Install a C compiler, `make`, NASM, `pkg-config`, Clang/libclang (for bindings), and `patchelf` (for packaging). From the repository root:

```sh
export FFMPEG_DIR="$PWD/target/native/x86_64-unknown-linux-gnu"
python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu
cargo build -p core-cli --release --locked --target x86_64-unknown-linux-gnu
cargo build -p core-bootstrap --bin bili-bootstrap --features packaged --release --locked --target x86_64-unknown-linux-gnu
```

`FFMPEG_DIR` is used only to find native libraries at build time. To run this unpackaged developer build, set the loader path:

```sh
export LD_LIBRARY_PATH="$FFMPEG_DIR/lib"
target/x86_64-unknown-linux-gnu/release/bili --help
target/x86_64-unknown-linux-gnu/release/bili --version
target/x86_64-unknown-linux-gnu/release/bili doctor
```

To create a local runtime package for development checks:

```sh
python3 scripts/package.py --target x86_64-unknown-linux-gnu \
  --profile release --native-prefix "$FFMPEG_DIR" --runtime-only
```

The runtime package keeps the stable entry at `bin/bili` and the CLI plus native libraries under `versions/<id>/`; the `CURRENT` pointer selects the payload. The package has independent inner-payload and outer-installation SHA256SUMS files and needs no build-time `FFMPEG_DIR`.

The release also publishes `bili-update-cli-<target>.json` beside the package archive; it is a separate Release asset, not a file inside the package, and it is covered by the same checksum file. The updater reads it to find the package. From a packaged build, `./bin/bili update --check` only reports a newer release; `./bin/bili update` installs one. An unpackaged developer build refuses to install, because it has no installation marker. Installing an update has **not** passed acceptance and must not be used on a real installation yet; the protocol and remaining verification gaps are in [the update protocol spec](.trellis/spec/bili_core/backend/update-protocol.md). The same applies to the Windows path, which has no published package and no MSVC verification.

## Linux build environment image

`scripts/ci/Dockerfile` describes the Linux build environment CI jobs run in: Ubuntu 22.04 (glibc 2.35, the packaging baseline), the native build tools listed above plus CMake and Ninja, a Python 3.13 built from the official source archive with its published checksum, and the Rust channel pinned in `rust-toolchain.toml` with rustfmt and clippy. The Dockerfile holds the exact Python patch version and checksum, and the build itself asserts both toolchains. The image contains only build inputs; CI reuses the pinned FFmpeg install and synthetic-corpus tool/output caches only when their fingerprints and contents validate, and rebuilds them on a miss or invalidation. The CLI and packages are built in the current job; a cache hit never skips its Rust build or checks.

The `CI image` workflow builds this image and publishes it to the GitHub Container Registry as `ghcr.io/<owner>/<repo>/ci-linux:sha-<commit>` when `scripts/ci/Dockerfile`, `rust-toolchain.toml`, or the workflow changes on a pushed branch, and when dispatched manually. It then pulls the published digest on a fresh runner and rechecks the environment; the job summary prints the `image@sha256` reference to consume.

Build it locally from the repository root, sending only the Dockerfile as context:

```sh
docker build -t bili-ci-linux - < scripts/ci/Dockerfile
docker run --rm -it -v "$PWD":/src -w /src bili-ci-linux
```

The image ships no checkout and no build output, so mount the repository to build. Check a built image with `docker run --rm bili-ci-linux rustc --version` and `docker run --rm bili-ci-linux python3 --version`.

Consume the published image by the `image@sha256` reference from the workflow summary, not by a moving tag. Pulling it needs a token with `packages: read`; the publishing job uses `GITHUB_TOKEN` with `packages: write` and no stored registry password.

Linux CI consumes that reference in the media-fixture job, the Linux build-matrix leg in `cli-packages.yml`, and the tagged candidate build in `release.yml`. After publishing a new image, update all three image references and their matching cache identities together. The independent smoke and release-verification jobs still run on clean hosted VMs. Rebuild the image only when `scripts/ci/Dockerfile` or `rust-toolchain.toml` changes; Rust build steps reject a toolchain mismatch instead of downloading another toolchain.

## CI routing and cache behavior

`cli-packages.yml` uses `scripts/ci/classify_paths.py` to choose a route. Its Docs whitelist is exactly `README.md`, `README.zh-CN.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `ACCESSIBILITY.md`, `BUILDING.md`, `.github/pull_request_template.md`, Markdown files under `.trellis/spec/`, and direct-child `.md`, `.yml`, or `.yaml` files under `.github/ISSUE_TEMPLATE/`. Docs-only changes take the Docs route. If code also changes, Docs validation runs alongside the code route selected from the non-documentation paths.

The Docs route runs routing and issue-form checks without Rust compilation, native builds, or corpus generation. The GUI route skips CLI packaging and package smoke checks.

When all non-documentation paths are under `core-cli/` or `core-bootstrap/`, the CLI route runs locked checks, Clippy, and tests for `core-cli`, `bili_core`, and `core-bootstrap`, plus the target-specific bootstrap build. Changes confined to `gui/` take the GUI route and run GUI checks. Any other code path or a mix of code areas falls back to the full-workspace route. When configuring branch protection, require `CI result`: it runs even when route-specific jobs are skipped and rejects failed or unexpectedly skipped checks. Tag releases do not use scoped PR routes; every release follows the full offline qualification and publication path. Online media acceptance is separate and does not replace or gate the offline release checks.

The native cache owns `target/native/<TRIPLE>` and the pinned source archives in `target/native/sources`. It reuses an installed prefix only when the source pin, recipe, target/configure inputs, compiler and tool identities, and build environment match, and the installed-file manifest verifies. The fingerprint includes `PATH`, compiler/build flags, `PKG_CONFIG*` settings and `LD_LIBRARY_PATH`. A mismatch or damaged installed file invalidates the prefix; a bad cached source checksum is discarded. On a native cache miss, the checksum-verified archive is fully materialized with the existing safe extraction filter before building.

The corpus cache owns `target/corpus-build`, `target/media-fixtures`, and `~/.cache/bili-native/corpus-sources`. It fingerprints the corpus lock and recipe, tools and relevant build environment, and validates the encoder and fixture manifests against their recorded hashes. Damaged fixtures are regenerated when the validated encoder remains usable; a missing, damaged, or mismatched encoder/toolchain is rebuilt. These caches accelerate inputs only: the current Rust build, checks, lint, and tests still run.

Rust/Cargo caches own `target/<TRIPLE>`, `target/debug`, `target/release`, `target/sdk-build/<TRIPLE>/rlib`, and `target/sdk-build/<TRIPLE>/sourceconsumer/rlib`; they never own `target/native` or native build trees. Stable target directories preserve reusable third-party compilation. SDK verification still copies core source and native inputs into the staged SDK and independently proves the copied source/native consumer; cache hits do not replace that proof, and changed source or staged native paths can require rebuilds.

The source recipe clears only `ffmpeg-sys-next` release artifacts before building: that dependency caches absolute native-library search paths without watching `FFMPEG_DIR`. Reusing those paths after the previous SDK stage is removed would break linking. Other third-party artifacts remain cached; the copied source still builds against its own copied native inputs.

Local WSL/Linux measurements (not hosted CI timings or estimates):

| Cached build | First measured run | Warm reuse | Corruption repair |
| --- | ---: | ---: | ---: |
| Native FFmpeg install | 61.51 s | 0.12 s | Corrupt installed header: 60.87 s |
| Synthetic media corpus | 139.23 s | 0.14 s | Corrupt fixture, existing encoder: 0.65 s |
| Independent rlib SDK verification | 133.99 s | 22.43 s | Not applicable |

The first SDK run used a new copied-source cache and an existing producer cache. The warm run rebuilt only the FFmpeg bindings/wrapper, copied core, and consumer; it still completed the independent SDK checks. These are local measurements, not hosted end-to-end benchmarks, and exclude cache transfer costs.

## Product downloads and durable CI inputs

The final product names are:

| Actions artifact | Product | Status |
|---|---|---|
| `linux-cli` | Linux x86_64 CLI | Produced and verified |
| `windows-cli` | Windows x86_64 CLI | Produced and verified |
| `windows-gui` | Windows x86_64 GUI | Reserved; developed separately, no empty package uploaded |

Each CLI artifact contains its executable package (stable bootstrap and versioned CLI payload), the matching `bili-native-source-<target>.tar.gz`, `bili-update-cli-<target>.json`, and `SHA256SUMS` covering exactly those three assets. The source sidecar contains the pinned FFmpeg source, build recipe, and notices; keep it with the binary when redistributing. SDK hashes never enter the product checksum file.

SDK verification still runs in both modes. Only the dylib SDK is needed by downstream smoke jobs, so CI no longer creates or uploads the unused rlib SDK archive. CLI/source bundles and the SDK handoff are staged separately. Native replacements, the synthetic fixture, and verification receipts remain short-lived job handoffs while checks run.

After a successful run, **Archive CI assets** stores every non-product Actions artifact as an opaque ZIP in `ghcr.io/sweeritter/bilibilianimedownloader/ci-artifacts`. It records the source run, revision, and artifact name; resolves the published manifest digest; pulls by that digest; and compares the ZIP SHA-256 before deleting the Actions copy. Product downloads stay in Actions with their existing retention. A failed publication/readback does not delete its input, and failed CI runs keep their inputs for debugging and retries.

The archiver runs trusted code from the default branch with package/action write access. It never checks out the source PR or extracts its ZIPs, and PR checks do not receive these write permissions. **The new archive workflow activates only after this change reaches the default branch.** Until then, PR runs still show their temporary handoffs. Its manual `run_id` input can archive successful runs using the new product names; legacy layouts without a recognized product are rejected rather than deleting their downloads. This does not change the current Linux-only Alpha release policy or publish an application release.

Archive references and digests appear in that workflow's step summary. Use the exact recorded reference with `oras pull` to retrieve the original Actions ZIP; package read permission may be required. Once handoffs have been archived, rerun the whole successful workflow rather than an individual smoke job that expects the deleted Actions inputs. Failed runs are not cleaned up.

## Windows x86_64

Use an initialized Visual Studio x64 MSVC developer environment, MSYS2 Bash/make/NASM, and the pinned Rust toolchain with the `x86_64-pc-windows-msvc` target. FFmpeg is built with MSVC, not MinGW. In PowerShell from the repository root:

```powershell
$env:FFMPEG_DIR = "$PWD\target\native\x86_64-pc-windows-msvc"
python scripts/build_ffmpeg.py --target x86_64-pc-windows-msvc
cargo build -p core-cli --release --locked --target x86_64-pc-windows-msvc
cargo rustc -p core-bootstrap --bin bili-bootstrap --features packaged --release --locked --target x86_64-pc-windows-msvc -- -C target-feature=+crt-static
```

No Windows CLI package has been published. Ordinary workspace checks use the development bootstrap; shipping builds enable `packaged`, which rejects Windows builds without `crt-static`. Package inspection also checks actual DLL imports. The installer entry is statically linked to the C runtime; runtime DLLs remain inside the selected `versions/<id>/` payload.

## Local checks

Run these from the Linux checkout; use the same `cargo` and `python` commands inside an initialized MSVC developer shell on Windows.

```sh
cargo fmt --all -- --check
cargo clippy -p bili_core -p core-cli --all-targets --locked -- -D warnings
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Media/remux tests use synthetic fixtures. Generate them first, then run the corpus test:

```sh
python3 scripts/generate_media_fixtures.py
BILI_MEDIA_FIXTURES="$PWD/target/media-fixtures" cargo test -p bili_core --test remux_corpus --locked
```

Fixture generation builds isolated test tools and requires CMake and Ninja; it is separate from the packaged application. Set `FFMPEG_DIR`/the loader path as above when building or running locally.

The runtime package includes applicable license/notice texts and replacement instructions. The matching FFmpeg source and build recipe are provided separately in the same release's native-source archive. Preserve the corresponding materials when redistributing or replacing the LGPL shared libraries. See [scripts/THIRD_PARTY_NOTICES.txt](scripts/THIRD_PARTY_NOTICES.txt) and the generated package notices for the exact component terms.
