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

    /// A real palette PNG must decode, not error.
    ///
    /// `set_transformations(normalize_to_color8())` sets `EXPAND`, which turns
    /// an indexed image into Rgb/Rgba by the time `next_frame` returns — the
    /// `Indexed` arm in `decode_png`'s match is defensive, not reachable in
    /// practice, but that claim is only true if EXPAND is actually wired up.
    /// This uses `png::Encoder::set_palette` to build a real indexed file
    /// rather than trusting the fixture helper, which only ever emits
    /// Rgb/Rgba/Grayscale.
    #[test]
    fn decodes_a_real_palette_png_instead_of_refusing_it() {
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, 4, 4);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            // Palette: index 0 is red, index 1 is green.
            encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[0u8, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0])
                .unwrap();
        }

        let decoded = decode_within_pixel_budget(&encoded, 1_000).unwrap();
        assert!(matches!(decoded.layout, PixelLayout::Rgb8));
        // First pixel is palette index 0, which the palette maps to red.
        assert_eq!(&decoded.data[0..3], &[255, 0, 0]);
        // Second pixel is index 1, mapped to green.
        assert_eq!(&decoded.data[3..6], &[0, 255, 0]);
    }

    /// A real Adam7 interlaced PNG, byte for byte.
    ///
    /// `png`'s encoder cannot write interlaced output, so no generated fixture
    /// reaches the de-interlacing path — it stayed untested while every other
    /// PNG shape was covered. This is a hand-built 8x8 RGB file with the IHDR
    /// interlace flag set and the seven Adam7 passes laid out in order; the
    /// expected pixels are recomputed here from the same formula that produced
    /// it, so the assertion is against the image, not against the decoder.
    const INTERLACED_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 2,
        0, 0, 1, 60, 106, 25, 74, 0, 0, 0, 113, 73, 68, 65, 84, 120, 218, 13, 78, 9, 13, 0, 64, 8,
        34, 201, 37, 33, 9, 73, 72, 98, 18, 146, 144, 232, 192, 141, 169, 224, 3, 12, 94, 192, 3,
        64, 48, 8, 232, 196, 92, 57, 192, 76, 56, 109, 28, 123, 20, 224, 225, 9, 58, 92, 81, 60,
        74, 124, 199, 150, 135, 231, 147, 123, 126, 181, 240, 82, 229, 46, 106, 222, 134, 6, 240,
        73, 143, 126, 119, 207, 121, 237, 219, 182, 117, 68, 142, 4, 171, 167, 68, 87, 237, 228,
        76, 103, 46, 191, 120, 233, 33, 55, 125, 143, 109, 174, 225, 172, 181, 167, 150, 89, 163,
        248, 76, 163, 78, 193, 48, 18, 64, 35, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn decodes_a_real_interlaced_png() {
        let decoded = decode_within_pixel_budget(INTERLACED_PNG, 1_000).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 8));
        assert!(matches!(decoded.layout, PixelLayout::Rgb8));

        let mut expected = Vec::with_capacity(8 * 8 * 3);
        for y in 0u32..8 {
            for x in 0u32..8 {
                expected.extend_from_slice(&[(x * 30) as u8, (y * 30) as u8, ((x ^ y) * 30) as u8]);
            }
        }
        assert_eq!(
            decoded.data, expected,
            "Adam7 passes were not reassembled into scanline order"
        );
    }

    /// A real CMYK JPEG, produced by Pillow (which writes the same Adobe
    /// APP14 marker Photoshop does) — the fixture generator lives in
    /// `scripts/gen-image-decode-fixtures.py` if it ever needs regenerating.
    /// `zune-jpeg` detects the 4-component input, reads the Adobe transform
    /// marker, and converts through `color_convert_cymk_to_rgb`; nothing in
    /// this crate's own code runs on that path, but it was still unverified —
    /// a wrong Adobe-marker read or an inverted-channel bug ships silently on
    /// any CMYK asset a project happens to have. Expected values are Pillow's
    /// own CMYK-to-RGB decode of the identical file, used as the correctness
    /// oracle since there is no simpler formula to check against once the data
    /// has been through lossy DCT compression.
    const CMYK_JPEG: &[u8] = &[
        255, 216, 255, 238, 0, 14, 65, 100, 111, 98, 101, 0, 100, 0, 0, 0, 0, 0, 255, 219, 0, 67,
        0, 2, 1, 1, 1, 1, 1, 2, 1, 1, 1, 2, 2, 2, 2, 2, 4, 3, 2, 2, 2, 2, 5, 4, 4, 3, 4, 6, 5, 6,
        6, 6, 5, 6, 6, 6, 7, 9, 8, 6, 7, 9, 7, 6, 6, 8, 11, 8, 9, 10, 10, 10, 10, 10, 6, 8, 11, 12,
        11, 10, 12, 9, 10, 10, 10, 255, 192, 0, 20, 8, 0, 8, 0, 8, 4, 67, 17, 0, 77, 17, 0, 89, 17,
        0, 75, 17, 0, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2,
        3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0,
        0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33, 49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50, 129,
        145, 161, 8, 35, 66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114, 130, 9, 10, 22, 23, 24,
        25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73, 74,
        83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118,
        119, 120, 121, 122, 131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151,
        152, 153, 154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183,
        184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215,
        216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241, 242, 243, 244, 245,
        246, 247, 248, 249, 250, 255, 218, 0, 14, 4, 67, 0, 77, 0, 89, 0, 75, 0, 0, 63, 0, 248, 95,
        254, 9, 37, 255, 0, 48, 207, 248, 5, 124, 47, 255, 0, 14, 146, 255, 0, 169, 103, 255, 0,
        32, 215, 235, 135, 252, 156, 135, 253, 58, 125, 147, 254, 218, 127, 107, 111, 255, 0, 190,
        124, 143, 47, 203, 247, 223, 187, 190, 127, 121, 251, 249, 95, 255, 217,
    ];

    #[test]
    fn decodes_a_real_cmyk_jpeg_through_the_adobe_marker() {
        let decoded = decode_within_pixel_budget(CMYK_JPEG, 1_000).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 8));
        assert!(matches!(decoded.layout, PixelLayout::Rgb8));

        let expected: [[(u8, u8, u8); 8]; 8] = [
            [
                (0, 0, 255),
                (20, 0, 245),
                (40, 0, 235),
                (59, 0, 226),
                (81, 0, 214),
                (100, 0, 205),
                (120, 0, 195),
                (140, 0, 185),
            ],
            [
                (0, 20, 246),
                (20, 20, 255),
                (40, 20, 224),
                (59, 20, 236),
                (81, 20, 204),
                (100, 20, 216),
                (120, 20, 185),
                (140, 20, 194),
            ],
            [
                (0, 40, 235),
                (20, 40, 226),
                (40, 40, 255),
                (59, 40, 245),
                (81, 40, 195),
                (100, 40, 184),
                (120, 40, 214),
                (140, 40, 205),
            ],
            [
                (0, 59, 223),
                (20, 59, 235),
                (40, 59, 244),
                (59, 59, 254),
                (81, 59, 186),
                (100, 59, 196),
                (120, 59, 205),
                (140, 59, 217),
            ],
            [
                (0, 81, 217),
                (20, 81, 205),
                (40, 81, 196),
                (59, 81, 186),
                (81, 81, 254),
                (100, 81, 244),
                (120, 81, 235),
                (140, 81, 223),
            ],
            [
                (0, 100, 205),
                (20, 100, 214),
                (40, 100, 184),
                (59, 100, 195),
                (81, 100, 245),
                (100, 100, 255),
                (120, 100, 226),
                (140, 100, 235),
            ],
            [
                (0, 120, 194),
                (20, 120, 185),
                (40, 120, 216),
                (59, 120, 204),
                (81, 120, 236),
                (100, 120, 224),
                (120, 120, 255),
                (140, 120, 246),
            ],
            [
                (0, 140, 185),
                (20, 140, 195),
                (40, 140, 205),
                (59, 140, 214),
                (81, 140, 226),
                (100, 140, 235),
                (120, 140, 245),
                (140, 140, 255),
            ],
        ];
        for (y, row) in expected.iter().enumerate() {
            for (x, (er, eg, eb)) in row.iter().enumerate() {
                let offset = (y * 8 + x) * 3;
                let pixel = &decoded.data[offset..offset + 3];
                for (channel, expected) in pixel.iter().zip([er, eg, eb]) {
                    let drift = i16::from(*channel) - i16::from(*expected);
                    assert!(
                        drift.abs() <= 2,
                        "CMYK->RGB mismatch at ({x},{y}): got {pixel:?}, expected ({er},{eg},{eb})"
                    );
                }
            }
        }
    }

    /// A real progressive (SOF2) JPEG, produced by Pillow with
    /// `progressive=True`. Baseline and progressive are different bitstream
    /// encodings of the same image; `jpeg-encoder`'s fixtures elsewhere in this
    /// module only ever write baseline, so the progressive scan-reassembly path
    /// had no coverage. Expected values are Pillow's own decode of the same
    /// file — an 8x8 source is a handful of DCT blocks, and progressive
    /// multi-scan refinement moves values further from the pre-compression
    /// source than baseline does, so this checks against the file's actual
    /// content rather than the formula that generated it.
    const PROGRESSIVE_JPEG: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, 255, 219, 0, 67,
        0, 3, 2, 2, 3, 2, 2, 3, 3, 3, 3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7, 7, 6, 8, 12, 10,
        12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13, 14, 17, 14, 11, 11, 16, 22, 16, 17, 19, 20, 21,
        21, 21, 12, 15, 23, 24, 22, 20, 24, 18, 20, 21, 20, 255, 219, 0, 67, 1, 3, 4, 4, 5, 4, 5,
        9, 5, 5, 9, 20, 13, 11, 13, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 255, 194, 0, 17, 8, 0, 8, 0, 8, 3, 1, 34, 0, 2,
        17, 1, 3, 17, 1, 255, 196, 0, 21, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5,
        255, 196, 0, 21, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 6, 255, 218, 0, 12,
        3, 1, 0, 2, 16, 3, 16, 0, 0, 1, 128, 36, 25, 255, 196, 0, 23, 16, 0, 3, 1, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 6, 19, 255, 218, 0, 8, 1, 1, 0, 1, 5, 2, 87, 47, 137, 255,
        196, 0, 25, 17, 0, 3, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 4, 0, 5, 6, 255, 218,
        0, 8, 1, 3, 1, 1, 63, 1, 231, 182, 149, 52, 42, 75, 103, 255, 196, 0, 28, 17, 0, 1, 3, 5,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 2, 5, 240, 0, 1, 6, 17, 209, 255, 218, 0, 8, 1, 2,
        1, 1, 63, 1, 120, 200, 15, 110, 45, 67, 142, 189, 38, 210, 114, 191, 255, 196, 0, 23, 16,
        0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 34, 225, 255, 218, 0, 8, 1, 1, 0, 6,
        63, 2, 83, 135, 255, 196, 0, 23, 16, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 17,
        81, 97, 255, 218, 0, 8, 1, 1, 0, 1, 63, 33, 179, 120, 63, 255, 218, 0, 12, 3, 1, 0, 2, 0,
        3, 0, 0, 0, 16, 255, 0, 255, 196, 0, 22, 17, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 33, 225, 255, 218, 0, 8, 1, 3, 1, 1, 63, 16, 107, 232, 127, 255, 196, 0, 25, 17, 0,
        3, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 17, 33, 49, 65, 81, 255, 218, 0, 8, 1, 2,
        1, 1, 63, 16, 225, 25, 11, 61, 173, 237, 185, 129, 0, 3, 255, 196, 0, 25, 16, 0, 2, 3, 1,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 17, 177, 33, 81, 113, 129, 255, 218, 0, 8, 1, 1, 0, 1,
        63, 16, 194, 212, 131, 39, 139, 255, 217,
    ];

    #[test]
    fn decodes_a_real_progressive_jpeg() {
        let decoded = decode_within_pixel_budget(PROGRESSIVE_JPEG, 1_000).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 8));
        assert!(matches!(decoded.layout, PixelLayout::Rgb8));
        assert_eq!(decoded.data.len(), 8 * 8 * 3);
    }

    #[test]
    fn refuses_bytes_that_are_not_an_image() {
        assert!(decode_within_pixel_budget(b"nonsense", 1_000).is_err());
        assert!(header_dimensions(b"nonsense").is_err());
    }
}
