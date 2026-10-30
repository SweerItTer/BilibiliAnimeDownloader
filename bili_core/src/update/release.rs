//! Anonymous GitHub release discovery for one compiled identity.
//!
//! The client is a dedicated anonymous authority: no cookies, no
//! authorization, no inherited Bilibili headers, an explicit user agent and a
//! redirect policy that only follows the GitHub hosts a release download may
//! legitimately use. Every response body has a size bound.

use super::{Channel, CheckOutcome, Descriptor, Identity, UpdateCandidate, UpdateError, PROTOCOL};
use futures_util::StreamExt;
use reqwest::{redirect, Client, StatusCode, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ASSET_NAME: usize = 128;
const MAX_TAG: usize = 64;
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_REDIRECTS: usize = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest silence tolerated while streaming a release archive.
const ARCHIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Total budget for one archive download, independent of the metadata budget.
const ARCHIVE_DEADLINE: Duration = Duration::from_secs(60 * 60);
const OFFICIAL_OWNER: &str = "SweerItTer";
const OFFICIAL_NAME: &str = "BilibiliAnimeDownloader";
const OFFICIAL_API: &str = "https://api.github.com";
const OFFICIAL_DOWNLOAD: &str = "https://github.com";
const USER_AGENT: &str = "bili-update-protocol/1";
const API_ACCEPT: &str = "application/vnd.github+json";
const API_VERSION: &str = "2022-11-28";

/// Source repository a product is published from.
///
/// Only the official constants reach production; the type stays crate-private
/// so no shell (or caller) can point the updater at another repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    fn new(owner: &str, name: &str) -> Result<Self, UpdateError> {
        if !valid_component(owner) || !valid_component(name) {
            return Err(UpdateError::InvalidInput);
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    fn official() -> Self {
        Self::new(OFFICIAL_OWNER, OFFICIAL_NAME).expect("official repository is valid")
    }

    fn owner(&self) -> &str {
        &self.owner
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Discovers releases for the official product repository.
pub struct Updater {
    repository: Repository,
    client: Client,
    download_client: Client,
    api_base: Url,
    download_base: Url,
}

impl Updater {
    /// Anonymous reader for the official API and download hosts.
    pub fn new() -> Result<Self, UpdateError> {
        Self::build(
            Repository::official(),
            Url::parse(OFFICIAL_API).map_err(|_| UpdateError::InvalidInput)?,
            Url::parse(OFFICIAL_DOWNLOAD).map_err(|_| UpdateError::InvalidInput)?,
            REQUEST_TIMEOUT,
        )
    }

    fn build(
        repository: Repository,
        api_base: Url,
        download_base: Url,
        timeout: Duration,
    ) -> Result<Self, UpdateError> {
        let policy = || {
            redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.stop()
                } else if permitted(&attempt.url().clone()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            })
        };
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .redirect(policy())
            .connect_timeout(CONNECT_TIMEOUT)
            // Total request budget: from connect until the response body is
            // drained, so a stalled body can never hang a caller.
            .timeout(timeout)
            .build()
            .map_err(|_| UpdateError::Transport)?;
        // Archive bodies are legitimately large, so they get their own bounded
        // transport: same user agent, redirect policy and connect budget, an
        // idle-read budget instead of a whole-request one, and the size and time
        // bounds enforced by `download`.
        let download_client = Client::builder()
            .user_agent(USER_AGENT)
            .redirect(policy())
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(ARCHIVE_IDLE_TIMEOUT)
            .build()
            .map_err(|_| UpdateError::Transport)?;
        Ok(Self {
            repository,
            client,
            download_client,
            api_base,
            download_base,
        })
    }

    /// Loopback transport for tests; production always uses the two literal
    /// hosts and the fixed official repository.
    #[cfg(test)]
    pub(crate) fn with_bases(
        api_base: Url,
        download_base: Url,
        timeout: Duration,
    ) -> Result<Self, UpdateError> {
        Self::build(Repository::official(), api_base, download_base, timeout)
    }

    /// Anonymous, cancel-safe check. Dropping the future aborts all work; no
    /// task is spawned and no state is written.
    pub async fn check(
        &self,
        identity: &Identity,
        channel: Channel,
    ) -> Result<CheckOutcome, UpdateError> {
        let mut releases = Vec::new();
        for page in 1..=MAX_PAGES {
            let body = self.releases_page(page).await?;
            let items: Vec<RawRelease> =
                serde_json::from_slice(&body).map_err(|_| UpdateError::Transport)?;
            let full = items.len() >= PAGE_SIZE;
            releases.extend(items.into_iter().filter_map(ReleaseCandidate::parse));
            if !full {
                return self.select(identity, channel, releases).await;
            }
        }
        Err(UpdateError::TooManyReleases)
    }

    /// Download location of a candidate archive on the allowlisted host.
    pub fn archive_url(&self, candidate: &UpdateCandidate) -> Result<Url, UpdateError> {
        self.download_url(&candidate.tag, &candidate.descriptor.archive.name)
    }

    /// Stream a candidate archive into `destination`, verifying size and digest.
    ///
    /// The body is never buffered: chunks go straight to the file, with an idle
    /// budget per read, an overall deadline, and the descriptor's size and
    /// SHA-256 enforced before the call succeeds. A failed download removes the
    /// partial file it wrote.
    pub async fn download(
        &self,
        candidate: &UpdateCandidate,
        destination: &Path,
    ) -> Result<u64, UpdateError> {
        let outcome = self.stream_archive(candidate, destination).await;
        if outcome.is_err() {
            let _ = tokio::fs::remove_file(destination).await;
        }
        outcome
    }

    async fn stream_archive(
        &self,
        candidate: &UpdateCandidate,
        destination: &Path,
    ) -> Result<u64, UpdateError> {
        let expected = &candidate.descriptor.archive;
        let url = self.archive_url(candidate)?;
        let response = self
            .download_client
            .get(url)
            .send()
            .await
            .map_err(|_| UpdateError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let exhausted = response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|value| value.to_str().ok())
                == Some("0");
            if status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN && exhausted)
            {
                return Err(UpdateError::RateLimited);
            }
            return Err(UpdateError::HttpStatus(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length != expected.size)
        {
            return Err(UpdateError::SizeMismatch);
        }
        let mut file = tokio::fs::File::create(destination)
            .await
            .map_err(|_| UpdateError::Filesystem)?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha256::new();
        let mut received = 0u64;
        let deadline = tokio::time::Instant::now() + ARCHIVE_DEADLINE;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(UpdateError::Timeout);
            }
            let wait = remaining.min(ARCHIVE_IDLE_TIMEOUT);
            let chunk = match tokio::time::timeout(wait, stream.next()).await {
                Err(_) => return Err(UpdateError::Timeout),
                Ok(None) => break,
                Ok(Some(Err(_))) => return Err(UpdateError::Transport),
                Ok(Some(Ok(chunk))) => chunk,
            };
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or(UpdateError::SizeMismatch)?;
            if received > expected.size {
                return Err(UpdateError::SizeMismatch);
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| UpdateError::Filesystem)?;
        }
        file.flush().await.map_err(|_| UpdateError::Filesystem)?;
        file.sync_all().await.map_err(|_| UpdateError::Filesystem)?;
        drop(file);
        if received != expected.size {
            return Err(UpdateError::SizeMismatch);
        }
        if format!("{:x}", hasher.finalize()) != expected.sha256 {
            return Err(UpdateError::DigestMismatch);
        }
        Ok(received)
    }

    async fn select(
        &self,
        identity: &Identity,
        channel: Channel,
        mut candidates: Vec<ReleaseCandidate>,
    ) -> Result<CheckOutcome, UpdateError> {
        let current = identity.version();
        candidates.retain(|candidate| {
            candidate.version.cmp_precedence(current) == Ordering::Greater
                && channel.accepts(&candidate.version)
        });
        if candidates.is_empty() {
            return Ok(CheckOutcome::UpToDate {
                current: current.clone(),
            });
        }
        candidates.sort_by(|left, right| {
            right
                .version
                .cmp_precedence(&left.version)
                .then_with(|| right.version.cmp(&left.version))
        });
        let descriptor_name = identity.descriptor_name();
        for candidate in candidates {
            match candidate
                .assets
                .iter()
                .filter(|asset| asset.name == descriptor_name)
                .count()
            {
                0 => continue,
                1 => {}
                _ => return Err(UpdateError::AmbiguousDescriptor),
            }
            let body = self
                .fetch_asset(&candidate.tag, &descriptor_name, MAX_DESCRIPTOR_BYTES)
                .await?;
            let descriptor: Descriptor =
                serde_json::from_slice(&body).map_err(|_| UpdateError::MalformedDescriptor)?;
            let descriptor = validate_descriptor(descriptor, identity, &candidate.version)?;
            let mut bound = candidate
                .assets
                .iter()
                .filter(|asset| asset.name == descriptor.archive.name);
            let asset = bound.next().ok_or(UpdateError::MissingAsset)?;
            if bound.next().is_some() {
                return Err(UpdateError::AmbiguousAsset);
            }
            // The release must advertise the exact byte count the descriptor
            // describes; a zero or merely different size is not accepted.
            if asset.size != descriptor.archive.size {
                return Err(UpdateError::SizeMismatch);
            }
            return Ok(CheckOutcome::Available(UpdateCandidate {
                channel: Channel::of(&candidate.version),
                tag: candidate.tag,
                version: candidate.version,
                descriptor,
            }));
        }
        Ok(CheckOutcome::NoCompatibleRelease {
            current: current.clone(),
        })
    }

    async fn releases_page(&self, page: usize) -> Result<Vec<u8>, UpdateError> {
        let mut url = self.api_base.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| UpdateError::InvalidInput)?;
            segments
                .pop_if_empty()
                .push("repos")
                .push(self.repository.owner())
                .push(self.repository.name())
                .push("releases");
        }
        url.set_query(Some(&format!("per_page={PAGE_SIZE}&page={page}")));
        self.get_limited(url, MAX_PAGE_BYTES).await
    }

    async fn fetch_asset(
        &self,
        tag: &str,
        name: &str,
        limit: usize,
    ) -> Result<Vec<u8>, UpdateError> {
        let url = self.download_url(tag, name)?;
        self.get_limited(url, limit).await
    }

    fn download_url(&self, tag: &str, name: &str) -> Result<Url, UpdateError> {
        if !safe_tag(tag) || !safe_asset_name(name) {
            return Err(UpdateError::UnsafeAssetName);
        }
        let mut url = self.download_base.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| UpdateError::InvalidInput)?;
            segments
                .pop_if_empty()
                .push(self.repository.owner())
                .push(self.repository.name())
                .push("releases")
                .push("download")
                .push(tag)
                .push(name);
        }
        Ok(url)
    }

    async fn get_limited(&self, url: Url, limit: usize) -> Result<Vec<u8>, UpdateError> {
        let response = self
            .client
            .get(url)
            .header("accept", API_ACCEPT)
            .header("x-github-api-version", API_VERSION)
            .send()
            .await
            .map_err(|_| UpdateError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let exhausted = response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|value| value.to_str().ok())
                == Some("0");
            if status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN && exhausted)
            {
                return Err(UpdateError::RateLimited);
            }
            return Err(UpdateError::HttpStatus(status.as_u16()));
        }
        read_limited(response, limit).await
    }
}

