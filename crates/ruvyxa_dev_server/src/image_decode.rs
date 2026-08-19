//! Format detection and bounded decoding for the three formats Ruvyxa accepts.
//!
//! This replaces the `image` crate's `ImageReader`/`DynamicImage` surface with
//! direct calls to the same decoders `image` itself wraps — `png`, `zune-jpeg`,
//! and `image-webp`. The reason is dependency shape, not capability: `image`
//! declares `avif` and `exr` as optional features, and Cargo records a
//! dependency's optional deps in `Cargo.lock` whether or not the feature is on,
//! so `rav1e`/`pulp` and their unmaintained `paste` macro sat in the lockfile
//! permanently. Calling the decoders directly removes them from the graph.
//!
//! Every decoder here normalizes to RGB8 or RGBA8, which is exactly what
//! `Pixels` wants, so the layout conversion `DynamicImage` needed disappears.

use std::io::Cursor;

use crate::image_codec::PixelLayout;

/// A decoded image, already in a layout the resizer and WebP encoder accept.
pub struct Decoded {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
}

/// Why an image could not be decoded.
///
/// `TooLarge` is a variant rather than a message because callers answer it
/// differently: the runtime image endpoint returns 413 for it and 400 for
/// everything else. Recovering that from formatted text would be the same
/// stringly-typed coupling this crate removed from the action replay guard, and
/// the alternative — re-reading the header at the call site to decide — parsed
/// the same prefix a second time on every request.
#[derive(Debug)]
pub enum DecodeError {
    /// The magic bytes matched no format this build accepts.
    Unsupported,
    /// The header declares more pixels than the caller allowed.
    TooLarge {
        width: u32,
        height: u32,
        max_pixels: u64,
    },
    /// The bytes claim a supported format but do not decode as one.
    Malformed(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("unrecognized image format"),
            Self::TooLarge {
                width,
                height,
                max_pixels,
            } => write!(
                formatter,
                "image is {width}x{height}, above the {max_pixels} pixel budget"
            ),
            Self::Malformed(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for DecodeError {}

type Result<T> = std::result::Result<T, DecodeError>;

fn fail(detail: impl std::fmt::Display) -> DecodeError {
    DecodeError::Malformed(detail.to_string())
}

/// The formats runtime and build-time optimization accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

/// Identify a format from its magic bytes.
///
/// Content sniffing, not the file extension: the extension is a caller's claim
/// about an untrusted file, and the decoder is what has to be right.
pub fn sniff(source: &[u8]) -> Option<Format> {
    if source.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some(Format::Png);
    }
    if source.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(Format::Jpeg);
    }
    if source.len() >= 12 && source.starts_with(b"RIFF") && source[8..12] == *b"WEBP" {
        return Some(Format::Webp);
    }
    None
}

/// Dimensions from the header, without decoding pixel data.
///
/// This is the only thing that can be known cheaply about an untrusted image:
/// the header is a fixed-size prefix, while decoding allocates
/// `width * height * channels` bytes before anything gets a chance to object.
pub fn header_dimensions(source: &[u8]) -> Result<(u32, u32)> {
    match sniff(source).ok_or(DecodeError::Unsupported)? {
        Format::Png => {
            let reader = png::Decoder::new(Cursor::new(source))
                .read_info()
                .map_err(fail)?;
            let info = reader.info();
            Ok((info.width, info.height))
        }
        Format::Jpeg => {
            let mut decoder =
                zune_jpeg::JpegDecoder::new(zune_core::bytestream::ZCursor::new(source));
            decoder.decode_headers().map_err(fail)?;
            let (width, height) = decoder
                .dimensions()
                .ok_or_else(|| fail("JPEG header declares no dimensions"))?;
            Ok((to_u32(width)?, to_u32(height)?))
        }
        Format::Webp => {
            let decoder = image_webp::WebPDecoder::new(Cursor::new(source)).map_err(fail)?;
            Ok(decoder.dimensions())
        }
    }
}

fn to_u32(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| fail("image dimension does not fit in u32"))
}

