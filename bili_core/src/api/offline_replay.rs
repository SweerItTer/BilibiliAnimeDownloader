//! Local response data for exercising the normal Core parsers without API GETs.
use super::MAX_JSON;
use crate::{CoreError, ErrorCode, Result};
use reqwest::Url;
use serde::{
    de::{DeserializeOwned, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    io::Read,
    path::Path,
    sync::Arc,
    time::Duration,
};

const MAX_RESPONSES: usize = 512;
const MAX_DELAY_MS: u64 = 5_000;

/// A validated, immutable set of local API JSON responses.
///
/// Normal Core parsing, access classification and exact selection still apply.
/// This replaces only API JSON GET responses. Core also restricts its HTTP
/// clients to a closed-loopback proxy; the host must separately isolate durable
/// state and any independently created update/network clients.
#[derive(Clone)]
pub struct OfflineApiReplay {
    responses: Arc<HashMap<RequestKey, Response>>,
}

impl fmt::Debug for OfflineApiReplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OfflineApiReplay { responses: [REDACTED] }")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayFile {
    schema_version: u32,
    responses: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    #[serde(default, deserialize_with = "unique_query")]
    query: BTreeMap<String, String>,
    body: Value,
    #[serde(default)]
    delay_ms: u64,
}

struct Response {
    body: Value,
    delay: Duration,
}

#[derive(Hash, PartialEq, Eq)]
struct RequestKey {
    path: String,
    query: BTreeMap<String, String>,
}

impl OfflineApiReplay {
    /// Load a schema-version-1 response file, bounded to eight MiB and 512
    /// distinct requests. Errors and Debug never expose paths, query or body.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|_| invalid("open offline API replay"))?;
        if !file
            .metadata()
            .map_err(|_| invalid("inspect offline API replay"))?
            .is_file()
        {
            return Err(invalid("offline API replay must be a file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_JSON as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("read offline API replay"))?;
        if bytes.len() > MAX_JSON {
            return Err(invalid("offline API replay exceeds its size limit"));
        }
        Self::decode(&bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let replay: ReplayFile =
            serde_json::from_slice(bytes).map_err(|_| invalid("decode offline API replay"))?;
        if replay.schema_version != 1
            || replay.responses.is_empty()
            || replay.responses.len() > MAX_RESPONSES
        {
            return Err(invalid(
                "invalid offline API replay version or response count",
            ));
        }
        let mut responses = HashMap::with_capacity(replay.responses.len());
        for entry in replay.responses {
            if entry.delay_ms > MAX_DELAY_MS
                || !entry.path.starts_with('/')
                || entry.path.starts_with("//")
                || entry.path.contains('?')
                || entry.path.contains('#')
            {
                return Err(invalid("invalid offline API replay response"));
            }
            let mut url = Url::parse("https://api.bilibili.com/")
                .map_err(|_| invalid("invalid offline API replay origin"))?;
            url.set_path(&entry.path);
            if url.path() != entry.path {
                return Err(invalid("offline API replay path must be canonical"));
            }
            url.query_pairs_mut().extend_pairs(&entry.query);
            let key = request_key(&url)?;
            let response = Response {
                body: entry.body,
                delay: Duration::from_millis(entry.delay_ms),
            };
            if responses.insert(key, response).is_some() {
                return Err(invalid("duplicate offline API replay request"));
            }
        }
        Ok(Self {
            responses: Arc::new(responses),
        })
    }

    pub(super) async fn response<T: DeserializeOwned>(&self, url: &Url) -> Result<T> {
        let key = request_key(url)?;
        let response = self
            .responses
            .get(&key)
            .ok_or_else(|| invalid("offline API replay response missing"))?;
        if !response.delay.is_zero() {
            tokio::time::sleep(response.delay).await;
        }
        serde_json::from_value(response.body.clone())
            .map_err(|_| invalid("decode offline API replay response"))
    }
}

fn request_key(url: &Url) -> Result<RequestKey> {
    if url.scheme() != "https"
        || url.host_str() != Some("api.bilibili.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("offline API replay origin rejected"));
    }
    let mut seen = HashSet::new();
    let mut query = BTreeMap::new();
    for (name, value) in url.query_pairs() {
        if !seen.insert(name.to_string()) {
            return Err(invalid("duplicate offline API request parameter"));
        }
        if matches!(
            url.path(),
            "/x/player/wbi/playurl" | "/x/web-interface/wbi/search/type"
        ) && matches!(name.as_ref(), "wts" | "w_rid")
        {
            continue;
        }
        query.insert(name.into_owned(), value.into_owned());
    }
    Ok(RequestKey {
        path: url.path().to_owned(),
        query,
    })
}

fn unique_query<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct UniqueQuery;
    impl<'de> Visitor<'de> for UniqueQuery {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a string map with distinct query names")
        }

        fn visit_map<M: MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut query = BTreeMap::new();
            while let Some((name, value)) = map.next_entry::<String, String>()? {
                if query.insert(name, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate offline API query name"));
                }
            }
            Ok(query)
        }
    }
    deserializer.deserialize_map(UniqueQuery)
}

