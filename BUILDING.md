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
