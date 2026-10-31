# CLI directory structure

| Path | Responsibility |
|---|---|
| `core-cli/src/args.rs` | Clap syntax, mutually exclusive routes, bounded numeric flags; no API calls or selection ranking |
| `core-cli/src/main.rs` | Ordinary settings, path conversion, explicit auth input/QR rendering, one Tokio host, Core calls, snapshot waiting and signal/shutdown lifecycle |
| `core-cli/src/output.rs` | Versioned JSON envelopes, human result/progress presentation, safe typed error/exit mapping |
| `core-cli/src/resolutions.rs` | Private shell catalog-query routing over Core resolve/catalog calls, per-input/part reports and batch exit aggregation; no discovery or access rules |
| `scripts/native-lock.json`, `scripts/build_ffmpeg.py` | Exact FFmpeg source/config pin and actual shared-library source build |
| `scripts/verify_core_linkage.py`, `scripts/sdk_consumer/` | Independent source/rlib and matched Rust dylib consumer/build tuple/loader proof |
| `scripts/package.py`, `scripts/verify_package.py` | Real ignored artifacts, checksums/provenance/notices and extracted-package execution |
| `scripts/collect_licenses.py` | Full actual Rust-dependency/runtime/native notice assets with source provenance; generated output only |
| `scripts/windows_redist.py`, `scripts/windows-redist-lock.json`, `scripts/dependency-license-lock.json` | Pinned official notice sources and authenticated published-license omissions; original assets/cache/output remain generated |
| `scripts/generate_media_fixtures.py`, `scripts/corpus-lock.json` | Isolated source-built test-only synthetic media tools/provenance, never app runtime/package payload |
| `.github/workflows/` | Source/config/lock/toolchain-keyed build, candidate artifact download and clean target qualification |
| `target/` | Ignored compiler/native/media/package/SDK output; never source Git inputs |

Core and GUI are separate packages; CLI has no GUI/graphics dependency. Public core changes are owned and migrated in `bili_core`, not wrapped with aliases in the CLI. Keep settings/output helpers local; no second engine interface, daemon, REPL or cross-process pause/cancel controls. [Executable cross-layer contract](quality-guidelines.md) is authoritative for commands, output, errors and delivery gates.
