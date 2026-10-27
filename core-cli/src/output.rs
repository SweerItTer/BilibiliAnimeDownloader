use bili_core::{CoreError, ErrorCode, JobSnapshot, RepresentationKey};
use serde::Serialize;
use std::io::{self, Write};

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
    let mut out = io::stdout().lock();
    serde_json::to_writer(
        &mut out,
        &serde_json::json!({
            "schema_version": 1, "type": kind, "data": data,
        }),
    )
    .map_err(|_| CoreError::new(ErrorCode::Filesystem, "write machine-readable output"))?;
    writeln!(out).map_err(CoreError::io)
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
