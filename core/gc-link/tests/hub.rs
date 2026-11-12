//! End-to-end tests of the development hub over real loopback sockets.

mod common;
use common::fake_reader;

use gc_link::{bind_dev, serve, ErrorBody, FileInfo, Health, HubConfig, HubError, Job, UploadInfo};
use std::net::SocketAddr;
use std::path::PathBuf;

const TOKEN: &str = "test-token-0123456789";

async fn start(max_file_bytes: u64) -> (String, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("gc-hub-test-{}", uuid::Uuid::new_v4()));
    let config = HubConfig {
        token: TOKEN.into(),
        data_dir: dir.clone(),
        max_file_bytes,
        allowed_origins: vec![],
        ocr_command: None,
        lan: None,
    };
    // The test client is built on the same TLS library as the hub, and wants
    // it named even for plain HTTP.
    gc_link::tls::install_crypto_provider();
    let listener = bind_dev("127.0.0.1:0".parse().unwrap(), &config).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, config));
    (base, dir)
}

#[tokio::test]
async fn refuses_non_loopback_and_weak_tokens() {
    let mut config = HubConfig {
        token: TOKEN.into(),
        data_dir: std::env::temp_dir(),
        max_file_bytes: 1,
        allowed_origins: vec![],
        ocr_command: None,
        lan: None,
    };
    let public: SocketAddr = "0.0.0.0:0".parse().unwrap();
    assert!(matches!(bind_dev(public, &config).await, Err(HubError::NotLoopback(_))));
    config.token = "short".into();
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    assert!(matches!(bind_dev(local, &config).await, Err(HubError::WeakToken)));
}

