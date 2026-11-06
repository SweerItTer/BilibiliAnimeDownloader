//! A composed journal/Core and transport crash boundary, without remote discovery.
use super::*;
use crate::{
    domain::*,
    state::{JobRecord, Journal, ResourceRecord},
    Core,
};
use std::{
    io::{BufRead, Read, Write},
    process::{Child, Command, Stdio},
};
use tokio::{net::TcpListener, sync::oneshot};

const CHILD_TEST: &str = "download::process_recovery::crash_transfer_child";
const READY: &str = "RECOVERY_READY";
const BUDGET: Duration = Duration::from_secs(15);

fn plan(root: &Path) -> StreamPlan {
    let part = PartRef {
        content: ContentRef::Video(Aid::new(1).unwrap()),
        aid: Aid::new(1).unwrap(),
        cid: Cid::new(2).unwrap(),
        episode: None,
    };
    let video = RepresentationKey {
        part: part.clone(),
        id: 16,
        quality: 16,
        codec: "avc1".into(),
        width: None,
        height: None,
        fps: None,
        dynamic_range: None,
        color: None,
        bandwidth: 1,
        audio: false,
        sample_rate: None,
        channels: None,
        language: None,
    };
    let mut plan = StreamPlan {
        schema_version: 1,
        fingerprint: String::new(),
        part,
        title: "Offline recovery fixture".into(),
        video,
        audio: None,
        topology: StreamTopology::Direct,
        segments: Vec::new(),
        destination: root.join("output/final.mp4"),
        merge: MergePolicy::Manual,
        sidecars: SidecarPolicy {
            subtitles: SubtitleSelection::None,
            danmaku: false,
        },
        keep_temp: true,
        catalog_revision: 1,
        context: None,
    };
    plan.fingerprint = crate::api::plan_fingerprint(&plan).unwrap();
    plan
}

fn resource_locator(url: Url) -> ResolvedLocator {
    ResolvedLocator {
        url,
        referer: "https://www.bilibili.com/".into(),
        identity: "offline-recovery-resource".into(),
        expected_size: Some(6),
        duration_ms: None,
        allowed_origins: Vec::new(),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(BUDGET * 2)
        .build()
        .unwrap()
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct OwnedServer(Option<tokio::task::JoinHandle<Vec<String>>>);
impl Drop for OwnedServer {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}
impl OwnedServer {
    async fn finish(&mut self) -> Vec<String> {
        tokio::time::timeout(BUDGET, self.0.as_mut().unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

async fn request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0];
    tokio::time::timeout(BUDGET, async {
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 8192);
            socket.read_exact(&mut byte).await.unwrap();
            bytes.push(byte[0]);
        }
    })
    .await
    .unwrap();
    String::from_utf8(bytes).unwrap()
}

async fn server() -> (Url, OwnedServer, oneshot::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}/media", listener.local_addr().unwrap())).unwrap();
    let (active, ready) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut active = Some(active);
        for stage in 0..3 {
            let (mut socket, _) = tokio::time::timeout(BUDGET, listener.accept())
                .await
                .unwrap()
                .unwrap();
            requests.push(request(&mut socket).await);
            let headers = if stage == 0 {
                "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n"
            } else {
                "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n"
            };
            socket.write_all(headers.as_bytes()).await.unwrap();
            if stage == 0 {
                socket.write_all(b"abc").await.unwrap();
            } else if stage == 1 {
                active.take().unwrap().send(()).unwrap();
            } else {
                socket.write_all(b"def").await.unwrap();
                continue;
            }
            // The first transfer cancels; the second is killed while waiting
            // for body bytes. No source write can race the injected tail.
            let mut byte = [0];
            let closed = tokio::time::timeout(BUDGET, socket.read(&mut byte))
                .await
                .unwrap();
            assert!(
                matches!(closed, Ok(0))
                    || matches!(closed, Err(error) if matches!(
                        error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted,
                    ))
            );
        }
        requests
    });
    (url, OwnedServer(Some(task)), ready)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned child helper for killed_transfer_recovers_checkpoint_and_normalizes_active_job"]
