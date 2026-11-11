use std::io::Cursor;

use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{DynamicImage, ImageBuffer, ImageDecoder, ImageReader, Limits, Rgba};

use crate::MediaError;

/// Sources wider or taller than this are rejected before any pixel data is decoded.
const MAX_SOURCE_DIM: u32 = 16_384;
/// Upper bound on decoder memory (and thus on source area: ~134 Mpx at 4 bytes/px).
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
/// Output ceiling regardless of what the caller asks for.
pub(crate) const MAX_OUTPUT_DIM: u32 = 4096;
/// Below this scale factor a box-filter thumbnail is faster than a convolution resize
/// and aliases less.
const THUMBNAIL_SCALE: f64 = 0.5;

pub(crate) struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes the first frame of `bytes` (animated GIF/WebP included), applies EXIF
/// orientation, and downscales so the longer side is at most `max_dim` (0 = no request
/// limit beyond [`MAX_OUTPUT_DIM`]). Never upscales.
pub(crate) fn decode(bytes: &[u8], max_dim: u32) -> Result<Pixels, MediaError> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| MediaError::Decode(e.to_string()))?
        .into_decoder()
        .map_err(|e| MediaError::Decode(e.to_string()))?;

    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 || w > MAX_SOURCE_DIM || h > MAX_SOURCE_DIM || u64::from(w) * u64::from(h) * 4 > MAX_ALLOC {
        return Err(MediaError::Dimensions(w, h));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    limits.max_alloc = Some(MAX_ALLOC);
    decoder.set_limits(limits).map_err(|e| MediaError::Decode(e.to_string()))?;

    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| MediaError::Decode(e.to_string()))?;
    img.apply_orientation(orientation);

    let limit = if max_dim == 0 { MAX_OUTPUT_DIM } else { max_dim.min(MAX_OUTPUT_DIM) };
    let has_alpha = img.color().has_alpha();
    Ok(downscale(img.into_rgba8(), limit, has_alpha))
}

fn downscale(img: ImageBuffer<Rgba<u8>, Vec<u8>>, limit: u32, has_alpha: bool) -> Pixels {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= limit {
        return Pixels { width: w, height: h, rgba: img.into_raw() };
    }
    let scale = f64::from(limit) / f64::from(longest);
    let nw = ((f64::from(w) * scale).round() as u32).clamp(1, limit);
    let nh = ((f64::from(h) * scale).round() as u32).clamp(1, limit);

    let mut img = img;
    // Filtering straight alpha bleeds the (often black) colour of transparent pixels into edges.
    if has_alpha {
        premultiply(&mut img);
    }
    let mut out = if scale < THUMBNAIL_SCALE {
        imageops::thumbnail(&img, nw, nh)
    } else {
        imageops::resize(&img, nw, nh, FilterType::CatmullRom)
    };
    if has_alpha {
        unpremultiply(&mut out);
    }
    Pixels { width: nw, height: nh, rgba: out.into_raw() }
}

fn premultiply(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    for px in img.pixels_mut() {
        let a = u32::from(px.0[3]);
        if a < 255 {
            for c in &mut px.0[..3] {
                *c = ((u32::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
}

fn unpremultiply(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    for px in img.pixels_mut() {
        let a = u32::from(px.0[3]);
        if a == 0 {
            px.0 = [0; 4];
        } else if a < 255 {
            for c in &mut px.0[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use image::codecs::gif::GifEncoder;
    use image::{Frame, ImageFormat, Rgb, RgbImage, RgbaImage};

    use super::*;

    fn encode(img: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        encode(&DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba(rgba))), ImageFormat::Png)
    }

    #[test]
    fn downscale_keeps_aspect_ratio_and_respects_max_dim() {
        let out = decode(&solid(400, 200, [10, 20, 30, 255]), 100).unwrap();
        assert_eq!((out.width, out.height), (100, 50));
        assert_eq!(out.rgba.len(), 100 * 50 * 4);

        let out = decode(&solid(200, 400, [10, 20, 30, 255]), 150).unwrap();
        assert_eq!((out.width, out.height), (75, 150));

        // Gentle ratio takes the convolution path.
        let out = decode(&solid(130, 100, [10, 20, 30, 255]), 100).unwrap();
        assert_eq!((out.width, out.height), (100, 77));
    }

    #[test]
    fn extreme_aspect_ratio_never_collapses_to_zero() {
        let out = decode(&solid(1000, 2, [0, 0, 0, 255]), 10).unwrap();
        assert_eq!((out.width, out.height), (10, 1));
    }

    #[test]
    fn never_upscales() {
        let out = decode(&solid(40, 20, [1, 2, 3, 255]), 100).unwrap();
        assert_eq!((out.width, out.height), (40, 20));
        assert_eq!(&out.rgba[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn zero_max_dim_means_no_request_limit() {
        let out = decode(&solid(300, 100, [1, 2, 3, 255]), 0).unwrap();
        assert_eq!((out.width, out.height), (300, 100));
    }

    #[test]
    fn jpeg_is_decoded_to_opaque_rgba() {
        let rgb = RgbImage::from_pixel(32, 32, Rgb([200, 100, 50]));
        let out = decode(&encode(&DynamicImage::ImageRgb8(rgb), ImageFormat::Jpeg), 16).unwrap();
        assert_eq!((out.width, out.height), (16, 16));
        assert!(out.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn transparent_pixels_do_not_darken_downscaled_edges() {
        let mut img = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        for y in 0..64 {
            for x in 32..64 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let out = decode(&encode(&DynamicImage::ImageRgba8(img), ImageFormat::Png), 10).unwrap();
        let visible: Vec<_> = out.rgba.chunks_exact(4).filter(|p| p[3] > 0).collect();
        assert!(!visible.is_empty());
        assert!(visible.iter().all(|p| p[..3].iter().all(|&c| c >= 254)), "{visible:?}");
    }

    #[test]
    fn animated_gif_yields_first_frame() {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                enc.encode_frame(Frame::new(RgbaImage::from_pixel(8, 6, Rgba(color)))).unwrap();
            }
        }
        let img = decode(&out, 100).unwrap();
        assert_eq!((img.width, img.height), (8, 6));
        assert_eq!(&img.rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn corrupt_and_empty_input_are_errors_not_panics() {
        assert!(matches!(decode(b"definitely not an image", 100), Err(MediaError::Decode(_))));
        assert!(decode(&[], 100).is_err());
        let mut png = solid(32, 32, [1, 2, 3, 255]);
        png.truncate(png.len() / 2);
        assert!(decode(&png, 100).is_err());
    }

    #[test]
    fn oversized_dimensions_are_rejected_without_decoding() {
        // Valid 1x1 PNG with the IHDR size rewritten to 20000x20000 (and its CRC fixed up).
        let mut png = solid(1, 1, [0, 0, 0, 255]);
        png[16..20].copy_from_slice(&20_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&20_000u32.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(matches!(decode(&png, 100), Err(MediaError::Dimensions(20_000, 20_000))));
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
}
