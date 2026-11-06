# Profile-bound browsing, batches and archive operations

## 1. Scope / trigger

Shared contracts used by the native GUI and CLI. Core owns discovery, authentication, exact selection, durable admission, transfer and artifact authority. Shells own presentation and OS integration. These contracts describe implemented source; observed platform/service acceptance is recorded separately.

## 2. Signatures

- `SearchQuery.filter: SearchFilter`, optional card metadata and source-provided `CategoryPage.page_size/total_pages`; `Core::browse_conditions(PgcSection)` and `browse(BrowseQuery)` are distinct from keyword search.
- `AuthAction::{ListProfiles,SelectProfile { id },ForgetProfile { id }}`; `StartQr { remember, activate }`. `AuthSnapshot` includes `profiles`, `selected_profile` and a QR-completion `login_profile` receipt.
- `Core::{resolve_for,quality_catalog_for}` take an explicit optional profile ID. None means anonymous. Ordinary discovery captures the current selection under the auth transaction lock.
- `Core::plan_in_directory(selection, DownloadContext, root)`; `DownloadContext { project, batch, profile_id }`, `BatchSettings { id, rate_limit }`; exact types live in domain.rs. `StreamPlan.context` and `JobSnapshot.created_at` are optional persisted metadata.
- `Core::{output_info,remove_jobs,retarget_profile}`; `RemoveMode::{RecordsOnly,RecycleFiles}` and actual `RemovalReport` outcomes. `CoreConfig.recycle: Option<RecycleBin>` supplies a trusted host OS callback.
- `Core::{begin_verification,complete_verification}` consume transient request/challenge/proof values. `CoreError.verification` is skipped by serde and redacted by Debug.

## 3. Contracts

Video channels are validated and submitted to source search. PGC web conditions use `/pgc/season/index/condition` with `type=1`, matching the first-party index; `type=0` returns a different option set. `BrowseConditions` exposes ordered typed filter groups and advertised sort choices. `BrowseQuery` replaces the genre-only Rust shape with selected field/value pairs, order/direction and source paging; matched SDK consumers must rebuild. Preserve composite area IDs and year/release-date intervals exactly. Only fixed `BrowseFilterField` keys become parameters; validate every explicit value/order/direction against that section's conditions. Supply source defaults for omitted selections and matching `season_type`/`st`. PGC browsing stays separate from keyword search. Unknown totals remain unknown; deduplicated page length is not page size. Direct season metadata without type evidence remains unclassified.

Credentials never enter ordinary configuration, plans or checkpoints. New credentials remain in memory unless remember is explicitly requested. A profile has an opaque ID, optional real expiry/membership evidence and checked/validity metadata. Missing explicit profiles fail, never substitute anonymous/current credentials. Sessions freeze separate jars for planning, locator renewal and sidecars; public WBI/genre caches may be shared, private jars may not.

Login/status/store candidates are staged before installing the active identity. Cancellation or capacity/store failure preserves the prior identity. `activate=false` registers a separate credential profile without changing global/stored selection, including a login for the same account. QR completion requires its explicit receipt; an already logged-in account is not a completed new login. A versioned registry with selected=None stays unselected on restore.

When an activating login opts to remember the same account as the currently selected in-memory profile, preserve that profile's opaque ID before committing the secure store. This upgrades session-only tasks to the persisted account without orphaning their captured ID. Prefer the matching selected profile over other remembered entries; a detached `activate=false` login still creates its independent session. Existing frozen worker jars do not change when the profile is refreshed.

Secure collections use immutable keyring generations, chunks of at most 1,024 UTF-8 bytes at character boundaries, a bounded root pointer/SHA-256, exact pending/retired generations and a private OS writer lock. This fits Windows' 2,560-byte UTF-16 password blob limit. Interrupted writes preserve the old root; cleanup failures stay retryable and forgetting is not acknowledged until required secret cleanup succeeds. Legacy single-session data remains readable. No keyring enumeration, truncation, implicit plaintext fallback or ordinary credential-bearing files. Store work is owned and joined through shutdown.

Source identity, original ordinal, opaque batch/rate and profile are persisted apart from media fingerprint. Admission time is original Unix milliseconds, retained through retry. Older checkpoints default optional fields to None. Core creates safe project folders for contextual directory planning; ordinary CLI planning keeps its existing flat output policy. Completed same-output re-admission cannot rewrite another immutable context: return Conflict.

Batch rate is aggregate bytes/second across every job/resource in that batch. Reserve debt even on cancellation, and await outstanding shared debt before consuming another response body chunk after resume. Separate batches are independent. Flush/checkpoint acknowledged bytes and join work before pause/shutdown acknowledgement.

Inspection verifies actual owned media before probing through shared FFmpeg. Removal rechecks inactive IDs, serializes against execution/admission/inspection, verifies completion/component hashes and symlink-safe exact paths, then invokes the OS adapter on owned files only. No recursive user-folder removal. The callback has no business/ownership decisions; no callback means an explicit capability failure. Only confirmed missing paths count as recycled; metadata errors retain the record. Partial moves and persistence failures remain visible, with no permanent-delete fallback. OS Recycle Bin recovery is distinct from an application Undo promise.

Verification accepts a bounded actual voucher, registers it on the fixed service endpoint, sends only CAPTCHA parameters to the shell, then validates the returned proof with the service. A grant is installed into the original profile's memory-only verification cookies and never persisted. No supported voucher means no invented challenge URL. Local expiry/cancellation discards the challenge. This is human verification, not automatic CAPTCHA solving or access bypass.

## 4. Validation / error matrix

| Condition | Result |
| --- | --- |
| Unsupported channel/category/order, invalid page or unlisted genre | InvalidInput before submitting that operation |
| Missing profile or expired real credentials | LoginRequired; preserve task/source identity |
| Explicit quality unavailable or preview-only | Typed selection/access failure; no silent degradation |
| Different context owns completed destination | Conflict; retain original plan/time/output |
| Recycle adapter absent / file modified or unowned | NativeCapability / Conflict; preserve files and record |
| Metadata error or partial recycle | Truthful per-operation errors; no invented deletion success |
| Secure blob corruption / cleanup failure | CredentialStore; no plaintext or anonymous fallback |
| Invalid/expired CAPTCHA proof | VerificationRequired; no grant installed |

## 5. Good / base / bad

Good: a batch overrides credentials without changing other tasks/global selection, rejects a missing strict quality and returns each durable acknowledgement. Base: old history has unknown source/date and manually retries anonymously or explicitly selects credentials. Bad: derive grouping from titles, mutate admitted batch settings, count inaccessible paths as deleted, or call a per-resource rate an aggregate limit.

## 6. Tests required

Owner tests cover credential isolation, detached login/cancellation/capacity/store failure, native blob bounds with isolated mocks, interrupted generation writes/cleanup, legacy restore, source filters/paging, per-part strict selection, immutable context/time recovery, aggregate throttling across real loopback resources and cancellation debt, exact owned recycling/probe errors and joined work. No real credentials or production keyring in tests. Consolidated Windows build/test/link/package evidence is separate from live Bilibili/account/CAPTCHA acceptance.

## 7. Wrong versus correct

Wrong: change the shared active jar, then attach an old profile ID to a job; sleep after each resource independently; remove anything matching the displayed filename. Correct: capture a private profile session, use one batch budget through cancellation/resume, and let the core verify each exact artifact before invoking the host's OS action.
