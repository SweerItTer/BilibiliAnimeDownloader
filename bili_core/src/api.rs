//! Credential-safe Bilibili discovery and short-lived stream resolution.
use crate::{cookie::CredentialJar, domain::*};
use reqwest::{header, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{
    sync::{Mutex, Semaphore},
    time::Instant,
};

const API: &str = "https://api.bilibili.com";
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";
const REFERER: &str = "https://www.bilibili.com/";
const MAX_JSON: usize = 8 * 1024 * 1024;
const MAX_COVER: usize = 8 * 1024 * 1024;
const WBI_KEY_TTL: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

pub(crate) struct ResolvedLocator {
    pub url: Url,
    pub referer: String,
    pub identity: String,
    pub expected_size: Option<u64>,
    pub duration_ms: Option<u64>,
    pub allowed_origins: Vec<String>,
}
impl ResolvedLocator {
    pub(crate) fn allows(&self, url: &Url) -> bool {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url
                .host_str()
                .is_some_and(|host| self.allowed_origins.iter().any(|origin| origin == host))
    }
}
impl fmt::Debug for ResolvedLocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("ResolvedLocator");
        debug
            .field("url", &"[REDACTED]")
            .field("referer", &"[REDACTED]")
            .field("identity", &self.identity)
            .field("expected_size", &self.expected_size)
            .field("allowed_origins", &"[REDACTED]");
        debug.finish()
    }
}

pub(crate) struct Api {
    client: reqwest::Client,
    credentials: Arc<RwLock<CredentialJar>>,
    connections: Arc<Semaphore>,
    wbi_key: Mutex<Option<(Instant, String)>>,
}

impl Api {
    pub(crate) fn new(
        client: reqwest::Client,
        credentials: Arc<RwLock<CredentialJar>>,
        connections: Arc<Semaphore>,
    ) -> Self {
        Self {
            client,
            credentials,
            connections,
            wbi_key: Mutex::new(None),
        }
    }

    pub(crate) async fn resolve(&self, input: ContentInput) -> Result<MediaItem> {
        let source_ordinal = parse_part_ordinal(&input.0)?;
        let expanded = self.resolve_short_link(&input.0).await?;
        let expanded_ordinal = parse_part_ordinal(&expanded)?;
        if source_ordinal.is_some()
            && expanded_ordinal.is_some()
            && source_ordinal != expanded_ordinal
        {
            return Err(invalid("conflicting video part selectors"));
        }
        let ordinal = source_ordinal.or(expanded_ordinal);
        match parse_content_input(&expanded)? {
            ParsedContentInput::Bvid(bvid) => {
                let item = self.resolve_video_by_bvid(bvid).await?;
                select_video_part(item, ordinal)
            }
            ParsedContentInput::Reference(ContentRef::Video(aid)) => {
                let item = self.resolve_video(aid, None).await?;
                select_video_part(item, ordinal)
            }
            ParsedContentInput::Reference(reference) => {
                if ordinal.is_some() {
                    return Err(invalid("part selector requires an ordinary video URL"));
                }
                match reference {
                    ContentRef::Bangumi(season) => self.resolve_season(season, None).await,
                    ContentRef::Episode(ep) => self.resolve_episode(ep).await,
                    ContentRef::Video(aid) => self.resolve_video(aid, None).await,
                }
            }
        }
    }

    async fn resolve_video_by_bvid(&self, bvid: crate::domain::Bvid) -> Result<MediaItem> {
        let url = api_url(
            "/x/web-interface/view",
            &[("bvid", bvid.as_str().to_owned())],
        )?;
        let value: Value = self.get_json(url, REFERER).await?;
        let data = api_result(&value, "resolve video")?;
        let aid = positive(data.get("aid").and_then(Value::as_u64))
            .map(Aid::new)
            .transpose()?
            .ok_or_else(|| invalid("video identifier missing"))?;
        parse_video_item(aid, Some(bvid), &value)
    }

