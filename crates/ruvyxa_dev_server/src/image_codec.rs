//! Pixel handling shared by the build-time optimizer and any other caller that
//! needs to resize or encode an image.
//!
//! Two things live here because getting either wrong is expensive and the cost
//! is invisible until someone profiles a large image:
//!
//! - **Borrowing, not copying.** `image_decode` normalizes every source to RGB8
//!   or RGBA8, so a decoded buffer goes straight to both the resizer and the
//!   WebP encoder. The `DynamicImage` path this replaced cloned for any other
//!   layout, which on a 6000x4000 source is a 68 MB allocation and memcpy — per
//!   encode, and one source produces nine of them.
//! - **SIMD convolution.** A scalar Lanczos3 loop takes 3628 ms to produce all
//!   eight responsive widths from a 6000x4000 source. `pic-scale` runs the same
//!   convolution through AVX2/SSE4.1/NEON in 89 ms. Re-check the number with
//!   `cargo test --release -p ruvyxa_dev_server measure_responsive -- --ignored
//!   --nocapture`.

use std::fmt;

use crate::image_decode::{self, Decoded};
use pic_scale::{
    ImageStore, ImageStoreMut, ImageStoreScaling, ResamplingFunction, ScalingOptions,
    ThreadingPolicy,
};

/// Why a resize or encode could not produce output.
///
/// This crate carries no error framework, and adding one for two call sites
/// would be the larger change. `std::error::Error` is enough for callers that
/// do have one to convert.
#[derive(Debug)]
pub enum ImageCodecError {
    InvalidBuffer(String),
    Resize(String),
    Encode(String),
}

impl fmt::Display for ImageCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBuffer(detail) => write!(formatter, "invalid pixel buffer: {detail}"),
            Self::Resize(detail) => write!(formatter, "image resize failed: {detail}"),
            Self::Encode(detail) => write!(formatter, "WebP encoding failed: {detail}"),
        }
    }
}

impl std::error::Error for ImageCodecError {}

pub type Result<T> = std::result::Result<T, ImageCodecError>;

/// Dimensions from the image header, without decoding pixel data.
///
/// This is the only thing that can be known cheaply about an untrusted image:
/// the header is a fixed-size prefix, while decoding allocates
/// `width * height * channels` bytes before anything gets a chance to object.
pub fn header_dimensions(source: &[u8]) -> Result<(u32, u32)> {
    image_decode::header_dimensions(source)
        .map_err(|error| ImageCodecError::InvalidBuffer(error.to_string()))
}

/// Decode an image only if it fits inside `max_pixels`.
///
/// The budget is checked against the header first and enforced again by the
/// decoder's own allocation limit. Checking only after `load_from_memory`
/// returns is checking after the damage: a 20 MB PNG can legally declare
/// 50000x50000, and the decoder will have committed ten gigabytes to the heap
/// before the caller ever sees a dimension to reject. The header covers the
/// honest case cheaply; `Limits` covers a header that lies about what follows.
pub fn decode_within_pixel_budget(source: &[u8], max_pixels: u64) -> Result<Decoded> {
    image_decode::decode_within_pixel_budget(source, max_pixels)
        .map_err(|error| ImageCodecError::InvalidBuffer(error.to_string()))
}

/// Pixels in a layout both the resizer and the WebP encoder accept directly.
#[derive(Clone, Copy)]
pub enum PixelLayout {
    Rgb8,
    Rgba8,
}

/// A decoded image whose pixels are ready to use without another conversion.
///
/// `borrowed` is the common case, and now the only one: every decoder in
/// `image_decode` normalizes to RGB8 or RGBA8, so the buffer a decode produces
/// is already in one of the two layouts WebP wants. The previous
/// `DynamicImage`-based path had to branch on eight color types and clone for
/// the ones that were neither, which on a 6000x4000 source was a 68 MB
/// allocation and memcpy.
pub struct Pixels<'a> {
    data: PixelData<'a>,
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
}

