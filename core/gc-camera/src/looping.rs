//! Where a recorded clip repeats, and in which order its frames are shown.
//!
//! The same rules as the Mac app's `Looping.swift`, so a loop behaves alike on
//! both desktops. Change the two together.

use crate::thumbnail::Thumbnail;
use serde::{Deserialize, Serialize};

/// How a clip repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopMode {
    /// Start to end, then a cross-fade back to the start.
    Forward,
    /// Start to end, then backwards to the start, and so on.
    PingPong,
}

/// The part of a clip that repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopPoints {
    /// First frame of the loop.
    pub start: usize,
    /// One past the last frame of the loop.
    pub end: usize,
    /// Frames over which the end fades into the lead-in before `start`.
    pub crossfade: usize,
}

impl LoopPoints {
    pub fn length(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// The cross-fade that fits: it borrows the frames before `start`, and
    /// leaves at least one frame of the loop unblended.
    fn fade(&self) -> usize {
        self.crossfade.min(self.start).min(self.length().saturating_sub(1))
    }
}

/// A seam mismatch above this many 8-bit levels shows as a ghost during the
/// cross-fade: about what a head moved by a few centimetres produces.
pub const VISIBLE_SEAM: f32 = 4.0;

/// Mean difference between the frames that the cross-fade at the end of the
/// loop blends: each of the last frames against its partner before `start`.
pub fn seam_mismatch(thumbnails: &[Thumbnail], points: &LoopPoints) -> f32 {
    let count = thumbnails.len();
    if count <= 1 || points.end > count || points.start >= points.end {
        return 0.0;
    }
    let fade = points.fade();
    if fade == 0 {
        return thumbnails[points.end - 1].difference(&thumbnails[points.start]);
    }
    let sum: f32 = (1..=fade).map(|back| thumbnails[points.end - back].difference(&thumbnails[points.start - back])).sum();
    sum / fade as f32
}

/// The way of repeating that hides the seam best. A clip whose end matches its
/// start plays start to end; one where the person has moved in between plays
/// back and forth, which has no jump at all.
pub fn suggested_mode(thumbnails: &[Thumbnail], points: &LoopPoints) -> LoopMode {
    match seam_mismatch(thumbnails, points) <= VISIBLE_SEAM {
        true => LoopMode::Forward,
        false => LoopMode::PingPong,
    }
}

/// Find where a clip should loop: nearly all of it, with the jump from the end
/// back to the start placed where it is hardest to see.
///
/// Someone who records five seconds expects about five seconds back, so the
/// start is looked for only in the first sixth of the clip and the end only in
/// the last sixth. Within those, the pair of moments that look most alike and
/// are quiet wins. This also trims the moment after pressing Record and the
/// reach for Stop.
///
/// `minimum_length` is the shortest acceptable loop, in frames. `crossfade` is
/// the frames of cross-fade wanted; the loop starts at least this far into the
/// clip, because the fade borrows the frames before the start.
pub fn find(thumbnails: &[Thumbnail], minimum_length: usize, crossfade: usize) -> LoopPoints {
    let count = thumbnails.len();
    let fade = crossfade.min(count / 4);
    let shortest = minimum_length.max(2);
    // Too short to choose: loop everything that is left after the lead-in.
    if count < fade + shortest + 1 {
        let lead_in = fade.min(count.saturating_sub(1));
        return LoopPoints { start: lead_in, end: count, crossfade: lead_in };
    }

    // How much each frame differs from the one before it.
    let mut motion = vec![0f32; count];
    for i in 1..count {
        motion[i] = thumbnails[i].difference(&thumbnails[i - 1]);
    }

    let window = (count / 6).max(1);
    let mut seam_offsets = match fade {
        0 => vec![1],
        _ => vec![1, (fade / 2).max(1), fade],
    };
    seam_offsets.sort_unstable();
    seam_offsets.dedup();
    let mut best = LoopPoints { start: fade, end: count, crossfade: fade };
    let mut best_cost = f32::INFINITY;
    let last_start = fade.max((fade + window).min(count - shortest));
    for start in fade..=last_start {
        let first_end = (start + shortest).max(count - window);
        for end in first_end..=count {
            // The frames leading up to the end must look like the frames
            // leading up to the start, because those are the ones the
            // cross-fade blends.
            let (mut seam, mut compared) = (0f32, 0u32);
            for &back in seam_offsets.iter().filter(|&&back| back <= start) {
                seam += thumbnails[end - back].difference(&thumbnails[start - back]);
                compared += 1;
            }
            let seam = match compared {
                0 => thumbnails[end - 1].difference(&thumbnails[start]),
                _ => seam / compared as f32,
            };
            // Prefer quiet moments, where a small mismatch is least visible,
            // and, between equals, the longer loop.
            let stillness = motion[start] + motion[end - 1];
            let length_bonus = (end - start) as f32 / count as f32;
            let cost = seam + 0.5 * stillness - 1.5 * length_bonus;
            if cost < best_cost {
                best_cost = cost;
                best = LoopPoints { start, end, crossfade: fade };
            }
        }
    }
    best
}

/// One output frame of a loop: a clip frame, optionally blended with a second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopStep {
    pub primary: usize,
    pub secondary: Option<usize>,
    /// Weight of the secondary frame, from 0 to 1.
    pub mix: f32,
}

