# Typed errors and access boundaries

## 1. Scope / trigger

Every upstream HTTP/JSON/native/filesystem failure is translated at the core boundary. Shells map typed classifications to presentation and exit codes, not parse remote error strings.

## 2. Signatures

`pub type Result<T, E = CoreError> = std::result::Result<T, E>`; `CoreError { code: ErrorCode, operation: String, retryable: bool, http_status: Option<u16>, native_code: Option<i32> }`. Both optional numeric fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`: missing fields in older serialized errors deserialize as `None`, and absent values are omitted. `CoreError::new(code, &'static str)` accepts a trusted safe operation label and initializes both optional codes to `None`. Auth actions and credential jars have redacted Debug; private resolved locators are never serializable.

## 3. Contracts

Do not propagate reqwest Display/Debug, remote messages, Cookie, signed media URLs, QR login/poll tokens or raw headers into errors/events/journals. Native diagnostics may describe library versions/configuration/capabilities, never private media locators. Job failures aggregate in snapshots and overall shell status; joined task failures are not discarded.
HTTP failures retain only the numeric response status in `http_status`, including non-2xx responses and response-body failures. API request-send failures use fixed operation labels for timeout, connect, or generic request failure; never include reqwest error text, remote messages, headers or URLs in the label.
QR generation accepts callback URLs only over HTTPS on `passport.bilibili.com` or `account.bilibili.com`, with the default port, no userinfo or fragment, and bounded challenge fields. Preserve that host boundary and use safe stage labels for generation rejection versus malformed challenge; never include a raw URL, QR token or remote body.

At each FFmpeg failure boundary preserve its original signed `AVERROR` number in `native_code` plus the exact trusted operation stage. This field survives snapshot/journal/JSON serialization; absent values default to `None` and are omitted. Display includes an actual native code without allocating an intermediate message. Do not mask distinct open/header/packet/trailer failures behind one generic remux label or retain native error text that might contain paths, URLs or cue contents.

`VerificationRequired` is distinct from an empty successful search even when upstream code is zero. Access labels require evidence; missing advertised streams remain `Unknown` if no specific reason is established. No captcha/entitlement bypass. HTTP 403/410 is access/possibly expired locator, not proof a representation changed: bounded renewal must re-resolve the same full key and validate bytes. Only renewed metadata/key mismatch establishes `RepresentationChanged`.

Pause/cancel acknowledges joined workers and durable checkpoints. Before commit the stop request is honored; once the reducer has persisted Committing, stop waits for actual promotion/manifest outcome, never labels a promoted output cancelled. Failed sidecars keep a verified video and retry independently.

## 4. Validation / error matrix

| Condition | Classification |
|---|---|
| Invalid URL/ID/range/filter or ambiguous repeated numeric stream ID | InvalidInput / Unavailable with static operation |
| Challenge-only payload | VerificationRequired |
| Proven login / entitlement / region restriction | Matching access code |
| Exact key absent on renewed metadata | RepresentationChanged |
| Transient HTTP/IO request, possibly expired locator | Transport with safe retry classification |
| Exhausted bounded attempts | RetryExhausted |
| Output collision / writer collision | Conflict / Busy |
| Bad checkpoint/digest/stream verification | Corruption |
| Unsupported mux combination / native packet failure | NativeCapability / Remux |
| Requested stop before commit | Cancelled |

## 5. Good / base / bad

Good: `CoreError::new(ErrorCode::Transport,"fetch player metadata")`. Base: a batch exposes each failed job while other jobs finish. Bad: `format!("request failed: {remote_error}")` leaking a signed URL.

## 6. Tests required

Synthetic credential canaries must be absent from serialized errors, snapshots, events and journal files. Discovery distinguishes challenge/empty/error; QR challenge parsing accepts the official account host while rejecting untrusted hosts and keeps error labels free of URL/token values. Quality tests cover strict selection/renewal; worker tests preserve failed inputs and surface native/sidecar errors. Check logs/evidence only after the owning consolidated run.

## 7. Wrong versus correct

Wrong: HTTP403 → RepresentationChanged → auto-select lower quality. Correct: boundedly reacquire the same complete RepresentationKey, revalidate range continuity, and fail access/renewal truthfully if still unavailable.
