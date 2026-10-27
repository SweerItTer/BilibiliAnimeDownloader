use super::*;
use serde_json::json;

#[test]
fn accepts_only_strict_supported_identifier_forms() {
    assert!(matches!(
        parse_content_input("BV1xx411c7mD").unwrap(),
        ParsedContentInput::Bvid(_)
    ));
    assert!(matches!(
        parse_content_input("av170001").unwrap(),
        ParsedContentInput::Reference(ContentRef::Video(_))
    ));
    assert!(matches!(
        parse_content_input("https://www.bilibili.com/bangumi/play/ss1234").unwrap(),
        ParsedContentInput::Reference(ContentRef::Bangumi(_))
    ));
    assert!(matches!(
        parse_content_input("https://www.bilibili.com/bangumi/play/ep5678").unwrap(),
        ParsedContentInput::Reference(ContentRef::Episode(_))
    ));
    assert!(parse_content_input("av123junk").is_err());
    assert!(parse_content_input("https://evil.example/video/BV1xx411c7mD").is_err());
    assert!(parse_content_input("https://user@www.bilibili.com/video/BV1xx411c7mD").is_err());
    assert!(matches!(
        parse_content_input("https://www.bilibili.com/video/BV1xx411c7mD").unwrap(),
        ParsedContentInput::Bvid(_)
    ));
    assert_eq!(
        parse_part_ordinal("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap(),
        Some(2)
    );
    assert!(parse_part_ordinal("https://www.bilibili.com/video/BV1xx411c7mD?p=2&p=3").is_err());
    assert!(parse_part_ordinal("https://www.bilibili.com/video/BV1xx411c7mD?p=0").is_err());
}

#[test]
fn ordinary_video_fixture_keeps_every_part_and_normalizes_cover() {
    let payload = json!({"code":0,"data":{"aid":9,"title":"Multi-part","pic":"//i0.hdslb.com/poster.jpg","duration":50,"pages":[{"cid":91,"page":1,"part":"Part one","duration":30},{"cid":92,"page":2,"part":"Part two","duration":20}]}});
    let item = parse_video_item(Aid::new(9).unwrap(), None, &payload).unwrap();
    assert_eq!(item.parts.len(), 2);
    assert_eq!(item.parts[1].reference.cid.get(), 92);
    assert_eq!(item.parts[1].duration_ms, Some(20_000));
    assert_eq!(
        item.cover.as_deref(),
        Some("https://i0.hdslb.com/poster.jpg")
    );
    let selected = select_video_part(item.clone(), Some(2)).unwrap();
    assert_eq!(selected.parts.len(), 1);
    assert_eq!(selected.parts[0].reference.cid.get(), 92);
    assert_eq!(selected.duration_ms, Some(20_000));
}
#[test]
fn public_ten_part_video_url_selects_its_second_part() {
    let url = "https://www.bilibili.com/video/BV17x411w7KC?p=2";
    let bvid = match parse_content_input(url).unwrap() {
        ParsedContentInput::Bvid(bvid) => bvid,
        ParsedContentInput::Reference(_) => panic!("video URL should preserve its BV identifier"),
    };
    let payload = json!({"code":0,"data":{"aid":170001,"bvid":"BV17x411w7KC","pages":[
        {"cid":279786,"page":1,"part":"Хоп","duration":199},
        {"cid":275431,"page":2,"part":"Imash li surce","duration":205},
        {"cid":279787,"page":3,"part":"No Kazvam Ti Stiga","duration":308},
        {"cid":280467,"page":4,"part":"Samo za teb","duration":273},
        {"cid":280468,"page":5,"part":"Tochno sega","duration":241},
        {"cid":280469,"page":6,"part":"Kak boli","duration":336},
        {"cid":274491,"page":7,"part":"Obicham Te","duration":250},
        {"cid":267410,"page":8,"part":"Mrazish","duration":201},
        {"cid":267714,"page":9,"part":"Няма накъде","duration":201},
        {"cid":270380,"page":10,"part":"Gadna poroda","duration":198}
    ]}});
    let item = parse_video_item(Aid::new(170001).unwrap(), Some(bvid), &payload).unwrap();
    assert_eq!(item.parts.len(), 10);
    let selected = select_video_part(item, parse_part_ordinal(url).unwrap()).unwrap();
    assert_eq!(selected.bvid.unwrap().as_str(), "BV17x411w7KC");
    assert_eq!(selected.parts[0].ordinal, 2);
    assert_eq!(selected.parts[0].reference.cid.get(), 275431);
    assert_eq!(selected.parts[0].duration_ms, Some(205_000));
    assert_eq!(selected.duration_ms, Some(205_000));
}

#[test]
fn season_fixture_keeps_episode_order_and_episode_filter() {
    let payload = json!({"code":0,"result":{"title":"Season","cover":"https://i1.hdslb.com/cover.jpg","episodes":[{"aid":1,"cid":11,"id":101,"title":"A","long_title":"One","duration":1000},{"aid":2,"cid":22,"id":102,"title":"B","long_title":"Two","duration":2000}]}});
    let season = SeasonId::new(8).unwrap();
    let item = parse_season_item(season, None, &payload).unwrap();
    assert_eq!(
        item.parts
            .iter()
            .map(|part| part.reference.cid.get())
            .collect::<Vec<_>>(),
        vec![11, 22]
    );
    assert_eq!(item.parts[1].duration_ms, Some(2000));
    let episode = parse_season_item(season, Some(EpisodeId::new(102).unwrap()), &payload).unwrap();
    assert_eq!(
        episode.reference,
        ContentRef::Episode(EpisodeId::new(102).unwrap())
    );
    assert_eq!(episode.parts.len(), 1);
}
#[test]
fn observed_pgc_series_and_episode_payloads_resolve_all_parts() {
    let payload = json!({"code":0,"result":{"season_id":336505,"title":"少年骇客 第四季 中文配音","cover":"https://i0.hdslb.com/bfs/bangumi/image/2f316f95787450b989c2d98a45d535c5b873d161.png","episodes":[
        {"id":7066729,"aid":117472439045149u64,"cid":42896918131u64,"title":"1","long_title":"Omnitrix的奥秘（上）","duration":1551000,"is_preview":null,"badge":""},
        {"id":7066730,"aid":117472422266602u64,"cid":42896916817u64,"title":"2","long_title":"Omnitrix的奥秘（中）","duration":1597000,"is_preview":null,"badge":""},
        {"id":7066731,"aid":117472439045132u64,"cid":42896917743u64,"title":"3","long_title":"Omnitrix的奥秘（下）","duration":1107000,"is_preview":null,"badge":"会员"},
        {"id":7066732,"aid":117472422267644u64,"cid":42896917231u64,"title":"4","long_title":"完美的一天","duration":1351000,"is_preview":null,"badge":"会员"},
        {"id":7066733,"aid":117472439174674u64,"cid":42896917652u64,"title":"5","long_title":"迪多好搭档","duration":1371000,"is_preview":null,"badge":"会员"},
        {"id":7066734,"aid":117472422398694u64,"cid":42896916563u64,"title":"6","long_title":"返老还童","duration":1367000,"is_preview":null,"badge":"会员"},
        {"id":7066735,"aid":117472422266615u64,"cid":42896917209u64,"title":"7","long_title":"外星人婚礼","duration":1378000,"is_preview":null,"badge":"会员"},
        {"id":7066736,"aid":117472422267380u64,"cid":42896917083u64,"title":"8","long_title":"爷爷的爱车","duration":1382000,"is_preview":null,"badge":"会员"},
        {"id":7066737,"aid":117472439110677u64,"cid":42923068239u64,"title":"9","long_title":"摔跤冠军","duration":1365000,"is_preview":null,"badge":"会员"},
        {"id":7066738,"aid":117472422267622u64,"cid":42896916732u64,"title":"10","long_title":"虎父无犬子","duration":1384000,"is_preview":null,"badge":"会员"},
        {"id":7066739,"aid":117472439042055u64,"cid":42896917615u64,"title":"11","long_title":"假期结束","duration":1373000,"is_preview":null,"badge":"会员"},
        {"id":7066740,"aid":117472439043592u64,"cid":42896917624u64,"title":"12","long_title":"正邪大战 上集","duration":1374000,"is_preview":null,"badge":"会员"},
        {"id":7066741,"aid":117472439043863u64,"cid":42896917979u64,"title":"13","long_title":"正邪大战 下集","duration":1377000,"is_preview":null,"badge":"会员"}
    ]}});
    let season_id = SeasonId::new(336505).unwrap();
    let season = parse_season_item(season_id, None, &payload).unwrap();
    assert_eq!(season.reference, ContentRef::Bangumi(season_id));
    assert_eq!(season.title, "少年骇客 第四季 中文配音");
    assert_eq!(
        season.cover.as_deref(),
        Some("https://i0.hdslb.com/bfs/bangumi/image/2f316f95787450b989c2d98a45d535c5b873d161.png")
    );
    assert_eq!(season.parts.len(), 13);
    assert_eq!(
        season.parts[0].reference.episode,
        Some(EpisodeId::new(7066729).unwrap())
    );
    assert_eq!(season.parts[0].reference.aid.get(), 117472439045149u64);
    assert_eq!(season.parts[0].reference.cid.get(), 42896918131u64);
    assert_eq!(season.parts[0].title, "Omnitrix的奥秘（上）");
    assert_eq!(season.parts[0].duration_ms, Some(1_551_000));
    assert_eq!(
        season.parts[12].reference.episode,
        Some(EpisodeId::new(7066741).unwrap())
    );
    assert_eq!(season.parts[12].ordinal, 13);

    let episode_id = EpisodeId::new(7066729).unwrap();
    let episode =
        match parse_content_input("https://www.bilibili.com/bangumi/play/ep7066729").unwrap() {
            ParsedContentInput::Reference(ContentRef::Episode(id)) => id,
            _ => panic!("episode URL should route through the PGC episode endpoint"),
        };
    assert_eq!(episode, episode_id);
    let selected = parse_episode_item(episode, &payload).unwrap();
    assert_eq!(selected.reference, ContentRef::Episode(episode_id));
    assert_eq!(selected.parts.len(), 1);
    assert_eq!(selected.parts[0].reference.cid.get(), 42896918131u64);
    assert_eq!(selected.parts[0].duration_ms, Some(1_551_000));
}

#[test]
fn observed_pgc_movie_payload_preserves_both_member_episodes() {
    let payload = json!({"code":0,"result":{"season_id":297253,"title":"空枪","cover":"https://i0.hdslb.com/bfs/bangumi/image/f8590ba633657af9c15e4754b3e60ee43f24860f.png","episodes":[
        {"id":7021237,"aid":117484099208926u64,"cid":42955704367u64,"title":"粤语版","long_title":"","duration":9626000,"is_preview":null,"badge":"会员"},
        {"id":7109601,"aid":117484434751686u64,"cid":42957998819u64,"title":"国语版","long_title":"","duration":9626000,"is_preview":null,"badge":"会员"}
    ]}});
    let season_id = SeasonId::new(297253).unwrap();
    let movie = parse_season_item(season_id, None, &payload).unwrap();
    assert_eq!(movie.reference, ContentRef::Bangumi(season_id));
    assert_eq!(movie.title, "空枪");
    assert_eq!(
        movie.cover.as_deref(),
        Some("https://i0.hdslb.com/bfs/bangumi/image/f8590ba633657af9c15e4754b3e60ee43f24860f.png")
    );
    assert_eq!(movie.parts.len(), 2);
    assert_eq!(
        movie.parts[0].reference.episode,
        Some(EpisodeId::new(7021237).unwrap())
    );
    assert_eq!(movie.parts[0].title, "粤语版");
    assert_eq!(movie.parts[0].duration_ms, Some(9_626_000));
    assert!(!movie.parts[0].preview);
    assert_eq!(
        movie.parts[1].reference.episode,
        Some(EpisodeId::new(7109601).unwrap())
    );
    assert_eq!(movie.parts[1].title, "国语版");
}

#[test]
fn observed_pgc_trial_playurl_marks_every_advertised_quality_preview_only() {
    let part = PartRef {
        content: ContentRef::Episode(EpisodeId::new(7021237).unwrap()),
        aid: Aid::new(117484099208926u64).unwrap(),
        cid: Cid::new(42955704367u64).unwrap(),
        episode: Some(EpisodeId::new(7021237).unwrap()),
    };
    // Signed locator values are intentionally omitted; this fixture exercises access classification only.
    let playurl = json!({"code":0,"result":{"video_info":{"quality":32,"is_preview":1,"accept_quality":[120,112,80,64,32,16],"support_formats":[
        {"quality":120,"new_description":"4K 超高清","display_desc":"4K","description":"超清 4K"},
        {"quality":112,"new_description":"1080P 高码率","display_desc":"1080P","description":"高清 1080P+"},
        {"quality":80,"new_description":"1080P 高清","display_desc":"1080P","description":"高清 1080P"},
        {"quality":64,"new_description":"720P 准高清","display_desc":"720P","description":"高清 720P"},
        {"quality":32,"new_description":"480P 标清","display_desc":"480P","description":"清晰 480P"},
        {"quality":16,"new_description":"360P 流畅","display_desc":"360P","description":"流畅 360P"}
    ],"durl":[{"quality":32}]}}});
    let catalog = parse_catalog(part, &playurl).unwrap();
    assert!(catalog.preview);
    assert!(catalog.representations.is_empty());
    assert_eq!(
        catalog
            .advertised
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![120, 112, 80, 64, 32, 16]
    );
    assert!(catalog
        .advertised
        .iter()
        .all(|entry| entry.availability == Availability::PreviewOnly));
}

#[test]
fn category_search_fixture_rejects_risk_only_payload() {
    let payload = json!({"code":0,"data":{"result":[{"aid":9,"title":"<em>Title</em>","pic":"http://i0.hdslb.com/a.jpg","duration":"1:05"}],"numResults":42}});
    let page = parse_search_page(SearchCategory::Video, 2, &payload).unwrap();
    assert_eq!(page.items[0].title, "Title");
    assert_eq!(page.items[0].duration_ms, Some(65_000));
    assert!(page.has_more);
    assert_eq!(
        page.items[0].cover.as_deref(),
        Some("https://i0.hdslb.com/a.jpg")
    );
    let risk = json!({"code":0,"data":{"v_voucher":"opaque"}});
    assert_eq!(
        parse_search_page(SearchCategory::Video, 1, &risk)
            .unwrap_err()
            .code,
        ErrorCode::VerificationRequired
    );
}

#[test]
fn anonymous_nav_signing_keys_and_wbi_encoding_are_supported() {
    let nav = json!({"code":-101,"data":{"isLogin":false,"wbi_img":{"img_url":"https://i0.hdslb.com/bfs/wbi/0123456789abcdef0123456789abcdef.png","sub_url":"https://i0.hdslb.com/bfs/wbi/fedcba9876543210fedcba9876543210.png"}}});
    assert_eq!(
        api_result(&nav, "load WBI signing key").unwrap_err().code,
        ErrorCode::LoginRequired
    );
    assert_eq!(wbi_key_from_nav(&nav).unwrap().len(), 32);
    let params = vec![("keyword".to_owned(), "A B!'()*東".to_owned())];
    assert_eq!(canonical_wbi_query(&params), "keyword=A%20B%E6%9D%B1");
}

#[test]
fn default_output_names_are_byte_bounded_and_media_distinct() {
    let title = "界😀".repeat(100);
    let first = safe_filename(&title, "_av1_cid2_part2");
    let second = safe_filename(&title, "_av1_cid3_part3");
    assert!(format!("{first}.mp4").len() <= 255);
    assert_ne!(first, second);
    assert!(windows_reserved_basename("CON"));
    assert!(!windows_reserved_basename(&safe_filename(
        "CON",
        "_av1_cid2_part2"
    )));
}
#[test]
fn typed_search_pages_video_bangumi_and_film_with_posters() {
    let video = json!({"code":0,"data":{"page":1,"pagesize":20,"numPages":2,"numResults":25,"result":[
        {"aid":170001,"bvid":"BV17x411w7KC","title":"<em>Video</em>","pic":"//i0.hdslb.com/poster.jpg","duration":"3:19"}
    ]}});
    let first = parse_search_page(SearchCategory::Video, 1, &video).unwrap();
    let mut last_response = video.clone();
    last_response["data"]["page"] = json!(2);
    let last = parse_search_page(SearchCategory::Video, 2, &last_response).unwrap();
    assert_eq!(first.total, Some(25));
    assert!(first.has_more);
    assert!(!last.has_more);
    assert_eq!(
        first.items[0].reference,
        ContentRef::Video(Aid::new(170001).unwrap())
    );
    assert_eq!(first.items[0].title, "Video");
    assert_eq!(
        first.items[0].cover.as_deref(),
        Some("https://i0.hdslb.com/poster.jpg")
    );
    assert_eq!(first.items[0].duration_ms, Some(199_000));

    let bangumi = json!({"code":0,"data":{"page":1,"pagesize":20,"numPages":1,"numResults":1,"result":[
        {"season_id":336505,"pgc_season_id":336505,"title":"少年骇客 第四季 中文配音","cover":"//i0.hdslb.com/bfs/bangumi/image/2f316f95787450b989c2d98a45d535c5b873d161.png","ep_size":13}
    ]}});
    let series = parse_search_page(SearchCategory::Bangumi, 1, &bangumi).unwrap();
    assert_eq!(
        series.items[0].reference,
        ContentRef::Bangumi(SeasonId::new(336505).unwrap())
    );
    assert_eq!(series.items[0].category, SearchCategory::Bangumi);
    assert_eq!(series.items[0].title, "少年骇客 第四季 中文配音");
    assert_eq!(
        series.items[0].cover.as_deref(),
        Some("https://i0.hdslb.com/bfs/bangumi/image/2f316f95787450b989c2d98a45d535c5b873d161.png")
    );
    assert!(!series.has_more);

    let film = json!({"code":0,"data":{"page":1,"pagesize":20,"numPages":1,"numResults":1,"result":[
        {"season_id":null,"pgc_season_id":297253,"title":"空枪","cover":"//i0.hdslb.com/bfs/bangumi/image/f8590ba633657af9c15e4754b3e60ee43f24860f.png","ep_size":2}
    ]}});
    let movies = parse_search_page(SearchCategory::Film, 1, &film).unwrap();
    assert_eq!(
        movies.items[0].reference,
        ContentRef::Bangumi(SeasonId::new(297253).unwrap())
    );
    assert_eq!(movies.items[0].category, SearchCategory::Film);
    assert_eq!(movies.items[0].title, "空枪");
    assert_eq!(
        movies.items[0].cover.as_deref(),
        Some("https://i0.hdslb.com/bfs/bangumi/image/f8590ba633657af9c15e4754b3e60ee43f24860f.png")
    );
}
