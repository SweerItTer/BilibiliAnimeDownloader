# Accessibility

## Scope

The current Alpha is operated through the `bili` command-line client; it does not include the GUI. The published Alpha package is for Linux x86_64, with Ubuntu 22.04 / glibc 2.35 as its packaging baseline. Windows packages are not included in this Alpha, and macOS is out of scope.

The CLI is used by entering commands and options in a terminal. Run `./bin/bili --help` or `./bin/bili <command> --help` to see the available commands and options.

## Text output and input

Command results are written to standard output. The default structured results are JSON; `--json` selects schema-versioned JSON output and newline-delimited job updates. Without `--json`, progress and ordinary errors go to standard error. With `--json`, job updates and errors are JSON on standard output; prompts and some notices may still appear on standard error. When redirecting output, handle standard output and standard error separately.

Cookie entry is available through a hidden terminal prompt (`--auth cookie-prompt`, or `bili login cookie`). The prompt requires terminal input and terminal standard error. It accepts typed text, including Unicode, plus Enter and Backspace; it is not a full-screen editor, and arrow-key editing is not supported. The prompt hides the cookie as it is entered. A redirected-input option is available with `--auth cookie-stdin` or `bili login cookie --stdin`; do not put cookies in command arguments, logs, or reports. Login is not saved unless `--remember` is requested.

## QR login

QR login requires an interactive terminal. It displays a Unicode QR graphic on standard error and does not print the QR's login URL as text. This visual-only presentation may be a barrier if your terminal cannot render the graphic or you cannot scan it. The project has not tested QR login with screen readers or other assistive technology. Cookie prompt and redirected cookie input are other login methods; use anonymous access when login is not needed.

## Reporting an accessibility barrier

Please report barriers through the [bug report form](https://github.com/SweerItTer/BilibiliAnimeDownloader/issues/new?template=bug_report.yml) and see the [contribution guide](CONTRIBUTING.md). If possible, describe what you expected, what happened, and steps to reproduce it. You may include your operating system, terminal emulator, and assistive technology and version if relevant.

Do not include cookies, passwords, login tokens, QR images, account details, personal information, or unredacted logs. Redact command arguments and file paths that contain private information. Do not send credentials in a report.

## Verification limits

Assistive technology compatibility has not been tested, and no accessibility conformance assessment has been performed. This statement does not claim WCAG conformance or compatibility with any particular screen reader, magnifier, speech input, or alternative input device.
