use super::*;
use resolutions::{CatalogSource, Report};
use std::{cell::RefCell, io::Write};

fn part(ordinal: u32) -> Part {
    Part {
        reference: PartRef {
            content: ContentRef::Video(Aid::new(1).unwrap()),
            aid: Aid::new(1).unwrap(),
            cid: Cid::new(ordinal.into()).unwrap(),
            episode: None,
        },
        ordinal,
        title: format!("Part {ordinal}"),
        duration_ms: None,
        preview: false,
    }
}
fn item(ordinals: &[u32]) -> MediaItem {
    MediaItem {
        reference: part(1).reference.content,
        bvid: None,
        title: "标题\n\x1b[31m".into(),
        cover: None,
        duration_ms: None,
        parts: ordinals.iter().map(|n| part(*n)).collect(),
    }
}
fn catalog(part: PartRef) -> QualityCatalog {
    QualityCatalog {
        part: part.clone(),
        advertised: vec![QualityEntry {
            id: 80,
            label: "1080P".into(),
            rank: 80,
            availability: Availability::Obtainable,
            requires_login: None,
            requires_vip: None,
            has_video_url: true,
            advertised_codecs: Vec::new(),
        }],
        representations: vec![RepresentationKey {
            part,
            id: 80,
            quality: 80,
            codec: "avc1".into(),
            width: Some(1920),
            height: Some(1080),
            fps: Some(Rational {
                numerator: 30000,
                denominator: 1001,
            }),
            dynamic_range: None,
            color: None,
            bandwidth: 1000,
            audio: false,
            sample_rate: None,
            channels: None,
            language: None,
        }],
        revision: 1,
        preview: false,
    }
}

