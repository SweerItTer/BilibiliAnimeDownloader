# Native desktop development

## Using the GUI

The BiliDL logo returns to search. Video, Bangumi and Film are the three top-level categories. Search accepts keywords or a video/episode link; category browsing uses its own filters. Click a cover to open details, then select the parts to download. For several results, enable bulk selection and open the same download configuration dialog.

The floating download dialog checks each selected part with the chosen account. Quality menus list concrete obtainable choices from the returned streams, such as 1080P or 720P; the initial selection is the highest obtainable choice. An explicit common choice that another item cannot provide stays marked until you choose a supported quality for that item or skip it. Advanced options start collapsed and contain the batch rate limit. New drafts inherit the default download rate from Settings (0 means unlimited); changing this draft's rate does not change the default or existing tasks. The rate is shared by the batch. A directory change applies to this batch; “Set as default” separately changes the starting directory for future content. Adding tasks keeps the current workflow open and shows the acknowledged result with a Download History entry. Failed admissions keep their selections for retry.

The two circles at the upper right open the account menu and Download History. The account menu includes account switching and Settings. New logins remain in memory unless you explicitly choose to remember them. Account changes apply to future tasks; an existing failed task can explicitly select replacement credentials. An unknown credential expiry is shown as unknown.

Download History has In Progress, Failed and Completed views, with content-type and added-time filters. Seasons and multipart videos open as projects with their original episode/part order. Paused and interrupted tasks remain in In Progress for manual continuation. Failures show the applicable retry, sidecar or account action and a persistent error. When a supported verification challenge is supplied, the Windows verification window opens only on request and closes on completion, cancellation or timeout. A successful CAPTCHA result is submitted to the service before any success is reported; challenges without a supported payload remain explicit failures.

Completed files can be located in Windows Explorer. Media Information shows actual available media/attachment parameters, with missing values left unknown. Removal distinguishes deleting the record from moving verified owned files to the system Recycle Bin. Modified or unrelated files are retained; recovery of recycled files uses the system Recycle Bin. A project with several output directories offers those actual directories.

These source features have not been published as a GUI release. Local/offline checks do not establish live-site availability or solve a real verification challenge.

Background opacity controls the original image's transparency. At 100% it retains its source colors; partial opacity blends over a neutral background. Theme changes affect interface controls without tinting the image. Crop and blur remain separate choices.

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
