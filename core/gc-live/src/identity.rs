//! One identity from several photos of the same person.
//!
//! A single photo fixes one expression, one angle and one light into the
//! identity vector. The average over several photos keeps what they share,
//! which is the person, and loses what differs. A photo of somebody else, or
//! one where the wrong face was picked, would pull the average away, so photos
//! that disagree with the rest are left out.

/// Two identity vectors of the same person are at least this alike, as the
/// cosine of the angle between them. Different people score near 0; the same
/// person in different photos usually scores above 0.5.
pub const SAME_PERSON: f32 = 0.3;

/// What was decided about one photo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoVerdict {
    /// The photo went into the combined identity.
    pub used: bool,
    /// How alike this photo's face and the combined identity are, -1 to 1.
    /// 0 when there is no combined identity or the photo's vector is unusable.
    pub similarity: f32,
}

/// The result of combining identity vectors.
#[derive(Debug, Clone, PartialEq)]
pub struct CombinedIdentity {
    /// The identity to swap in, or empty when no photo could be used.
    pub identity: Vec<f32>,
    /// One verdict per photo, in the order given.
    pub photos: Vec<PhotoVerdict>,
}

impl CombinedIdentity {
    pub fn used(&self) -> usize {
        self.photos.iter().filter(|photo| photo.used).count()
    }
}

fn norm(vector: &[f32]) -> f32 {
    vector.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>().sqrt() as f32
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (*x as f64) * (*y as f64)).sum::<f64>() as f32
}

