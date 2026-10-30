//! Optional GitHub release check. Queries only the public releases API; no
//! account data, download records, or Bilibili requests are involved.
use crate::prefs::UpdateChannel;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

pub const REPO_URL: &str = "https://github.com/SweerItTer/BilibiliAnimeDownloader";
pub const ISSUES_URL: &str = "https://github.com/SweerItTer/BilibiliAnimeDownloader/issues/new";
const API: &str = "https://api.github.com/repos/SweerItTer/BilibiliAnimeDownloader";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Release {
    pub tag: String,
    pub url: String,
}

pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Error(String),
}

impl Status {
    pub fn is_checking(&self) -> bool {
        matches!(self, Status::Checking)
    }
}

pub struct UpdateState {
    status: Status,
    pending: Option<Receiver<Result<Option<Release>, String>>>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            pending: None,
        }
    }
}

impl UpdateState {
    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn start(&mut self, channel: UpdateChannel) {
        if self.status.is_checking() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch(channel));
        });
        self.pending = Some(rx);
        self.status = Status::Checking;
    }

    /// Returns true once a result (success or failure) was applied.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(Some(release))) => {
                self.status = Status::Available(release);
                self.pending = None;
                true
            }
            Ok(Ok(None)) => {
                self.status = Status::UpToDate;
                self.pending = None;
                true
            }
            Ok(Err(error)) => {
                self.status = Status::Error(error);
                self.pending = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.status = Status::Error("更新检查中断，请稍后重试。".into());
                self.pending = None;
                true
            }
        }
    }
}

fn fetch(channel: UpdateChannel) -> Result<Option<Release>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(format!("Bili-GUI/{VERSION}"))
        .build()
        .map_err(|_| "无法创建更新检查请求。".to_owned())?;
    let body: serde_json::Value = match channel {
        UpdateChannel::Stable => client
            .get(format!("{API}/releases/latest"))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|_| "无法获取稳定版更新信息，请检查网络。".to_owned())?,
        UpdateChannel::Beta => {
            let releases: serde_json::Value = client
                .get(format!("{API}/releases?per_page=10"))
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.json())
                .map_err(|_| "无法获取 Beta 更新信息，请检查网络。".to_owned())?;
            releases
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .find(|release| {
                            !release
                                .get("draft")
                                .and_then(|d| d.as_bool())
                                .unwrap_or(false)
                        })
                        .cloned()
                })
                .unwrap_or(serde_json::Value::Null)
        }
    };
    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let url = body
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or(REPO_URL)
        .to_owned();
    if tag.is_empty() {
        return Ok(None);
    }
    if newer_than(&tag, VERSION) {
        Ok(Some(Release { tag, url }))
    } else {
        Ok(None)
    }
}

fn numeric(version: &str) -> Vec<u64> {
    let core = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    core.split('.')
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn newer_than(latest: &str, current: &str) -> bool {
    let latest = numeric(latest);
    let current = numeric(current);
    let len = latest.len().max(current.len());
    for index in 0..len {
        let a = latest.get(index).copied().unwrap_or(0);
        let b = current.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_handles_prefixes_and_beta_suffixes() {
        assert!(newer_than("v2.0.0", "0.1.0"));
        assert!(newer_than("0.1.1", "0.1.0"));
        assert!(newer_than("0.2.0-beta.1", "0.1.9"));
        assert!(!newer_than("v0.1.0", "0.1.0"));
        assert!(!newer_than("0.1.0-beta.2", "0.1.0"));
        assert!(!newer_than("", "0.1.0"));
    }
}
