//! The app's camera as a paired phone or tablet works it through the hub.
//!
//! The hub serves the routes (`gc_link::remote`); this answers them from the
//! app's own model, as the window's controls would. What goes wrong is said
//! as a short code, which the device shows in its own language:
//!
//! | Code | Meaning |
//! |---|---|
//! | `accept_rules_on_computer` | the swap was never turned on here, so its rules were never accepted |
//! | `add_a_face_first` | no face is saved on the computer |
//! | `no_such_face` | the face asked for is not there any more |
//! | `cannot_record_now` | a recording can start only while the picture is live |
//! | `no_loop_yet` | there is no loop to play |
//!
//! [`RemoteState::swap_problem`] uses these codes: `getting_ready`,
//! `model_missing`, `failed`, `no_face_seen` and `not_on_face`.

use std::sync::{Arc, Weak};

use gc_link::{CameraRemote, RemoteAction, RemoteFace, RemoteState};

use crate::model::{DesktopApp, EngineMode, SwapState};

/// The longest side of the picture a device is sent: enough for a phone's
/// screen, small enough to send several times a second.
const PICTURE_SIDE: u32 = 640;

/// `app`'s camera, for the hub to hand to paired devices. It does not keep the app alive.
pub fn camera(app: &Arc<DesktopApp>) -> Arc<dyn CameraRemote> {
    Arc::new(AppCamera(Arc::downgrade(app)))
}

/// The app, as the hub sees it.
struct AppCamera(Weak<DesktopApp>);

impl CameraRemote for AppCamera {
    fn state(&self) -> RemoteState {
        let Some(app) = self.0.upgrade() else {
            return RemoteState {
                mode: "idle".into(),
                recording: false,
                recorded_seconds: 0.0,
                can_record: false,
                can_loop: false,
                swap_wanted: false,
                swapping: false,
                swap_problem: None,
                selected_face: None,
                watched: false,
            };
        };
        let state = app.state();
        let swap_problem = match (state.swap_wanted, state.swap_state) {
            (false, _) => None,
            (true, SwapState::Loading) => Some("getting_ready"),
            (true, SwapState::Missing) => Some("model_missing"),
            (true, SwapState::Failed) => Some("failed"),
            (true, _) if state.swap_error.is_some() => Some("failed"),
            (true, _) if state.swapping => None,
            (true, _) if state.picture_hidden => Some("not_on_face"),
            (true, _) if state.mode == EngineMode::Live && !state.face_found => Some("no_face_seen"),
            (true, _) => None,
        };
        RemoteState {
            mode: match state.mode {
                EngineMode::Live => "live",
                EngineMode::Looping => "looping",
                EngineMode::Idle => "idle",
            }
            .into(),
            recording: state.is_recording,
            recorded_seconds: state.recorded_seconds,
            can_record: state.can_record,
            can_loop: state.can_loop,
            swap_wanted: state.swap_wanted,
            swapping: state.swapping,
            swap_problem: swap_problem.map(str::to_string),
            selected_face: state.selected_face,
            watched: state.watched,
        }
    }

    fn faces(&self) -> Vec<RemoteFace> {
        let Some(app) = self.0.upgrade() else { return Vec::new() };
        app.state().faces.into_iter().map(|face| RemoteFace { id: face.id, name: face.name }).collect()
    }

    fn face_picture(&self, id: &str) -> Option<Vec<u8>> {
        let app = self.0.upgrade()?;
        let png = app.face_picture(id.to_string(), None);
        (!png.is_empty()).then_some(png)
    }

    fn picture(&self) -> Option<Vec<u8>> {
        let app = self.0.upgrade()?;
        let mut bgra = Vec::new();
        let frame = app.preview(&mut bgra)?;
        jpeg(&bgra, frame.width, frame.height)
    }

    fn act(&self, action: RemoteAction) -> Result<(), String> {
        let app = self.0.upgrade().ok_or("cannot_record_now")?;
        let state = app.state();
        match action {
            RemoteAction::Swap { on } => {
                if on && state.faces.is_empty() {
                    return Err("add_a_face_first".into());
                }
                // The rules are shown and accepted on the computer, by the person in front of it.
                if !app.set_swap_wanted(on) {
                    return Err("accept_rules_on_computer".into());
                }
            }
            RemoteAction::Face { id } => {
                if !state.faces.iter().any(|face| face.id == id) {
                    return Err("no_such_face".into());
                }
                app.select_face(id);
            }
            RemoteAction::Record => {
                if !state.can_record && !state.is_recording {
                    return Err("cannot_record_now".into());
                }
                app.toggle_recording();
            }
            RemoteAction::Loop => {
                if !state.can_loop {
                    return Err("no_loop_yet".into());
                }
                app.toggle_loop();
            }
        }
        Ok(())
    }
}

/// The picture as a JPEG no larger than [`PICTURE_SIDE`] on its longer side.
/// `bgra` is four bytes a pixel: blue, green, red, unused.
fn jpeg(bgra: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || bgra.len() < (width * height * 4) as usize {
        return None;
    }
    let scale = (PICTURE_SIDE as f32 / width.max(height) as f32).min(1.0);
    let (out_width, out_height) = (((width as f32 * scale) as u32).max(1), ((height as f32 * scale) as u32).max(1));
    // Each pixel of the small picture is the mean of the block of the large one it covers.
    let mut rgb = Vec::with_capacity((out_width * out_height * 3) as usize);
    for y in 0..out_height {
        let (top, bottom) = (y * height / out_height, ((y + 1) * height / out_height).max(y * height / out_height + 1));
        for x in 0..out_width {
            let (left, right) = (x * width / out_width, ((x + 1) * width / out_width).max(x * width / out_width + 1));
            let mut sum = [0u32; 3];
            for row in top..bottom.min(height) {
                for column in left..right.min(width) {
                    let at = ((row * width + column) * 4) as usize;
                    sum[0] += bgra[at + 2] as u32;
                    sum[1] += bgra[at + 1] as u32;
                    sum[2] += bgra[at] as u32;
                }
            }
            let count = ((bottom.min(height) - top) * (right.min(width) - left)).max(1);
            rgb.extend(sum.map(|value| (value / count) as u8));
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 72)
        .encode(&rgb, out_width, out_height, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_made_small_and_keeps_its_colours() {
        // A 1280 by 720 picture: the left half red, the right half blue.
        let (width, height) = (1280u32, 720u32);
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(if x < width / 2 { &[0, 0, 255, 255] } else { &[255, 0, 0, 255] });
            }
        }
        let jpeg = jpeg(&bgra, width, height).unwrap();
        let picture = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(picture.dimensions(), (640, 360));
        let (left, right) = (picture.get_pixel(100, 180).0, picture.get_pixel(540, 180).0);
        assert!(left[0] > 200 && left[2] < 60, "{left:?}");
        assert!(right[2] > 200 && right[0] < 60, "{right:?}");
        assert!(jpeg.len() < 40_000, "{} bytes", jpeg.len());
        // Nothing to send without a picture.
        assert!(super::jpeg(&[], 0, 0).is_none());
        assert!(super::jpeg(&[0; 16], 1280, 720).is_none());
    }
}
