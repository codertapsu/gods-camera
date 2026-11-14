//! What the models need from the system they run on, for the systems whose
//! app uses them. Today that is Windows.

use crate::model::AppConfig;
use crate::scanner::{ScanPlatform, ScannerConfig};
use crate::secrets::{FolderSecrets, Secrets};
use std::path::Path;
use std::sync::Arc;

/// Where a try-out keeps its secrets: in its own data folder.
fn try_out_secrets(data_dir: &str) -> Arc<dyn Secrets> {
    Arc::new(FolderSecrets::new(Path::new(data_dir).join("Secrets")))
}

/// The platform for the system this was built for.
#[cfg(not(windows))]
pub fn native(_config: &AppConfig) -> Result<Arc<dyn crate::model::Platform>, String> {
    Err("this system's app keeps its model elsewhere".into())
}

/// The scanner's platform on a system that has none written for it: the hub
/// runs, its secrets are kept in the data folder, and nothing is announced.
#[cfg(not(windows))]
pub fn scan_platform(config: &ScannerConfig) -> Arc<dyn ScanPlatform> {
    struct Plain(Arc<dyn Secrets>);
    impl ScanPlatform for Plain {
        fn secrets(&self) -> Arc<dyn Secrets> {
            Arc::clone(&self.0)
        }
        fn computer_name(&self) -> String {
            "This computer".into()
        }
    }
    Arc::new(Plain(try_out_secrets(&config.data_dir)))
}

#[cfg(windows)]
pub use windows::{native, scan_platform};

#[cfg(windows)]
mod windows {
    use super::{try_out_secrets, AppConfig, ScanPlatform, ScannerConfig, Secrets};
    use crate::faces::{new_key, FaceError, KeyStore};
    use crate::model::{CameraChoice, CaptureFrom, MicrophoneProblem, OnFrame, OnSound, Platform, SourceProblem, VirtualCamera};
    use crate::scanner::{Announce, NetworkAccess};
    use gc_camera::Output;
    use gc_win::{Announced, Announcement, CameraKeeper, CameraOutput, Capture, CaptureSource, Credential, Feed, FeedError};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// Secrets in Credential Manager, each an entry called "GodsCamera." and its name.
    struct Vault;

    impl Vault {
        fn entry(name: &str) -> Credential {
            Credential::new(&format!("GodsCamera.{name}"))
        }
    }

    impl Secrets for Vault {
        fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
            Self::entry(name).read()
        }

        fn write(&self, name: &str, secret: &[u8], note: &str) -> Result<(), String> {
            Self::entry(name).write(secret, note)
        }

