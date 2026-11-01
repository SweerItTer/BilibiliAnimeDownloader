# Native desktop development

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
