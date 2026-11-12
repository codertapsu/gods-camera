//! The faces a user can swap in, kept on disk, each in its own encrypted file.
//!
//! The same rules as the Mac app's `FaceLibrary` and `FaceModel`: a face is
//! made from up to twenty photos of one person, the photos themselves are not
//! kept, and nobody can save a face without saying whose it is.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use gc_infer::image::{ImageView, PixelFormat};
use gc_live::combine_identities;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The most photos one face is made from. More add little and cost space.
pub const MAX_PHOTOS: usize = 20;
/// Photos are made no larger than this on their longer side before they are read.
const LONGEST_SIDE: u32 = 2048;
/// Side of the small picture kept of each photo's face.
const THUMBNAIL: u32 = 160;

/// What the user stated about a face when adding it. A face cannot be saved without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Consent {
    /// "This is me."
    Myself,
    /// "I have this person's permission."
    Permission,
    /// "This face is AI-generated and shows no real person."
    Synthetic,
}

/// One photo a saved face was made from. The photo itself is not kept: only
/// what the face looked like in it, and a small picture for the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacePhoto {
    pub id: String,
    /// 512 numbers that stand for the face as this photo shows it.
    pub identity: Vec<f32>,
    /// A small picture of the face, as PNG.
    pub thumbnail: Vec<u8>,
}

/// A face the user can swap in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedFace {
    pub id: String,
    pub name: String,
    /// When it was added, in milliseconds since 1970. The list is in this order.
    pub added: u64,
    pub consent: Consent,
    /// 512 numbers that stand for the face: its photos' identities combined.
    /// No photo is needed to use it.
    pub identity: Vec<f32>,
    /// The photos the identity was made from, at least one.
    pub photos: Vec<FacePhoto>,
}

impl SavedFace {
    /// A small picture for the list, as PNG: the first photo's.
    pub fn thumbnail(&self) -> &[u8] {
        self.photos.first().map_or(&[], |photo| &photo.thumbnail)
    }
}

/// A name no other face or photo has: 32 hexadecimal digits.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's random generator is available");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
pub enum FaceError {
    /// The key that protects the faces could not be had from where it is kept.
    Key(String),
    /// A face's file is missing, damaged, or sealed with another key.
    Unreadable(String),
    Io(std::io::Error),
}

impl fmt::Display for FaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FaceError::Key(why) => write!(f, "the key for saved faces is not available: {why}"),
            FaceError::Unreadable(id) => write!(f, "the saved face {id} cannot be read"),
            FaceError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FaceError {}

impl From<std::io::Error> for FaceError {
    fn from(error: std::io::Error) -> Self {
        FaceError::Io(error)
    }
}

/// Where the key that encrypts saved faces lives: apart from the files it
/// protects. The Keychain on a Mac, Credential Manager on Windows.
pub trait KeyStore: Send + Sync {
    /// The key, made on first use.
    fn load_or_create(&self) -> Result<[u8; 32], FaceError>;
    /// Remove the key. Saved faces can no longer be read afterwards.
    fn delete(&self);
}

/// A new key, for a key store that has none yet.
pub fn new_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).expect("the operating system's random generator is available");
    key
}

/// Saved faces on disk.
///
/// A face's identity vector is biometric data. Files are sealed with
/// ChaCha20-Poly1305 under a key kept elsewhere, so a copy of the folder alone
/// reveals nothing.
pub struct FaceLibrary {
    directory: PathBuf,
    key: [u8; 32],
}

