use std::error::Error;
use std::io::Cursor;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use unity_rs_core::image_export::{
    ImageEncodeOptions, ImageFormat, ImageRowOrder, PngCompression, write_rgba_image_with_options,
};
use unity_rs_core::texture::RgbaImage;

use crate::export::hash_display_rgba;

type EncodingResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EncodedFormat {
    Png,
    Webp,
    Auto,
}

impl EncodedFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OptimizationEffort {
    None,
    Balanced,
    Small,
}

pub struct EncodingRequest {
    pub format: EncodedFormat,
    pub effort: OptimizationEffort,
    pub compression: PngCompression,
    pub budget_bytes: u64,
    pub timeout_seconds: u64,
}

pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub format: EncodedFormat,
    pub report: EncodingReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct EncodingReport {
    pub baseline_bytes: u64,
    pub selected_bytes: u64,
    pub selected_format: EncodedFormat,
    pub effort: OptimizationEffort,
    pub rgba_sha256: String,
    pub optimization_seconds: f64,
    pub working_set_estimate_bytes: u64,
    pub candidates: Vec<CandidateReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateReport {
    pub encoder: String,
    pub format: EncodedFormat,
    pub status: String,
    pub bytes: Option<u64>,
    pub seconds: f64,
    pub rgba_verified: bool,
    pub selected: bool,
    pub working_set_estimate_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct DecodedImageHash {
    pub width: u32,
    pub height: u32,
    pub rgba_sha256: String,
    pub icc_sha256: Option<String>,
}

pub fn encode_image(image: &RgbaImage, request: &EncodingRequest) -> EncodingResult<EncodedImage> {
    let rgba_bytes = pixel_bytes(image.width, image.height)?;
    if image.pixels.len() as u64 != rgba_bytes {
        return Err("RGBA载荷长度与尺寸不一致".into());
    }
    let baseline_estimate = workspace(rgba_bytes, 5, 8 * MIB)?;
    if baseline_estimate > request.budget_bytes {
        return Err(format!(
            "无损编码基线需要约{baseline_estimate}字节工作区，超过预算{}",
            request.budget_bytes
        )
        .into());
    }
    let rgba_sha256 = hash_display_rgba(image);
    let started = Instant::now();
    let mut best = encode(image, EncodedFormat::Png, request)?;
    verify_candidate(
        &best,
        EncodedFormat::Png,
        image,
        &rgba_sha256,
        request.budget_bytes - rgba_bytes,
    )?;
    let baseline_bytes = best.len() as u64;
    let mut candidates = vec![CandidateReport {
        encoder: "baseline_png".into(),
        format: EncodedFormat::Png,
        status: "verified".into(),
        bytes: Some(baseline_bytes),
        seconds: started.elapsed().as_secs_f64(),
        rgba_verified: true,
        selected: false,
        working_set_estimate_bytes: baseline_estimate,
        error: None,
    }];
    let search_started = Instant::now();
    let deadline = Duration::from_secs(request.timeout_seconds);
    let mut selected = 0;
    let mut format = EncodedFormat::Png;
    let mut peak_estimate = baseline_estimate;
    let png_level = match request.effort {
        OptimizationEffort::None => None,
        OptimizationEffort::Balanced => Some(2),
        OptimizationEffort::Small => Some(4),
    };
    let trials = png_level
        .filter(|_| request.format != EncodedFormat::Webp)
        .map(|level| (format!("oxipng_{level}"), EncodedFormat::Png, Some(level)))
        .into_iter()
        .chain((request.format != EncodedFormat::Png).then_some((
            "image_webp_lossless".into(),
            EncodedFormat::Webp,
            None,
        )));
    for (encoder, candidate_format, level) in trials {
        let estimate = workspace(rgba_bytes, 12, 16 * MIB)?;
        let mandatory = request.format == EncodedFormat::Webp;
        let mut candidate = CandidateReport {
            encoder,
            format: candidate_format,
            status: "rejected".into(),
            bytes: None,
            seconds: 0.0,
            rgba_verified: false,
            selected: false,
            working_set_estimate_bytes: estimate,
            error: None,
        };
        if estimate > request.budget_bytes {
            if mandatory {
                return Err(format!(
                    "无损WebP需要约{estimate}字节工作区，超过预算{}",
                    request.budget_bytes
                )
                .into());
            }
            candidate.status = "skipped_budget".into();
            candidates.push(candidate);
            continue;
        }
        if !mandatory && search_started.elapsed() >= deadline {
            candidate.status = "skipped_timeout".into();
            candidates.push(candidate);
            continue;
        }
        peak_estimate = peak_estimate.max(estimate);
        let trial_started = Instant::now();
        let encoded = if let Some(level) = level {
            let mut options = oxipng::Options::from_preset(level);
            options.optimize_alpha = false;
            options.scale_16 = false;
            options.timeout = Some(deadline.saturating_sub(search_started.elapsed()));
            options.max_decompressed_size = Some(usize::try_from(
                rgba_bytes
                    .checked_add(u64::from(image.height))
                    .ok_or("PNG扫描线长度溢出")?,
            )?);
            oxipng::optimize_from_memory(&best, &options).map_err(Into::into)
        } else {
            encode(image, candidate_format, request)
        };
        match encoded {
            Ok(bytes) => {
                candidate.bytes = Some(bytes.len() as u64);
                let available = request
                    .budget_bytes
                    .checked_sub(rgba_bytes)
                    .and_then(|value| value.checked_sub(best.len() as u64))
                    .ok_or("候选验证预算不足")?;
                match verify_candidate(&bytes, candidate_format, image, &rgba_sha256, available) {
                    Ok(()) => {
                        candidate.status = "verified".into();
                        candidate.rgba_verified = true;
                        if mandatory || bytes.len() < best.len() {
                            best = bytes;
                            format = candidate_format;
                            selected = candidates.len();
                        }
                    }
                    Err(error) if mandatory => return Err(error),
                    Err(error) => candidate.error = Some(error.to_string()),
                }
            }
            Err(error) if mandatory => return Err(error),
            Err(error) => candidate.error = Some(error.to_string()),
        }
        candidate.seconds = trial_started.elapsed().as_secs_f64();
        candidates.push(candidate);
    }
    candidates[selected].selected = true;
    Ok(EncodedImage {
        report: EncodingReport {
            baseline_bytes,
            selected_bytes: best.len() as u64,
            selected_format: format,
            effort: request.effort,
            rgba_sha256,
            optimization_seconds: search_started.elapsed().as_secs_f64(),
            working_set_estimate_bytes: peak_estimate,
            candidates,
        },
        bytes: best,
        format,
    })
}

fn encode(
    image: &RgbaImage,
    format: EncodedFormat,
    request: &EncodingRequest,
) -> EncodingResult<Vec<u8>> {
    let mut bytes = Vec::new();
    write_rgba_image_with_options(
        image,
        match format {
            EncodedFormat::Png => ImageFormat::Png,
            EncodedFormat::Webp => ImageFormat::Webp,
            EncodedFormat::Auto => return Err("自动选择不是具体编码格式".into()),
        },
        ImageRowOrder::UnityDecoded,
        &ImageEncodeOptions {
            png_compression: request.compression,
            maximum_output_bytes: request.budget_bytes,
            ..ImageEncodeOptions::default()
        },
        &mut bytes,
    )?;
    Ok(bytes)
}

fn verify_candidate(
    data: &[u8],
    format: EncodedFormat,
    image: &RgbaImage,
    expected: &str,
    budget: u64,
) -> EncodingResult<()> {
    let decoded = decoded_image_hash(data, format, budget)?;
    if decoded.width != image.width
        || decoded.height != image.height
        || decoded.rgba_sha256 != expected
    {
        return Err("候选图像的尺寸或完整RGBA校验失败".into());
    }
    if decoded.icc_sha256.is_some() {
        return Err("候选图像增加了源纹理不存在的ICC配置".into());
    }
    Ok(())
}

pub fn decoded_image_hash(
    data: &[u8],
    format: EncodedFormat,
    budget: u64,
) -> EncodingResult<DecodedImageHash> {
    if data.len() as u64 > budget {
        return Err("编码图像超过验证内存预算".into());
    }
    match format {
        EncodedFormat::Png => {
            let mut decoder = png::Decoder::new(Cursor::new(data));
            decoder.set_limits(png::Limits {
                bytes: usize::try_from(budget - data.len() as u64)?,
            });
            decoder.set_transformations(png::Transformations::EXPAND);
            let mut reader = decoder.read_info()?;
            let width = reader.info().width;
            let height = reader.info().height;
            let rgba_bytes = pixel_bytes(width, height)?;
            let required = workspace(rgba_bytes, 2, 8 * MIB)?
                .checked_add(data.len() as u64)
                .ok_or("验证预算溢出")?;
            if required > budget {
                return Err(format!("PNG验证需要约{required}字节，超过预算{budget}").into());
            }
            if reader.info().animation_control.is_some() {
                return Err("不支持将动画PNG当作静态贴图验证".into());
            }
            let icc_sha256 = reader
                .info()
                .icc_profile
                .as_ref()
                .map(|profile| hash_bytes(profile));
            let mut pixels = vec![0; reader.output_buffer_size().ok_or("PNG解码长度溢出")?];
            let info = reader.next_frame(&mut pixels)?;
            if info.bit_depth != png::BitDepth::Eight {
                return Err("PNG验证仅支持无损展开为8位的颜色深度".into());
            }
            let rgba_sha256 = hash_channels(
                &pixels[..info.buffer_size()],
                match info.color_type {
                    png::ColorType::Rgba => 4,
                    png::ColorType::Rgb => 3,
                    png::ColorType::GrayscaleAlpha => 2,
                    png::ColorType::Grayscale => 1,
                    png::ColorType::Indexed => return Err("PNG索引色尚未展开".into()),
                },
            );
            Ok(DecodedImageHash {
                width,
                height,
                rgba_sha256,
                icc_sha256,
            })
        }
        EncodedFormat::Webp => {
            let mut decoder = image_webp::WebPDecoder::new(Cursor::new(data))?;
            decoder.set_memory_limit(usize::try_from(budget - data.len() as u64)?);
            if decoder.is_animated() || decoder.is_lossy() {
                return Err("验证目标不是静态无损WebP".into());
            }
            let (width, height) = decoder.dimensions();
            let required = workspace(pixel_bytes(width, height)?, 6, 8 * MIB)?
                .checked_add(data.len() as u64)
                .ok_or("验证预算溢出")?;
            if required > budget {
                return Err(format!("WebP验证需要约{required}字节，超过预算{budget}").into());
            }
            let icc_sha256 = decoder
                .icc_profile()?
                .as_ref()
                .map(|profile| hash_bytes(profile));
            let channels = if decoder.has_alpha() { 4 } else { 3 };
            let mut pixels = vec![0; decoder.output_buffer_size().ok_or("WebP解码长度溢出")?];
            decoder.read_image(&mut pixels)?;
            Ok(DecodedImageHash {
                width,
                height,
                rgba_sha256: hash_channels(&pixels, channels),
                icc_sha256,
            })
        }
        EncodedFormat::Auto => Err("验证必须指定文件的实际编码格式".into()),
    }
}

fn pixel_bytes(width: u32, height: u32) -> EncodingResult<u64> {
    if width == 0 || height == 0 {
        return Err("编码图像尺寸不能为零".into());
    }
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "RGBA长度溢出".into())
}

fn workspace(rgba: u64, factor: u64, fixed: u64) -> EncodingResult<u64> {
    rgba.checked_mul(factor)
        .and_then(|value| value.checked_add(fixed))
        .ok_or_else(|| "编码工作区估算溢出".into())
}

fn hash_channels(pixels: &[u8], channels: usize) -> String {
    let mut hash = Sha256::new();
    if channels == 4 {
        hash.update(pixels);
        return format!("{:x}", hash.finalize());
    }
    pixels
        .chunks_exact(channels)
        .for_each(|pixel| match channels {
            3 => hash.update([pixel[0], pixel[1], pixel[2], 255]),
            2 => hash.update([pixel[0], pixel[0], pixel[0], pixel[1]]),
            1 => hash.update([pixel[0], pixel[0], pixel[0], 255]),
            _ => unreachable!(),
        });
    format!("{:x}", hash.finalize())
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::{
        DecodedImageHash, EncodedFormat, EncodingRequest, OptimizationEffort, decoded_image_hash,
        encode_image, hash_display_rgba,
    };
    use unity_rs_core::image_export::PngCompression;
    use unity_rs_core::texture::RgbaImage;

    fn request(format: EncodedFormat, effort: OptimizationEffort) -> EncodingRequest {
        EncodingRequest {
            format,
            effort,
            compression: PngCompression::Fast,
            budget_bytes: 256 * 1024 * 1024,
            timeout_seconds: 5,
        }
    }

    fn image() -> RgbaImage {
        RgbaImage {
            width: 4,
            height: 2,
            pixels: vec![
                99, 44, 201, 0, 22, 155, 77, 1, 244, 1, 100, 128, 11, 22, 33, 255, 155, 22, 77, 0,
                22, 155, 77, 254, 1, 255, 100, 127, 0, 0, 0, 255,
            ],
        }
    }

    #[test]
    fn transparent_rgb_and_row_order_are_exact() {
        let source = image();
        for format in [EncodedFormat::Png, EncodedFormat::Webp, EncodedFormat::Auto] {
            let encoded =
                encode_image(&source, &request(format, OptimizationEffort::Small)).unwrap();
            let decoded =
                decoded_image_hash(&encoded.bytes, encoded.format, 256 * 1024 * 1024).unwrap();
            assert_eq!(decoded.rgba_sha256, hash_display_rgba(&source));
            assert_eq!(
                (decoded.width, decoded.height),
                (source.width, source.height)
            );
            assert!(
                encoded
                    .report
                    .candidates
                    .iter()
                    .all(|candidate| candidate.rgba_verified)
            );
        }
    }

    #[test]
    fn automatic_selection_never_grows_baseline() {
        let encoded = encode_image(
            &image(),
            &request(EncodedFormat::Auto, OptimizationEffort::Balanced),
        )
        .unwrap();
        assert!(encoded.report.selected_bytes <= encoded.report.baseline_bytes);
        assert_eq!(
            encoded
                .report
                .candidates
                .iter()
                .filter(|candidate| candidate.selected)
                .count(),
            1
        );
        assert_eq!(
            encoded.report.selected_bytes,
            encoded
                .report
                .candidates
                .iter()
                .filter(|candidate| candidate.rgba_verified)
                .filter_map(|candidate| candidate.bytes)
                .min()
                .unwrap()
        );
    }

    #[test]
    fn opaque_palette_reduction_remains_exact() {
        let source = RgbaImage {
            width: 64,
            height: 64,
            pixels: (0..64 * 64)
                .flat_map(|index| {
                    if index % 2 == 0 {
                        [10, 70, 170, 255]
                    } else {
                        [230, 90, 13, 255]
                    }
                })
                .collect(),
        };
        let encoded = encode_image(
            &source,
            &request(EncodedFormat::Png, OptimizationEffort::Small),
        )
        .unwrap();
        assert!(encoded.report.selected_bytes < encoded.report.baseline_bytes);
        assert_ne!(encoded.bytes[25], 6);
        assert_eq!(
            decoded_image_hash(&encoded.bytes, EncodedFormat::Png, 256 * 1024 * 1024)
                .unwrap()
                .rgba_sha256,
            hash_display_rgba(&source)
        );
    }

    #[test]
    fn optional_trials_obey_budget_and_soft_timeout() {
        let source = image();
        let mut budget = request(EncodedFormat::Auto, OptimizationEffort::Small);
        budget.budget_bytes = 10 * 1024 * 1024;
        let encoded = encode_image(&source, &budget).unwrap();
        assert!(
            encoded.report.candidates[1..]
                .iter()
                .all(|candidate| candidate.status == "skipped_budget")
        );
        budget.format = EncodedFormat::Webp;
        assert!(encode_image(&source, &budget).is_err());
        let mut timeout = request(EncodedFormat::Auto, OptimizationEffort::Small);
        timeout.timeout_seconds = 0;
        let encoded = encode_image(&source, &timeout).unwrap();
        assert!(
            encoded.report.candidates[1..]
                .iter()
                .all(|candidate| candidate.status == "skipped_timeout")
        );
        timeout.format = EncodedFormat::Webp;
        assert_eq!(
            encode_image(&source, &timeout).unwrap().format,
            EncodedFormat::Webp
        );
    }

    #[test]
    fn rejected_shapes_and_small_verify_budget_fail() {
        assert!(
            encode_image(
                &RgbaImage {
                    width: 4,
                    height: 3,
                    pixels: vec![0; 4]
                },
                &request(EncodedFormat::Png, OptimizationEffort::None)
            )
            .is_err()
        );
        let encoded = encode_image(
            &image(),
            &request(EncodedFormat::Png, OptimizationEffort::None),
        )
        .unwrap();
        let result: Result<DecodedImageHash, _> =
            decoded_image_hash(&encoded.bytes, EncodedFormat::Png, 1);
        assert!(result.is_err());
    }
}
