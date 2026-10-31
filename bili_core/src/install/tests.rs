//! Focused installation-transaction tests: real archives, real filesystem.

use super::archive::safe_relative;
use super::*;
use crate::update::{ArchiveKind, ArchiveRef, Descriptor};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const TARGET: Target = Target::LinuxX86_64Gnu;
const NEW_VERSION: &str = "0.2.0";

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn marker(target: Target) -> Marker {
    Marker {
        schema_version: SCHEMA_VERSION,
        layout_version: LAYOUT_VERSION,
        bootstrap_protocol: BOOTSTRAP_PROTOCOL,
        product: Product::Cli,
        target,
    }
}

/// Payload id the packaging contract derives from a payload digest.
fn derived_id(app: &[u8]) -> String {
    format!("v-{}", &sha(app)[..32])
}

fn identity(version: &str, target: Target) -> Identity {
    Identity::new(Product::Cli, target, version).expect("identity")
}

fn candidate(
    version: &str,
    target: Target,
    format: ArchiveKind,
    archive: &Path,
) -> UpdateCandidate {
    let bytes = fs::read(archive).expect("archive bytes");
    UpdateCandidate {
        tag: format!("v{version}"),
        version: Version::parse(version).expect("version"),
        channel: crate::update::Channel::Stable,
        descriptor: Descriptor {
            schema_version: crate::update::PROTOCOL,
            product: Product::Cli,
            target,
            version: Version::parse(version).expect("version"),
            archive: ArchiveRef {
                name: if format == ArchiveKind::Zip {
                    "bili-release.zip".into()
                } else {
                    "bili-release.tar.gz".into()
                },
                format,
                sha256: sha(&bytes),
                size: bytes.len() as u64,
            },
        },
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
    fs::write(path, bytes).expect("write");
}

/// Write a `SHA256SUMS` inventory for every non-directory entry under `tree`.
fn write_inventory(tree: &Path, name: &str) {
    let mut lines = Vec::new();
    collect(tree, Path::new(""), &mut |relative, absolute| {
        if relative == Path::new(name) {
            return;
        }
        let (_, digest) = hash_file(absolute).expect("hash");
        lines.push(format!(
            "{digest}  {}",
            relative.to_string_lossy().replace('\\', "/")
        ));
    });
    lines.sort();
    write(
        &tree.join(name),
        format!("{}\n", lines.join("\n")).as_bytes(),
    );
}

fn collect(tree: &Path, relative: &Path, visit: &mut impl FnMut(&Path, &Path)) {
    for entry in fs::read_dir(tree.join(relative)).expect("read dir") {
        let entry = entry.expect("entry");
        let child = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(tree.join(&child)).expect("metadata");
        if metadata.is_dir() {
            collect(tree, &child, visit);
        } else {
            visit(&child, &tree.join(&child));
        }
    }
}

/// Build a complete payload directory (entry, build info, inventory).
fn payload(dir: &Path, target: Target, version: &str, app: &[u8], product: &str) {
    let entry = dir.join(payload_entry(target));
    write(&entry, app);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o755)).expect("mode");
    }
    let info = serde_json::json!({
        "schema_version": 1,
        "target": target.triple(),
        "profile": "release",
        "app_version": version,
        "app_sha256": sha(app),
        "product": product,
        "update_protocol": UPDATE_PROTOCOL,
        "layout_version": LAYOUT_VERSION,
        "bootstrap_protocol": BOOTSTRAP_PROTOCOL,
    });
    write(&dir.join("BUILD_INFO.json"), info.to_string().as_bytes());
    write_inventory(dir, "SHA256SUMS");
}

/// Package tree the release job ships: marker, CURRENT, one payload directory.
fn package(dir: &Path, target: Target, version: &str, app: &[u8]) {
    let id = derived_id(app);
    write(
        &dir.join(MARKER_FILE),
        &serde_json::to_vec(&marker(target)).expect("marker json"),
    );
    write(&dir.join(CURRENT_FILE), format!("{id}\n").as_bytes());
    payload(
        &dir.join(VERSIONS_DIR).join(&id),
        target,
        version,
        app,
        "cli",
    );
    write_inventory(dir, "SHA256SUMS");
}

fn archive_tar(source: &Path, top: &str, out: &Path) {
    let file = File::create(out).expect("archive");
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all(top, source).expect("append tree");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip");
}