/// Decode an image only if it fits inside `max_pixels`.
///
/// The budget is checked against the header first and enforced again by each
/// decoder's own allocation limit. Checking only after a full decode is
/// checking after the damage: a 20 MB PNG can legally declare 50000x50000, and
/// the decoder will have committed ten gigabytes to the heap before the caller
/// ever sees a dimension to reject. The header covers the honest case cheaply;
/// the decoder limit covers a header that lies about what follows.
pub fn decode_within_pixel_budget(source: &[u8], max_pixels: u64) -> Result<Decoded> {
    // Four channels is the widest layout `Decoded` keeps, and a decoder needs
    // room for one such buffer. The slack absorbs per-decoder scratch without
    // widening the budget enough to matter.
    let max_alloc = max_pixels.saturating_mul(4).saturating_add(1 << 20);

    // The header check happens inside each decoder, against the reader it has
    // already built. Calling `header_dimensions` here instead re-sniffed the
    // magic bytes and parsed the header a second time, and the decoder then
    // parsed it a third — three passes over the same prefix for one answer.
    // That is invisible on a 6000x4000 photo and is most of the per-image cost
    // on a directory of icons.
    match sniff(source).ok_or(DecodeError::Unsupported)? {
        Format::Png => decode_png(source, max_pixels, max_alloc),
        Format::Jpeg => decode_jpeg(source, max_pixels),
        Format::Webp => decode_webp(source, max_pixels, max_alloc),
    }
}

/// Reject a header that declares more pixels than the caller allowed.
fn check_budget(width: u32, height: u32, max_pixels: u64) -> Result<()> {
    if u64::from(width) * u64::from(height) > max_pixels {
        return Err(DecodeError::TooLarge {
            width,
            height,
            max_pixels,
        });
    }
    Ok(())
}

/// PNG, normalized to 8-bit RGB or RGBA.
///
/// `normalize_to_color8` expands palette, sub-byte grayscale, and `tRNS`
/// transparency, and strips 16-bit samples down to 8 — so the output is one of
/// four color types rather than the full PNG matrix. Grayscale still arrives as
/// grayscale, and is widened here.
fn decode_png(source: &[u8], max_pixels: u64, max_alloc: u64) -> Result<Decoded> {
    let mut decoder = png::Decoder::new(Cursor::new(source));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    decoder.set_limits(png::Limits {
        bytes: usize::try_from(max_alloc).unwrap_or(usize::MAX),
    });
    let mut reader = decoder.read_info().map_err(fail)?;
    let info = reader.info();
    check_budget(info.width, info.height, max_pixels)?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| fail("PNG output buffer size overflows"))?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(fail)?;
    buffer.truncate(info.buffer_size());

    let (data, layout) = match info.color_type {
        png::ColorType::Rgb => (buffer, PixelLayout::Rgb8),
        png::ColorType::Rgba => (buffer, PixelLayout::Rgba8),
        png::ColorType::Grayscale => (widen(&buffer, 1, false), PixelLayout::Rgb8),
        png::ColorType::GrayscaleAlpha => (widen(&buffer, 2, true), PixelLayout::Rgba8),
        png::ColorType::Indexed => {
            return Err(fail("PNG palette was not expanded by the decoder"));
        }
    };
    Ok(Decoded {
        data,
        width: info.width,
        height: info.height,
        layout,
    })
}

/// Grayscale to RGB, preserving alpha when the source carries it.
///
/// Dropping to RGB for `GrayscaleAlpha` would silently flatten transparency the
/// original file had, so the alpha channel is carried across rather than
/// discarded.
fn widen(buffer: &[u8], channels: usize, has_alpha: bool) -> Vec<u8> {
    let out_channels = if has_alpha { 4 } else { 3 };
    let pixels = buffer.len() / channels;
    // Written into a pre-sized buffer through fixed-width chunks rather than
    // grown by `extend_from_slice`/`push` per pixel: the destination stride is a
    // compile-time constant inside each branch, so the loop vectorizes and the
    // bounds checks fold away. The pushing version reallocated as it went and
    // re-checked capacity on every channel.
    let mut out = vec![0u8; pixels * out_channels];
    if has_alpha {
        for (source, target) in buffer
            .chunks_exact(channels)
            .zip(out.chunks_exact_mut(out_channels))
        {
            let [luma, alpha] = [source[0], source[1]];
            target.copy_from_slice(&[luma, luma, luma, alpha]);
        }
    } else {
        for (source, target) in buffer
            .chunks_exact(channels)
            .zip(out.chunks_exact_mut(out_channels))
        {
            let luma = source[0];
            target.copy_from_slice(&[luma, luma, luma]);
        }
    }
    out
}

