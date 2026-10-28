# Building

The repository pins Rust in `rust-toolchain.toml`; build scripts require Python 3.11.9 or later. Use the normal Cargo dependency cache. FFmpeg 8.0.3 is built from pinned source as shared libraries; the CLI does not use an `ffmpeg` executable.

## Linux x86_64

Install a C compiler, `make`, NASM, `pkg-config`, Clang/libclang (for bindings), and `patchelf` (for packaging). From the repository root:

```sh
export FFMPEG_DIR="$PWD/target/native/x86_64-unknown-linux-gnu"
python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu
cargo build -p core-cli --release --locked --target x86_64-unknown-linux-gnu
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

This local package is not a public release. Public packages are built by CI. Keep packaged `bin/` and `lib/` together; the package uses application-relative library paths and does not need the build-time `FFMPEG_DIR`.

## Linux build environment image

`scripts/ci/Dockerfile` describes the Linux build environment CI jobs run in: Ubuntu 22.04 (glibc 2.35, the packaging baseline), the native build tools listed above plus CMake and Ninja, a Python 3.13 built from the official source archive with its published checksum, and the Rust channel pinned in `rust-toolchain.toml` with rustfmt and clippy. The Dockerfile holds the exact Python patch version and checksum, and the build itself asserts both toolchains. The image contains only build inputs; FFmpeg, the CLI, and packages are still built in each job.

The `CI image` workflow builds this image and publishes it to the GitHub Container Registry as `ghcr.io/<owner>/<repo>/ci-linux:sha-<commit>` when `scripts/ci/Dockerfile`, `rust-toolchain.toml`, or the workflow changes on a pushed branch, and when dispatched manually. It then pulls the published digest on a fresh runner and rechecks the environment; the job summary prints the `image@sha256` reference to consume.

Build it locally from the repository root, sending only the Dockerfile as context:

```sh
docker build -t bili-ci-linux - < scripts/ci/Dockerfile
docker run --rm -it -v "$PWD":/src -w /src bili-ci-linux
```

The image ships no checkout and no build output, so mount the repository to build. Check a built image with `docker run --rm bili-ci-linux rustc --version` and `docker run --rm bili-ci-linux python3 --version`.

Consume the published image by the `image@sha256` reference from the workflow summary, not by a moving tag. Pulling it needs a token with `packages: read`; the publishing job uses `GITHUB_TOKEN` with `packages: write` and no stored registry password.

## Windows x86_64

Use an initialized Visual Studio x64 MSVC developer environment, MSYS2 Bash/make/NASM, and the pinned Rust toolchain with the `x86_64-pc-windows-msvc` target. FFmpeg is built with MSVC, not MinGW. In PowerShell from the repository root:

```powershell
$env:FFMPEG_DIR = "$PWD\target\native\x86_64-pc-windows-msvc"
python scripts/build_ffmpeg.py --target x86_64-pc-windows-msvc
cargo build -p core-cli --release --locked --target x86_64-pc-windows-msvc
```

No Windows CLI package has been published. Keep `bili.exe` and its runtime DLLs together when producing a local package.

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
