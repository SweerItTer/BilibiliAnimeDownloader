# Using the `bili_core` SDK

Start with the SDK's `build-info.json`: it records the compiler version and settings used to build this SDK. Run the generated compiler command from the SDK root. `consumer/build-command.txt` uses the prebuilt Rust core; `consumer/source-build-command.txt` rebuilds the included core source. The SDK-root-relative commands and native paths are relocatable with the SDK; do not substitute producer-machine paths.

## What must match

The prebuilt `dylib` is Rust-specific, not a stable ABI. Use the recorded rustc version, target, profile, flags, dependency graph, allocator, panic strategy, native compiler, and headers. Keep the Rust core and dependency artifacts together. The generated compiler inputs may include proc-macro shared libraries in addition to `.rlib`/`.rmeta`; do not remove them as duplicate runtime files. Use the SDK's `native/lib/pkgconfig` metadata without producer paths or overrides.

The SDK includes matching FFmpeg headers and relocatable pkg-config files. FFmpeg remains a separate shared-library dependency: Linux libraries are under `lib/`; Windows runtime DLLs are under `bin/` and import libraries under `native/lib/`. Windows SDKs may include Microsoft CRT files with separate terms; review `licenses/windows-redist/` and `notices/third_party/` before redistribution.

## What it is not

The `dylib` is not a C API, stable cross-build ABI, `staticlib`, `cdylib`, `dlopen` plugin, or hot-unload interface. Keep handles and futures inside the matching process, shut down `Core`, and drop its Rust values before process exit. Never unload or replace the library while values created by it remain alive. A source-built consumer is not a self-contained application. A matching Rust compiler is required to build SDK consumers, but not to run the packaged CLI.

Retain the SDK's license notices and matched build metadata when redistributing it. CLI packages have separate `USAGE.txt` and `BUILDING.txt` instructions; they are not part of the SDK.
