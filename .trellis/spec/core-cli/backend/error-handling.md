# CLI error handling

Core owns safe `CoreError { code, operation, retryable }`. CLI never formats raw reqwest/upstream/native diagnostic errors, locators, headers or imported Cookie data. `output::exit_code` is the single shell mapping: InvalidInput→2; VerificationRequired/LoginRequired/EntitlementRequired/RegionRestricted/Unavailable/RepresentationChanged→3; Cancelled→130; other operational failures (including Busy/CredentialStore)→4.

`output::error` emits a schema-versioned error envelope with typed error and exit code in JSON mode, or the safe core Display value on stderr in human mode. Keep individual batch failures rather than replacing them with a false-success summary. Requested component errors contribute to job/invocation exit code even with completed usable video. Unknown/failed snapshot streams cannot be treated as completion.

Clap usage errors retain exit2. Shell-only config/input/output errors use trusted static operation labels, never exception text that can echo secret input or untrusted source URLs. JSON parse errors are summarized without offending field contents. Configuration permits only the named ordinary fields and never credentials.

Shutdown is awaited on both success and failure; a cooperative interrupt returns130 after safe checkpoint/join, while a second Ctrl+C explicitly forces crash recovery. An automatic job's intermediate Downloaded phase is not completion. See [validation matrix, batch precedence and runtime contracts](quality-guidelines.md).
