use std::error::Error;
use std::io::{Read, Write};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use unity_rs_core::endian::{Endian, EndianReader};
use unity_rs_core::image_export::{
    ImageEncodeOptions, ImageFormat, ImageRowOrder, PngCompression, write_rgba_image_with_options,
};
use unity_rs_core::loader::AssetCollection;
use unity_rs_core::serialized::SerializedFile;
use unity_rs_core::simple_assets::{SimpleAssetReadLimits, read_font, read_video_clip};
use unity_rs_core::source::Region;
use unity_rs_core::texture::{
    RgbaImage, Texture2D, TextureFormat, TextureReadLimits, read_texture2d,
};
use unity_rs_core::type_tree::{TypeTreeReadLimits, TypeValue};

use crate::encoding::{
    EncodedFormat, EncodingReport, EncodingRequest, OptimizationEffort, encode_image,
};

pub type ExportResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Native,
    Png,
    Webp,
    Auto,
}

#[derive(Debug, Clone)]
pub struct ExportRequest {
    pub format: ExportFormat,
    pub all_mips: bool,
    pub decoder_budget_bytes: u64,
    pub png_compression: PngCompression,
    pub alpha8_rgb: u8,
    pub optimize: OptimizationEffort,
    pub optimize_seconds: u64,
}

