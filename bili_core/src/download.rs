//! Crash-safe, single-resource HTTP streaming transport.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use reqwest::{header, StatusCode, Url};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::{
    api::ResolvedLocator,
    domain::{CoreError, ErrorCode, ResourceCheckpoint, Result, RetryPolicy},
};

/// Borrowed cancellation and rate controls for a single resource transfer.
pub(crate) struct TransferOptions<'a> {
    pub cancel: &'a CancellationToken,
    pub rate_limit: Option<u64>,
}

const CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
const MAX_REDIRECTS: usize = 5;
const BUFFER_SIZE: usize = 128 * 1024;

/// Stream one representation to its owned path; the adjacent sidecar is the
/// only durable authority for bytes that can be resumed after a crash.
pub(crate) async fn transfer(
    client: &reqwest::Client,
    locator: &ResolvedLocator,
    path: &Path,
    checkpoint: &mut ResourceCheckpoint,
    options: TransferOptions<'_>,
    policy: &RetryPolicy,
    progress: impl Fn(u64) + Send + Sync,
) -> Result<()> {
    let cancel = options.cancel;
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if !allowed_redirect(locator, &locator.url) {
        return Err(CoreError::new(
            ErrorCode::Transport,
            "resource URL origin rejected",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "resource path has no parent"))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(CoreError::io)?;
    crate::state::reject_symlink(path)?;
    let sidecar = checkpoint_path(path);
    crate::state::reject_symlink(&sidecar)?;

    reconcile(path, &sidecar, checkpoint, cancel).await?;
    if checkpoint.complete {
        return Ok(());
    }
    if checkpoint.durable_bytes > 0
        && !checkpoint
            .strong_etag
            .as_deref()
            .is_some_and(is_strong_etag)
    {
        reset_resource(path, checkpoint).await?;
        persist(&sidecar, checkpoint).await?;
    }

    let attempts = policy.attempts.max(1);
    for attempt in 0..attempts {
        if cancel.is_cancelled() {
            checkpoint_resource(path, &sidecar, checkpoint).await?;
            return Err(cancelled());
        }
        match stream_once(
            client, locator, path, &sidecar, checkpoint, &options, &progress,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(failure) if failure.retryable && attempt + 1 < attempts => {
                checkpoint_resource(path, &sidecar, checkpoint).await?;
                let delay = retry_delay(policy, attempt, failure.retry_after);
                tokio::select! {
                    _ = cancel.cancelled() => return Err(cancelled()),
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            Err(failure) => {
                checkpoint_resource(path, &sidecar, checkpoint).await?;
                if failure.retryable {
                    return Err(CoreError::new(
                        ErrorCode::RetryExhausted,
                        "resource transfer retries exhausted",
                    ));
                }
                return Err(failure.error);
            }
        }
    }
    Err(CoreError::new(
        ErrorCode::RetryExhausted,
        "resource transfer retries exhausted",
    ))
}

/// Load the bounded resource sidecar. `transfer` reconciles it with the owned
/// file before trusting any byte offset.
pub(crate) fn load_checkpoint(path: &Path) -> Result<ResourceCheckpoint> {
    let sidecar = checkpoint_path(path);
    match std::fs::symlink_metadata(&sidecar) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ResourceCheckpoint::default())
        }
        Err(error) => Err(CoreError::io(error)),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CoreError::new(
                ErrorCode::Conflict,
                "checkpoint sidecar is not a regular file",
            ))
        }
        Ok(metadata) if metadata.len() > 16 * 1024 => Err(CoreError::new(
            ErrorCode::Corruption,
            "checkpoint sidecar too large",
        )),
        Ok(_) => serde_json::from_slice(&std::fs::read(sidecar).map_err(CoreError::io)?)
            .map_err(|_| CoreError::new(ErrorCode::Corruption, "checkpoint sidecar is invalid")),
    }
}

