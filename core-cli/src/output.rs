use bili_core::{
    Availability, ContentRef, CoreError, ErrorCode, JobSnapshot, MediaItem, Part, QualityCatalog,
    RepresentationKey,
};
use serde::Serialize;
use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

pub fn exit_code(error: &CoreError) -> u8 {
    match error.code {
        ErrorCode::InvalidInput => 2,
        ErrorCode::VerificationRequired
        | ErrorCode::LoginRequired
        | ErrorCode::EntitlementRequired
        | ErrorCode::RegionRestricted
        | ErrorCode::Unavailable
        | ErrorCode::RepresentationChanged => 3,
        ErrorCode::Cancelled => 130,
        _ => 4,
    }
}

pub fn json(kind: &str, data: &impl Serialize) -> bili_core::Result<()> {
    json_to(&mut io::stdout().lock(), kind, data)
}

fn json_to(out: &mut impl Write, kind: &str, data: &impl Serialize) -> bili_core::Result<()> {
    serde_json::to_writer(
        &mut *out,
        &serde_json::json!({
            "schema_version": 1, "type": kind, "data": data,
        }),
    )
    .map_err(|_| CoreError::new(ErrorCode::Filesystem, "write machine-readable output"))?;
    writeln!(out).map_err(CoreError::io)
}

pub fn qualities(
    item: &MediaItem,
    part: &Part,
    catalog: &QualityCatalog,
    machine: bool,
) -> bili_core::Result<()> {
    qualities_to(&mut io::stdout().lock(), item, part, catalog, machine)
}

pub(super) fn qualities_to(
    out: &mut impl Write,
    item: &MediaItem,
    part: &Part,
    catalog: &QualityCatalog,
    machine: bool,
) -> bili_core::Result<()> {
    if machine {
        json_to(out, "qualities", catalog)
    } else {
        catalog_to(out, 1, item, part, catalog).map_err(CoreError::io)
    }
}

pub fn resolution_report(
    report: &crate::resolutions::Report<'_>,
    machine: bool,
) -> bili_core::Result<()> {
    if machine || report.result.is_ok() {
        resolution_report_to(&mut io::stdout().lock(), report, machine)
    } else {
        resolution_report_to(&mut io::stderr().lock(), report, false)
    }
}

pub(super) fn resolution_report_to(
    out: &mut impl Write,
    report: &crate::resolutions::Report<'_>,
    machine: bool,
) -> bili_core::Result<()> {
    let input = serde_json::json!({"index": report.index, "content": report.item.map(|item| &item.reference)});
    match &report.result {
        Ok(catalog) => {
            let item = report
                .item
                .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "missing resolution item"))?;
            let part = report
                .part
                .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "missing resolution part"))?;
            if machine {
                json_to(
                    out,
                    "resolutions",
                    &serde_json::json!({"input": input, "title": item.title, "part": part, "catalog": catalog}),
                )
            } else {
                catalog_to(out, report.index, item, part, catalog).map_err(CoreError::io)
            }
        }
        Err(error) => {
            if machine {
                json_to(
                    out,
                    "error",
                    &serde_json::json!({"input": input, "part": report.part, "error": error, "exit_code": exit_code(error)}),
                )
            } else {
                writeln!(
                    out,
                    "Input {}{}{}: {}",
                    report.index,
                    report
                        .item
                        .map(|item| format!(" ({})", content_id(&item.reference)))
                        .unwrap_or_default(),
                    report
                        .part
                        .map(|part| format!(" / P{}", part.ordinal))
                        .unwrap_or_default(),
                    error
                )
                .map_err(CoreError::io)
            }
        }
    }
}

fn content_id(content: &ContentRef) -> String {
    match content {
        ContentRef::Video(id) => format!("av{}", id.get()),
        ContentRef::Bangumi(id) => format!("ss{}", id.get()),
        ContentRef::Episode(id) => format!("ep{}", id.get()),
    }
}

