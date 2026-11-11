//! Face alignment maths: the ArcFace 5-point template and similarity transforms.
//!
//! Ported from InsightFace (MIT), `python-package/insightface/utils/face_align.py`.

/// A 2x3 affine matrix in row-major order: `[[a, b, tx], [c, d, ty]]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine(pub [[f32; 3]; 2]);

/// Five keypoints: left eye, right eye, nose, left mouth corner, right mouth corner.
pub type Keypoints = [[f32; 2]; 5];

/// The ArcFace template on a 112 px canvas.
pub const ARCFACE_112: Keypoints = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

#[derive(Debug, PartialEq, Eq)]
pub enum AlignError {
    /// The crop size is not a multiple of 112 or 128.
    UnsupportedSize(u32),
    /// The source points are all the same, so no transform exists.
    Degenerate,
}

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlignError::UnsupportedSize(s) => {
                write!(f, "crop size {s} is not a multiple of 112 or 128")
            }
            AlignError::Degenerate => write!(f, "keypoints are degenerate"),
        }
    }
}

impl std::error::Error for AlignError {}

/// The template scaled for a crop of `size` pixels.
///
/// Multiples of 112 scale the template directly. Multiples of 128 scale by
/// `size / 128` and shift x by `8 * scale`, so 128 px uses +8 and 256 px uses +16.
pub fn template(size: u32) -> Result<Keypoints, AlignError> {
    let (ratio, diff_x) = if size != 0 && size.is_multiple_of(112) {
        (size as f32 / 112.0, 0.0)
    } else if size != 0 && size.is_multiple_of(128) {
        let r = size as f32 / 128.0;
        (r, 8.0 * r)
    } else {
        return Err(AlignError::UnsupportedSize(size));
    };
    let mut t = ARCFACE_112;
    for p in &mut t {
        p[0] = p[0] * ratio + diff_x;
        p[1] *= ratio;
    }
    Ok(t)
}

/// Least-squares similarity (rotation, uniform scale, translation) mapping `src` onto `dst`.
pub fn estimate_similarity(src: &[[f32; 2]], dst: &[[f32; 2]]) -> Result<Affine, AlignError> {
    assert_eq!(src.len(), dst.len());
    let n = src.len() as f64;
    let (mut sx, mut sy, mut dx, mut dy) = (0f64, 0f64, 0f64, 0f64);
    for (s, d) in src.iter().zip(dst) {
        sx += s[0] as f64;
        sy += s[1] as f64;
        dx += d[0] as f64;
        dy += d[1] as f64;
    }
    let (sx, sy, dx, dy) = (sx / n, sy / n, dx / n, dy / n);
    let (mut num_a, mut num_b, mut den) = (0f64, 0f64, 0f64);
    for (s, d) in src.iter().zip(dst) {
        let (x, y) = (s[0] as f64 - sx, s[1] as f64 - sy);
        let (u, v) = (d[0] as f64 - dx, d[1] as f64 - dy);
        num_a += x * u + y * v;
        num_b += x * v - y * u;
        den += x * x + y * y;
    }
    if den < 1e-12 {
        return Err(AlignError::Degenerate);
    }
    let (a, b) = (num_a / den, num_b / den);
    let tx = dx - (a * sx - b * sy);
    let ty = dy - (b * sx + a * sy);
    Ok(Affine([
        [a as f32, -b as f32, tx as f32],
        [b as f32, a as f32, ty as f32],
    ]))
}

/// The transform that maps a face's keypoints into an aligned crop of `size` pixels.
pub fn estimate_norm(kps: &Keypoints, size: u32) -> Result<Affine, AlignError> {
    let dst = template(size)?;
    estimate_similarity(kps, &dst)
}

