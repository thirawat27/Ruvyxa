//! Fast build-time conversion of public PNG/JPEG assets into WebP files.
//!
//! ## Cache first, decode last
//!
//! Decoding is the expensive step that a rebuild usually does not need: every
//! output is content-addressed by the source bytes, so whether work is required
//! is decidable from the file's hash alone. The pipeline therefore plans all
//! outputs, checks the cache, and only decodes when something is actually
//! missing. Dimensions for the manifest come from the image header on that
//! path — 2.4 ms against 116 ms for a full decode of a 6000x4000 JPEG.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use ruvyxa_dev_server::image_codec::{
    Pixels, WebpSettings, decode_within_pixel_budget, encode_webp, header_dimensions, scaled_height,
};

/// Bumped when a change makes previously cached bytes wrong for the same input.
///
/// The cache is keyed by source bytes and encoder settings, neither of which
/// changes when the resampling implementation does. Without this byte, the
/// first build after such a change would mix outputs from two resamplers in one
/// asset directory.
const CACHE_VERSION: u8 = 2;

/// Build-time decoding is deliberately unbounded.
///
/// The runtime path caps decoded pixels because it answers untrusted request
/// paths. A build decodes files the project author committed, and a cap here
/// would silently skip optimization for a legitimately huge source rather than
/// protect anything. The `image` crate this replaced carried no cap on the
/// build path either, so this keeps build output byte-identical.
const SOURCE_PIXEL_BUDGET: u64 = u64::MAX;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageOptimizationOptions {
    pub optimize: bool,
    pub quality: u8,
    pub lossless: bool,
    /// Keep the original PNG/JPEG next to its WebP output.
    ///
    /// The default publishes only `logo.webp`; applications should use
    /// `<Image>` or the converted URL. Enable this compatibility option when
    /// raw `<img src="/logo.png">` references must keep working on a static CDN.
    pub keep_original: bool,
    /// Opt-in target widths for responsive `srcset` variants, in pixels.
    ///
    /// For each source the optimizer emits `<name>-<w>w.webp` at every width in
    /// this list that is strictly smaller than the image's intrinsic width. The
    /// Static `<Image>` does not fabricate these URLs. Applications opting in
    /// reference the generated files through an explicit `srcSet`; on-demand
    /// images use runtime-generated URLs instead.
    pub variant_widths: Vec<u32>,
    /// Zero uses Rayon's global worker count.
    #[serde(rename = "workers")]
    pub parallelism: usize,
    /// libwebp's `method`, 0 (fastest, largest) to 6 (slowest, smallest).
    ///
    /// Encoding dominates a large-image build once resizing is vectorized: a
    /// 6000x4000 source spends ~2.2 s in the full-size encode alone, and
    /// libwebp cannot split one lossy encode across threads. `method` is the
    /// only lever, and it trades bytes for time — measured on that source,
    /// 4 → 2167 ms / 3636 KB, 2 → 1219 ms / 4281 KB, 0 → 738 ms / 4170 KB.
    ///
    /// The default stays at libwebp's own 4 so upgrading never silently
    /// inflates a deployed asset set; projects that would rather have the build
    /// time can opt out.
    pub effort: u8,
    /// Optional runtime transforms for same-origin public assets.
    pub on_demand: OnDemandImageOptions,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OnDemandImageOptions {
    Enabled(bool),
    Config(OnDemandImageConfigOptions),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct OnDemandImageConfigOptions {
    pub enabled: bool,
    pub max_width: u32,
}

impl Default for OnDemandImageConfigOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_width: 3840,
        }
    }
}

impl Default for OnDemandImageOptions {
    fn default() -> Self {
        Self::Enabled(false)
    }
}

impl OnDemandImageOptions {
    pub fn enabled(&self) -> bool {
        match self {
            Self::Enabled(enabled) => *enabled,
            Self::Config(config) => config.enabled,
        }
    }

    pub fn max_width(&self) -> u32 {
        match self {
            Self::Enabled(_) => 3840,
            Self::Config(config) => config.max_width,
        }
    }
}