fn safe_text(text: &str) -> String {
    let mut escaped = String::new();
    for ch in text.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

fn known_text(text: Option<&str>) -> String {
    text.filter(|s| !s.is_empty() && *s != "unknown")
        .map(safe_text)
        .unwrap_or_else(|| "未知".into())
}

fn catalog_to(
    out: &mut impl Write,
    index: usize,
    item: &MediaItem,
    part: &Part,
    catalog: &QualityCatalog,
) -> io::Result<()> {
    writeln!(
        out,
        "Input {index} ({}) / P{} {}",
        content_id(&item.reference),
        part.ordinal,
        safe_text(&item.title)
    )?;
    if part.title != item.title && !part.title.is_empty() {
        writeln!(out, "{}", safe_text(&part.title))?;
    }
    let mut rows = Vec::new();
    for entry in &catalog.advertised {
        let streams: Vec<_> = catalog
            .representations
            .iter()
            .filter(|stream| !stream.audio && stream.quality == entry.id)
            .collect();
        for stream in streams
            .iter()
            .copied()
            .map(Some)
            .chain(streams.is_empty().then_some(None))
        {
            let dimension = |value: Option<u32>| {
                value
                    .filter(|value| *value > 0)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "未知".into())
            };
            let dimensions = stream
                .map(|stream| format!("{}×{}", dimension(stream.width), dimension(stream.height)))
                .unwrap_or_else(|| "未知".into());
            let fps = stream
                .and_then(|stream| stream.fps.as_ref())
                .filter(|fps| fps.numerator > 0 && fps.denominator > 0)
                .map(|fps| {
                    format!(
                        "{:.3}",
                        f64::from(fps.numerator) / f64::from(fps.denominator)
                    )
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
                })
                .unwrap_or_else(|| "未知".into());
            let codec = stream
                .filter(|stream| !stream.codec.is_empty() && stream.codec != "unknown")
                .map(|stream| safe_text(&stream.codec))
                .unwrap_or_else(|| known_text(Some(&entry.advertised_codecs.join(" / "))));
            let requirement = |value| match value {
                Some(true) => "需要",
                Some(false) => "不需要",
                None => "未知",
            };
            let login = entry
                .requires_login
                .or((entry.availability == Availability::LoginRequired).then_some(true));
            // Generic entitlement errors are not specific evidence of VIP.
            let address = match (entry.has_video_url, entry.availability) {
                (true, Availability::PreviewOnly) => "已返回试看地址",
                (true, _) => "已返回地址",
                (false, Availability::LoginRequired) => "未返回地址（需登录）",
                (false, Availability::EntitlementRequired) => "未返回地址（需权益）",
                (false, Availability::RegionRestricted) => "未返回地址（地区限制）",
                (false, Availability::Unavailable) => "未返回地址（不可用）",
                (false, Availability::PreviewOnly) => "未返回地址（仅试看）",
                (false, _) => "未返回地址",
            };
            let cells = vec![
                entry.id.to_string(),
                known_text(Some(&entry.label)),
                dimensions,
                fps,
                codec,
                requirement(login).into(),
                requirement(entry.requires_vip).into(),
                address.into(),
            ];
            let details = stream
                .map(|stream| {
                    let exact_fps = stream
                        .fps
                        .as_ref()
                        .map(|fps| format!("{}/{}", fps.numerator, fps.denominator))
                        .unwrap_or_else(|| "未知".into());
                    format!(
                        "ID {} / FPS {exact_fps} / HDR {} / Color {} / {} bit/s",
                        stream.id,
                        known_text(stream.dynamic_range.as_deref()),
                        known_text(stream.color.as_deref()),
                        stream.bandwidth
                    )
                })
                .unwrap_or_default();
            rows.push((cells, details));
        }
    }
    let details = rows.iter().enumerate().any(|(index, row)| {
        rows[..index]
            .iter()
            .any(|other| row.0 == other.0 && row.1 != other.1)
    });
    let mut headers: Vec<String> = [
        "Quality ID",
        "清晰度描述",
        "尺寸",
        "FPS",
        "Codec",
        "登录要求",
        "大会员要求",
        "本次档位地址",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if details {
        headers.push("流信息".into());
        for row in &mut rows {
            row.0.push(if row.1.is_empty() {
                "未知".into()
            } else {
                row.1.clone()
            });
        }
    }
    let widths: Vec<_> = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| row.0[index].width())
                .max()
                .unwrap_or(0)
                .max(header.width())
        })
        .collect();
    writeln!(out)?;
    table_row(out, &headers, &widths)?;
    writeln!(
        out,
        "{}",
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("-+-")
    )?;
    for row in rows {
        table_row(out, &row.0, &widths)?;
    }
    writeln!(
        out,
        "\n下载：--quality <Quality ID>（返回地址不代表已完成下载验证）"
    )
}

fn table_row(out: &mut impl Write, cells: &[String], widths: &[usize]) -> io::Result<()> {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            write!(out, " | ")?;
        }
        write!(out, "{cell}")?;
        if index + 1 < cells.len() {
            write!(out, "{}", " ".repeat(widths[index] - cell.width()))?;
        }
    }
    writeln!(out)
}

pub fn result(kind: &str, data: &impl Serialize, machine: bool) -> bili_core::Result<()> {
    if machine {
        return json(kind, data);
    }
    // Typed core data contains no credential jar, QR payload or media locators.
    let text = serde_json::to_string_pretty(data)
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "format result"))?;
    let mut out = io::stdout().lock();
    writeln!(out, "{text}").map_err(CoreError::io)
}

pub fn error(error: &CoreError, machine: bool) {
    if machine {
        let _ = json(
            "error",
            &serde_json::json!({"error": error, "exit_code": exit_code(error)}),
        );
    } else {
        eprintln!("{error}");
    }
}

pub fn representation(key: &RepresentationKey) -> String {
    let spatial = match (key.width, key.height) {
        (Some(w), Some(h)) => format!("{w}x{h}"),
        _ => "resolution unknown".into(),
    };
    let fps = key
        .fps
        .as_ref()
        .map(|f| format!("{}/{} fps", f.numerator, f.denominator))
        .unwrap_or_else(|| "FPS unknown".into());
    format!(
        "representation {} / quality {} / {} / {} / {} / {}",
        key.id,
        key.quality,
        spatial,
        fps,
        key.dynamic_range
            .as_deref()
            .unwrap_or("dynamic range unknown"),
        key.codec
    )
}

pub fn progress(job: &JobSnapshot, machine: bool) -> bili_core::Result<()> {
    if machine {
        return json("job", job);
    }
    eprintln!(
        "{} {:?}: {}{} bytes{}",
        job.id,
        job.phase,
        job.downloaded,
        job.total.map(|n| format!("/{n}")).unwrap_or_default(),
        if job.skipped { " (verified skip)" } else { "" }
    );
    Ok(())
}
