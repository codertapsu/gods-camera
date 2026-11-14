//! The remote over a real loopback socket, with a stand-in for the desktop app's camera.

use gc_link::{CameraRemote, ErrorBody, Hub, HubConfig, RemoteAction, RemoteFace, RemoteState};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "test-token-0123456789";

/// A camera that remembers what it was asked to do.
#[derive(Default)]
struct StandIn {
    done: Mutex<Vec<RemoteAction>>,
    swap: Mutex<bool>,
}

impl CameraRemote for StandIn {
    fn state(&self) -> RemoteState {
        RemoteState {
            mode: "live".into(),
            recording: false,
            recorded_seconds: 0.0,
            can_record: true,
            can_loop: false,
            swap_wanted: *self.swap.lock().unwrap(),
            swapping: false,
            swap_problem: None,
            selected_face: Some("a".into()),
            watched: true,
        }
    }

    fn faces(&self) -> Vec<RemoteFace> {
        vec![RemoteFace { id: "a".into(), name: "Sample: me".into() }, RemoteFace { id: "b".into(), name: "Mặt mẫu".into() }]
    }

    fn face_picture(&self, id: &str) -> Option<Vec<u8>> {
        (id == "a").then(|| b"\x89PNG not really".to_vec())
    }

    fn picture(&self) -> Option<Vec<u8>> {
        Some(vec![0xFF, 0xD8, 0xFF, 0xD9])
    }

    fn act(&self, action: RemoteAction) -> Result<(), String> {
        if let RemoteAction::Face { id } = &action {
            if id != "a" && id != "b" {
                return Err("That face is not on the computer any more.".into());
            }
        }
        if let RemoteAction::Swap { on } = &action {
            *self.swap.lock().unwrap() = *on;
        }
        self.done.lock().unwrap().push(action);
        Ok(())
    }
}

async fn start() -> (String, Hub, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("gc-remote-test-{}", uuid::Uuid::new_v4()));
    let config = HubConfig { token: TOKEN.into(), data_dir: dir.clone(), max_file_bytes: 1 << 20, allowed_origins: vec![], ocr_command: None, lan: None };
    gc_link::tls::install_crypto_provider();
    let bound = Hub::bind("127.0.0.1:0".parse().unwrap(), config).await.unwrap();
    let base = format!("http://{}", bound.local_addr().unwrap());
    let hub = bound.hub();
    tokio::spawn(bound.serve());
    (base, hub, dir)
}

#[tokio::test]
async fn a_device_works_the_camera_through_the_hub() {
    let (base, hub, dir) = start().await;
    let http = reqwest::Client::new();

    // Before the app hands its camera over, the routes say there is none.
    let none = http.get(format!("{base}/v1/camera")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(none.status(), 404);
    assert_eq!(none.json::<ErrorBody>().await.unwrap().error, "the camera is not available here");

    let camera = Arc::new(StandIn::default());
    hub.set_camera_remote(Some(camera.clone()));

    // Every route wants the token, as the hub's other routes do.
    assert_eq!(http.get(format!("{base}/v1/camera")).send().await.unwrap().status(), 401);
    assert_eq!(http.get(format!("{base}/v1/camera/picture")).send().await.unwrap().status(), 401);

    let state: RemoteState = http.get(format!("{base}/v1/camera")).bearer_auth(TOKEN).send().await.unwrap().json().await.unwrap();
    assert_eq!((state.mode.as_str(), state.watched, state.selected_face.as_deref()), ("live", true, Some("a")));
    let faces: Vec<RemoteFace> = http.get(format!("{base}/v1/camera/faces")).bearer_auth(TOKEN).send().await.unwrap().json().await.unwrap();
    assert_eq!(faces.iter().map(|face| face.name.as_str()).collect::<Vec<_>>(), ["Sample: me", "Mặt mẫu"]);

    let picture = http.get(format!("{base}/v1/camera/picture")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(picture.status(), 200);
    assert_eq!(picture.headers()["content-type"], "image/jpeg");
    assert_eq!(picture.bytes().await.unwrap().as_ref(), [0xFF, 0xD8, 0xFF, 0xD9]);
    let face = http.get(format!("{base}/v1/camera/faces/a/picture")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!((face.status().as_u16(), face.headers()["content-type"].to_str().unwrap()), (200, "image/png"));
    assert_eq!(http.get(format!("{base}/v1/camera/faces/zzz/picture")).bearer_auth(TOKEN).send().await.unwrap().status(), 404);

    // The controls, as JSON.
    for body in [r#"{"action":"swap","on":true}"#, r#"{"action":"face","id":"b"}"#, r#"{"action":"record"}"#, r#"{"action":"loop"}"#] {
        let done = http
            .post(format!("{base}/v1/camera"))
            .bearer_auth(TOKEN)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(done.status(), 204, "{body}");
    }
    assert_eq!(
        *camera.done.lock().unwrap(),
        [RemoteAction::Swap { on: true }, RemoteAction::Face { id: "b".into() }, RemoteAction::Record, RemoteAction::Loop]
    );
    let state: RemoteState = http.get(format!("{base}/v1/camera")).bearer_auth(TOKEN).send().await.unwrap().json().await.unwrap();
    assert!(state.swap_wanted);

    // What the camera refuses comes back with its reason; what it cannot read, with the hub's.
    let refused = http.post(format!("{base}/v1/camera")).bearer_auth(TOKEN).json(&RemoteAction::Face { id: "gone".into() }).send().await.unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(refused.json::<ErrorBody>().await.unwrap().error, "That face is not on the computer any more.");
    let nonsense = http.post(format!("{base}/v1/camera")).bearer_auth(TOKEN).header("content-type", "application/json").body(r#"{"action":"fly"}"#).send().await.unwrap();
    assert_eq!(nonsense.status(), 422);
    let huge = http.post(format!("{base}/v1/camera")).bearer_auth(TOKEN).header("content-type", "application/json").body(format!(r#"{{"action":"face","id":"{}"}}"#, "x".repeat(10_000))).send().await.unwrap();
    assert_eq!(huge.status(), 413);

    // Taken back, the camera is gone from the hub again.
    hub.set_camera_remote(None);
    assert_eq!(http.get(format!("{base}/v1/camera")).bearer_auth(TOKEN).send().await.unwrap().status(), 404);
    std::fs::remove_dir_all(dir).ok();
}
