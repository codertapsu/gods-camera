//! The swapper contract: what each swap model expects and returns.

/// How the identity vector is prepared for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityInput {
    /// Unit-length embedding multiplied by the model's `emap`, then normalised again.
    EmapProjected,
    /// Unit-length embedding, passed as is.
    Normalised,
    /// Raw embedding.
    Raw,
}

/// Everything the engine needs to know to drive one swap model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapperContract {
    pub name: &'static str,
    /// Side of the square aligned crop, in pixels.
    pub crop_size: u32,
    /// Input is `(rgb / 255 - mean) / std`.
    pub mean: f32,
    pub std: f32,
    pub identity: IdentityInput,
}

impl SwapperContract {
    pub const INSWAPPER_128: Self = Self {
        name: "inswapper_128",
        crop_size: 128,
        mean: 0.0,
        std: 1.0,
        identity: IdentityInput::EmapProjected,
    };

    pub const HYPERSWAP_256: Self = Self {
        name: "hyperswap_256",
        crop_size: 256,
        mean: 0.5,
        std: 0.5,
        identity: IdentityInput::Normalised,
    };

    /// Convert one 8-bit channel value into the model's input range.
    pub fn normalise(&self, value: u8) -> f32 {
        (value as f32 / 255.0 - self.mean) / self.std
    }

    /// Convert one model output value back to 8 bits.
    pub fn denormalise(&self, value: f32) -> u8 {
        ((value * self.std + self.mean).clamp(0.0, 1.0) * 255.0) as u8
    }
}

fn l2_normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt() as f32;
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// The latent for an `EmapProjected` model: `normalise(normalise(embedding) x emap)`.
///
/// `emap` is the square matrix stored as the last initialiser of the model file,
/// in row-major order with side `embedding.len()`.
pub fn latent_from_embedding(embedding: &[f32], emap: &[f32]) -> Option<Vec<f32>> {
    let n = embedding.len();
    if n == 0 || emap.len() != n * n {
        return None;
    }
    let mut e = embedding.to_vec();
    l2_normalise(&mut e);
    let mut latent = vec![0f32; n];
    for (i, ei) in e.iter().enumerate() {
        let row = &emap[i * n..(i + 1) * n];
        for (l, m) in latent.iter_mut().zip(row) {
            *l += ei * m;
        }
    }
    l2_normalise(&mut latent);
    Some(latent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_round_trip() {
        for c in [SwapperContract::INSWAPPER_128, SwapperContract::HYPERSWAP_256] {
            for v in [0u8, 1, 127, 128, 254, 255] {
                assert!((c.denormalise(c.normalise(v)) as i16 - v as i16).abs() <= 1, "{}", c.name);
            }
        }
        assert_eq!(SwapperContract::HYPERSWAP_256.normalise(0), -1.0);
        assert_eq!(SwapperContract::HYPERSWAP_256.normalise(255), 1.0);
    }

    #[test]
    fn latent_with_identity_emap_is_the_unit_embedding() {
        let emap = [1.0, 0.0, 0.0, 1.0];
        let latent = latent_from_embedding(&[3.0, 4.0], &emap).unwrap();
        assert!((latent[0] - 0.6).abs() < 1e-6 && (latent[1] - 0.8).abs() < 1e-6);
        assert!(latent_from_embedding(&[1.0, 2.0], &[1.0; 3]).is_none());
    }
}