fn archive_zip(source: &Path, top: &str, out: &Path) {
    let mut writer = zip::ZipWriter::new(File::create(out).expect("archive"));
    let options = zip::write::SimpleFileOptions::default();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        for entry in fs::read_dir(source.join(&relative)).expect("read dir") {
            let entry = entry.expect("entry");
            let child = relative.join(entry.file_name());
            let absolute = source.join(&child);
            if absolute.is_dir() {
                stack.push(child);
                continue;
            }
            writer
                .start_file(
                    format!("{top}/{}", child.to_string_lossy().replace('\\', "/")),
                    options,
                )
                .expect("zip entry");
            writer
                .write_all(&fs::read(&absolute).expect("payload bytes"))
                .expect("zip write");
        }
    }
    writer.finish().expect("finish zip");
}

/// Installed tree with an existing payload, pointer and unrelated user files.
struct Installed {
    directory: TempDir,
    root: PathBuf,
    _old_id: String,
    old_app: Vec<u8>,
}

impl Installed {
    fn new(target: Target) -> Self {
        let directory = TempDir::new().expect("tempdir");
        let root = directory.path().join("bili");
        let old_id = derived_id(b"old payload");
        write(
            &root.join(MARKER_FILE),
            &serde_json::to_vec(&marker(target)).expect("marker json"),
        );
        write(&root.join(CURRENT_FILE), format!("{old_id}\n").as_bytes());
        payload(
            &root.join(VERSIONS_DIR).join(&old_id),
            target,
            "0.1.0",
            b"old payload",
            "cli",
        );
        write(&root.join("downloads/movie.mp4"), b"user data");
        Self {
            directory,
            root,
            _old_id: old_id,
            old_app: b"old payload".to_vec(),
        }
    }

    fn exe(&self, target: Target) -> PathBuf {
        self.root
            .join(VERSIONS_DIR)
            .join(&self._old_id)
            .join(payload_entry(target))
    }

    fn current(&self) -> String {
        fs::read_to_string(self.root.join(CURRENT_FILE)).expect("current")
    }

    fn versions(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.root.join(VERSIONS_DIR))
            .expect("versions")
            .filter_map(|entry| {
                let entry = entry.expect("entry");
                let name = entry.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.')).then_some(name)
            })
            .collect();
        names.sort();
        names
    }

    fn assert_unchanged_by_failure(&self) {
        assert_eq!(self.current(), format!("{}\n", self._old_id));
        assert_eq!(self.versions(), vec![self._old_id.clone()]);
        assert_eq!(
            fs::read(
                self.root
                    .join(VERSIONS_DIR)
                    .join(&self._old_id)
                    .join(payload_entry(TARGET))
            )
            .expect("old payload"),
            self.old_app
        );
        assert_eq!(
            fs::read(self.root.join("downloads/movie.mp4")).expect("user data"),
            b"user data"
        );
        self.assert_staging_clean();
    }

    fn staging(&self) -> PathBuf {
        self.root.join(UPDATE_DIR)
    }

    /// No transaction staging survives a failure that happened inside it.
    fn assert_staging_clean(&self) {
        let staging = self.staging();
        if staging.exists() {
            assert_eq!(
                fs::read_dir(&staging).expect("staging").count(),
                0,
                "staging is cleaned after a failed transaction"
            );
        }
    }
}

/// Package the fixture into `archive` and return the matching candidate.
fn staged(
    installed: &Installed,
    version: &str,
    app: &[u8],
    top: &str,
) -> (PathBuf, UpdateCandidate) {
    let work = TempDir::new().expect("tempdir");
    let source = work.path().join("pkg");
    package(&source, TARGET, version, app);
    let archive = installed.directory.path().join("release.tar.gz");
    archive_tar(&source, top, &archive);
    let candidate = candidate(version, TARGET, ArchiveKind::TarGz, &archive);
    (archive, candidate)
}

fn tar_with(path: &Path, entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) {
    let file = File::create(path).expect("archive");
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, kind, bytes, link) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        match kind {
            tar::EntryType::Symlink | tar::EntryType::Link => builder
                .append_link(&mut header, name, link.expect("link target"))
                .expect("append link"),
            _ => builder
                .append_data(&mut header, name, &bytes[..])
                .expect("append data"),
        }
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip");
}

