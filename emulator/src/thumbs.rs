//! Cover thumbnails on disk: a tiny header followed by raw RGBA pixels, so
//! loading one is a single read with no decoding.

use std::path::PathBuf;

use crate::config::file_key;
use crate::platform;
use crate::swfinfo::Image;

/// Stored thumbnails fit in this box.
pub const THUMB_W: u32 = 400;
pub const THUMB_H: u32 = 226;

const MAGIC: &[u8; 4] = b"FVT1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbKind {
    /// Pulled from a bitmap embedded in the SWF.
    Extracted = 0,
    /// Captured while playing.
    Screenshot = 1,
}

pub fn path(game_key: &str) -> PathBuf {
    platform::thumbs_dir().join(format!("{}.fvt", file_key(game_key)))
}

pub fn save(game_key: &str, img: &Image, kind: ThumbKind) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(12 + img.rgba.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(img.w as u16).to_le_bytes());
    out.extend_from_slice(&(img.h as u16).to_le_bytes());
    out.extend_from_slice(&[kind as u8, 0, 0, 0]);
    out.extend_from_slice(&img.rgba);
    std::fs::write(path(game_key), out)
}

pub fn load(game_key: &str) -> Option<(Image, ThumbKind)> {
    let data = std::fs::read(path(game_key)).ok()?;
    if data.len() < 12 || &data[..4] != MAGIC {
        return None;
    }
    let w = u16::from_le_bytes([data[4], data[5]]) as u32;
    let h = u16::from_le_bytes([data[6], data[7]]) as u32;
    let kind = if data[8] == 1 { ThumbKind::Screenshot } else { ThumbKind::Extracted };
    let rgba = data[12..].to_vec();
    if rgba.len() != (w * h * 4) as usize {
        return None;
    }
    Some((Image { w, h, rgba }, kind))
}
