# CLI development specifications

`core-cli` is the `bili` shell over reusable `bili_core`, not a second downloader or GUI test harness. English code-specs describe the implemented boundary; executed release evidence remains in task research.

## Pre-Development Checklist

1. Read the active core+CLI task PRD/design/implementation and parent design §§3–8/10.
2. Read [directory structure](directory-structure.md), [executable command/native contracts](quality-guidelines.md), [safe errors](error-handling.md), [output/privacy](logging-guidelines.md), and [persistence ownership](database-guidelines.md).
3. Read shared cross-layer/reuse guides. Obtain actual exported core types before editing calls; language-server references precede exported symbol changes.
4. State the smallest change boundary. Preserve unrelated user edits, license terms and source-only Git; no automatic credential imports or helper/GUI dependencies.

## Quality Check

One integration owner runs consolidated fmt/check/clippy/test after source/dependencies settle; workers never run overlapping workspace builds/lock resolution. Then verify real native source configuration, independent rlib/dylib linkage, package loader/source/license closure, extracted help/version/doctor and authorized real media/remux/interrupt/resume/skip. See [quality guidelines §§4–7](quality-guidelines.md). A local glibc2.43 artifact is not baseline2.35 proof, and workflow source is not an executed/published download.

## Guidelines

| Guide | Contract |
|---|---|
| [Directory Structure](directory-structure.md) | Shell/native/package ownership and artifact paths |
| [Persistence](database-guidelines.md) | Core-only journal and explicit opt-in auth storage |
| [Error Handling](error-handling.md) | Typed safe errors, per-job/batch exit precedence |
| [Quality Guidelines](quality-guidelines.md) | Seven-section commands/config/output/native/SDK/package acceptance |
| [Logging Guidelines](logging-guidelines.md) | stdout/stderr, secret exclusion and evidence |
| [Update protocol](../../bili_core/backend/update-protocol.md) | Shared static HTTPS discovery, release descriptor, channel rules, install layout and the `--pre-release`/`auto_update` boundary the CLI must follow |
