//! Anonymous release discovery and reminder bookkeeping shared by every shell.
//!
//! This module decides what a shell may offer to install, determines whether a
//! user-visible reminder is due, and exposes `Updater::download` for archive
//! retrieval. It does not extract or replace a running installation or spawn
//! its own task: the caller owns the runtime, one-second budget and install transaction.
//!
//! Trust boundary: the compiled product, target and version are supplied by the
//! shell. A remote descriptor is only ever *checked against* that identity, so
//! remote metadata can never widen what the shell is willing to install.

mod release;
mod reminder;

pub use release::Updater;
pub use reminder::{CheckPermit, Notified, ReminderState, ReminderStore};

use semver::Version;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Release-descriptor schema understood by this build.
pub const PROTOCOL: u32 = 1;
/// Minimum interval between automatic checks.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// Typed update failure. Every variant carries only local, static information:
/// remote text, URLs, headers and bodies never reach a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateError {
    /// Request could not be sent, the connection failed or the body was not JSON.
    Transport,
    /// Response carried a status that is neither success nor a rate-limit reply.
    HttpStatus(u16),
    /// A transfer exceeded its idle or total time budget.
    Timeout,
    /// Upstream refused the request because the anonymous budget is exhausted.
    RateLimited,
    /// More release pages exist than this build is willing to enumerate.
    TooManyReleases,
    /// A remote document exceeded its size bound.
    ResponseTooLarge,
    /// Descriptor is not valid JSON or misses a required field.
    MalformedDescriptor,
    /// A release shipped more than one descriptor for this product and target.
    AmbiguousDescriptor,
    /// Descriptor schema is newer than this build understands.
    UnsupportedSchema(u32),
    /// Descriptor belongs to a different product.
    ProductMismatch,
    /// Descriptor belongs to a different target.
    TargetMismatch,
    /// Descriptor version disagrees with its release tag.
    VersionMismatch,
    /// Archive name could escape the download directory or is not a plain name.
    UnsafeAssetName,
    /// Archive format is impossible for the target platform.
    ArchiveKindMismatch,
    /// Archive digest is not a lowercase SHA-256 hex string.
    DigestMismatch,
    /// Archive size is zero, above the bound, or disagrees with the release.
    SizeMismatch,
    /// Descriptor names an archive the release does not publish.
    MissingAsset,
    /// A release advertises more than one asset with the descriptor's name.
    AmbiguousAsset,
    /// Reminder state is locked by another process right now.
    Busy,
    /// Local filesystem access failed.
    Filesystem,
    /// Supplied product, target or version text is unusable.
    InvalidInput,
}

impl UpdateError {
    /// HTTP status observed for the failure, when a response was received.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::HttpStatus(status) => Some(*status),
            _ => None,
        }
    }

    /// Failure a later retry may resolve without any local change.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport
                | Self::Timeout
                | Self::RateLimited
                | Self::Busy
                | Self::HttpStatus(500..=599)
        )
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Transport => "update transport failure",
            Self::HttpStatus(_) => "update response status",
            Self::Timeout => "update transfer exceeded its time budget",
            Self::RateLimited => "update anonymous quota exhausted",
            Self::TooManyReleases => "release listing exceeds the supported page limit",
            Self::ResponseTooLarge => "update response exceeds its size bound",
            Self::MalformedDescriptor => "update descriptor is malformed",
            Self::AmbiguousDescriptor => "release publishes conflicting descriptors",
            Self::UnsupportedSchema(_) => "update descriptor schema is unsupported",
            Self::ProductMismatch => "update descriptor belongs to another product",
            Self::TargetMismatch => "update descriptor belongs to another target",
            Self::VersionMismatch => "update descriptor version disagrees with its tag",
            Self::UnsafeAssetName => "update asset name is not a safe plain name",
            Self::ArchiveKindMismatch => "update archive format is impossible for the target",
            Self::DigestMismatch => "update archive digest is not a SHA-256 hex string",
            Self::SizeMismatch => "update archive size is implausible or disagrees",
            Self::MissingAsset => "update descriptor names an unpublished archive",
            Self::AmbiguousAsset => "release publishes duplicate archive assets",
            Self::Busy => "update reminder state is busy",
            Self::Filesystem => "update local filesystem failure",
            Self::InvalidInput => "update identity input is invalid",
        };
        write!(formatter, "{text}")?;
        if let Self::HttpStatus(status) = self {
            write!(formatter, " ({status})")?;
        }
        Ok(())
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(_: std::io::Error) -> Self {
        Self::Filesystem
    }
}

/// Application shell that owns an installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Product {
    Cli,
    Gui,
    Tui,
}

