//! Separate subtitle and current-pool danmaku sidecars. All remote failures are redacted.
use crate::{
    api::USER_AGENT,
    cookie::CredentialJar,
    domain::{ComponentResult, CoreError, ErrorCode, PartRef, SidecarPolicy, SubtitleSelection},
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use tokio_util::sync::CancellationToken;

const API: &str = "https://api.bilibili.com";

#[derive(Debug, Deserialize)]
struct SubtitleBody {
    body: Vec<SubtitleCue>,
}
#[derive(Debug, Deserialize)]
struct SubtitleCue {
    from: f64,
    to: f64,
    content: String,
}
#[derive(Debug, Clone, Deserialize)]
struct SubtitleTrack {
    id: Option<u64>,
    id_str: Option<String>,
    lan: Option<String>,
    #[serde(rename = "type")]
    track_type: Option<i32>,
    ai_type: Option<i32>,
    subtitle_url: Option<String>,
    #[serde(default)]
    format: Option<i32>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct SubtitleViewReply {
    #[prost(message, optional, tag = "1")]
    subtitle: Option<VideoSubtitle>,
}
#[derive(Clone, PartialEq, prost::Message)]
struct VideoSubtitle {
    #[prost(message, repeated, tag = "3")]
    subtitles: Vec<SubtitleWireItem>,
}
#[derive(Clone, PartialEq, prost::Message)]
struct SubtitleWireItem {
    #[prost(int64, tag = "1")]
    id: i64,
    #[prost(string, tag = "2")]
    id_str: String,
    #[prost(string, tag = "3")]
    lan: String,
    #[prost(string, tag = "5")]
    subtitle_url: String,
    #[prost(int32, tag = "7")]
    track_type: i32,
    #[prost(int32, tag = "9")]
    ai_type: i32,
    #[prost(int32, tag = "13")]
    format: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DanmakuComment {
    pub id: String,
    pub time_ms: u64,
    pub mode: u32,
    pub font_size: u32,
    pub color: u32,
    pub text: String,
    pub user_hash: String,
    pub date: i64,
    pub weight: i32,
    pub action: String,
    pub pool: i32,
    pub attr: i32,
    pub animation: String,
    pub colorful: i32,
    pub oid: i64,
    pub dm_from: i32,
}
fn failed(
    name: impl Into<String>,
    code: ErrorCode,
    label: &'static str,
    omissions: u64,
) -> ComponentResult {
    ComponentResult {
        name: name.into(),
        path: None,
        error: Some(CoreError::new(code, label)),
        omissions,
    }
}
fn component(name: impl Into<String>, path: PathBuf, omissions: u64) -> ComponentResult {
    ComponentResult {
        name: name.into(),
        path: Some(path),
        error: None,
        omissions,
    }
}
fn safe_label(value: &str) -> String {
    let mut out: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(48)
        .collect();
    if out.is_empty() {
        out.push_str("und");
    }
    out
}
fn track_label(track: &SubtitleTrack) -> String {
    let origin = match (track.track_type, track.ai_type) {
        (Some(1), Some(1)) => "translated",
        (Some(1), _) => "ai",
        (Some(0), _) => "human",
        _ => "subtitle",
    };
    format!(
        "{}.{}",
        safe_label(track.lan.as_deref().unwrap_or("und")),
        origin
    )
}
fn track_id(track: &SubtitleTrack) -> String {
    track
        .id_str
        .clone()
        .or_else(|| track.id.map(|v| v.to_string()))
        .unwrap_or_else(|| "unknown".into())
}
fn selected(policy: &SubtitleSelection, track: &SubtitleTrack) -> bool {
    match policy {
        SubtitleSelection::None => false,
        SubtitleSelection::All => true,
        SubtitleSelection::Languages(languages) => languages.iter().any(|l| {
            track
                .lan
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case(l))
        }),
        SubtitleSelection::Tracks(ids) => ids.iter().any(|id| id == &track_id(track)),
    }
}
fn subtitle_api_url(part: &PartRef) -> Url {
    let mut url =
        Url::parse(&format!("{API}/x/v2/subtitle/web/view")).expect("static subtitle API URL");
    url.query_pairs_mut()
        .append_pair("oid", &part.cid.get().to_string())
        .append_pair("pid", &part.aid.get().to_string())
        .append_pair("type", "1");
    url
}
async fn subtitle_view(
    client: &Client,
    jar: &Arc<RwLock<CredentialJar>>,
    url: Url,
    cancel: &CancellationToken,
) -> Result<Vec<SubtitleTrack>, CoreError> {
    let mut request = client
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, "https://www.bilibili.com/");
    if let Ok(jar) = jar.read() {
        if let Some(cookie) = jar.header_for(&url) {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
    }
    let response = send(request, cancel, "subtitle discovery").await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(CoreError::new(
            ErrorCode::LoginRequired,
            "subtitle access required",
        ));
    }
    if !response.status().is_success() {
        return Err(CoreError::new(ErrorCode::Transport, "subtitle response"));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_json = content_type.contains("json");
    let is_protobuf = content_type.starts_with("application/octet-stream");
    let payload = limited_body(response, 8 * 1024 * 1024, "subtitle payload size", cancel).await?;
    if is_json {
        let value: serde_json::Value = serde_json::from_slice(&payload)
            .map_err(|_| CoreError::new(ErrorCode::Corruption, "subtitle error payload"))?;
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default();
        let error = if code == -101 || code == -404 {
            ErrorCode::LoginRequired
        } else {
            ErrorCode::Corruption
        };
        return Err(CoreError::new(error, "subtitle API error response"));
    }
    if !is_protobuf {
        return Err(CoreError::new(
            ErrorCode::Corruption,
            "subtitle response content type",
        ));
    }
    decode_subtitle_view(&payload)
}
fn decode_subtitle_view(payload: &[u8]) -> Result<Vec<SubtitleTrack>, CoreError> {
    use prost::Message;
    let reply = SubtitleViewReply::decode(payload)
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "subtitle protobuf payload"))?;
    let subtitle = reply
        .subtitle
        .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "subtitle view field missing"))?;
    Ok(subtitle
        .subtitles
        .into_iter()
        .map(|item| SubtitleTrack {
            id: (item.id > 0).then_some(item.id as u64),
            id_str: (!item.id_str.is_empty()).then_some(item.id_str),
            lan: (!item.lan.is_empty()).then_some(item.lan),
            track_type: Some(item.track_type),
            ai_type: Some(item.ai_type),
            subtitle_url: (!item.subtitle_url.is_empty()).then_some(item.subtitle_url),
            format: Some(item.format),
        })
        .collect())
}
fn dm_url(route: &str, part: &PartRef) -> Url {
    let mut url = Url::parse(&format!("{API}{route}")).expect("static danmaku API URL");
    url.query_pairs_mut()
        .append_pair("type", "1")
        .append_pair("oid", &part.cid.get().to_string())
        .append_pair("pid", &part.aid.get().to_string());
    url
}
fn subtitle_url(raw: &str) -> Result<Url, CoreError> {
    let parsed = if raw.starts_with("//") {
        Url::parse(&format!("https:{raw}"))
    } else if raw.starts_with('/') {
        Url::parse(&format!("{API}{raw}"))
    } else {
        Url::parse(raw)
    }
    .map_err(|_| CoreError::new(ErrorCode::InvalidInput, "subtitle URL"))?;
    let host = parsed.host_str().unwrap_or_default();
    if parsed.scheme() != "https"
        || !(host == "bilibili.com"
            || host.ends_with(".bilibili.com")
            || host == "hdslb.com"
            || host.ends_with(".hdslb.com"))
    {
        return Err(CoreError::new(ErrorCode::InvalidInput, "subtitle URL"));
    }
    Ok(parsed)
}
async fn send(
    request: reqwest::RequestBuilder,
    cancel: &CancellationToken,
    operation: &'static str,
) -> Result<reqwest::Response, CoreError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(CoreError::new(ErrorCode::Cancelled, "sidecar cancelled")),
        response = request.send() => response.map_err(|_| CoreError::new(ErrorCode::Transport, operation)),
    }
}
async fn limited_body(
    response: reqwest::Response,
    limit: usize,
    operation: &'static str,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, CoreError> {
    use futures_util::StreamExt;
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(CoreError::new(ErrorCode::Corruption, operation));
    }
    let mut stream = response.bytes_stream();
    let mut data = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::new(ErrorCode::Cancelled, "sidecar cancelled")),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|_| CoreError::new(ErrorCode::Transport, operation))?;
        if data.len().saturating_add(chunk.len()) > limit {
            return Err(CoreError::new(ErrorCode::Corruption, operation));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
async fn bytes(
    client: &Client,
    jar: &Arc<RwLock<CredentialJar>>,
    url: Url,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, CoreError> {
    if url.scheme() != "https" {
        return Err(CoreError::new(ErrorCode::InvalidInput, "sidecar URL"));
    }
    let mut request = client
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, "https://www.bilibili.com/");
    if let Ok(jar) = jar.read() {
        if let Some(cookie) = jar.header_for(&url) {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
    }
    let response = send(request, cancel, "sidecar download").await?;
    if !response.status().is_success() {
        return Err(CoreError::new(ErrorCode::Transport, "sidecar download"));
    }
    limited_body(response, 32 * 1024 * 1024, "sidecar size limit", cancel).await
}

/// Download requested sidecars only; a failure is retained per component and never affects video.
pub(crate) async fn download(
    part: &PartRef,
    policy: &SidecarPolicy,
    basename: &Path,
    client: &Client,
    jar: &Arc<RwLock<CredentialJar>>,
    cancel: &CancellationToken,
) -> Vec<ComponentResult> {
    let mut result = Vec::new();
    if cancel.is_cancelled() {
        return vec![failed(
            "sidecars",
            ErrorCode::Cancelled,
            "sidecar cancelled",
            0,
        )];
    }
    if !matches!(policy.subtitles, SubtitleSelection::None) {
        result.extend(
            download_subtitles(part, &policy.subtitles, basename, client, jar, cancel).await,
        );
    }
    if policy.danmaku {
        result.extend(download_danmaku(part, basename, client, jar, cancel).await);
    }
    result
}

async fn download_subtitles(
    part: &PartRef,
    policy: &SubtitleSelection,
    basename: &Path,
    client: &Client,
    jar: &Arc<RwLock<CredentialJar>>,
    cancel: &CancellationToken,
) -> Vec<ComponentResult> {
    let new_tracks = match subtitle_view(client, jar, subtitle_api_url(part), cancel).await {
        Ok(tracks) => tracks,
        Err(error) => {
            return vec![failed(
                "subtitles",
                error.code,
                "subtitle discovery failed",
                0,
            )]
        }
    };
    let mut tracks = BTreeMap::<String, SubtitleTrack>::new();
    for track in new_tracks {
        tracks.insert(track_id(&track), track);
    }
    if tracks.is_empty() {
        return match policy {
            SubtitleSelection::All => vec![ComponentResult {
                name: "subtitles".into(),
                path: None,
                error: None,
                omissions: 0,
            }],
            _ => vec![failed(
                "subtitles",
                ErrorCode::Unavailable,
                "selected subtitle tracks unavailable",
                0,
            )],
        };
    }
    let known: Vec<(String, SubtitleTrack)> = tracks
        .iter()
        .map(|(id, track)| (id.clone(), track.clone()))
        .collect();
    let mut results = Vec::new();
    for (identity_id, track) in tracks {
        let id = safe_label(&identity_id);
        if cancel.is_cancelled() {
            results.push(failed(
                format!("subtitle-{id}"),
                ErrorCode::Cancelled,
                "sidecar cancelled",
                0,
            ));
            break;
        }
        if !selected(policy, &track) {
            continue;
        }
        let Some(raw_url) = track.subtitle_url.as_deref() else {
            results.push(failed(
                format!("subtitle-{id}"),
                ErrorCode::Unavailable,
                "subtitle URL unavailable",
                0,
            ));
            continue;
        };
        let Ok(url) = transformed_subtitle_url(raw_url) else {
            results.push(failed(
                format!("subtitle-{id}"),
                ErrorCode::Corruption,
                "subtitle URL invalid",
                0,
            ));
            continue;
        };
        let is_ass = match track.format {
            Some(0) => false,
            Some(1) => true,
            Some(_) => {
                results.push(failed(
                    format!("subtitle-{id}"),
                    ErrorCode::Unavailable,
                    "subtitle format unsupported",
                    0,
                ));
                continue;
            }
            None => url.path().to_ascii_lowercase().ends_with(".ass"),
        };
        let label = track_label(&track);
        if is_ass {
            let Some(path) = sidecar_path(basename, &format!(".{label}.{}.ass", safe_label(&id)))
            else {
                results.push(failed(
                    format!("subtitle-{id}-ass"),
                    ErrorCode::Filesystem,
                    "sidecar path invalid",
                    0,
                ));
                continue;
            };
            let identity = subtitle_identity(part, &track, &identity_id, "ass");
            match existing_component(&path, &identity, cancel).await {
                Ok(true) => {
                    results.push(component(format!("subtitle-{id}-ass"), path, 0));
                    continue;
                }
                Err(error) => {
                    results.push(ComponentResult {
                        name: format!("subtitle-{id}-ass"),
                        path: None,
                        error: Some(error),
                        omissions: 0,
                    });
                    continue;
                }
                Ok(false) => {}
            }
            match bytes(client, jar, url, cancel).await {
                Ok(data) => match write_owned_component(path.clone(), data, identity, cancel).await
                {
                    Ok(()) => results.push(component(format!("subtitle-{id}-ass"), path, 0)),
                    Err(error) => results.push(ComponentResult {
                        name: format!("subtitle-{id}-ass"),
                        path: None,
                        error: Some(error),
                        omissions: 0,
                    }),
                },
                Err(error) => results.push(ComponentResult {
                    name: format!("subtitle-{id}-ass"),
                    path: None,
                    error: Some(error),
                    omissions: 0,
                }),
            }
            continue;
        }
        let Some(raw_path) =
            sidecar_path(basename, &format!(".{label}.{}.raw.json", safe_label(&id)))
        else {
            results.push(failed(
                format!("subtitle-{id}-raw"),
                ErrorCode::Filesystem,
                "sidecar path invalid",
                0,
            ));
            continue;
        };
        let Some(srt_path) = sidecar_path(basename, &format!(".{label}.{}.srt", safe_label(&id)))
        else {
            results.push(failed(
                format!("subtitle-{id}-srt"),
                ErrorCode::Filesystem,
                "sidecar path invalid",
                0,
            ));
            continue;
        };
        let raw_identity = subtitle_identity(part, &track, &identity_id, "raw-json");
        let srt_identity = subtitle_identity(part, &track, &identity_id, "srt");
        let raw_exists = existing_component(&raw_path, &raw_identity, cancel).await;
        let srt_exists = existing_component(&srt_path, &srt_identity, cancel).await;
        if let Err(error) = &raw_exists {
            results.push(ComponentResult {
                name: format!("subtitle-{id}-raw"),
                path: None,
                error: Some(error.clone()),
                omissions: 0,
            });
        }
        if let Err(error) = &srt_exists {
            results.push(ComponentResult {
                name: format!("subtitle-{id}-srt"),
                path: None,
                error: Some(error.clone()),
                omissions: 0,
            });
        }
        let raw_valid = raw_exists.as_ref().is_ok_and(|exists| *exists);
        let srt_valid = srt_exists.as_ref().is_ok_and(|exists| *exists);
        let raw_missing = raw_exists.as_ref().is_ok_and(|exists| !*exists);
        let srt_missing = srt_exists.as_ref().is_ok_and(|exists| !*exists);
        if raw_valid && srt_valid {
            results.push(component(format!("subtitle-{id}-raw"), raw_path, 0));
            results.push(component(format!("subtitle-{id}-srt"), srt_path, 0));
            continue;
        }
        if raw_exists.is_err() && srt_exists.is_err() {
            continue;
        }
        match bytes(client, jar, url, cancel).await {
            Ok(data) => match serde_json::from_slice::<SubtitleBody>(&data) {
                Ok(mut body) => {
                    let initial = body.body.len();
                    body.body.retain(|cue| {
                        cue.from.is_finite()
                            && cue.to.is_finite()
                            && cue.from >= 0.0
                            && cue.to > cue.from
                            && cue.to <= 86_400.0
                    });
                    let omitted = (initial - body.body.len()) as u64;
                    body.body.sort_by(|a, b| {
                        a.from
                            .total_cmp(&b.from)
                            .then_with(|| a.to.total_cmp(&b.to))
                    });
                    let srt = cues_to_srt(&body.body).into_bytes();
                    if raw_missing {
                        results.push(
                            match write_owned_component(
                                raw_path.clone(),
                                data,
                                raw_identity,
                                cancel,
                            )
                            .await
                            {
                                Ok(()) => component(format!("subtitle-{id}-raw"), raw_path, 0),
                                Err(error) => ComponentResult {
                                    name: format!("subtitle-{id}-raw"),
                                    path: None,
                                    error: Some(error),
                                    omissions: 0,
                                },
                            },
                        );
                    } else if raw_valid {
                        results.push(component(format!("subtitle-{id}-raw"), raw_path, 0));
                    }
                    if srt_missing {
                        results.push(
                            match write_owned_component(srt_path.clone(), srt, srt_identity, cancel)
                                .await
                            {
                                Ok(()) => {
                                    component(format!("subtitle-{id}-srt"), srt_path, omitted)
                                }
                                Err(error) => ComponentResult {
                                    name: format!("subtitle-{id}-srt"),
                                    path: None,
                                    error: Some(error),
                                    omissions: omitted,
                                },
                            },
                        );
                    } else if srt_valid {
                        results.push(component(format!("subtitle-{id}-srt"), srt_path, omitted));
                    }
                }
                Err(_) => {
                    if raw_missing {
                        results.push(failed(
                            format!("subtitle-{id}-raw"),
                            ErrorCode::Corruption,
                            "subtitle raw payload",
                            0,
                        ));
                    }
                    if srt_missing {
                        results.push(failed(
                            format!("subtitle-{id}-srt"),
                            ErrorCode::Corruption,
                            "subtitle cue payload",
                            0,
                        ));
                    }
                }
            },
            Err(error) => {
                if raw_missing {
                    results.push(ComponentResult {
                        name: format!("subtitle-{id}-raw"),
                        path: None,
                        error: Some(error.clone()),
                        omissions: 0,
                    });
                }
                if srt_missing {
                    results.push(ComponentResult {
                        name: format!("subtitle-{id}-srt"),
                        path: None,
                        error: Some(error),
                        omissions: 0,
                    });
                }
            }
        }
    }
    match policy {
        SubtitleSelection::Languages(languages) => {
            for language in languages {
                if !known.iter().any(|(_, track)| {
                    track
                        .lan
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case(language))
                }) {
                    results.push(failed(
                        format!("subtitle-language-{}", safe_label(language)),
                        ErrorCode::Unavailable,
                        "selected subtitle language unavailable",
                        0,
                    ));
                }
            }
        }
        SubtitleSelection::Tracks(ids) => {
            for id in ids {
                if !known.iter().any(|(known_id, _)| known_id == id) {
                    results.push(failed(
                        format!("subtitle-track-{}", safe_label(id)),
                        ErrorCode::Unavailable,
                        "selected subtitle track unavailable",
                        0,
                    ));
                }
            }
        }
        SubtitleSelection::All | SubtitleSelection::None => {}
    }
    if results.is_empty() {
        results.push(failed(
            "subtitles",
            ErrorCode::Unavailable,
            "selected subtitle tracks unavailable",
            0,
        ));
    }
    results
}
fn transformed_subtitle_url(raw: &str) -> Result<Url, CoreError> {
    let original = subtitle_url(raw)?;
    if original.host_str() != Some("subtitle.bilibili.com") {
        return Ok(original);
    }
    let encoded = original
        .path()
        .strip_prefix('/')
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "subtitle URL"))?;
    let decoded = percent_decode(encoded)
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "subtitle URL"))?;
    const PAIRS: [(&str, &str); 2] = [
        (
            r#"nP](wOFRvU.+<fjS{jn-!$D|Dz&",zT`"#,
            r#"=CFxYRn{.y|uVyO$uh&sikph?N.ilF/`"#,
        ),
        (
            r#"Bn"q~|albg@]Go~ACgyDvKnd+)_D}^&J?"#,
            r#"Cu~L!xs~f^&r@'vh=q]q{eeng*sEg^kp#J"#,
        ),
    ];
    for (prefix, key) in PAIRS {
        if let Some(path) = xor_js_string(&decoded, &format!("{key}bilibili"))
            .filter(|value| value.starts_with(prefix))
            .map(|value| value[prefix.len()..].to_owned())
        {
            if !path.starts_with('/') || path.contains('\\') {
                return Err(CoreError::new(ErrorCode::InvalidInput, "subtitle URL"));
            }
            let mut transformed = Url::parse("https://aisubtitle.hdslb.com")
                .map_err(|_| CoreError::new(ErrorCode::InvalidInput, "subtitle URL"))?;
            transformed.set_path(&path);
            transformed.set_query(original.query());
            return subtitle_url(transformed.as_str());
        }
    }
    Ok(original)
}
fn percent_decode(value: &str) -> Option<String> {
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(|value| value.into_owned())
}
fn xor_js_string(value: &str, key: &str) -> Option<String> {
    let key: Vec<u16> = key.encode_utf16().collect();
    if key.is_empty() {
        return None;
    }
    let output: Vec<u16> = value
        .encode_utf16()
        .enumerate()
        .map(|(index, unit)| unit ^ key[index % key.len()])
        .collect();
    String::from_utf16(&output).ok()
}
fn sidecar_path(base: &Path, suffix: &str) -> Option<PathBuf> {
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    let name = base.file_stem()?.to_str()?;
    Some(parent.join(format!("{name}{suffix}")))
}
async fn existing_component(
    path: &Path,
    identity: &str,
    cancel: &CancellationToken,
) -> Result<bool, CoreError> {
    let path = path.to_owned();
    let identity = identity.to_owned();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || crate::state::component_exists(&path, &identity, &cancel))
        .await
        .map_err(|_| {
            CoreError::new(
                ErrorCode::Filesystem,
                "component verification worker failed",
            )
        })?
}
async fn write_owned_component(
    path: PathBuf,
    data: Vec<u8>,
    identity: String,
    cancel: &CancellationToken,
) -> Result<(), CoreError> {
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        crate::state::write_component(&path, &data, &identity, &cancel)
    })
    .await
    .map_err(|_| CoreError::new(ErrorCode::Filesystem, "component writer failed"))?
}
fn subtitle_identity(
    part: &PartRef,
    track: &SubtitleTrack,
    track_id: &str,
    format: &str,
) -> String {
    format!(
        "subtitle:{}:{}:{}:{}:{}:{}",
        part.aid,
        part.cid,
        track_id,
        track.lan.as_deref().unwrap_or("und"),
        track_label(track),
        format
    )
}
fn danmaku_identity(part: &PartRef, format: &str) -> String {
    format!("danmaku:{}:{}:{format}", part.aid, part.cid)
}
fn time_srt(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round().max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1_000) % 60,
        ms % 1000
    )
}
fn cues_to_srt(cues: &[SubtitleCue]) -> String {
    let mut out = String::new();
    for (i, cue) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            time_srt(cue.from),
            time_srt(cue.to),
            cue.content.replace('\0', "")
        ));
    }
    out
}

