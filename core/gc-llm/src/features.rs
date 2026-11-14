//! Which features may send the user's data to a cloud provider.
//!
//! Section 6.11 of the plan: cloud use is a switch per feature, off until the
//! user turns it on, with a notice of what leaves the device. Until then a
//! feature may only use a model on the user's own computers, and the gateway
//! refuses a cloud provider for it before anything is sent.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::LlmError;

/// The features that can use a language or image model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Fixing what text recognition got wrong in a scanned document.
    CleanUp,
    Summary,
    Translation,
    /// Filling a form of fields from a document.
    Extraction,
    /// Answering questions about a document.
    Questions,
    /// Reading pages the text reader could not, by sending their pictures.
    HardPages,
    /// Pictures of the user wearing a garment.
    TryOn,
    /// Naming and describing an object scanned in 3D.
    NameScan,
}

impl Feature {
    pub const ALL: [Feature; 8] = [
        Feature::CleanUp,
        Feature::Summary,
        Feature::Translation,
        Feature::Extraction,
        Feature::Questions,
        Feature::HardPages,
        Feature::TryOn,
        Feature::NameScan,
    ];

    /// The name the settings file uses.
    pub fn id(self) -> &'static str {
        match self {
            Feature::CleanUp => "clean_up",
            Feature::Summary => "summary",
            Feature::Translation => "translation",
            Feature::Extraction => "extraction",
            Feature::Questions => "questions",
            Feature::HardPages => "hard_pages",
            Feature::TryOn => "try_on",
            Feature::NameScan => "name_scan",
        }
    }

    /// The feature's name in plain English. The apps have their own, in both languages.
    pub fn name(self) -> &'static str {
        match self {
            Feature::CleanUp => "Document clean-up",
            Feature::Summary => "Summaries",
            Feature::Translation => "Translation",
            Feature::Extraction => "Filling in fields from a document",
            Feature::Questions => "Questions about a document",
            Feature::HardPages => "Reading hard pages",
            Feature::TryOn => "Try-on",
            Feature::NameScan => "Naming a 3D scan",
        }
    }

    /// What leaves the device when the feature uses a cloud provider: the
    /// notice the settings screen shows beside the switch.
    pub fn notice(self) -> &'static str {
        match self {
            Feature::CleanUp | Feature::Summary | Feature::Translation | Feature::Extraction => {
                "The document's text is sent to the provider."
            }
            Feature::Questions => "The document's text and your question are sent to the provider.",
            Feature::HardPages => "Pictures of the pages are sent to the provider.",
            Feature::TryOn => "Your photo and the photos of the clothes are sent to the provider.",
            Feature::NameScan => "Pictures of the scanned object are sent to the provider.",
        }
    }

    fn from_id(id: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|feature| feature.id() == id)
    }
}

/// The switches: which features may use a cloud provider. A feature not
/// listed stays on the user's devices, so a new switch starts off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudSwitches {
    allowed: BTreeSet<Feature>,
}

/// The file as it is written: the features allowed, by name, so that a name a
/// later version adds does not stop an earlier one from reading the rest.
#[derive(Serialize, Deserialize)]
struct SwitchFile {
    version: u32,
    cloud_allowed: Vec<String>,
}

impl CloudSwitches {
    pub const FILE: &'static str = "cloud-features.json";

    /// The switches kept in `folder`. A missing file means every feature
    /// stays on the user's devices, and so does a file that cannot be read:
    /// a mistake there must never send data out.
    pub fn load(folder: &Path) -> CloudSwitches {
        let Ok(bytes) = std::fs::read(folder.join(Self::FILE)) else { return CloudSwitches::default() };
        let Ok(file) = serde_json::from_slice::<SwitchFile>(&bytes) else { return CloudSwitches::default() };
        CloudSwitches { allowed: file.cloud_allowed.iter().filter_map(|id| Feature::from_id(id)).collect() }
    }

    pub fn save(&self, folder: &Path) -> Result<(), LlmError> {
        let file = SwitchFile { version: 1, cloud_allowed: self.allowed.iter().map(|feature| feature.id().to_string()).collect() };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|error| LlmError::Storage { message: error.to_string() })?;
        crate::meter::write_atomically(&folder.join(Self::FILE), &bytes)
    }

    pub fn allowed(&self, feature: Feature) -> bool {
        self.allowed.contains(&feature)
    }

    pub fn set(&mut self, feature: Feature, allowed: bool) {
        if allowed {
            self.allowed.insert(feature);
        } else {
            self.allowed.remove(&feature);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("gc-llm-switch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn every_feature_starts_on_the_device() {
        let folder = scratch("start");
        let switches = CloudSwitches::load(&folder);
        assert!(Feature::ALL.iter().all(|feature| !switches.allowed(*feature)));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn switches_are_kept_between_runs() {
        let folder = scratch("kept");
        let mut switches = CloudSwitches::load(&folder);
        switches.set(Feature::Summary, true);
        switches.set(Feature::TryOn, true);
        switches.set(Feature::TryOn, false);
        switches.save(&folder).unwrap();
        let again = CloudSwitches::load(&folder);
        assert!(again.allowed(Feature::Summary));
        assert!(!again.allowed(Feature::TryOn));
        assert!(!again.allowed(Feature::CleanUp));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_damaged_file_or_an_unknown_name_sends_nothing_out() {
        let folder = scratch("damaged");
        std::fs::write(folder.join(CloudSwitches::FILE), b"{ not json").unwrap();
        assert_eq!(CloudSwitches::load(&folder), CloudSwitches::default());
        std::fs::write(folder.join(CloudSwitches::FILE), br#"{"version": 2, "cloud_allowed": ["summary", "time_travel"]}"#).unwrap();
        let switches = CloudSwitches::load(&folder);
        assert!(switches.allowed(Feature::Summary));
        assert_eq!(Feature::ALL.iter().filter(|feature| switches.allowed(**feature)).count(), 1);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn every_feature_has_a_name_and_a_notice() {
        for feature in Feature::ALL {
            assert_eq!(Feature::from_id(feature.id()), Some(feature));
            assert!(!feature.name().is_empty() && feature.notice().ends_with('.'));
        }
    }
}
