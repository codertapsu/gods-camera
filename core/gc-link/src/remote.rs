//! The remote: a paired phone or tablet works the computer's Camera screen
//! during a call (plan, sections 6.7 and 6.12, and Phase 3b).
//!
//! The desktop app hands the hub a [`CameraRemote`] with
//! [`Hub::set_camera_remote`](crate::Hub::set_camera_remote). A device then
//! asks how things stand, sees the picture others see as a small JPEG,
//! chooses a face, and turns the swap, a recording and the loop on and off:
//! the controls under the picture on the computer. Without a camera, as when
//! the hub runs from the command line, the routes say so.
//!
//! | Route | What |
//! |---|---|
//! | `GET /v1/camera` | [`RemoteState`] |
//! | `GET /v1/camera/faces` | the faces, as [`RemoteFace`] |
//! | `GET /v1/camera/faces/{id}/picture` | a face's small picture, PNG |
//! | `GET /v1/camera/picture` | the picture others see, JPEG |
//! | `POST /v1/camera` | a [`RemoteAction`] as JSON; 204, or 409 with the reason |
//!
//! Every route needs the device's token, as every other route does.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{locked, AppState};

/// How the computer's camera stands, as a device shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteState {
    /// `"live"`, `"looping"` or `"idle"`.
    pub mode: String,
    pub recording: bool,
    pub recorded_seconds: f64,
    /// A recording can start now: the picture is live.
    pub can_record: bool,
    /// A loop can be played, or the one that plays ended.
    pub can_loop: bool,
    /// The user wants the swap on.
    pub swap_wanted: bool,
    /// A swapped face is being drawn.
    pub swapping: bool,
    /// Why the swap is not on the face when it is wanted, as a short code the
    /// device puts in its own words: `getting_ready`, `model_missing`,
    /// `failed`, `no_face_seen` or `not_on_face`.
    pub swap_problem: Option<String>,
    /// The face that would be shown.
    pub selected_face: Option<String>,
    /// An app is reading the camera: someone in a call sees this picture.
    pub watched: bool,
}

/// A face the computer can show, by its id and name. Its photos stay on the computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFace {
    pub id: String,
    pub name: String,
}

/// What a device asks the computer to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RemoteAction {
    /// Turn the swap on or off. The first time, the rules for swapping a face
    /// must be accepted on the computer; until then this is refused.
    Swap { on: bool },
    /// Show this face.
    Face { id: String },
    /// Start a recording, or end the one that runs.
    Record,
    /// Play the loop, or go back to live.
    Loop,
}

/// The desktop app's camera, as the hub lets a paired device work it.
pub trait CameraRemote: Send + Sync {
    fn state(&self) -> RemoteState;
    fn faces(&self) -> Vec<RemoteFace>;
    /// A face's small picture as PNG, or `None` for a face that is not there.
    fn face_picture(&self, id: &str) -> Option<Vec<u8>>;
    /// The picture others see now, as a small JPEG; `None` before the first one.
    fn picture(&self) -> Option<Vec<u8>>;
    /// Do what the device asks. An error says why not, as a short code the
    /// device puts in its own words, such as `no_loop_yet`.
    fn act(&self, action: RemoteAction) -> Result<(), String>;
}

const NOT_HERE: &str = "the camera is not available here";

fn remote(state: &AppState) -> Option<Arc<dyn CameraRemote>> {
    locked(&state.remote).clone()
}

/// Run the app's call off the server's threads: it takes the app's locks, and a picture is encoded.
async fn ask<T: Send + 'static>(
    state: &AppState,
    call: impl FnOnce(&dyn CameraRemote) -> T + Send + 'static,
) -> Result<T, (StatusCode, &'static str)> {
    let Some(remote) = remote(state) else {
        return Err((StatusCode::NOT_FOUND, NOT_HERE));
    };
    tokio::task::spawn_blocking(move || call(remote.as_ref()))
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "the camera did not answer"))
}

pub(crate) async fn state(State(state): State<AppState>) -> Response {
    match ask(&state, |remote| remote.state()).await {
        Ok(answer) => Json(answer).into_response(),
        Err(failure) => failure.into_response(),
    }
}

pub(crate) async fn faces(State(state): State<AppState>) -> Response {
    match ask(&state, |remote| remote.faces()).await {
        Ok(answer) => Json(answer).into_response(),
        Err(failure) => failure.into_response(),
    }
}

pub(crate) async fn face_picture(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match ask(&state, move |remote| remote.face_picture(&id)).await {
        Ok(Some(png)) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], png).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such face").into_response(),
        Err(failure) => failure.into_response(),
    }
}

pub(crate) async fn picture(State(state): State<AppState>) -> Response {
    match ask(&state, |remote| remote.picture()).await {
        Ok(Some(jpeg)) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], jpeg).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "there is no picture yet").into_response(),
        Err(failure) => failure.into_response(),
    }
}

pub(crate) async fn act(State(state): State<AppState>, Json(action): Json<RemoteAction>) -> Response {
    match ask(&state, move |remote| remote.act(action)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
        Err(failure) => failure.into_response(),
    }
}