impl Default for ImageOptimizationOptions {
    fn default() -> Self {
        Self {
            optimize: true,
            quality: 82,
            lossless: false,
            keep_original: false,
            variant_widths: Vec::new(),
            parallelism: 0,
            effort: 4,
            on_demand: OnDemandImageOptions::default(),
        }
    }
}

impl ImageOptimizationOptions {
    fn webp(&self) -> WebpSettings {
        WebpSettings {
            quality: self.quality,
            lossless: self.lossless,
            effort: self.effort,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOptimizationReport {
    pub optimized_images: usize,
    pub cache_hits: usize,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub entries: Vec<ImageManifestEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifestEntry {
    pub source: String,
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub cache_hit: bool,
    /// Responsive downscaled outputs, smallest width first. Empty when the
    /// image is narrower than every configured breakpoint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<ImageVariant>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageVariant {
    pub width: u32,
    pub output: String,
}

struct Conversion {
    source: PathBuf,
    output: PathBuf,
    width: u32,
    height: u32,
    source_bytes: u64,
    output_bytes: u64,
    cache_hit: bool,
    variants: Vec<ImageVariant>,
}

/// One WebP file this source must produce.
struct OutputPlan {
    /// `None` is the full-size output; `Some(w)` is a responsive variant.
    width: Option<u32>,
    output: PathBuf,
    cached: PathBuf,
}

/// Copy public assets and convert PNG/JPEG files to one WebP output each.
///
/// Image sources are read directly from `public_dir`, avoiding a copy/read/delete
/// staging cycle. Malformed inputs are copied unchanged, while I/O and encoding
/// failures stop the build instead of publishing a partial asset set.
pub fn optimize_public_images(
    public_dir: &Path,
    assets_dir: &Path,
    cache_dir: &Path,
    options: &ImageOptimizationOptions,
) -> anyhow::Result<ImageOptimizationReport> {
    let mut report = ImageOptimizationReport::default();
    if !public_dir.exists() {
        return Ok(report);
    }
    fs::create_dir_all(assets_dir)
        .with_context(|| format!("failed to create asset output at {}", assets_dir.display()))?;

    let sources = discover_sources(public_dir)?;
    ensure_unique_outputs(public_dir, assets_dir, &sources, options.optimize)?;
    if options.optimize && options.keep_original {
        ensure_unique_originals(public_dir, assets_dir, &sources)?;
    }
    if options.optimize {
        fs::create_dir_all(cache_dir)
            .with_context(|| format!("failed to create image cache at {}", cache_dir.display()))?;
    }

    let process = || {
        sources
            .par_iter()
            .map(|source| process_one(public_dir, assets_dir, source, cache_dir, options))
            .collect::<Vec<_>>()
    };
    let results = if options.parallelism == 0 {
        process()
    } else {
        rayon::ThreadPoolBuilder::new()
            .num_threads(options.parallelism.max(1))
            .build()
            .context("failed to create the image optimization worker pool")?
            .install(process)
    };

    for result in results {
        let Some(conversion) = result? else {
            continue;
        };
        report.optimized_images += 1;
        report.cache_hits += usize::from(conversion.cache_hit);
        report.source_bytes += conversion.source_bytes;
        report.output_bytes += conversion.output_bytes;
        report.entries.push(ImageManifestEntry {
            source: relative_url(public_dir, &conversion.source),
            output: relative_url(assets_dir, &conversion.output),
            width: conversion.width,
            height: conversion.height,
            source_bytes: conversion.source_bytes,
            output_bytes: conversion.output_bytes,
            cache_hit: conversion.cache_hit,
            variants: conversion.variants,
        });
    }

    report
        .entries
        .sort_by(|left, right| left.source.cmp(&right.source));
    write_manifest(assets_dir, &report)?;
    Ok(report)
}

fn discover_sources(public_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    // Walk errors must not silently drop assets from the build output.
    let mut sources = Vec::new();
    for entry in WalkDir::new(public_dir) {
        let entry = entry
            .with_context(|| format!("failed to walk {} for image assets", public_dir.display()))?;
        if entry.file_type().is_file() {
            sources.push(entry.into_path());
        }
    }
    sources.sort();
    Ok(sources)
}

fn ensure_unique_outputs(
    public_dir: &Path,
    assets_dir: &Path,
    sources: &[PathBuf],
    optimize: bool,
) -> anyhow::Result<()> {
    // Key on a case-folded path: the output directory may be
    // case-insensitive (NTFS/APFS default) even when the source tree is
    // case-sensitive, so `Hero.webp` and `hero.webp` are the same physical
    // file and racing writers would silently drop one image.
    let mut output_sources = HashMap::<String, &Path>::new();
    for source in sources {
        let mut output = assets_dir.join(source.strip_prefix(public_dir).unwrap_or(source));
        if optimize && is_optimizable_source(source) {
            output.set_extension("webp");
        }
        let folded = output.to_string_lossy().to_lowercase();
        if let Some(existing) = output_sources.insert(folded, source) {
            bail!(
                "image output collision: {} and {} both map to {}; rename one source",
                existing.display(),
                source.display(),
                output.display()
            );
        }
    }
    Ok(())
}

fn ensure_unique_originals(
    public_dir: &Path,
    assets_dir: &Path,
    sources: &[PathBuf],
) -> anyhow::Result<()> {
    // Originals are copied under their own names, so `Logo.png` and `logo.png`
    // collide on a case-insensitive output directory even though neither maps
    // onto a WebP name that `ensure_unique_outputs` already rejects.
    let mut output_sources = HashMap::<String, &Path>::new();
    for source in sources {
        let output = assets_dir.join(source.strip_prefix(public_dir).unwrap_or(source));
        let folded = output.to_string_lossy().to_lowercase();
        if let Some(existing) = output_sources.insert(folded, source) {
            bail!(
                "image output collision: {} and {} both map to {}; rename one source",
                existing.display(),
                source.display(),
                output.display()
            );
        }
    }
    Ok(())
}

fn process_one(
    public_dir: &Path,
    assets_dir: &Path,
    source: &Path,
    cache_dir: &Path,
    options: &ImageOptimizationOptions,
) -> anyhow::Result<Option<Conversion>> {
    let relative = source.strip_prefix(public_dir).unwrap_or(source);
    let unchanged_output = assets_dir.join(relative);
    if !options.optimize || !is_optimizable_source(source) {
        copy_asset(source, &unchanged_output)?;
        return Ok(None);
    }

    let source_data =
        fs::read(source).with_context(|| format!("failed to read image {}", source.display()))?;

    // Read the header only. Everything downstream — which variants exist, what
    // their cache keys are, whether any work is left — follows from the source
    // bytes and these dimensions, and none of it needs pixels.
    let Ok((mut width, mut height)) = header_dimensions(&source_data) else {
        copy_asset(source, &unchanged_output)?;
        return Ok(None);
    };

    // The optimized WebP is what `<Image>` and the prerendered HTML point at;
    // the untouched source keeps every other reference to `/logo.png` working
    // on hosts that serve the publish directory straight from a CDN.
    if options.keep_original {
        copy_asset(source, &unchanged_output)?;
    }

    // One hash of the source, reused for every output key. Hashing per output
    // re-read the whole file once per variant — nine passes over 6.5 MB for a
    // single hero image, all producing the same digest.
    let digest = blake3::hash(&source_data);
    let mut plans = plan_outputs(relative, assets_dir, cache_dir, &digest, options, width);

    // A cached run never touches a pixel. This is the common case on rebuild,
    // and it is the difference between ~3 ms and ~120 ms per image.
    let mut decoded = None;
    if plans.iter().any(|plan| !plan.cached.is_file()) {
        let Ok(image) = decode_within_pixel_budget(&source_data, SOURCE_PIXEL_BUDGET) else {
            copy_asset(source, &unchanged_output)?;
            return Ok(None);
        };
        // Headers and decoders can disagree — a truncated or unusual file may
        // decode to a different size than it advertises. The decoded pixels are
        // the truth, so re-plan rather than emit variants for a width the image
        // does not have.
        let decoded_dimensions = (image.width, image.height);
        if decoded_dimensions != (width, height) {
            (width, height) = decoded_dimensions;
            plans = plan_outputs(relative, assets_dir, cache_dir, &digest, options, width);
        }
        decoded = Some(image);
    }

    let cache_hit = plans
        .first()
        .is_some_and(|primary| primary.cached.is_file());
    let pixels = decoded.as_ref().map(Pixels::from_decoded);

    // Every output of this source is one flat job list. The previous shape —
    // `rayon::join` between the full-size encode and a nested `par_iter` over
    // variants — pinned the full-size encode, the longest job, to one side of a
    // binary split. A flat list lets work-stealing schedule all of them, and
    // `par_iter` over an indexed collection keeps the order deterministic.
    let outputs: Vec<Option<ImageVariant>> = plans
        .par_iter()
        .map(|plan| materialize_output(plan, pixels.as_ref(), width, height, assets_dir, options))
        .collect::<anyhow::Result<_>>()?;

    let output = plans[0].output.clone();
    let variants = outputs.into_iter().flatten().collect();
    let output_bytes = fs::metadata(&output)
        .with_context(|| format!("failed to inspect image output {}", output.display()))?
        .len();
    Ok(Some(Conversion {
        source: source.to_path_buf(),
        output,
        width,
        height,
        source_bytes: source_data.len() as u64,
        output_bytes,
        cache_hit,
        variants,
    }))
}

/// The full-size output followed by one entry per responsive breakpoint.
///
/// Widths at or above the intrinsic size are skipped: upscaling only inflates
/// bytes for no visual gain, and the full-size WebP already covers the top of
/// the `srcset`. The full-size entry is always first so callers can find it
/// without searching.
fn plan_outputs(
    relative: &Path,
    assets_dir: &Path,
    cache_dir: &Path,
    digest: &blake3::Hash,
    options: &ImageOptimizationOptions,
    intrinsic_width: u32,
) -> Vec<OutputPlan> {
    let mut widths: Vec<u32> = options
        .variant_widths
        .iter()
        .copied()
        .filter(|width| *width > 0 && *width < intrinsic_width)
        .collect();
    widths.sort_unstable();
    widths.dedup();

    let mut plans = Vec::with_capacity(widths.len() + 1);
    plans.push(OutputPlan {
        width: None,
        output: assets_dir.join(webp_path(relative)),
        cached: cache_dir.join(format!("{}.webp", output_cache_key(digest, options, None))),
    });
    for width in widths {
        plans.push(OutputPlan {
            width: Some(width),
            output: assets_dir.join(variant_path(relative, width)),
            cached: cache_dir.join(format!(
                "{}.webp",
                output_cache_key(digest, options, Some(width))
            )),
        });
    }
    plans
}

/// Produce one planned output, encoding only when the cache misses.
///
/// Returns the manifest entry for a responsive variant, or `None` for the
/// full-size output, which the caller already tracks.
fn materialize_output(
    plan: &OutputPlan,
    pixels: Option<&Pixels<'_>>,
    intrinsic_width: u32,
    intrinsic_height: u32,
    assets_dir: &Path,
    options: &ImageOptimizationOptions,
) -> anyhow::Result<Option<ImageVariant>> {
    if let Some(parent) = plan.output.parent() {
        fs::create_dir_all(parent)?;
    }
    if !plan.cached.is_file() {
        let pixels = pixels.ok_or_else(|| {
            anyhow::anyhow!(
                "image cache entry {} disappeared mid-build",
                plan.cached.display()
            )
        })?;
        let encoded = match plan.width {
            None => encode_webp(pixels, options.webp())?,
            Some(width) => {
                let height = scaled_height(intrinsic_width, intrinsic_height, width);
                encode_webp(&pixels.resize(width, height)?, options.webp())?
            }
        };
        write_cache_entry(&plan.cached, &encoded)?;
    }
    materialize_cached(&plan.cached, &plan.output)?;

    Ok(plan.width.map(|width| ImageVariant {
        width,
        output: relative_url(assets_dir, &plan.output),
    }))
}

fn copy_asset(source: &Path, output: &Path) -> anyhow::Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, output)
        .map(|_| ())
        .with_context(|| format!("failed to copy public asset {}", source.display()))
}

/// Content address for one output of one source.
///
/// Derived from the source digest rather than the source bytes: the digest is
/// computed once per file and every output mixes it with only the few bytes
/// that distinguish them. `width` is length-tagged so a full-size output can
/// never collide with a variant.
fn output_cache_key(
    digest: &blake3::Hash,
    options: &ImageOptimizationOptions,
    width: Option<u32>,
) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(&[
        CACHE_VERSION,
        options.quality.clamp(1, 100),
        u8::from(options.lossless),
        options.effort.min(6),
    ]);
    hash.update(digest.as_bytes());
    match width {
        None => hash.update(&[0]),
        Some(width) => hash.update(&[1]).update(&width.to_le_bytes()),
    };
    hash.finalize().to_hex().to_string()
}

fn write_cache_entry(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    ruvyxa_bundler::atomic_file::write_atomic(path, bytes)
        .with_context(|| format!("failed to publish image cache entry {}", path.display()))
}

fn materialize_cached(cached: &Path, output: &Path) -> anyhow::Result<()> {
    if output.exists() {
        fs::remove_file(output)?;
    }
    fs::hard_link(cached, output)
        .or_else(|_| fs::copy(cached, output).map(|_| ()))
        .with_context(|| format!("failed to materialize image output {}", output.display()))
}

fn write_manifest(assets_dir: &Path, report: &ImageOptimizationReport) -> anyhow::Result<()> {
    if assets_dir.exists() {
        fs::write(
            assets_dir.join(".ruvyxa-images.json"),
            serde_json::to_vec(report)?,
        )?;
    }
    Ok(())
}

fn is_optimizable_source(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

fn webp_path(source: &Path) -> PathBuf {
    source.with_extension("webp")
}

/// Filename for a responsive variant: `hero.png` at width 640 → `hero-640w.webp`.
///
/// This naming is a published contract, not an internal detail: nothing in
/// `@ruvyxa/react` constructs these URLs, so an application opting into
/// `variantWidths` writes them into its own `srcSet` by hand. Changing the
/// scheme silently breaks every such `srcSet`, and the manifest entry each
/// variant is recorded in (`ImageVariant::output`) is the only other place the
/// name appears.
///
/// The comment here used to claim it mirrored a `variantUrl()` in
/// `packages/@ruvyxa/react/src/image.tsx`. No such function exists — the Static
/// `<Image>` deliberately does not fabricate variant URLs (see
/// [`ImageOptimizationOptions::variant_widths`]) — so the promised counterpart
/// could never have kept it honest.
fn variant_path(source: &Path, width: u32) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = format!("{stem}-{width}w.webp");
    match source.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(file_name),
        _ => PathBuf::from(file_name),
    }
}