#[tokio::test]
async fn health_is_public_and_everything_else_needs_the_token() {
    let (base, dir) = start(1 << 20).await;
    let http = reqwest::Client::new();

    let health: Health =
        http.get(format!("{base}/v1/health")).send().await.unwrap().json().await.unwrap();
    assert_eq!(health.status, "ok");

    let denied = http.post(format!("{base}/v1/uploads")).send().await.unwrap();
    assert_eq!(denied.status(), 401);
    assert!(denied.headers().contains_key("x-request-id"));
    let body: ErrorBody = denied.json().await.unwrap();
    assert_eq!(body.error, "missing or invalid token");

    let wrong = http.post(format!("{base}/v1/uploads")).bearer_auth("nope").send().await.unwrap();
    assert_eq!(wrong.status(), 401);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn browser_origins_are_rejected() {
    let (base, dir) = start(1 << 20).await;
    let http = reqwest::Client::new();
    let res = http
        .post(format!("{base}/v1/uploads"))
        .bearer_auth(TOKEN)
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // Even the public health route refuses a foreign origin.
    let res = http
        .get(format!("{base}/v1/health"))
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn upload_resume_and_job() {
    let (base, dir) = start(1 << 20).await;
    let http = reqwest::Client::new();
    let upload: UploadInfo = http
        .post(format!("{base}/v1/uploads"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = format!("{base}/v1/uploads/{}/files/page-001.jpg", upload.id);

    // First half, then a wrong offset, then the rest from the stored size.
    let first: FileInfo = http
        .put(&url)
        .bearer_auth(TOKEN)
        .body(vec![1u8; 600])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first.bytes, 600);
    let conflict =
        http.put(format!("{url}?offset=0")).bearer_auth(TOKEN).body(vec![2u8; 10]).send().await.unwrap();
    assert_eq!(conflict.status(), 409);
    let done: FileInfo = http
        .put(format!("{url}?offset=600"))
        .bearer_auth(TOKEN)
        .body(vec![2u8; 400])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(done.bytes, 1000);

    let listed: UploadInfo = http
        .get(format!("{base}/v1/uploads/{}", upload.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.files, vec![FileInfo { name: "page-001.jpg".into(), bytes: 1000 }]);

    let job: Job = http
        .post(format!("{base}/v1/jobs"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "kind": "inventory", "upload_id": upload.id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(job.status, "done");
    assert_eq!(job.result.unwrap()["bytes"], 1000);
    let fetched = http.get(format!("{base}/v1/jobs/{}", job.id)).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(fetched.status(), 200);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn rejects_bad_names_oversize_files_and_unknown_ids() {
    let (base, dir) = start(1000).await;
    let http = reqwest::Client::new();
    let upload: UploadInfo = http
        .post(format!("{base}/v1/uploads"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let files = format!("{base}/v1/uploads/{}/files", upload.id);

    // A bare ".." never reaches the handler: HTTP clients resolve it away.
    for bad in ["..%2Fx.jpg", "a%2Fb.jpg", "sp%20ace.jpg", ".hidden%5C"] {
        let res = http.put(format!("{files}/{bad}")).bearer_auth(TOKEN).body("x").send().await.unwrap();
        assert_eq!(res.status(), 400, "name {bad}");
    }
    let big = http.put(format!("{files}/big.bin")).bearer_auth(TOKEN).body(vec![0u8; 1001]).send().await.unwrap();
    assert_eq!(big.status(), 413);
    assert!(!dir.join("uploads").join(&upload.id).join("big.bin").exists());

    let missing = http
        .get(format!("{base}/v1/uploads/00000000-0000-0000-0000-000000000000"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    let traversal = http.get(format!("{base}/v1/uploads/..%2F..")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(traversal.status(), 404);
    let job = http
        .post(format!("{base}/v1/jobs"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "kind": "format-disk", "upload_id": upload.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(job.status(), 400);
    std::fs::remove_dir_all(dir).ok();
}

async fn hub_with_reader(reader: Option<Vec<String>>) -> (String, PathBuf) {
    let dir = std::env::temp_dir().join(format!("gc-hub-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let ocr_command = reader.or_else(|| Some(fake_reader(&dir, false)));
    let config =
        HubConfig { token: TOKEN.into(), data_dir: dir.clone(), max_file_bytes: 1 << 20, allowed_origins: vec![], ocr_command, lan: None };
    gc_link::tls::install_crypto_provider();
    let listener = bind_dev("127.0.0.1:0".parse().unwrap(), &config).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, config));
    (base, dir)
}

async fn upload_pictures(http: &reqwest::Client, base: &str, names: &[&str]) -> String {
    let upload: UploadInfo =
        http.post(format!("{base}/v1/uploads")).bearer_auth(TOKEN).send().await.unwrap().json().await.unwrap();
    for name in names {
        let url = format!("{base}/v1/uploads/{}/files/{name}", upload.id);
        assert!(http.put(url).bearer_auth(TOKEN).body("not really a picture").send().await.unwrap().status().is_success());
    }
    upload.id
}

async fn wait_for(http: &reqwest::Client, base: &str, id: &str) -> Job {
    for _ in 0..200 {
        let job: Job =
            http.get(format!("{base}/v1/jobs/{id}")).bearer_auth(TOKEN).send().await.unwrap().json().await.unwrap();
        if job.status == "done" || job.status == "failed" {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("job {id} did not finish");
}

#[tokio::test]
async fn document_job_reads_merges_and_serves_results() {
    let (base, dir) = hub_with_reader(None).await;
    let http = reqwest::Client::new();
    let upload = upload_pictures(&http, &base, &["a.jpg", "b.PNG", "notes.txt"]).await;
    let started: Job = http
        .post(format!("{base}/v1/jobs"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "kind": "document", "upload_id": upload }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(started.status == "queued" || started.status == "running");

    let job = wait_for(&http, &base, &started.id).await;
    assert_eq!(job.status, "done", "{:?}", job.error);
    assert_eq!(job.progress, 1.0);
    let result = job.result.clone().unwrap();
    // The text file is not a picture and is left out.
    assert_eq!(result["pictures"], 2);
    assert_eq!(result["lines_read"], 4);
    assert_eq!(result["repeated_lines_removed"], 2);
    assert_eq!(job.files, ["document.md", "document.txt", "document.json"]);

    let text = http
        .get(format!("{base}/v1/jobs/{}/files/document.txt", job.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(text, "Dòng chữ thứ nhất của trang dòng chữ thứ hai của trang.\n");

    // Only the files the job lists can be fetched, and only with the token.
    let other = http.get(format!("{base}/v1/jobs/{}/files/job.json", job.id)).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(other.status(), 404);
    let open = http.get(format!("{base}/v1/jobs/{}/files/document.txt", job.id)).send().await.unwrap();
    assert_eq!(open.status(), 401);

    // The job is on disk, so a hub started on the same folder still knows it.
    let saved: Job = serde_json::from_str(&std::fs::read_to_string(dir.join("jobs").join(&job.id).join("job.json")).unwrap()).unwrap();
    assert_eq!(saved, job);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn document_job_reports_a_failing_reader_in_plain_words() {
    let dir = std::env::temp_dir().join(format!("gc-hub-reader-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (base, data) = hub_with_reader(Some(fake_reader(&dir, true))).await;
    let http = reqwest::Client::new();
    let upload = upload_pictures(&http, &base, &["a.jpg"]).await;
    let started: Job = http
        .post(format!("{base}/v1/jobs"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "kind": "document", "upload_id": upload }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job = wait_for(&http, &base, &started.id).await;
    assert_eq!(job.status, "failed");
    // The client is told what happened, not what the tool printed.
    assert_eq!(job.error.as_deref(), Some("the text reader failed"));
    assert!(job.files.is_empty());
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(data).ok();
}

#[tokio::test]
async fn document_job_needs_pictures_and_a_reader() {
    let (base, dir) = hub_with_reader(None).await;
    let http = reqwest::Client::new();
    let upload = upload_pictures(&http, &base, &["notes.txt"]).await;
    let none = http
        .post(format!("{base}/v1/jobs"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "kind": "document", "upload_id": upload }))
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 400);
    std::fs::remove_dir_all(dir).ok();

    // A hub without a text reader refuses document jobs outright.
    let (base, dir) = start(1 << 20).await;
    let upload = upload_pictures(&http, &base, &["a.jpg"]).await;
    let refused = http
        .post(format!("{base}/v1/jobs"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "kind": "document", "upload_id": upload }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 501);
    std::fs::remove_dir_all(dir).ok();
}