enum PixelData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl<'a> Pixels<'a> {
    /// Borrow a decoded image's pixels. Never copies: normalization already
    /// happened inside the decoder.
    pub fn from_decoded(decoded: &'a Decoded) -> Self {
        Self {
            data: PixelData::Borrowed(&decoded.data),
            width: decoded.width,
            height: decoded.height,
            layout: decoded.layout,
        }
    }

    /// Take ownership of an already-materialized buffer, such as a resize result.
    pub fn from_owned(data: Vec<u8>, width: u32, height: u32, layout: PixelLayout) -> Self {
        Self {
            data: PixelData::Owned(data),
            width,
            height,
            layout,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        match &self.data {
            PixelData::Borrowed(slice) => slice,
            PixelData::Owned(vec) => vec,
        }
    }

    /// Downscale to `width` x `height`, keeping the source layout.
    ///
    /// Every target is produced from these pixels rather than from the
    /// previously emitted, smaller variant. Chaining would be cheaper on a
    /// scalar resizer, but it resamples the same image once per step and the
    /// error compounds; with SIMD the full-source path is already fast enough
    /// that there is nothing to buy with that trade.
    pub fn resize(&self, width: u32, height: u32) -> Result<Pixels<'static>> {
        // `premultiply_alpha` is set explicitly rather than left to
        // `ScalingOptions::default()`, which is `false`. Convolving
        // non-premultiplied RGBA blends the colour of fully transparent pixels
        // into their visible neighbours, so a sprite on a transparent canvas
        // picks up a halo along every edge. The previous resizer premultiplied
        // by default, and silently losing that would have changed output no
        // test asserts on.
        let options = ScalingOptions {
            resampling_function: ResamplingFunction::Lanczos3,
            premultiply_alpha: true,
            // One resize already parallelizes internally. The build runs
            // several concurrently on top of that, which oversubscribes only if
            // both layers fan out to every core; `Adaptive` sizes its pool to
            // the work rather than to the machine.
            threading_policy: ThreadingPolicy::Adaptive,
        };
        let (source_width, source_height) = (self.width as usize, self.height as usize);
        let (target_width, target_height) = (width as usize, height as usize);

        let data = match self.layout {
            PixelLayout::Rgb8 => {
                let store =
                    ImageStore::<u8, 3>::from_slice(self.as_slice(), source_width, source_height)
                        .map_err(|error| ImageCodecError::InvalidBuffer(error.to_string()))?;
                let mut destination = ImageStoreMut::<u8, 3>::alloc(target_width, target_height);
                store
                    .scale(&mut destination, options)
                    .map_err(|error| ImageCodecError::Resize(error.to_string()))?;
                destination.buffer.borrow().to_vec()
            }
            PixelLayout::Rgba8 => {
                let store =
                    ImageStore::<u8, 4>::from_slice(self.as_slice(), source_width, source_height)
                        .map_err(|error| ImageCodecError::InvalidBuffer(error.to_string()))?;
                let mut destination = ImageStoreMut::<u8, 4>::alloc(target_width, target_height);
                store
                    .scale(&mut destination, options)
                    .map_err(|error| ImageCodecError::Resize(error.to_string()))?;
                destination.buffer.borrow().to_vec()
            }
        };

        Ok(Pixels::from_owned(data, width, height, self.layout))
    }
}

/// WebP encoder settings.
#[derive(Debug, Clone, Copy)]
pub struct WebpSettings {
    pub quality: u8,
    pub lossless: bool,
    /// libwebp's `method`: 0 is fastest and largest, 6 is slowest and smallest.
    pub effort: u8,
}