/// Write a gzip'd tar whose member names are not filtered by the tar builder.
fn raw_tar(path: &Path, entries: &[(&str, &[u8])]) {
    let mut body = Vec::new();
    for (name, data) in entries {
        let mut header = [0u8; 512];
        let bytes = name.as_bytes();
        header[..bytes.len()].copy_from_slice(bytes);
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156..157].copy_from_slice(b"0");
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        body.extend_from_slice(&header);
        body.extend_from_slice(data);
        body.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
    }
    body.extend_from_slice(&[0u8; 1024]);
    let mut encoder = flate2::write::GzEncoder::new(
        File::create(path).expect("archive"),
        flate2::Compression::fast(),
    );
    encoder.write_all(&body).expect("raw tar");
    encoder.finish().expect("finish gzip");
}

#[test]
fn unsafe_member_names_are_rejected() {
    for name in [
        ("parent", "../escape.txt"),
        ("absolute", "/etc/escape.txt"),
        ("inner-parent", "top/../../escape.txt"),
    ]
    .map(|(_, name)| name)
    {
        assert!(safe_relative(Path::new(name)).is_none(), "{name}");
    }
    for name in ["top/file", "top/bin/bili", "bili.exe"] {
        assert!(safe_relative(Path::new(name)).is_some(), "{name}");
    }
    assert!(safe_relative(Path::new("")).is_none());
    assert!(safe_relative(Path::new("./top")).is_none());
}

#[test]
fn install_commits_staged_payload_and_keeps_every_other_file() {
    let installed = Installed::new(TARGET);
    let app = b"new payload";
    let (archive, candidate) = staged(
        &installed,
        NEW_VERSION,
        app,
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );
    let (_, old_digest) = hash_file(
        &installed
            .root
            .join(VERSIONS_DIR)
            .join(&installed._old_id)
            .join(payload_entry(TARGET)),
    )
    .expect("old hash");

    let result = install_payload(
        &installed.exe(TARGET),
        &identity("0.1.0", TARGET),
        &candidate,
        &archive,
        |staged| {
            assert_eq!(staged.identity(), &identity(NEW_VERSION, TARGET));
            assert_eq!(
                fs::read(staged.entry()).expect("staged entry"),
                app,
                "validator sees the staged payload before commit"
            );
            Ok(())
        },
    )
    .expect("install");

    assert_eq!(
        result.version,
        Version::parse(NEW_VERSION).expect("version")
    );
    assert_eq!(installed.current(), format!("{}\n", result.id));
    let new_entry = installed
        .root
        .join(VERSIONS_DIR)
        .join(result.id.as_str())
        .join(payload_entry(TARGET));
    assert_eq!(fs::read(&new_entry).expect("new payload"), app);

    // Previous payload and unrelated files are untouched.
    let versions = installed.versions();
    assert_eq!(versions.len(), 2);
    assert!(versions.contains(&installed._old_id));
    let (_, digest) = hash_file(
        &installed
            .root
            .join(VERSIONS_DIR)
            .join(&installed._old_id)
            .join(payload_entry(TARGET)),
    )
    .expect("old hash");
    assert_eq!(digest, old_digest);
    assert_eq!(
        fs::read(installed.root.join("downloads/movie.mp4")).expect("user data"),
        b"user data"
    );
    assert_eq!(
        fs::read_dir(installed.staging()).expect("staging").count(),
        0,
        "staging is cleaned after a successful transaction"
    );
}

#[test]
fn digest_or_size_mismatch_is_rejected_before_any_change() {
    let installed = Installed::new(TARGET);
    let (archive, mut candidate) = staged(
        &installed,
        NEW_VERSION,
        b"new",
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );

    let mut wrong_digest = candidate.clone();
    wrong_digest.descriptor.archive.sha256 = "0".repeat(64);
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &wrong_digest,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::ArchiveDigest)
    );

    candidate.descriptor.archive.size += 1;
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::ArchiveSize)
    );
    installed.assert_unchanged_by_failure();
}

#[test]
fn validator_failure_keeps_the_previous_pointer_and_payload() {
    let installed = Installed::new(TARGET);
    let (archive, candidate) = staged(
        &installed,
        NEW_VERSION,
        b"new",
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );
    let result = install_payload(
        &installed.exe(TARGET),
        &identity("0.1.0", TARGET),
        &candidate,
        &archive,
        |_| Err(InstallError::Validator),
    );
    assert_eq!(result, Err(InstallError::Validator));
    installed.assert_unchanged_by_failure();
}

