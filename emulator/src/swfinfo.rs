//! Cheap SWF inspection for the library: header metadata without inflating
//! the whole file, and a best-guess cover image pulled from embedded bitmaps.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SwfInfo {
    pub version: u8,
    /// 'F' (none), 'C' (zlib) or 'Z' (LZMA).
    pub compression: char,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub frames: u16,
    pub as3: bool,
    pub background: Option<[u8; 3]>,
    /// `dc:title` from the Metadata tag, when the author set one.
    pub title: Option<String>,
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
}

impl BitReader<'_> {
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            let byte = *self.data.get(self.pos)?;
            let b = (byte >> (7 - self.bit)) & 1;
            v = (v << 1) | b as u32;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Some(v)
    }

    fn sbits(&mut self, n: u32) -> Option<i32> {
        let v = self.bits(n)?;
        Some(if n > 0 && v & (1 << (n - 1)) != 0 { v as i32 - (1 << n) } else { v as i32 })
    }

    fn byte_pos(&self) -> usize {
        if self.bit == 0 { self.pos } else { self.pos + 1 }
    }
}

fn u16le(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

fn u32le(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

/// Iterates `(code, body)` over a tag stream, stopping at the first
/// truncated tag.
fn tags(data: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        let head = u16le(data, pos)?;
        pos += 2;
        let code = head >> 6;
        let mut len = (head & 0x3f) as usize;
        if len == 0x3f {
            len = u32le(data, pos)? as usize;
            pos += 4;
        }
        let body = data.get(pos..pos.checked_add(len)?)?;
        pos += len;
        if code == 0 { None } else { Some((code, body)) }
    })
}

fn extract_title(xml: &str) -> Option<String> {
    let start = xml.find("<dc:title")?;
    let rest = &xml[start..];
    let open_end = rest.find('>')? + 1;
    let close = rest.find("</dc:title>")?;
    let inner = rest.get(open_end..close)?;
    // Titles are sometimes wrapped in rdf:Alt/rdf:li.
    let text = match inner.find("<rdf:li") {
        Some(i) => {
            let li = &inner[i..];
            let s = li.find('>')? + 1;
            let e = li.find("</rdf:li>")?;
            li.get(s..e)?
        }
        None => inner,
    };
    let t = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    let t = t.trim();
    (!t.is_empty() && t.len() < 120).then(|| t.to_owned())
}

/// Parses the header and the first few tags. Only the first 64 KiB of the
/// movie body is inflated for zlib files, so this is fast even for 100 MB
/// games.
pub fn read_info(path: &Path) -> Result<SwfInfo, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut head = [0u8; 8];
    file.read_exact(&mut head).map_err(|_| "File is too small to be a SWF".to_string())?;
    let compression = head[0] as char;
    if &head[1..3] != b"WS" || !matches!(compression, 'F' | 'C' | 'Z') {
        return Err("Not a SWF file".into());
    }
    let version = head[3];

    const PREFIX: u64 = 64 * 1024;
    let (mut info, tag_data) = match compression {
        'F' | 'C' => {
            let mut body = Vec::new();
            if compression == 'F' {
                file.take(PREFIX).read_to_end(&mut body).map_err(|e| e.to_string())?;
            } else {
                // A truncated read is fine: we only need the prefix.
                let _ = flate2::read::ZlibDecoder::new(file).take(PREFIX).read_to_end(&mut body);
            }
            let mut br = BitReader { data: &body, pos: 0, bit: 0 };
            let truncated = || "Truncated header".to_string();
            let nbits = br.bits(5).ok_or_else(truncated)?;
            let xmin = br.sbits(nbits).ok_or_else(truncated)?;
            let xmax = br.sbits(nbits).ok_or_else(truncated)?;
            let ymin = br.sbits(nbits).ok_or_else(truncated)?;
            let ymax = br.sbits(nbits).ok_or_else(truncated)?;
            let p = br.byte_pos();
            let rate = u16le(&body, p).ok_or_else(truncated)?;
            let frames = u16le(&body, p + 2).ok_or_else(truncated)?;
            let info = SwfInfo {
                version,
                compression,
                width: ((xmax - xmin).max(0) / 20) as u32,
                height: ((ymax - ymin).max(0) / 20) as u32,
                fps: (rate >> 8) as f32 + (rate & 0xff) as f32 / 256.0,
                frames,
                as3: false,
                background: None,
                title: None,
            };
            let start = (p + 4).min(body.len());
            (info, body[start..].to_vec())
        }
        _ => {
            // LZMA has no cheap streaming path here; decode the whole thing.
            let mut all = head.to_vec();
            file.read_to_end(&mut all).map_err(|e| e.to_string())?;
            let buf = swf::decompress_swf(&all[..]).map_err(|e| e.to_string())?;
            let h = &buf.header;
            let stage = h.stage_size();
            let info = SwfInfo {
                version,
                compression,
                width: stage.width().to_pixels().max(0.0) as u32,
                height: stage.height().to_pixels().max(0.0) as u32,
                fps: h.frame_rate().to_f32(),
                frames: h.num_frames(),
                as3: h.is_action_script_3(),
                background: None,
                title: None,
            };
            // `data` starts at the first tag (the header is stripped).
            let mut data = buf.data;
            data.truncate(PREFIX as usize);
            (info, data)
        }
    };

    for (i, (code, tag)) in tags(&tag_data).enumerate() {
        match code {
            69 => info.as3 = tag.first().is_some_and(|f| f & 0x08 != 0),
            77 => info.title = extract_title(&String::from_utf8_lossy(tag)),
            9 if tag.len() >= 3 => info.background = Some([tag[0], tag[1], tag[2]]),
            // Display list tags mean the header tags are behind us.
            1 | 26 | 70 => break,
            _ => {}
        }
        if i > 64 {
            break;
        }
    }
    Ok(info)
}