impl Product {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Gui => "gui",
            Self::Tui => "tui",
        }
    }
}

/// Operating-system target this installation was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "x86_64-unknown-linux-gnu")]
    LinuxX86_64Gnu,
    #[serde(rename = "x86_64-pc-windows-msvc")]
    WindowsX86_64Msvc,
}

impl Target {
    pub const fn triple(self) -> &'static str {
        match self {
            Self::LinuxX86_64Gnu => "x86_64-unknown-linux-gnu",
            Self::WindowsX86_64Msvc => "x86_64-pc-windows-msvc",
        }
    }

    pub fn from_triple(triple: &str) -> Option<Self> {
        match triple {
            "x86_64-unknown-linux-gnu" => Some(Self::LinuxX86_64Gnu),
            "x86_64-pc-windows-msvc" => Some(Self::WindowsX86_64Msvc),
            _ => None,
        }
    }

    /// The only archive format a release for this target may ship.
    pub const fn archive_kind(self) -> ArchiveKind {
        match self {
            Self::LinuxX86_64Gnu => ArchiveKind::TarGz,
            Self::WindowsX86_64Msvc => ArchiveKind::Zip,
        }
    }
}

/// Archive container used by a product package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveKind {
    #[serde(rename = "tar.gz")]
    TarGz,
    #[serde(rename = "zip")]
    Zip,
}

impl ArchiveKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

/// Release channel a shell is willing to move to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Channel {
    #[serde(rename = "stable")]
    Stable,
    #[serde(rename = "pre-release")]
    PreRelease,
}

impl Channel {
    /// Channel implied by a version's pre-release segment.
    pub fn of(version: &Version) -> Self {
        if version.pre.is_empty() {
            Self::Stable
        } else {
            Self::PreRelease
        }
    }

    /// Whether a version belongs to this channel.
    pub fn accepts(self, version: &Version) -> bool {
        match self {
            Self::Stable => version.pre.is_empty(),
            Self::PreRelease => true,
        }
    }
}

/// Update preferences owned by shell configuration.
///
/// Unknown fields are tolerated so a newer GUI may share one configuration
/// file with an older CLI; `auto_update` gates check-only TTY reminders, never
/// an automatic download or install, which still requires an explicit update command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    pub allow_pre_release: bool,
    pub check_updates: bool,
    pub auto_update: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            allow_pre_release: false,
            check_updates: true,
            auto_update: false,
        }
    }
}

impl UpdateSettings {
    /// Channel these preferences select.
    pub fn channel(self) -> Channel {
        if self.allow_pre_release {
            Channel::PreRelease
        } else {
            Channel::Stable
        }
    }
}

/// Compiled identity of the running installation.
///
/// The version text is parsed exactly once; comparisons reuse the parsed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    product: Product,
    target: Target,
    version: Version,
}

impl Identity {
    pub fn new(product: Product, target: Target, version: &str) -> Result<Self, UpdateError> {
        let version = Version::parse(version).map_err(|_| UpdateError::InvalidInput)?;
        Ok(Self {
            product,
            target,
            version,
        })
    }

    pub fn product(&self) -> Product {
        self.product
    }

    pub fn target(&self) -> Target {
        self.target
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Release asset that describes this product and target.
    pub fn descriptor_name(&self) -> String {
        format!(
            "bili-update-{}-{}.json",
            self.product.as_str(),
            self.target.triple()
        )
    }
}

/// A release this installation may move to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCandidate {
    pub tag: String,
    pub version: Version,
    pub channel: Channel,
    pub descriptor: Descriptor,
}

/// Release archive a descriptor points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRef {
    pub name: String,
    pub format: ArchiveKind,
    pub sha256: String,
    pub size: u64,
}

/// Per-product release descriptor (`bili-update-<product>-<target>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub schema_version: u32,
    pub product: Product,
    pub target: Target,
    pub version: Version,
    pub archive: ArchiveRef,
}

/// Result of one anonymous check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    /// A strictly newer, complete release matched this identity and channel.
    Available(UpdateCandidate),
    /// Nothing newer than the running version exists.
    UpToDate { current: Version },
    /// Newer releases exist but none publishes this product and target.
    NoCompatibleRelease { current: Version },
}