async fn reconcile(
    path: &Path,
    sidecar: &Path,
    checkpoint: &mut ResourceCheckpoint,
    cancel: &CancellationToken,
) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CoreError::new(
                ErrorCode::Conflict,
                "resource path is not a regular file",
            ));
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if checkpoint.durable_bytes != 0 || checkpoint.complete {
                *checkpoint = ResourceCheckpoint::default();
                persist(sidecar, checkpoint).await?;
            }
            return Ok(());
        }
        Err(error) => return Err(CoreError::io(error)),
    };
    if metadata.len() < checkpoint.durable_bytes {
        *checkpoint = ResourceCheckpoint::default();
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .await
            .map_err(CoreError::io)?;
        file.set_len(0).await.map_err(CoreError::io)?;
        file.sync_all().await.map_err(CoreError::io)?;
        persist(sidecar, checkpoint).await?;
        return Ok(());
    }
    if metadata.len() > checkpoint.durable_bytes {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .await
            .map_err(CoreError::io)?;
        file.set_len(checkpoint.durable_bytes)
            .await
            .map_err(CoreError::io)?;
        file.sync_all().await.map_err(CoreError::io)?;
    }
    if checkpoint.durable_bytes == 0 {
        if checkpoint.complete || checkpoint.sha256.is_some() {
            *checkpoint = ResourceCheckpoint::default();
            persist(sidecar, checkpoint).await?;
        }
        return Ok(());
    }
    if checkpoint.sha256.is_none() {
        reset_resource(path, checkpoint).await?;
        persist(sidecar, checkpoint).await?;
        return Ok(());
    }
    let (_, digest) = digest_file(path, cancel).await?;
    if checkpoint.sha256.as_deref() != Some(digest.as_str())
        || checkpoint.complete && checkpoint.expected_total != Some(checkpoint.durable_bytes)
    {
        reset_resource(path, checkpoint).await?;
        persist(sidecar, checkpoint).await?;
    }
    Ok(())
}

