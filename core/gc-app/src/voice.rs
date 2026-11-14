//! Going back to live when the user starts to speak.
//!
//! While a loop plays, the user is usually looking away, and when they speak
//! they are back in the call: the picture should be live again at once. With
//! the option on, the app listens to the microphone while a loop plays and
//! ends the loop when speech has lasted a quarter of a second. Only the
//! loudness of each short stretch of sound is looked at; nothing is recorded
//! or kept. The rule is the Mac app's (`VoiceTrigger.swift`).

/// Decides from a run of sound levels when someone has started to speak.
#[derive(Debug, Clone)]
pub struct VoiceActivity {
    /// The level a sound must reach to count as speech, in decibels below full scale.
    pub threshold_db: f32,
    /// How long the sound must last, so that a click or a cough does not count.
    pub hold_seconds: f64,
    heard: f64,
}

impl Default for VoiceActivity {
    fn default() -> Self {
        Self { threshold_db: -32.0, hold_seconds: 0.25, heard: 0.0 }
    }
}

impl VoiceActivity {
    /// Feed the level of the next stretch of sound. True once speech has lasted long enough.
    pub fn feed(&mut self, level_db: f32, seconds: f64) -> bool {
        if level_db >= self.threshold_db {
            self.heard += seconds;
        } else {
            // Quiet counts double, so scattered noises do not add up to speech.
            self.heard = (self.heard - seconds * 2.0).max(0.0);
        }
        self.heard >= self.hold_seconds
    }

    pub fn reset(&mut self) {
        self.heard = 0.0;
    }
}

/// The level of a block of samples between -1 and 1, in decibels below full scale.
pub fn level_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let power = samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    let rms = power.sqrt();
    if rms > 0.0 {
        (20.0 * rms.log10()).max(-120.0)
    } else {
        -120.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone of the given loudness, `seconds` long at 48 000 samples a second.
    fn tone(amplitude: f32, seconds: f64) -> Vec<f32> {
        (0..(48_000.0 * seconds) as usize).map(|n| amplitude * (n as f32 * 0.05).sin()).collect()
    }

    #[test]
    fn levels_are_in_decibels_below_full_scale() {
        assert_eq!(level_db(&[]), -120.0);
        assert_eq!(level_db(&[0.0; 480]), -120.0);
        // A sine at full scale has a mean square of a half: 3 dB down.
        assert!((level_db(&tone(1.0, 0.1)) + 3.01).abs() < 0.1);
        assert!((level_db(&tone(0.1, 0.1)) + 23.01).abs() < 0.1);
    }

    #[test]
    fn speech_counts_once_it_lasts_a_quarter_of_a_second() {
        let mut voice = VoiceActivity::default();
        let speech = level_db(&tone(0.2, 0.02));
        // Twelve blocks of 20 ms are 0.24 s: not yet.
        for _ in 0..12 {
            assert!(!voice.feed(speech, 0.02));
        }
        assert!(voice.feed(speech, 0.02));
    }

    #[test]
    fn quiet_and_short_noises_do_not_add_up_to_speech() {
        let mut voice = VoiceActivity::default();
        let (noise, quiet) = (level_db(&tone(0.2, 0.02)), level_db(&tone(0.005, 0.02)));
        // A click of 40 ms, then 60 ms of quiet, again and again for two seconds.
        for _ in 0..20 {
            assert!(!voice.feed(noise, 0.02) && !voice.feed(noise, 0.02));
            for _ in 0..3 {
                assert!(!voice.feed(quiet, 0.02));
            }
        }
        // A room's hum below the threshold never counts.
        voice.reset();
        assert!((0..200).all(|_| !voice.feed(level_db(&tone(0.02, 0.02)), 0.02)));
    }
}
