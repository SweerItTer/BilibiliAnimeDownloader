# Bilibili Anime Downloader
English | [简体中文](README.zh-CN.md)

A Rust Bilibili-only downloader: `bili_core` is the reusable business library, `bili` is its CLI shell, and `gui` is a separate native GUI shell planned for later. The first Alpha targets the Linux x86_64 CLI; no Alpha download is available yet. No player integration or FFmpeg executable subprocess is included. No WebView is implemented; a future native GUI may load one lazily only for a human verification challenge.

**License:** the application retains [MIT License with Commons Clause](LICENSE), not plain MIT. Bundled FFmpeg libraries have separate LGPL-2.1-or-later rights; the application's restriction does not apply to those libraries. Download only content you are authorized to save; this project does not bypass login, entitlement, regional restrictions, DRM or verification challenges.

## Linux CLI quick start

From the repository root, this uses an archive already present in this checkout under `target/packages`; it is not a download URL and is not available to someone who only cloned the source. Extract into a fresh `target/cli` directory. Once extracted, later terminals only need to assign `BILI` again; do not re-extract. The archive was built on the current WSL host with glibc 2.43; it is not Ubuntu 22.04 release evidence. Keep packaged `bin/` and `lib/` together. No Rust toolchain, FFmpeg executable, compiler setup, or build-only environment is needed to run a packaged CLI.

```sh
printf '%s  %s\n' 'd79a79e8d873db837b36f64adadebdbbacaa4d7e9a58c9a8a598b3fea1d3311b' 'target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz' | sha256sum -c - &&
mkdir target/cli &&
tar -xzf target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz -C target/cli &&
BILI="$PWD/target/cli/bili-x86_64-unknown-linux-gnu-release/bin/bili" &&
"$BILI" --help &&
"$BILI" search --help &&
"$BILI" download --help
```

Search a keyword and inspect results, then choose an ID to download; search does not automatically enqueue results. Page selects the page; there is no page-size option. The user has confirmed authentication, video search, download/remux, subtitles and danmaku. This does not qualify the Alpha package or assert stability/performance. Stop if an access or risk challenge appears.

```sh
"$BILI" search "keyword" --type video --page 1 --auth anonymous
"$BILI" search "keyword" --type video --page 2 --auth anonymous
"$BILI" info BV... --auth anonymous
"$BILI" qualities BV... --part 1 --auth anonymous
"$BILI" download av123456789 --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
"$BILI" download av123456789 ss12345 ep12345 --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

Replace the numeric examples with actual IDs from search (video results use `av` plus the numeric aid; season results use `ss` plus the numeric season ID). `BV...`, `ep...`, and supported Bilibili URLs are also accepted download inputs. The shown IDs are placeholders and must be replaced before running the commands. The final command shows multiple positional IDs. Downloads use anonymous access by default in these examples; subtitles and danmaku are included only when available. Relative output paths resolve from the current working directory, not the CLI installation directory. The CLI reports completed file paths; open the video yourself in a player you already have. It does not install or launch a player.

Press Ctrl+C once to request a cooperative pause. Inspect jobs and resume the same job ID; repeating a download skips only a matching, verified completed file.

```sh
"$BILI" jobs
"$BILI" resume JOB-ID --auth anonymous
"$BILI" download BV... --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

For one-command, non-persistent QR login, put `--auth qr` on the actual `search`, `info`, `qualities`, `download`, or `resume` command. In an interactive terminal, the CLI renders a scannable QR; scan it with the Bilibili app. Both stdin and stderr must be terminals, so do not pipe or redirect the login command. Standalone `login qr` also displays this terminal QR. Without `--remember`, that login is process-local: a later command starts unauthenticated. To remember a production QR login for later processes, run:

```sh
"$BILI" login qr --remember
"$BILI" download av123456789 --parts all --output "$HOME/Videos/Bilibili" --auth stored
```

Remembered login is off by default. Secure OS storage is preferred, but if unavailable the application may fall back to plaintext with a warning in private local storage. Never remember test-only credentials.