#[test]
fn query_arguments_keep_defaults_and_reject_explicit_download_options() {
    let allowed = Cli::try_parse_from([
        "bili",
        "--json",
        "download",
        "av1",
        "av2",
        "--list-resolutions",
        "--parts",
        "1,3-5",
        "--connections",
        "2",
        "--auth",
        "anonymous",
    ])
    .unwrap();
    assert!(!monitors_jobs(&allowed.command));
    let Command::Download(options) = allowed.command else {
        panic!("download expected")
    };
    assert!(options.list_resolutions);
    assert_eq!(options.inputs.len(), 2);
    for (flag, value) in [
        ("--quality", Some("highest")),
        ("--video-repr", Some("80")),
        ("--audio-repr", Some("30280")),
        ("--codec", Some("avc")),
        ("--hdr", Some("sdr")),
        ("--output", Some("unused")),
        ("--subtitles", Some("all")),
        ("--danmaku", Some("current")),
        ("--merge", Some("auto")),
        ("--jobs", Some("1")),
        ("--keep-temp", None),
    ] {
        let mut args = vec!["bili", "download", "av1", "--list-resolutions", flag];
        args.extend(value);
        assert_eq!(
            Cli::try_parse_from(args).err().unwrap().kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "{flag}"
        );
    }
    let ordinary = Cli::try_parse_from(["bili", "download", "av1"]).unwrap();
    assert!(monitors_jobs(&ordinary.command));
    assert_eq!(
        configuration(&ordinary, &Settings::default())
            .unwrap()
            .state_mode,
        StateMode::ReadWrite
    );
    for args in [
        vec![
            "bili",
            "download",
            "av1",
            "--list-resolutions",
            "--auth",
            "anonymous",
        ],
        vec!["bili", "qualities", "av1", "--auth", "anonymous"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        let config = configuration(&cli, &Settings::default()).unwrap();
        assert_eq!(config.state_mode, StateMode::ReadOnly);
        assert!(matches!(
            config.credential_store,
            bili_core::cookie::StorePolicy::MemoryOnly
        ));
        assert!(!monitors_jobs(&cli.command));
    }
}

#[derive(Default)]
struct Source {
    calls: RefCell<Vec<String>>,
}
impl CatalogSource for Source {
    async fn resolve(&self, input: &str) -> Result<MediaItem> {
        self.calls.borrow_mut().push(input.into());
        match input {
            "bad" => Err(invalid("invalid content identifier")),
            "ep" => Ok(item(&[13])),
            "gap" => Ok(item(&[1, 3])),
            _ => Ok(item(&[1, 2, 3])),
        }
    }
    async fn catalog(&self, part: PartRef) -> Result<QualityCatalog> {
        self.calls
            .borrow_mut()
            .push(format!("cid{}", part.cid.get()));
        if part.cid.get() == 2 {
            Err(CoreError::new(
                ErrorCode::LoginRequired,
                "read quality catalog",
            ))
        } else {
            Ok(catalog(part))
        }
    }
}

#[tokio::test]
async fn batch_query_retains_context_continues_after_failures_and_preserves_original_ordinals() {
    let source = Source::default();
    let mut bytes = Vec::new();
    let code = resolutions::query(
        &source,
        &["bad".into(), "video".into(), "ep".into()],
        "all",
        |report| output::resolution_report_to(&mut bytes, &report, true),
    )
    .await
    .unwrap();
    assert_eq!(code, 3);
    assert_eq!(
        *source.calls.borrow(),
        ["bad", "video", "cid1", "cid2", "cid3", "ep", "cid13"]
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows[0]["data"]["input"],
        serde_json::json!({"index":1,"content":null})
    );
    assert_eq!(rows[2]["type"], "error");
    assert_eq!(rows[2]["data"]["part"]["ordinal"], 2);
    assert_eq!(rows[3]["data"]["part"]["ordinal"], 3);
    assert_eq!(rows[4]["data"]["part"]["ordinal"], 13);
    assert_eq!(rows[4]["type"], "resolutions");
    assert_eq!(rows[4]["schema_version"], 1);
    assert_eq!(
        rows[4]["data"]["input"]["content"],
        serde_json::to_value(item(&[]).reference).unwrap()
    );
}

#[tokio::test]
async fn invalid_parts_skip_catalog_and_output_failure_stops_further_queries() {
    let source = Source::default();
    let mut reports = Vec::new();
    let code = resolutions::query(&source, &["gap".into(), "video".into()], "1-3", |report| {
        reports.push(report.result.is_ok());
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(code, 3);
    assert_eq!(reports, [false, true, false, true]);
    assert_eq!(
        *source.calls.borrow(),
        ["gap", "video", "cid1", "cid2", "cid3"]
    );
    let source = Source::default();
    let error = resolutions::query(&source, &["video".into(), "ep".into()], "all", |_| {
        Err(CoreError::new(ErrorCode::Filesystem, "write output"))
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Filesystem);
    assert_eq!(*source.calls.borrow(), ["video", "cid1"]);
    let mut seen = Vec::new();
    assert_eq!(
        resolutions::query(&Source::default(), &["ep".into()], "13", |report| {
            seen.push(report.part.unwrap().ordinal);
            Ok(())
        })
        .await
        .unwrap(),
        0
    );
    assert_eq!(seen, [13]);
}

#[test]
fn readable_catalog_preserves_access_unknowns_and_distinct_video_streams() {
    let item = item(&[1]);
    let part = &item.parts[0];
    let mut catalog = catalog(part.reference.clone());
    for (index, availability) in [
        Availability::LoginRequired,
        Availability::EntitlementRequired,
        Availability::RegionRestricted,
        Availability::PreviewOnly,
        Availability::Unknown,
        Availability::Unavailable,
    ]
    .into_iter()
    .enumerate()
    {
        catalog.advertised.push(QualityEntry {
            id: 81 + index as u32,
            label: "高码率\t4K".into(),
            rank: 0,
            availability,
            requires_login: None,
            requires_vip: None,
            has_video_url: false,
            advertised_codecs: Vec::new(),
        });
    }
    let mut stream = catalog.representations[0].clone();
    stream.fps = Some(Rational {
        numerator: 60,
        denominator: 1,
    });
    stream.dynamic_range = Some("2".into());
    stream.color = Some("BT.2020".into());
    catalog.representations.push(stream.clone());
    stream.color = Some("BT.709".into());
    catalog.representations.push(stream.clone());
    stream.codec = "av01".into();
    catalog.representations.push(stream.clone());
    stream.codec = "unknown".into();
    stream.width = Some(0);
    stream.height = None;
    stream.fps = None;
    catalog.representations.push(stream.clone());
    stream.audio = true;
    stream.codec = "AUDIO_CANARY".into();
    catalog.representations.push(stream);
    let mut bytes = Vec::new();
    output::qualities_to(&mut bytes, &item, part, &catalog, false).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    for expected in [
        "Quality ID",
        "清晰度描述",
        "尺寸",
        "FPS",
        "1920×1080",
        "30000/1001",
        "60/1",
        "avc1",
        "av01",
        "HDR 2",
        "BT.2020",
        "1000 bit/s",
        "需登录",
        "需权益",
        "地区限制",
        "仅试看",
        "未知",
        "不可用",
        "已返回地址",
        "高码率\\t4K",
        "标题\\n\\u{1b}[31m",
        "--quality",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_eq!(
        text.lines()
            .filter(|line| line.split('|').next().unwrap().trim() == "80")
            .count(),
        5
    );
    assert!(!text.contains("AUDIO_CANARY"));
    assert!(!text.contains('\x1b'));
    assert!(!text.contains("3840"));
}

#[test]
fn single_table_retains_description_and_marks_only_missing_fields() {
    use unicode_width::UnicodeWidthStr;
    let item = item(&[1]);
    let part = &item.parts[0];
    let mut catalog = catalog(part.reference.clone());
    catalog.advertised[0].label = "1080P 高清".into();
    catalog.advertised[0].requires_login = Some(true);
    catalog.advertised[0].requires_vip = Some(false);
    catalog.representations[0].fps = None;
    catalog.advertised.push(QualityEntry {
        id: 16,
        label: "360P 流畅".into(),
        rank: 16,
        availability: Availability::Unknown,
        requires_login: None,
        requires_vip: Some(true),
        has_video_url: false,
        advertised_codecs: vec!["av01".into()],
    });
    let mut partial = catalog.representations[0].clone();
    partial.quality = 16;
    partial.width = Some(640);
    partial.height = None;
    partial.codec = "unknown".into();
    catalog.representations.push(partial);
    let mut bytes = Vec::new();
    output::qualities_to(&mut bytes, &item, part, &catalog, false).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.matches("Quality ID").count(), 2);
    assert!(!text.contains("\n可下载\n"));
    assert!(!text.contains("\n未知\n"));
    let lines: Vec<_> = text.lines().filter(|line| line.contains('|')).collect();
    assert_eq!(lines.len(), 3);
    let cells: Vec<_> = lines[1].split('|').map(str::trim).collect();
    assert_eq!(
        cells,
        [
            "80",
            "1080P 高清",
            "1920×1080",
            "未知",
            "avc1",
            "需要",
            "不需要",
            "已返回地址"
        ]
    );
    let cells: Vec<_> = lines[2].split('|').map(str::trim).collect();
    assert_eq!(
        cells,
        [
            "16",
            "360P 流畅",
            "640×未知",
            "未知",
            "av01",
            "未知",
            "需要",
            "未返回地址"
        ]
    );
    let offsets: Vec<_> = lines
        .iter()
        .map(|line| line.split('|').map(|cell| cell.width()).collect::<Vec<_>>())
        .collect();
    assert_eq!(offsets[0][..7], offsets[1][..7]);
    assert_eq!(offsets[0][..7], offsets[2][..7]);
    assert!(!text.contains("流信息"));
}

#[test]
fn qualities_json_is_unchanged_and_report_errors_do_not_echo_raw_inputs() {
    let item = item(&[13]);
    let part = &item.parts[0];
    let catalog = catalog(part.reference.clone());
    let mut bytes = Vec::new();
    output::qualities_to(&mut bytes, &item, part, &catalog, true).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"schema_version":1,"type":"qualities","data":catalog})
    );
    bytes.clear();
    output::resolution_report_to(
        &mut bytes,
        &Report {
            index: 3,
            item: None,
            part: None,
            result: Err(invalid("invalid content identifier")),
        },
        false,
    )
    .unwrap();
    assert!(String::from_utf8(bytes).unwrap().starts_with("Input 3: "));
}

struct BrokenWriter;
impl Write for BrokenWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[test]
fn resolution_output_failures_are_checked() {
    let item = item(&[1]);
    let part = &item.parts[0];
    let report = Report {
        index: 1,
        item: Some(&item),
        part: Some(part),
        result: Ok(catalog(part.reference.clone())),
    };
    for machine in [false, true] {
        assert_eq!(
            output::resolution_report_to(&mut BrokenWriter, &report, machine)
                .unwrap_err()
                .code,
            ErrorCode::Filesystem
        );
        assert_eq!(
            output::qualities_to(
                &mut BrokenWriter,
                &item,
                part,
                report.result.as_ref().unwrap(),
                machine
            )
            .unwrap_err()
            .code,
            ErrorCode::Filesystem
        );
    }
}

#[tokio::test]
async fn readonly_query_core_leaves_state_and_output_untouched_even_with_existing_writer() {
    let root = tempfile::tempdir().unwrap();
    let settings = root.path().join("settings.json");
    std::fs::write(
        &settings,
        serde_json::to_vec(&serde_json::json!({"output_dir": root.path().join("output")})).unwrap(),
    )
    .unwrap();
    let state = root.path().join("state");
    let cli = Cli::try_parse_from([
        std::ffi::OsStr::new("bili"),
        std::ffi::OsStr::new("--config"),
        settings.as_os_str(),
        std::ffi::OsStr::new("--state-dir"),
        state.as_os_str(),
        std::ffi::OsStr::new("download"),
        std::ffi::OsStr::new("av1"),
        std::ffi::OsStr::new("--list-resolutions"),
        std::ffi::OsStr::new("--auth"),
        std::ffi::OsStr::new("anonymous"),
    ])
    .unwrap();
    let config = configuration(&cli, &super::settings(&cli).unwrap()).unwrap();
    let core = Core::new(config.clone(), tokio::runtime::Handle::current()).unwrap();
    assert!(core.jobs().borrow().jobs.is_empty());
    core.shutdown(StopMode::Pause).await.unwrap();
    assert!(!config.state_dir.exists());
    assert!(!config.output_dir.exists());
    let mut writer_config = config.clone();
    writer_config.state_mode = StateMode::ReadWrite;
    let writer = Core::new(writer_config, tokio::runtime::Handle::current()).unwrap();
    let before = std::fs::read(config.state_dir.join("queue.json")).unwrap();
    let reader = Core::new(config.clone(), tokio::runtime::Handle::current()).unwrap();
    assert!(reader.jobs().borrow().jobs.is_empty());
    reader.shutdown(StopMode::Pause).await.unwrap();
    assert_eq!(
        std::fs::read(config.state_dir.join("queue.json")).unwrap(),
        before
    );
    assert!(!config.output_dir.exists());
    writer.shutdown(StopMode::Pause).await.unwrap();
}