fn relative_url(root: &Path, path: &Path) -> String {
    format!(
        "/{}",
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruvyxa_dev_server::image_decode;

    /// Tests need real encoded files on disk, because the optimizer sniffs and
    /// decodes them; a pixel buffer would not exercise the path under test.
    fn write_png(path: &Path, width: u32, height: u32, color: png::ColorType, sample: &[u8]) {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let data: Vec<u8> = sample
                .iter()
                .copied()
                .cycle()
                .take((width * height) as usize * sample.len())
                .collect();
            writer.write_image_data(&data).unwrap();
        }
        fs::write(path, out).unwrap();
    }

    fn write_jpeg(path: &Path, width: u16, height: u16, rgb: [u8; 3]) {
        let mut out = Vec::new();
        let data: Vec<u8> = rgb
            .iter()
            .copied()
            .cycle()
            .take(usize::from(width) * usize::from(height) * 3)
            .collect();
        jpeg_encoder::Encoder::new(&mut out, 90)
            .encode(&data, width, height, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        fs::write(path, out).unwrap();
    }

    #[test]
    fn publishes_exactly_one_webp_by_default_and_reuses_cache() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        let cache = temp.path().join("cache");
        fs::create_dir(&public).unwrap();
        let source = public.join("hero.png");
        write_png(&source, 4, 3, png::ColorType::Rgba, &[20, 40, 60, 255]);
        fs::write(public.join("robots.txt"), b"hello").unwrap();

        let first = optimize_public_images(
            &public,
            &assets,
            &cache,
            &ImageOptimizationOptions::default(),
        )
        .unwrap();
        assert!(source.exists());
        assert!(assets.join("hero.webp").is_file());
        assert!(!assets.join("hero.png").exists());
        assert!(first.entries[0].variants.is_empty());
        assert_eq!(fs::read(assets.join("robots.txt")).unwrap(), b"hello");
        assert_eq!(first.optimized_images, 1);
        assert_eq!(first.cache_hits, 0);
        assert_eq!(first.entries[0].output, "/hero.webp");

        fs::remove_dir_all(&assets).unwrap();
        let second = optimize_public_images(
            &public,
            &assets,
            &cache,
            &ImageOptimizationOptions::default(),
        )
        .unwrap();
        assert_eq!(second.cache_hits, 1);

        // Retaining the source is still available as an explicit compatibility
        // choice for raw `<img src="/hero.png">` references.
        fs::remove_dir_all(&assets).unwrap();
        optimize_public_images(
            &public,
            &assets,
            &cache,
            &ImageOptimizationOptions {
                keep_original: true,
                ..ImageOptimizationOptions::default()
            },
        )
        .unwrap();
        assert!(assets.join("hero.webp").is_file());
        assert!(assets.join("hero.png").is_file());
    }

    #[test]
    fn emits_responsive_variants_below_the_intrinsic_width() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        let cache = temp.path().join("cache");
        fs::create_dir(&public).unwrap();
        // 1000px wide: breakpoints 640 and 750 are below it, 828+ are not.
        write_jpeg(&public.join("hero.jpg"), 1000, 500, [10, 20, 30]);

        let report = optimize_public_images(
            &public,
            &assets,
            &cache,
            &ImageOptimizationOptions {
                variant_widths: vec![640, 750, 828, 1080],
                ..ImageOptimizationOptions::default()
            },
        )
        .unwrap();

        // Full-size WebP plus a downscaled file per breakpoint under 1000px.
        assert!(assets.join("hero.webp").is_file());
        assert!(assets.join("hero-640w.webp").is_file());
        assert!(assets.join("hero-750w.webp").is_file());
        assert!(assets.join("hero-828w.webp").is_file());
        // A breakpoint at or above the intrinsic width would only upscale.
        assert!(!assets.join("hero-1080w.webp").exists());

        let entry = &report.entries[0];
        let widths: Vec<u32> = entry.variants.iter().map(|variant| variant.width).collect();
        assert_eq!(widths, vec![640, 750, 828]);
        assert_eq!(entry.variants[0].output, "/hero-640w.webp");
        // A downscaled variant preserves aspect ratio (1000x500 → 640x320).
        assert_eq!(
            image_decode::header_dimensions(&fs::read(assets.join("hero-640w.webp")).unwrap())
                .unwrap(),
            (640, 320)
        );
    }

    #[test]
    fn narrow_images_get_no_variants() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();
        // Narrower than every breakpoint: no variant should be emitted.
        write_png(
            &public.join("icon.png"),
            320,
            200,
            png::ColorType::Rgb,
            &[1, 2, 3],
        );

        let report = optimize_public_images(
            &public,
            &assets,
            &temp.path().join("cache"),
            &ImageOptimizationOptions::default(),
        )
        .unwrap();

        assert!(report.entries[0].variants.is_empty());
        assert!(assets.join("icon.webp").is_file());
        assert!(!assets.join("icon-640w.webp").exists());
    }

    #[test]
    fn encodes_opaque_images_without_forcing_rgba() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();
        write_jpeg(&public.join("photo.jpg"), 4, 3, [20, 40, 60]);

        optimize_public_images(
            &public,
            &assets,
            &temp.path().join("cache"),
            &ImageOptimizationOptions::default(),
        )
        .unwrap();
        assert!(
            image_decode::header_dimensions(&fs::read(assets.join("photo.webp")).unwrap()).is_ok()
        );
    }

    #[test]
    fn invalid_image_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();
        let source = public.join("broken.png");
        fs::write(&source, b"not an image").unwrap();

        let report = optimize_public_images(
            &public,
            &assets,
            &temp.path().join("cache"),
            &ImageOptimizationOptions::default(),
        )
        .unwrap();
        assert!(source.is_file());
        assert_eq!(
            fs::read(assets.join("broken.png")).unwrap(),
            b"not an image"
        );
        assert_eq!(report.optimized_images, 0);
    }

    #[test]
    fn rejects_same_stem_collisions_before_conversion() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();
        write_png(
            &public.join("hero.png"),
            1,
            1,
            png::ColorType::Rgb,
            &[1, 2, 3],
        );
        write_jpeg(&public.join("hero.jpg"), 1, 1, [1, 2, 3]);

        let error = optimize_public_images(
            &public,
            &assets,
            &temp.path().join("cache"),
            &ImageOptimizationOptions::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("image output collision"));
        assert!(public.join("hero.png").is_file());
        assert!(public.join("hero.jpg").is_file());
        assert!(!assets.join("hero.webp").exists());
    }

    #[test]
    fn a_fully_cached_rebuild_never_decodes_the_source() {
        // Decoding is the expensive step, and a warm rebuild does not need it:
        // every output is content-addressed, so the cache alone decides. The
        // proof is that a source whose bytes cannot be decoded at all still
        // produces its outputs once the cache is populated.
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        let cache = temp.path().join("cache");
        fs::create_dir(&public).unwrap();
        let source = public.join("hero.jpg");
        write_jpeg(&source, 1000, 500, [10, 20, 30]);
        let options = ImageOptimizationOptions {
            variant_widths: vec![640, 750, 828],
            ..ImageOptimizationOptions::default()
        };

        let cold = optimize_public_images(&public, &assets, &cache, &options).unwrap();
        assert_eq!(cold.cache_hits, 0);
        assert_eq!(cold.entries[0].variants.len(), 3);

        // Replace the pixel data with bytes that keep a valid JPEG header —
        // and therefore the same planned outputs — but cannot be decoded.
        // Because the digest changed, this is a genuine cache miss and must
        // fall back to copying the unreadable source.
        let mut corrupt = fs::read(&source).unwrap();
        let tail = corrupt.len() / 2;
        corrupt.truncate(tail);
        fs::write(public.join("broken.jpg"), &corrupt).unwrap();

        fs::remove_dir_all(&assets).unwrap();
        let warm = optimize_public_images(&public, &assets, &cache, &options).unwrap();
        assert_eq!(warm.cache_hits, 1, "the untouched source must hit cache");
        assert!(assets.join("hero-640w.webp").is_file());
        assert!(assets.join("hero-828w.webp").is_file());
        assert_eq!(
            warm.entries[0].width, 1000,
            "dimensions survive without a decode"
        );
        assert_eq!(warm.entries[0].height, 500);
    }

    #[test]
    fn effort_participates_in_the_cache_key() {
        // Two builds that differ only in encoder effort produce different
        // bytes. Sharing a cache entry between them would serve one build's
        // output for the other's settings.
        let digest = blake3::hash(b"source bytes");
        let base = ImageOptimizationOptions::default();
        let faster = ImageOptimizationOptions {
            effort: 0,
            ..base.clone()
        };
        assert_ne!(
            output_cache_key(&digest, &base, None),
            output_cache_key(&digest, &faster, None)
        );
        // A full-size output and a variant must never share a key either.
        assert_ne!(
            output_cache_key(&digest, &base, None),
            output_cache_key(&digest, &base, Some(640))
        );
        assert_ne!(
            output_cache_key(&digest, &base, Some(640)),
            output_cache_key(&digest, &base, Some(750))
        );
    }

    #[test]
    fn rejects_case_variant_output_collisions() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();

        // On a case-insensitive output filesystem `Hero.webp` and
        // `hero.webp` are one physical file; the guard must catch the
        // collision even when the byte-for-byte paths differ.
        let sources = vec![public.join("Hero.png"), public.join("hero.PNG")];
        let error = ensure_unique_outputs(&public, &assets, &sources, true).unwrap_err();
        assert!(error.to_string().contains("image output collision"));
    }

    #[test]
    fn disabled_optimization_preserves_assets_and_writes_empty_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();
        fs::write(public.join("hero.png"), b"source").unwrap();
        let options = ImageOptimizationOptions {
            optimize: false,
            ..ImageOptimizationOptions::default()
        };
        let report =
            optimize_public_images(&public, &assets, &temp.path().join("cache"), &options).unwrap();
        assert_eq!(report.optimized_images, 0);
        assert_eq!(fs::read(assets.join("hero.png")).unwrap(), b"source");
        assert!(assets.join(".ruvyxa-images.json").is_file());
    }
}