## Alpha status and supported targets

The first planned release is `v2.0.0-alpha.1` for the Linux x86_64 CLI. It is being prepared, not published; there is no release download URL yet. A locally available archive was built on WSL with glibc 2.43 and is not Ubuntu 22.04/glibc 2.35 qualification. Release requires a build on the declared Ubuntu 22.04 baseline and clean hosted-VM package/basic anonymous smoke of the exact release artifact.

Windows is not a first-Alpha release target; macOS is out of scope. The GUI, Aria2, a verification WebView, search sorting and corrupted-segment recovery are not included or implemented for this Alpha. Stability, performance and unverified recovery behavior are not claimed.

The application retains the [MIT License with Commons Clause](LICENSE), not plain MIT. FFmpeg is built from pinned source as replaceable shared libraries under separate LGPL-2.1-or-later terms; matching source and notices accompany a release. The application's restriction does not apply to FFmpeg. No `ffmpeg`/`ffprobe` executable is shipped or invoked.


A qualified Windows ZIP places `bili.exe` and its required DLLs together. A Linux tarball has `bin/bili` and `lib/` with application-relative loader paths. Keep the package layout intact. Neither package requires Rust or an installed `ffmpeg`/`ffprobe` executable. Missing load-time libraries can prevent `doctor` itself from starting.

## Maintainer offline package check (Linux)

The following uses the archive layout shared by the historical gate33 generation and the current local-standard generation. The default `target/packages` archive has now been regenerated; identify its bytes using the current application/native-source checksum manifest, which excludes preserved SDK archives, not historical gate33–35 hashes. This does not download a release or qualify a different operating system. From the repository root, extract the CLI archive and keep `bin/` and `lib/` together:

```sh
(cd target/packages/x86_64-unknown-linux-gnu && sha256sum -c SHA256SUMS)
tar -xzf target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz
APP="$PWD/bili-x86_64-unknown-linux-gnu-release"
WORK="$PWD/candidate-local"
mkdir -p "$WORK/home"
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --help
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --version
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --state-dir "$WORK/state" --json doctor
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --state-dir "$WORK/state" --json merge --video "$PWD/target/media-fixtures/h264-aac.mp4" --output "$WORK/manual-merge.mp4"
```

The merge example needs the existing verified synthetic fixture; it does not create one. The recorded gate35 public sample used exactly this command shape, with fresh isolated paths, unchanged defaults and explicit anonymous authentication:

```sh
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --state-dir "$WORK/state" --json download BV1gD4y1o72p --parts 1 --output "$WORK/output" --auth anonymous --merge auto
```

This is a documented example, not authorization to rerun network acceptance. First Ctrl+C cooperatively pauses; `resume JOB-ID --auth anonymous` uses the same state. Verification/access refusals remain errors, never instructions to bypass or switch identity. No proxy was enabled in the recorded acceptance, and a proxy is not a mainline requirement.

## Commands

```text
bili --help
bili --version
bili doctor --json
bili search "keyword" --type all --page 1 --auth anonymous --json
bili info BV... --auth anonymous --json
bili qualities BV... --part 1 --auth anonymous --json
bili download BV... ss... ep... --parts all --output ./downloads --auth anonymous
bili jobs --json
bili resume --all --auth anonymous
bili merge JOB-ID
bili merge --video ./video.m4s --audio ./audio.m4s --output ./merged.mp4
bili clean JOB-ID --dry-run
bili clean --abandoned --include-resumable --dry-run
```

`BV...`, `ss...`, `ep...` and `JOB-ID` above describe argument forms, not executed examples. Canonical BV/AV/season/episode IDs and supported Bilibili URLs resolve through the same core; ordinary multi-P videos, seasons and films expose every part. Search is paged discovery, not a download queue.

Download defaults are **all parts**, highest actually obtainable spatial resolution, all available dialogue subtitle tracks, the current available danmaku pool and automatic local library remux. The selected representation, actual width/height, FPS, dynamic range and codec are displayed separately. A high numeric HDR/Dolby quality label is not a resolution ranking. The core owns ranking; no access or representation fallback is silently invented.