    async fn resolve_short_link(&self, input: &str) -> Result<String> {
        let mut url = match Url::parse(input.trim()) {
            Ok(url) if is_short_host(url.host_str().unwrap_or_default()) => url,
            _ => return Ok(input.to_owned()),
        };
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(invalid("invalid Bilibili short URL"));
        }
        for hop in 0..=MAX_REDIRECTS {
            let _permit =
                self.connections.acquire().await.map_err(|_| {
                    CoreError::new(ErrorCode::Shutdown, "core connection pool closed")
                })?;
            let response = self
                .client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| transport("resolve Bilibili short URL"))?;
            if response.status().is_redirection() {
                if hop == MAX_REDIRECTS {
                    return Err(invalid("too many Bilibili short URL redirects"));
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| invalid("short URL redirect target missing"))?;
                let next = url
                    .join(location)
                    .map_err(|_| invalid("short URL redirect target invalid"))?;
                if next.scheme() != "https"
                    || !next.username().is_empty()
                    || next.password().is_some()
                    || next.port().is_some()
                    || !is_bili_host(next.host_str().unwrap_or_default())
                {
                    return Err(invalid("short URL redirected outside Bilibili"));
                }
                drop(_permit);
                url = next;
                continue;
            }
            if !response.status().is_success()
                || !is_bili_host(response.url().host_str().unwrap_or_default())
            {
                return Err(transport("resolve Bilibili short URL"));
            }
            return Ok(response.url().to_string());
        }
        Err(invalid("too many Bilibili short URL redirects"))
    }

    async fn resolve_episode(&self, ep: EpisodeId) -> Result<MediaItem> {
        let value: Value = self
            .get_json(
                api_url("/pgc/view/web/season", &[("ep_id", ep.get().to_string())])?,
                REFERER,
            )
            .await?;
        parse_episode_item(ep, &value)
    }

    async fn is_preview_episode(&self, part: &PartRef) -> Result<bool> {
        let Some(episode) = part.episode else {
            return Ok(false);
        };
        if part.content != ContentRef::Episode(episode) {
            return Err(invalid("episode reference does not match its part"));
        }
        let value: Value = self
            .get_json(
                api_url(
                    "/pgc/view/web/season",
                    &[("ep_id", episode.get().to_string())],
                )?,
                REFERER,
            )
            .await?;
        let item = parse_episode_item(episode, &value)?;
        Ok(item.parts.iter().any(|part| part.preview))
    }

    async fn resolve_season(
        &self,
        season: SeasonId,
        selected: Option<EpisodeId>,
    ) -> Result<MediaItem> {
        let value: Value = self
            .get_json(
                api_url(
                    "/pgc/view/web/season",
                    &[("season_id", season.get().to_string())],
                )?,
                REFERER,
            )
            .await?;
        parse_season_item(season, selected, &value)
    }

    async fn resolve_video(
        &self,
        aid: Aid,
        bvid: Option<crate::domain::Bvid>,
    ) -> Result<MediaItem> {
        let value: Value = self
            .get_json(
                api_url("/x/web-interface/view", &[("aid", aid.get().to_string())])?,
                REFERER,
            )
            .await?;
        parse_video_item(aid, bvid, &value)
    }

    pub(crate) async fn search(&self, query: SearchQuery) -> Result<CandidatePage> {
        if query.keyword.trim().is_empty()
            || query.keyword.len() > 256
            || query.page == 0
            || query.page > 1000
        {
            return Err(invalid("invalid search query"));
        }
        let categories: &[(&str, SearchCategory)] = match query.category {
            SearchCategory::All => &[
                ("video", SearchCategory::Video),
                ("media_bangumi", SearchCategory::Bangumi),
                ("media_ft", SearchCategory::Film),
            ],
            SearchCategory::Video => &[("video", SearchCategory::Video)],
            SearchCategory::Bangumi => &[("media_bangumi", SearchCategory::Bangumi)],
            SearchCategory::Film => &[("media_ft", SearchCategory::Film)],
        };
        let futures = categories.iter().map(|(kind, category)| {
            let kind = *kind;
            let category = *category;
            let keyword = query.keyword.as_str();
            let page = query.page;
            async move {
                let url = api_url(
                    "/x/web-interface/search/type",
                    &[
                        ("search_type", kind.to_owned()),
                        ("keyword", keyword.to_owned()),
                        ("page", page.to_string()),
                    ],
                )?;
                let value: Value = self
                    .get_json(self.sign_wbi(url, &[]).await?, REFERER)
                    .await?;
                parse_search_page(category, page, &value)
            }
        });
        let mut pages = futures_util::future::join_all(futures)
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let mut seen = HashSet::new();
        for page in &mut pages {
            page.items
                .retain(|item| seen.insert(item.reference.clone()));
        }
        Ok(CandidatePage { pages })
    }

    pub(crate) async fn cover(&self, request: CoverRequest) -> Result<CoverBytes> {
        let url = Url::parse(&request.url).map_err(|_| invalid("invalid cover URL"))?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || !is_cover_host(url.host_str().unwrap_or_default())
        {
            return Err(invalid("cover host is not allowed"));
        }
        let _permit = self
            .connections
            .acquire()
            .await
            .map_err(|_| CoreError::new(ErrorCode::Shutdown, "core connection pool closed"))?;
        let mut response = self
            .client
            .get(url)
            .header(header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|_| transport("fetch cover"))?;
        if !response.status().is_success() {
            return Err(transport("fetch cover"));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_COVER as u64)
        {
            return Err(invalid("cover exceeds size limit"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| transport("read cover"))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_COVER {
                return Err(invalid("cover exceeds size limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let mime = validate_image(&bytes)?.to_owned();
        Ok(CoverBytes { bytes, mime })
    }

    pub(crate) async fn quality_catalog(&self, part: PartRef) -> Result<QualityCatalog> {
        let trailer = self.is_preview_episode(&part).await?;
        let raw = self.fetch_playurl(&part, 127).await?;
        let mut catalog = parse_catalog(part.clone(), &raw)?;
        allow_explicit_trailer(&mut catalog, trailer);
        let missing: Vec<u32> = if catalog.preview {
            Vec::new()
        } else {
            catalog
                .advertised
                .iter()
                .filter(|entry| {
                    !catalog
                        .representations
                        .iter()
                        .any(|r| !r.audio && r.quality == entry.id)
                })
                .map(|entry| entry.id)
                .take(16)
                .collect()
        };
        for quality in missing {
            match self.fetch_playurl(&part, quality).await {
                Ok(raw) => {
                    let mut variant = parse_catalog(part.clone(), &raw)?;
                    allow_explicit_trailer(&mut variant, trailer);
                    for offered in &variant.advertised {
                        if let Some(entry) = catalog
                            .advertised
                            .iter_mut()
                            .find(|entry| entry.id == offered.id)
                        {
                            entry.requires_login = entry.requires_login.or(offered.requires_login);
                            entry.requires_vip = entry.requires_vip.or(offered.requires_vip);
                            entry.has_video_url |= offered.has_video_url;
                            if entry.advertised_codecs.is_empty() {
                                entry
                                    .advertised_codecs
                                    .clone_from(&offered.advertised_codecs);
                            }
                        }
                    }
                    if variant.preview {
                        if let Some(entry) = catalog
                            .advertised
                            .iter_mut()
                            .find(|entry| entry.id == quality)
                        {
                            entry.availability = Availability::PreviewOnly;
                        }
                    } else {
                        for representation in variant.representations {
                            if !catalog.representations.iter().any(|r| r == &representation) {
                                catalog.representations.push(representation);
                            }
                        }
                    }
                }
                Err(error) => {
                    let availability = match error.code {
                        ErrorCode::LoginRequired => Some(Availability::LoginRequired),
                        ErrorCode::EntitlementRequired => Some(Availability::EntitlementRequired),
                        ErrorCode::RegionRestricted => Some(Availability::RegionRestricted),
                        ErrorCode::Unavailable => Some(Availability::Unavailable),
                        _ => None,
                    };
                    if let Some(availability) = availability {
                        if let Some(entry) = catalog
                            .advertised
                            .iter_mut()
                            .find(|entry| entry.id == quality)
                        {
                            entry.availability = availability;
                        }
                    } else {
                        return Err(error);
                    }
                }
            }
        }
        if !catalog.preview {
            for entry in &mut catalog.advertised {
                if entry.availability != Availability::PreviewOnly
                    && catalog
                        .representations
                        .iter()
                        .any(|r| !r.audio && r.quality == entry.id)
                {
                    entry.availability = Availability::Obtainable;
                }
            }
        }
        catalog.revision = catalog_revision(&catalog.advertised, &catalog.representations)?;
        Ok(catalog)
    }

    pub(crate) async fn plan(
        &self,
        selection: DownloadSelection,
        default_output: &Path,
    ) -> Result<StreamPlan> {
        let catalog = self.quality_catalog(selection.part.clone()).await?;
        let (video, mut audio) = catalog.select(&selection)?;
        let raw = self.fetch_playurl(&selection.part, video.quality).await?;
        let trailer = self.is_preview_episode(&selection.part).await?;
        let current = parse_catalog(selection.part.clone(), &raw)?;
        if current.preview && !trailer {
            return Err(CoreError::new(
                ErrorCode::Unavailable,
                "only a restricted preview stream is available",
            ));
        }
        let dash_video = dash_video_stream(&selection.part, &video, &raw);
        if dash_video.is_none() {
            if selection.audio_representation.is_some() {
                return Err(CoreError::new(
                    ErrorCode::Unavailable,
                    "requested audio representation unavailable",
                ));
            }
            audio = None;
        }
        let locators = parse_locators(&selection.part, &video, audio.as_ref(), &raw)?;
        let topology = if dash_video.is_some() {
            StreamTopology::Dash
        } else if raw_durl_count(&raw) > 1 {
            StreamTopology::Sequential
        } else {
            StreamTopology::Direct
        };
        let segments = locators
            .iter()
            .enumerate()
            .map(|(i, l)| SegmentIdentity {
                ordinal: i as u32,
                size: l.expected_size,
                duration_ms: l.duration_ms,
            })
            .collect();
        let destination = selection.output.unwrap_or_else(|| {
            let part = &selection.part;
            let part_id = part
                .episode
                .map(EpisodeId::get)
                .unwrap_or_else(|| part.cid.get());
            let suffix = format!("_av{}_cid{}_part{part_id}", part.aid.get(), part.cid.get());
            default_output.join(format!("{}.mp4", safe_filename(&selection.title, &suffix)))
        });
        let mut plan = StreamPlan {
            schema_version: 1,
            fingerprint: String::new(),
            part: selection.part,
            title: selection.title,
            video,
            audio,
            topology,
            segments,
            destination,
            merge: selection.merge,
            sidecars: selection.sidecars,
            keep_temp: selection.keep_temp,
            catalog_revision: catalog.revision,
        };
        plan.fingerprint = plan_fingerprint(&plan)?;
        Ok(plan)
    }

    pub(crate) async fn locators(&self, plan: &StreamPlan) -> Result<Vec<ResolvedLocator>> {
        let raw = self.fetch_playurl(&plan.part, plan.video.quality).await?;
        let trailer = self.is_preview_episode(&plan.part).await?;
        let refreshed = parse_catalog(plan.part.clone(), &raw)?;
        if refreshed.preview && !trailer {
            return Err(CoreError::new(
                ErrorCode::Unavailable,
                "only a restricted preview stream is available",
            ));
        }
        for chosen in std::iter::once(&plan.video).chain(plan.audio.iter()) {
            if !refreshed.representations.iter().any(|r| r == chosen) {
                return Err(CoreError::new(
                    ErrorCode::RepresentationChanged,
                    "selected representation changed",
                ));
            }
        }
        let locators = parse_locators(&plan.part, &plan.video, plan.audio.as_ref(), &raw)?;
        Ok(locators)
    }

    async fn fetch_playurl(&self, part: &PartRef, qn: u32) -> Result<Value> {
        let value: Value = if let Some(ep) = part.episode {
            let url = api_url(
                "/pgc/player/web/v2/playurl",
                &[
                    ("ep_id", ep.get().to_string()),
                    ("qn", qn.to_string()),
                    ("fnval", "12240".into()),
                    ("fourk", "1".into()),
                ],
            )?;
            self.get_json(url, REFERER).await?
        } else {
            let params = [
                ("avid", part.aid.get().to_string()),
                ("cid", part.cid.get().to_string()),
                ("qn", qn.to_string()),
                ("fnver", "0".into()),
                ("fnval", "4048".into()),
                ("fourk", "1".into()),
            ];
            let signed = self
                .sign_wbi(api_url("/x/player/wbi/playurl", &params)?, &[])
                .await?;
            self.get_json(signed, REFERER).await?
        };
        api_result(&value, "resolve playurl")?;
        Ok(value)
    }

    async fn wbi_key(&self) -> Result<String> {
        let mut cached = self.wbi_key.lock().await;
        if let Some((expires, key)) = cached.as_ref() {
            if Instant::now() < *expires {
                return Ok(key.clone());
            }
        }
        let nav: Value = self
            .get_json(
                Url::parse(&format!("{API}/x/web-interface/nav"))
                    .map_err(|_| invalid("invalid WBI endpoint"))?,
                REFERER,
            )
            .await?;
        let key = wbi_key_from_nav(&nav)?;
        *cached = Some((Instant::now() + WBI_KEY_TTL, key.clone()));
        Ok(key)
    }

    async fn sign_wbi(&self, mut url: Url, extra: &[(String, String)]) -> Result<Url> {
        let key = self.wbi_key().await?;
        let mut params: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        params.extend_from_slice(extra);
        params.retain(|(k, _)| k != "w_rid" && k != "wts");
        params.push((
            "wts".into(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string(),
        ));
        params.sort_by(|a, b| a.0.cmp(&b.0));
        let canonical = canonical_wbi_query(&params);
        let digest = md5::Md5::digest(format!("{canonical}{key}").as_bytes());
        let signature: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        url.set_query(Some(&format!("{canonical}&w_rid={signature}")));
        Ok(url)
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url, referer: &str) -> Result<T> {
        get_json_with(
            &self.client,
            &self.credentials,
            url,
            referer,
            &self.connections,
        )
        .await
    }
}

async fn get_json_with<T: DeserializeOwned>(
    client: &reqwest::Client,
    jar: &Arc<RwLock<CredentialJar>>,
    url: Url,
    referer: &str,
    connections: &Semaphore,
) -> Result<T> {
    if url.scheme() != "https" || !is_api_host(url.host_str().unwrap_or_default()) {
        return Err(invalid("API host is not allowed"));
    }
    let _permit = connections
        .acquire()
        .await
        .map_err(|_| CoreError::new(ErrorCode::Shutdown, "core connection pool closed"))?;
    let cookie = {
        let credentials = jar.read().map_err(|_| {
            CoreError::new(ErrorCode::CredentialStore, "credential lock unavailable")
        })?;
        credentials.header_for(&url)
    };
    let mut req = client
        .get(url)
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::REFERER, referer);
    if let Some(cookie) = cookie {
        req = req.header(header::COOKIE, cookie);
    }
    let mut response = req.send().await.map_err(|error| request_failure(&error))?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(http_response_failure(status));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_JSON as u64)
    {
        return Err(with_http_status(
            invalid("API response exceeds size limit"),
            status,
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| with_http_status(transport("read Bilibili API response"), status))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_JSON {
            return Err(with_http_status(
                invalid("API response exceeds size limit"),
                status,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| with_http_status(invalid("Bilibili API response is malformed"), status))
}

fn parse_search_page(category: SearchCategory, page: u32, value: &Value) -> Result<CategoryPage> {
    let data = api_result(value, "search Bilibili")?;
    let rows = data
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error(value, "search result shape invalid"))?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let reference = if category == SearchCategory::Video {
            row.get("aid")
                .and_then(Value::as_u64)
                .and_then(|n| Aid::new(n).ok())
                .map(ContentRef::Video)
        } else {
            row.get("season_id")
                .and_then(Value::as_u64)
                .or_else(|| row.get("pgc_season_id").and_then(Value::as_u64))
                .and_then(|n| SeasonId::new(n).ok())
                .map(ContentRef::Bangumi)
        };
        let Some(reference) = reference else { continue };
        let title = strip_markup(&text(row.get("title")).unwrap_or_default());
        if title.is_empty() {
            continue;
        }
        items.push(Candidate {
            reference,
            category,
            title,
            cover: safe_cover_url(text(row.get("pic")).or_else(|| text(row.get("cover")))),
            duration_ms: duration_ms(row.get("duration"))
                .or_else(|| parse_duration(row.get("duration"))),
        });
    }
    let total = data
        .get("numResults")
        .and_then(Value::as_u64)
        .or_else(|| data.get("total").and_then(Value::as_u64));
    let has_more = data.get("numPages").and_then(Value::as_u64).map_or_else(
        || {
            let page_size = data
                .get("pagesize")
                .and_then(Value::as_u64)
                .filter(|size| *size > 0)
                .unwrap_or(20);
            total.is_some_and(|n| n > u64::from(page).saturating_mul(page_size))
                || (!items.is_empty() && total.is_none())
        },
        |page_count| u64::from(page) < page_count,
    );
    Ok(CategoryPage {
        category,
        page,
        total,
        has_more,
        items,
    })
}

fn parse_season_item(
    season: SeasonId,
    selected: Option<EpisodeId>,
    value: &Value,
) -> Result<MediaItem> {
    let result = api_result(value, "resolve Bangumi season")?;
    let title = text(result.get("title")).unwrap_or_else(|| format!("Season {}", season.get()));
    let cover = safe_cover_url(text(result.get("cover")));
    let episodes = result
        .get("episodes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Bangumi episode list missing"))?;
    let mut parts = Vec::with_capacity(episodes.len());
    for (index, ep) in episodes.iter().enumerate() {
        let aid = positive(ep.get("aid").and_then(Value::as_u64))
            .map(Aid::new)
            .transpose()?
            .ok_or_else(|| invalid("Bangumi episode aid missing"))?;
        let cid = positive(ep.get("cid").and_then(Value::as_u64))
            .map(Cid::new)
            .transpose()?
            .ok_or_else(|| invalid("Bangumi episode cid missing"))?;
        let epid = positive(ep.get("id").and_then(Value::as_u64))
            .map(EpisodeId::new)
            .transpose()?
            .ok_or_else(|| invalid("Bangumi episode ID missing"))?;
        if selected.is_some_and(|wanted| wanted != epid) {
            continue;
        }
        parts.push(Part {
            reference: PartRef {
                content: ContentRef::Episode(epid),
                aid,
                cid,
                episode: Some(epid),
            },
            ordinal: index as u32 + 1,
            title: text(ep.get("long_title"))
                .or_else(|| text(ep.get("title")))
                .unwrap_or_else(|| format!("Episode {}", index + 1)),
            duration_ms: ep.get("duration").and_then(Value::as_u64),
            preview: truthy(ep.get("is_preview"))
                || text(ep.get("badge"))
                    .or_else(|| text(ep.pointer("/badge_info/text")))
                    .is_some_and(|badge| badge.contains("预告")),
        });
    }
    if parts.is_empty() {
        return Err(invalid("Bangumi has no matching episodes"));
    }
    let reference = selected
        .map(ContentRef::Episode)
        .unwrap_or(ContentRef::Bangumi(season));
    Ok(MediaItem {
        reference,
        bvid: None,
        title,
        cover,
        duration_ms: None,
        parts,
    })
}
fn parse_episode_item(episode: EpisodeId, value: &Value) -> Result<MediaItem> {
    let result = api_result(value, "resolve Bangumi episode")?;
    let season = positive(result.get("season_id").and_then(Value::as_u64))
        .map(SeasonId::new)
        .transpose()?
        .ok_or_else(|| invalid("Bangumi season identifier missing"))?;
    parse_season_item(season, Some(episode), value)
}

fn parse_video_item(
    aid: Aid,
    bvid: Option<crate::domain::Bvid>,
    value: &Value,
) -> Result<MediaItem> {
    let data = api_result(value, "resolve video")?;
    let aid = positive(data.get("aid").and_then(Value::as_u64))
        .map(Aid::new)
        .transpose()?
        .unwrap_or(aid);
    let bvid = text(data.get("bvid"))
        .and_then(|v| crate::domain::Bvid::new(v).ok())
        .or(bvid);
    let title = text(data.get("title")).unwrap_or_else(|| format!("Video {}", aid.get()));
    let cover = safe_cover_url(text(data.get("pic")));
    let pages = data
        .get("pages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("video pages missing"))?;
    let mut parts = Vec::with_capacity(pages.len());
    for (index, page) in pages.iter().enumerate() {
        let cid = positive(page.get("cid").and_then(Value::as_u64))
            .map(Cid::new)
            .transpose()?
            .ok_or_else(|| invalid("video page CID missing"))?;
        parts.push(Part {
            reference: PartRef {
                content: ContentRef::Video(aid),
                aid,
                cid,
                episode: None,
            },
            ordinal: page
                .get("page")
                .and_then(Value::as_u64)
                .unwrap_or(index as u64 + 1) as u32,
            title: text(page.get("part")).unwrap_or_else(|| title.clone()),
            duration_ms: duration_ms(page.get("duration")),
            preview: false,
        });
    }
    if parts.is_empty() {
        return Err(invalid("video has no pages"));
    }
    Ok(MediaItem {
        reference: ContentRef::Video(aid),
        bvid,
        title,
        cover,
        duration_ms: duration_ms(data.get("duration")),
        parts,
    })
}

enum ParsedContentInput {
    Bvid(crate::domain::Bvid),
    Reference(ContentRef),
}

fn parse_content_input(input: &str) -> Result<ParsedContentInput> {
    let value = input.trim();
    if value.is_empty() || value.len() > 2048 {
        return Err(invalid("invalid content input"));
    }
    if value.contains("://") {
        let url = Url::parse(value).map_err(|_| invalid("invalid content URL"))?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || !is_bili_host(url.host_str().unwrap_or_default())
        {
            return Err(invalid("content URL host is not allowed"));
        }
        let path: Vec<_> = url.path_segments().into_iter().flatten().collect();
        if let Some(v) = path.iter().find_map(|s| s.strip_prefix("BV")) {
            return parse_bvid(&format!("BV{v}"));
        }
        if let Some(s) = path.iter().find_map(|s| s.strip_prefix("ss")) {
            return Ok(ParsedContentInput::Reference(ContentRef::Bangumi(
                SeasonId::new(parse_positive(s)?)?,
            )));
        }
        if let Some(e) = path.iter().find_map(|s| s.strip_prefix("ep")) {
            return Ok(ParsedContentInput::Reference(ContentRef::Episode(
                EpisodeId::new(parse_positive(e)?)?,
            )));
        }
        if let Some(v) = query_value(&url, "bvid") {
            return parse_bvid(&v);
        }
        if let Some(a) = query_value(&url, "aid").or_else(|| {
            path.iter()
                .find_map(|s| s.strip_prefix("av").map(str::to_owned))
        }) {
            return Ok(ParsedContentInput::Reference(ContentRef::Video(Aid::new(
                parse_positive(&a)?,
            )?)));
        }
        return Err(invalid("content URL has no supported Bilibili identifier"));
    }
    if value.starts_with("BV") {
        return parse_bvid(value);
    }
    let reference = if let Some(n) = value.strip_prefix("av") {
        ContentRef::Video(Aid::new(parse_positive(n)?)?)
    } else if let Some(n) = value.strip_prefix("ss") {
        ContentRef::Bangumi(SeasonId::new(parse_positive(n)?)?)
    } else if let Some(n) = value.strip_prefix("ep") {
        ContentRef::Episode(EpisodeId::new(parse_positive(n)?)?)
    } else {
        return Err(invalid(
            "expected BV, AV, ss, ep identifier or Bilibili URL",
        ));
    };
    Ok(ParsedContentInput::Reference(reference))
}

fn parse_bvid(value: &str) -> Result<ParsedContentInput> {
    Ok(ParsedContentInput::Bvid(crate::domain::Bvid::new(
        value.to_owned(),
    )?))
}

fn parse_part_ordinal(input: &str) -> Result<Option<u32>> {
    let Ok(url) = Url::parse(input.trim()) else {
        return Ok(None);
    };
    let mut parts = url
        .query_pairs()
        .filter(|(key, _)| key == "p")
        .map(|(_, value)| value.into_owned());
    let Some(value) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() {
        return Err(invalid("duplicate video part selector"));
    }
    let ordinal = u32::try_from(parse_positive(&value)?)
        .map_err(|_| invalid("invalid video part selector"))?;
    Ok(Some(ordinal))
}

fn select_video_part(mut item: MediaItem, ordinal: Option<u32>) -> Result<MediaItem> {
    if let Some(ordinal) = ordinal {
        let part = item
            .parts
            .into_iter()
            .find(|part| part.ordinal == ordinal)
            .ok_or_else(|| invalid("video part does not exist"))?;
        item.duration_ms = part.duration_ms;
        item.parts = vec![part];
    }
    Ok(item)
}

fn parse_catalog(part: PartRef, value: &Value) -> Result<QualityCatalog> {
    api_result(value, "read quality catalog")?;
    let play = play_info(value);
    let video_info = play.get("video_info").unwrap_or(play);
    if truthy(video_info.get("drm")) {
        return Err(CoreError::new(
            ErrorCode::Unavailable,
            "protected streams are unsupported",
        ));
    }
    if is_region_restricted(play) {
        return Err(CoreError::new(
            ErrorCode::RegionRestricted,
            "Bilibili content is region restricted",
        ));
    }
    let mut reps = Vec::new();
    let mut advertised: Vec<QualityEntry> = Vec::new();
    let descriptions = video_info
        .get("support_formats")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for f in descriptions {
        let id = f
            .get("quality")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        if id == 0 {
            continue;
        }
        let label = text(f.get("new_description"))
            .or_else(|| text(f.get("description")))
            .or_else(|| text(f.get("display_desc")))
            .unwrap_or_else(|| format!("Quality {id}"));
        if !advertised.iter().any(|e| e.id == id) {
            advertised.push(QualityEntry {
                id,
                label,
                rank: f.get("quality").and_then(Value::as_u64).unwrap_or(0) as u32,
                availability: Availability::Unknown,
                requires_login: explicit_bool(f.get("need_login")),
                requires_vip: explicit_bool(f.get("need_vip")),
                has_video_url: false,
                advertised_codecs: f
                    .get("codecs")
                    .and_then(Value::as_array)
                    .map(|codecs| {
                        codecs
                            .iter()
                            .filter_map(|codec| text(Some(codec)))
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    if let Some(qualities) = video_info.get("accept_quality").and_then(Value::as_array) {
        for (index, quality) in qualities.iter().enumerate() {
            let Some(id) = quality
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|id| *id > 0)
            else {
                continue;
            };
            if !advertised.iter().any(|entry| entry.id == id) {
                let label = video_info
                    .get("accept_description")
                    .and_then(Value::as_array)
                    .and_then(|descriptions| descriptions.get(index))
                    .and_then(|value| text(Some(value)))
                    .unwrap_or_else(|| format!("Quality {id}"));
                advertised.push(QualityEntry {
                    id,
                    label,
                    rank: id,
                    availability: Availability::Unknown,
                    requires_login: None,
                    requires_vip: None,
                    has_video_url: false,
                    advertised_codecs: Vec::new(),
                });
            }
        }
    }
    let dash = video_info.get("dash");
    if let Some(video) = dash.and_then(|d| d.get("video")).and_then(Value::as_array) {
        for stream in video {
            if let Some(r) = parse_representation(&part, stream, false) {
                reps.push(r);
            }
        }
    }
    if let Some(dash) = dash {
        for stream in dash_audio_streams(dash) {
            if let Some(r) = parse_representation(&part, stream, true) {
                reps.push(r);
            }
        }
    }
    if let Some(durl) = video_info
        .get("durl")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        let quality = video_info
            .get("quality")
            .and_then(Value::as_u64)
            .or_else(|| durl.get("quality").and_then(Value::as_u64))
            .unwrap_or(0);
        if let Some(quality_id) = u32::try_from(quality).ok().filter(|id| *id > 0) {
            if stream_url_present(durl) {
                reps.push(RepresentationKey {
                    part: part.clone(),
                    id: quality_id as u64,
                    quality: quality_id,
                    codec: text(durl.get("codecs"))
                        .or_else(|| text(durl.get("codec")))
                        .unwrap_or_else(|| "unknown".into()),
                    width: None,
                    height: None,
                    fps: None,
                    dynamic_range: None,
                    color: None,
                    bandwidth: durl.get("bandwidth").and_then(Value::as_u64).unwrap_or(0),
                    audio: false,
                    sample_rate: None,
                    channels: None,
                    language: None,
                });
            }
        }
    }
    let mut unique = Vec::with_capacity(reps.len());
    for representation in reps {
        if !unique.contains(&representation) {
            unique.push(representation);
        }
    }
    unique.sort_by_key(|r| (r.audio, r.quality, r.id));
    let reps = unique;
    let preview = is_trial_play(play, video_info);
    let returned = returned_video_qualities(video_info);
    for entry in &mut advertised {
        entry.has_video_url = returned.contains(&entry.id);
        entry.availability = if preview {
            Availability::PreviewOnly
        } else if reps.iter().any(|r| !r.audio && r.quality == entry.id) {
            Availability::Obtainable
        } else {
            Availability::Unknown
        };
    }
    for r in &reps {
        if !r.audio && !advertised.iter().any(|e| e.id == r.quality) {
            advertised.push(QualityEntry {
                id: r.quality,
                label: format!("Quality {}", r.quality),
                rank: r.quality,
                availability: if preview {
                    Availability::PreviewOnly
                } else {
                    Availability::Obtainable
                },
                requires_login: None,
                requires_vip: None,
                has_video_url: returned.contains(&r.quality),
                advertised_codecs: Vec::new(),
            });
        }
    }
    if advertised.is_empty() && reps.is_empty() {
        return Err(CoreError::new(
            ErrorCode::Unavailable,
            "no stream representations returned",
        ));
    }
    let revision = catalog_revision(&advertised, &reps)?;
    Ok(QualityCatalog {
        part,
        advertised,
        representations: reps,
        revision,
        preview,
    })
}

fn is_trial_play(play: &Value, video: &Value) -> bool {
    truthy(play.get("is_preview"))
        || truthy(video.get("is_preview"))
        || ["play_check", "play_detail"]
            .iter()
            .any(|field| play.get(*field).and_then(Value::as_str) == Some("PLAY_PREVIEW"))
        || play.get("play_video_type").and_then(Value::as_str) == Some("preview")
}
fn explicit_bool(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(value)) => Some(*value),
        Some(value) if value.as_u64() == Some(0) => Some(false),
        Some(value) if value.as_u64() == Some(1) => Some(true),
        _ => None,
    }
}

// Only keep quality IDs, never URLs, in the public catalog. Metadata alone
// does not demonstrate that this account was given a usable stream address.
fn returned_video_qualities(info: &Value) -> std::collections::BTreeSet<u32> {
    let mut returned = std::collections::BTreeSet::new();
    if let Some(video) = info
        .get("dash")
        .and_then(|dash| dash.get("video"))
        .and_then(Value::as_array)
    {
        for stream in video {
            if stream_url(stream).ok().flatten().is_some() {
                if let Some(id) = stream
                    .get("id")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                {
                    returned.insert(id);
                }
            }
        }
    }
    if let Some(stream) = info
        .get("durl")
        .and_then(Value::as_array)
        .and_then(|streams| streams.first())
    {
        if stream_url(stream).ok().flatten().is_some() {
            if let Some(id) = info
                .get("quality")
                .or_else(|| stream.get("quality"))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
            {
                returned.insert(id);
            }
        }
    }
    returned
}

fn allow_explicit_trailer(catalog: &mut QualityCatalog, trailer: bool) {
    if !trailer || !catalog.preview {
        return;
    }
    catalog.preview = false;
    for entry in &mut catalog.advertised {
        if entry.availability == Availability::PreviewOnly {
            entry.availability = if catalog
                .representations
                .iter()
                .any(|r| !r.audio && r.quality == entry.id)
            {
                Availability::Obtainable
            } else {
                Availability::Unknown
            };
        }
    }
}
fn parse_representation(part: &PartRef, v: &Value, audio: bool) -> Option<RepresentationKey> {
    let id = v.get("id").and_then(Value::as_u64)?;
    let quality = u32::try_from(id).ok()?;
    let fps = text(v.get("frameRate"))
        .or_else(|| text(v.get("frame_rate")))
        .or_else(|| numeric_text(v.get("fps")))
        .or_else(|| numeric_text(v.get("frameRate")))
        .or_else(|| numeric_text(v.get("frame_rate")))
        .and_then(parse_fps);
    Some(RepresentationKey {
        part: part.clone(),
        id,
        quality,
        codec: text(v.get("codecs"))
            .or_else(|| text(v.get("codecs_name")))
            .unwrap_or_else(|| "unknown".into()),
        width: v
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        height: v
            .get("height")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        fps,
        dynamic_range: text(v.get("dynamic_range"))
            .or_else(|| text(v.get("hdr_type")))
            .or_else(|| text_number(v.get("hdr_type"))),
        color: text(v.get("color")).or_else(|| text(v.get("color_space"))),
        bandwidth: v.get("bandwidth").and_then(Value::as_u64).unwrap_or(0),
        audio,
        sample_rate: text(v.get("audio_sampling_rate"))
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                v.get("audio_sampling_rate")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            }),
        channels: v
            .get("channels")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        language: text(v.get("lang")),
    })
}
fn dash_audio_streams(dash: &Value) -> Vec<&Value> {
    let mut streams = Vec::new();
    if let Some(audio) = dash.get("audio").and_then(Value::as_array) {
        streams.extend(audio);
    }
    for group in ["dolby", "flac", "lossless"] {
        if let Some(audio) = dash.get(group).and_then(|v| v.get("audio")) {
            if let Some(items) = audio.as_array() {
                streams.extend(items)
            } else if audio.is_object() {
                streams.push(audio)
            }
        }
    }
    streams
}

fn dash_video_stream<'a>(
    part: &PartRef,
    video: &RepresentationKey,
    raw: &'a Value,
) -> Option<&'a Value> {
    let play = play_info(raw);
    play.get("video_info")
        .unwrap_or(play)
        .get("dash")?
        .get("video")
        .and_then(Value::as_array)?
        .iter()
        .find(|stream| parse_representation(part, stream, false).as_ref() == Some(video))
}

fn parse_locators(
    part: &PartRef,
    video: &RepresentationKey,
    audio: Option<&RepresentationKey>,
    raw: &Value,
) -> Result<Vec<ResolvedLocator>> {
    let play = play_info(raw);
    let info = play.get("video_info").unwrap_or(play);
    let mut out = Vec::new();
    if let Some(stream) = dash_video_stream(part, video, raw) {
        append_stream_locators(&mut out, stream, part, video)?;
        if let Some(audio) = audio {
            let dash = info.get("dash").ok_or_else(|| {
                CoreError::new(
                    ErrorCode::RepresentationChanged,
                    "selected audio stream changed",
                )
            })?;
            let stream = dash_audio_streams(dash)
                .into_iter()
                .find(|s| parse_representation(part, s, true).as_ref() == Some(audio))
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::RepresentationChanged,
                        "selected audio stream changed",
                    )
                })?;
            append_stream_locators(&mut out, stream, part, audio)?;
        }
    } else if let Some(durls) = info.get("durl").and_then(Value::as_array) {
        if video.id != video.quality as u64 || audio.is_some() {
            return Err(CoreError::new(
                ErrorCode::RepresentationChanged,
                "selected direct stream changed",
            ));
        }
        for (i, stream) in durls.iter().enumerate() {
            let url = stream_url(stream)?.ok_or_else(|| invalid("direct stream URL missing"))?;
            let resolved = locator(
                url,
                part,
                video,
                stream.get("size").and_then(Value::as_u64),
                stream_duration_ms(stream),
                i as u32,
            )?;
            out.push(resolved);
        }
    } else {
        return Err(CoreError::new(
            ErrorCode::RepresentationChanged,
            "selected stream is no longer available",
        ));
    }
    if out.is_empty() {
        return Err(CoreError::new(
            ErrorCode::Unavailable,
            "no downloadable stream locator",
        ));
    }
    Ok(out)
}
fn append_stream_locators(
    out: &mut Vec<ResolvedLocator>,
    stream: &Value,
    part: &PartRef,
    key: &RepresentationKey,
) -> Result<()> {
    let urls = stream.get("segments").and_then(Value::as_array);
    if let Some(urls) = urls {
        for (i, segment) in urls.iter().enumerate() {
            let url = stream_url(segment)?.ok_or_else(|| invalid("DASH segment URL missing"))?;
            let resolved = locator(
                url,
                part,
                key,
                segment.get("size").and_then(Value::as_u64),
                stream_duration_ms(segment),
                i as u32,
            )?;
            out.push(resolved);
        }
    } else {
        let url = stream_url(stream)?.ok_or_else(|| invalid("DASH stream URL missing"))?;
        let resolved = locator(url, part, key, None, stream_duration_ms(stream), 0)?;
        out.push(resolved);
    }
    Ok(())
}

