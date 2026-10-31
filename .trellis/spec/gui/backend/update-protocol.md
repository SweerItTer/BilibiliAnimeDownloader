# GUI update integration

## 1. Scope / trigger
Read the shared core update protocol first. The native GUI schedules updates and validates its staged runtime; core owns discovery, descriptors, downloads, archive/inventory checks and activation.

## 2. Signatures
`identity() -> Result<Identity, String>` compiles Product::Gui, target and CARGO_PKG_VERSION into the shell. `doctor() -> bili_core::Result<serde_json::Value>` implements `bili --json doctor`; `restart_entry() -> Result<PathBuf, String>` resolves the stable entry. `UpdateState::connected(&tokio::runtime::Handle)` returns state and an owner that main awaits on shutdown. Take full signatures from gui/src/update.rs before changing callers.

## 3. Contracts
`GuiPrefs.update` is shared UpdateSettings: allow_pre_release=false, check_updates=true, auto_update=false. Stable/Beta select stable/pre-release. GUI/product/target/channel ReminderStore enforces the 24-hour interval, failed-check throttling, clock rollback and per-version announcement deduplication. Available candidates can be rechecked after the interval. Automatic check failures are silent; explicit actions show safe errors. last_check is display-only.

The host's one Tokio runtime owns the update worker. Painting sends bounded requests and polls results. Cancellation stops network work; after a blocking installation starts, shutdown awaits its transaction. No helper or detached runtime is allowed.

Keep GUI Tokio features limited to fs/macros/rt-multi-thread/sync/time; do not restore full without an actual need and a full Windows workspace build. Feature unification also affects core's Rust dylib: the broad graph exceeded MSVC's 65535 import-library object limit, while the narrowed graph passed the complete workspace checks and tests.

Headless doctor executes before preferences, auth, Core or window creation. It emits one strict schema-version 1 doctor envelope with compiled build identity and real remux::diagnostics. Option values such as --state-dir doctor remain paths. Staged validation clears inherited environment, creates physical private profile folders, obtains Windows directories through Win32, bounds output to 256 KiB and execution to 15 seconds, and checks candidate identity. Windows starts the canonical verbatim executable with CREATE_NO_WINDOW; DLL search has only package-adjacent libraries and system directories.

GUI packages use package.py --product gui: stable bili.exe, versions/<id>/bili.exe, GUI marker/build metadata, bili-gui-* archive and bili-update-gui-<target>.json. Build bootstrap with packaged,gui and static CRT; its actual x64 PE32+ subsystem must be 2 (GUI), while CLI is 3 (console). GUI import inspection additionally permits only the Windows-provided DWM, OpenGL, UI Automation and theme libraries: dwmapi.dll, opengl32.dll, uiautomationcore.dll, uxtheme.dll. Other native dependencies must remain bundled. Windows package staging uses extended filesystem paths so deep embedded font/license files survive inventory, ZIP creation and cleanup. The shared receiver verifies source, notices, inventories, native closure, subsystem and the GUI headless report; it rejects CLI media acceptance options for GUI packages.

See Microsoft's [PE format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#windows-subsystem), [DWM](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmiscompositionenabled), [OpenGL](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-wglgetprocaddress), [UI Automation](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationcoreapi/nf-uiautomationcoreapi-uiareturnrawelementprovider) and [themes](https://learn.microsoft.com/en-us/windows/win32/api/uxtheme/nf-uxtheme-drawthemebackground) requirements for those system fields/imports.

Activation selects an independently validated payload while the current window/jobs continue on the old one. Restart finishes backend shutdown and both worker joins, then starts the unchanged stable entry and forwards original arguments, including --state-dir. No old-version deletion or automatic rollback is implemented. Existing flat builds need a manual first installation. CI uses windows-gui as product artifact; Linux GUI handoffs remain internal and unqualified. No GUI release is published by this change.

## 4. Validation / error matrix
| Condition | Observable result |
| --- | --- |
| Wrong product/target/version or damaged archive/inventory | Core rejects before CURRENT changes |
| Missing native DLL, invalid/oversized doctor, wrong identity or timeout | Staged GUI rejected; old/bootstrap/user bytes retained |
| Wrong bootstrap subsystem or unbundled non-system dependency | Package creation/inspection fails |
| Developer/old flat build | Checks supported; installation refused |
| Repeated/stale candidate | Core rejects before runtime validation |
| Close during network / installation | Network cancelled; owned transaction joined before exit |

## 5. Good / base / bad
Good: core discovers a GUI candidate and installs it only after actual headless native validation. Base: offline doctor opens no window or saved login. Bad: a shell calls Releases API, extracts files itself, overwrites active payload or writes CURRENT.

## 6. Tests and observed acceptance
Keep ordinary unit tests offline. The ignored native_gui_packages_update_at_multiple_depths_and_preserve_failures test requires explicit local baseline/archive/descriptor paths, runs the real doctor, rejects missing DLLs, checks byte preservation, activates and rejects repeats.

On 2026-11-01 native Windows 11 build 26200 acceptance passed for source-built 2.0.0-alpha.6 and version-only 2.0.0-alpha.6.windows-test.20261101.1 packages at root lengths 37, 195 and 325 UTF-16 units, including spaces/Chinese names. Source/license/native/subsystem inspection passed. A private-profile native window opened in guest mode, the About page successfully checked the official static catalog, and exit used the normal confirmation/shutdown path. Logs/hashes are ignored local evidence under target/update-checks/windows-20261101/. This is offline package installation plus a live catalog check, not a published GUI archive download or Linux GUI qualification.

## 7. Wrong versus correct
Wrong: helper-based file copying after restart. Correct: core verifies and activates an independent version before reporting installation, then the GUI restarts only after its owned work is joined.