`qualities` without `--part` uses the first/sole part selected by resolution, so an episode with original season ordinal13 or a video URL with `?p=3` remains valid. An explicit `--part` refers to that original ordinal, not a renumbered list starting at1.

Use `--parts 1,3-5`, `--quality ID`, `--video-repr ID`, `--audio-repr ID`, `--codec CODEC` and `--hdr MODE` to constrain selection. An exact unavailable or ambiguous choice fails visibly. `--subtitles all|none|en,ja` selects dialogue tracks; `--subtitles track:ID,track:ID` selects exact track IDs. Do not mix track IDs and language selectors. `--danmaku current|none`, `--merge auto|manual`, `--jobs N`, `--connections N` and `--keep-temp` control the corresponding core policies. `--merge manual` leaves verified managed inputs for `bili merge JOB-ID`; local explicit merge never adopts or deletes your input files.

## Authentication and privacy

Authentication and download/resume happen in **one invocation and one core lifetime**:

```text
bili download BV... --auth qr
bili download BV... --auth cookie-prompt
bili download BV... --auth cookie-stdin
bili resume --all --auth stored
bili login qr --remember
bili login cookie --stdin --remember
bili login status
bili logout
```

- `auto` loads an existing opt-in remembered session, otherwise runs anonymous. A stored-record read failure is an error, not an anonymous fallback. `anonymous` never loads remembered login; `stored` requires it.
- QR and hidden Cookie prompts require an interactive terminal. Non-TTY mode never silently prompts. Hidden prompts restore terminal settings when submitted, rejected or interrupted. `cookie-stdin` / `login cookie --stdin` require explicitly redirected stdin and read bounded secret input into memory. Arrange a trusted secret-input producer; **never put a Cookie in command text, producer arguments, environment variables, a temporary file, a bug report or a fixture**.
- Standalone login without `--remember` only validates this process's session; it does **not** log in a later CLI process. Prefer the same-invocation `download/resume --auth ...` forms for memory-only authentication.
- Remember-me is off by default. `--remember` is accepted only with a newly supplied QR/Cookie session. Explicit production persistence prefers the OS store; an unavailable secure store can use a clearly noticed, platform-private plaintext fallback. Set `allow_plaintext_fallback` to `false` in ordinary settings to reject that fallback. Previously supplied test-only credentials must never be remembered, recovered or put into this repository.
- Cookie-header imports retain every applicable name/value but cannot reconstruct absent original domain/path/expiry metadata. Core applies its safe HTTPS Bilibili scope defaults. Signed media URLs, headers and QR poll tokens are not durable job identity or machine output.

## Jobs, interruption, output and settings

One mutating CLI/GUI core session owns an OS lock for its entire state directory. A second writer gets `Busy` even for a different job; `jobs` is read-only inspection. Use different explicit state directories for independent sessions, not stale-lock deletion.

First Ctrl+C requests a cooperative stop, waits for tracked work and durable checkpoints, and returns 130. A second Ctrl+C forces exit and requires crash recovery. Resume reacquires current locators for the exact selected streams and validates range continuity; a repeated download skips only a matching, verified managed completion, never merely a same-sized file. Failed sidecars remain visible component failures and can be retried without downloading a verified completed video again.

Human progress and notices go to stderr; final results/paths go to stdout. `--json` emits schema-version-1 JSON envelopes (`type` plus `data`), one per line during jobs. It never emits credentials, media locators, QR payloads or ANSI progress. Per-job result records include their exit code. Exit status: 0 all requested components succeeded/verified skipped; 2 invalid usage/input; 3 login/access/verification/selection attention; 4 transport/filesystem/corruption/native/remux/partial failure; 130 interruption. Mixed batches retain each result; interruption takes precedence, then 4, 3 and 2.

