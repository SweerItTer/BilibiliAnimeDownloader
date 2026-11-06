//! Exercise the worker's real resource loop with isolated, validated checkpoints.
use super::*;
use crate::{api::ResolvedLocator, cookie::StorePolicy};
use reqwest::Url;
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn checkpoint(bytes: &[u8], total: u64, complete: bool) -> ResourceCheckpoint {
    ResourceCheckpoint {
        durable_bytes: bytes.len() as u64,
        expected_total: Some(total),
        strong_etag: Some("\"v1\"".into()),
        sha256: Some(format!("{:x}", Sha256::digest(bytes))),
        complete,
    }
}

fn worker(
    root: &Path,
    second_bytes: &[u8],
    second_checkpoint: ResourceCheckpoint,
) -> (Worker, StreamPlan, mpsc::Receiver<Command>) {
    let mut config = CoreConfig::new(root.join("state"), root.join("out"));
    config.retry = RetryPolicy {
        attempts: 1,
        base_delay: Duration::ZERO,
    };
    let plan = super::lifecycle_tests::offline_plan(root.join("output.mp4"));
    let mut record = JobRecord::planned(JobId::new(), plan.clone());
    record.snapshot.phase = JobPhase::Paused;
    record.snapshot.downloaded = 6;
    let journal = Arc::new(Journal::open(&config.state_dir, StateMode::ReadWrite).unwrap());
    for (index, (bytes, checkpoint)) in [
        (b"abc".as_slice(), checkpoint(b"abc", 3, true)),
        (second_bytes, second_checkpoint),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("resource-{index}.media");
        let path = journal.owned_file(record.snapshot.id, &name).unwrap();
        std::fs::write(&path, bytes).unwrap();
        state::atomic_json(&path.with_extension("media.checkpoint.json"), &checkpoint).unwrap();
        record.resources.push(ResourceRecord {
            name,
            identity: format!("test resource {index}"),
        });
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let credentials = Arc::new(RwLock::new(CredentialJar::default()));
    let connections = Arc::new(Semaphore::new(1));
    let api = Arc::new(Api::new(
        client.clone(),
        credentials.clone(),
        connections.clone(),
        config.retry.clone(),
    ));
    let auth = Arc::new(Mutex::new(AuthManager::new(
        client.clone(),
        credentials.clone(),
        StorePolicy::MemoryOnly,
        connections.clone(),
    )));
    let (commands, receiver) = mpsc::channel(128);
    (
        Worker {
            record,
            journal,
            commands,
            api,
            client: client.clone(),
            sidecar_client: client,
            credentials,
            auth,
            budget: None,
            connections,
            merges: Arc::new(Semaphore::new(1)),
            config,
            cancel: CancellationToken::new(),
        },
        plan,
        receiver,
    )
}

struct TransferRun {
    progress: Vec<u64>,
    request: String,
    files: Vec<Vec<u8>>,
    checkpoints: Vec<ResourceCheckpoint>,
}

async fn run_transfer(
    second_bytes: &[u8],
    second_checkpoint: ResourceCheckpoint,
    status: &str,
    headers: &str,
    body: &str,
) -> TransferRun {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let response = format!("HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n{body}");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
        String::from_utf8(request).unwrap().to_ascii_lowercase()
    });
    let root = tempfile::tempdir().unwrap();
    let (worker, plan, mut receiver) = worker(root.path(), second_bytes, second_checkpoint);
    let locators: Vec<_> = (0..2)
        .map(|index| ResolvedLocator {
            url: Url::parse(&format!("http://{address}/resource-{index}")).unwrap(),
            referer: "https://www.bilibili.com/".into(),
            identity: format!("test resource {index}"),
            expected_size: Some(if index == 0 { 3 } else { 6 }),
            duration_ms: None,
            allowed_origins: Vec::new(),
        })
        .collect();
    let paths = tokio::time::timeout(
        Duration::from_secs(5),
        worker.transfer_resources(&plan, &locators),
    )
    .await
    .expect("worker transfer timed out")
    .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("resource server timed out")
        .unwrap();
    let mut progress = Vec::new();
    while let Ok(command) = receiver.try_recv() {
        match command {
            Command::Progress(id, bytes) => {
                assert_eq!(id, worker.id());
                progress.push(bytes);
            }
            _ => panic!("unexpected command from the resource transfer loop"),
        }
    }
    TransferRun {
        progress,
        request,
        files: paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect(),
        checkpoints: paths
            .iter()
            .map(|path| crate::download::load_checkpoint(path).unwrap())
            .collect(),
    }
}