impl FaceLibrary {
    pub fn new(directory: impl Into<PathBuf>, key: [u8; 32]) -> Result<Self, FaceError> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        Ok(Self { directory, key })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.face"))
    }

    pub fn save(&self, face: &SavedFace) -> Result<(), FaceError> {
        if !is_id(&face.id) {
            return Err(FaceError::Unreadable(face.id.clone()));
        }
        let plain = serde_json::to_vec(face).map_err(|error| FaceError::Io(error.into()))?;
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).expect("the operating system's random generator is available");
        // The file's name is bound into the seal, so one face's file cannot be passed off as another's.
        let sealed = ChaCha20Poly1305::new(&Key::from(self.key))
            .encrypt(&Nonce::from(nonce), Payload { msg: &plain, aad: face.id.as_bytes() })
            .map_err(|_| FaceError::Unreadable(face.id.clone()))?;
        let mut file = nonce.to_vec();
        file.extend_from_slice(&sealed);
        // Replaced in one step, so a crash never leaves half a face.
        let partial = self.path(&face.id).with_extension("partial");
        std::fs::write(&partial, &file)?;
        Ok(std::fs::rename(&partial, self.path(&face.id))?)
    }

    pub fn load(&self, id: &str) -> Result<SavedFace, FaceError> {
        let unreadable = || FaceError::Unreadable(id.to_string());
        if !is_id(id) {
            return Err(unreadable());
        }
        let file = std::fs::read(self.path(id)).map_err(|_| unreadable())?;
        if file.len() < 12 + 16 {
            return Err(unreadable());
        }
        let (nonce, sealed) = file.split_at(12);
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| unreadable())?;
        let plain = ChaCha20Poly1305::new(&Key::from(self.key))
            .decrypt(&Nonce::from(nonce), Payload { msg: sealed, aad: id.as_bytes() })
            .map_err(|_| unreadable())?;
        let face: SavedFace = serde_json::from_slice(&plain).map_err(|_| unreadable())?;
        (face.id == id && !face.photos.is_empty()).then_some(face).ok_or_else(unreadable)
    }

    /// Every readable face, oldest first. Files that cannot be opened are left out.
    pub fn list(&self) -> Vec<SavedFace> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else { return Vec::new() };
        let mut faces: Vec<SavedFace> = entries
            .filter_map(|entry| entry.ok()?.path().file_name()?.to_str()?.strip_suffix(".face").map(str::to_string))
            .filter_map(|id| self.load(&id).ok())
            .collect();
        faces.sort_by(|a, b| a.added.cmp(&b.added).then_with(|| a.id.cmp(&b.id)));
        faces
    }

    pub fn delete(&self, id: &str) {
        if is_id(id) {
            let _ = std::fs::remove_file(self.path(id));
        }
    }

    /// Remove every saved face.
    pub fn delete_all(&self) {
        let Ok(entries) = std::fs::read_dir(&self.directory) else { return };
        for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
            if path.extension().is_some_and(|extension| extension == "face") {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// What was found in one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundFace {
    pub identity: Vec<f32>,
    /// How many faces the photo shows. The largest one was used.
    pub faces: usize,
    /// That face's box: left, top, right, bottom.
    pub bbox: [f32; 4],
}

/// Finds the face in a photo. The models do this in the app; a stand-in does in tests.
pub trait FindFace {
    /// The largest face in a picture, or `None` if it shows none.
    fn find(&mut self, picture: &ImageView<'_>) -> Result<Option<FoundFace>, String>;
}

impl FindFace for gc_live::Enroller {
    fn find(&mut self, picture: &ImageView<'_>) -> Result<Option<FoundFace>, String> {
        let found = self.enrol(picture).map_err(|error| error.to_string())?;
        Ok(found.map(|found| FoundFace { identity: found.identity, faces: found.faces, bbox: found.bbox }))
    }
}

/// One photo read for a face that is being added.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPhoto {
    pub id: String,
    pub file_name: String,
    pub identity: Vec<f32>,
    /// A small picture of the face, as PNG.
    pub thumbnail: Vec<u8>,
    pub faces_in_photo: usize,
    /// The photo shows the same person as the others, so it goes into the face.
    pub used: bool,
}

/// Photos that were read and now wait for the user: to become a new face, or
/// to be added to a saved one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PendingFace {
    pub photos: Vec<PendingPhoto>,
    /// Files in which no face was found, or that could not be read.
    pub skipped: Vec<String>,
    /// The saved face the photos are for, or `None` for a new face.
    pub target: Option<String>,
    /// More files were given than a face has room for; the rest were not read.
    pub some_not_read: bool,
}