`--state-dir PATH` selects the journal location; `--config PATH` reads ordinary JSON settings. Relative CLI file paths are resolved from the current directory without silently overwriting destinations. Default state/output directories are the platform's `bili` local-data `state`/`downloads` directories, not an implicit working-directory fallback. Supported settings (unknown fields fail):

```json
{
  "state_dir": "/absolute/private/state",
  "output_dir": "/absolute/downloads",
  "jobs": 3,
  "connections": 6,
  "allow_plaintext_fallback": false
}
```

Use platform-appropriate paths on Windows. Settings contain no Cookies. Cleanup is job-scoped; inspect `--dry-run` first. Removing resumable owned partials requires `--include-resumable`; cleanup never recursively deletes arbitrary user folders or completed user outputs.

## Subtitle and danmaku compatibility

Dialogue tracks are independent language/type/track-labelled UTF-8 SRT or preserved ASS files. Current-pool danmaku has separate raw/XML and moving-comment ASS outputs, not a forced combined dialogue/danmaku ASS. Missing segments, unsupported modes and requested component failures are reported; deleted/moderated/historical comments are not promised.
A legitimately empty current pool can produce a zero-byte JSONL alongside structurally valid XML/ASS with zero comments/events. Receiver validation parses all three formats, checks count/omission consistency and retains safe-path/file/hash checks; it does not turn transport, protobuf, malformed-output or access errors into empty success. The earlier development-pipeline danmaku corruption cause remains unknown; the later empty-JSONL receiver bug was separate.


Ordinary video needs no special player. Simultaneous dialogue plus moving ASS overlays need a separately installed compatible player and suitable fonts. `bili` never installs or launches one.

The following manual recipe was exercised with **mpv 0.41.0 / libass 0.17.4 on isolated Linux**, using actual core-generated SRT and moving-comment ASS:

```sh
mpv --sub-file="dialogue.srt" --sub-file="danmaku.ass" --sub-ass-override=no --secondary-sub-ass-override=no "video.mp4"
```

Inspect the player's actual subtitle track IDs, then select the dialogue ID as `sid` and the danmaku ID as `secondary-sid`; do not assume fixed IDs. Set `sid=no` or `secondary-sid=no` to hide only that track, and restore its corresponding ID to show it again. **`secondary-sub-ass-override=no` is important when moving ASS is the secondary track.** Simultaneous rendering, independent toggles, restored pixels and timed comment movement were observed in that Linux run.

This is not Windows, font-availability, layout, performance or advanced-mode qualification. XML danmaku and other players require their own format support; those combinations were not exercised here.

## Building and native replacement

Rust is pinned by `rust-toolchain.toml`; normal Cargo dependencies/caches are used. Build FFmpeg from the exact source pin and recipe:

```sh
python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu
FFMPEG_DIR="$PWD/target/native/x86_64-unknown-linux-gnu" \
  cargo build -p core-cli --release --locked --target x86_64-unknown-linux-gnu
```

Build scripts require Python3.11.9+ (CI uses Python3.13); Ubuntu22.04's default Python3.10 is insufficient. Linux additionally needs a C compiler, make, NASM, pkg-config, libclang for bindings and patchelf for packaging. Windows uses the corresponding source-build workflow with inherited MSVC developer environment plus MSYS2 build tools/NASM; MinGW binaries are not a substitute for the MSVC target. `FFMPEG_DIR` is build-time discovery, **not** a runtime library picker. No `ffmpeg-next` static-implying `build` feature is enabled.

### Current-host offline build and package recipe

The current local-first run is recorded in `.trellis/tasks/10-24-core-cli-delivery/research/local-standard-current.json`, with extracted-runtime details in `local-standard-extracted-package.json` alongside it. It actually built current-source FFmpeg and CLI, launched help/version/doctor, completed a synthetic local library merge, produced both SDK modes and the package, and passed the offline extracted-package receiver. This WSL host uses **glibc 2.43**: these bytes are current-host evidence, not Ubuntu 22.04 release bytes or Windows source-build evidence. No new commit, push or CI run is required for this procedure.