fn stream_duration_ms(v: &Value) -> Option<u64> {
    v.get("length")
        .and_then(Value::as_u64)
        .filter(|duration| *duration > 0)
        .or_else(|| {
            v.get("duration_ms")
                .and_then(Value::as_u64)
                .filter(|duration| *duration > 0)
        })
}
fn locator(
    url: Url,
    part: &PartRef,
    key: &RepresentationKey,
    size: Option<u64>,
    duration_ms: Option<u64>,
    ordinal: u32,
) -> Result<ResolvedLocator> {
    let origin = url
        .host_str()
        .ok_or_else(|| invalid("stream host missing"))?
        .to_owned();
    let identity_bytes = serde_json::to_vec(key)
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "serialize stream identity"))?;
    let identity_hash = Sha256::digest(identity_bytes);
    let identity = format!(
        "{}:{}:{:x}:{}",
        part.aid.get(),
        part.cid.get(),
        identity_hash,
        ordinal
    );
    Ok(ResolvedLocator {
        url,
        referer: REFERER.into(),
        identity,
        duration_ms,
        expected_size: size,
        allowed_origins: vec![origin],
    })
}
fn stream_url(v: &Value) -> Result<Option<Url>> {
    let mut invalid_candidate = None;
    for key in ["baseUrl", "base_url", "url"] {
        if let Some(candidate) = v.get(key).and_then(Value::as_str) {
            match parse_stream_candidate(candidate) {
                Ok(url) => return Ok(Some(url)),
                Err(error) => {
                    invalid_candidate.get_or_insert(error);
                }
            }
        }
    }
    for key in ["backupUrl", "backup_url"] {
        if let Some(candidates) = v.get(key) {
            if let Some(candidates) = candidates.as_array() {
                for candidate in candidates.iter().filter_map(Value::as_str) {
                    match parse_stream_candidate(candidate) {
                        Ok(url) => return Ok(Some(url)),
                        Err(error) => {
                            invalid_candidate.get_or_insert(error);
                        }
                    }
                }
            } else if let Some(candidate) = candidates.as_str() {
                match parse_stream_candidate(candidate) {
                    Ok(url) => return Ok(Some(url)),
                    Err(error) => {
                        invalid_candidate.get_or_insert(error);
                    }
                }
            }
        }
    }
    match invalid_candidate {
        Some(error) => Err(invalid(error)),
        None => Ok(None),
    }
}
fn stream_url_present(v: &Value) -> bool {
    ["baseUrl", "base_url", "url", "backupUrl", "backup_url"]
        .iter()
        .any(|key| match v.get(*key) {
            Some(Value::String(_)) => true,
            Some(Value::Array(values)) => values.iter().any(Value::is_string),
            _ => false,
        })
}
fn parse_stream_candidate(candidate: &str) -> std::result::Result<Url, &'static str> {
    let url = Url::parse(candidate).map_err(|_| "stream URL malformed")?;
    if allowed_stream_url(&url) {
        Ok(url)
    } else {
        Err("stream host is not allowed")
    }
}
fn allowed_stream_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username() == ""
        && url.password().is_none()
        && url.port().is_none()
        && is_cdn_host(url.host_str().unwrap_or_default())
}
fn obtainable(c: &QualityCatalog) -> Vec<&RepresentationKey> {
    if c.preview {
        return Vec::new();
    }
    c.representations
        .iter()
        .filter(|r| {
            c.advertised
                .iter()
                .any(|q| q.id == r.quality && q.availability == Availability::Obtainable)
                || !c.advertised.iter().any(|q| q.id == r.quality)
        })
        .collect()
}
impl QualityCatalog {
    pub fn select(
        &self,
        selection: &DownloadSelection,
    ) -> Result<(RepresentationKey, Option<RepresentationKey>)> {
        if selection.part != self.part {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "selection does not match quality catalog",
            ));
        }
        let video = select_video(
            self,
            selection.quality,
            selection.video_representation,
            selection.codec.as_deref(),
            selection.dynamic_range.as_deref(),
        )?
        .clone();
        let audio = select_audio(self, selection.audio_representation)?;
        Ok((video, audio))
    }
}

