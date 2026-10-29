# Events, diagnostics and credential privacy

## 1. Scope / trigger

Core has no terminal logger. It exposes bounded safe events and immutable snapshots; shells choose rendering. Persistent auth is a separate explicitly opted-in credential store, never ordinary queue state.

## 2. Signatures

`Core::subscribe() -> broadcast::Receiver<CoreEvent>` (256); `Core::jobs() -> watch::Receiver<Arc<QueueSnapshot>>`; `CoreEvent { sequence, job, phase, message }`. `AuthAction::ImportCookie { header, remember }`, `StartQr { remember }` and other auth controls run within the same Core lifetime. `StorePolicy::{MemoryOnly,Remember { directory, allow_plaintext_fallback }}` grants storage capability but does not imply remember intent.

## 3. Contracts

Events contain monotonic safe transition sequences; broadcast lag requires resynchronizing from the latest snapshot, not replaying an unbounded log. Progress is coalesced and never emits a message per chunk. Diagnostics expose native versions/configuration/capabilities and safe failure classes, not URLs/headers/raw remote messages.

A Cookie request header cannot recover domain/path/expiry. Imported pairs are HTTPS/session-only on explicit Bilibili API/login hosts, with no unrelated CDN access; structured cookies retain actual available attributes. CredentialJar and private locators have redacted Debug and no Serialize. QR login URL/image is transient, non-journaled and skipped by general AuthSnapshot serialization; poll tokens stay inside the auth manager. Remember-off creates no credential file.

The maintained keyring 3.6.3 backend is Windows `windows-native`, Linux `async-secret-service` + `async-io` + `crypto-rust` (pure Rust zbus, not native libdbus/OpenSSL). Synchronous Entry operations run outside async runtime/control threads and are owned/joined by auth lifecycle. If secure storage is unavailable, allowed plaintext fallback is explicitly noticed and protected by Unix 0700/0600 or a Windows protected current-user+SYSTEM DACL. Failure to establish private permissions is an error, not a plaintext success. No adjacent-key encryption. No implicit old-cookie file import or browser-profile reads.

## 4. Validation / error matrix

CR/LF/invalid cookie boundary → InvalidInput; unusable existing opt-in store → CredentialStore, never anonymous identity substitution; unauthorized persistence → explicit error; failed private ACL/mode → Filesystem; challenge/access → corresponding safe typed result. Logout clears selected persistent auth and memory, without deleting unrelated files.

## 5. Good / base / bad

Good: safe job ID/phase and `fetch player metadata` label. Base: memory-only synthetic cookie validation and anonymous public smoke. Bad: printing reqwest errors, signed media locators, Cookie, QR login URL/token or serializing credential jars into an event/journal.

## 6. Tests required

Auth canaries must never appear in errors, serialized snapshots/events/journals or temporary ordinary config files. Test keyring operations use an isolated test/mock backend/namespace initialized safely once; a temporary plaintext directory alone does not isolate OS Entry service/user. No user production keyring/profile/secret access is allowed. Actual Windows CI must exercise protected fallback ACL behavior; Unix tests cannot establish Windows privacy.

## 7. Wrong versus correct

Wrong: enable Remember store configuration and silently persist every QR login. Correct: store only after that QR action's explicit `remember: true`; a no-remember session remains in memory through download and URL renewal, then disappears on logout/shutdown.