impl PendingFace {
    pub fn used_count(&self) -> usize {
        self.photos.iter().filter(|photo| photo.used).count()
    }

    pub fn left_out_count(&self) -> usize {
        self.photos.len() - self.used_count()
    }
}

/// A picture as packed red, green, blue, turned the way it was taken and no
/// larger than the models need.
pub struct Photo {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Photo {
    /// Read a picture file. JPEG, PNG, WebP, TIFF and BMP are understood here.
    pub fn open(path: &Path) -> Result<Self, String> {
        use image::ImageDecoder;
        let reader = image::ImageReader::open(path).map_err(|error| error.to_string())?;
        let mut decoder = reader.with_guessed_format().map_err(|error| error.to_string())?.into_decoder().map_err(|error| error.to_string())?;
        // A phone stores a picture as the sensor saw it, and says beside it which way up it was held.
        let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
        picture.apply_orientation(orientation);
        Ok(Self::from_image(picture))
    }

    pub fn from_image(picture: image::DynamicImage) -> Self {
        let picture = match picture.width().max(picture.height()) > LONGEST_SIDE {
            true => picture.resize(LONGEST_SIDE, LONGEST_SIDE, image::imageops::FilterType::Triangle),
            false => picture,
        };
        let rgb = picture.to_rgb8();
        Self { width: rgb.width(), height: rgb.height(), rgb: rgb.into_raw() }
    }

    pub fn view(&self) -> Option<ImageView<'_>> {
        ImageView::new(&self.rgb, self.width, self.height, self.width as usize * 3, PixelFormat::Rgb)
    }

    /// A square around a face with some margin, 160 pixels a side, as PNG.
    pub fn thumbnail(&self, bbox: [f32; 4]) -> Vec<u8> {
        let side = (bbox[2] - bbox[0]).max(bbox[3] - bbox[1]) * 1.5;
        let centre = [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0];
        // The square, kept inside the picture.
        let left = (centre[0] - side / 2.0).max(0.0);
        let top = (centre[1] - side / 2.0).max(0.0);
        let right = (centre[0] + side / 2.0).min(self.width as f32);
        let bottom = (centre[1] + side / 2.0).min(self.height as f32);
        let (width, height) = ((right - left).max(1.0) as u32, (bottom - top).max(1.0) as u32);
        let Some(whole) = image::RgbImage::from_raw(self.width, self.height, self.rgb.clone()) else { return Vec::new() };
        let cut = image::imageops::crop_imm(&whole, left as u32, top as u32, width.min(self.width), height.min(self.height));
        let small = image::imageops::resize(&cut.to_image(), THUMBNAIL, THUMBNAIL, image::imageops::FilterType::CatmullRom);
        let mut png = Vec::new();
        let written = image::DynamicImage::ImageRgb8(small).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
        if written.is_err() {
            png.clear();
        }
        png
    }
}

/// Read photos and say what was found in them, ready to be saved: as a new
/// face, or as more photos of `target`.
///
/// The photos must show one person: the one in the saved face, or the one
/// most of the new photos agree on. The others are kept in the answer, marked
/// as left out, so the user sees which they were.
pub fn read_photos(paths: &[PathBuf], target: Option<&SavedFace>, finder: &mut dyn FindFace) -> PendingFace {
    let room = MAX_PHOTOS.saturating_sub(target.map_or(0, |face| face.photos.len()));
    let mut pending = PendingFace {
        target: target.map(|face| face.id.clone()),
        some_not_read: paths.len() > room,
        ..PendingFace::default()
    };
    for path in paths.iter().take(room) {
        let file_name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
        let found = Photo::open(path).and_then(|photo| {
            let view = photo.view().ok_or("an empty picture")?;
            Ok(finder.find(&view)?.map(|found| (photo.thumbnail(found.bbox), found)))
        });
        match found {
            Ok(Some((thumbnail, found))) => pending.photos.push(PendingPhoto {
                id: new_id(),
                file_name,
                identity: found.identity,
                thumbnail,
                faces_in_photo: found.faces,
                used: true,
            }),
            // No face in it, or not a picture this app can read.
            Ok(None) | Err(_) => pending.skipped.push(file_name),
        }
    }
    let existing: Vec<Vec<f32>> = target.map_or_else(Vec::new, |face| face.photos.iter().map(|photo| photo.identity.clone()).collect());
    let all: Vec<Vec<f32>> = existing.iter().cloned().chain(pending.photos.iter().map(|photo| photo.identity.clone())).collect();
    let combined = combine_identities(&all);
    for (photo, verdict) in pending.photos.iter_mut().zip(&combined.photos[existing.len()..]) {
        photo.used = verdict.used;
    }
    pending
}