fn select_video<'a>(
    catalog: &'a QualityCatalog,
    quality: Option<u32>,
    representation: Option<u64>,
    codec: Option<&str>,
    dynamic_range: Option<&str>,
) -> Result<&'a RepresentationKey> {
    let mut candidates: Vec<&RepresentationKey> = obtainable(catalog)
        .into_iter()
        .filter(|r| !r.audio)
        .collect();
    if let Some(q) = quality {
        candidates.retain(|r| r.quality == q);
    }
    if let Some(id) = representation {
        candidates.retain(|r| r.id == id);
    }
    if let Some(codec) = codec {
        candidates.retain(|r| r.codec.eq_ignore_ascii_case(codec));
    }
    if let Some(range) = dynamic_range {
        candidates.retain(|r| {
            r.dynamic_range
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(range))
        });
    }
    if representation.is_some() && candidates.len() > 1 {
        return Err(CoreError::new(
            ErrorCode::InvalidInput,
            "video representation identifier is ambiguous; select codec and dynamic range",
        ));
    }
    if candidates.len() > 1
        && candidates
            .iter()
            .any(|r| r.width.is_none() || r.height.is_none())
    {
        return Err(CoreError::new(
            ErrorCode::Unavailable,
            "automatic video resolution cannot be established",
        ));
    }
    candidates
        .into_iter()
        .max_by_key(|r| {
            (
                r.width.unwrap_or(0) as u64 * r.height.unwrap_or(0) as u64,
                r.height.unwrap_or(0),
                r.width.unwrap_or(0),
                fps_milli(r),
                r.bandwidth,
            )
        })
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::Unavailable,
                "requested video representation unavailable",
            )
        })
}

