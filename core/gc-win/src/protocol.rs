//! The contract between the app and its camera's media source, as the app
//! needs it. The source of truth is `apps/windows/shared/gc_vcam_protocol.h`;
//! a test compares this file with it, so the two cannot drift apart unnoticed.

/// The class Windows creates in its camera service.
pub const CLSID: &str = "{1E1EB430-08A7-42F4-9F7D-C53A00B36024}";
/// The string attribute that tells the media source which pipe to fetch pictures from.
pub const ATTRIBUTE_PIPE_NAME: u128 = 0x94A0A41C_B68F_4E28_98AA_B471A72D5025;
/// The name people see in a camera list.
pub const FRIENDLY_NAME: &str = "God's Camera";
/// The pipe's name is this followed by the user's SID as text.
pub const PIPE_PREFIX: &str = r"\\.\pipe\godscamera.frames.";

pub const MAGIC_REQUEST: u32 = 0x5256_4347; // "GCVR"
pub const MAGIC_FRAME: u32 = 0x4656_4347; // "GCVF"
pub const VERSION: u16 = 1;
pub const FOURCC_NV12: u32 = 0x3231_564E; // "NV12"

/// The largest picture either side accepts: 4K.
pub const MAX_WIDTH: u32 = 3840;
pub const MAX_HEIGHT: u32 = 2160;
/// The smallest. Nothing a meeting app asks for is near it.
pub const MIN_SIDE: u32 = 16;

/// `gc_vcam_request` on the wire: packed, little-endian.
pub const REQUEST_BYTES: usize = 28;
/// `gc_vcam_frame_header` on the wire: packed, little-endian.
pub const FRAME_HEADER_BYTES: usize = 40;

/// What the reading side asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Wanted {
    pub width: u32,
    pub height: u32,
    /// Frames a second, as a fraction.
    pub frame_rate: (u32, u32),
}

/// Whether a picture of this size can cross the pipe.
pub fn size_is_sensible(width: u32, height: u32) -> bool {
    (MIN_SIDE..=MAX_WIDTH).contains(&width)
        && (MIN_SIDE..=MAX_HEIGHT).contains(&height)
        && width.is_multiple_of(2)
        && height.is_multiple_of(2)
}

/// Bytes in one NV12 picture: the luma plane, then the chroma plane at half its height.
pub fn nv12_bytes(width: u32, height: u32) -> usize {
    let luma = width as usize * height as usize;
    luma + luma / 2
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Read a request from the media source. `None` if it is not one this version understands.
pub fn parse_request(bytes: &[u8; REQUEST_BYTES]) -> Option<Wanted> {
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let size = u16::from_le_bytes([bytes[6], bytes[7]]);
    let wanted =
        Wanted { width: u32_at(bytes, 8), height: u32_at(bytes, 12), frame_rate: (u32_at(bytes, 20), u32_at(bytes, 24)) };
    let sensible = u32_at(bytes, 0) == MAGIC_REQUEST
        && version == VERSION
        && size as usize == REQUEST_BYTES
        && u32_at(bytes, 16) == FOURCC_NV12
        && size_is_sensible(wanted.width, wanted.height)
        && wanted.frame_rate.0 > 0
        && wanted.frame_rate.1 > 0;
    sensible.then_some(wanted)
}

/// A request as the media source writes it. The app never sends one; tests do,
/// standing in for the source.
pub fn request(wanted: &Wanted) -> [u8; REQUEST_BYTES] {
    let mut bytes = [0u8; REQUEST_BYTES];
    bytes[0..4].copy_from_slice(&MAGIC_REQUEST.to_le_bytes());
    bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
    bytes[6..8].copy_from_slice(&(REQUEST_BYTES as u16).to_le_bytes());
    bytes[8..12].copy_from_slice(&wanted.width.to_le_bytes());
    bytes[12..16].copy_from_slice(&wanted.height.to_le_bytes());
    bytes[16..20].copy_from_slice(&FOURCC_NV12.to_le_bytes());
    bytes[20..24].copy_from_slice(&wanted.frame_rate.0.to_le_bytes());
    bytes[24..28].copy_from_slice(&wanted.frame_rate.1.to_le_bytes());
    bytes
}

/// The header that goes before every picture.
pub fn frame_header(width: u32, height: u32, sequence: u64, time_100ns: i64) -> [u8; FRAME_HEADER_BYTES] {
    let mut bytes = [0u8; FRAME_HEADER_BYTES];
    bytes[0..4].copy_from_slice(&MAGIC_FRAME.to_le_bytes());
    bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
    bytes[6..8].copy_from_slice(&(FRAME_HEADER_BYTES as u16).to_le_bytes());
    bytes[8..12].copy_from_slice(&width.to_le_bytes());
    bytes[12..16].copy_from_slice(&height.to_le_bytes());
    bytes[16..20].copy_from_slice(&FOURCC_NV12.to_le_bytes());
    bytes[20..24].copy_from_slice(&(nv12_bytes(width, height) as u32).to_le_bytes());
    bytes[24..32].copy_from_slice(&sequence.to_le_bytes());
    bytes[32..40].copy_from_slice(&time_100ns.to_le_bytes());
    bytes
}

/// A frame header as the media source reads it. For tests and tools that stand in for the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub width: u32,
    pub height: u32,
    pub payload_bytes: u32,
    pub sequence: u64,
    pub time_100ns: i64,
}

/// Read a frame header. `None` if it is not one this version understands.
pub fn parse_frame_header(bytes: &[u8; FRAME_HEADER_BYTES]) -> Option<FrameHeader> {
    let header = FrameHeader {
        width: u32_at(bytes, 8),
        height: u32_at(bytes, 12),
        payload_bytes: u32_at(bytes, 20),
        sequence: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        time_100ns: i64::from_le_bytes(bytes[32..40].try_into().ok()?),
    };
    let sensible = u32_at(bytes, 0) == MAGIC_FRAME
        && u16::from_le_bytes([bytes[4], bytes[5]]) == VERSION
        && u16::from_le_bytes([bytes[6], bytes[7]]) as usize == FRAME_HEADER_BYTES
        && u32_at(bytes, 16) == FOURCC_NV12
        && size_is_sensible(header.width, header.height)
        && header.payload_bytes as usize == nv12_bytes(header.width, header.height)
        && header.sequence != 0;
    sensible.then_some(header)
}
