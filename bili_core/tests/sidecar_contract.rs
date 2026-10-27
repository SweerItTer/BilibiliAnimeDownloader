use bili_core::sidecar::{
    danmaku_ass, danmaku_ass_with_omissions, danmaku_jsonl, danmaku_xml, decode_danmaku_segment,
};

// DmSegMobileReply field 1 contains one DmElem. It carries a stable string ID,
// timestamp, scroll mode, and text. The trailing unknown protobuf field verifies
// forward-compatible decoding rather than an ad-hoc parser rejecting extensions.
const SEGMENT: &[u8] = &[
    0x0a, 0x1a, 0x08, 0x07, 0x10, 0xe2, 0x09, 0x18, 0x01, 0x20, 0x19, 0x28, 0xff, 0xff, 0xff, 0xff,
    0x0f, 0x3a, 0x04, b'a', b'<', b'&', b'b', 0x62, 0x03, b'i', b'd', b'7', 0x98, 0x06, 0x01,
];

#[test]
fn decodes_current_pool_and_exports_escaped_independent_formats() {
    let comments = decode_danmaku_segment(SEGMENT).expect("valid protobuf segment");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "id7");
    assert_eq!(comments[0].time_ms, 1250);
    assert_eq!(comments[0].text, "a<&b");
    let jsonl = danmaku_jsonl(&comments);
    assert!(jsonl.ends_with('\n'));
    let record: serde_json::Value = serde_json::from_str(jsonl.trim()).unwrap();
    assert_eq!(record["text"], "a<&b");
    let xml = danmaku_xml(&comments);
    assert!(xml.starts_with("<?xml"));
    assert!(xml.contains("a&lt;&amp;b"));
    let fields = xml
        .split(" p=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .split(',')
        .count();
    assert_eq!(fields, 8);
    let ass = danmaku_ass(&comments);
    assert!(ass.contains("[Events]"));
    assert!(ass.contains("a<&b"));
}

// Consumer semantics for the emitted ASS profile, not encoder byte spelling.
// Mirrors libass ass_get_next_char; the real libass smoke validates rendering.
fn interpreted_dialogue(ass: &str) -> (String, usize) {
    let soft_break_is_newline = ass
        .lines()
        .filter_map(|line| line.strip_prefix("WrapStyle:"))
        .any(|style| style.trim() == "2");
    let line = ass
        .lines()
        .find(|line| line.starts_with("Dialogue:"))
        .unwrap();
    let text = line.splitn(10, ',').nth(9).unwrap();
    let mut chars = text.chars().peekable();
    let mut visible = String::new();
    let mut overrides = 0;
    while let Some(character) = chars.next() {
        match character {
            '{' => {
                overrides += 1;
                assert!(
                    chars.any(|character| character == '}'),
                    "unterminated override"
                );
            }
            '\\' => match chars.peek().copied() {
                Some('N') => {
                    chars.next();
                    visible.push('\n');
                }
                Some('n') => {
                    chars.next();
                    visible.push(if soft_break_is_newline { '\n' } else { ' ' });
                }
                Some('h') => {
                    chars.next();
                    visible.push('\u{a0}');
                }
                Some('{' | '}') => visible.push(chars.next().unwrap()),
                _ => visible.push('\\'),
            },
            '\u{200b}' => {}
            _ => visible.push(character),
        }
    }
    (visible, overrides)
}

#[test]
fn ass_escaping_prevents_comment_text_from_injecting_override_tags() {
    let mut comments = [bili_core::sidecar::DanmakuComment {
        id: "safe".into(),
        time_ms: 0,
        mode: 1,
        font_size: 25,
        color: 0xffffff,
        text: String::new(),
        user_hash: String::new(),
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
    for text in [
        "{\\pos(0,0)}\nsecond\\line",
        r"\N\n\h",
        r"\\\{x\}",
        "plain {braces}",
    ] {
        comments[0].text.clear();
        comments[0].text.push_str(text);
        let ass = danmaku_ass(&comments);
        let (visible, overrides) = interpreted_dialogue(&ass);
        assert_eq!(visible, text, "consumer-visible text changed");
        assert_eq!(overrides, 1, "comment text injected an ASS override");
    }
}

#[test]
fn raw_and_xml_keep_unsupported_modes_while_ass_reports_the_omission() {
    let segment = [
        0x0a, 0x0f, 0x18, 0x07, 0x20, 0x19, 0x3a, 0x05, b'l', b'a', b'r', b'g', b'e', 0x62, 0x02,
        b'i', b'd',
    ];
    let comments = decode_danmaku_segment(&segment).expect("valid protobuf segment");
    assert_eq!(comments[0].mode, 7);
    assert!(danmaku_jsonl(&comments).contains(r#""mode":7"#));
    assert!(danmaku_xml(&comments).contains(",7,25,"));
    let (ass, omitted) = danmaku_ass_with_omissions(&comments);
    assert_eq!(omitted, 1);
    assert!(!ass.contains("Dialogue:"));
}

#[test]
fn malformed_and_empty_segments_are_distinguished() {
    assert!(decode_danmaku_segment(&[0x0a, 0x05, 0x08]).is_err());
    assert!(decode_danmaku_segment(&[]).unwrap().is_empty());
}