On this host the required native tools were already privately extracted; no tool installation or global environment edit was needed. The following subshell reproduces that process-only configuration. On another machine use its installed tool paths instead. The first local Cargo attempt failed because relocated libclang could not find its builtin `limits.h`; its unmodified log is `research/logs/local-standard-cargo-current.txt` under the task above. The `-resource-dir` below points to the existing Clang 21 builtin headers; it does not fabricate or replace system headers.

```sh
(
  tools="$HOME/.cache/bili-native/tools/root/usr"
  export PATH="$tools/bin:$PATH"
  export LD_LIBRARY_PATH="$tools/lib/x86_64-linux-gnu"
  export PKG_CONFIG="$tools/bin/pkgconf"
  export LIBCLANG_PATH="$tools/lib/x86_64-linux-gnu"
  export BINDGEN_EXTRA_CLANG_ARGS="-resource-dir=$tools/lib/llvm-21/lib/clang/21"
  export FFMPEG_DIR="$PWD/target/native/x86_64-unknown-linux-gnu"
  export CARGO_NET_OFFLINE=true
  python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu \
    --source-cache "$HOME/.cache/bili-native/sources" --jobs 8
  cargo build -p core-cli --release --locked --offline --target x86_64-unknown-linux-gnu
  # Direct developer launch needs the freshly built shared libraries.
  LD_LIBRARY_PATH="$FFMPEG_DIR/lib" target/x86_64-unknown-linux-gnu/release/bili --help
  # SDK producers refuse overwrite: preserve any existing target/sdk/<target>
  # outside that path before generating fresh, matched outputs.
  python3 scripts/verify_core_linkage.py --mode rlib --target x86_64-unknown-linux-gnu --native-prefix "$FFMPEG_DIR"
  python3 scripts/verify_core_linkage.py --mode dylib --target x86_64-unknown-linux-gnu --native-prefix "$FFMPEG_DIR"
  python3 scripts/package.py --target x86_64-unknown-linux-gnu --profile release --native-prefix "$FFMPEG_DIR"
)
# Receiver is deliberately outside the build subshell; no private build loader,
# FFMPEG_DIR, libclang or binding-generation environment reaches the app.
env -u LD_LIBRARY_PATH -u FFMPEG_DIR -u LIBCLANG_PATH -u BINDGEN_EXTRA_CLANG_ARGS \
  -u PKG_CONFIG -u PKG_CONFIG_PATH -u PKG_CONFIG_LIBDIR \
  python3 scripts/verify_package.py \
    target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz \
    --target x86_64-unknown-linux-gnu \
    --native-source target/packages/x86_64-unknown-linux-gnu/bili-native-source-x86_64-unknown-linux-gnu.tar.gz \
    --sdk-artifact target/packages/x86_64-unknown-linux-gnu/bili-sdk-dylib-x86_64-unknown-linux-gnu.tar.gz
```

The default offline receiver checks the extracted application, isolated state, help/version and doctor; it does **not** perform a manual synthetic merge without the optional replacement/live branches. The current delivery-path synthetic merge was therefore exercised separately from a fresh extraction with an explicit minimal environment, no build loader/tool variables and package-only PATH: see [the independent extracted merge proof](.trellis/tasks/10-24-core-cli-delivery/research/local-standard-extracted-merge.json). Omitting `--public-media-id` means no Bilibili requests. Optional distinct replacement-library experiments need their own marked source-built replacement prefix, not the ordinary native prefix. Direct developer merge evidence and independent extracted merge evidence are separately retained in the current local-standard record. Native Windows compilation remains blocked by missing Rust/MSVC/SDK/LLVM/MSYS2 dependencies pending installation permission; previously downloaded Windows-package runs are runtime-only evidence.

For the direct developer merge stage, use a fresh private directory and the existing synthetic fixture (the recorded run used `target/media-fixtures/h264-aac.mp4`). These are developer-build loader settings, not extracted-package runtime settings:

