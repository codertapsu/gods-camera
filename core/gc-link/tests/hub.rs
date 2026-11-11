//! End-to-end tests of the development hub over real loopback sockets.

use gc_link::{bind_dev, serve, ErrorBody, FileInfo, Health, HubConfig, HubError, Job, UploadInfo};
use std::net::SocketAddr;

const TOKEN: &str = "test-token-0123456789";

async fn start(max_file_bytes: u64) -> (String, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("gc-hub-test-{}", uuid::Uuid::new_v4()));
    let config = HubConfig {
        token: TOKEN.into(),
        data_dir: dir.clone(),
        max_file_bytes,
        allowed_origins: vec![],
    };
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
