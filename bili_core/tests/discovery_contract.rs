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
        },
        SearchQuery {
            keyword: "sample".into(),
            category: SearchCategory::Video,
            page: 0,
        },
    ] {
        assert_eq!(
            core.search(query).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
    core.shutdown(StopMode::Pause).await.unwrap();
}