#[test]
fn pointer_write_failure_leaves_no_new_payload_behind() {
    let installed = Installed::new(TARGET);
    let (archive, candidate) = staged(
        &installed,
        NEW_VERSION,
        b"new",
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );

    let result = install_payload(
        &installed.exe(TARGET),
        &identity("0.1.0", TARGET),
        &candidate,
        &archive,
        |_| {
            // Fail activation after the locked CURRENT check has succeeded.
            fs::remove_file(installed.root.join(CURRENT_FILE)).expect("remove pointer");
            fs::create_dir(installed.root.join(CURRENT_FILE)).expect("blocking directory");
            write(&installed.root.join(CURRENT_FILE).join("keep"), b"x");
            Ok(())
        },
    );
    assert_eq!(result, Err(InstallError::Filesystem));
    assert_eq!(
        installed.versions(),
        vec![installed._old_id.clone()],
        "no payload is left behind when the commit rename fails"
    );
    assert!(installed.root.join(CURRENT_FILE).is_dir());
}

#[test]
fn traversal_hardlink_and_link_escapes_are_rejected() {
    let installed = Installed::new(TARGET);
    for (label, name) in [
        ("parent.tar.gz", "../escape.txt"),
        ("absolute.tar.gz", "/etc/escape.txt"),
        ("inner-parent.tar.gz", "top/../../escape.txt"),
    ] {
        let archive = installed.directory.path().join(label);
        raw_tar(&archive, &[(name, b"x")]);
        let candidate = candidate("0.2.0", TARGET, ArchiveKind::TarGz, &archive);
        assert_eq!(
            install_payload(
                &installed.exe(TARGET),
                &identity("0.1.0", TARGET),
                &candidate,
                &archive,
                |_| Ok(())
            ),
            Err(InstallError::ArchiveLayout),
            "{label}"
        );
        assert!(!installed.directory.path().join("escape.txt").exists());
        installed.assert_unchanged_by_failure();
    }

    for (label, entries) in [
        (
            "hardlink.tar.gz",
            vec![(
                "top/file",
                tar::EntryType::Link,
                &b""[..],
                Some("top/other"),
            )],
        ),
        (
            "escape-link.tar.gz",
            vec![(
                "top/link",
                tar::EntryType::Symlink,
                &b""[..],
                Some("../../etc/passwd"),
            )],
        ),
        (
            "write-through.tar.gz",
            vec![
                ("top/dir", tar::EntryType::Symlink, &b""[..], Some(".")),
                ("top/dir/file", tar::EntryType::Regular, &b"x"[..], None),
            ],
        ),
    ] {
        let archive = installed.directory.path().join(label);
        tar_with(&archive, &entries);
        let candidate = candidate("0.2.0", TARGET, ArchiveKind::TarGz, &archive);
        let result = install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(()),
        );
        assert_eq!(result, Err(InstallError::ArchiveLayout), "{label}");
        installed.assert_unchanged_by_failure();
    }
}

#[test]
fn inventory_mismatches_are_rejected() {
    let installed = Installed::new(TARGET);
    let work = TempDir::new().expect("tempdir");

    for (label, damage) in [("tampered", 0u8), ("extra", 1u8), ("missing", 2u8)] {
        let source = work.path().join(label);
        package(&source, TARGET, NEW_VERSION, b"new");
        let payload_dir = source.join(VERSIONS_DIR).join(derived_id(b"new"));
        match damage {
            0 => {
                write(&payload_dir.join("bin/bili"), b"tampered");
            }
            1 => {
                write(&source.join("unexpected.txt"), b"not in inventory");
            }
            _ => {
                fs::remove_file(payload_dir.join("BUILD_INFO.json")).expect("remove");
            }
        }
        let archive = work.path().join(format!("{label}.tar.gz"));
        archive_tar(&source, "bili-0.2.0-x86_64-unknown-linux-gnu", &archive);
        let candidate = candidate("0.2.0", TARGET, ArchiveKind::TarGz, &archive);
        assert_eq!(
            install_payload(
                &installed.exe(TARGET),
                &identity("0.1.0", TARGET),
                &candidate,
                &archive,
                |_| Ok(())
            ),
            Err(InstallError::InventoryMismatch),
            "{label}"
        );
        installed.assert_unchanged_by_failure();
    }
}

