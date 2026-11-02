# Native browse/download interaction

## 1. Scope / trigger

The approved native egui interaction, without a palette/appearance redesign. Read the shared core browse/download contract and actual Rust modules; older generic GUI guidelines remain templates.

## 2. Signatures

`App` owns navigation intent, one `SearchState`, retained `DetailState` drafts, `ConfigState` and grouped `QueueState`. `backend::Operation/Outcome` is the bounded typed bridge. `Prepare` carries exact parts, explicit profile and refresh-versus-new-intent metadata; acknowledged admission returns immutable plan/job receipts. The host owns one Tokio runtime and joins background/native work on shutdown.

## 3. Contracts

The BiliDL-only logo returns home. Header categories, shorter search and right-aligned equal circular avatar/history replace the sidebar. Settings is an avatar submenu. Covers open details; only explicit bulk mode selects cards across pages/categories. Video owns channels/sorts; PGC has source-provided genre/section browsing separate from keyword search. Pagination is `< current/total >`; current is editable in place, Enter validates, Escape cancels, and unknown totals remain unknown.

Cover painting retains the decoded texture's intrinsic dimensions. Video cards use landscape slots and Bangumi/Film use portrait slots; contain and center the actual image without changing its aspect ratio. Keep the full slot as the card hit target. Never pass target-slot dimensions as a texture's source size. Category controls need no redundant “分类浏览” caption. Single-line fields share vertically centered normal body text and explicitly muted placeholder text; inherited global ink must not make a hint look like entered content. Text, caret and pointer selection must agree after alignment.

Unique/explicit resolved parts initially select themselves; whole multi-part details start empty. Detail and bulk use the same configuration model. A deliberately selected whole bulk content includes its unadmitted parts, adjustable before admission. Per-part catalogs and `QualityCatalog::select` determine red blocking rows, including previews/login/VIP restrictions. Highest-available and skipping are explicit. Server planning revalidates all pending parts before any admission.

Batch directory changes remain separate from opt-in default changes. Capture directory/default intent, batch rate/profile/sidecars/quality when submitting. Only acknowledged parts leave selection. Keep receipts independent from mutable retry settings and wait for admitted IDs to appear in snapshots before conflicting actions. Partial failure/cancel/reentry retains remaining intent. Refresh updates catalogs without replacing deliberately changed parts. Failed preparations/browses retry their original typed intent.

Navigation revisions distinguish awaited results from current display intent. Home/history/categories/settings/back or cancelling a read operation invalidate automatic detail/dialog opening. Late successful reads cannot reopen a dismissed workflow or replace cancelled committed results. Enqueue receipts and committed QR results survive cancellation; they do not take over navigation.

History has exactly In Progress, Failed and Completed. Paused/interrupted tasks remain resumable in In Progress; sidecar failures and NeedsAttention join Failed. Group stable source projects, filter type and sort by admission time; order original parts within projects. Completed actions reveal the actual Windows path and expose useful media/attachment parameters. Project directory choices reflect distinct actual roots. Confirmation re-evaluates whole-project IDs; no deletion during admission/active work. Record removal and verified-file recycling are separate; system recovery has no fake in-app Undo.

Account switching preserves drafts and affects future operations. Task-specific replacement retargets only that stopped task; adding a new QR profile uses activate=false and the explicit login_profile receipt, preserving global selection and preferences. New accounts require a fresh remember choice. Cancellation only cancels the challenge and retains the prior account. Unknown expiry/membership stays unknown; catalog obtainability is authoritative.

Only a supplied supported voucher exposes verification. Windows `verification::View` is created after explicit action, uses a temporary/incognito context and an in-memory custom protocol origin, bounded nonce-matched IPC, no downloads/new windows/permissions and no Bilibili credentials in JavaScript. About-blank IPC is unsuitable for this library's HTTP URI wrapper. The server must validate the proof before success. Drop the native controller before submitting proof or on close/cancel/timeout/shutdown. No hidden startup WebView or browser daemon.

## 4. Validation / error matrix

| Condition | UI result |
| --- | --- |
| Unsupported quality/preview/rights | Red row plus cause and explicit override/skip; Start disabled |
| Read cancelled or navigation changed | Preserve committed results/draft; no reopening |
| Partial admission | Keep actual receipts and remaining retryable parts on current workflow |
| Network / sidecar / auth failure | Corresponding checkpoint / sidecar-only / task credential action and persistent safe code |
| Unknown/missing media or modified output | Honest unknown/error; offer record-only removal when appropriate |
| Missing WebView runtime or unsupported challenge | Explicit failure; retain retry intent, no fake verification |

## 5. Good / base / bad

Good: continue adding while acknowledged receipts remain visible and account changes refresh only relevant catalogs. Base: anonymous browsing and original saved tasks survive a normal close/reopen. Bad: stale preparation opens over Home, a retry changes old receipts, a task-only QR changes the global account, or a generic task detail replaces useful file/media actions.

## 6. Tests required

Input-driven egui tests cover header/layout, search/detail/config/admission/history flow, scroll return, inline paging, mixed strict quality, cancelled/new part intent, partial acknowledgement, late-result/cancel races, typed retry, account isolation and QR cancellation. Core tests own transfer/durability/credential/filesystem semantics. A separate ignored native Windows test loads only an in-memory local page to exercise actual WebView creation/IPC/destruction; it never solves a real CAPTCHA. Report executed tests and remaining live-service/platform limits separately.

Layout changes also need native rendered inspection with representative portrait and landscape textures, empty and populated inputs, focus/caret, details and directory fields. Geometry/existence assertions alone do not establish acceptable rendering. Use an isolated test host with default in-memory preferences, no bridge/update owner and no credential or service access; do not launch the production host with its real remembered profile merely to inspect layout. Keep manual inspection evidence separate from automated pass counts.

## 7. Wrong versus correct

Wrong: open a modal whenever preparation completes, resolve an explicit-account batch with global credentials, or persist the whole UI mock as real job state. Correct: gate display by live intent, use `resolve_for`/catalog/planning with the captured profile, and consume durable core snapshots and acknowledgements.
