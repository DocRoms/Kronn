//! Reads real dimensions out of a produced media file.
//!
//! Necessary because the provider does not honour the requested geometry: a
//! "480p" / "16:9" video request came back as 864x496 (ratio 1.742, not 1.778)
//! and 5042 ms for a 5 s request.
//! Sizing a player from the REQUEST therefore produces black bars or stretch.
//!
//! Best-effort by design: an unreadable header leaves the fields absent rather
//! than guessed, and callers already treat absence as "unknown".

use crate::models::MediaRendered;

/// Minimal MP4 box walk: `moov/mvhd` for duration, the first `trak/tkhd` with
/// a non-zero size for dimensions. No dependency, and unknown boxes are simply
/// skipped.
pub fn probe_mp4(bytes: &[u8]) -> MediaRendered {
    let mut out = MediaRendered::default();
    let Some(moov) = find_box(bytes, b"moov") else {
        return out;
    };

    if let Some(mvhd) = find_box(moov, b"mvhd") {
        if let Some((timescale, duration)) = read_mvhd(mvhd) {
            if timescale > 0 {
                out.duration_ms = Some(duration.saturating_mul(1000) / u64::from(timescale));
            }
        }
    }

    // Several traks exist (video, audio, …); the first with real dimensions is
    // the visual one.
    let mut cursor = moov;
    while let Some((trak, rest)) = next_box(cursor, b"trak") {
        if let Some(tkhd) = find_box(trak, b"tkhd") {
            if let Some((width, height)) = read_tkhd(tkhd) {
                if width > 0 && height > 0 {
                    out.width = Some(width);
                    out.height = Some(height);
                    break;
                }
            }
        }
        cursor = rest;
    }
    out
}

/// PNG and JPEG headers only — the formats these providers return.
pub fn probe_image(bytes: &[u8]) -> MediaRendered {
    if let Some(size) = png_size(bytes) {
        return MediaRendered {
            width: Some(size.0),
            height: Some(size.1),
            duration_ms: None,
        };
    }
    if let Some(size) = jpeg_size(bytes) {
        return MediaRendered {
            width: Some(size.0),
            height: Some(size.1),
            duration_ms: None,
        };
    }
    MediaRendered::default()
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset + 8)?;
    Some(u64::from_be_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

/// Returns the body of the first box of `kind` at this level, plus the bytes
/// that follow it.
fn next_box<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> Option<(&'a [u8], &'a [u8])> {
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let size = u32_at(bytes, offset)? as usize;
        let name = bytes.get(offset + 4..offset + 8)?;
        let (body_start, box_size) = match size {
            // 64-bit size in the eight bytes that follow.
            1 => (offset + 16, u64_at(bytes, offset + 8)? as usize),
            // Extends to the end of the container.
            0 => (offset + 8, bytes.len() - offset),
            _ => (offset + 8, size),
        };
        // A malformed size must not loop forever nor slice out of range.
        if box_size < 8 || offset + box_size > bytes.len() || body_start > offset + box_size {
            return None;
        }
        if name == kind {
            return Some((
                &bytes[body_start..offset + box_size],
                &bytes[offset + box_size..],
            ));
        }
        offset += box_size;
    }
    None
}

fn find_box<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    next_box(bytes, kind).map(|(body, _)| body)
}

/// `(timescale, duration)` from an `mvhd`, version 0 or 1.
fn read_mvhd(body: &[u8]) -> Option<(u32, u64)> {
    let version = *body.first()?;
    // 1 byte version + 3 bytes flags, then the time fields.
    let base = 4;
    if version == 1 {
        // creation(8) + modification(8) then timescale(4) + duration(8)
        Some((u32_at(body, base + 16)?, u64_at(body, base + 20)?))
    } else {
        // creation(4) + modification(4) then timescale(4) + duration(4)
        Some((
            u32_at(body, base + 8)?,
            u64_at(body, base + 12).unwrap_or(0) >> 32,
        ))
    }
}

/// `(width, height)` from a `tkhd`, stored as 16.16 fixed point.
fn read_tkhd(body: &[u8]) -> Option<(u32, u32)> {
    let version = *body.first()?;
    let base = 4 + if version == 1 { 32 } else { 20 };
    let width = u32_at(body, base + 52)? >> 16;
    let height = u32_at(body, base + 56)? >> 16;
    Some((width, height))
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some((u32_at(bytes, 16)?, u32_at(bytes, 20)?))
}

fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut offset = 2usize;
    while offset + 9 < bytes.len() {
        if bytes[offset] != 0xFF {
            offset += 1;
            continue;
        }
        let marker = bytes[offset + 1];
        // Start-of-frame markers carry the dimensions; SOF4/SOF8/SOF12 do not.
        let is_sof =
            (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
        let length = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        if is_sof {
            let height = u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]);
            let width = u16::from_be_bytes([bytes[offset + 7], bytes[offset + 8]]);
            return Some((u32::from(width), u32::from(height)));
        }
        if length < 2 {
            return None;
        }
        offset += 2 + length;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a box: 4-byte size, 4-byte name, body.
    fn mp4_box(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(name);
        out.extend_from_slice(body);
        out
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut body = vec![0u8; 4]; // version 0 + flags
        body.extend_from_slice(&0u32.to_be_bytes()); // creation
        body.extend_from_slice(&0u32.to_be_bytes()); // modification
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&[0u8; 8]);
        mp4_box(b"mvhd", &body)
    }

    fn tkhd_v0(width: u32, height: u32) -> Vec<u8> {
        let mut body = vec![0u8; 4 + 20];
        body.extend_from_slice(&[0u8; 52]);
        body.extend_from_slice(&(width << 16).to_be_bytes());
        body.extend_from_slice(&(height << 16).to_be_bytes());
        mp4_box(b"tkhd", &body)
    }

    #[test]
    fn reads_the_real_geometry_of_a_video() {
        // The shape actually produced for a "480p 16:9" request.
        let trak = mp4_box(b"trak", &tkhd_v0(864, 496));
        let mut moov_body = mvhd_v0(1000, 5040);
        moov_body.extend_from_slice(&trak);
        let file = mp4_box(b"moov", &moov_body);

        let probed = probe_mp4(&file);
        assert_eq!(probed.width, Some(864), "not the requested 854");
        assert_eq!(probed.height, Some(496), "not the requested 480");
        assert_eq!(probed.duration_ms, Some(5040));
    }

    #[test]
    fn skips_a_leading_track_without_dimensions() {
        // Audio-like trak first: it must not win over the visual one.
        let silent = mp4_box(b"trak", &tkhd_v0(0, 0));
        let visual = mp4_box(b"trak", &tkhd_v0(1920, 1080));
        let mut moov_body = mvhd_v0(600, 3000);
        moov_body.extend_from_slice(&silent);
        moov_body.extend_from_slice(&visual);
        let file = mp4_box(b"moov", &moov_body);

        let probed = probe_mp4(&file);
        assert_eq!(probed.width, Some(1920));
        assert_eq!(probed.duration_ms, Some(5000));
    }

    #[test]
    fn an_unreadable_file_leaves_the_fields_absent_rather_than_guessed() {
        for junk in [&b""[..], &b"not an mp4"[..], &[0u8; 64][..]] {
            let probed = probe_mp4(junk);
            assert_eq!(
                probed,
                MediaRendered::default(),
                "junk must not yield numbers"
            );
        }
    }

    #[test]
    fn a_malformed_box_size_does_not_hang_or_panic() {
        // Size 0xFFFFFFFF overruns the buffer; size 3 is below the header.
        let mut bomb = 0xFFFF_FFFFu32.to_be_bytes().to_vec();
        bomb.extend_from_slice(b"moov");
        assert_eq!(probe_mp4(&bomb), MediaRendered::default());

        let mut tiny = 3u32.to_be_bytes().to_vec();
        tiny.extend_from_slice(b"moov");
        assert_eq!(probe_mp4(&tiny), MediaRendered::default());
    }

    #[test]
    fn reads_png_dimensions() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1024u32.to_be_bytes());
        png.extend_from_slice(&768u32.to_be_bytes());
        let probed = probe_image(&png);
        assert_eq!((probed.width, probed.height), (Some(1024), Some(768)));
        assert!(
            probed.duration_ms.is_none(),
            "a still image has no duration"
        );
    }

    #[test]
    fn reads_jpeg_dimensions_and_ignores_a_non_sof_marker() {
        let mut jpeg = vec![0xFF, 0xD8];
        // APP0 segment, skipped.
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        // SOF0 with height 600, width 800.
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend_from_slice(&600u16.to_be_bytes());
        jpeg.extend_from_slice(&800u16.to_be_bytes());
        jpeg.extend_from_slice(&[0u8; 8]);
        let probed = probe_image(&jpeg);
        assert_eq!((probed.width, probed.height), (Some(800), Some(600)));
    }

    #[test]
    fn a_non_image_yields_nothing() {
        assert_eq!(probe_image(b"hello"), MediaRendered::default());
        assert_eq!(probe_image(&[]), MediaRendered::default());
    }
    /// Opt-in check against a real provider file, which cannot be versioned
    /// (a 5 s clip weighs ~1.5 MB). Run it with:
    ///   KRONN_MEDIA_PROBE_MP4=/path/to/clip.mp4 \
    ///   KRONN_MEDIA_PROBE_EXPECT=864x496x5042 cargo test --lib media_probe
    #[test]
    fn matches_a_real_provider_file_when_one_is_supplied() {
        let Ok(path) = std::env::var("KRONN_MEDIA_PROBE_MP4") else {
            return; // no fixture supplied: nothing to assert
        };
        let bytes = std::fs::read(&path).expect("fixture readable");
        let probed = probe_mp4(&bytes);

        if let Ok(expected) = std::env::var("KRONN_MEDIA_PROBE_EXPECT") {
            let parts: Vec<u64> = expected
                .split('x')
                .map(|p| p.parse().expect("WxHxDURATION_MS"))
                .collect();
            assert_eq!(probed.width, Some(parts[0] as u32), "width");
            assert_eq!(probed.height, Some(parts[1] as u32), "height");
            assert_eq!(probed.duration_ms, Some(parts[2]), "duration");
        } else {
            assert!(probed.width.is_some(), "a real file must yield dimensions");
        }
        eprintln!("probed {path}: {probed:?}");
    }
}
