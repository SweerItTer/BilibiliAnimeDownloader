# Core module ownership

## 1. Scope / trigger

The core is a reusable library; `core-cli` and `gui` are shells, not sibling engines. This replaces the earlier season/first-DURL and GUI-oriented mutable State contract.

## 2. Signatures

`Core::new(CoreConfig, tokio::runtime::Handle) -> Result<Core>` is the host boundary. Public root exports are `Core`, typed `domain` values, `AuthAction` and `AuthSnapshot`. `remux` diagnostics/probe and pure `sidecar` conversion operations are reusable library APIs; HTTP locators/native pointers remain internal.

## 3. Contracts

- `domain.rs`: validated IDs, credential-free plans/snapshots/errors/configuration.
- `api.rs`: Bilibili adapters, WBI, discovery/catalog/strict selection and private expiring locators.
- `cookie.rs`, `auth.rs`: origin-scoped in-memory credentials, opt-in stores and QR lifecycle. The core returns the validated login URL; shells render QR images.
- `service.rs`: one bounded reducer, structurally shared immutable snapshots, supervised jobs, cancellation and joined shutdown.
- `download.rs`: identity-encoded HTTP range transfer and the sole durable resource-offset checkpoints.
- `state.rs`: state-directory OS lock, atomic queue/job metadata, exact artifact ownership, hashes and no-clobber promotion.
- `remux.rs`: local-only shared FFmpeg packet remux/probe; no executable invocation/transcoding.
- `sidecar.rs`: independent dialogue and current-pool danmaku conversion/acquisition.

Source/rlib and matched Rust dylib use these same implementations. Shell dependencies, generic engines, plugins, IPC and C ABI are absent by design.

## 4. Validation / error matrix

Invalid IDs/configuration → `InvalidInput`; writer collision → `Busy`; read-only mutation → `ReadOnly`; upstream challenge → `VerificationRequired`; exact stream disappearance → `RepresentationChanged`; filesystem/native failures retain sources and return typed safe errors.

## 5. Good / base / bad

Good: one host runtime, one `Core`, memory-only auth and download/renewal in that lifetime, awaited shutdown. Base: read-only `jobs()` inspection needs no writer ownership. Bad: GUI recreating API/HTTP/selection logic or a detached runtime surviving Core shutdown.

## 6. Tests required

`download::transfer_recovery` lives in `src/download/transfer_recovery.rs` under the real library's `#[cfg(test)]` boundary; run `cargo test -p bili_core --lib download::transfer_recovery --locked`. Do not recompile production modules via an integration target's `#[path]` imports or expose private locators solely for testing. `job_recovery` covers competing writers/read-only/crash release and unmanaged input retention. `discovery_contract`, `quality_contract`, `auth_privacy`, `sidecar_contract`, `remux_corpus` and source-side private-boundary tests exercise their owning contracts. Test existence is not evidence of a passing run; delivery records executed checks.

## 7. Wrong versus correct

Wrong: expose raw playurl JSON/URLs for shells to download. Correct: shells select `RepresentationKey` values and enqueue `StreamPlan`; core privately resolves fresh locators and owns transfer/remux/journal behavior.
