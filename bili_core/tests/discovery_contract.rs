use bili_core::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_content_and_untrusted_origins_fail_before_network() {
    let temporary = tempfile::tempdir().unwrap();
    let mut settings = CoreConfig::new(
        temporary.path().join("absent-state"),
        temporary.path().join("output"),
    );
    settings.state_mode = StateMode::ReadOnly;
    let core = Core::new(settings, tokio::runtime::Handle::current()).unwrap();
    for value in [
        "",
        "BVbad",
        "av0",
        "ss0",
        "ep0",
        "https://evil.invalid/video/BV1n44y1Q7sc",
        "https://www.bilibili.com.evil.invalid/video/BV1n44y1Q7sc",
        "https://user:password@www.bilibili.com/video/BV1n44y1Q7sc",
    ] {
        let error = core.resolve(ContentInput::from(value)).await.unwrap_err();
        assert_eq!(
            error.code,
            ErrorCode::InvalidInput,
            "invalid content must not become an upstream request"
        );
        assert!(!error.to_string().contains("password"));
    }
    assert!(core
        .cover(CoverRequest {
            url: "https://evil.invalid/cover.png".into()
        })
        .await
        .is_err());
    core.shutdown(StopMode::Pause).await.unwrap();
    assert!(!temporary.path().join("absent-state").exists());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_search_pagination_is_not_empty_success_or_network_retry() {
    let temporary = tempfile::tempdir().unwrap();
    let mut settings = CoreConfig::new(
        temporary.path().join("absent-state"),
        temporary.path().join("output"),
    );
    settings.state_mode = StateMode::ReadOnly;
    let core = Core::new(settings, tokio::runtime::Handle::current()).unwrap();
    for query in [
        SearchQuery {
            keyword: " ".into(),
            category: SearchCategory::All,
            page: 1,
            sort: SearchSort::Relevance,
        },
        SearchQuery {
            keyword: "sample".into(),
            category: SearchCategory::Video,
            page: 0,
            sort: SearchSort::Relevance,
        },
    ] {
        assert_eq!(
            core.search(query).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
    core.shutdown(StopMode::Pause).await.unwrap();
}

#[tokio::test]
async fn public_search_rejects_unsupported_sort_before_network() {
    let temporary = tempfile::tempdir().unwrap();
    let mut settings = CoreConfig::new(
        temporary.path().join("absent-state"),
        temporary.path().join("output"),
    );
    settings.state_mode = StateMode::ReadOnly;
    let core = Core::new(settings, tokio::runtime::Handle::current()).unwrap();
    for category in [
        SearchCategory::All,
        SearchCategory::Bangumi,
        SearchCategory::Film,
    ] {
        for &sort in SearchSort::available_for(SearchCategory::Video)
            .iter()
            .skip(1)
        {
            let error = core
                .search(SearchQuery {
                    keyword: "sample".into(),
                    category,
                    page: 1,
                    sort,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput);
            assert!(!error.retryable);
        }
    }
    core.shutdown(StopMode::Pause).await.unwrap();
    assert!(!temporary.path().join("absent-state").exists());
}

#[test]
fn older_search_queries_keep_default_sort_and_unknown_sorts_fail() {
    let query: SearchQuery =
        serde_json::from_str(r#"{"keyword":"sample","category":"All","page":2}"#).unwrap();
    assert_eq!(query.sort, SearchSort::Relevance);
    query.validate().unwrap();
    let mut serialized = serde_json::to_value(query).unwrap();
    serialized["sort"] = serde_json::json!("random");
    assert!(serde_json::from_value::<SearchQuery>(serialized).is_err());
}
