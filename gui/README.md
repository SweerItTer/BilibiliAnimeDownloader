# Native desktop development

## Using the GUI

The BiliDL logo returns to search. Video, Bangumi and Film are the three top-level categories. Search accepts keywords or a video/episode link; category browsing uses its own filters. Click a cover to open details, then select the parts to download. For several results, enable bulk selection and open the same download configuration dialog.

Details place the summary above a full-width episode grid. Single videos and explicit episode links have one download action. Searches, page changes and detail preparation show a spinner and elapsed waiting time until the response arrives; cancellation remains available. Paging and bulk-selection controls are disabled while a request is pending, preserving the requested page and selected content. Cover requests yield to these operations. Temporary connection, interrupted-body and selected server failures have bounded automatic retries; verification and access failures require their corresponding action. A terminal network error includes the available safe stage and HTTP status. Errors with a Retry or verification action stay visible until dismissed or acted on.

The floating download dialog checks each selected part with the chosen account. Episode choices wrap across the dialog's available width. Quality menus list concrete obtainable choices from the returned streams, such as 1080P or 720P; the initial selection is the highest obtainable choice. An explicit common choice that another item cannot provide stays marked until you choose a supported quality for that item or skip it. Advanced options start collapsed and contain the batch rate limit. New drafts inherit the default download rate from Settings (0 means unlimited); changing this draft's rate does not change the default or existing tasks. The rate is shared by the batch. A directory change applies to this batch; “Set as default” separately changes the starting directory for future content. Adding tasks keeps the current workflow open and shows the acknowledged result with a Download History entry. Failed admissions keep their selections for retry.

The two circles at the upper right open the account menu and Download History. The account menu includes account switching and Settings. New logins remain in memory unless you explicitly choose to remember them. Account changes apply to future tasks; an existing failed task can explicitly select replacement credentials. An unknown credential expiry is shown as unknown.

Download History has In Progress, Failed and Completed views, with content-type and added-time filters. Seasons and multipart videos open as projects with their original episode/part order. Paused and interrupted tasks remain in In Progress for manual continuation. Failures show the applicable retry, sidecar or account action and a persistent error. When a supported verification challenge is supplied, the Windows verification window opens only on request and closes on completion, cancellation or timeout. A successful CAPTCHA result is submitted to the service before any success is reported; challenges without a supported payload remain explicit failures.

When continuing a paused download, the byte count includes all safely retained video and audio data, including parts that have not yet resumed. It does not restart its count at zero just because the next transfer begins. If retained data is corrupt or cannot be safely continued, the count reflects the actual discarded bytes rather than hiding a necessary restart.

Completed files can be located in Windows Explorer. Media Information shows actual available media/attachment parameters, with missing values left unknown. Removal distinguishes deleting the record from moving verified owned files to the system Recycle Bin. Modified or unrelated files are retained; recovery of recycled files uses the system Recycle Bin. A project with several output directories offers those actual directories.

These source features have not been published as a GUI release. Local/offline checks do not establish live-site availability or solve a real verification challenge.

Settings offers Light, Dark and System appearance, saved separately from the accent color. System follows the operating system and uses Dark when no system preference is available. Both modes use neutral surfaces and readable text, with accent colors reserved for controls and emphasis. The header and window controls keep a neutral background so they remain readable over photos; the title bar has no duplicate logo or slogan.

Background opacity controls the original image's transparency; partial opacity blends over a neutral base for the current light/dark mode. Background brightness is a separate control for reducing glare. At 100% opacity and 100% brightness, the image retains its source colors. Changing the accent color never tints the image. Crop and blur remain separate choices.

Source quality names and pixel dimensions are separate. For example, a provider's 1080P movie stream can be 1920 × 804 when its widescreen image has no encoded letterboxing. Preparation shows the selected source quality and its known video dimensions; Media Information probes the saved file's actual dimensions. Downloading and remuxing preserve the source pixels without adding black bars or upscaling. An unavailable explicit quality remains an error rather than silently selecting another tier.

## Build and updates