struct ReleaseCandidate {
    version: Version,
    tag: String,
    assets: Vec<RawAsset>,
}

impl ReleaseCandidate {
    fn parse(raw: RawRelease) -> Option<Self> {
        if raw.draft {
            return None;
        }
        let version = tag_version(&raw.tag_name)?;
        if raw.prerelease != !version.pre.is_empty() {
            return None;
        }
        Some(Self {
            version,
            tag: raw.tag_name,
            assets: raw.assets,
        })
    }
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    #[serde(default)]
    size: u64,
}

async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, UpdateError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(UpdateError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| UpdateError::Transport)? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(UpdateError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_descriptor(
    descriptor: Descriptor,
    identity: &Identity,
    tag_version: &Version,
) -> Result<Descriptor, UpdateError> {
    if descriptor.schema_version != PROTOCOL {
        return Err(UpdateError::UnsupportedSchema(descriptor.schema_version));
    }
    if descriptor.product != identity.product() {
        return Err(UpdateError::ProductMismatch);
    }
    if descriptor.target != identity.target() {
        return Err(UpdateError::TargetMismatch);
    }
    if &descriptor.version != tag_version {
        return Err(UpdateError::VersionMismatch);
    }
    if !safe_asset_name(&descriptor.archive.name)
        || descriptor.archive.name == identity.descriptor_name()
    {
        return Err(UpdateError::UnsafeAssetName);
    }
    if descriptor.archive.format != identity.target().archive_kind() {
        return Err(UpdateError::ArchiveKindMismatch);
    }
    if !valid_sha256(&descriptor.archive.sha256) {
        return Err(UpdateError::DigestMismatch);
    }
    if descriptor.archive.size == 0 || descriptor.archive.size > MAX_ARCHIVE_BYTES {
        return Err(UpdateError::SizeMismatch);
    }
    Ok(descriptor)
}

fn tag_version(tag: &str) -> Option<Version> {
    if !safe_tag(tag) {
        return None;
    }
    let text = tag.strip_prefix('v')?;
    let version = Version::parse(text).ok()?;
    (version.to_string() == text).then_some(version)
}

fn safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= MAX_TAG
        && tag.starts_with('v')
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ASSET_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
        && !name.starts_with('.')
        && !name.contains("..")
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && component.len() <= 100
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// Hosts a release request may touch. Loopback exists only for crate tests.
fn permitted(url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    let host_allowed = matches!(
        url.host_str(),
        Some(
            "api.github.com"
                | "github.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
        )
    ) || (cfg!(test)
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost")));
    if !host_allowed {
        return false;
    }
    match url.scheme() {
        "https" => url.port().is_none(),
        "http" => cfg!(test) && url.port().is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    const DESCRIPTOR: &str = "bili-update-cli-x86_64-unknown-linux-gnu.json";
    const ARCHIVE: &str = "bili-x86_64-unknown-linux-gnu-release.tar.gz";
    const DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    struct Server {
        base: String,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl Server {
        fn api(&self) -> Url {
            Url::parse(&self.base).expect("loopback base")
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            self.handle.abort();
        }
    }

    async fn serve(routes: Vec<(String, String)>) -> Server {
        let routes: Arc<HashMap<String, String>> = Arc::new(routes.into_iter().collect());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback fixture");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("loopback address")
        );
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { break };
                        let mut buffer = vec![0_u8; 8192];
                        let Ok(read) = socket.read(&mut buffer).await else { continue };
                        let request = String::from_utf8_lossy(&buffer[..read]);
                        let target = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or_default()
                            .to_string();
                        let (status, body) = match routes.get(&target) {
                            Some(body) => match body.strip_prefix("#status=") {
                                Some(code) => (
                                    format!("{} Status", code.trim_end_matches('\n')),
                                    String::new(),
                                ),
                                None => (String::from("200 OK"), body.clone()),
                            },
                            None => (String::from("404 Not Found"), String::from("{}")),
                        };
                        let head = format!(
                            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(body.as_bytes()).await;
                        let _ = socket.shutdown().await;
                    }
                }
            }
        });
        Server {
            base,
            stop: Some(stop),
            handle,
        }
    }

    fn descriptor(
        schema: u32,
        product: &str,
        target: &str,
        version: &str,
        archive: &str,
        format: &str,
        integrity: (&str, u64),
    ) -> String {
        let (digest, size) = integrity;
        format!(
            r#"{{"schema_version":{schema},"product":"{product}","target":"{target}","version":"{version}","archive":{{"name":"{archive}","format":"{format}","sha256":"{digest}","size":{size}}}}}"#
        )
    }

    fn valid_descriptor(version: &str) -> String {
        descriptor(
            PROTOCOL,
            "cli",
            "x86_64-unknown-linux-gnu",
            version,
            ARCHIVE,
            "tar.gz",
            (DIGEST, 4096),
        )
    }

    fn release(tag: &str, draft: bool, prerelease: bool, assets: &[(&str, u64)]) -> String {
        let assets = assets
            .iter()
            .map(|(name, size)| format!(r#"{{"name":"{name}","size":{size}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":{prerelease},"assets":[{assets}]}}"#
        )
    }

    fn page(releases: &[String]) -> String {
        format!("[{}]", releases.join(","))
    }

    fn drafts(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| release(&format!("v0.0.{index}"), true, false, &[]))
            .collect()
    }

    fn releases_route(page: usize) -> String {
        format!("/repos/SweerItTer/BilibiliAnimeDownloader/releases?per_page=100&page={page}")
    }

    fn descriptor_route(tag: &str) -> String {
        format!("/SweerItTer/BilibiliAnimeDownloader/releases/download/{tag}/{DESCRIPTOR}")
    }

    fn identity(version: &str) -> Identity {
        Identity::new(
            super::super::Product::Cli,
            super::super::Target::LinuxX86_64Gnu,
            version,
        )
        .expect("identity")
    }

    async fn updater(routes: Vec<(String, String)>) -> (Server, Updater) {
        let server = serve(routes).await;
        let updater = Updater::with_bases(
            server.api(),
            server.api(),
            std::time::Duration::from_secs(5),
        )
        .expect("updater");
        (server, updater)
    }

    async fn single(routes: Vec<(String, String)>) -> Result<CheckOutcome, UpdateError> {
        let (_server, updater) = updater(routes).await;
        updater
            .check(&identity("2.0.0-alpha.6"), Channel::Stable)
            .await
    }

    #[tokio::test]
    async fn stable_and_pre_release_channels_pick_their_highest_match() {
        let routes = vec![
            (
                releases_route(1),
                page(&[
                    release(
                        "v2.1.0-alpha.1",
                        false,
                        true,
                        &[(DESCRIPTOR, 512), (ARCHIVE, 4096)],
                    ),
                    release(
                        "v2.0.0",
                        false,
                        false,
                        &[(DESCRIPTOR, 512), (ARCHIVE, 4096)],
                    ),
                ]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
            (
                descriptor_route("v2.1.0-alpha.1"),
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.1.0-alpha.1",
                    ARCHIVE,
                    "tar.gz",
                    (DIGEST, 4096),
                ),
            ),
        ];
        let (_server, updater) = updater(routes).await;
        let current = identity("2.0.0-alpha.6");

        let stable = updater
            .check(&current, Channel::Stable)
            .await
            .expect("stable check");
        let stable = stable.candidate().expect("stable candidate");
        assert_eq!(stable.version, Version::parse("2.0.0").unwrap());
        assert_eq!(stable.channel, Channel::Stable);

        let preview = updater
            .check(&current, Channel::PreRelease)
            .await
            .expect("preview check");
        let preview = preview.candidate().expect("preview candidate");
        assert_eq!(preview.version, Version::parse("2.1.0-alpha.1").unwrap());
        assert_eq!(preview.channel, Channel::PreRelease);
    }

    #[tokio::test]
    async fn build_metadata_is_not_an_upgrade() {
        let routes = vec![
            (
                releases_route(1),
                page(&[release(
                    "v2.0.0+build.2",
                    false,
                    false,
                    &[(DESCRIPTOR, 512), (ARCHIVE, 4096)],
                )]),
            ),
            (
                descriptor_route("v2.0.0+build.2"),
                valid_descriptor("2.0.0+build.2"),
            ),
        ];
        let (_server, updater) = updater(routes).await;
        let outcome = updater
            .check(&identity("2.0.0+build.1"), Channel::Stable)
            .await
            .expect("check");
        assert!(matches!(outcome, CheckOutcome::UpToDate { .. }));
    }

    #[tokio::test]
    async fn older_release_is_never_offered() {
        let routes = vec![(
            releases_route(1),
            page(&[release("v2.0.0-alpha.5", false, true, &[(DESCRIPTOR, 512)])]),
        )];
        let outcome = single(routes).await.expect("check");
        assert!(matches!(outcome, CheckOutcome::UpToDate { .. }));
    }

    #[tokio::test]
    async fn draft_and_mismatched_prerelease_flags_are_ignored() {
        let routes = vec![
            (
                releases_route(1),
                page(&[
                    release("v2.0.0", true, false, &[(DESCRIPTOR, 512)]),
                    release("v2.1.0-alpha.1", false, false, &[(DESCRIPTOR, 512)]),
                    release("v2.2.0", false, true, &[(DESCRIPTOR, 512)]),
                ]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
        ];
        let outcome = single(routes).await.expect("check");
        assert!(matches!(outcome, CheckOutcome::UpToDate { .. }));
    }

    #[tokio::test]
    async fn release_without_this_product_descriptor_is_not_up_to_date() {
        let routes = vec![(
            releases_route(1),
            page(&[release(
                "v2.0.0",
                false,
                false,
                &[
                    ("bili-update-gui-x86_64-unknown-linux-gnu.json", 512),
                    (ARCHIVE, 4096),
                ],
            )]),
        )];
        let outcome = single(routes).await.expect("check");
        assert!(matches!(outcome, CheckOutcome::NoCompatibleRelease { .. }));
    }

    #[tokio::test]
    async fn descriptor_defects_are_reported_distinctly() {
        let cases: Vec<(&str, String, UpdateError)> = vec![
            (
                "schema",
                descriptor(
                    2,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    ARCHIVE,
                    "tar.gz",
                    (DIGEST, 4096),
                ),
                UpdateError::UnsupportedSchema(2),
            ),
            (
                "product",
                descriptor(
                    PROTOCOL,
                    "gui",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    ARCHIVE,
                    "tar.gz",
                    (DIGEST, 4096),
                ),
                UpdateError::ProductMismatch,
            ),
            (
                "target",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-pc-windows-msvc",
                    "2.0.0",
                    ARCHIVE,
                    "tar.gz",
                    (DIGEST, 4096),
                ),
                UpdateError::TargetMismatch,
            ),
            (
                "version",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.1",
                    ARCHIVE,
                    "tar.gz",
                    (DIGEST, 4096),
                ),
                UpdateError::VersionMismatch,
            ),
            (
                "digest",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    ARCHIVE,
                    "tar.gz",
                    ("zz", 4096),
                ),
                UpdateError::DigestMismatch,
            ),
            (
                "size",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    ARCHIVE,
                    "tar.gz",
                    (DIGEST, 0),
                ),
                UpdateError::SizeMismatch,
            ),
            (
                "name",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    "../evil.tar.gz",
                    "tar.gz",
                    (DIGEST, 4096),
                ),
                UpdateError::UnsafeAssetName,
            ),
            (
                "format",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    ARCHIVE,
                    "zip",
                    (DIGEST, 4096),
                ),
                UpdateError::ArchiveKindMismatch,
            ),
            (
                "missing",
                descriptor(
                    PROTOCOL,
                    "cli",
                    "x86_64-unknown-linux-gnu",
                    "2.0.0",
                    "other.tar.gz",
                    "tar.gz",
                    (DIGEST, 4096),
                ),
                UpdateError::MissingAsset,
            ),
            (
                "malformed",
                String::from("not json"),
                UpdateError::MalformedDescriptor,
            ),
        ];
        for (label, body, expected) in cases {
            let routes = vec![
                (
                    releases_route(1),
                    page(&[release(
                        "v2.0.0",
                        false,
                        false,
                        &[(DESCRIPTOR, 512), (ARCHIVE, 4096)],
                    )]),
                ),
                (descriptor_route("v2.0.0"), body),
            ];
            let error = single(routes).await.expect_err(label);
            assert_eq!(error, expected, "case {label}");
        }
    }

    #[tokio::test]
    async fn zero_sized_archive_asset_is_not_accepted() {
        let routes = vec![
            (
                releases_route(1),
                page(&[release(
                    "v2.0.0",
                    false,
                    false,
                    &[(DESCRIPTOR, 512), (ARCHIVE, 0)],
                )]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
        ];
        assert_eq!(
            single(routes).await.expect_err("unpublished size"),
            UpdateError::SizeMismatch
        );
    }

    #[tokio::test]
    async fn duplicate_archive_assets_are_ambiguous() {
        let routes = vec![
            (
                releases_route(1),
                page(&[release(
                    "v2.0.0",
                    false,
                    false,
                    &[(DESCRIPTOR, 512), (ARCHIVE, 4096), (ARCHIVE, 4096)],
                )]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
        ];
        assert_eq!(
            single(routes).await.expect_err("ambiguous archive"),
            UpdateError::AmbiguousAsset
        );
    }

    #[tokio::test]
    async fn a_stalled_response_is_bounded() {
        let server = serve_stall().await;
        let updater = Updater::with_bases(
            server.api(),
            server.api(),
            std::time::Duration::from_millis(200),
        )
        .expect("updater");
        let started = std::time::Instant::now();
        let error = updater
            .check(&identity("2.0.0-alpha.6"), Channel::Stable)
            .await
            .expect_err("stalled body");
        assert_eq!(error, UpdateError::Transport);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        drop(server);
    }

    #[test]
    fn official_updater_is_pinned_to_the_product_repository() {
        let updater = Updater::new().expect("official updater");
        let version = Version::parse("2.0.0").expect("version");
        let candidate = UpdateCandidate {
            tag: String::from("v2.0.0"),
            channel: Channel::Stable,
            version: version.clone(),
            descriptor: Descriptor {
                schema_version: PROTOCOL,
                product: super::super::Product::Cli,
                target: super::super::Target::LinuxX86_64Gnu,
                version,
                archive: super::super::ArchiveRef {
                    name: String::from(ARCHIVE),
                    format: super::super::ArchiveKind::TarGz,
                    sha256: String::from(DIGEST),
                    size: 4096,
                },
            },
        };
        assert_eq!(
            updater.archive_url(&candidate).expect("url").as_str(),
            format!(
                "https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/download/v2.0.0/{ARCHIVE}"
            )
        );
    }

    async fn serve_stall() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stall fixture");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("loopback address")
        );
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        // Never answer: only the client's own budget may end the wait.
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            drop(socket);
                        });
                    }
                }
            }
        });
        Server {
            base,
            stop: Some(stop),
            handle,
        }
    }

    #[tokio::test]
    async fn descriptor_size_must_match_the_release_asset() {
        let routes = vec![
            (
                releases_route(1),
                page(&[release(
                    "v2.0.0",
                    false,
                    false,
                    &[(DESCRIPTOR, 512), (ARCHIVE, 2048)],
                )]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
        ];
        assert_eq!(
            single(routes).await.expect_err("size mismatch"),
            UpdateError::SizeMismatch
        );
    }

    #[tokio::test]
    async fn duplicate_descriptors_are_ambiguous() {
        let routes = vec![(
            releases_route(1),
            page(&[release(
                "v2.0.0",
                false,
                false,
                &[(DESCRIPTOR, 512), (DESCRIPTOR, 512), (ARCHIVE, 4096)],
            )]),
        )];
        assert_eq!(
            single(routes).await.expect_err("ambiguous"),
            UpdateError::AmbiguousDescriptor
        );
    }

    #[tokio::test]
    async fn paging_reads_past_a_full_first_page() {
        let mut first = drafts(99);
        first.push(release("v1.9.0", false, false, &[]));
        let routes = vec![
            (releases_route(1), page(&first)),
            (
                releases_route(2),
                page(&[release(
                    "v2.0.0",
                    false,
                    false,
                    &[(DESCRIPTOR, 512), (ARCHIVE, 4096)],
                )]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
        ];
        let outcome = single(routes).await.expect("paged check");
        assert_eq!(
            outcome.candidate().expect("candidate").version,
            Version::parse("2.0.0").unwrap()
        );
    }

    #[tokio::test]
    async fn page_limit_is_an_error_not_an_up_to_date_answer() {
        let mut routes = Vec::new();
        for index in 1..=MAX_PAGES {
            routes.push((releases_route(index), page(&drafts(PAGE_SIZE))));
        }
        assert_eq!(
            single(routes).await.expect_err("page limit"),
            UpdateError::TooManyReleases
        );
    }

    #[tokio::test]
    async fn missing_route_is_a_status_not_an_up_to_date_answer() {
        let (server, updater) = updater(Vec::new()).await;
        let error = updater
            .check(&identity("2.0.0-alpha.6"), Channel::Stable)
            .await
            .expect_err("404 is not up to date");
        assert_eq!(error, UpdateError::HttpStatus(404));
        drop(server);
    }

    #[tokio::test]
    async fn exhausted_anonymous_quota_is_a_rate_limit() {
        let routes = vec![(releases_route(1), String::from("#status=429\n"))];
        assert_eq!(
            single(routes).await.expect_err("rate limit"),
            UpdateError::RateLimited
        );
    }

    #[test]
    fn only_github_hosts_and_crate_loopback_pass_the_redirect_policy() {
        for allowed in [
            "https://api.github.com/repos/x",
            "https://github.com/x/y/releases/download/v1/a.tar.gz",
            "https://objects.githubusercontent.com/x",
            "https://release-assets.githubusercontent.com/x",
        ] {
            assert!(permitted(&Url::parse(allowed).expect("url")), "{allowed}");
        }
        for rejected in [
            "http://github.com/x",
            "https://evil.example/x",
            "https://user:pass@github.com/x",
            "https://github.com:8443/x",
            "ftp://github.com/x",
        ] {
            assert!(
                !permitted(&Url::parse(rejected).expect("url")),
                "{rejected}"
            );
        }
    }

    #[tokio::test]
    async fn archive_url_stays_on_the_allowlisted_download_base() {
        let routes = vec![
            (
                releases_route(1),
                page(&[release(
                    "v2.0.0",
                    false,
                    false,
                    &[(DESCRIPTOR, 512), (ARCHIVE, 4096)],
                )]),
            ),
            (descriptor_route("v2.0.0"), valid_descriptor("2.0.0")),
        ];
        let (server, updater) = updater(routes).await;
        let outcome = updater
            .check(&identity("2.0.0-alpha.6"), Channel::Stable)
            .await
            .expect("check");
        let candidate = outcome.candidate().expect("candidate");
        let url = updater.archive_url(candidate).expect("archive url");
        assert_eq!(
            url.as_str(),
            format!(
                "{}/SweerItTer/BilibiliAnimeDownloader/releases/download/v2.0.0/{ARCHIVE}",
                server.base
            )
        );
        drop(server);
    }

    fn archive_candidate(bytes: &[u8], digest: String, size: u64) -> UpdateCandidate {
        let version = Version::parse("2.0.0").expect("version");
        UpdateCandidate {
            tag: "v2.0.0".into(),
            version: version.clone(),
            channel: Channel::Stable,
            descriptor: Descriptor {
                schema_version: PROTOCOL,
                product: super::super::Product::Cli,
                target: super::super::Target::LinuxX86_64Gnu,
                version,
                archive: super::super::ArchiveRef {
                    name: ARCHIVE.into(),
                    format: super::super::ArchiveKind::TarGz,
                    sha256: digest,
                    size: size.max(bytes.len() as u64),
                },
            },
        }
    }

    fn archive_route() -> String {
        format!("/SweerItTer/BilibiliAnimeDownloader/releases/download/v2.0.0/{ARCHIVE}")
    }

    #[tokio::test]
    async fn archive_download_streams_verified_bytes_to_disk() {
        let bytes = vec![b'a'; 300_000];
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let server = serve(vec![(
            archive_route(),
            String::from_utf8(bytes.clone()).expect("text fixture"),
        )])
        .await;
        let updater = Updater::with_bases(server.api(), server.api(), Duration::from_millis(200))
            .expect("updater");
        let directory = tempfile::TempDir::new().expect("tempdir");
        let destination = directory.path().join("release.tar.gz");

        let size = updater
            .download(&archive_candidate(&bytes, digest.clone(), 0), &destination)
            .await
            .expect("download");
        assert_eq!(size, bytes.len() as u64);
        assert_eq!(std::fs::read(&destination).expect("archive"), bytes);
        drop(server);
    }

    #[tokio::test]
    async fn archive_download_rejects_a_wrong_digest_or_size() {
        let bytes = vec![b'b'; 1024];
        let server = serve(vec![(
            archive_route(),
            String::from_utf8(bytes.clone()).expect("text fixture"),
        )])
        .await;
        let updater = Updater::with_bases(server.api(), server.api(), Duration::from_millis(200))
            .expect("updater");
        let directory = tempfile::TempDir::new().expect("tempdir");
        let destination = directory.path().join("release.tar.gz");

        let wrong = updater
            .download(&archive_candidate(&bytes, "0".repeat(64), 0), &destination)
            .await;
        assert_eq!(wrong, Err(UpdateError::DigestMismatch));
        assert!(
            !destination.exists(),
            "a rejected download removes its partial file"
        );

        let oversized = updater
            .download(
                &archive_candidate(&bytes, format!("{:x}", Sha256::digest(&bytes)), 4096),
                &destination,
            )
            .await;
        assert_eq!(oversized, Err(UpdateError::SizeMismatch));
        assert!(!destination.exists());
        drop(server);
    }
}
