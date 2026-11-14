//! What the app remembers between runs: one small file in its data folder.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Where the picture comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    #[default]
    Camera,
    Image,
    Video,
    /// Colour bars with a moving block.
    Pattern,
}

/// How a loop repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "camelCase")]
pub enum LoopStyle {
    /// Start to end when the loop ends as it began, otherwise back and forth.
    #[default]
    Automatic,
    Forward,
    PingPong,
}

/// Which swap model draws the face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    #[default]
    Standard,
    Sharper,
}

/// Everything the app remembers. A field a newer version adds takes its
/// default when an older file is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: SourceKind,
    /// The user has said that the app may turn the camera on. Until then it stays off.
    pub camera_allowed: bool,
    /// The camera last chosen, by what the system opens it by.
    pub camera: Option<String>,
    /// The picture file last chosen.
    pub file: Option<String>,
    /// The video file last chosen.
    pub video: Option<String>,
    pub mirror: bool,
    pub loop_style: LoopStyle,
    pub show_loop_badge: bool,
    pub swap_wanted: bool,
    pub quality: Quality,
    pub selected_face: Option<String>,
    /// When the user accepted the rules for swapping a face, in seconds since 1970.
    pub policy_accepted: Option<u64>,
    pub show_ai_badge: bool,
    pub colour_match: bool,
    pub sharpen: f32,
    pub keep_objects_in_front: bool,
    pub real_mouth: bool,
    pub blur_when_unswapped: bool,
    /// Closing the window leaves the app running, so other apps keep their camera.
    pub keep_running: bool,
    /// While a loop plays, listen to the microphone and go back to live when the user speaks.
    pub back_to_live_on_voice: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: SourceKind::Camera,
            camera_allowed: false,
            camera: None,
            file: None,
            video: None,
            mirror: false,
            loop_style: LoopStyle::Automatic,
            show_loop_badge: false,
            swap_wanted: false,
            quality: Quality::Standard,
            selected_face: None,
            policy_accepted: None,
            // On unless the user turns it off.
            show_ai_badge: true,
            colour_match: true,
            sharpen: gc_live::LiveSettings::default().sharpen,
            keep_objects_in_front: true,
            real_mouth: false,
            blur_when_unswapped: false,
            keep_running: true,
            // The microphone is opened only when the user asks for this.
            back_to_live_on_voice: false,
        }
    }
}

impl Settings {
    /// The settings in `path`, or the defaults if there is no readable file.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// Write the settings, replacing the file in one step.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let partial = path.with_extension("partial");
        std::fs::write(&partial, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&partial, path)
    }

    /// The settings the face swap itself understands.
    pub fn live(&self) -> gc_live::LiveSettings {
        gc_live::LiveSettings {
            colour_match: self.colour_match,
            sharpen: self.sharpen.clamp(0.0, 1.0),
            occlusion: self.keep_objects_in_front,
            real_mouth: self.real_mouth,
            blur_when_unswapped: self.blur_when_unswapped,
            ..gc_live::LiveSettings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_restart_and_an_older_file_gets_defaults() {
        let folder = std::env::temp_dir().join(format!("gc-app-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let path = folder.join("deep").join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default(), "no file yet");
        assert!(Settings::default().show_ai_badge, "the AI badge is on unless the user turns it off");

        let changed = Settings {
            source: SourceKind::Pattern,
            mirror: true,
            loop_style: LoopStyle::PingPong,
            quality: Quality::Sharper,
            selected_face: Some("abc".into()),
            policy_accepted: Some(1_800_000_000),
            sharpen: 0.9,
            ..Settings::default()
        };
        changed.save(&path).unwrap();
        assert_eq!(Settings::load(&path), changed);

        // A file from a version that knew fewer settings, and one that is not ours at all.
        std::fs::write(&path, br#"{"mirror": true, "source": "image", "somethingNew": 5}"#).unwrap();
        let older = Settings::load(&path);
        assert_eq!((older.mirror, older.source, older.show_ai_badge), (true, SourceKind::Image, true));
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_swap_gets_the_settings_the_user_chose() {
        let settings = Settings { sharpen: 7.0, real_mouth: true, keep_objects_in_front: false, ..Settings::default() };
        let live = settings.live();
        assert_eq!((live.sharpen, live.real_mouth, live.occlusion, live.colour_match), (1.0, true, false, true));
    }
}