#[test]
fn payload_metadata_must_match_the_expected_identity() {
    let installed = Installed::new(TARGET);
    let work = TempDir::new().expect("tempdir");

    for (label, product, version, target) in [
        ("product", "gui", "0.2.0", TARGET),
        ("version", "cli", "0.3.0", TARGET),
        ("target", "cli", "0.2.0", Target::WindowsX86_64Msvc),
    ] {
        let source = work.path().join(label);
        let id = derived_id(b"new");
        write(
            &source.join(MARKER_FILE),
            &serde_json::to_vec(&marker(TARGET)).expect("marker"),
        );
        write(&source.join(CURRENT_FILE), format!("{id}\n").as_bytes());
        payload(
            &source.join(VERSIONS_DIR).join(&id),
            target,
            version,
            b"new",
            product,
        );
        write_inventory(&source, "SHA256SUMS");
        let archive = work.path().join(format!("{label}.tar.gz"));
        archive_tar(&source, &format!("bili-{label}"), &archive);
        let candidate = candidate("0.2.0", TARGET, ArchiveKind::TarGz, &archive);
        let result = install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(()),
        );
        assert!(result.is_err(), "{label} mismatch must be rejected");
        installed.assert_unchanged_by_failure();
    }
}

#[test]
fn payload_id_must_derive_from_the_payload_digest() {
    let installed = Installed::new(TARGET);
    let work = TempDir::new().expect("tempdir");
    let source = work.path().join("pkg");
    let id = "v-0123456789abcdef0123456789abcdef";
    write(
        &source.join(MARKER_FILE),
        &serde_json::to_vec(&marker(TARGET)).expect("marker"),
    );
    write(&source.join(CURRENT_FILE), format!("{id}\n").as_bytes());
    payload(
        &source.join(VERSIONS_DIR).join(id),
        TARGET,
        NEW_VERSION,
        b"new",
        "cli",
    );
    write_inventory(&source, "SHA256SUMS");
    let archive = work.path().join("mismatch.tar.gz");
    archive_tar(&source, "bili-0.2.0-x86_64-unknown-linux-gnu", &archive);
    let candidate = candidate(NEW_VERSION, TARGET, ArchiveKind::TarGz, &archive);
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::PayloadIdentity)
    );
    installed.assert_unchanged_by_failure();
}

#[test]
fn incompatible_protocols_are_rejected() {
    let installed = Installed::new(TARGET);
    let work = TempDir::new().expect("tempdir");
    let source = work.path().join("pkg");
    let id = derived_id(b"new");
    let mut newer = marker(TARGET);
    newer.bootstrap_protocol = BOOTSTRAP_PROTOCOL + 1;
    write(
        &source.join(MARKER_FILE),
        &serde_json::to_vec(&newer).expect("marker"),
    );
    write(&source.join(CURRENT_FILE), format!("{id}\n").as_bytes());
    payload(
        &source.join(VERSIONS_DIR).join(&id),
        TARGET,
        NEW_VERSION,
        b"new",
        "cli",
    );
    write_inventory(&source, "SHA256SUMS");
    let archive = work.path().join("protocol.tar.gz");
    archive_tar(&source, "bili-0.2.0-x86_64-unknown-linux-gnu", &archive);
    let candidate = candidate(NEW_VERSION, TARGET, ArchiveKind::TarGz, &archive);
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::ProtocolIncompatible)
    );
    installed.assert_unchanged_by_failure();
}

#[test]
fn downgrade_and_unknown_installs_are_rejected() {
    let installed = Installed::new(TARGET);
    let (archive, candidate) = staged(
        &installed,
        "0.1.0",
        b"new",
        "bili-0.1.0-x86_64-unknown-linux-gnu",
    );
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::Downgrade)
    );

    let loose = installed.directory.path().join("loose/bin/bili");
    write(&loose, b"dev build");
    let (archive, candidate) = staged(
        &installed,
        NEW_VERSION,
        b"new",
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );
    assert_eq!(
        install_payload(
            &loose,
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::NotPackaged)
    );
    installed.assert_unchanged_by_failure();
}