/// Walks through a clip's frames in loop order.
#[derive(Debug, Clone)]
pub struct LoopSequencer {
    points: LoopPoints,
    mode: LoopMode,
    position: usize,
    forwards: bool,
}

impl LoopSequencer {
    /// `None` if the loop has no frames.
    pub fn new(points: LoopPoints, mode: LoopMode) -> Option<Self> {
        (points.length() >= 1).then_some(Self { points, mode, position: points.start, forwards: true })
    }

    /// The step to show now; the sequencer moves on to the one after it.
    pub fn step(&mut self) -> LoopStep {
        let points = self.points;
        let current = self.position;
        match self.mode {
            LoopMode::Forward => {
                self.position = if current + 1 >= points.end { points.start } else { current + 1 };
                // Over the last frames, fade towards the lead-in before
                // `start`, so that the frame after the fade is `start` itself.
                let fade = points.fade();
                if fade > 0 && current + fade >= points.end {
                    let into_fade = current + fade - points.end;
                    return LoopStep {
                        primary: current,
                        secondary: Some(points.start - fade + into_fade),
                        mix: (into_fade + 1) as f32 / (fade + 1) as f32,
                    };
                }
                LoopStep { primary: current, secondary: None, mix: 0.0 }
            }
            LoopMode::PingPong => {
                if points.length() > 1 {
                    let at_end = self.forwards && current + 1 >= points.end;
                    let at_start = !self.forwards && current == points.start;
                    if at_end || at_start {
                        self.forwards = !self.forwards;
                    }
                    self.position = if self.forwards { current + 1 } else { current - 1 };
                }
                LoopStep { primary: current, secondary: None, mix: 0.0 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbnail::CELLS;

    fn flat(level: u8) -> Thumbnail {
        Thumbnail::from_cells([level; CELLS])
    }

    /// Thumbnails for a scene that repeats exactly every `period` frames.
    fn periodic(count: usize, period: usize) -> Vec<Thumbnail> {
        (0..count)
            .map(|frame| {
                let phase = (frame % period) as f64 / period as f64 * std::f64::consts::TAU;
                let mut cells = [0u8; CELLS];
                for (cell, value) in cells.iter_mut().enumerate() {
                    *value = (128.0 + 100.0 * (phase + cell as f64 * 0.37).sin()) as u8;
                }
                Thumbnail::from_cells(cells)
            })
            .collect()
    }

    #[test]
    fn the_finder_picks_a_whole_number_of_periods() {
        let period = 20;
        let thumbnails = periodic(150, period);
        let found = find(&thumbnails, 30, 8);
        assert_eq!(found.length() % period, 0, "a loop of {} frames is not a whole number of periods", found.length());
        assert!(found.length() >= 30);
        assert!(found.start >= found.crossfade, "the fade needs frames before the start");
        assert!(found.end <= thumbnails.len());
    }

    #[test]
    fn the_finder_skips_a_disturbance_at_the_start_and_the_end() {
        // The first 15 and the last 10 frames flicker, as when a hand reaches
        // for the Record and Stop buttons; the frames between are perfectly still.
        let thumbnails: Vec<Thumbnail> = (0..150)
            .map(|frame| match !(15..140).contains(&frame) {
                true => flat(if frame % 2 == 0 { 40 } else { 200 }),
                false => flat(120),
            })
            .collect();
        let found = find(&thumbnails, 30, 8);
        assert!(found.start >= 15, "the loop should start after the disturbance, not at {}", found.start);
        assert!(found.end <= 140, "the loop should end before the disturbance, not at {}", found.end);
    }

    #[test]
    fn the_seam_mismatch_tells_a_matching_end_from_a_moved_one() {
        // The picture ends as it began.
        let steady: Vec<Thumbnail> = (0..60).map(|_| flat(100)).collect();
        let points = LoopPoints { start: 8, end: 60, crossfade: 8 };
        assert_eq!(seam_mismatch(&steady, &points), 0.0);
        assert_eq!(suggested_mode(&steady, &points), LoopMode::Forward);
        // The picture ends 30 levels brighter than it began.
        let moved: Vec<Thumbnail> = (0..60).map(|frame| flat(if frame < 30 { 100 } else { 130 })).collect();
        assert_eq!(seam_mismatch(&moved, &points), 30.0);
        assert_eq!(suggested_mode(&moved, &points), LoopMode::PingPong);
        // Without a cross-fade, the last frame is compared with the first.
        assert_eq!(seam_mismatch(&moved, &LoopPoints { start: 0, end: 60, crossfade: 0 }), 30.0);
    }

    #[test]
    fn the_finder_keeps_most_of_a_slowly_changing_clip() {
        // Brightness drifts steadily, as when the light or the pose changes a
        // little over five seconds. The shortest loop would hide the drift
        // best, but someone who records five seconds wants about five seconds back.
        let thumbnails: Vec<Thumbnail> = (0..150).map(|frame| flat(100 + (frame / 5) as u8)).collect();
        let found = find(&thumbnails, 30, 8);
        assert!(found.length() >= 90, "the loop keeps most of the recording, not {} frames", found.length());
        assert!(found.start <= 8 + 150 / 6);
        assert!(found.end >= 150 - 150 / 6);
    }

    #[test]
    fn the_finder_handles_clips_too_short_to_choose() {
        let thumbnails = periodic(6, 3);
        let found = find(&thumbnails, 30, 8);
        assert!(found.end <= 6);
        assert!(found.length() >= 1);
        assert!(found.crossfade <= found.start);
        // Nothing at all still gives points that can be asked about.
        let nothing = find(&[], 30, 8);
        assert_eq!((nothing.start, nothing.end, nothing.length()), (0, 0, 0));
        assert!(LoopSequencer::new(nothing, LoopMode::Forward).is_none());
    }

    #[test]
    fn a_forward_loop_fades_into_its_lead_in() {
        let points = LoopPoints { start: 10, end: 20, crossfade: 4 };
        let mut sequencer = LoopSequencer::new(points, LoopMode::Forward).unwrap();
        let steps: Vec<LoopStep> = (0..20).map(|_| sequencer.step()).collect();
        // Two passes through frames 10...19.
        let primaries: Vec<usize> = steps.iter().map(|step| step.primary).collect();
        assert_eq!(primaries, (10..20).chain(10..20).collect::<Vec<_>>());
        // No blending until the last four frames.
        assert!(steps[..6].iter().all(|step| step.secondary.is_none() && step.mix == 0.0));
        // Frames 16...19 blend with 6...9, more strongly each frame.
        assert_eq!(steps[6..10].iter().map(|step| step.secondary).collect::<Vec<_>>(), [Some(6), Some(7), Some(8), Some(9)]);
        assert_eq!(steps[6..10].iter().map(|step| step.mix).collect::<Vec<_>>(), [0.2, 0.4, 0.6, 0.8]);
        // The frame after the fade is the start itself, unblended: 9 leads into 10.
        assert_eq!(steps[10], LoopStep { primary: 10, secondary: None, mix: 0.0 });
    }

    #[test]
    fn a_forward_loop_without_room_for_a_fade_just_wraps() {
        let mut sequencer = LoopSequencer::new(LoopPoints { start: 0, end: 3, crossfade: 8 }, LoopMode::Forward).unwrap();
        let steps: Vec<LoopStep> = (0..7).map(|_| sequencer.step()).collect();
        assert_eq!(steps.iter().map(|step| step.primary).collect::<Vec<_>>(), [0, 1, 2, 0, 1, 2, 0]);
        assert!(steps.iter().all(|step| step.secondary.is_none()));
    }

    #[test]
    fn a_back_and_forth_loop_turns_at_both_ends() {
        let mut sequencer = LoopSequencer::new(LoopPoints { start: 2, end: 6, crossfade: 0 }, LoopMode::PingPong).unwrap();
        let primaries: Vec<usize> = (0..12).map(|_| sequencer.step().primary).collect();
        assert_eq!(primaries, [2, 3, 4, 5, 4, 3, 2, 3, 4, 5, 4, 3]);
    }

    #[test]
    fn a_loop_of_one_frame_stays_put() {
        for mode in [LoopMode::Forward, LoopMode::PingPong] {
            let mut sequencer = LoopSequencer::new(LoopPoints { start: 4, end: 5, crossfade: 2 }, mode).unwrap();
            assert_eq!((0..3).map(|_| sequencer.step().primary).collect::<Vec<_>>(), [4, 4, 4]);
        }
    }

    #[test]
    fn the_words_are_the_ones_the_mac_app_writes() {
        assert_eq!(serde_json::to_string(&LoopMode::PingPong).unwrap(), "\"pingPong\"");
        assert_eq!(serde_json::to_string(&LoopMode::Forward).unwrap(), "\"forward\"");
        let points = LoopPoints { start: 8, end: 140, crossfade: 8 };
        assert_eq!(serde_json::to_string(&points).unwrap(), r#"{"start":8,"end":140,"crossfade":8}"#);
    }
}