async fn crash_transfer_child() {
    let Some(root) = std::env::var_os("BILI_RECOVERY_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let id: JobId = std::env::var("BILI_RECOVERY_TEST_JOB")
        .unwrap()
        .parse()
        .unwrap();
    let url = Url::parse(&std::env::var("BILI_RECOVERY_TEST_URL").unwrap()).unwrap();
    let journal = Journal::open(&root.join("state"), StateMode::ReadWrite).unwrap();
    let path = journal.owned_file(id, "resource-0.media").unwrap();
    let mut record = JobRecord::planned(id, plan(&root));
    record.snapshot.phase = JobPhase::Downloading;
    record.snapshot.revision = 1;
    record.snapshot.total = Some(6);
    record.resources.push(ResourceRecord {
        name: "resource-0.media".into(),
        identity: "offline-recovery-resource".into(),
    });
    let mut records = vec![record];
    journal.save(1, &mut records, None).unwrap();
    let client = client();
    let locator = resource_locator(url);
    let policy = RetryPolicy {
        attempts: 1,
        base_delay: Duration::ZERO,
    };
    let cancel = CancellationToken::new();
    let mut checkpoint = ResourceCheckpoint::default();
    let result = {
        let first = transfer(
            &client,
            &locator,
            &path,
            &mut checkpoint,
            TransferOptions {
                cancel: &cancel,
                rate_limit: None,
                budget: None,
            },
            &policy,
            |_| {},
        );
        tokio::pin!(first);
        let written = async {
            loop {
                if tokio::fs::metadata(&path)
                    .await
                    .is_ok_and(|metadata| metadata.len() == 3)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            result = &mut first => panic!("prefix transfer ended before cancellation: {result:?}"),
            result = tokio::time::timeout(BUDGET, written) => { result.unwrap(); cancel.cancel(); },
        }
        first.await
    };
    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
    assert_eq!(load_checkpoint(&path).unwrap().durable_bytes, 3);

    let cancel = CancellationToken::new();
    let running = transfer(
        &client,
        &locator,
        &path,
        &mut checkpoint,
        TransferOptions {
            cancel: &cancel,
            rate_limit: None,
            budget: None,
        },
        &policy,
        |_| {},
    );
    tokio::pin!(running);
    let injection = tokio::task::spawn_blocking(|| {
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        assert_eq!(byte, [1]);
    });
    tokio::select! {
        result = &mut running => panic!("transfer ended before process kill: {result:?}"),
        result = injection => result.unwrap(),
    }
    // Deliberately reproduce bytes written after the last committed resource
    // checkpoint. This file is private, owned, and no response body is pending.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"XY").unwrap();
    file.sync_all().unwrap();
    records[0].snapshot.downloaded = 5;
    records[0].snapshot.revision = 2;
    journal.save(2, &mut records, None).unwrap();
    println!("{READY}");
    std::io::stdout().flush().unwrap();
    std::future::pending::<()>().await;
    let _ = running.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_transfer_recovers_checkpoint_and_normalizes_active_job() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("output")).unwrap();
    let unrelated = root.path().join("output/final.mp4");
    std::fs::write(&unrelated, b"unrelated user media").unwrap();
    let id = JobId::new();
    let (url, mut server, active) = server().await;
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
            .env("BILI_RECOVERY_TEST_ROOT", root.path())
            .env("BILI_RECOVERY_TEST_JOB", id.to_string())
            .env("BILI_RECOVERY_TEST_URL", url.as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let ready = tokio::task::spawn_blocking(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap().contains(READY) {
                return;
            }
        }
        panic!("child exited before readiness");
    });
    tokio::time::timeout(BUDGET, active).await.unwrap().unwrap();
    child.0.stdin.as_mut().unwrap().write_all(&[1]).unwrap();
    tokio::time::timeout(BUDGET, ready).await.unwrap().unwrap();

    let path = root
        .path()
        .join("state/jobs")
        .join(id.to_string())
        .join("resource-0.media");
    let saved = load_checkpoint(&path).unwrap();
    let checkpoint_bytes = std::fs::read(checkpoint_path(&path)).unwrap();
    assert_eq!(saved.durable_bytes, 3);
    assert_eq!(saved.strong_etag.as_deref(), Some("\"v1\""));
    assert_eq!(saved.sha256, Some(format!("{:x}", Sha256::digest(b"abc"))));
    assert_eq!(std::fs::read(&path).unwrap(), b"abcXY");
    let index = root.path().join("state/queue.json");
    let before = std::fs::read(&index).unwrap();
    let mut config = CoreConfig::new(root.path().join("state"), root.path().join("output"));
    config.credential_store = crate::cookie::StorePolicy::MemoryOnly;
    assert!(
        matches!(Core::new(config.clone(), tokio::runtime::Handle::current()), Err(error) if error.code == ErrorCode::Busy)
    );
    config.state_mode = StateMode::ReadOnly;
    let observer = Core::new(config.clone(), tokio::runtime::Handle::current()).unwrap();
    assert_eq!(
        observer.jobs().borrow().jobs[0].phase,
        JobPhase::Downloading
    );
    assert_eq!(observer.jobs().borrow().jobs[0].downloaded, 5);
    observer.shutdown(StopMode::Pause).await.unwrap();
    assert_eq!(std::fs::read(&index).unwrap(), before);
    let mut other = config.clone();
    other.state_dir = root.path().join("different-state");
    let empty = Core::new(other, tokio::runtime::Handle::current()).unwrap();
    assert!(empty.jobs().borrow().jobs.is_empty());
    empty.shutdown(StopMode::Pause).await.unwrap();
    assert!(!root.path().join("different-state").exists());

    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let after_crash = Core::new(config.clone(), tokio::runtime::Handle::current()).unwrap();
    assert_eq!(
        after_crash.jobs().borrow().jobs[0].phase,
        JobPhase::Downloading
    );
    after_crash.shutdown(StopMode::Pause).await.unwrap();
    assert_eq!(std::fs::read(&index).unwrap(), before);
    config.state_mode = StateMode::ReadWrite;
    let recovered = Core::new(config, tokio::runtime::Handle::current()).unwrap();
    assert_eq!(recovered.jobs().borrow().jobs[0].id, id);
    assert_eq!(recovered.jobs().borrow().jobs[0].phase, JobPhase::Paused);
    assert_eq!(
        recovered.jobs().borrow().jobs[0]
            .plan
            .as_ref()
            .unwrap()
            .fingerprint,
        plan(root.path()).fingerprint
    );
    assert_ne!(std::fs::read(&index).unwrap(), before);
    assert_eq!(
        std::fs::read(checkpoint_path(&path)).unwrap(),
        checkpoint_bytes
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"abcXY");
    let journal = Journal::open(&root.path().join("state"), StateMode::ReadOnly).unwrap();
    let (_, records) = journal.load().unwrap();
    assert_eq!(
        records[0].resources[0].identity,
        "offline-recovery-resource"
    );

    let mut checkpoint = load_checkpoint(&path).unwrap();
    let result = transfer(
        &client(),
        &resource_locator(url),
        &path,
        &mut checkpoint,
        TransferOptions {
            cancel: &CancellationToken::new(),
            rate_limit: None,
            budget: None,
        },
        &RetryPolicy {
            attempts: 1,
            base_delay: Duration::ZERO,
        },
        |_| {},
    )
    .await;
    let requests = server.finish().await;
    result.unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests[1..] {
        let request = request.to_ascii_lowercase();
        assert!(request.contains("range: bytes=3-\r\n"));
        assert!(request.contains("if-range: \"v1\"\r\n"));
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
    assert!(checkpoint.complete);
    assert_eq!(checkpoint.durable_bytes, 6);
    assert_eq!(
        checkpoint.sha256,
        Some(format!("{:x}", Sha256::digest(b"abcdef")))
    );
    assert_eq!(load_checkpoint(&path).unwrap().sha256, checkpoint.sha256);
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated user media");
    recovered.shutdown(StopMode::Pause).await.unwrap();
}