const MAX_DANMAKU_SEGMENTS: u64 = 100_000;
const MAX_DANMAKU_BYTES: usize = 128 * 1024 * 1024;
async fn download_danmaku(
    part: &PartRef,
    basename: &Path,
    client: &Client,
    jar: &Arc<RwLock<CredentialJar>>,
    cancel: &CancellationToken,
) -> Vec<ComponentResult> {
    let (Some(raw_path), Some(xml_path), Some(ass_path)) = (
        sidecar_path(basename, ".danmaku.jsonl"),
        sidecar_path(basename, ".danmaku.xml"),
        sidecar_path(basename, ".danmaku.ass"),
    ) else {
        return vec![failed(
            "danmaku",
            ErrorCode::Filesystem,
            "sidecar path invalid",
            0,
        )];
    };
    let names = ["danmaku-raw", "danmaku-xml", "danmaku-ass"];
    let paths = [raw_path, xml_path, ass_path];
    let identities = [
        danmaku_identity(part, "jsonl"),
        danmaku_identity(part, "xml"),
        danmaku_identity(part, "ass"),
    ];
    let mut already_owned = [false; 3];
    for index in 0..3 {
        match existing_component(&paths[index], &identities[index], cancel).await {
            Ok(exists) => already_owned[index] = exists,
            Err(error) => {
                return vec![ComponentResult {
                    name: names[index].into(),
                    path: None,
                    error: Some(error),
                    omissions: 0,
                }]
            }
        }
    }
    if already_owned.iter().all(|exists| *exists) {
        return (0..3)
            .map(|index| component(names[index], paths[index].clone(), 0))
            .collect();
    }
    let data = match binary(client, jar, dm_url("/x/v2/dm/web/view", part), cancel).await {
        Ok(data) => data,
        Err(error) => {
            return vec![ComponentResult {
                name: "danmaku".into(),
                path: None,
                error: Some(error),
                omissions: 0,
            }]
        }
    };
    let (segment_duration_ms, segment_count) = match decode_view(&data) {
        Ok(value) => value,
        Err(()) => {
            return vec![failed(
                "danmaku",
                ErrorCode::Corruption,
                "danmaku view payload",
                0,
            )]
        }
    };
    if segment_duration_ms == 0 || segment_count > MAX_DANMAKU_SEGMENTS {
        return vec![failed(
            "danmaku",
            ErrorCode::Corruption,
            "danmaku segment limit",
            0,
        )];
    }
    let mut comments: Vec<DanmakuComment> = Vec::new();
    let mut missing = 0u64;
    let mut malformed = 0u64;
    let mut total_bytes = 0usize;
    for segment in 1..=segment_count {
        if cancel.is_cancelled() {
            return vec![failed(
                "danmaku",
                ErrorCode::Cancelled,
                "sidecar cancelled",
                0,
            )];
        }
        let mut url = dm_url("/x/v2/dm/web/seg.so", part);
        url.query_pairs_mut()
            .append_pair("segment_index", &segment.to_string());
        match binary(client, jar, url, cancel).await {
            Ok(bytes) => {
                total_bytes = total_bytes.saturating_add(bytes.len());
                if total_bytes > MAX_DANMAKU_BYTES {
                    return vec![failed(
                        "danmaku",
                        ErrorCode::Corruption,
                        "danmaku aggregate size limit",
                        0,
                    )];
                }
                match decode_segment_details(&bytes, segment) {
                    Ok(mut batch) => comments.append(&mut batch),
                    Err(_) => malformed += 1,
                }
            }
            Err(error) if error.code == ErrorCode::Cancelled => {
                return vec![failed(
                    "danmaku",
                    ErrorCode::Cancelled,
                    "sidecar cancelled",
                    0,
                )]
            }
            Err(_) => missing += 1,
        }
    }
    if missing > 0 || malformed > 0 {
        let (code, label) = if malformed > 0 {
            (ErrorCode::Corruption, "danmaku segment payload")
        } else {
            (ErrorCode::Transport, "danmaku segments missing")
        };
        return vec![failed("danmaku", code, label, missing + malformed)];
    }
    comments.sort_by(|a, b| a.time_ms.cmp(&b.time_ms).then_with(|| a.id.cmp(&b.id)));
    let omissions = 0;
    let estimate = comments.iter().fold(0usize, |total, comment| {
        total
            .saturating_add(comment.text.len().saturating_mul(20))
            .saturating_add(comment.id.len().saturating_mul(12))
            .saturating_add(comment.user_hash.len().saturating_mul(6))
            .saturating_add(comment.action.len().saturating_mul(6))
            .saturating_add(comment.animation.len().saturating_mul(6))
            .saturating_add(512)
    });
    if estimate > MAX_DANMAKU_BYTES {
        return vec![failed(
            "danmaku",
            ErrorCode::Corruption,
            "danmaku export size limit",
            omissions,
        )];
    }
    let raw = danmaku_jsonl(&comments).into_bytes();
    let xml = danmaku_xml(&comments).into_bytes();
    let (ass, ass_omitted) = danmaku_ass_with_omissions(&comments);
    let outputs = [
        (raw, omissions),
        (xml, omissions),
        (ass.into_bytes(), omissions + ass_omitted),
    ];
    let mut results = Vec::new();
    for (index, (bytes, omissions)) in outputs.into_iter().enumerate() {
        match write_owned_component(
            paths[index].clone(),
            bytes,
            identities[index].clone(),
            cancel,
        )
        .await
        {
            Ok(()) => results.push(component(names[index], paths[index].clone(), omissions)),
            Err(error) => results.push(ComponentResult {
                name: names[index].into(),
                path: None,
                error: Some(error),
                omissions,
            }),
        }
    }
    results
}
async fn binary(
    client: &Client,
    jar: &Arc<RwLock<CredentialJar>>,
    url: Url,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, CoreError> {
    let mut request = client
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, "https://www.bilibili.com/");
    if let Ok(jar) = jar.read() {
        if let Some(cookie) = jar.header_for(&url) {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
    }
    let response = send(request, cancel, "danmaku segment").await?;
    if !response.status().is_success() {
        return Err(CoreError::new(ErrorCode::Transport, "danmaku segment"));
    }
    limited_body(response, 16 * 1024 * 1024, "danmaku segment size", cancel).await
}

#[derive(Clone, PartialEq, prost::Message)]
struct DmWebViewReply {
    #[prost(message, optional, tag = "4")]
    dm_sge: Option<DmSegmentConfig>,
    #[prost(int64, tag = "8")]
    count: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct DmSegmentConfig {
    #[prost(int64, tag = "1")]
    page_size: i64,
    #[prost(int64, tag = "2")]
    total: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct DmSegMobileReply {
    #[prost(message, repeated, tag = "1")]
    elems: Vec<DmElem>,
}
#[derive(Clone, PartialEq, prost::Message)]
struct DmElem {
    #[prost(int32, tag = "2")]
    progress: i32,
    #[prost(int32, tag = "3")]
    mode: i32,
    #[prost(int32, tag = "4")]
    fontsize: i32,
    #[prost(uint32, tag = "5")]
    color: u32,
    #[prost(string, tag = "6")]
    user_hash: String,
    #[prost(string, tag = "7")]
    content: String,
    #[prost(int64, tag = "8")]
    date: i64,
    #[prost(int32, tag = "9")]
    weight: i32,
    #[prost(string, tag = "10")]
    action: String,
    #[prost(int32, tag = "11")]
    pool: i32,
    #[prost(string, tag = "12")]
    id_str: String,
    #[prost(int32, tag = "13")]
    attr: i32,
    #[prost(string, tag = "22")]
    animation: String,
    #[prost(int32, tag = "24")]
    colorful: i32,
    #[prost(int64, tag = "26")]
    oid: i64,
    #[prost(int32, tag = "27")]
    dm_from: i32,
}
fn decode_view(data: &[u8]) -> Result<(u64, u64), ()> {
    use prost::Message;
    let reply = DmWebViewReply::decode(data).map_err(|_| ())?;
    let config = reply.dm_sge.ok_or(())?;
    if config.page_size <= 0 || config.total < 0 || reply.count < 0 {
        return Err(());
    }
    Ok((config.page_size as u64, config.total as u64))
}

/// Decode one Bilibili current-pool segment; protobuf unknown fields remain forward-compatible.
pub fn decode_danmaku_segment(data: &[u8]) -> Result<Vec<DanmakuComment>, &'static str> {
    decode_segment_details(data, 1)
}
fn decode_segment_details(data: &[u8], segment: u64) -> Result<Vec<DanmakuComment>, &'static str> {
    use prost::Message;
    let reply = DmSegMobileReply::decode(data).map_err(|_| "invalid danmaku segment")?;
    let comments = reply
        .elems
        .into_iter()
        .enumerate()
        .map(|(index, elem)| {
            let id = if elem.id_str.is_empty() {
                format!("segment-{segment}-item-{index}")
            } else {
                elem.id_str
            };
            DanmakuComment {
                id,
                time_ms: elem.progress.max(0) as u64,
                mode: elem.mode.max(0) as u32,
                font_size: elem.fontsize.max(0) as u32,
                color: elem.color,
                text: elem.content,
                user_hash: elem.user_hash,
                date: elem.date,
                weight: elem.weight,
                action: elem.action,
                pool: elem.pool,
                attr: elem.attr,
                animation: elem.animation,
                colorful: elem.colorful,
                oid: elem.oid,
                dm_from: elem.dm_from,
            }
        })
        .collect();
    Ok(comments)
}

pub fn danmaku_jsonl(comments: &[DanmakuComment]) -> String {
    comments
        .iter()
        .filter_map(|comment| {
            serde_json::to_string(comment)
                .ok()
                .map(|line| format!("{line}\n"))
        })
        .collect()
}
pub fn danmaku_xml(comments: &[DanmakuComment]) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><i><chatserver>chat.bilibili.com</chatserver><chatid>0</chatid><mission>0</mission><maxlimit>0</maxlimit><state>0</state><real_name>0</real_name><source>k-v</source>"#,
    );
    for comment in comments {
        xml.push_str(&format!(
            r#"<d p="{:.3},{},{},{},{},{},{},{}">{}</d>"#,
            comment.time_ms as f64 / 1000.0,
            comment.mode,
            comment.font_size,
            comment.color,
            comment.date,
            comment.pool,
            xml_escape(&comment.user_hash),
            xml_escape(&comment.id),
            xml_escape(&comment.text)
        ));
    }
    xml.push_str("</i>");
    xml
}
pub fn danmaku_ass(comments: &[DanmakuComment]) -> String {
    danmaku_ass_with_omissions(comments).0
}
pub fn danmaku_ass_with_omissions(comments: &[DanmakuComment]) -> (String, u64) {
    let mut out = String::from("[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H64000000,0,0,1,2,0,7,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    let mut order: Vec<usize> = (0..comments.len()).collect();
    order.sort_by(|&a, &b| {
        comments[a]
            .time_ms
            .cmp(&comments[b].time_ms)
            .then_with(|| comments[a].id.cmp(&comments[b].id))
    });
    let mut scroll_end = [0u64; 18];
    let mut top_end = [0u64; 4];
    let mut bottom_end = [0u64; 4];
    let mut omitted = 0;
    for index in order {
        let comment = &comments[index];
        if !matches!(comment.mode, 1 | 4 | 5 | 6) {
            omitted += 1;
            continue;
        }
        let (lanes, group) = match comment.mode {
            4 => (&mut bottom_end[..], 1),
            5 => (&mut top_end[..], 0),
            _ => (&mut scroll_end[..], 2),
        };
        let Some(lane) = lanes.iter().position(|end| *end <= comment.time_ms) else {
            omitted += 1;
            continue;
        };
        lanes[lane] = comment.time_ms.saturating_add(8_000);
        out.push_str(&ass_line(comment, lane, group));
    }
    (out, omitted)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
}
fn ass_escape(s: &str) -> String {
    let slash = char::from(92);
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            // libass has no "\\" escape: separate literal slashes from N/n/h
            // controls and brace escapes with a rendered zero-width character.
            '\\' => {
                out.push(slash);
                out.push('\u{200b}');
            }
            '{' | '}' => {
                out.push(slash);
                out.push(ch);
            }
            '\n' => {
                out.push(slash);
                out.push('N');
            }
            '\r' | '\0' => {}
            _ => out.push(ch),
        }
    }
    out
}
fn ass_line(c: &DanmakuComment, lane: usize, group: u8) -> String {
    let t = c.time_ms as f64 / 1000.0;
    let start = ass_time(t);
    let end = ass_time(t + 8.0);
    let size = c.font_size.clamp(10, 72);
    let r = (c.color >> 16) & 0xff;
    let g = (c.color >> 8) & 0xff;
    let b = c.color & 0xff;
    let color = format!("{b:02X}{g:02X}{r:02X}");
    let y = match group {
        0 => 30 + lane as u32 * 42,
        1 => 760 - lane as u32 * 38,
        _ => 100 + lane as u32 * 38,
    };
    let tag = match group {
        0 => format!(r"{{\an8\pos(960,{y})\fs{size}\c&H{color}&}}"),
        1 => format!(r"{{\an2\pos(960,{y})\fs{size}\c&H{color}&}}"),
        _ if c.mode == 6 => format!(r"{{\move(0,{y},1920,{y},0,8000)\fs{size}\c&H{color}&}}"),
        _ => format!(r"{{\move(1920,{y},0,{y},0,8000)\fs{size}\c&H{color}&}}"),
    };
    format!(
        "Dialogue: 0,{start},{end},Default,,0,0,0,,{tag}{}\n",
        ass_escape(&c.text)
    )
}
fn ass_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360000,
        (cs / 6000) % 60,
        (cs / 100) % 60,
        cs % 100
    )
}
#[cfg(test)]
mod wire_contract_tests {
    use super::*;