async fn stream_once(
    client: &reqwest::Client,
    locator: &ResolvedLocator,
    path: &Path,
    sidecar: &Path,
    checkpoint: &mut ResourceCheckpoint,
    options: &TransferOptions<'_>,
    progress: &(impl Fn(u64) + Send + Sync),
) -> std::result::Result<(), TransferFailure> {
    let cancel = options.cancel;
    let rate_limit = options.rate_limit;
    let mut url = locator.url.clone();
    let mut redirects = 0;
    let mut restarted = false;
    loop {
        if cancel.is_cancelled() {
            return Err(TransferFailure::fatal(cancelled()));
        }
        let offset = checkpoint.durable_bytes;
        let (range_start, if_range) = if offset > 0 {
            let Some(etag) = checkpoint.strong_etag.as_deref() else {
                reset_resource(path, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                persist(sidecar, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                continue;
            };
            (Some(offset), Some(etag))
        } else {
            (None, None)
        };
        let mut request = client
            .get(url.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::REFERER, locator.referer.as_str());
        if let Some(start) = range_start {
            request = request.header(header::RANGE, format!("bytes={start}-"));
        }
        if let Some(etag) = if_range {
            request = request.header(header::IF_RANGE, etag);
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(TransferFailure::fatal(cancelled())),
            response = request.send() => response.map_err(|_| {
                TransferFailure::retryable(CoreError::new(ErrorCode::Transport, "resource request failed"), None)
            })?,
        };
        if response.url().origin() != url.origin() {
            return Err(TransferFailure::fatal(CoreError::new(
                ErrorCode::Transport,
                "resource redirect origin rejected",
            )));
        }
        let status = response.status();
        if status.is_redirection() {
            if redirects >= MAX_REDIRECTS {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource redirect limit exceeded",
                )));
            }
            let Some(location) = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource redirect is invalid",
                )));
            };
            let target = url.join(location).map_err(|_| {
                TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource redirect is invalid",
                ))
            })?;
            if !allowed_redirect(locator, &target) {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource redirect origin rejected",
                )));
            }
            url = target;
            redirects += 1;
            continue;
        }
        if status == StatusCode::FORBIDDEN || status == StatusCode::GONE {
            let mut error =
                CoreError::new(ErrorCode::Transport, "resource locator may have expired");
            error.retryable = true;
            return Err(TransferFailure::fatal(error));
        }
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(TransferFailure::retryable(
                CoreError::new(ErrorCode::Transport, "transient resource response"),
                parse_retry_after(response.headers()),
            ));
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
        {
            return Err(TransferFailure::fatal(CoreError::new(
                ErrorCode::Transport,
                "encoded resource response rejected",
            )));
        }
        let etag = strong_etag(response.headers());
        if status == StatusCode::RANGE_NOT_SATISFIABLE {
            let total = unsatisfied_total(response.headers());
            let (length, digest) = digest_file(path, cancel)
                .await
                .map_err(TransferFailure::fatal)?;
            if offset > 0
                && length == offset
                && total == Some(offset)
                && checkpoint.expected_total.is_none_or(|n| n == offset)
                && checkpoint
                    .strong_etag
                    .as_deref()
                    .is_some_and(is_strong_etag)
                && etag.as_deref() == checkpoint.strong_etag.as_deref()
                && checkpoint.sha256.as_deref() == Some(digest.as_str())
            {
                checkpoint.expected_total = Some(offset);
                checkpoint.sha256 = Some(digest);
                checkpoint.complete = true;
                persist(sidecar, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                return Ok(());
            }
            return Err(TransferFailure::fatal(CoreError::new(
                ErrorCode::RepresentationChanged,
                "resource range cannot be validated",
            )));
        }
        if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
            return Err(TransferFailure::fatal(CoreError::new(
                ErrorCode::Transport,
                "resource request was rejected",
            )));
        }

        let resumed = offset > 0 && status == StatusCode::PARTIAL_CONTENT;
        let mut start_at = offset;
        if status == StatusCode::PARTIAL_CONTENT {
            let range = content_range(response.headers()).ok_or_else(|| {
                TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource range response is malformed",
                ))
            })?;
            let Some(total) = range.2 else {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource range total is missing",
                )));
            };
            if range.0 != offset || range.1 < range.0 || range.1.checked_add(1) != Some(total) {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource range response does not match request",
                )));
            }
            let expected_range_length = range.1 - range.0 + 1;
            if response
                .content_length()
                .is_some_and(|length| length != expected_range_length)
            {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::Transport,
                    "resource range length is inconsistent",
                )));
            }
            if offset > 0
                && (checkpoint.strong_etag.is_none()
                    || etag.as_deref() != checkpoint.strong_etag.as_deref())
            {
                if restarted {
                    return Err(TransferFailure::fatal(CoreError::new(
                        ErrorCode::RepresentationChanged,
                        "resource validator changed",
                    )));
                }
                reset_resource(path, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                persist(sidecar, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                restarted = true;
                url = locator.url.clone();
                redirects = 0;
                continue;
            }
            if checkpoint
                .expected_total
                .is_some_and(|saved| saved != total)
                || locator
                    .expected_size
                    .is_some_and(|expected| expected != total)
            {
                if offset == 0 || restarted {
                    return Err(TransferFailure::fatal(CoreError::new(
                        ErrorCode::RepresentationChanged,
                        "resource length changed",
                    )));
                }
                reset_resource(path, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                persist(sidecar, checkpoint)
                    .await
                    .map_err(TransferFailure::fatal)?;
                restarted = true;
                url = locator.url.clone();
                redirects = 0;
                continue;
            }
            checkpoint.expected_total = Some(total);
        } else if offset > 0 {
            // A server ignored Range or If-Range: this body is a full replacement.
            start_at = 0;
            reset_resource(path, checkpoint)
                .await
                .map_err(TransferFailure::fatal)?;
            checkpoint.expected_total = response.content_length().or(locator.expected_size);
            checkpoint.strong_etag = etag.clone();
            persist(sidecar, checkpoint)
                .await
                .map_err(TransferFailure::fatal)?;
        } else {
            checkpoint.expected_total = response.content_length().or(locator.expected_size);
        }
        if let Some(remote_etag) = &etag {
            if offset > 0
                && resumed
                && remote_etag != checkpoint.strong_etag.as_ref().unwrap_or(remote_etag)
            {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::RepresentationChanged,
                    "resource validator changed",
                )));
            }
            checkpoint.strong_etag = Some(remote_etag.clone());
        } else if !resumed {
            checkpoint.strong_etag = None;
        }
        if let Some(expected) = locator.expected_size {
            if checkpoint
                .expected_total
                .is_some_and(|total| total != expected)
            {
                return Err(TransferFailure::fatal(CoreError::new(
                    ErrorCode::RepresentationChanged,
                    "resource length changed",
                )));
            }
            checkpoint.expected_total = Some(expected);
        }
        let mut hasher = if resumed {
            hash_prefix(path, cancel)
                .await
                .map_err(TransferFailure::fatal)?
        } else {
            Sha256::new()
        };
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .await
            .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
        if !resumed {
            file.set_len(0)
                .await
                .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
            checkpoint.durable_bytes = 0;
            checkpoint.sha256 = Some(format!("{:x}", Sha256::new().finalize()));
            checkpoint.complete = false;
            file.sync_all()
                .await
                .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
            persist(sidecar, checkpoint)
                .await
                .map_err(TransferFailure::fatal)?;
        } else {
            file.seek(std::io::SeekFrom::Start(start_at))
                .await
                .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
        }
        let mut received = 0u64;
        let mut last_reported = None;
        let mut last_progress_at: Option<Instant> = None;
        let mut last_checkpoint = Instant::now();
        let download_started = Instant::now();
        let expected_body = response.content_length();
        let expected_end = if status == StatusCode::PARTIAL_CONTENT {
            content_range(response.headers()).map(|r| r.1 + 1)
        } else {
            None
        };
        let mut stream = response.bytes_stream();
        loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => {
                    flush_stream_progress(&file, sidecar, checkpoint, start_at, received, &hasher).await.map_err(TransferFailure::fatal)?;
                    let current = start_at + received;
                    if last_reported != Some(current) {
                        progress(current);
                    }
                    return Err(TransferFailure::fatal(cancelled()));
                }
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = next else { break };
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(_) => {
                    file.flush()
                        .await
                        .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
                    let current = start_at + received;
                    if last_reported != Some(current) {
                        progress(current);
                    }
                    return Err(TransferFailure::retryable(
                        CoreError::new(ErrorCode::Transport, "resource stream interrupted"),
                        None,
                    ));
                }
            };
            file.write_all(&bytes)
                .await
                .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
            hasher.update(&bytes);
            received += bytes.len() as u64;
            let current = start_at + received;
            if last_reported != Some(current)
                && last_progress_at.is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL)
            {
                progress(current);
                last_reported = Some(current);
                last_progress_at = Some(Instant::now());
            }
            if let Some(limit) = rate_limit.filter(|limit| *limit > 0) {
                let fractional_nanos =
                    ((u128::from(received % limit) * 1_000_000_000) / u128::from(limit)) as u64;
                let target = Duration::from_secs(received / limit)
                    .saturating_add(Duration::from_nanos(fractional_nanos));
                let wait = target.saturating_sub(download_started.elapsed());
                if !wait.is_zero() {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            flush_stream_progress(&file, sidecar, checkpoint, start_at, received, &hasher).await.map_err(TransferFailure::fatal)?;
                            let current = start_at + received;
                            if last_reported != Some(current) {
                                progress(current);
                            }
                            return Err(TransferFailure::fatal(cancelled()));
                        }
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
            }
            if start_at + received - checkpoint.durable_bytes >= CHECKPOINT_BYTES
                || last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL
            {
                flush_stream_progress(&file, sidecar, checkpoint, start_at, received, &hasher)
                    .await
                    .map_err(TransferFailure::fatal)?;
                last_checkpoint = Instant::now();
            }
        }
        let current = start_at + received;
        if last_reported != Some(current) {
            progress(current);
        }
        if expected_body.is_some_and(|length| received != length)
            || expected_end.is_some_and(|end| start_at + received != end)
            || checkpoint
                .expected_total
                .is_some_and(|total| start_at + received != total)
        {
            file.flush()
                .await
                .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
            return Err(TransferFailure::retryable(
                CoreError::new(
                    ErrorCode::Transport,
                    "resource ended before expected length",
                ),
                None,
            ));
        }
        file.sync_all()
            .await
            .map_err(|error| TransferFailure::fatal(CoreError::io(error)))?;
        checkpoint.durable_bytes = start_at + received;
        checkpoint.sha256 = Some(format!("{:x}", hasher.finalize()));
        checkpoint.complete = false;
        if checkpoint.expected_total.is_none() {
            checkpoint.expected_total = Some(checkpoint.durable_bytes);
        }
        persist(sidecar, checkpoint)
            .await
            .map_err(TransferFailure::fatal)?;
        let (length, verified) = digest_file(path, cancel)
            .await
            .map_err(TransferFailure::fatal)?;
        if length != checkpoint.durable_bytes
            || checkpoint.sha256.as_deref() != Some(verified.as_str())
        {
            return Err(TransferFailure::fatal(CoreError::new(
                ErrorCode::Corruption,
                "resource digest verification failed",
            )));
        }
        checkpoint.complete = true;
        persist(sidecar, checkpoint)
            .await
            .map_err(TransferFailure::fatal)?;
        return Ok(());
    }
}