fn select_audio(
    catalog: &QualityCatalog,
    representation: Option<u64>,
) -> Result<Option<RepresentationKey>> {
    let mut candidates: Vec<&RepresentationKey> = obtainable(catalog)
        .into_iter()
        .filter(|r| r.audio)
        .collect();
    if let Some(id) = representation {
        candidates.retain(|r| r.id == id);
    }
    if representation.is_some() && candidates.is_empty() {
        return Err(CoreError::new(
            ErrorCode::Unavailable,
            "requested audio representation unavailable",
        ));
    }
    if representation.is_some() && candidates.len() > 1 {
        return Err(CoreError::new(
            ErrorCode::InvalidInput,
            "audio representation identifier is ambiguous",
        ));
    }
    Ok(candidates
        .into_iter()
        .max_by_key(|r| {
            (
                r.bandwidth,
                r.sample_rate.unwrap_or(0),
                r.channels.unwrap_or(0),
            )
        })
        .cloned())
}

pub(crate) fn plan_fingerprint(p: &StreamPlan) -> Result<String> {
    let extension = p
        .destination
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid("output container extension is required"))?;
    let container = extension.to_ascii_lowercase();
    if container != "mp4" {
        return Err(invalid("unsupported output container"));
    }
    let bytes = serde_json::to_vec(&(
        p.schema_version,
        &p.part,
        &p.video,
        &p.audio,
        &p.topology,
        &p.segments,
        container,
    ))
    .map_err(|_| CoreError::new(ErrorCode::Corruption, "serialize stream plan identity"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn catalog_revision(
    advertised: &[QualityEntry],
    representations: &[RepresentationKey],
) -> Result<u64> {
    let bytes = serde_json::to_vec(&(advertised, representations))
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "serialize quality catalog"))?;
    let digest = Sha256::digest(bytes);
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    Ok(u64::from_be_bytes(prefix))
}
fn api_code(value: &Value) -> Option<i64> {
    let mut current = value;
    for _ in 0..4 {
        if let Some(code) = current.get("code").and_then(Value::as_i64) {
            return Some(code);
        }
        let Some(next) = current
            .get("raw")
            .or_else(|| current.get("data"))
            .or_else(|| current.get("result"))
        else {
            break;
        };
        current = next;
    }
    None
}
fn api_result<'a>(v: &'a Value, operation: &'static str) -> Result<&'a Value> {
    let root = v.get("raw").unwrap_or(v);
    let code = api_code(v).unwrap_or(-1);
    if code == 0 {
        let data = root
            .get("data")
            .or_else(|| root.get("result"))
            .ok_or_else(|| response_error(v, operation))?;
        if data.get("v_voucher").is_some() {
            Err(CoreError::new(
                ErrorCode::VerificationRequired,
                "Bilibili verification required",
            ))
        } else {
            Ok(data)
        }
    } else if code == -352 || code == -412 || has_voucher(v) {
        Err(CoreError::new(
            ErrorCode::VerificationRequired,
            "Bilibili verification required",
        ))
    } else if matches!(code, -101 | -111) {
        Err(CoreError::new(
            ErrorCode::LoginRequired,
            "Bilibili login required",
        ))
    } else if matches!(code, -10403 | -403) {
        Err(CoreError::new(
            ErrorCode::EntitlementRequired,
            "Bilibili access entitlement required",
        ))
    } else if code == -404 {
        Err(CoreError::new(ErrorCode::Unavailable, operation))
    } else {
        Err(CoreError::new(ErrorCode::Transport, operation))
    }
}
fn has_voucher(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields.contains_key("v_voucher") || fields.values().any(has_voucher)
        }
        Value::Array(items) => items.iter().any(has_voucher),
        _ => false,
    }
}
fn play_info(raw: &Value) -> &Value {
    let mut current = raw;
    for _ in 0..4 {
        if current.get("video_info").is_some_and(Value::is_object)
            || current.get("dash").is_some_and(Value::is_object)
            || current.get("durl").is_some_and(Value::is_array)
            || current.get("support_formats").is_some_and(Value::is_array)
        {
            return current;
        }
        let next = ["raw", "data", "result"].iter().find_map(|field| {
            current
                .get(*field)
                .filter(|candidate| candidate.is_object())
        });
        let Some(next) = next else { break };
        current = next;
    }
    current
}
fn is_region_restricted(play: &Value) -> bool {
    play.get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|plugins| {
            plugins.iter().any(|plugin| {
                plugin.get("name").and_then(Value::as_str) == Some("AreaLimitPanel")
                    && truthy(plugin.pointer("/config/is_block"))
            })
        })
}
fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(|x| {
        x.as_bool()
            .unwrap_or_else(|| x.as_i64().is_some_and(|n| n != 0))
    })
}
fn raw_durl_count(raw: &Value) -> usize {
    let play = play_info(raw);
    play.get("video_info")
        .unwrap_or(play)
        .get("durl")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}