```sh
repo="$PWD"
work="$(mktemp -d "$repo/target/local-cli-XXXXXX")"
mkdir -m 700 "$work/home" "$work/config" "$work/data" "$work/state" "$work/cache"
(
  export HOME="$work/home" XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data"
  export XDG_STATE_HOME="$work/state" XDG_CACHE_HOME="$work/cache"
  export LD_LIBRARY_PATH="$repo/target/native/x86_64-unknown-linux-gnu/lib"
  cli="$repo/target/x86_64-unknown-linux-gnu/release/bili"
  "$cli" --version
  "$cli" --state-dir "$work/doctor-state" --json doctor
  "$cli" --state-dir "$work/merge-state" --json merge \
    --video "$repo/target/media-fixtures/h264-aac.mp4" --output "$work/merged.mp4"
)
```

The trimmed shared profile builds avformat/avcodec/avutil, local file demux/mux/parsers/BSFs and narrowly retained probing decoders. No programs, network protocols, encoders, playback, GPL, nonfree or version3 profile is selected. Effective configure closure and native corpus qualification, not a universal decoder-free assertion, determine support. An incompatible MP4 combination retains inputs and reports failure; no transcoding is done.

Release packages include build provenance, checksums, full applicable Rust-dependency/runtime/native license and NOTICE/copyright texts, replacement instructions and an equivalently available matching native-source/recipe asset. Static Rust linking does not remove runtime attribution obligations. Third-party component rights are separate; the application's Commons Clause is not applied to them. Missing/unknown license coverage withholds release rather than substituting SPDX names for the actual texts.

Replace the corresponding FFmpeg shared libraries with compatible same-major-ABI libraries while preserving package layout; you may modify/rebuild the LGPL libraries and reverse-engineer the application as needed to debug those modifications. For example, `python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu --prefix ./target/native/replacement --extra-version replacement-smoke` builds an observable compatible replacement; use the same source/configuration pin and required ABI/capabilities. On Linux each replaced library needs its own `$ORIGIN` RUNPATH (the package recipe uses patchelf); Windows DLLs stay executable-adjacent. Repeat `doctor` and loader/media smoke after replacement. This is not blanket legal or patent clearance.

## Reusable Rust core and SDK

Normal Cargo/source/rlib consumers statically integrate Rust core code while still dynamically using FFmpeg. The separately packaged Rust `dylib` SDK is for an **exact matched build tuple**, including rustc commit/target/flags/profile, lockfile/dependency graph, panic strategy/allocator and native compiler/headers. It is not a stable ABI, C API, `staticlib`, `cdylib`, `dlopen` plugin or hot-unload interface.

The consumer owns one Tokio runtime, creates `Core` with its handle, uses typed APIs/snapshots, then awaits `shutdown` and drops owned values while the matched libraries remain loaded. `scripts/verify_core_linkage.py` builds independent consumers in `rlib` and `dylib` modes and verifies actual imports/runtime failure when the core dylib is removed. `scripts/package.py` and `scripts/verify_package.py` package and exercise the real extracted layouts. Build metadata and consumer recipes accompany each SDK; semver alone never establishes Rust binary compatibility.

Both prebuilt modes execute one SDK-root-relative compiler argv and serialize that same argv to `consumer/build-command.txt`; `-o` and its path are separate arguments. Compiler-input closure includes required normal-dependency proc-macro DSOs as well as rlib/rmeta files; it is not the app runtime closure. The rlib verifier separately executes `consumer/source-build-command.txt` and its readonly lifecycle. Native paths expand from the recipient SDK root, and all pkg-config prefix/libdir/includedir fields relocate together. These SDK builds require the exact matching installed Rust compiler; running the packaged app does not.

Complete installed distro copyright material can include the GCC Runtime Library Exception together with GPL text; preserve the full genuine notice/provenance rather than requiring only a standalone exception filename. The native policy guard likewise reads generated configuration semantics: negated `!CONFIG_GPL=yes` is disabled, while a positive Make assignment or C define still rejects the build. These are packaging/notice checks, not permission to change the application's license or native profile.