    #[test]
    fn danmaku_view_total_is_not_the_comment_count() {
        let body = [
            0x22, 0x06, 0x08, 0xc0, 0xfc, 0x15, 0x10, 0x02, 0x40, 0xd0, 0x0f,
        ];
        assert_eq!(decode_view(&body).unwrap(), (360_000, 2));
    }

    #[test]
    fn modern_subtitle_wire_distinguishes_present_empty_from_missing_field() {
        assert!(decode_subtitle_view(&[0x0a, 0x00]).unwrap().is_empty());
        assert!(decode_subtitle_view(&[]).is_err());
        assert!(decode_subtitle_view(br#"{"code":-400}"#).is_err());
    }

    #[test]
    fn recognized_subtitle_url_ciphertext_uses_first_party_transform() {
        let input = "https://subtitle.bilibili.com/S%13%1BP.%1D%28%29X%2CR%5Ej%1F%25w%0E%02H%5EHO4%14%7B4%08K%40%3C%7B%00M%0B%0A%1AM%1A%19%0BI%2A2%14%3C%7D%1D%02%40%0D%14%10%22%10%2C%0A%1F%1BI%1D?synthetic=1";
        assert_eq!(
            transformed_subtitle_url(input).unwrap().as_str(),
            "https://aisubtitle.hdslb.com/bfs/subtitle/synthetic.json?synthetic=1"
        );
    }

    #[test]
    fn direct_subtitle_urls_are_not_rewritten() {
        let input = "https://subtitle.bilibili.com/bfs/subtitle/direct.json?token=synthetic";
        assert_eq!(subtitle_url(input).unwrap().as_str(), input);
        assert_eq!(percent_decode("a+b").as_deref(), Some("a+b"));
    }

    #[test]
    fn subtitle_srt_preserves_cue_timing_and_text() {
        let input = br#"{"body":[{"from":0.0,"to":4.0,"content":"CORE_DIALOGUE_PRIMARY"},{"from":4.0,"to":8.0,"content":"CORE_DIALOGUE_SECOND_CUE"}]}"#;
        let body: SubtitleBody = serde_json::from_slice(input).unwrap();
        let srt = cues_to_srt(&body.body);
        assert_eq!(srt, "1\n00:00:00,000 --> 00:00:04,000\nCORE_DIALOGUE_PRIMARY\n\n2\n00:00:04,000 --> 00:00:08,000\nCORE_DIALOGUE_SECOND_CUE\n\n");
    }

    #[test]
    fn danmaku_ass_preserves_movement_and_jsonl_metadata() {
        let comments = [DanmakuComment {
            id: "synthetic-moving-1".into(),
            time_ms: 250,
            mode: 1,
            font_size: 36,
            color: 0x00ff00,
            text: "CORE_DANMAKU_MOVING".into(),
            user_hash: "synthetic".into(),
            date: 0,
            weight: 0,
            action: String::new(),
            pool: 0,
            attr: 0,
            animation: String::new(),
            colorful: 0,
            oid: 0,
            dm_from: 0,
        }];
        let (ass, omissions) = danmaku_ass_with_omissions(&comments);
        assert_eq!(omissions, 0);
        assert!(ass.contains("\\move("));
        assert!(ass.contains("CORE_DANMAKU_MOVING"));
        let jsonl = danmaku_jsonl(&comments);
        let recovered: Vec<DanmakuComment> = jsonl
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(recovered, comments);
    }
}