fn response_error(v: &Value, op: &'static str) -> CoreError {
    if has_voucher(v) {
        CoreError::new(
            ErrorCode::VerificationRequired,
            "Bilibili verification required",
        )
    } else {
        CoreError::new(ErrorCode::Transport, op)
    }
}
fn api_url(path: &str, params: &[(&str, String)]) -> Result<Url> {
    let mut u = Url::parse(&format!("{API}{path}")).map_err(|_| invalid("invalid API endpoint"))?;
    u.query_pairs_mut()
        .extend_pairs(params.iter().map(|(a, b)| (*a, b.as_str())));
    Ok(u)
}
fn is_api_host(h: &str) -> bool {
    matches!(
        h,
        "api.bilibili.com" | "passport.bilibili.com" | "www.bilibili.com"
    )
}
fn is_bili_host(h: &str) -> bool {
    is_api_host(h)
        || h == "bilibili.com"
        || h == "m.bilibili.com"
        || h == "b23.tv"
        || h == "www.bilibili.tv"
}
fn is_short_host(h: &str) -> bool {
    h == "b23.tv"
}
fn is_cover_host(h: &str) -> bool {
    h == "i0.hdslb.com" || h == "i1.hdslb.com" || h == "i2.hdslb.com" || h.ends_with(".hdslb.com")
}
fn is_cdn_host(h: &str) -> bool {
    h.ends_with(".bilivideo.com")
        || h.ends_with(".akamaized.net")
        || h.ends_with(".bilivideo.cn")
        || h == "cn-gotcha01.bilivideo.com"
}
fn safe_cover_url(url: Option<String>) -> Option<String> {
    let source = url.as_deref()?;
    let normalized = if source.starts_with("//") {
        format!("https:{source}")
    } else {
        source.to_owned()
    };
    let mut u = Url::parse(&normalized).ok()?;
    if u.scheme() == "http" {
        u.set_scheme("https").ok()?;
    }
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || !is_cover_host(u.host_str()?)
    {
        return None;
    }
    Some(u.to_string())
}
fn query_value(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}
fn positive(n: Option<u64>) -> Option<u64> {
    n.filter(|n| *n > 0)
}
fn parse_positive(s: &str) -> Result<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("invalid numeric identifier"));
    }
    let n = s
        .parse()
        .map_err(|_| invalid("invalid numeric identifier"))?;
    if n == 0 {
        Err(invalid("invalid numeric identifier"))
    } else {
        Ok(n)
    }
}
fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn text_number(v: Option<&Value>) -> Option<String> {
    v.and_then(|x| {
        x.as_u64()
            .map(|n| n.to_string())
            .or_else(|| x.as_i64().map(|n| n.to_string()))
    })
}
fn duration_ms(v: Option<&Value>) -> Option<u64> {
    v.and_then(Value::as_u64).map(|s| s.saturating_mul(1000))
}
fn wbi_key_from_nav(value: &Value) -> Result<String> {
    let root = value.get("raw").unwrap_or(value);
    let data = root
        .get("data")
        .or_else(|| root.get("result"))
        .ok_or_else(|| CoreError::new(ErrorCode::Transport, "WBI signing key unavailable"))?;
    let code = api_code(value).unwrap_or(-1);
    let logged_out = data
        .get("isLogin")
        .is_some_and(|v| v.as_bool() == Some(false) || v.as_i64() == Some(0));
    if code != 0 && !(code == -101 && logged_out) {
        api_result(value, "load WBI signing key")?;
    }
    let image = data
        .pointer("/wbi_img/img_url")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::new(ErrorCode::Transport, "WBI signing key unavailable"))?;
    let sub = data
        .pointer("/wbi_img/sub_url")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::new(ErrorCode::Transport, "WBI signing key unavailable"))?;
    let image = image
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    let sub = sub
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    if image.len() != 32
        || sub.len() != 32
        || !image
            .bytes()
            .chain(sub.bytes())
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid("WBI signing key malformed"));
    }
    Ok(mixin_key(&format!("{image}{sub}")))
}
fn numeric_text(value: Option<&Value>) -> Option<String> {
    value.and_then(|v| v.as_f64().map(|n| n.to_string()))
}
fn canonical_wbi_query(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", wbi_escape(key), wbi_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}
fn parse_duration(v: Option<&Value>) -> Option<u64> {
    let s = v.and_then(Value::as_str)?;
    let mut fields = s.split(':');
    let first = fields.next()?.parse::<u64>().ok()?;
    let second = fields.next()?.parse::<u64>().ok()?;
    let third = fields.next();
    if fields.next().is_some() {
        return None;
    }
    let seconds = match third {
        Some(value) => first
            .checked_mul(3600)?
            .checked_add(second.checked_mul(60)?)?
            .checked_add(value.parse::<u64>().ok()?)?,
        None => first.checked_mul(60)?.checked_add(second)?,
    };
    seconds.checked_mul(1000)
}
fn parse_fps(s: String) -> Option<Rational> {
    if let Some((a, b)) = s.split_once('/') {
        Some(Rational {
            numerator: a.parse::<u32>().ok()?,
            denominator: b.parse::<u32>().ok()?.max(1),
        })
    } else {
        let (whole, fraction) = s.split_once('.').unwrap_or((&s, ""));
        let denominator = 10u32.checked_pow(fraction.len() as u32)?;
        let numerator = whole
            .parse::<u32>()
            .ok()?
            .checked_mul(denominator)?
            .checked_add(if fraction.is_empty() {
                0
            } else {
                fraction.parse::<u32>().ok()?
            })?;
        Some(Rational {
            numerator,
            denominator: denominator.max(1),
        })
    }
}
fn fps_milli(r: &RepresentationKey) -> u64 {
    r.fps
        .as_ref()
        .map(|f| f.numerator as u64 * 1000 / f.denominator.max(1) as u64)
        .unwrap_or(0)
}
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => (),
        }
    }
    out
}
fn safe_filename(title: &str, suffix: &str) -> String {
    const MAX_COMPONENT_BYTES: usize = 255;
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    let title_budget = MAX_COMPONENT_BYTES.saturating_sub(".mp4".len() + suffix.len());
    let mut stem = String::new();
    for ch in cleaned.chars() {
        if stem.len() + ch.len_utf8() > title_budget {
            break;
        }
        stem.push(ch);
    }
    if stem.is_empty() {
        stem.push_str("video")
    }
    let mut basename = format!("{stem}{suffix}");
    if windows_reserved_basename(&basename) {
        basename.insert(0, '_')
    }
    basename
}
fn windows_reserved_basename(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit())
    })
}
fn wbi_escape(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else if byte == 33 || byte == 39 || byte == 40 || byte == 41 || byte == 42 {
            continue;
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 15) as usize] as char);
        }
    }
    encoded
}
fn mixin_key(key: &str) -> String {
    const ORDER: [usize; 64] = [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19,
        29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
    ];
    let chars: Vec<char> = key.chars().collect();
    ORDER
        .iter()
        .take(32)
        .filter_map(|i| chars.get(*i).copied())
        .collect()
}
fn validate_image(bytes: &[u8]) -> Result<&'static str> {
    use std::io::Cursor;
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid("cover image malformed"))?;
    let mime = match reader.format() {
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::WebP) => "image/webp",
        Some(image::ImageFormat::Gif) => "image/gif",
        _ => return Err(invalid("cover image format is unsupported")),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| invalid("cover image dimensions invalid"))?;
    Ok(mime)
}
fn invalid(label: &'static str) -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, label)
}
fn transport(label: &'static str) -> CoreError {
    CoreError::new(ErrorCode::Transport, label)
}
fn request_failure(error: &reqwest::Error) -> CoreError {
    let operation = if error.is_timeout() {
        "request Bilibili API timed out"
    } else if error.is_connect() {
        "connect to Bilibili API"
    } else {
        "request Bilibili API"
    };
    transport(operation)
}
fn http_response_failure(status: u16) -> CoreError {
    with_http_status(transport("request Bilibili API"), status)
}
fn with_http_status(mut error: CoreError, status: u16) -> CoreError {
    error.http_status = Some(status);
    error
}

#[cfg(test)]
#[path = "api/discovery_contract.rs"]
mod discovery_contract;
#[cfg(test)]
#[path = "api/quality_contract.rs"]
mod quality_contract;
#[cfg(test)]
#[path = "api/transport_diagnostics.rs"]
mod transport_diagnostics;