impl Default for ExportRequest {
    fn default() -> Self {
        Self {
            format: ExportFormat::Native,
            all_mips: false,
            decoder_budget_bytes: 128 * 1024 * 1024,
            png_compression: PngCompression::Fast,
            alpha8_rgb: 0,
            optimize: OptimizationEffort::None,
            optimize_seconds: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputDescriptor {
    pub relative_name: String,
    pub kind: String,
    pub name: String,
    pub face: Option<u32>,
    pub mip: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub rgba_sha256: Option<String>,
    pub source_payload_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<EncodingReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WrittenOutput {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportedFile {
    pub descriptor: OutputDescriptor,
    pub output: WrittenOutput,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectExport {
    pub kind: String,
    pub status: String,
    pub name: String,
    pub metadata: Value,
    pub outputs: Vec<ExportedFile>,
}

pub enum OutputContent<'a> {
    Region(&'a Region),
    Encoded(&'a [u8]),
    Png {
        image: &'a RgbaImage,
        options: &'a ImageEncodeOptions,
    },
}

impl OutputContent<'_> {
    pub fn write_to<W: Write>(&self, writer: &mut W) -> ExportResult<u64> {
        match self {
            Self::Region(region) => Ok(region.copy_range(0, region.len(), writer)?),
            Self::Encoded(bytes) => {
                writer.write_all(bytes)?;
                Ok(bytes.len() as u64)
            }
            Self::Png { image, options } => Ok(write_rgba_image_with_options(
                image,
                ImageFormat::Png,
                ImageRowOrder::UnityDecoded,
                options,
                writer,
            )?),
        }
    }
}

pub trait OutputSink {
    fn write(
        &mut self,
        descriptor: &OutputDescriptor,
        content: OutputContent<'_>,
    ) -> ExportResult<WrittenOutput>;
}

pub fn export_object(
    collection: &AssetCollection,
    file_index: usize,
    object_index: usize,
    request: &ExportRequest,
    sink: &mut dyn OutputSink,
) -> ExportResult<ObjectExport> {
    let file = &collection
        .serialized_files()
        .get(file_index)
        .ok_or("序列化文件索引越界")?
        .file;
    let object = file.objects.get(object_index).ok_or("对象索引越界")?;
    match object.class_id {
        28 | 89 => export_texture(collection, file, object_index, request, sink),
        49 => {
            let (name, payload) = read_text_region(file, object_index)?;
            let extension = name
                .rsplit_once('.')
                .map(|(_, value)| value)
                .unwrap_or("bytes");
            let extension = safe_extension(&format!(".{extension}")).to_owned();
            export_region(
                "textasset",
                &name,
                &format!("payload{extension}"),
                &payload,
                json!({"content_encoding": "raw_bytes"}),
                sink,
            )
        }
        128 => {
            let value = read_font(file, object_index, simple_limits())?;
            export_region(
                "font",
                &value.name,
                &format!("payload{}", value.suggested_extension),
                &value.payload,
                json!({"payload_kind": value.payload_kind}),
                sink,
            )
        }
        329 => {
            let value = read_video_clip(collection, file, object_index, simple_limits())?;
            export_region(
                "video",
                &value.name,
                &format!("payload{}", safe_extension(&value.suggested_extension)),
                &value.payload,
                json!({"payload_kind": value.payload_kind}),
                sink,
            )
        }
        84 => {
            let tree = small_tree(file, object_index)?;
            let name = string_field(&tree, "m_Name")?;
            let metadata = json!({
                "width": integer_field(&tree, "m_Width")?,
                "height": integer_field(&tree, "m_Height")?,
                "reason": "运行时渲染目标没有静态像素",
                "serialized_file_required": true,
            });
            if request.format == ExportFormat::Native {
                return export_region(
                    "render_texture",
                    name,
                    "object.bin",
                    &file.object_region(object_index)?,
                    metadata,
                    sink,
                );
            }
            Ok(ObjectExport {
                kind: "render_texture".into(),
                status: "metadata_only".into(),
                name: name.to_owned(),
                metadata,
                outputs: Vec::new(),
            })
        }
        _ if request.format == ExportFormat::Native => export_region(
            "serialized_object",
            "",
            "object.bin",
            &file.object_region(object_index)?,
            json!({
                "class_id": object.class_id,
                "reason": "保存原始序列化对象；没有宣称语义解码成功",
                "serialized_file_required": true,
            }),
            sink,
        ),
        _ => Ok(ObjectExport {
            kind: "serialized_object".into(),
            status: "decoded_not_supported".into(),
            name: String::new(),
            metadata: json!({"class_id": object.class_id}),
            outputs: Vec::new(),
        }),
    }
}

fn export_texture(
    collection: &AssetCollection,
    file: &SerializedFile,
    object_index: usize,
    request: &ExportRequest,
    sink: &mut dyn OutputSink,
) -> ExportResult<ObjectExport> {
    let cube = file.objects[object_index].class_id == 89;
    let tree = small_tree(file, object_index)?;
    let texture = if cube {
        cubemap_texture(collection, file, object_index, &tree)?
    } else {
        read_texture2d(collection, file, object_index, texture_limits(request))?
    };
    let kind = if cube { "cubemap" } else { "texture2d" };
    let mut result = ObjectExport {
        kind: kind.into(),
        status: if request.format == ExportFormat::Native {
            "raw_verified"
        } else {
            "decoded_verified"
        }
        .into(),
        name: texture.name.clone(),
        metadata: json!({
            "width": texture.width,
            "height": texture.height,
            "texture_format": texture.format.0,
            "mip_count": texture.mip_count,
            "mips_stripped": texture.mips_stripped,
            "image_count": texture.image_count,
            "dimension": texture.dimension,
            "color_space": integer_field(&tree, "m_ColorSpace")?,
            "complete_image_size": texture.complete_image_size,
            "payload_bytes": texture.data.len(),
            "row_order": "unity_decoded_bottom_up",
            "alpha8_derived_rgb": if texture.format == TextureFormat::ALPHA8 {
                Some(request.alpha8_rgb)
            } else {
                None
            },
            "face_order": if cube { Some("unity_stored_face_order_0_to_5") } else { None },
        }),
        outputs: Vec::new(),
    };
    if texture.width == 0 || texture.height == 0 || texture.data.is_empty() {
        result.status = "expected_empty".into();
        result.metadata["reason"] = json!("对象没有静态像素载荷");
        if request.format == ExportFormat::Native {
            let payload = file.object_region(object_index)?;
            let descriptor = OutputDescriptor {
                source_payload_sha256: Some(hash_region(&payload)?),
                ..descriptor("object.bin", kind, &texture.name)
            };
            let output = sink.write(&descriptor, OutputContent::Region(&payload))?;
            result.outputs.push(ExportedFile { descriptor, output });
            result.status = "raw_verified".into();
            result.metadata["pixel_status"] = json!("expected_empty");
            result.metadata["serialized_file_required"] = json!(true);
        }
        return Ok(result);
    }
    if request.format == ExportFormat::Native {
        match texture_layout(&texture, cube) {
            Ok(layout) => result.metadata["layout"] = layout,
            Err(error) => {
                result.metadata["layout_status"] = json!("not_supported");
                result.metadata["layout_error"] = json!(error.to_string());
            }
        }
        let descriptor = OutputDescriptor {
            source_payload_sha256: Some(hash_region(&texture.data)?),
            ..descriptor("payload.bin", kind, &texture.name)
        };
        let output = sink.write(&descriptor, OutputContent::Region(&texture.data))?;
        result.outputs.push(ExportedFile { descriptor, output });
        return Ok(result);
    }
    if !rgba8_exact_format(texture.format) {
        result.status = "decoded_not_supported".into();
        result.metadata["reason"] = json!("该像素格式不能在此首版中承诺RGBA8保真，请使用native");
        return Ok(result);
    }
    let layout = texture_layout(&texture, cube)?;
    let faces = if cube { 6 } else { 1 };
    let levels = if request.all_mips {
        texture.mips_stripped..texture.mip_count
    } else {
        texture.mips_stripped..texture.mips_stripped + 1
    };
    let face_bytes = if cube {
        u64::from(texture.complete_image_size)
    } else {
        texture.data.len()
    };
    for face in 0..faces {
        let surface = Texture2D {
            data: texture
                .data
                .subregion(face_bytes * u64::from(face), face_bytes)?,
            image_count: 1,
            dimension: 2,
            ..texture.clone()
        };
        for mip in levels.clone() {
            let (width, height) = surface.mip_dimensions(mip)?;
            let rgba_bytes = u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|value| value.checked_mul(4))
                .ok_or("像素大小溢出")?;
            let payload_bytes = surface.mip_region(mip)?.len();
            let working_bytes = rgba_bytes
                .checked_mul(2)
                .and_then(|value| value.checked_add(payload_bytes))
                .and_then(|value| value.checked_add(1024 * 1024))
                .ok_or("解码预算溢出")?;
            if working_bytes > request.decoder_budget_bytes {
                return Err(format!(
                    "该面及mip需要约{working_bytes}字节解码工作区，超过预算{}",
                    request.decoder_budget_bytes
                )
                .into());
            }
            let mut image = surface.decode_mip_rgba8(mip, texture_limits(request))?;
            if texture.format == TextureFormat::ALPHA8 {
                image.pixels.chunks_exact_mut(4).for_each(|pixel| {
                    pixel[..3].fill(request.alpha8_rgb);
                });
            }
            let encoded = if request.format == ExportFormat::Png
                && request.optimize == OptimizationEffort::None
            {
                None
            } else {
                Some(encode_image(
                    &image,
                    &EncodingRequest {
                        format: match request.format {
                            ExportFormat::Png => EncodedFormat::Png,
                            ExportFormat::Webp => EncodedFormat::Webp,
                            ExportFormat::Auto => EncodedFormat::Auto,
                            ExportFormat::Native => unreachable!(),
                        },
                        effort: request.optimize,
                        compression: request.png_compression,
                        budget_bytes: request.decoder_budget_bytes,
                        timeout_seconds: request.optimize_seconds,
                    },
                )?)
            };
            let extension = encoded
                .as_ref()
                .map(|value| value.format.extension())
                .unwrap_or("png");
            let descriptor = OutputDescriptor {
                relative_name: if cube {
                    format!("face-{face}/mip-{mip:02}.{extension}")
                } else {
                    format!("mip-{mip:02}.{extension}")
                },
                kind: kind.into(),
                name: texture.name.clone(),
                face: cube.then_some(face),
                mip: Some(mip),
                width: Some(width),
                height: Some(height),
                rgba_sha256: Some(hash_display_rgba(&image)),
                source_payload_sha256: None,
                encoding: encoded.as_ref().map(|value| value.report.clone()),
            };
            let options = ImageEncodeOptions {
                png_compression: request.png_compression,
                maximum_output_bytes: request.decoder_budget_bytes,
                ..ImageEncodeOptions::default()
            };
            let output = sink.write(
                &descriptor,
                match &encoded {
                    Some(value) => OutputContent::Encoded(&value.bytes),
                    None => OutputContent::Png {
                        image: &image,
                        options: &options,
                    },
                },
            )?;
            result.outputs.push(ExportedFile { descriptor, output });
        }
    }
    result.metadata["layout"] = layout;
    Ok(result)
}

fn export_region(
    kind: &str,
    name: &str,
    relative_name: &str,
    payload: &Region,
    metadata: Value,
    sink: &mut dyn OutputSink,
) -> ExportResult<ObjectExport> {
    let descriptor = OutputDescriptor {
        source_payload_sha256: Some(hash_region(payload)?),
        ..descriptor(relative_name, kind, name)
    };
    let output = sink.write(&descriptor, OutputContent::Region(payload))?;
    Ok(ObjectExport {
        kind: kind.into(),
        status: "raw_verified".into(),
        name: name.to_owned(),
        metadata,
        outputs: vec![ExportedFile { descriptor, output }],
    })
}

fn descriptor(relative_name: &str, kind: &str, name: &str) -> OutputDescriptor {
    OutputDescriptor {
        relative_name: relative_name.into(),
        kind: kind.into(),
        name: name.into(),
        face: None,
        mip: None,
        width: None,
        height: None,
        rgba_sha256: None,
        source_payload_sha256: None,
        encoding: None,
    }
}

fn read_text_region(file: &SerializedFile, object_index: usize) -> ExportResult<(String, Region)> {
    let object = &file.objects[object_index];
    let payload = file.object_region(object_index)?;
    let mut reader = EndianReader::new(
        payload.cursor(),
        if file.header.endianness == 0 {
            Endian::Little
        } else {
            Endian::Big
        },
    );
    if file.target_platform == -2 {
        reader.read_u32()?;
        for _ in 0..2 {
            reader.read_i32()?;
            if file.header.version.0 < 14 {
                reader.read_i32()?;
            } else {
                reader.read_i64()?;
            }
        }
    }
    let length = u32::try_from(reader.read_i32()?).map_err(|_| "TextAsset名称长度为负")?;
    if length > 1024 * 1024 {
        return Err("TextAsset名称超过1MiB".into());
    }
    let name = reader.read_utf8(length as usize)?;
    if length != 0 {
        let absolute = object.byte_start + reader.position()?;
        reader.set_position(absolute.div_ceil(4) * 4 - object.byte_start)?;
    }
    let length = u64::try_from(reader.read_i32()?).map_err(|_| "TextAsset载荷长度为负")?;
    Ok((name, payload.subregion(reader.position()?, length)?))
}

fn cubemap_texture(
    collection: &AssetCollection,
    file: &SerializedFile,
    object_index: usize,
    tree: &TypeValue,
) -> ExportResult<Texture2D> {
    let payload = file.object_region(object_index)?;
    let data = match field(tree, "image data")? {
        TypeValue::TypelessData { offset, size } if *size > 0 => {
            payload.subregion(*offset, *size)?
        }
        TypeValue::TypelessData { .. } => {
            let stream = field(tree, "m_StreamData")?;
            let path = string_field(stream, "path")?;
            let resource = collection.resource(path).ok_or("Cubemap外部资源未加载")?;
            resource.region.subregion(
                unsigned_field(stream, "offset")?,
                unsigned_field(stream, "size")?,
            )?
        }
        _ => return Err("Cubemap像素字段不是受支持的原始span".into()),
    };
    let texture = Texture2D {
        path_id: file.objects[object_index].path_id,
        name: string_field(tree, "m_Name")?.to_owned(),
        width: u32_field(tree, "m_Width")?,
        height: u32_field(tree, "m_Height")?,
        complete_image_size: u32_field(tree, "m_CompleteImageSize")?,
        mips_stripped: u32_field(tree, "m_MipsStripped")?,
        mip_count: u32_field(tree, "m_MipCount")?,
        format: TextureFormat(i32::try_from(integer_field(tree, "m_TextureFormat")?)?),
        image_count: u32_field(tree, "m_ImageCount")?,
        dimension: i32::try_from(integer_field(tree, "m_TextureDimension")?)?,
        platform_blob_size: 0,
        platform_blob: None,
        target_platform: file.target_platform,
        uses_unity_crunch: true,
        data,
    };
    if texture.image_count != 6 || texture.width != texture.height {
        return Err("Cubemap不是已验证的六面正方形布局".into());
    }
    let platform_bytes = match field(tree, "m_PlatformBlob")? {
        TypeValue::TypelessData { size, .. } => *size,
        TypeValue::ByteArray { bytes, .. } => bytes.len() as u64,
        TypeValue::Array(values) => values.len() as u64,
        _ => return Err("Cubemap平台字段不是字节结构".into()),
    };
    if platform_bytes != 0 {
        return Err("Cubemap平台特定排列尚未验证".into());
    }
    Ok(texture)
}

fn texture_layout(texture: &Texture2D, cube: bool) -> ExportResult<Value> {
    if cube && !matches!(texture.format.0, 47..=59) {
        return Err("Cubemap首版只支持已验证的ASTC或ETC2线性块布局".into());
    }
    if texture.image_count != 1 && !cube {
        return Err("多层Texture2D布局未验证，不能作为单面导出".into());
    }
    let faces = if cube { 6 } else { 1 };
    let face_bytes = if cube {
        u64::from(texture.complete_image_size)
    } else {
        texture.data.len()
    };
    if face_bytes.checked_mul(faces).ok_or("六面长度溢出")? != texture.data.len() {
        return Err("Cubemap载荷长度与六面步长不符".into());
    }
    let mut layout = Vec::new();
    for face in 0..faces {
        let surface = Texture2D {
            image_count: 1,
            dimension: 2,
            data: texture.data.subregion(face * face_bytes, face_bytes)?,
            ..texture.clone()
        };
        let mut offset = 0_u64;
        for mip in texture.mips_stripped..texture.mip_count {
            let region = surface.mip_region(mip)?;
            let (width, height) = surface.mip_dimensions(mip)?;
            layout.push(json!({
                "face": if cube { Some(face) } else { None },
                "mip": mip, "offset": face * face_bytes + offset, "bytes": region.len(),
                "width": width, "height": height,
            }));
            offset = offset.checked_add(region.len()).ok_or("mip长度溢出")?;
        }
        if offset > face_bytes {
            return Err("mip链超出面载荷范围".into());
        }
    }
    Ok(json!({"faces": faces, "face_stride": face_bytes, "subresources": layout}))
}

fn small_tree(file: &SerializedFile, object_index: usize) -> ExportResult<TypeValue> {
    Ok(file.read_type_tree_value_with_limits(
        object_index,
        TypeTreeReadLimits {
            maximum_values: 4096,
            maximum_array_elements: 4096,
            maximum_string_bytes: 1024 * 1024,
            maximum_byte_array_bytes: 1024 * 1024,
            maximum_materialized_bytes: 2 * 1024 * 1024,
            ..TypeTreeReadLimits::default()
        },
    )?)
}

fn field<'a>(value: &'a TypeValue, name: &str) -> ExportResult<&'a TypeValue> {
    let TypeValue::Object(fields) = value else {
        return Err("纹理元数据字段不是结构体".into());
    };
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
        .ok_or_else(|| format!("纹理元数据缺少{name}").into())
}

fn integer_field(value: &TypeValue, name: &str) -> ExportResult<i64> {
    match field(value, name)? {
        TypeValue::Signed(value) => Ok(*value),
        TypeValue::Unsigned(value) => Ok(i64::try_from(*value)?),
        _ => Err(format!("纹理元数据{name}不是整数").into()),
    }
}

fn unsigned_field(value: &TypeValue, name: &str) -> ExportResult<u64> {
    Ok(u64::try_from(integer_field(value, name)?)?)
}

fn u32_field(value: &TypeValue, name: &str) -> ExportResult<u32> {
    Ok(u32::try_from(unsigned_field(value, name)?)?)
}

fn string_field<'a>(value: &'a TypeValue, name: &str) -> ExportResult<&'a str> {
    match field(value, name)? {
        TypeValue::String(value) => Ok(value),
        _ => Err(format!("纹理元数据{name}不是字符串").into()),
    }
}

fn texture_limits(request: &ExportRequest) -> TextureReadLimits {
    TextureReadLimits {
        maximum_output_bytes: request.decoder_budget_bytes,
        maximum_decoder_working_bytes: request.decoder_budget_bytes,
        maximum_payload_bytes: u64::MAX,
        maximum_string_bytes: 1024 * 1024,
        maximum_platform_blob_bytes: 1024 * 1024,
        ..TextureReadLimits::default()
    }
}

fn simple_limits() -> SimpleAssetReadLimits {
    SimpleAssetReadLimits {
        maximum_string_bytes: 1024 * 1024,
        maximum_payload_bytes: u64::MAX,
        ..SimpleAssetReadLimits::default()
    }
}

fn rgba8_exact_format(format: TextureFormat) -> bool {
    matches!(format.0, 1..=5 | 7..=8 | 10..=14 | 25..=27 | 34..=36 | 45..=59 | 63)
}

fn safe_extension(extension: &str) -> &str {
    if extension.starts_with('.')
        && extension.len() >= 2
        && extension.len() <= 12
        && extension[1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return extension;
    }
    ".bin"
}

pub fn hash_region(region: &Region) -> ExportResult<String> {
    let mut digest = Sha256::new();
    let mut cursor = region.cursor();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = cursor.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn hash_display_rgba(image: &RgbaImage) -> String {
    let mut digest = Sha256::new();
    let stride = image.width as usize * 4;
    image
        .pixels
        .chunks_exact(stride)
        .rev()
        .for_each(|row| digest.update(row));
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use unity_rs_core::source::Region;
    use unity_rs_core::texture::{RgbaImage, Texture2D, TextureFormat};

    use super::{hash_display_rgba, hash_region, safe_extension, texture_layout};

    fn texture(face_count: u32, face_bytes: u32, payload_bytes: usize) -> Texture2D {
        Texture2D {
            path_id: 7,
            name: "test".into(),
            width: 8,
            height: 8,
            complete_image_size: face_bytes,
            mips_stripped: 0,
            mip_count: 4,
            format: TextureFormat::ASTC_RGB_6X6,
            image_count: face_count,
            dimension: 2,
            platform_blob_size: 0,
            platform_blob: None,
            target_platform: 13,
            uses_unity_crunch: true,
            data: Region::from_bytes(vec![0; payload_bytes]),
        }
    }

    #[test]
    fn cube_layout_retains_all_faces_and_mips() {
        let layout = texture_layout(&texture(6, 112, 672), true).unwrap();
        let rows = layout["subresources"].as_array().unwrap();
        assert_eq!(rows.len(), 24);
        assert_eq!(rows[0]["bytes"], json!(64));
        assert_eq!(rows[4]["face"], json!(1));
        assert_eq!(rows[4]["offset"], json!(112));
        assert_eq!(rows[23]["offset"], json!(656));
        assert_eq!(rows[23]["width"], json!(1));
    }

    #[test]
    fn cube_layout_rejects_payload_shortfall() {
        assert!(texture_layout(&texture(6, 112, 671), true).is_err());
    }

    #[test]
    fn cube_layout_keeps_declared_padding() {
        let layout = texture_layout(&texture(6, 128, 768), true).unwrap();
        assert_eq!(layout["face_stride"], json!(128));
        assert_eq!(layout["subresources"][4]["offset"], json!(128));
    }

    #[test]
    fn region_hash_covers_bytes_without_text_conversion() {
        let bytes = vec![0, 0xff, 0x80, b'X'];
        let expected = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(hash_region(&Region::from_bytes(bytes)).unwrap(), expected);
    }

    #[test]
    fn rgba_hash_uses_display_rows_and_transparent_rgb() {
        let image = RgbaImage {
            width: 1,
            height: 2,
            pixels: vec![1, 2, 3, 0, 4, 5, 6, 0],
        };
        let expected = format!("{:x}", Sha256::digest([4, 5, 6, 0, 1, 2, 3, 0]));
        assert_eq!(hash_display_rgba(&image), expected);
        assert_ne!(
            hash_display_rgba(&image),
            format!("{:x}", Sha256::digest([0; 8]))
        );
    }

    #[test]
    fn source_extensions_cannot_escape_output() {
        assert_eq!(safe_extension(".skel"), ".skel");
        assert_eq!(safe_extension("."), ".bin");
        assert_eq!(safe_extension("../oops"), ".bin");
        assert_eq!(safe_extension(".png/../../oops"), ".bin");
    }
}