/// Make the face the pending photos stand for: a new one, or `target` with
/// the photos added. `None` if no photo is to be used. `number` is the new
/// face's place in the list, for a face saved without a name.
pub fn face_from_pending(
    pending: &PendingFace,
    target: Option<&SavedFace>,
    name: &str,
    consent: Consent,
    number: usize,
) -> Option<SavedFace> {
    let added: Vec<FacePhoto> = pending
        .photos
        .iter()
        .filter(|photo| photo.used)
        .map(|photo| FacePhoto { id: photo.id.clone(), identity: photo.identity.clone(), thumbnail: photo.thumbnail.clone() })
        .collect();
    if added.is_empty() {
        return None;
    }
    let identity_of = |photos: &[FacePhoto]| {
        combine_identities(&photos.iter().map(|photo| photo.identity.clone()).collect::<Vec<_>>()).identity
    };
    Some(match target {
        Some(current) => {
            let photos: Vec<FacePhoto> = current.photos.iter().cloned().chain(added).take(MAX_PHOTOS).collect();
            SavedFace { identity: identity_of(&photos), photos, ..current.clone() }
        }
        None => SavedFace {
            id: new_id(),
            name: match name.trim() {
                "" => format!("Face {number}"),
                given => given.to_string(),
            },
            added: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64),
            consent,
            identity: identity_of(&added),
            photos: added,
        },
    })
}

