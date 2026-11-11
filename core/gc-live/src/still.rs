//! Face swap on one still picture, on the CPU path the tests check against
//! InsightFace. The command-line tool uses it, and so do previews in the app.

use std::path::Path;

use gc_infer::image::{self, ImageMut, ImageView, MaskShape};
use gc_infer::runtime::{Accelerator, InferError, Runtime, Swapper};
use gc_infer::{Detection, SwapperContract};

use crate::engine::{Enroller, SwapModel};

/// Which faces in the picture get the new identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Targets {
    All,
    /// Only the largest face.
    Largest,
}

pub struct StillSwapper {
    enroller: Enroller,
    swapper: Swapper,
}

impl StillSwapper {
    pub fn new(packs: &Path, model: SwapModel, accelerator: Accelerator) -> Result<Self, InferError> {
        let runtime = Runtime::shared(None)?;
        let swapper = match model {
            SwapModel::Inswapper128 => Swapper::load(
                runtime,
                &packs.join("inswapper_128.onnx"),
                SwapperContract::INSWAPPER_128,
                Some(&packs.join("inswapper_128.emap.bin")),
                accelerator,
            )?,
            SwapModel::Hyperswap256 => Swapper::load(
                runtime,
                &packs.join("hyperswap_1b_256.onnx"),
                SwapperContract::HYPERSWAP_256,
                None,
                accelerator,
            )?,
        };
        Ok(Self { enroller: Enroller::new(packs, None)?, swapper })
    }

    pub fn enroller(&mut self) -> &mut Enroller {
        &mut self.enroller
    }

    /// Replace faces in `picture` with the face `identity` describes.
    /// Returns how many faces were swapped.
    pub fn swap(
        &mut self,
        picture: &mut ImageMut<'_>,
        identity: &[f32],
        targets: Targets,
        mask: MaskShape,
        colour_match: bool,
    ) -> Result<usize, InferError> {
        let size = self.swapper.contract().crop_size;
        let input = self.swapper.identity(identity)?;
        let shape = image::crop_mask(size, mask);
        // Detection and cropping read the picture as it was; pasting changes a copy
        // of nothing but the faces, so one face's patch never feeds another's crop.
        let original = picture.data.to_vec();
        let view = ImageView::new(&original, picture.width, picture.height, picture.stride, picture.format)
            .ok_or_else(|| InferError::Model("the picture's size does not match its data".into()))?;
        let mut faces: Vec<Detection> = self.enroller.detect(&view)?;
        if targets == Targets::Largest {
            let area = |face: &Detection| (face.bbox[2] - face.bbox[0]) * (face.bbox[3] - face.bbox[1]);
            faces.sort_by(|a, b| area(b).total_cmp(&area(a)));
            faces.truncate(1);
        }
        for face in &faces {
            let frame_to_crop =
                gc_align::estimate_norm(&face.kps, size).map_err(|e| InferError::Model(e.to_string()))?;
            let crop = image::warp_to_crop(&view, &frame_to_crop, size);
            let mut swapped = self.swapper.swap(&crop, &input)?;
            let face_mask: Vec<u8> = match &swapped.mask {
                Some(own) => shape.iter().zip(own).map(|(a, b)| ((*a as u16 * *b as u16) / 255) as u8).collect(),
                None => shape.clone(),
            };
            if colour_match {
                image::match_colour(&mut swapped.rgb, &crop, &face_mask);
            }
            image::paste_crop(picture, &swapped.rgb, &face_mask, size, &frame_to_crop, 1.0);
        }
        Ok(faces.len())
    }
}
