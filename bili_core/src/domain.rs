//! Credential-free application contracts shared by every Rust shell.
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    InvalidInput,
    VerificationRequired,
    LoginRequired,
    EntitlementRequired,
    RegionRestricted,
    Unavailable,
    RepresentationChanged,
    Transport,
    RetryExhausted,
    Filesystem,
    Conflict,
    Busy,
    Corruption,
    NativeCapability,
    Remux,
    Cancelled,
    ReadOnly,
    Shutdown,
    CredentialStore,
}
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub struct CoreError {
    pub code: ErrorCode,
    pub operation: String,
    pub retryable: bool,
    /// HTTP response status when a response was received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Original signed FFmpeg AVERROR value; never native text, paths or URLs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_code: Option<i32>,
}
impl CoreError {
    /// Operation is a static, trusted label; never pass remote text or URLs.
    pub fn new(code: ErrorCode, operation: &'static str) -> Self {
        Self {
            code,
            operation: operation.into(),
            retryable: false,
            http_status: None,
            native_code: None,
        }
    }
    pub fn io(_: std::io::Error) -> Self {
        Self::new(ErrorCode::Filesystem, "local filesystem operation")
    }
}
impl std::fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.operation)?;
        if let Some(code) = self.native_code {
            write!(formatter, " (FFmpeg code {code})")?;
        }
        Ok(())
    }
}
macro_rules! id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "u64", into = "u64")]
        pub struct $name(u64);
        impl $name {
            pub fn new(value: u64) -> Result<Self> {
                if value == 0 {
                    Err(CoreError::new(
                        ErrorCode::InvalidInput,
                        "identifier must be positive",
                    ))
                } else {
                    Ok(Self(value))
                }
            }
            pub fn get(self) -> u64 {
                self.0
            }
        }
        impl TryFrom<u64> for $name {
            type Error = CoreError;
            fn try_from(v: u64) -> Result<Self> {
                Self::new(v)
            }
        }
        impl From<$name> for u64 {
            fn from(v: $name) -> u64 {
                v.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
id!(Aid);
id!(Cid);
id!(SeasonId);
id!(EpisodeId);
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Bvid(String);
impl Bvid {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let v = value.into();
        if v.len() == 12 && v.starts_with("BV") && v.bytes().all(|b| b.is_ascii_alphanumeric()) {
            Ok(Self(v))
        } else {
            Err(CoreError::new(
                ErrorCode::InvalidInput,
                "invalid BV identifier",
            ))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Bvid {
    type Error = CoreError;
    fn try_from(v: String) -> Result<Self> {
        Self::new(v)
    }
}
impl From<Bvid> for String {
    fn from(v: Bvid) -> String {
        v.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub uuid::Uuid);
impl JobId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}
impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::str::FromStr for JobId {
    type Err = CoreError;
    fn from_str(v: &str) -> Result<Self> {
        uuid::Uuid::parse_str(v)
            .map(Self)
            .map_err(|_| CoreError::new(ErrorCode::InvalidInput, "invalid job identifier"))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentRef {
    Video(Aid),
    Bangumi(SeasonId),
    Episode(EpisodeId),
}
#[derive(Debug, Clone)]
pub struct ContentInput(pub String);
impl From<String> for ContentInput {
    fn from(v: String) -> Self {
        Self(v)
    }
}
impl From<&str> for ContentInput {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartRef {
    pub content: ContentRef,
    pub aid: Aid,
    pub cid: Cid,
    pub episode: Option<EpisodeId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub reference: PartRef,
    pub ordinal: u32,
    pub title: String,
    pub duration_ms: Option<u64>,
    pub preview: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub reference: ContentRef,
    pub bvid: Option<Bvid>,
    pub title: String,
    pub cover: Option<String>,
    pub duration_ms: Option<u64>,
    pub parts: Vec<Part>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchCategory {
    All,
    Video,
    Bangumi,
    Film,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub keyword: String,
    pub category: SearchCategory,
    pub page: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub reference: ContentRef,
    pub category: SearchCategory,
    pub title: String,
    pub cover: Option<String>,
    pub duration_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryPage {
    pub category: SearchCategory,
    pub page: u32,
    pub total: Option<u64>,
    pub has_more: bool,
    pub items: Vec<Candidate>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidatePage {
    pub pages: Vec<CategoryPage>,
}
#[derive(Debug, Clone)]
pub struct CoverRequest {
    pub url: String,
}
#[derive(Debug, Clone)]
pub struct CoverBytes {
    pub bytes: Vec<u8>,
    pub mime: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Obtainable,
    LoginRequired,
    EntitlementRequired,
    RegionRestricted,
    PreviewOnly,
    Unknown,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational {
    pub numerator: u32,
    pub denominator: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepresentationKey {
    pub part: PartRef,
    pub id: u64,
    pub quality: u32,
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<Rational>,
    pub dynamic_range: Option<String>,
    pub color: Option<String>,
    pub bandwidth: u64,
    pub audio: bool,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub language: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityEntry {
    pub id: u32,
    pub label: String,
    pub rank: u32,
    pub availability: Availability,
    /// Explicit upstream requirements; absence is not equivalent to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_login: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_vip: Option<bool>,
    /// This catalog response returned a valid video address for this quality.
    /// Metadata and account requirements are independent of this evidence.
    #[serde(default)]
    pub has_video_url: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advertised_codecs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCatalog {
    pub part: PartRef,
    pub advertised: Vec<QualityEntry>,
    pub representations: Vec<RepresentationKey>,
    pub revision: u64,
    pub preview: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamTopology {
    Dash,
    Direct,
    Sequential,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentIdentity {
    pub ordinal: u32,
    pub size: Option<u64>,
    pub duration_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtitleSelection {
    All,
    None,
    Languages(Vec<String>),
    Tracks(Vec<String>),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarPolicy {
    pub subtitles: SubtitleSelection,
    pub danmaku: bool,
}
impl Default for SidecarPolicy {
    fn default() -> Self {
        Self {
            subtitles: SubtitleSelection::All,
            danmaku: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergePolicy {
    Automatic,
    Manual,
}
#[derive(Debug, Clone)]
pub struct DownloadSelection {
    pub part: PartRef,
    pub title: String,
    pub quality: Option<u32>,
    pub video_representation: Option<u64>,
    pub audio_representation: Option<u64>,
    pub codec: Option<String>,
    pub dynamic_range: Option<String>,
    pub output: Option<PathBuf>,
    pub merge: MergePolicy,
    pub sidecars: SidecarPolicy,
    pub keep_temp: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPlan {
    pub schema_version: u32,
    pub fingerprint: String,
    pub part: PartRef,
    pub title: String,
    pub video: RepresentationKey,
    pub audio: Option<RepresentationKey>,
    pub topology: StreamTopology,
    pub segments: Vec<SegmentIdentity>,
    pub destination: PathBuf,
    pub merge: MergePolicy,
    pub sidecars: SidecarPolicy,
    pub keep_temp: bool,
    pub catalog_revision: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobPhase {
    Queued,
    Resolving,
    Downloading,
    Downloaded,
    Remuxing,
    Verifying,
    Committing,
    Completed,
    Pausing,
    Paused,
    Cancelling,
    Cancelled,
    Failed,
    NeedsAttention,
}
impl JobPhase {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::Resolving
                | Self::Downloading
                | Self::Remuxing
                | Self::Verifying
                | Self::Committing
                | Self::Pausing
                | Self::Cancelling
        )
    }
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Downloaded
                | Self::Paused
                | Self::Cancelled
                | Self::Failed
                | Self::NeedsAttention
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentResult {
    pub name: String,
    pub path: Option<PathBuf>,
    pub error: Option<CoreError>,
    pub omissions: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: JobId,
    pub revision: u64,
    pub plan: Option<std::sync::Arc<StreamPlan>>,
    pub phase: JobPhase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub output: Option<PathBuf>,
    pub skipped: bool,
    pub error: Option<CoreError>,
    pub components: Vec<ComponentResult>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub schema_version: u32,
    pub revision: u64,
    pub jobs: Vec<std::sync::Arc<JobSnapshot>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreEvent {
    pub sequence: u64,
    pub job: Option<JobId>,
    pub phase: Option<JobPhase>,
    pub message: String,
}
#[derive(Debug, Clone, Copy)]
pub enum JobAction {
    Pause,
    Resume,
    Cancel,
    Retry,
    Remove { cleanup: bool },
}
#[derive(Debug, Clone, Copy)]
pub enum StopMode {
    Pause,
    Cancel,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShutdownReport {
    pub completed: u64,
    pub paused: u64,
    pub failed: u64,
}
#[derive(Debug, Clone)]
pub enum MergeRequest {
    Managed(JobId),
    Local {
        video: PathBuf,
        audio: Option<PathBuf>,
        output: PathBuf,
    },
}
#[derive(Debug, Clone)]
pub struct CleanupRequest {
    pub job: Option<JobId>,
    pub abandoned: bool,
    pub include_resumable: bool,
    pub dry_run: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CleanupReport {
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
    pub errors: Vec<CoreError>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateMode {
    ReadWrite,
    ReadOnly,
}
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub base_delay: Duration,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(500),
        }
    }
}
#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub state_dir: PathBuf,
    pub output_dir: PathBuf,
    pub state_mode: StateMode,
    pub max_jobs: usize,
    pub max_connections: usize,
    pub timeout: Duration,
    pub retry: RetryPolicy,
    pub rate_limit: Option<u64>,
    pub credential_store: crate::cookie::StorePolicy,
}
impl CoreConfig {
    pub fn new(state_dir: PathBuf, output_dir: PathBuf) -> Self {
        Self {
            state_dir,
            output_dir,
            state_mode: StateMode::ReadWrite,
            max_jobs: 3,
            max_connections: 6,
            timeout: Duration::from_secs(30),
            retry: RetryPolicy::default(),
            rate_limit: None,
            credential_store: crate::cookie::StorePolicy::MemoryOnly,
        }
    }
}

/// Sole durable byte-offset authority, written by transfer beside each resource.
/// Job manifests reference resource names; queue progress is presentation only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceCheckpoint {
    pub durable_bytes: u64,
    pub expected_total: Option<u64>,
    pub strong_etag: Option<String>,
    pub sha256: Option<String>,
    pub complete: bool,
}
