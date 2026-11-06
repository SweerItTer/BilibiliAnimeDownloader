# Durable journal and artifact contracts

## 1. Scope / trigger

There is no database. Schema-1 private JSON job checkpoints and `queue.json` own durable queue metadata; resource checkpoint sidecars are the sole durable-offset authority. This boundary applies to every queue/control/merge/cleanup operation.

## 2. Signatures

`CoreConfig.state_mode: StateMode::{ReadWrite,ReadOnly}`; `Core::jobs() -> watch::Receiver<Arc<QueueSnapshot>>`; `Core::control(JobId,JobAction)`; `Core::clean(CleanupRequest) -> Result<CleanupReport>`.

Private `Journal::open` holds an exclusive OS advisory lock for the entire mutating session. `ResourceCheckpoint { durable_bytes, expected_total, strong_etag, sha256, complete }` lives at `resource-N.media.checkpoint.json`. Job records reference exact resource names and representation identities; snapshot progress never advances recovery offsets.

## 3. Contracts

- Acquire the state-directory writer lock before recovery or mutation. A different job is not a different ownership domain. Different explicit state directories are independent.
- OS handle closure/crash releases ownership. A stale PID/filename is never authority to steal it.
- Classify a contended `fs2` try-lock by comparing its native OS error with [`fs2::lock_contended_error()`](https://docs.rs/fs2/latest/fs2/fn.lock_contended_error.html), for both state writers and component writers. Do not assume Windows maps contention to Unix `ErrorKind::WouldBlock`; all other lock/open IO failures remain filesystem errors.
- Read-only inspection never creates/repairs state or imports credentials; absent state is an empty snapshot. Index capture races are retried, then `Busy` is explicit.
- ReadOnly returns the last committed job phase, even if an active phase survived a terminated writer. It cannot establish that a process is currently downloading. A new ReadWrite session first acquires the OS lock, normalizes retained active phases to Paused and commits that recovery; it does not trust displayed downloaded bytes as resource offsets.
- Each atomic queue index names immutable `job-{checkpoint_revision}.json` records. Write new dirty record versions before swapping the index; retire old versions only after the swap succeeds. An unreferenced new version is never an acknowledged admission or control result.
- Snapshot progress revisions and journal checkpoint revisions are separate. Every observable per-job progress mutation advances that job's snapshot revision and refreshes its cached `Arc`; progress event sequences agree with the published snapshot. Per-job immutable snapshots/plan values reuse unchanged history; checkpoint persistence writes changed records rather than rewriting every job on each progress update.
- Transfer flushes/syncs bytes before atomic resource checkpoint replacement. Restart truncates uncommitted tails and validates durable hashes/validators before append. A nonzero durable offset requires its previously stored matching digest; never create missing proof by hashing current bytes. If that digest mismatches or the resource is shorter than the durable offset, reset only that resource's progress to zero; never retain a smaller unproven offset.
- Before an interrupted/short response returns, drain the original Tokio file handle with `flush`; only then may the existing separate-handle rollback truncate to the proven checkpoint. Dropping a buffered writer does not join its blocking I/O and can otherwise write through rollback or a later retry. Drain failures are fatal filesystem errors and never advance checkpoint offset/hash.
- Queue history is bounded at 1024 retained jobs; new enqueue beyond that fails `Busy` rather than silently dropping jobs. Commands are bounded at 128 and events at 256.
- Filesystem paths are generated from validated job IDs; directory symlink escapes and managed-file symlinks are rejected. Cleanup enumerates exact owned resources/staging, never recursively removes user directories or unmanaged merge inputs.
- Remux staging is on the output filesystem in a private `.bili-JOBID` directory carrying an ownership marker. Promotion is a no-clobber hard link. A pre-promotion durable digest is not enough to adopt a same-named unrelated output: crash recovery also requires the surviving staging hard-link ownership proof until promotion has been explicitly journaled.
- Completion stores stable selected-media identity (canonical part, exact video/audio keys, topology/ordered segments and container) plus output SHA-256/size. Display title, destination, catalog revision and delivery/sidecar policies are not media identity; destination matching is checked separately. Enqueue recomputes identity from the editable plan, so a stale caller fingerprint cannot silently skip changed quality/audio. Verified skip rehashes actual output; filename or size alone is not completion.
- A persistence failure poisons that mutating session: stop admission, clear pending execution and cooperatively cancel owned work. Repairing the filesystem does not resurrect unacknowledged commands; a new writer recovers the last committed index.
- A completed video can retain failed or newly requested sidecar work. `components_pending` is durable; retry verifies the video then reacquires requested sidecars without downloading/remuxing verified video again. Successful retry applies normal owned-source cleanup.
- A failed remux/selected-plan verification may already have every input completely downloaded. Before acquiring locators, match the ordered resource identities against the original full representation keys and validate every complete resource against its recorded length/total/SHA-256. If all pass, proceed directly to native remux, selected-plan checks and normal durable promotion without API/CDN contact for those inputs. Incomplete resources retain the existing locator/transfer recovery path. A complete but corrupt resource fails visibly without truncating proof or silently redownloading; do not confuse snapshot byte totals with completion proof.
- An unavailable captured account must not prevent purely local validation/remux/promotion of complete inputs. Freeze the credential lookup result, require it before locator requests, and report it as a component failure when requested sidecars need networking. Never substitute global/anonymous credentials for a missing explicit account; a successfully verified video survives a sidecar account failure.
- Sidecar ownership manifests and stable OS component lock files live in `state/components`, keyed by the full absolute output path. Equal filenames in different folders remain independent; only temporary no-clobber stages live beside outputs. Proof retains canonical identity, SHA-256/size and staged/published state. Migrate legacy adjacent proof under its lock, durably writing the private proof before removing old metadata. Retry reuses verified bytes, never overwrites an unowned file and still requires a surviving owned staging hard link after interrupted promotion.
- After verified media/attachment recycling, remove a generated source-project folder only with a nonrecursive empty-directory removal. Its basename must match the captured project name/reference. Preserve custom output roots, symlinks/junctions and folders with any remaining user files. Stable private lock files are not unlinked during ordinary component operations.

## 4. Validation / error matrix

Unknown schema, malformed metadata, short checkpoint file or identity mismatch → `Corruption`/`RepresentationChanged`, not a fresh silently successful job. Existing unrelated output → `Conflict`. Competing writer → `Busy`. Read-only command → `ReadOnly`. Failed checkpoint/promotion/sync → visible filesystem failure; no completed snapshot is published first.

## 5. Good / base / bad

Good: writer plus concurrent read-only observer; crash releases writer lock; restart sees either atomic checkpoint version and reconciles known promotion. Base: cancellation retains owned partials. Bad: append using displayed progress, mark completion from HTTP 416 alone, adopt an arbitrary equal-sized destination, or remove unrelated files by age.

## 6. Tests required

`job_recovery` exercises exclusive writers, readers, distinct dirs, actual child-process crash release, unknown schema, unmanaged originals and persistence-failure admission rejection. Private state tests exercise immutable half-commit recovery, matching-byte unrelated output versus owned hard-link recovery, same-sized corruption, dry-run cleanup and retained partials. The component-lock test uses real competing handles, verifies release/reacquisition and distinguishes a non-contention file-open error. The service lifetime test holds an actual owned blocking worker while shutdown retains the writer lock. Transfer tests cover range/validator/flush boundaries. Consolidated delivery evidence owns observed results; Linux execution cannot qualify Windows `Busy` behavior, whose actual regression remains a Windows CI gate.

The composed `download::process_recovery::killed_transfer_recovers_checkpoint_and_normalizes_active_job` regression holds an actual child writer and loopback transfer, creates a durable prefix, injects an uncommitted tail, checks live-writer Busy and nonmutating ReadOnly state, kills/waits only its owned child, and verifies real Core Paused normalization plus actual Range/If-Range transfer from the checkpoint offset. Assert plan/resource identity, SHA-256, final bytes and unrelated-file preservation. Its ignored helper is invoked explicitly by the parent. This composes journal/Core and transport boundaries; it does not execute full content resolution/remux or establish physical power-loss durability. Keep generated evidence outside source Git.

## 7. Wrong versus correct

Wrong: persist a signed URL and displayed offset in `JobSnapshot`, then append blindly. Correct: persist selected IDs/keys and exact resource names; reacquire URL, load resource checkpoint, reconcile hash/length and validate strong If-Range continuity before writing.