fn invalid(operation: &'static str) -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(path: &str, query: Value, body: Value, delay_ms: u64) -> OfflineApiReplay {
        OfflineApiReplay::decode(
            &serde_json::to_vec(&json!({
                "schema_version": 1,
                "responses": [{"path":path,"query":query,"body":body,"delay_ms":delay_ms}]
            }))
            .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn exact_request_matching_ignores_only_wbi_volatile_fields() {
        let replay = fixture(
            "/x/player/wbi/playurl",
            json!({"avid":"9401","cid":"9501","qn":"80"}),
            json!({"ok":true}),
            0,
        );
        let matching = Url::parse("https://api.bilibili.com/x/player/wbi/playurl?qn=80&wts=123&cid=9501&w_rid=local&avid=9401").unwrap();
        assert_eq!(
            replay.response::<Value>(&matching).await.unwrap(),
            json!({"ok":true})
        );
        for uri in [
            "https://api.bilibili.com/x/player/wbi/playurl?avid=9401&cid=9502&qn=80",
            "https://api.bilibili.com/x/player/wbi/playurl?avid=9401&cid=9501&qn=64",
            "https://api.bilibili.com/x/player/wbi/playurl?avid=9401&cid=9501&qn=80&fourk=1",
            "https://api.bilibili.com/x/player/wbi/playurl?avid=9401&cid=9501&qn=80&qn=80",
            "https://other.invalid/x/player/wbi/playurl?avid=9401&cid=9501&qn=80",
        ] {
            assert!(replay
                .response::<Value>(&Url::parse(uri).unwrap())
                .await
                .is_err());
        }
        let non_wbi = fixture("/x/web-interface/nav", json!({}), json!({}), 0);
        assert!(non_wbi
            .response::<Value>(
                &Url::parse("https://api.bilibili.com/x/web-interface/nav?wts=123").unwrap()
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn signed_search_keeps_keyword_category_sort_and_page_in_the_replay_key() {
        let replay = fixture(
            "/x/web-interface/wbi/search/type",
            json!({"keyword":"你好","search_type":"video","page":"1"}),
            json!({"ok":true}),
            0,
        );
        let mut url =
            Url::parse("https://api.bilibili.com/x/web-interface/wbi/search/type").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("keyword", "你好"),
            ("search_type", "video"),
            ("page", "1"),
            ("wts", "123"),
            ("w_rid", "local"),
        ]);
        assert_eq!(
            replay.response::<Value>(&url).await.unwrap(),
            json!({"ok":true})
        );
        for (field, value) in [
            ("keyword", "其他"),
            ("search_type", "media_ft"),
            ("page", "2"),
            ("order", "click"),
        ] {
            let mut changed = url.clone();
            let mut pairs: Vec<(String, String)> = changed
                .query_pairs()
                .into_owned()
                .filter(|(key, _)| key != field)
                .collect();
            pairs.push((field.into(), value.into()));
            changed.set_query(None);
            changed.query_pairs_mut().extend_pairs(pairs);
            assert!(replay.response::<Value>(&changed).await.is_err());
        }
    }

    #[test]
    fn malformed_ambiguous_and_unbounded_files_fail_without_exposing_content() {
        for data in [
            r#"{"schema_version":2,"responses":[]}"#,
            r#"{"schema_version":1,"responses":[]}"#,
            r#"{"schema_version":1,"responses":[{"path":"/x","query":{"qn":"80","qn":"64"},"body":{"canary":"not-for-errors"}}]}"#,
            r#"{"schema_version":1,"responses":[{"path":"/x","query":{},"body":{}},{"path":"/x","query":{},"body":{}}]}"#,
            r#"{"schema_version":1,"responses":[{"path":"/x","query":{},"body":{},"delay_ms":5001}]}"#,
        ] {
            let error = OfflineApiReplay::decode(data.as_bytes()).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput);
            assert!(!format!("{error:?}").contains("not-for-errors"));
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("oversized.json");
        std::fs::write(&path, vec![b' '; MAX_JSON + 1]).unwrap();
        assert!(OfflineApiReplay::load(&path).is_err());
        let replay = fixture(
            "/x",
            json!({"canary":"not-for-debug"}),
            json!({"canary":"not-for-debug"}),
            0,
        );
        assert!(!format!("{replay:?}").contains("not-for-debug"));
    }

    #[tokio::test]
    async fn delayed_response_is_cancellable() {
        let replay = fixture("/x", json!({}), json!({"ok":true}), 5_000);
        let url = Url::parse("https://api.bilibili.com/x").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), replay.response::<Value>(&url))
                .await
                .is_err()
        );
    }
}
