//! Fast build-time conversion of public PNG/JPEG assets into WebP files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use image::{DynamicImage, GenericImageView};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use webp::Encoder;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageOptimizationOptions {
    pub optimize: bool,
    pub quality: u8,
    pub lossless: bool,
    /// Keep the original PNG/JPEG next to its WebP output.
    ///
    /// `public/` is a URL contract: whatever the developer puts there is
    /// served at the matching path. Replacing `logo.png` with `logo.webp`
    /// broke that contract everywhere except the Rust server, which quietly
    /// resolves the old URL to the new file (`resolve_public_asset`). A CDN
    /// has no such fallback, so every plain `<img src="/logo.png">` 404s once
    /// the app is deployed — a failure that only ever appears in production.
    pub keep_original: bool,
    /// Zero uses Rayon's global worker count.
    #[serde(rename = "workers")]
    pub parallelism: usize,
}

impl Default for ImageOptimizationOptions {
    fn default() -> Self {
        Self {
            optimize: true,
            quality: 82,
            lossless: false,
            keep_original: true,
            parallelism: 0,
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
}

struct Conversion {
    source: PathBuf,
    output: PathBuf,
    width: u32,
    height: u32,
    source_bytes: u64,
    output_bytes: u64,
    cache_hit: bool,
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
    let Ok(decoded) = image::load_from_memory(&source_data) else {
        copy_asset(source, &unchanged_output)?;
        return Ok(None);
    };
    let (width, height) = decoded.dimensions();
    let output = assets_dir.join(webp_path(relative));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    // The optimized WebP is what `<Image>` and the prerendered HTML point at;
    // the untouched source keeps every other reference to `/logo.png` working
    // on hosts that serve the publish directory straight from a CDN.
    if options.keep_original {
        copy_asset(source, &unchanged_output)?;
    }
    let cache_key = cache_key(&source_data, options);
    let cached = cache_dir.join(format!("{cache_key}.webp"));
    let cache_hit = cached.is_file();

    if cache_hit {
        materialize_cached(&cached, &output)?;
    } else {
        let encoded = encode_webp(decoded, options)?;
        write_cache_entry(&cached, &encoded)?;
        materialize_cached(&cached, &output)?;
    }

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

fn encode_webp(
    decoded: DynamicImage,
    options: &ImageOptimizationOptions,
) -> anyhow::Result<Vec<u8>> {
    let (width, height) = decoded.dimensions();
    let memory = if decoded.color().has_alpha() {
        let pixels = decoded.to_rgba8();
        Encoder::from_rgba(pixels.as_raw(), width, height)
            .encode_simple(options.lossless, options.quality.clamp(1, 100) as f32)
    } else {
        let pixels = decoded.to_rgb8();
        Encoder::from_rgb(pixels.as_raw(), width, height)
            .encode_simple(options.lossless, options.quality.clamp(1, 100) as f32)
    }
    .map_err(|error| anyhow::anyhow!("WebP encoding failed: {error:?}"))?;
    Ok(memory.to_vec())
}

fn cache_key(source: &[u8], options: &ImageOptimizationOptions) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(&[options.quality.clamp(1, 100), u8::from(options.lossless)]);
    hash.update(source);
    hash.finalize().to_hex().to_string()
}

fn write_cache_entry(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    let worker = rayon::current_thread_index().unwrap_or(usize::MAX);
    let temporary = path.with_extension(format!("{}.{worker}.tmp", std::process::id()));
    fs::write(&temporary, bytes)
        .with_context(|| format!("failed to write image cache entry {}", temporary.display()))?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_) if path.is_file() => {
            let _ = fs::remove_file(temporary);
            Ok(())
        }
        Err(error) => Err(error).context("failed to publish image cache entry"),
    }
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
    use image::{ImageBuffer, Rgb, Rgba};

    #[test]
    fn publishes_one_webp_beside_the_original_and_reuses_cache() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        let cache = temp.path().join("cache");
        fs::create_dir(&public).unwrap();
        let source = public.join("hero.png");
        let image = ImageBuffer::from_pixel(4, 3, Rgba([20u8, 40, 60, 255]));
        image.save(&source).unwrap();
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
        // The original stays published: a static host has no server-side
        // format fallback, so dropping it 404s every plain
        // `<img src="/hero.png">` the moment the app is deployed.
        assert!(assets.join("hero.png").is_file());
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

        // Opting out trades that compatibility for the smaller publish
        // directory, and must then actually drop the source.
        fs::remove_dir_all(&assets).unwrap();
        optimize_public_images(
            &public,
            &assets,
            &cache,
            &ImageOptimizationOptions {
                keep_original: false,
                ..ImageOptimizationOptions::default()
            },
        )
        .unwrap();
        assert!(assets.join("hero.webp").is_file());
        assert!(!assets.join("hero.png").exists());
    }

    #[test]
    fn encodes_opaque_images_without_forcing_rgba() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        let assets = temp.path().join("assets");
        fs::create_dir(&public).unwrap();
        ImageBuffer::from_pixel(4, 3, Rgb([20u8, 40, 60]))
            .save(public.join("photo.jpg"))
            .unwrap();

        optimize_public_images(
            &public,
            &assets,
            &temp.path().join("cache"),
            &ImageOptimizationOptions::default(),
        )
        .unwrap();
        assert!(image::open(assets.join("photo.webp")).is_ok());
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
        let image = ImageBuffer::from_pixel(1, 1, Rgb([1u8, 2, 3]));
        image.save(public.join("hero.png")).unwrap();
        image.save(public.join("hero.jpg")).unwrap();

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