impl CheckOutcome {
    pub fn candidate(&self) -> Option<&UpdateCandidate> {
        match self {
            Self::Available(candidate) => Some(candidate),
            _ => None,
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_stable_and_parse_explicit_values() {
        let default: UpdateSettings = serde_json::from_str("{}").expect("empty settings parse");
        assert_eq!(default, UpdateSettings::default());
        assert!(!default.allow_pre_release);
        assert!(default.check_updates);
        assert!(!default.auto_update);
        assert_eq!(default.channel(), Channel::Stable);

        let explicit: UpdateSettings = serde_json::from_str(
            r#"{"allow_pre_release":false,"check_updates":false,"auto_update":false}"#,
        )
        .expect("explicit settings parse");
        assert!(!explicit.check_updates);
        assert_eq!(explicit.channel(), Channel::Stable);

        let opt_in: UpdateSettings =
            serde_json::from_str(r#"{"allow_pre_release":true}"#).expect("opt-in settings parse");
        assert_eq!(opt_in.channel(), Channel::PreRelease);
        assert!(opt_in.check_updates);
    }

    #[test]
    fn settings_tolerate_fields_a_newer_shell_adds() {
        let settings: UpdateSettings =
            serde_json::from_str(r#"{"allow_pre_release":true,"future_knob":7}"#)
                .expect("forward-compatible settings parse");
        assert!(settings.allow_pre_release);
    }

    #[test]
    fn identity_parses_once_and_names_its_descriptor() {
        let identity =
            Identity::new(Product::Cli, Target::LinuxX86_64Gnu, "2.0.0-alpha.6").expect("identity");
        assert_eq!(identity.version().to_string(), "2.0.0-alpha.6");
        assert_eq!(Channel::of(identity.version()), Channel::PreRelease);
        assert_eq!(
            identity.descriptor_name(),
            "bili-update-cli-x86_64-unknown-linux-gnu.json"
        );
        assert!(Identity::new(Product::Cli, Target::WindowsX86_64Msvc, "not-a-version").is_err());
    }

    #[test]
    fn target_maps_to_its_only_legal_archive_kind() {
        assert_eq!(Target::LinuxX86_64Gnu.archive_kind(), ArchiveKind::TarGz);
        assert_eq!(Target::WindowsX86_64Msvc.archive_kind(), ArchiveKind::Zip);
        for target in [Target::LinuxX86_64Gnu, Target::WindowsX86_64Msvc] {
            assert_eq!(Target::from_triple(target.triple()), Some(target));
            assert_eq!(Target::from_triple("aarch64-apple-darwin"), None);
        }
    }

    #[test]
    fn descriptor_uses_the_published_wire_shape() {
        let descriptor: Descriptor = serde_json::from_str(
            r#"{
                "schema_version": 1,
                "product": "cli",
                "target": "x86_64-unknown-linux-gnu",
                "version": "2.0.0",
                "archive": {
                    "name": "bili-x86_64-unknown-linux-gnu-release.tar.gz",
                    "format": "tar.gz",
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "size": 1024
                },
                "packaged_at": "ignored by older builds"
            }"#,
        )
        .expect("descriptor parse");
        assert_eq!(descriptor.product, Product::Cli);
        assert_eq!(descriptor.target, Target::LinuxX86_64Gnu);
        assert_eq!(descriptor.archive.format, ArchiveKind::TarGz);
        assert_eq!(descriptor.schema_version, PROTOCOL);

        let reencoded = serde_json::to_value(&descriptor).expect("descriptor encode");
        assert_eq!(reencoded["product"], "cli");
        assert_eq!(reencoded["target"], "x86_64-unknown-linux-gnu");
        assert_eq!(reencoded["archive"]["format"], "tar.gz");
    }

    #[test]
    fn outcome_distinguishes_available_from_up_to_date() {
        let candidate = UpdateCandidate {
            tag: "v2.0.0".into(),
            version: Version::parse("2.0.0").expect("version"),
            channel: Channel::Stable,
            descriptor: Descriptor {
                schema_version: PROTOCOL,
                product: Product::Cli,
                target: Target::LinuxX86_64Gnu,
                version: Version::parse("2.0.0").expect("version"),
                archive: ArchiveRef {
                    name: "bili-x86_64-unknown-linux-gnu-release.tar.gz".into(),
                    format: ArchiveKind::TarGz,
                    sha256: "0".repeat(64),
                    size: 1,
                },
            },
        };
        let available = CheckOutcome::Available(candidate);
        assert!(available.is_available());
        assert!(available.candidate().is_some());

        let current = Version::parse("2.0.0").expect("version");
        let up_to_date = CheckOutcome::UpToDate {
            current: current.clone(),
        };
        assert!(!up_to_date.is_available());
        assert_eq!(
            CheckOutcome::NoCompatibleRelease { current },
            serde_json::from_str::<CheckOutcome>(
                r#"{"outcome":"no_compatible_release","current":"2.0.0"}"#
            )
            .expect("outcome parse")
        );
    }
}
