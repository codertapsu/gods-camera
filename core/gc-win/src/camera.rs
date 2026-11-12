//! The camera itself, as Windows shows it in every app's camera list.
//!
//! Windows 11 makes a virtual camera on request. It loads the media source,
//! which the installer registered for the whole machine, into its camera
//! service, and the camera stays in the list for as long as the app that asked
//! is running.

use crate::protocol;
use std::fmt;
use windows::core::{GUID, HRESULT, HSTRING};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFShutdown, MFStartup, MFVirtualCameraAccess_CurrentUser,
    MFVirtualCameraLifetime_Session, MFVirtualCameraType_SoftwareCameraSource, MFSTARTUP_FULL, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Why Windows did not make the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraError {
    /// The call that failed.
    pub step: &'static str,
    /// Windows' own code for the failure.
    pub code: i32,
}

impl CameraError {
    /// The media source is not registered on this computer, or Windows cannot
    /// load its file: the camera has not been installed.
    pub fn not_installed(&self) -> bool {
        matches!(
            self.code as u32,
            0x8004_0154 // REGDB_E_CLASSNOTREG
            | 0x8007_007E // ERROR_MOD_NOT_FOUND
            | 0x8007_0002 // ERROR_FILE_NOT_FOUND
            | 0x8007_0490 // ERROR_NOT_FOUND
        )
    }

    /// Windows' camera privacy settings refused.
    pub fn refused(&self) -> bool {
        self.code as u32 == 0x8007_0005 // E_ACCESSDENIED
    }
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let why = if self.not_installed() {
            "the camera is not installed on this computer"
        } else if self.refused() {
            "Windows refused; camera access for desktop apps must be on in Settings > Privacy & security > Camera"
        } else {
            "Windows could not make the camera"
        };
        write!(f, "{why} ({} failed with {:#010x})", self.step, self.code)
    }
}

impl std::error::Error for CameraError {}

fn failed(step: &'static str) -> impl Fn(windows::core::Error) -> CameraError {
    move |error| CameraError { step, code: error.code().0 }
}

/// Media Foundation, started for as long as this is held.
struct MediaFoundation;

impl MediaFoundation {
    fn start() -> Result<Self, CameraError> {
        // The thread may already belong to COM in another way; that is fine,
        // and nothing is undone when this is dropped.
        let _: HRESULT = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(failed("MFStartup"))?;
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

/// The camera, in every app's list for as long as this is held.
pub struct VirtualCamera {
    // Dropped first, while Media Foundation is still running.
    device: IMFVirtualCamera,
    _media_foundation: MediaFoundation,
}

impl VirtualCamera {
    /// Ask Windows for the camera, and tell its media source which pipe to
    /// fetch pictures from: the name of the app's [`crate::Feed`].
    pub fn start(pipe_name: &str) -> Result<Self, CameraError> {
        let media_foundation = MediaFoundation::start()?;
        let device = unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &HSTRING::from(protocol::FRIENDLY_NAME),
                &HSTRING::from(protocol::CLSID),
                None,
            )
        }
        .map_err(failed("MFCreateVirtualCamera"))?;
        // The media source runs in the camera service, as another account.
        // This tells it whose pipe to fetch pictures from.
        let key = GUID::from_u128(protocol::ATTRIBUTE_PIPE_NAME);
        unsafe { device.SetString(&key, &HSTRING::from(pipe_name)) }.map_err(failed("IMFVirtualCamera::SetString"))?;
        if let Err(error) = unsafe { device.Start(None) } {
            // Windows may have listed the camera before it failed to start it.
            unsafe {
                let _ = device.Remove();
                let _ = device.Shutdown();
            }
            return Err(failed("IMFVirtualCamera::Start")(error));
        }
        Ok(Self { device, _media_foundation: media_foundation })
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        // Take the camera out of the list at once instead of when the process ends.
        unsafe {
            let _ = self.device.Remove();
            let _ = self.device.Shutdown();
        }
    }
}