impl Affine {
    pub const IDENTITY: Affine = Affine([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);

    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let m = &self.0;
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2],
        ]
    }

    /// The inverse transform, or `None` if the matrix is singular.
    pub fn invert(&self) -> Option<Affine> {
        let m = &self.0;
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (m[1][1] / det, -m[0][1] / det, -m[1][0] / det, m[0][0] / det);
        Some(Affine([
            [a, b, -(a * m[0][2] + b * m[1][2])],
            [c, d, -(c * m[0][2] + d * m[1][2])],
        ]))
    }

    /// `self` applied after `first`.
    pub fn compose(&self, first: &Affine) -> Affine {
        let (p, q) = (&self.0, &first.0);
        let mut r = [[0f32; 3]; 2];
        for i in 0..2 {
            r[i][0] = p[i][0] * q[0][0] + p[i][1] * q[1][0];
            r[i][1] = p[i][0] * q[0][1] + p[i][1] * q[1][1];
            r[i][2] = p[i][0] * q[0][2] + p[i][1] * q[1][2] + p[i][2];
        }
        Affine(r)
    }

    /// Uniform scale factor of a similarity transform.
    pub fn scale(&self) -> f32 {
        (self.0[0][0] * self.0[0][0] + self.0[1][0] * self.0[1][0]).sqrt()
    }
}

/// Paste-back mask kernel sizes from InsightFace's `inswapper.py`, for a face whose
/// warped crop covers `mask_h` by `mask_w` pixels in the frame.
///
/// Returns `(erode_kernel, blur_kernel)`: a square erosion kernel side, and the
/// odd Gaussian kernel side.
pub fn paste_back_kernels(mask_h: u32, mask_w: u32) -> (u32, u32) {
    let mask_size = ((mask_h as f64) * (mask_w as f64)).sqrt() as u32;
    let erode = (mask_size / 10).max(10);
    let blur = 2 * (mask_size / 20).max(5) + 1;
    (erode, blur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn template_sizes() {
        assert_eq!(template(112).unwrap(), ARCFACE_112);
        let t128 = template(128).unwrap();
        assert!(close(t128[0][0], 38.2946 + 8.0, 1e-4));
        assert!(close(t128[0][1], 51.6963, 1e-4));
        let t256 = template(256).unwrap();
        assert!(close(t256[2][0], 56.0252 * 2.0 + 16.0, 1e-3));
        assert_eq!(template(100), Err(AlignError::UnsupportedSize(100)));
        assert_eq!(template(0), Err(AlignError::UnsupportedSize(0)));
    }

    #[test]
    fn recovers_known_similarity() {
        let (s, th) = (1.7f32, 0.4f32);
        let m = Affine([
            [s * th.cos(), -s * th.sin(), 12.5],
            [s * th.sin(), s * th.cos(), -3.0],
        ]);
        let dst: Vec<[f32; 2]> = ARCFACE_112.iter().map(|p| m.apply(*p)).collect();
        let est = estimate_similarity(&ARCFACE_112, &dst).unwrap();
        for i in 0..2 {
            for j in 0..3 {
                assert!(close(est.0[i][j], m.0[i][j], 1e-3), "{est:?} vs {m:?}");
            }
        }
        assert!(close(est.scale(), s, 1e-4));
    }

    #[test]
    fn invert_and_compose_round_trip() {
        let m = Affine([[0.8, -0.3, 40.0], [0.3, 0.8, -7.0]]);
        let id = m.compose(&m.invert().unwrap());
        for i in 0..2 {
            for j in 0..3 {
                assert!(close(id.0[i][j], Affine::IDENTITY.0[i][j], 1e-4));
            }
        }
        assert!(Affine([[0.0; 3]; 2]).invert().is_none());
    }

    #[test]
    fn degenerate_points_are_rejected() {
        let same = [[1.0f32, 1.0]; 5];
        assert_eq!(
            estimate_similarity(&same, &ARCFACE_112),
            Err(AlignError::Degenerate)
        );
    }

    #[test]
    fn kernels_match_reference_formula() {
        assert_eq!(paste_back_kernels(200, 200), (20, 21));
        assert_eq!(paste_back_kernels(40, 40), (10, 11));
    }
}