fn assert_completed(run: &TransferRun) {
    assert_eq!(run.files, [b"abc".to_vec(), b"abcdef".to_vec()]);
    assert_eq!(run.progress.last(), Some(&9));
    for (checkpoint, bytes) in run.checkpoints.iter().zip(&run.files) {
        assert!(checkpoint.complete);
        assert_eq!(checkpoint.durable_bytes, bytes.len() as u64);
        assert_eq!(
            checkpoint.sha256,
            Some(format!("{:x}", Sha256::digest(bytes)))
        );
    }
    assert!(run.request.starts_with("get /resource-1 "));
}

#[tokio::test]
async fn resume_retains_later_partial_resource_in_aggregate_progress() {
    let run = run_transfer(
        b"abc",
        checkpoint(b"abc", 6, false),
        "206 Partial Content",
        "Content-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"v1\"\r\n",
        "def",
    )
    .await;

    assert_eq!(run.progress.first(), Some(&6));
    assert!(run.progress.iter().all(|bytes| *bytes >= 6));
    assert!(run.request.contains("\r\nrange: bytes=3-\r\n"));
    assert!(run.request.contains("\r\nif-range: \"v1\"\r\n"));
    assert_completed(&run);
}

#[tokio::test]
async fn invalid_checkpoint_contributes_no_retained_progress() {
    let run = run_transfer(
        b"bad",
        checkpoint(b"abc", 6, false),
        "200 OK",
        "Content-Length: 6\r\nETag: \"v1\"\r\n",
        "abcdef",
    )
    .await;

    assert_eq!(run.progress.first(), Some(&3));
    assert!(!run.request.contains("\r\nrange:"));
    assert!(!run.request.contains("\r\nif-range:"));
    assert_completed(&run);
}

#[tokio::test]
async fn ignored_range_reports_only_the_invalidated_resource_reset() {
    let run = run_transfer(
        b"abc",
        checkpoint(b"abc", 6, false),
        "200 OK",
        "Content-Length: 6\r\nETag: \"v2\"\r\n",
        "abcdef",
    )
    .await;

    assert_eq!(run.progress.first(), Some(&6));
    assert!(run.progress.windows(2).any(|pair| pair == [6, 3]));
    assert!(run.request.contains("\r\nrange: bytes=3-\r\n"));
    assert!(run.request.contains("\r\nif-range: \"v1\"\r\n"));
    assert_completed(&run);
}

#[tokio::test]
async fn completed_inputs_require_matching_identity_and_digest_without_modifying_files() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut plan, _receiver) =
        worker(root.path(), b"def", checkpoint(b"def", 3, true));
    plan.topology = StreamTopology::Dash;
    let mut audio = plan.video.clone();
    audio.audio = true;
    plan.audio = Some(audio);
    for (resource, identity) in worker
        .record
        .resources
        .iter_mut()
        .zip(crate::api::planned_resource_identities(&plan).unwrap())
    {
        resource.identity = identity;
    }
    let paths = worker.completed_resources(&plan).await.unwrap().unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(std::fs::read(&paths[1]).unwrap(), b"def");
    worker.record.resources.push(ResourceRecord {
        name: "resource-2.media".into(),
        identity: "additional DASH segment".into(),
    });
    assert!(worker.completed_resources(&plan).await.unwrap().is_none());
    worker.record.resources.pop();
    let checkpoint_path = paths[1].with_extension("media.checkpoint.json");
    let proof = std::fs::read(&checkpoint_path).unwrap();
    std::fs::write(&paths[1], b"bad").unwrap();
    assert_eq!(
        worker.completed_resources(&plan).await.unwrap_err().code,
        ErrorCode::Corruption
    );
    assert_eq!(std::fs::read(&paths[1]).unwrap(), b"bad");
    assert_eq!(std::fs::read(&checkpoint_path).unwrap(), proof);
    std::fs::write(&paths[1], b"def").unwrap();
    worker.record.resources[0].identity = "different representation".into();
    assert_eq!(
        worker.completed_resources(&plan).await.unwrap_err().code,
        ErrorCode::RepresentationChanged
    );
    worker.record.resources[0].identity = crate::api::planned_resource_identities(&plan)
        .unwrap()
        .remove(0);
    state::atomic_json(&checkpoint_path, &checkpoint(b"def", 6, false)).unwrap();
    assert!(worker.completed_resources(&plan).await.unwrap().is_none());
}
