//! What the model needs from the system it runs on, for the systems whose app
//! uses this model. Today that is Windows.

/// The platform for the system this was built for.
#[cfg(not(windows))]
pub fn native() -> Result<std::sync::Arc<dyn crate::model::Platform>, String> {
    Err("this system's app keeps its model elsewhere".into())
}

#[cfg(windows)]
pub use windows::native;

#[cfg(windows)]
mod windows {
    use crate::faces::{new_key, FaceError, KeyStore};
    use crate::model::{CameraChoice, CaptureFrom, OnFrame, Platform, SourceProblem, VirtualCamera};
    use gc_camera::Output;
    use gc_win::{CameraKeeper, CameraOutput, Capture, CaptureSource, Credential, Feed, FeedError};
    use std::sync::Arc;

    /// The name the key for saved faces goes under in Credential Manager.
    const FACE_KEY: &str = "GodsCamera.FaceStore";

    struct FaceKey;

    impl KeyStore for FaceKey {
        fn load_or_create(&self) -> Result<[u8; 32], FaceError> {
            let entry = Credential::new(FACE_KEY);
            // An entry of another length is not a key of ours: a new one replaces it.
            if let Some(key) = entry.read().map_err(FaceError::Key)?.and_then(|secret| <[u8; 32]>::try_from(secret).ok()) {
                return Ok(key);
            }
            let key = new_key();
            entry.write(&key, "The key that encrypts the faces saved in God's Camera.").map_err(FaceError::Key)?;
            Ok(key)
        }

        fn delete(&self) {
            Credential::new(FACE_KEY).delete();
        }
    }

    struct Windows {
        /// The pipe the camera's media source fetches pictures from, if this app could make it.
        feed: Option<Arc<Feed>>,
        /// Holds the camera in every app's list for as long as the app runs.
        _camera: Option<CameraKeeper>,
        state: (VirtualCamera, String),
    }

    impl Platform for Windows {
        fn output(&self) -> Option<Box<dyn Output>> {
            self.feed.as_ref().map(|feed| Box::new(CameraOutput::new(Arc::clone(feed))) as Box<dyn Output>)
        }

        fn clock(&self) -> u64 {
            CameraOutput::clock()
        }

        fn virtual_camera(&self) -> (VirtualCamera, String) {
            self.state.clone()
        }

        fn key_store(&self) -> Box<dyn KeyStore> {
            Box::new(FaceKey)
        }

        fn cameras(&self) -> Vec<CameraChoice> {
            gc_win::cameras().into_iter().map(|camera| CameraChoice { id: camera.id, name: camera.name }).collect()
        }

        fn capture(&self, from: CaptureFrom, on_frame: OnFrame) -> Result<Box<dyn Send>, SourceProblem> {
            let (source, from_file) = match from {
                CaptureFrom::Camera(id) => (CaptureSource::Camera(id), false),
                CaptureFrom::File(path) => (CaptureSource::File(path), true),
            };
            match Capture::start(source, on_frame) {
                Ok(capture) => Ok(Box::new(capture)),
                Err(_) if from_file => Err(SourceProblem::FileUnreadable),
                Err(error) if error.refused() => Err(SourceProblem::CameraRefused),
                Err(_) => Err(SourceProblem::CameraUnavailable),
            }
        }
    }

    /// Make the pipe and ask Windows for the camera. Neither failing stops
    /// the app: it then runs with a preview and says why other apps see no camera.
    pub fn native() -> Result<Arc<dyn Platform>, String> {
        let feed = match Feed::open() {
            Ok(feed) => Arc::new(feed),
            Err(FeedError::Taken) => {
                return Ok(Arc::new(Windows { feed: None, _camera: None, state: (VirtualCamera::Busy, String::new()) }));
            }
            Err(error) => {
                return Ok(Arc::new(Windows { feed: None, _camera: None, state: (VirtualCamera::Failed, error.to_string()) }));
            }
        };
        let (camera, state) = match CameraKeeper::start(feed.pipe_name()) {
            Ok(camera) => (Some(camera), (VirtualCamera::Ready, String::new())),
            Err(error) if error.not_installed() => (None, (VirtualCamera::NotInstalled, error.to_string())),
            Err(error) if error.refused() => (None, (VirtualCamera::Refused, error.to_string())),
            Err(error) => (None, (VirtualCamera::Failed, error.to_string())),
        };
        Ok(Arc::new(Windows { feed: Some(feed), _camera: camera, state }))
    }
}
