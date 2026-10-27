# Project guide

Rust Bilibili downloader. `bili_core` is the reusable business library;
`core-cli` builds the `bili` command, and `gui` is a separate native shell.
Keep discovery, selection, authentication, transfer, durable state, remux,
sidecars, and cleanup in core. Shells own input, presentation, and runtime
integration. CLI is the primary delivery path; do not move core behavior into
either shell.

## Common work

- **Before changing code** — read `.trellis/workflow.md`, then the package
  index and only the guides it points to:
  - `bili_core`: `.trellis/spec/bili_core/backend/index.md`
  - CLI: `.trellis/spec/core-cli/backend/index.md`
  - GUI: `.trellis/spec/gui/backend/index.md`
  The core and CLI indexes describe current contracts; the GUI index is still
  a template, so inspect the implementation rather than treating it as a GUI
  convention.
- **Changing the core API** — inspect `bili_core/src/lib.rs` and the owning
  module under `bili_core/src/`; update every affected shell and test. Core's
  public boundary is Rust `rlib` and matched `dylib`, not a C ABI.
- **Changing CLI syntax or output** — `core-cli/src/args.rs` owns Clap
  arguments; `core-cli/src/main.rs` wires one host runtime and core calls;
  `core-cli/src/output.rs` owns presentation and JSON envelopes.
  `core-cli/src/secret_input.rs` owns hidden terminal input and secret
  handoff.
- **Changing native dependencies or delivery** — use
  `scripts/native-lock.json` and `scripts/build_ffmpeg.py` as the FFmpeg source
  pin and build recipe. Build only the pinned shared FFmpeg libraries from
  source; do not add `ffmpeg`/`ffprobe` executable invocation or package them.
  `scripts/verify_core_linkage.py` verifies independent SDK consumers;
  `scripts/package.py` and `scripts/verify_package.py` own package creation
  and inspection. `target/` is generated ignored output; do not commit it or
  treat it as canonical source. Pins and build recipes live under `scripts/`.
- **Adding a helper or test** — first check the owning module and its tests.
  Keep behavioral tests at the layer that owns the behavior; do not duplicate
  core logic in a shell or verify wiring with source-text assertions.
- **Changing a license or shipped notice** — preserve the root MIT License
  with Commons Clause and FFmpeg's separate LGPL-2.1-or-later terms. Generate
  dependency/native notices from the actual package inputs; never invent or
  trim attribution.

## Local development

Develop and debug the source/native build and actual CLI locally first. Start
with the [README building/native setup](README.md) and the pinned source
recipe; CI repeats these entrypoints on target machines, not as the primary
edit/debug loop. Do not treat workflow definitions or uploaded artifacts as
proof of release qualification.

Rust is pinned by `rust-toolchain.toml`. Supported packaging targets are
`x86_64-unknown-linux-gnu` (Ubuntu 22.04 / glibc 2.35 baseline) and
`x86_64-pc-windows-msvc`; macOS is out of scope. Build FFmpeg on its target OS;
Windows requires initialized x64 MSVC plus MSYS2 configure tools. Linux native
builds need the compiler, make, pkg-config, clang/libclang, and inspection
tools described in `.github/workflows/cli-packages.yml`.

Workspace checks and tests require the source-built FFmpeg libraries to be
discoverable by Cargo (`FFMPEG_DIR` and pkg-config paths; runtime loader paths
for executing binaries). Set these up as described in README.md before checks.
Workspace tests also require the generated test corpus: use
`scripts/generate_media_fixtures.py` with `scripts/corpus-lock.json` to create
it under ignored `target/` output, or point `BILI_MEDIA_FIXTURES` at an
existing corpus. Its isolated GPL encoders are test-only; production uses the
separately pinned LGPL FFmpeg shared libraries.

Replace `TARGET` in the following commands with the matching target triple.
These examples require the native toolchain and environment described above;
the Windows recipe runs in the initialized MSVC/MSYS2 environment.