/// Encode pixels to WebP.
///
/// Mirrors what `Encoder::encode_simple` sets up, plus `method`. libwebp's
/// `thread_level` is deliberately left alone: it does not split a single lossy
/// image encode, and measurement confirmed it (2385 ms vs 2351 ms on a
/// 6000x4000 source). Encode parallelism comes from running the independent
/// outputs concurrently, not from inside one encode.
pub fn encode_webp(pixels: &Pixels<'_>, settings: WebpSettings) -> Result<Vec<u8>> {
    let mut config = webp::WebPConfig::new().map_err(|()| {
        ImageCodecError::Encode("could not initialize the encoder configuration".to_string())
    })?;
    config.lossless = i32::from(settings.lossless);
    config.alpha_compression = i32::from(!settings.lossless);
    config.quality = f32::from(settings.quality.clamp(1, 100));
    config.method = i32::from(settings.effort.min(6));

    let encoder = match pixels.layout {
        PixelLayout::Rgb8 => {
            webp::Encoder::from_rgb(pixels.as_slice(), pixels.width, pixels.height)
        }
        PixelLayout::Rgba8 => {
            webp::Encoder::from_rgba(pixels.as_slice(), pixels.width, pixels.height)
        }
    };
    encoder
        .encode_advanced(&config)
        .map(|memory| memory.to_vec())
        .map_err(|error| ImageCodecError::Encode(format!("{error:?}")))
}

