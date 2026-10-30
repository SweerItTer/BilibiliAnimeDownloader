# Contributing

Bug reports, feature suggestions, and focused changes are welcome.

## Start here

- Read the [README](README.md) for the current product scope and use, then use [BUILDING.md](BUILDING.md) for developer prerequisites and build commands. The build guide is the source of truth; this file does not repeat its native-toolchain setup.
- `bili_core` owns discovery, selection, authentication, transfer, durable state, remux, sidecars, and cleanup. The CLI owns input, presentation, and runtime integration. Keep behavior in core rather than implementing a second version in a shell.
- The current Alpha is a Linux x86_64 CLI. The repository also has Windows source support, but no Windows Alpha package; macOS is out of scope.

## Checks

Run checks that match the changed layer. Rust checks require the native setup described in [BUILDING.md](BUILDING.md); replace `TARGET` with the target triple being checked.

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --target TARGET
cargo clippy --workspace --all-targets --locked --target TARGET -- -D warnings
cargo test --workspace --all-targets --locked --target TARGET
```

For packaging and other Python script changes, run the existing script tests:

```sh
python -m unittest discover -s scripts -p "test_*.py"
```

Media/remux tests use synthetic fixtures. Generate them under ignored `target/` output and follow the fixture-test commands in [BUILDING.md](BUILDING.md). Add behavioral tests at the layer that owns the behavior; avoid source-text or mock-forwarding tests. Authentication tests must use isolated synthetic stores and must not touch the production OS keyring. Documentation-only changes do not need code checks.

Pull-request CI runs offline checks on Linux and Windows and does not contact Bilibili. Online checks are separate manual acceptance and require explicit authorization for the specific operation. Do not include live-service results as routine PR evidence.

## Changes and PRs

- Keep changes focused and describe the user problem and behavior clearly.
- For user-visible changes, update the relevant documentation. Do not commit credentials, cookies, captured logs or receipts, packages, generated native/media output, or anything under `target/`.
- Preserve the root [MIT License with Commons Clause](LICENSE) and FFmpeg's separate LGPL-2.1-or-later terms and notices.
- Use English Gitmoji Conventional Commit messages in this format: `<emoji> <type>(<stable-scope>): <imperative subject>`, for example `📝 docs(repo): clarify contributor checks`.
- In the PR description, report the checks actually run and their observed results. If a check was not run, say why; do not claim unobserved results.