Common checks, after native discovery is configured:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --target TARGET
cargo clippy --workspace --all-targets --locked --target TARGET -- -D warnings
cargo test --workspace --all-targets --locked --target TARGET
python -m unittest discover -s scripts -p "test_*.py"
```

For either target, the local native/CLI/SDK/package entrypoints are:

```sh
python scripts/build_ffmpeg.py --target TARGET --prefix target/native/TARGET --source-cache target/native/sources
cargo build -p core-cli --release --locked --target TARGET
python scripts/verify_core_linkage.py --mode rlib --target TARGET --profile release --native-prefix target/native/TARGET
python scripts/verify_core_linkage.py --mode dylib --target TARGET --profile release --native-prefix target/native/TARGET
python scripts/package.py --target TARGET --profile release --native-prefix target/native/TARGET
```

To inspect an extracted package, pass the package archive, matching
native-source archive, and `--target TARGET` to `scripts/verify_package.py`;
do not add `--public-media-id` without explicit live-test authorization.
Scripts write generated output under ignored `target/`. Never use developer
loader overrides as evidence that an extracted package works; runtime library
loading is package-relative.

## Coding and verification conventions

- Rust style follows the pinned `rust-toolchain.toml` and rustfmt. For Rust
  changes, run `cargo fmt --all -- --check` and applicable
  `cargo clippy --workspace --all-targets --locked --target TARGET -- -D warnings`.
  Run the existing focused behavioral tests at the owning layer; report only
  checks actually run.
- Python and YAML follow adjacent project style and existing validation tools.
  Do not add Black, Ruff, or another formatter/linter dependency.
- Keep behavioral tests with the owning package and assert observable behavior,
  not source text. Cancellation, durable checkpoints, and recovery belong in
  core: persist acknowledged state before replying, join owned work before
  releasing the state writer, and preserve recovery data on pause/cancel/shutdown.
- The `v2.0.0-alpha.1` Alpha CI/CD delivery and acceptance target is Linux
  x86_64 only. Run its fixed BBB smoke in the designated anonymous VM with
  anonymous authentication; never fall back to remembered or interactive
  credentials.
- Git contains source and necessary documentation/configuration only. Keep
  `target/`, packages, generated native/media output, captured logs/receipts,
  and credentials out of commits.

## Git history and changelog

- Use English Gitmoji Conventional Commit messages:
  `<emoji> <type>(<stable-scope>): <imperative subject>`.
- Order commits by responsibility, behavior boundary, and dependency. Keep
  each commit reviewable and a valid intermediate state; a commit may span
  files. Keep an API change with required callers and its behavior test
  together when separating them would leave an invalid or meaningless
  intermediate state. Do not split by file count or put the rewrite in one
  giant baseline commit.
- Keep new work under the existing `Unreleased` changelog heading until
  release. For an approved release, use its actual version and publication
  date. List implemented changes; separate verified results from known
  limitations, and never claim an unrun check or unmeasured performance.
- Use these categories only when they have entries: `✨ Features`, `🐞 Fixes`,
  `⚡ Performance`, `📦 Build/Packaging`, `👷 CI/CD`, `📝 Documentation`,
  `♻️ Refactoring`, `🔒 Security`, `✅ Tests`, and `💥 Breaking Changes`.
  Omit empty categories and planned/unimplemented work.
- Commit subjects use `<emoji> <type>(<stable-scope>): <imperative subject>`.
  Map `feat`→✨, `fix`→🐞, `perf`→⚡, `docs`→📝, `refactor`→♻️,
  `test`→✅, `build`→📦, `ci`→👷, `chore`→🔧, and `revert`→⏪.
  Mark breaking changes with `!` or a `BREAKING CHANGE:` footer and use
  `💥` for the changelog note; do not invent a Conventional Commit type.

## Credentials and live services

Never record real credentials in source, tests, fixtures, docs, logs, task
artifacts, or command transcripts. New login input is memory-only unless the
user explicitly asks to remember it. Test-only credentials must never be
persisted. Use isolated stores/HOME for auth tests; an isolated HOME alone
does not isolate an OS keyring.

Do not call Bilibili or run public-media acceptance without explicit
authorization for that specific operation. Automated public-media smokes
must use anonymous auth explicitly. Any separately authorized authenticated
test must use its supplied credentials only in memory and never persist them.
Never silently retry as a remembered account or bypass a verification/access
challenge. Keep network acceptance separate from offline tests and report
only what was actually exercised.

## Workflow boundaries

Follow `.trellis/workflow.md` for task and spec routing. Do not create task
paperwork when the user declines it; keep small approved edits within their
stated scope. Preserve unrelated working-tree changes. Do not add aliases,
compatibility shims, unrequested runtime processes or a second media engine.
FFmpeg remains an in-process shared-library integration; do not add player
integration or a downloader daemon. The GUI is native except for the approved
captcha/verification flow: load its WebView only when that challenge is
explicitly needed, never preload a hidden page or browser process at startup,
and destroy it on success, cancellation, or timeout. If platform constraints
require retaining it between challenge operations, pause it and verify low
idle resource use; merely minimizing it is insufficient. Aria2 is the
preferred future transfer engine, not an Alpha capability until implemented
and qualified.

<!-- TRELLIS:START -->
# Trellis Instructions

These instructions are for AI assistants working in this project.

This project is managed by Trellis. The working knowledge you need lives under `.trellis/`:

- `.trellis/workflow.md` — development phases, when to create tasks, skill routing
- `.trellis/spec/` — package- and layer-scoped coding guidelines (read before writing code in a given layer)
- `.trellis/workspace/` — per-developer journals and session traces
- `.trellis/tasks/` — active and archived tasks (PRDs, research, jsonl context)

If a Trellis command is available on your platform (e.g. `/trellis:finish-work`, `/trellis:continue`), prefer it over manual steps. Not every platform exposes every command.

If you're using Codex or another agent-capable tool, additional project-scoped helpers may live in:
- `.agents/skills/` — reusable Trellis skills
- `.codex/agents/` — optional custom subagents

Managed by Trellis. Edits outside this block are preserved; edits inside may be overwritten by a future `trellis update`.

<!-- TRELLIS:END -->