        fn delete(&self, name: &str) {
            Self::entry(name).delete();
        }
    }

    /// The user's own app keeps its secrets in Credential Manager. A try-out
    /// keeps them in its scratch folder, and so cannot touch the user's.
    fn secrets(data_dir: &str, try_out: bool) -> Arc<dyn Secrets> {
        match try_out {
            true => try_out_secrets(data_dir),
            false => Arc::new(Vault),
        }
    }

    /// The name the key for saved faces goes under.
    const FACE_KEY: &str = "FaceStore";

    struct FaceKey(Arc<dyn Secrets>);

    impl KeyStore for FaceKey {
        fn load_or_create(&self) -> Result<[u8; 32], FaceError> {
            // An entry of another length is not a key of ours: a new one replaces it.
            if let Some(key) = self.0.read(FACE_KEY).map_err(FaceError::Key)?.and_then(|secret| <[u8; 32]>::try_from(secret).ok()) {
                return Ok(key);
            }
            let key = new_key();
            self.0.write(FACE_KEY, &key, "The key that encrypts the faces saved in God's Camera.").map_err(FaceError::Key)?;
            Ok(key)
        }

        fn delete(&self) {
            self.0.delete(FACE_KEY);
        }
    }

    /// The virtual camera as it stands: held if Windows made it, and what to tell the user.
    struct Held {
        /// Holds the camera in every app's list for as long as the app runs.
        camera: Option<CameraKeeper>,
        state: (VirtualCamera, String),
    }

    struct Windows {
        /// The pipe the camera's media source fetches pictures from, if this app could make it.
        feed: Option<Arc<Feed>>,
        held: Mutex<Held>,
        secrets: Arc<dyn Secrets>,
    }

    impl Windows {
        /// Ask Windows for the camera, and say how that went.
        fn ask(feed: &Feed) -> Held {
            match CameraKeeper::start(feed.pipe_name()) {
                Ok(camera) => Held { camera: Some(camera), state: (VirtualCamera::Ready, String::new()) },
                Err(error) if error.not_installed() => Held { camera: None, state: (VirtualCamera::NotInstalled, error.to_string()) },
                Err(error) if error.refused() => Held { camera: None, state: (VirtualCamera::Refused, error.to_string()) },
                Err(error) => Held { camera: None, state: (VirtualCamera::Failed, error.to_string()) },
            }
        }

        fn held(&self) -> std::sync::MutexGuard<'_, Held> {
            self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    impl Platform for Windows {
        fn output(&self) -> Option<Box<dyn Output>> {
            self.feed.as_ref().map(|feed| Box::new(CameraOutput::new(Arc::clone(feed))) as Box<dyn Output>)
        }

        fn clock(&self) -> u64 {
            CameraOutput::clock()
        }

        fn virtual_camera(&self) -> (VirtualCamera, String) {
            self.held().state.clone()
        }

        fn watched(&self) -> bool {
            self.feed.as_ref().is_some_and(|feed| feed.watched().is_some())
        }

        fn retry_virtual_camera(&self) {
            let Some(feed) = &self.feed else { return };
            let mut held = self.held();
            // Let the old camera go first: Windows keeps one of a name for one app.
            held.camera = None;
            *held = Self::ask(feed);
        }

        fn key_store(&self) -> Box<dyn KeyStore> {
            Box::new(FaceKey(Arc::clone(&self.secrets)))
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

        fn listen(&self, mut on_sound: OnSound) -> Result<Box<dyn Send>, MicrophoneProblem> {
            match gc_win::listen(move |samples, seconds| on_sound(samples, seconds)) {
                Ok(listener) => Ok(Box::new(listener)),
                Err(gc_win::MicrophoneError::NoMicrophone) => Err(MicrophoneProblem::NoMicrophone),
                Err(gc_win::MicrophoneError::Refused) => Err(MicrophoneProblem::Refused),
                Err(gc_win::MicrophoneError::Failed(_)) => Err(MicrophoneProblem::Failed),
            }
        }
    }

    /// Make the pipe and ask Windows for the camera. Neither failing stops
    /// the app: it then runs with a preview and says why other apps see no camera.
    pub fn native(config: &AppConfig) -> Result<Arc<dyn Platform>, String> {
        let secrets = secrets(&config.data_dir, config.try_out);
        let without = |state: VirtualCamera, detail: String, secrets: Arc<dyn Secrets>| -> Result<Arc<dyn Platform>, String> {
            Ok(Arc::new(Windows { feed: None, held: Mutex::new(Held { camera: None, state: (state, detail) }), secrets }))
        };
        let feed = match Feed::open() {
            Ok(feed) => Arc::new(feed),
            Err(FeedError::Taken) => return without(VirtualCamera::Busy, String::new(), secrets),
            Err(error) => return without(VirtualCamera::Failed, error.to_string(), secrets),
        };
        let held = Windows::ask(&feed);
        Ok(Arc::new(Windows { feed: Some(feed), held: Mutex::new(held), secrets }))
    }

    /// The hub's announcement, as Windows makes it.
    struct Told(Announcement);

    impl Announce for Told {
        fn up(&self) -> Option<bool> {
            match self.0.state() {
                Announced::Starting => None,
                Announced::Up => Some(true),
                Announced::Failed(_) | Announced::Down => Some(false),
            }
        }
    }

    struct ScanWindows {
        secrets: Arc<dyn Secrets>,
    }

    impl ScanPlatform for ScanWindows {
        fn secrets(&self) -> Arc<dyn Secrets> {
            Arc::clone(&self.secrets)
        }

        fn computer_name(&self) -> String {
            gc_win::computer_name()
        }

        fn network_access(&self, program: &Path, port: u16) -> NetworkAccess {
            // When Windows will not say, the hub listens and Windows decides for itself.
            let Ok(answer) = gc_win::firewall(program, port) else { return NetworkAccess::Allowed };
            if !answer.ready() {
                NetworkAccess::RuleNeeded
            } else if answer.on_public_network && !answer.on_private_network {
                NetworkAccess::PublicOnly
            } else {
                NetworkAccess::Allowed
            }
        }

        fn announce(&self, name: &str, port: u16, fingerprint: &str) -> Option<Box<dyn Announce>> {
            Announcement::start(name, port, fingerprint).ok().map(|announcement| Box::new(Told(announcement)) as Box<dyn Announce>)
        }

        fn addresses(&self) -> Vec<String> {
            gc_win::addresses().iter().map(ToString::to_string).collect()
        }
    }

    pub fn scan_platform(config: &ScannerConfig) -> Arc<dyn ScanPlatform> {
        Arc::new(ScanWindows { secrets: secrets(&config.data_dir, config.try_out) })
    }
}