#[test]
fn a_held_lock_is_busy() {
    let installed = Installed::new(TARGET);
    let (archive, candidate) = staged(
        &installed,
        NEW_VERSION,
        b"new",
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(installed.root.join(VERSIONS_DIR).join(".update.lock"))
        .expect("lock file");
    lock.lock_exclusive().expect("external lock");
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::Busy)
    );
    installed.assert_unchanged_by_failure();
    lock.unlock().expect("unlock");
}

#[test]
fn an_existing_payload_directory_is_never_reused() {
    let installed = Installed::new(TARGET);
    let app = b"new payload";
    let (archive, candidate) = staged(
        &installed,
        NEW_VERSION,
        app,
        "bili-0.2.0-x86_64-unknown-linux-gnu",
    );
    let occupied = installed.root.join(VERSIONS_DIR).join(derived_id(app));
    fs::create_dir_all(&occupied).expect("occupied");
    write(&occupied.join("keep"), b"other");

    let result = install_payload(
        &installed.exe(TARGET),
        &identity("0.1.0", TARGET),
        &candidate,
        &archive,
        |_| Ok(()),
    )
    .expect("install");
    assert_ne!(result.id.as_str(), derived_id(app));
    assert_eq!(fs::read(occupied.join("keep")).expect("kept"), b"other");
    assert_eq!(installed.current(), format!("{}\n", result.id));
}

#[test]
fn zip_payload_for_the_windows_target_installs() {
    const WINDOWS: Target = Target::WindowsX86_64Msvc;
    let installed = Installed::new(WINDOWS);
    let work = TempDir::new().expect("tempdir");
    let source = work.path().join("pkg");
    package(&source, WINDOWS, NEW_VERSION, b"new payload");
    let archive = installed.directory.path().join("release.zip");
    archive_zip(&source, "bili-0.2.0-x86_64-pc-windows-msvc", &archive);
    let candidate = candidate(NEW_VERSION, WINDOWS, ArchiveKind::Zip, &archive);

    let result = install_payload(
        &installed.exe(WINDOWS),
        &identity("0.1.0", WINDOWS),
        &candidate,
        &archive,
        |staged| {
            assert_eq!(
                fs::read(staged.entry()).expect("staged entry"),
                b"new payload"
            );
            Ok(())
        },
    )
    .expect("install");
    assert_eq!(installed.current(), format!("{}\n", result.id));
    assert_eq!(
        fs::read(
            installed
                .root
                .join(VERSIONS_DIR)
                .join(result.id.as_str())
                .join("bili.exe")
        )
        .expect("new payload"),
        b"new payload"
    );
}

#[test]
fn pointer_grammar_accepts_one_newline_only() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join(CURRENT_FILE);
    let id = "v-0123456789abcdef0123456789abcdef";

    for accepted in [format!("{id}\n"), format!("{id}\r\n"), id.to_owned()] {
        fs::write(&path, accepted.as_bytes()).expect("write");
        assert_eq!(read_current(&path).expect("parse").as_str(), id);
    }
    for rejected in [
        format!("  {id}\n"),
        format!("{id}\n\n"),
        format!("{id} "),
        format!("{}\n", id.to_uppercase()),
        "v-0123\n".to_owned(),
        format!("versions/{id}\n"),
        format!("{id}\u{0}\n"),
        String::new(),
    ] {
        fs::write(&path, rejected.as_bytes()).expect("write");
        assert!(
            read_current(&path).is_err(),
            "{rejected:?} must be rejected"
        );
    }
    assert_eq!(
        VersionId::parse("v-0123456789abcdef0123456789abcdef")
            .expect("parse")
            .as_str(),
        id
    );
    assert_eq!(VersionId::random().as_str().len(), 34);
}

#[cfg(unix)]
#[test]
fn archive_file_links_can_precede_targets_but_must_resolve() {
    for target in ["libcodec.so.1", "missing.so"] {
        let work = TempDir::new().expect("tempdir");
        let archive = work.path().join("links.tar.gz");
        tar_with(
            &archive,
            &[
                (
                    "top/libcodec.so",
                    tar::EntryType::Symlink,
                    b"".as_ref(),
                    Some(target),
                ),
                (
                    "top/libcodec.so.1",
                    tar::EntryType::Regular,
                    b"library bytes".as_ref(),
                    None,
                ),
            ],
        );
        let tree = archive::extract(
            &archive,
            ArchiveKind::TarGz,
            &work.path().join("extracted"),
            TARGET,
        )
        .expect("extract entries before resolving file links");
        if target == "missing.so" {
            assert_eq!(
                archive::check_links(&tree),
                Err(InstallError::ArchiveLayout)
            );
        } else {
            archive::check_links(&tree).expect("contained file link");
            assert_eq!(
                fs::read(tree.join("libcodec.so")).expect("linked library"),
                b"library bytes"
            );
        }
    }
}

