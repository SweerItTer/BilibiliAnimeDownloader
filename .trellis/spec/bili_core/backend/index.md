# Core backend development guidelines

## Scope

`bili_core` is the reusable Bilibili engine. The CLI and GUI own presentation and runtime integration, not alternate discovery, selection, authentication, transfer or media implementations. These contracts describe the implemented Rust source; test requirements are not claims that checks have passed.

## Guidelines index

| Guide | Contract |
|-------|----------|
| [Directory structure](./directory-structure.md) | Public library boundary, private adapters, module ownership and supervised lifetime |
| [Durable state](./database-guidelines.md) | OS writer ownership, immutable checkpoint versions, resource-offset authority and no-clobber output promotion |
| [Error handling](./error-handling.md) | Typed safe failures, strict selection, cancellation and explicit partial component results |
| [Quality guidelines](./quality-guidelines.md) | Behavioral tests, source-built shared FFmpeg, stream preservation and no competing implementations |
| [Logging guidelines](./logging-guidelines.md) | Credential-free diagnostics, bounded events and truthful progress |
| [Update protocol](./update-protocol.md) | Shared static HTTPS discovery, release descriptor, identity checks, install layout and verification limits; shells must not reimplement either |

## Read before changing code

1. Read the owning contract and its referenced source/tests.
2. For exported symbols, inspect language-server references before a clean cutover; update every affected shell, test and document together.
3. Keep signed URLs, credentials and native pointers out of persisted/public plans. Resolve BV identifiers through the supported view API rather than maintaining a conversion alphabet.
4. Preserve the single reducer, state-directory writer lock and tracked worker ownership. All network adapters share the configured connection budget; blocking native/store work remains owned until it joins.
5. Use the host runtime. Do not add detached runtimes, shell-side engines, transcoding, FFmpeg executable invocation or compatibility shims.

## Completion checklist

- Public IDs/plans remain validated and credential-free; exact representation keys survive renewal.
- Default quality selects actual obtainable spatial resolution before separate FPS/HDR/codec preferences; unknown metadata is not invented.
- Durable bytes advance only after resource data is synced; queue admission/control acknowledgement follows a committed index capture.
- Cancellation/shutdown joins owned work before releasing the writer lock. Persistence failure stops admission/execution until a new session.
- Output success means native probe/preservation checks plus hash-backed, no-clobber promotion. Sidecar failures remain explicit and retry independently of verified video.
- Cleanup touches only exact owned artifacts; unowned merge sources and unrelated output files survive failures.
- Add behavioral regression coverage at the owning boundary. Synthetic authentication tests use memory or an isolated mock store, never the production keyring.
- Coordinate one consolidated verification run after all source changes land; report only observed checks and preserve outstanding platform/package acceptance gaps.
