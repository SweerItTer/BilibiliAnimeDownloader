# CLI output and privacy

CLI does not install an HTTP debug logger. Human progress/notices go to stderr; final typed results/paths go to stdout. Machine mode emits one-line schema-version1 envelopes through `output::json`; no ANSI progress is mixed into stdout. Each job result carries its own exit code. Core coalesces progress; CLI consumes watch snapshots rather than depending on every broadcast event.

Allowed output includes canonical IDs, titles/posters, advertised availability, exact selected representation properties, managed output paths, revisions, safe errors, native versions/configuration/capabilities and private-permission diagnostics. Never print Cookie/header values, signed media URLs, QR login URL/poll token, raw remote errors or secret input buffers. A QR is rendered transiently only on an interactive terminal's stderr; QR payloads are omitted from serializable account snapshots.

Never recover/open raw conversations, browser profiles or legacy credential files during development. Use synthetic canaries for security tests and anonymous authorized samples for live worker smoke. A production --remember action is not permission to persist test-only credentials. Do not dump environment variables or `gh auth` token/account output into evidence; inspect nonsecret status booleans or read-only repository permission metadata instead.

Record exact verification commands/exits, safe outputs, source/toolchain/configuration tuple, loader closure and actual artifact URLs/hashes under task research. Distinguish candidate native closure, local smoke and qualified clean target execution. Never fabricate a passed Windows run, source/license qualification or downloadable artifact. Full contracts: [quality guidelines](quality-guidelines.md).