/// JPEG, decoded straight to RGB8.
///
/// `zune-jpeg` performs the YCbCr/CMYK/YCCK conversions internally and is asked
/// for RGB, so grayscale and CMYK sources arrive already widened. The pixel
/// budget is expressed through the decoder's own max width/height, which is the
/// limit this decoder exposes.
fn decode_jpeg(source: &[u8], max_pixels: u64) -> Result<Decoded> {
    let bound = usize::try_from(max_pixels).unwrap_or(usize::MAX);
    let options = zune_core::options::DecoderOptions::default()
        .jpeg_set_out_colorspace(zune_core::colorspace::ColorSpace::RGB)
        .set_max_width(bound)
        .set_max_height(bound);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
        zune_core::bytestream::ZCursor::new(source),
        options,
    );
    // Headers first, so an oversized image is refused before its pixel buffer
    // is allocated rather than after.
    decoder.decode_headers().map_err(fail)?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| fail("JPEG header declares no dimensions"))?;
    check_budget(to_u32(width)?, to_u32(height)?, max_pixels)?;
    let data = decoder.decode().map_err(fail)?;
    Ok(Decoded {
        data,
        width: to_u32(width)?,
        height: to_u32(height)?,
        layout: PixelLayout::Rgb8,
    })
}

/// WebP, decoded to RGB8 or RGBA8 depending on what the file carries.
fn decode_webp(source: &[u8], max_pixels: u64, max_alloc: u64) -> Result<Decoded> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(source)).map_err(fail)?;
    decoder.set_memory_limit(usize::try_from(max_alloc).unwrap_or(usize::MAX));
    let (width, height) = decoder.dimensions();
    check_budget(width, height, max_pixels)?;
    let layout = if decoder.has_alpha() {
        PixelLayout::Rgba8
    } else {
        PixelLayout::Rgb8
    };
    let size = decoder
        .output_buffer_size()
        .ok_or_else(|| fail("WebP output buffer size overflows"))?;
    let mut data = vec![0u8; size];
    decoder.read_image(&mut data).map_err(fail)?;
    Ok(Decoded {
        data,
        width,
        height,
        layout,
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    /// Encode a solid image as PNG in the given color type.
    ///
    /// Tests need real encoded bytes rather than a pixel buffer, because the
    /// behavior under test is the decoder's — palette expansion, 16-bit
    /// stripping, and grayscale widening all happen during decode.
    pub(crate) fn png(
        width: u32,
        height: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        sample: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(color);
            encoder.set_depth(depth);
            let mut writer = encoder.write_header().expect("fixture header");
            let pixels = (width * height) as usize;
            let data: Vec<u8> = sample
                .iter()
                .copied()
                .cycle()
                .take(pixels * sample.len())
                .collect();
            writer.write_image_data(&data).expect("fixture pixels");
        }
        out
    }

    /// Encode a solid RGB image as JPEG.
    pub(crate) fn jpeg(width: u16, height: u16, rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        let data: Vec<u8> = rgb
            .iter()
            .copied()
            .cycle()
            .take(usize::from(width) * usize::from(height) * 3)
            .collect();
        jpeg_encoder::Encoder::new(&mut out, 90)
            .encode(&data, width, height, jpeg_encoder::ColorType::Rgb)
            .expect("fixture jpeg");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_formats_from_magic_bytes_not_extensions() {
        assert_eq!(
            sniff(&fixtures::png(
                2,
                2,
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                &[1, 2, 3]
            )),
            Some(Format::Png)
        );
        assert_eq!(
            sniff(&fixtures::jpeg(8, 8, [10, 20, 30])),
            Some(Format::Jpeg)
        );
        assert_eq!(sniff(b"not an image at all"), None);
        // A RIFF container that is not WebP must not be claimed.
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
    }

    #[test]
    fn reads_header_dimensions_without_decoding() {
        let png_bytes = fixtures::png(7, 3, png::ColorType::Rgb, png::BitDepth::Eight, &[1, 2, 3]);
        assert_eq!(header_dimensions(&png_bytes).unwrap(), (7, 3));
        let jpeg_bytes = fixtures::jpeg(9, 4, [1, 2, 3]);
        assert_eq!(header_dimensions(&jpeg_bytes).unwrap(), (9, 4));
    }

    #[test]
    fn refuses_an_image_above_the_pixel_budget_before_decoding() {
        let png_bytes = fixtures::png(
            64,
            64,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[1, 2, 3],
        );
        // 64x64 is 4096 pixels, so a 4095 budget must reject and 4096 must not.
        assert!(decode_within_pixel_budget(&png_bytes, 4095).is_err());
        let decoded = decode_within_pixel_budget(&png_bytes, 4096).unwrap();
        assert_eq!((decoded.width, decoded.height), (64, 64));
    }

    /// The layout normalization that `DynamicImage` used to perform now happens
    /// inside the decoder, so it is held here rather than in `Pixels`.
    #[test]
    fn normalizes_every_png_color_type_to_rgb8_or_rgba8() {
        let cases: [(png::ColorType, &[u8], PixelLayout, usize); 4] = [
            (png::ColorType::Rgb, &[1, 2, 3], PixelLayout::Rgb8, 3),
            (png::ColorType::Rgba, &[1, 2, 3, 4], PixelLayout::Rgba8, 4),
            // Grayscale widens to RGB; it carries no alpha to preserve.
            (png::ColorType::Grayscale, &[9], PixelLayout::Rgb8, 3),
            // Grayscale+alpha must keep the alpha channel: flattening it here
            // would silently drop transparency the source file carried.
            (
                png::ColorType::GrayscaleAlpha,
                &[9, 128],
                PixelLayout::Rgba8,
                4,
            ),
        ];
        for (color, sample, expected, channels) in cases {
            let bytes = fixtures::png(4, 2, color, png::BitDepth::Eight, sample);
            let decoded = decode_within_pixel_budget(&bytes, 1_000).unwrap();
            assert!(
                matches!(
                    (decoded.layout, expected),
                    (PixelLayout::Rgb8, PixelLayout::Rgb8)
                        | (PixelLayout::Rgba8, PixelLayout::Rgba8)
                ),
                "{color:?} decoded to the wrong layout"
            );
            assert_eq!(decoded.data.len(), 4 * 2 * channels, "{color:?}");
        }
    }

    /// A grayscale+alpha source keeps its alpha values rather than being
    /// widened with an opaque channel.
    #[test]
    fn grayscale_alpha_carries_its_alpha_channel_across() {
        let bytes = fixtures::png(
            2,
            1,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[9, 128],
        );
        let decoded = decode_within_pixel_budget(&bytes, 1_000).unwrap();
        assert_eq!(decoded.data, vec![9, 9, 9, 128, 9, 9, 9, 128]);
    }

    /// 16-bit samples are stripped to 8 by `normalize_to_color8`, so the
    /// decoder never hands out a layout `Pixels` cannot express.
    #[test]
    fn strips_sixteen_bit_samples_to_eight() {
        let bytes = fixtures::png(
            2,
            2,
            png::ColorType::Rgb,
            png::BitDepth::Sixteen,
            &[0, 1, 0, 2, 0, 3],
        );
        let decoded = decode_within_pixel_budget(&bytes, 1_000).unwrap();
        assert!(matches!(decoded.layout, PixelLayout::Rgb8));
        assert_eq!(decoded.data.len(), 2 * 2 * 3);
    }

    #[test]
    fn decodes_jpeg_to_rgb8() {
        let bytes = fixtures::jpeg(16, 8, [200, 100, 50]);
        let decoded = decode_within_pixel_budget(&bytes, 1_000).unwrap();
        assert!(matches!(decoded.layout, PixelLayout::Rgb8));
        assert_eq!((decoded.width, decoded.height), (16, 8));
        assert_eq!(decoded.data.len(), 16 * 8 * 3);
    }

    /// The budget rejection must stay a distinct variant: the runtime image
    /// endpoint answers 413 for it and 400 for everything else, and it used to
    /// tell them apart by parsing the header a second time at the call site.
    #[test]
    fn an_oversized_image_is_distinguishable_from_a_corrupt_one() {
        let png_bytes = fixtures::png(
            64,
            64,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[1, 2, 3],
        );
        assert!(matches!(
            decode_within_pixel_budget(&png_bytes, 4095),
            Err(DecodeError::TooLarge {
                width: 64,
                height: 64,
                max_pixels: 4095
            })
        ));
        assert!(matches!(
            decode_within_pixel_budget(b"nonsense", 4096),
            Err(DecodeError::Unsupported)
        ));

        // A real PNG header followed by garbage is malformed, not oversized.
        let mut truncated = png_bytes.clone();
        truncated.truncate(png_bytes.len() / 2);
        assert!(matches!(
            decode_within_pixel_budget(&truncated, u64::MAX),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn refuses_bytes_that_are_not_an_image() {
        assert!(decode_within_pixel_budget(b"nonsense", 1_000).is_err());
        assert!(header_dimensions(b"nonsense").is_err());
    }
}