/// Combine the identity vectors of several photos into one.
///
/// The photos that show the same person as most of the others are averaged,
/// each with the same weight whatever its vector's length. The rest are left
/// out and marked. With two photos that disagree, the first one wins.
pub fn combine_identities(identities: &[Vec<f32>]) -> CombinedIdentity {
    let length = identities.iter().map(Vec::len).find(|len| *len > 0).unwrap_or(0);
    // Each usable vector, scaled to length 1, with its original length.
    let units: Vec<Option<(Vec<f32>, f32)>> = identities
        .iter()
        .map(|vector| {
            let size = norm(vector);
            (vector.len() == length && length > 0 && size.is_finite() && size > 0.0 && vector.iter().all(|v| v.is_finite()))
                .then(|| (vector.iter().map(|v| v / size).collect(), size))
        })
        .collect();
    let usable: Vec<usize> = (0..units.len()).filter(|i| units[*i].is_some()).collect();
    let unit = |i: usize| &units[i].as_ref().expect("usable").0;
    let mut photos = vec![PhotoVerdict { used: false, similarity: 0.0 }; identities.len()];
    if usable.is_empty() {
        return CombinedIdentity { identity: Vec::new(), photos };
    }

    // The photo most like all the others stands for the person. Ties go to the earliest.
    let mut reference = usable[0];
    let mut best = f32::NEG_INFINITY;
    for &i in &usable {
        let agreement: f32 = usable.iter().filter(|j| **j != i).map(|&j| dot(unit(i), unit(j))).sum();
        if agreement > best {
            best = agreement;
            reference = i;
        }
    }
    let mut group: Vec<usize> =
        usable.iter().copied().filter(|&i| i == reference || dot(unit(i), unit(reference)) >= SAME_PERSON).collect();

    let average = |group: &[usize]| -> Vec<f32> {
        let mut sum = vec![0f64; length];
        for &i in group {
            for (total, value) in sum.iter_mut().zip(unit(i)) {
                *total += *value as f64;
            }
        }
        let size = sum.iter().map(|v| v * v).sum::<f64>().sqrt().max(f64::MIN_POSITIVE);
        sum.iter().map(|v| (v / size) as f32).collect()
    };
    // Judge every photo once more against the group's average, which is a
    // better picture of the person than any single photo.
    let mut mean = average(&group);
    let regrouped: Vec<usize> = usable.iter().copied().filter(|&i| dot(unit(i), &mean) >= SAME_PERSON).collect();
    if !regrouped.is_empty() && regrouped != group {
        group = regrouped;
        mean = average(&group);
    }

    for &i in &usable {
        photos[i] = PhotoVerdict { used: group.contains(&i), similarity: dot(unit(i), &mean) };
    }
    // Encoder vectors have a typical length, which some swap models rely on.
    let size = group.iter().map(|&i| units[i].as_ref().expect("usable").1).sum::<f32>() / group.len() as f32;
    CombinedIdentity { identity: mean.iter().map(|v| v * size).collect(), photos }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vector of `length` numbers pointing mostly along axis `axis`, with a
    /// little of axis `lean` mixed in, scaled to `size`.
    fn vector(axis: usize, lean: usize, amount: f32, size: f32) -> Vec<f32> {
        let mut v = vec![0f32; 16];
        v[axis] = 1.0;
        v[lean] += amount;
        let n = norm(&v);
        v.iter().map(|x| x / n * size).collect()
    }

    #[test]
    fn one_photo_is_returned_as_it_is() {
        let only = vector(0, 1, 0.2, 21.0);
        let combined = combine_identities(std::slice::from_ref(&only));
        assert_eq!(combined.used(), 1);
        assert!(combined.photos[0].similarity > 0.999);
        for (a, b) in combined.identity.iter().zip(&only) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn photos_of_one_person_are_averaged_with_equal_weight() {
        // The same direction seen with three different slants, and three different lengths.
        let photos = vec![vector(0, 1, 0.5, 18.0), vector(0, 2, 0.5, 24.0), vector(0, 3, 0.5, 21.0)];
        let combined = combine_identities(&photos);
        assert_eq!(combined.used(), 3);
        // The slants cancel: the average points along axis 0 more than any single photo does.
        let unit: Vec<f32> = combined.identity.iter().map(|v| v / norm(&combined.identity)).collect();
        let single = photos[0][0] / norm(&photos[0]);
        assert!(unit[0] > single + 0.02, "{} against {single}", unit[0]);
        // The three slants are equally present although the vectors differ in length.
        assert!((unit[1] - unit[2]).abs() < 1e-4 && (unit[2] - unit[3]).abs() < 1e-4);
        assert!((norm(&combined.identity) - 21.0).abs() < 0.01, "the usual length is kept");
        assert!(combined.photos.iter().all(|photo| photo.similarity > 0.9));
    }

    #[test]
    fn a_photo_of_somebody_else_is_left_out() {
        let photos = vec![
            vector(0, 1, 0.3, 20.0),
            vector(5, 6, 0.3, 20.0), // another person
            vector(0, 2, 0.3, 20.0),
            vector(0, 3, 0.3, 20.0),
        ];
        let combined = combine_identities(&photos);
        assert_eq!(combined.photos.iter().map(|photo| photo.used).collect::<Vec<_>>(), [true, false, true, true]);
        assert!(combined.photos[1].similarity < SAME_PERSON);
        assert!(combined.identity[5].abs() < 1e-6, "the other person left no trace");
    }

    #[test]
    fn of_two_photos_that_disagree_the_first_wins() {
        let photos = vec![vector(0, 1, 0.1, 20.0), vector(7, 8, 0.1, 20.0)];
        let combined = combine_identities(&photos);
        assert_eq!(combined.photos.iter().map(|photo| photo.used).collect::<Vec<_>>(), [true, false]);
    }

    #[test]
    fn unusable_vectors_are_marked_and_ignored() {
        let good = vector(0, 1, 0.2, 20.0);
        let photos = vec![vec![0.0; 16], good.clone(), vec![f32::NAN; 16], vec![1.0; 3]];
        let combined = combine_identities(&photos);
        assert_eq!(combined.photos.iter().map(|photo| photo.used).collect::<Vec<_>>(), [false, true, false, false]);
        assert_eq!(combined.identity.len(), 16);

        let none = combine_identities(&[vec![0.0; 16], Vec::new()]);
        assert!(none.identity.is_empty());
        assert_eq!(none.used(), 0);
        assert!(combine_identities(&[]).identity.is_empty());
    }
}