/// An RGBA image, not premultiplied.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Largest movie we are willing to fully inflate just to find a cover.
const MAX_PREVIEW_SOURCE: u64 = 48 * 1024 * 1024;

/// Picks the embedded bitmap most likely to be a title screen or key art:
/// big, roughly screen-shaped, and ideally close to the stage size.
pub fn extract_cover(path: &Path, stage: (u32, u32)) -> Option<Image> {
    if std::fs::metadata(path).ok()?.len() > MAX_PREVIEW_SOURCE {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let buf = swf::decompress_swf(&data[..]).ok()?;
    drop(data);
    // `data` starts at the first tag; the header was already stripped.
    let body = &buf.data;

    let (sw, sh) = (stage.0.max(1) as f32, stage.1.max(1) as f32);
    let score = |w: u32, h: u32| -> f32 {
        if w < 96 || h < 64 {
            return 0.0;
        }
        let aspect = w as f32 / h as f32;
        if !(0.9..=2.6).contains(&aspect) {
            return 0.0;
        }
        let area = (w * h) as f32;
        let coverage = ((w as f32 / sw).min(1.0)) * ((h as f32 / sh).min(1.0));
        let stage_like = if coverage > 0.6 { 2.0 } else { 1.0 };
        area.min(sw * sh * 1.5) * stage_like
    };

    let mut best: Option<(f32, u16, &[u8])> = None;
    for (code, tag) in tags(body) {
        let dims = match code {
            // DefineBitsJPEG2 / DefineBitsJPEG3 (image data after the id / alpha offset).
            21 | 35 => {
                let img_start = if code == 21 { 2 } else { 6 };
                let img = tag.get(img_start..)?;
                ruffle_render::utils::decode_define_bits_jpeg_dimensions(img)
                    .ok()
                    .map(|(w, h)| (w as u32, h as u32))
            }
            // DefineBitsLossless / 2
            20 | 36 => Some((u16le(tag, 3)? as u32, u16le(tag, 5)? as u32)),
            _ => None,
        };
        if let Some((w, h)) = dims {
            let s = score(w, h);
            if s > 0.0 && best.is_none_or(|(bs, ..)| s > bs) {
                best = Some((s, code, tag));
            }
        }
    }

    let (_, code, tag) = best?;
    let bitmap = match code {
        21 => ruffle_render::utils::decode_define_bits_jpeg(tag.get(2..)?, None).ok()?,
        35 => {
            let alpha_off = u32le(tag, 2)? as usize;
            let img = tag.get(6..6 + alpha_off)?;
            let alpha = tag.get(6 + alpha_off..);
            ruffle_render::utils::decode_define_bits_jpeg(img, alpha).ok()?
        }
        _ => {
            let mut reader = swf::read::Reader::new(tag, buf.header.version());
            let lossless = reader.read_define_bits_lossless(if code == 20 { 1 } else { 2 }).ok()?;
            ruffle_render::utils::decode_define_bits_lossless(&lossless).ok()?
        }
    };

    let (w, h) = (bitmap.width(), bitmap.height());
    let rgba = bitmap.to_rgba();
    let mut data = rgba.data().to_vec();
    // Ruffle decodes to premultiplied alpha; flatten onto black for display.
    for px in data.chunks_exact_mut(4) {
        px[3] = 255;
    }
    Some(Image { w, h, rgba: data })
}

/// Box-filter downscale so the image fits within `max_w` x `max_h`.
pub fn downscale(img: &Image, max_w: u32, max_h: u32) -> Image {
    let scale = (max_w as f32 / img.w as f32).min(max_h as f32 / img.h as f32).min(1.0);
    let w = ((img.w as f32 * scale).round() as u32).max(1);
    let h = ((img.h as f32 * scale).round() as u32).max(1);
    if w == img.w && h == img.h {
        return Image { w, h, rgba: img.rgba.clone() };
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let sy0 = (y as f32 / h as f32 * img.h as f32) as u32;
        let sy1 = (((y + 1) as f32 / h as f32 * img.h as f32) as u32).clamp(sy0 + 1, img.h);
        for x in 0..w {
            let sx0 = (x as f32 / w as f32 * img.w as f32) as u32;
            let sx1 = (((x + 1) as f32 / w as f32 * img.w as f32) as u32).clamp(sx0 + 1, img.w);
            let mut acc = [0u32; 4];
            for sy in sy0..sy1 {
                let row = (sy * img.w) as usize;
                for sx in sx0..sx1 {
                    let i = (row + sx as usize) * 4;
                    for c in 0..4 {
                        acc[c] += img.rgba[i + c] as u32;
                    }
                }
            }
            let n = (sy1 - sy0) * (sx1 - sx0);
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    Image { w, h, rgba: out }
}