fn checkpoint_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".checkpoint.json");
    PathBuf::from(name)
}

async fn flush_stream_progress(
    file: &tokio::fs::File,
    sidecar: &Path,
    checkpoint: &mut ResourceCheckpoint,
    start_at: u64,
    received: u64,
    hasher: &Sha256,
) -> Result<()> {
    file.sync_all().await.map_err(CoreError::io)?;
    let durable_bytes = start_at.saturating_add(received);
    if file.metadata().await.map_err(CoreError::io)?.len() < durable_bytes {
        return Err(CoreError::new(
            ErrorCode::Filesystem,
            "resource flush was incomplete",
        ));
    }
    checkpoint.durable_bytes = durable_bytes;
    checkpoint.sha256 = Some(format!("{:x}", hasher.clone().finalize()));
    persist(sidecar, checkpoint).await
}
async fn reset_resource(path: &Path, checkpoint: &mut ResourceCheckpoint) -> Result<()> {
    match tokio::fs::OpenOptions::new().write(true).open(path).await {
        Ok(file) => {
            file.set_len(0).await.map_err(CoreError::io)?;
            file.sync_all().await.map_err(CoreError::io)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(CoreError::io(error)),
    }
    *checkpoint = ResourceCheckpoint::default();
    Ok(())
}
async fn checkpoint_resource(
    path: &Path,
    sidecar: &Path,
    checkpoint: &mut ResourceCheckpoint,
) -> Result<()> {
    match tokio::fs::OpenOptions::new().write(true).open(path).await {
        Ok(file) => {
            file.sync_all().await.map_err(CoreError::io)?;
            let length = file.metadata().await.map_err(CoreError::io)?.len();
            if length < checkpoint.durable_bytes {
                file.set_len(0).await.map_err(CoreError::io)?;
                file.sync_all().await.map_err(CoreError::io)?;
                *checkpoint = ResourceCheckpoint::default();
            } else if length > checkpoint.durable_bytes {
                file.set_len(checkpoint.durable_bytes)
                    .await
                    .map_err(CoreError::io)?;
                file.sync_all().await.map_err(CoreError::io)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            *checkpoint = ResourceCheckpoint::default();
        }
        Err(error) => return Err(CoreError::io(error)),
    }
    persist(sidecar, checkpoint).await
}
async fn persist(path: &Path, checkpoint: &ResourceCheckpoint) -> Result<()> {
    let path = path.to_owned();
    let checkpoint = checkpoint.clone();
    tokio::task::spawn_blocking(move || crate::state::atomic_json(&path, &checkpoint))
        .await
        .map_err(|_| CoreError::new(ErrorCode::Filesystem, "checkpoint writer failed"))?
}
async fn digest_file(path: &Path, cancel: &CancellationToken) -> Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path).await.map_err(CoreError::io)?;
    let length = file.metadata().await.map_err(CoreError::io)?.len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            read = file.read(&mut buffer) => read.map_err(CoreError::io)?,
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((length, format!("{:x}", hasher.finalize())))
}
async fn hash_prefix(path: &Path, cancel: &CancellationToken) -> Result<Sha256> {
    let mut file = tokio::fs::File::open(path).await.map_err(CoreError::io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            read = file.read(&mut buffer) => read.map_err(CoreError::io)?,
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher)
}
fn allowed_redirect(locator: &ResolvedLocator, url: &Url) -> bool {
    if locator.allows(url)
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
    {
        return true;
    }
    #[cfg(test)]
    {
        url.scheme() == "http"
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.host_str(), Some("127.0.0.1" | "::1"))
            && locator.url.host_str() == url.host_str()
    }
    #[cfg(not(test))]
    {
        false
    }
}
fn strong_etag(headers: &header::HeaderMap) -> Option<String> {
    let value = headers.get(header::ETAG)?.to_str().ok()?.trim();
    is_strong_etag(value).then(|| value.to_owned())
}
fn is_strong_etag(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes[0] == b'"'
        && bytes[bytes.len() - 1] == b'"'
        && bytes[1..bytes.len() - 1]
            .iter()
            .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
}
fn content_range(headers: &header::HeaderMap) -> Option<(u64, u64, Option<u64>)> {
    let value = headers
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((
        start.parse().ok()?,
        end.parse().ok()?,
        (total != "*").then(|| total.parse().ok()).flatten(),
    ))
}

fn unsatisfied_total(headers: &header::HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes */")?
        .parse()
        .ok()
}
pub(crate) fn parse_retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(60)));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .min(Duration::from_secs(60)),
    )
}
fn retry_delay(policy: &RetryPolicy, attempt: u32, retry_after: Option<Duration>) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    let base = retry_after.unwrap_or_else(|| policy.base_delay.saturating_mul(factor));
    let jitter = rand::random::<u16>() as u64 % 251;
    (base + Duration::from_millis(jitter)).min(Duration::from_secs(60))
}
fn cancelled() -> CoreError {
    CoreError::new(ErrorCode::Cancelled, "resource transfer cancelled")
}
struct TransferFailure {
    error: CoreError,
    retryable: bool,
    retry_after: Option<Duration>,
}
impl TransferFailure {
    fn fatal(error: CoreError) -> Self {
        Self {
            error,
            retryable: false,
            retry_after: None,
        }
    }
    fn retryable(mut error: CoreError, retry_after: Option<Duration>) -> Self {
        error.retryable = true;
        Self {
            error,
            retryable: true,
            retry_after,
        }
    }
}

#[cfg(test)]
mod transfer_recovery;
