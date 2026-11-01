//! On-demand CAPTCHA surface; it owns no Bilibili credentials or background loop.
use bili_core::{VerificationChallenge, VerificationProof};

#[cfg(windows)]
pub struct View {
    // Drop the controller before its ephemeral context and directory.
    webview: wry::WebView,
    messages: std::sync::mpsc::Receiver<Result<VerificationProof, String>>,
    deadline: std::time::Instant,
    _context: wry::WebContext,
    _directory: tempfile::TempDir,
}

#[cfg(not(windows))]
pub struct View;

impl View {
    pub fn new(
        frame: &eframe::Frame,
        challenge: &VerificationChallenge,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let nonce = bili_core::JobId::new().to_string();
            Self::build(
                frame,
                html(&challenge.gt, &challenge.challenge, &nonce),
                nonce,
                challenge.expires_in_secs,
                ctx,
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (frame, challenge, ctx);
            Err("此平台尚未接入内嵌验证，请保留失败记录并稍后重试。".to_owned())
        }
    }

    #[cfg(windows)]
    fn build(
        frame: &eframe::Frame,
        html: String,
        nonce: String,
        lifetime: u64,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let directory = tempfile::Builder::new()
            .prefix("bilidl-verification-")
            .tempdir()
            .map_err(|_| "无法创建临时验证目录。".to_owned())?;
        let mut context = wry::WebContext::new(Some(directory.path().to_owned()));
        let (sender, messages) = std::sync::mpsc::sync_channel(1);
        let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_incognito(true)
            .with_devtools(false)
            .with_custom_protocol("bilidlverify".into(), move |_, request| {
                let (status, content) = if request.uri().path() == "/" {
                    (200, html.as_bytes())
                } else {
                    (404, b"Not found".as_slice())
                };
                wry::http::Response::builder()
                    .status(status)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .body(std::borrow::Cow::Owned(content.to_vec()))
                    .expect("static verification response")
            })
            .with_url("http://bilidlverify.localhost/")
            .with_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(1, 1).into(),
            })
            .with_navigation_handler(|url| {
                url == "http://bilidlverify.localhost/" || url == "about:blank"
            })
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_ipc_handler(move |request| {
                if let Some(message) = parse_message(request.body(), &nonce) {
                    if sender.try_send(message).is_ok() {
                        ctx.request_repaint();
                    }
                }
            })
            .build_as_child(frame)
            .map_err(|_| {
                "无法打开验证窗口，请检查 Microsoft Edge WebView2 Runtime 是否可用。".to_owned()
            })?;
        Ok(Self {
            webview,
            messages,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(lifetime),
            _context: context,
            _directory: directory,
        })
    }

    pub fn poll(&mut self) -> Option<Result<VerificationProof, String>> {
        #[cfg(windows)]
        {
            if std::time::Instant::now() >= self.deadline {
                return Some(Err("验证已超时，请重新发起。".to_owned()));
            }
            self.messages.try_recv().ok()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn set_bounds(&self, bounds: egui::Rect, pixels_per_point: f32) -> Result<(), String> {
        #[cfg(windows)]
        {
            let rect = wry::Rect {
                position: wry::dpi::PhysicalPosition::new(
                    (bounds.min.x * pixels_per_point).round() as i32,
                    (bounds.min.y * pixels_per_point).round() as i32,
                )
                .into(),
                size: wry::dpi::PhysicalSize::new(
                    (bounds.width() * pixels_per_point).max(1.0).round() as u32,
                    (bounds.height() * pixels_per_point).max(1.0).round() as u32,
                )
                .into(),
            };
            self.webview
                .set_bounds(rect)
                .map_err(|_| "无法调整验证窗口。".to_owned())
        }
        #[cfg(not(windows))]
        {
            let _ = (bounds, pixels_per_point);
            Ok(())
        }
    }
}

#[cfg(any(windows, test))]
fn html(gt: &str, challenge: &str, nonce: &str) -> String {
    let config = serde_json::json!({"gt":gt,"challenge":challenge,"nonce":nonce})
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    include_str!("../assets/verification.html").replace("__CAPTCHA_CONFIG__", &config)
}

#[cfg(any(windows, test))]
fn parse_message(body: &str, nonce: &str) -> Option<Result<VerificationProof, String>> {
    if body.len() > 16 * 1024 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("nonce")?.as_str()? != nonce {
        return None;
    }
    match value.get("type")?.as_str()? {
        "proof" => {
            let data = value.get("result")?;
            let field = |key| {
                data.get(key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|v| {
                        !v.is_empty()
                            && v.len() <= 4096
                            && v.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"-_.|+/=".contains(&b))
                    })
                    .map(str::to_owned)
            };
            Some(Ok(VerificationProof {
                challenge: field("geetest_challenge")?,
                validate: field("geetest_validate")?,
                seccode: field("geetest_seccode")?,
            }))
        }
        "cancel" => Some(Err("验证已取消，失败记录已保留。".to_owned())),
        "error" => Some(Err("验证码加载未成功，请关闭后重试。".to_owned())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ipc_requires_matching_session_and_a_complete_bounded_proof() {
        let body=serde_json::json!({"type":"proof","nonce":"session-a","result":{"geetest_challenge":"challenge","geetest_validate":"proof","geetest_seccode":"proof|jordan"}}).to_string();
        assert!(parse_message(&body, "session-b").is_none());
        let proof = parse_message(&body, "session-a").unwrap().unwrap();
        assert_eq!(proof.validate, "proof");
        assert!(parse_message(&body.replace("geetest_validate", "absent"), "session-a").is_none());
        assert!(parse_message(&"x".repeat(20 * 1024), "session-a").is_none());
        assert!(parse_message(r#"{"type":"success","nonce":"session-a"}"#, "session-a").is_none());
    }
    #[test]
    fn challenge_values_cannot_close_the_embedded_script() {
        let content = html("</script><script>unsafe()", "challenge", "nonce");
        assert!(!content.contains("</script><script>unsafe()"));
        assert!(content.contains("\\u003c/script\\u003e"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens an isolated native window with a local-only WebView page; no CAPTCHA service"]
    fn native_webview_creates_receives_local_ipc_and_destroys() {
        use std::sync::{Arc, Mutex};
        use winit::platform::windows::EventLoopBuilderExtWindows;
        struct Host {
            view: Option<View>,
            result: Arc<Mutex<Option<Result<(), String>>>>,
            started: std::time::Instant,
            opened: bool,
        }
        impl eframe::App for Host {
            fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
                if !self.opened {
                    self.opened = true;
                    println!("Local WebView native host received its first frame");
                    let local=r#"<!doctype html><script>window.ipc.postMessage(JSON.stringify({type:'proof',nonce:'local-native-test',result:{geetest_challenge:'challenge',geetest_validate:'proof',geetest_seccode:'proof|jordan'}}));</script>"#.to_owned();
                    match View::build(frame, local, "local-native-test".into(), 10, ctx.clone()) {
                        Ok(view) => {
                            println!("Local WebView controller created");
                            self.view = Some(view);
                        }
                        Err(error) => {
                            *self.result.lock().unwrap() = Some(Err(error));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            return;
                        }
                    }
                }
                let received = self.view.as_mut().and_then(View::poll);
                if let Some(result) = received {
                    *self.result.lock().unwrap() = Some(result.and_then(|proof| {
                        if proof.validate == "proof" {
                            Ok(())
                        } else {
                            Err("unexpected local IPC proof".into())
                        }
                    }));
                    drop(self.view.take());
                    println!("Local WebView controller destroyed after IPC");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else if self.started.elapsed() > std::time::Duration::from_secs(15) {
                    drop(self.view.take());
                    *self.result.lock().unwrap() = Some(Err("local WebView IPC timed out".into()));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(25));
            }
        }
        let result = Arc::new(Mutex::new(None));
        let shared = result.clone();
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_visible(true)
                .with_active(false)
                .with_inner_size([400.0, 300.0]),
            event_loop_builder: Some(Box::new(|builder| {
                builder.with_any_thread(true);
            })),
            ..Default::default()
        };
        eframe::run_native(
            "BiliDL local WebView check",
            options,
            Box::new(move |_| {
                Ok(Box::new(Host {
                    view: None,
                    result: shared,
                    started: std::time::Instant::now(),
                    opened: false,
                }))
            }),
        )
        .unwrap();
        result
            .lock()
            .unwrap()
            .take()
            .expect("native check returned a result")
            .unwrap();
    }
}
