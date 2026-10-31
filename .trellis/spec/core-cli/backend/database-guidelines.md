# Persistence boundary

`download --list-resolutions` is discovery despite its command name: it uses ReadOnly, creates no state/output directories, plans or jobs, and may coexist with a writer. Explicit `--remember` remains a separate core authentication action. Shell configuration and job-monitor decisions must inspect the query flag rather than classifying every Download as a mutation.

The CLI has no database/ORM and no independent journal format. It passes state/output directories, policies and explicit auth actions to Core; Core owns schema versions, atomic checkpoints, state-directory writer lock, recovery, output identity/verification and job-scoped cleanup. CLI must not edit those files or adopt legacy state/cookie files.

`jobs`, discovery, login/status/logout and doctor use read-only journal mode; actual download/resume/merge/clean use one read-write Core session through shutdown. Authentication storage is a separate explicit action; permitting a Remember store policy does not itself store login. Ordinary `--config` JSON contains only state/output paths, concurrency limits and plaintext-fallback preference, with unknown fields rejected.

State-dir overrides do not relocate platform auth storage; isolated smoke must set private HOME/platform config as well. All generated media/state/package/SDK/native binary output stays under ignored/private directories. [Command, privacy and cleanup contracts](quality-guidelines.md) define executable behavior; use core specs for actual journal and credential-store schemas.