/// `face` without one of its photos. `None` if that is its last photo, or not
/// one of its photos: the last photo cannot be removed; delete the face instead.
pub fn without_photo(face: &SavedFace, photo_id: &str) -> Option<SavedFace> {
    if face.photos.len() < 2 || !face.photos.iter().any(|photo| photo.id == photo_id) {
        return None;
    }
    let photos: Vec<FacePhoto> = face.photos.iter().filter(|photo| photo.id != photo_id).cloned().collect();
    let identity = combine_identities(&photos.iter().map(|photo| photo.identity.clone()).collect::<Vec<_>>()).identity;
    Some(SavedFace { identity, photos, ..face.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gc-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// An identity pointing mostly along one of three directions, one per "person".
    fn identity(person: usize, variation: f32) -> Vec<f32> {
        let mut vector = vec![0.01f32; 512];
        vector[person] = 1.0;
        vector[100 + person] = variation;
        vector
    }

    fn face(name: &str, person: usize, photos: usize) -> SavedFace {
        let photos: Vec<FacePhoto> = (0..photos)
            .map(|index| FacePhoto { id: new_id(), identity: identity(person, index as f32 * 0.1), thumbnail: vec![index as u8; 40] })
            .collect();
        SavedFace { id: new_id(), name: name.into(), added: 1_000, consent: Consent::Myself, identity: photos[0].identity.clone(), photos }
    }

    #[test]
    fn a_saved_face_comes_back_and_its_file_says_nothing() {
        let folder = scratch("faces");
        let library = FaceLibrary::new(&folder, [7; 32]).unwrap();
        let mut first = face("Mai", 0, 3);
        let mut second = face("An", 1, 1);
        second.added = 2_000;
        second.consent = Consent::Permission;
        library.save(&second).unwrap();
        library.save(&first).unwrap();
        assert_eq!(library.list(), [first.clone(), second.clone()], "oldest first");
        assert_eq!(library.load(&first.id).unwrap().thumbnail(), &[0u8; 40][..]);

        // Nothing of the face can be read from the file: not its name, not its numbers.
        let file = std::fs::read(folder.join(format!("{}.face", first.id))).unwrap();
        assert!(!file.windows(3).any(|window| window == b"Mai"));
        assert!(!file.windows(8).any(|window| window == b"identity"));

        // Saving again replaces the file.
        first.name = "Mai Anh".into();
        library.save(&first).unwrap();
        assert_eq!(library.load(&first.id).unwrap().name, "Mai Anh");

        library.delete(&second.id);
        assert_eq!(library.list().len(), 1);
        library.delete_all();
        assert!(library.list().is_empty());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_file_that_was_touched_moved_or_sealed_with_another_key_is_not_read() {
        let folder = scratch("sealed");
        let library = FaceLibrary::new(&folder, [7; 32]).unwrap();
        let (mine, other) = (face("Mai", 0, 1), face("An", 1, 1));
        library.save(&mine).unwrap();
        library.save(&other).unwrap();
        let path = |id: &str| folder.join(format!("{id}.face"));

        // One face's file under another's name.
        std::fs::copy(path(&other.id), path(&mine.id)).unwrap();
        assert!(matches!(library.load(&mine.id), Err(FaceError::Unreadable(_))));
        assert_eq!(library.list(), std::slice::from_ref(&other), "the file that does not open is left out");

        // One byte changed.
        let mut bytes = std::fs::read(path(&other.id)).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(path(&other.id), &bytes).unwrap();
        assert!(library.load(&other.id).is_err());

        // Another key: as after "delete all my data".
        library.save(&mine).unwrap();
        assert!(FaceLibrary::new(&folder, [8; 32]).unwrap().load(&mine.id).is_err());
        // A name that is not an id never becomes a path.
        assert!(library.load("..\\..\\elsewhere").is_err());
        library.delete("../elsewhere");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_words_for_consent_are_the_ones_the_mac_app_writes() {
        assert_eq!(serde_json::to_string(&Consent::Myself).unwrap(), "\"myself\"");
        assert_eq!(serde_json::to_string(&Consent::Permission).unwrap(), "\"permission\"");
        assert_eq!(serde_json::to_string(&Consent::Synthetic).unwrap(), "\"synthetic\"");
    }

    /// Finds a "face" by the colour of a picture's first pixel: red is person 0,
    /// green person 1, and a black picture has no face.
    struct ByColour;

    impl FindFace for ByColour {
        fn find(&mut self, picture: &ImageView<'_>) -> Result<Option<FoundFace>, String> {
            let pixel = picture.row(0);
            let person = match (pixel[0], pixel[1]) {
                (red, _) if red > 200 => 0,
                (_, green) if green > 200 => 1,
                _ => return Ok(None),
            };
            let (width, height) = (picture.width as f32, picture.height as f32);
            Ok(Some(FoundFace {
                identity: identity(person, pixel[2] as f32 / 255.0),
                faces: 1 + (pixel[2] > 100) as usize,
                bbox: [width * 0.25, height * 0.25, width * 0.75, height * 0.75],
            }))
        }
    }

    fn photo_file(folder: &Path, name: &str, colour: [u8; 3]) -> PathBuf {
        let path = folder.join(name);
        image::RgbImage::from_pixel(64, 48, image::Rgb(colour)).save(&path).unwrap();
        path
    }

    #[test]
    fn photos_of_one_person_become_a_face_and_strangers_are_left_out() {
        let folder = scratch("photos");
        let paths = vec![
            photo_file(&folder, "a.png", [255, 0, 10]),
            photo_file(&folder, "b.png", [255, 0, 60]),
            photo_file(&folder, "stranger.png", [0, 255, 0]),
            photo_file(&folder, "wall.png", [0, 0, 0]),
            photo_file(&folder, "c.png", [255, 0, 120]),
        ];
        std::fs::write(folder.join("notes.txt"), b"not a picture").unwrap();
        let mut all = paths.clone();
        all.push(folder.join("notes.txt"));
        all.push(folder.join("missing.jpg"));

        let pending = read_photos(&all, None, &mut ByColour);
        let names: Vec<&str> = pending.photos.iter().map(|photo| photo.file_name.as_str()).collect();
        assert_eq!(names, ["a.png", "b.png", "stranger.png", "c.png"]);
        assert_eq!(pending.skipped, ["wall.png", "notes.txt", "missing.jpg"]);
        assert_eq!(pending.photos.iter().map(|photo| photo.used).collect::<Vec<_>>(), [true, true, false, true]);
        assert_eq!((pending.used_count(), pending.left_out_count(), pending.some_not_read), (3, 1, false));
        assert_eq!(pending.photos[3].faces_in_photo, 2);
        // Each photo has its small picture: a PNG of 160 pixels a side.
        let small = image::load_from_memory(&pending.photos[0].thumbnail).unwrap();
        assert_eq!((small.width(), small.height()), (160, 160));

        let saved = face_from_pending(&pending, None, "  ", Consent::Synthetic, 3).unwrap();
        assert_eq!((saved.name.as_str(), saved.consent, saved.photos.len()), ("Face 3", Consent::Synthetic, 3));
        assert!(saved.identity[0] > 0.9 && saved.identity[1] < 0.1, "the face is the person most photos show");
        assert_eq!(face_from_pending(&pending, None, "Mai", Consent::Myself, 1).unwrap().name, "Mai");

        // More photos for the saved face: the stranger is left out again, and the rest join.
        let more = read_photos(&[paths[2].clone(), photo_file(&folder, "d.png", [255, 0, 200])], Some(&saved), &mut ByColour);
        assert_eq!(more.target.as_deref(), Some(saved.id.as_str()));
        assert_eq!(more.photos.iter().map(|photo| photo.used).collect::<Vec<_>>(), [false, true]);
        let grown = face_from_pending(&more, Some(&saved), "ignored", Consent::Myself, 9).unwrap();
        assert_eq!((grown.id.as_str(), grown.name.as_str(), grown.consent), (saved.id.as_str(), "Face 3", Consent::Synthetic));
        assert_eq!(grown.photos.len(), 4);

        // Only strangers: there is nothing to save.
        let none = read_photos(&[paths[2].clone()], Some(&saved), &mut ByColour);
        assert_eq!(none.used_count(), 0);
        assert!(face_from_pending(&none, Some(&saved), "", Consent::Myself, 1).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_face_takes_twenty_photos_and_keeps_at_least_one() {
        let folder = scratch("limit");
        let full = face("Mai", 0, 18);
        let paths: Vec<PathBuf> = (0..5).map(|index| photo_file(&folder, &format!("{index}.png"), [255, 0, index * 20])).collect();
        let pending = read_photos(&paths, Some(&full), &mut ByColour);
        assert_eq!((pending.photos.len(), pending.some_not_read), (2, true), "only as many as there is room for are read");
        assert_eq!(face_from_pending(&pending, Some(&full), "", Consent::Myself, 1).unwrap().photos.len(), MAX_PHOTOS);

        let two = face("An", 1, 2);
        let one = without_photo(&two, &two.photos[0].id).unwrap();
        assert_eq!(one.photos, std::slice::from_ref(&two.photos[1]));
        assert_eq!(one.thumbnail(), &two.photos[1].thumbnail[..]);
        assert!(without_photo(&one, &one.photos[0].id).is_none(), "the last photo stays");
        assert!(without_photo(&two, "no such photo").is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_photo_is_turned_the_way_it_was_held_and_made_small_enough() {
        // A wide picture comes out no longer than the models need.
        let photo = Photo::from_image(image::DynamicImage::new_rgb8(4096, 1024));
        assert_eq!((photo.width, photo.height), (2048, 512));
        assert!(photo.view().is_some());
        // A face at the very edge still gets a whole small picture.
        let png = photo.thumbnail([2000.0, 400.0, 2048.0, 512.0]);
        let small = image::load_from_memory(&png).unwrap();
        assert_eq!((small.width(), small.height()), (160, 160));
    }
}