The GUI is developed on this branch and is not part of the published Linux CLI Alpha. Follow [the native Windows build setup](../BUILDING.md#windows-x86_64), then build the GUI and the stable installation entry:

```powershell
cargo build -p gui --release --locked --target x86_64-pc-windows-msvc
cargo rustc -p core-bootstrap --bin bili-bootstrap --features packaged,gui --release --locked --target x86_64-pc-windows-msvc -- -C target-feature=+crt-static
python scripts/package.py --product gui --target x86_64-pc-windows-msvc --profile release --native-prefix target/native/x86_64-pc-windows-msvc --runtime-only
```

The package output is under `target/packages/x86_64-pc-windows-msvc/gui`. Keep the GUI archive, matching FFmpeg source archive, descriptor and checksums together. Extract the complete GUI package into a writable directory and run `bili.exe`. Its stable entry starts the version selected by `CURRENT`; keep `versions/` and the marker intact. The GUI bootstrap uses the Windows GUI subsystem, so Explorer does not open a console beside the application; packaging checks the actual PE header. Rebuild the bootstrap with `--features packaged` before packaging the CLI again, whose entry keeps its console behavior.

On the About page, “Check for updates” reads the same static catalog as the CLI. It only accepts GUI descriptors for this platform. A CLI-only release cannot update a GUI. Stable is the default channel; Beta also considers prereleases. Automatic checks are enabled by default, while automatic installation is opt-in. These settings use the shared `update` block in `gui-preferences.json`:

```json
{"update":{"allow_pre_release":false,"check_updates":true,"auto_update":false}}
```

Installing an update downloads and verifies the archive, starts the new GUI in a headless native check, and only then selects the new version. The current window and jobs continue running on the old version. Restart after installation to use the new one; normal shutdown finishes before the replacement window starts. The old version and user files remain in place. A failed check or installation never selects an unvalidated candidate.

Development executables support checks but cannot install updates. Previous flat GUI packages also need a manual first installation of this versioned layout. No GUI update is available until a release publishes its matching GUI descriptor and package. These changes do not publish a GUI release.

For offline diagnostics without a window or saved login access, run `bili.exe --json doctor`. To inspect a local package with its source bundle, use `python scripts/verify_package.py PACKAGE.zip --product gui --target x86_64-pc-windows-msvc --native-source SOURCE.tar.gz`. Windows acceptance must run natively; this document makes no Linux GUI runtime qualification claim.

Review interactions and appearance by launching the actual GUI executable normally, in a visible window, and using screenshots and Computer Use or scripted clicks and typing. Use its real pages, dialogs and window controls. Do not use an alternate test GUI or offscreen renderer as acceptance evidence. Operations that need unavailable data remain unverified. Ordinary input/state regressions and nonvisual doctor, package and update checks remain separate from inspecting the actual GUI. A visible window does not establish that the previously reported crash is resolved.

For disposable developer debugging, the launcher can give the GUI child process a fresh absolute `BILI_GUI_ISOLATED_DEBUG_DIR`, set `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` to `http://127.0.0.1:0`, and set `NO_PROXY` to an empty value. Verify the restriction with a local-only connection probe before opening the window. The GUI refuses incomplete settings, an existing debug directory or a conflicting `--state-dir`. It uses new state/output folders and a private preferences file, keeps accounts in memory and starts no update worker. Resetting appearance preserves that private settings destination. This restricts the current HTTP clients; it is not a system firewall or a general network sandbox. Keep the environment changes local to the child process. Normal launches use the existing storage and update behavior.

After building the production GUI, run `python scripts/debug_gui.py` from the repository for isolated interaction checks with local response data. In its search field, `ss9001` opens a 24-episode season, `ep9101` a single episode, `av9401` a multipart video and `av9402` a single video. The last episode offers only 720P, episode 23 returns a catalog error, and advertised 4K has no usable stream. These scenarios exercise the normal GUI, Core parsing and strict selection; they do not establish live availability, account rights or a successful download. The script generates everything under ignored `target/`, changes only the child environment and reads no saved account. Advanced use can set `BILI_GUI_OFFLINE_REPLAY` to another bounded local response file, always together with the isolation settings above. Missing or malformed responses never fall back to a service. Core also blocks its HTTP clients explicitly while replay is enabled.