/// Height that preserves aspect ratio, never zero.
///
/// A zero height would make the encoder reject the buffer on extreme aspect
/// ratios, so the floor is part of the contract rather than a caller's job.
pub fn scaled_height(source_width: u32, source_height: u32, target_width: u32) -> u32 {
    ((u64::from(target_width) * u64::from(source_height)) / u64::from(source_width.max(1))).max(1)
        as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_decode::fixtures;

    fn decoded(width: u32, height: u32, layout: PixelLayout, sample: &[u8]) -> Decoded {
        let channels = match layout {
            PixelLayout::Rgb8 => 3,
            PixelLayout::Rgba8 => 4,
        };
        Decoded {
            data: sample
                .iter()
                .copied()
                .cycle()
                .take((width * height) as usize * channels)
                .collect(),
            width,
            height,
            layout,
        }
    }

    /// `Pixels` no longer converts anything: the decoder already normalized the
    /// layout, so this must borrow the decoded buffer rather than clone it. On a
    /// 6000x4000 source a clone here was a 68 MB allocation per encode.
    #[test]
    fn borrows_the_decoded_buffer_without_copying() {
        for (layout, sample) in [
            (PixelLayout::Rgb8, &[1u8, 2, 3][..]),
            (PixelLayout::Rgba8, &[1u8, 2, 3, 4][..]),
        ] {
            let source = decoded(4, 3, layout, sample);
            let pixels = Pixels::from_decoded(&source);
            assert!(std::ptr::eq(
                pixels.as_slice().as_ptr(),
                source.data.as_ptr()
            ));
            assert_eq!((pixels.width, pixels.height), (4, 3));
        }
    }

    #[test]
    fn resize_preserves_layout_and_target_size() {
        let source = decoded(100, 40, PixelLayout::Rgba8, &[7u8; 4]);
        let pixels = Pixels::from_decoded(&source);
        let resized = pixels.resize(50, scaled_height(100, 40, 50)).unwrap();
        assert_eq!((resized.width, resized.height), (50, 20));
        assert!(matches!(resized.layout, PixelLayout::Rgba8));
        assert_eq!(resized.as_slice().len(), 50 * 20 * 4);
    }

    /// Resizing must premultiply alpha.
    ///
    /// The source is fully transparent red everywhere except one opaque blue
    /// column. Convolving the raw channels averages that invisible red into the
    /// blue column and the result shows a magenta halo; premultiplying weights
    /// each colour by its own alpha first, so a fully transparent pixel
    /// contributes no colour at all. `ScalingOptions::default()` leaves this
    /// off, so it is asserted rather than assumed.
    #[test]
    fn resizing_rgba_does_not_bleed_transparent_colour_into_visible_pixels() {
        let (width, height) = (64u32, 8u32);
        let mut data = Vec::with_capacity((width * height) as usize * 4);
        for _ in 0..height {
            for x in 0..width {
                if x < width / 2 {
                    data.extend_from_slice(&[0, 0, 255, 255]); // opaque blue
                } else {
                    data.extend_from_slice(&[255, 0, 0, 0]); // invisible red
                }
            }
        }
        let source = Decoded {
            data,
            width,
            height,
            layout: PixelLayout::Rgba8,
        };
        let resized = Pixels::from_decoded(&source).resize(8, 4).unwrap();
        let pixels = resized.as_slice();

        // Every pixel that is visible at all must still be blue: any red in a
        // pixel with alpha is red that came from the transparent half.
        for pixel in pixels.chunks_exact(4) {
            let [red, _green, blue, alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            if alpha > 0 {
                assert!(
                    red < 16,
                    "transparent red bled into a visible pixel: {pixel:?}"
                );
                assert!(blue > red, "visible pixel lost its colour: {pixel:?}");
            }
        }
    }

    /// A solid image must survive a downscale unchanged.
    ///
    /// This is the cheapest possible check that the resizer is wired to the
    /// right buffers and strides: any row/stride mistake turns a constant image
    /// into stripes or noise, which a dimensions-only assertion would miss.
    #[test]
    fn resizing_a_solid_image_preserves_its_colour() {
        let source = decoded(40, 40, PixelLayout::Rgb8, &[17, 99, 200]);
        let resized = Pixels::from_decoded(&source).resize(10, 10).unwrap();
        for pixel in resized.as_slice().chunks_exact(3) {
            assert_eq!(pixel, [17, 99, 200], "solid colour changed during resize");
        }
    }

    /// Not an assertion — a measurement, printed with `--nocapture`, so the
    /// number in the module docs can be re-checked on any machine.
    #[test]
    #[ignore = "measurement, not a correctness check"]
    fn measure_responsive_ladder_throughput() {
        let (width, height) = (6000u32, 4000u32);
        let mut data = Vec::with_capacity((width * height) as usize * 3);
        for index in 0..(width * height) as usize {
            data.extend_from_slice(&[
                (index % 251) as u8,
                (index % 241) as u8,
                (index % 239) as u8,
            ]);
        }
        let source = Decoded {
            data,
            width,
            height,
            layout: PixelLayout::Rgb8,
        };
        let pixels = Pixels::from_decoded(&source);
        let started = std::time::Instant::now();
        for target in [640u32, 750, 828, 1080, 1200, 1920, 2048, 3840] {
            let scaled = scaled_height(width, height, target);
            std::hint::black_box(pixels.resize(target, scaled).unwrap());
        }
        println!(
            "eight responsive widths from 6000x4000: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn scaled_height_never_returns_zero() {
        // A 10000x1 banner scaled to 16px wide rounds to 0 before the floor.
        assert_eq!(scaled_height(10_000, 1, 16), 1);
        assert_eq!(scaled_height(1000, 500, 640), 320);
    }

    #[test]
    fn refuses_oversized_images_without_decoding_them() {
        let png = fixtures::png(
            64,
            64,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[1, 2, 3],
        );
        assert_eq!(header_dimensions(&png).unwrap(), (64, 64));
        // 64x64 is 4096 pixels, so a 4095 budget must reject it and a 4096
        // budget must not.
        assert!(decode_within_pixel_budget(&png, 4095).is_err());
        let decoded = decode_within_pixel_budget(&png, 4096).unwrap();
        assert_eq!((decoded.width, decoded.height), (64, 64));
    }

    #[test]
    fn effort_changes_output_without_changing_dimensions() {
        let mut data = Vec::new();
        for y in 0..64u32 {
            for x in 0..64u32 {
                data.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 2) as u8]);
            }
        }
        let source = Decoded {
            data,
            width: 64,
            height: 64,
            layout: PixelLayout::Rgb8,
        };
        let pixels = Pixels::from_decoded(&source);
        let base = WebpSettings {
            quality: 82,
            lossless: false,
            effort: 4,
        };
        let slow = encode_webp(&pixels, base).unwrap();
        let fast = encode_webp(&pixels, WebpSettings { effort: 0, ..base }).unwrap();
        for encoded in [&slow, &fast] {
            assert_eq!(header_dimensions(encoded).unwrap(), (64, 64));
        }
    }
}