#[test]
fn symlinks_inside_a_payload_are_verified_through_their_targets() {
    let installed = Installed::new(TARGET);
    let work = TempDir::new().expect("tempdir");
    let source = work.path().join("pkg");
    let app = b"new payload";
    let id = derived_id(app);
    write(
        &source.join(MARKER_FILE),
        &serde_json::to_vec(&marker(TARGET)).expect("marker"),
    );
    write(&source.join(CURRENT_FILE), format!("{id}\n").as_bytes());
    let payload_dir = source.join(VERSIONS_DIR).join(&id);
    payload(&payload_dir, TARGET, NEW_VERSION, app, "cli");
    #[cfg(unix)]
    std::os::unix::fs::symlink("bili", payload_dir.join("bin/bili-alias")).expect("symlink");
    write_inventory(&payload_dir, "SHA256SUMS");
    write_inventory(&source, "SHA256SUMS");
    let archive = work.path().join("links.tar.gz");
    archive_tar(&source, "bili-0.2.0-x86_64-unknown-linux-gnu", &archive);
    let linked_candidate = candidate(NEW_VERSION, TARGET, ArchiveKind::TarGz, &archive);

    install_payload(
        &installed.exe(TARGET),
        &identity("0.1.0", TARGET),
        &linked_candidate,
        &archive,
        |_| Ok(()),
    )
    .expect("install");

    // An escaping link inside an otherwise valid payload is refused.
    let escaping = work.path().join("escape-pkg");
    write(
        &escaping.join(MARKER_FILE),
        &serde_json::to_vec(&marker(TARGET)).expect("marker"),
    );
    write(&escaping.join(CURRENT_FILE), format!("{id}\n").as_bytes());
    let archive = work.path().join("escape-links.tar.gz");
    tar_with(
        &archive,
        &[
            (
                "top/SHA256SUMS",
                tar::EntryType::Regular,
                b""[..].as_ref(),
                None,
            ),
            (
                "top/link",
                tar::EntryType::Symlink,
                b"".as_ref(),
                Some("../outside"),
            ),
        ],
    );
    let escaping_candidate = candidate(NEW_VERSION, TARGET, ArchiveKind::TarGz, &archive);
    assert!(install_payload(
        &installed.exe(TARGET),
        &identity("0.1.0", TARGET),
        &escaping_candidate,
        &archive,
        |_| Ok(())
    )
    .is_err());
}

#[test]
fn stale_process_cannot_replace_a_newer_selected_payload() {
    let installed = Installed::new(TARGET);
    let running = identity("0.1.0", TARGET);
    let (archive, newest) = staged(&installed, "0.3.0", b"newest", "package");
    install_payload(&installed.exe(TARGET), &running, &newest, &archive, |_| {
        Ok(())
    })
    .expect("first updater commits");
    let selected = installed.current();
    let versions = installed.versions();
    for version in ["0.2.0", "0.3.0", "0.3.0+different-build"] {
        let (archive, older) = staged(&installed, version, b"stale candidate", "package");
        assert_eq!(
            install_payload(&installed.exe(TARGET), &running, &older, &archive, |_| {
                panic!("stale candidate must not reach validation")
            }),
            Err(InstallError::Downgrade),
            "{version}"
        );
        assert_eq!(installed.current(), selected);
        assert_eq!(installed.versions(), versions);
        installed.assert_staging_clean();
    }
}

#[test]
fn invalid_selected_payload_refuses_update_without_staging() {
    let installed = Installed::new(TARGET);
    let (archive, candidate) = staged(&installed, NEW_VERSION, b"new", "package");
    write(&installed.root.join(CURRENT_FILE), b"invalid pointer");
    assert_eq!(
        install_payload(
            &installed.exe(TARGET),
            &identity("0.1.0", TARGET),
            &candidate,
            &archive,
            |_| Ok(())
        ),
        Err(InstallError::CurrentInvalid)
    );
    assert_eq!(installed.current(), "invalid pointer");
    installed.assert_staging_clean();
}
