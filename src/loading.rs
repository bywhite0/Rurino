use std::collections::BTreeMap;
use std::io;
use std::mem::size_of;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use unity_rs_core::bundle::{
    BundleParseLimits, CompressionType, ENTRY_REGION_CACHE_BYTES, UnityFsBundle,
};
use unity_rs_core::endian::{Endian, EndianReader};
use unity_rs_core::file_type::{FileType, HEADER_SCAN_LENGTH, detect_file_type};
use unity_rs_core::loader::{AssetCollection, LoadedResource, LoadedSerializedFile};
use unity_rs_core::serialized::{
    ObjectInfo, SerializedFile, SerializedOpenOptions, SerializedParseLimits, TypeTreeNode,
};
use unity_rs_core::serialized_file::SerializedFileHeader;
use unity_rs_core::source::{Region, Source};
use unity_rs_core::{Error, Result};

const MIB: u64 = 1024 * 1024;

pub struct LoadedBundle {
    pub collection: AssetCollection,
    pub stats: BundleStats,
    pub io: Arc<ReadStats>,
    pub source: Region,
}

#[derive(Debug, Serialize)]
pub struct BundleStats {
    pub input_bytes: u64,
    pub entries: usize,
    pub storage_blocks: usize,
    pub serialized_files: usize,
    pub resources: usize,
    pub objects: usize,
    pub classes: BTreeMap<i32, usize>,
    pub unity_versions: Vec<String>,
    pub maximum_compressed_block_bytes: u64,
    pub maximum_uncompressed_block_bytes: u64,
    pub uncompressed_entry_bytes: u64,
    pub serialized_metadata_bytes: u64,
    pub metadata_heap_estimate_bytes: u64,
    pub entry_cache_allowance_bytes: u64,
    pub admission_estimate_bytes: u64,
    pub memory_budget_bytes: u64,
    pub budget_contract: &'static str,
}

#[derive(Debug, Default)]
pub struct ReadStats {
    calls: AtomicU64,
    bytes: AtomicU64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ReadSnapshot {
    pub source_read_calls: u64,
    pub source_read_bytes: u64,
    pub meaning: &'static str,
}

impl ReadStats {
    pub fn snapshot(&self) -> ReadSnapshot {
        ReadSnapshot {
            source_read_calls: self.calls.load(Ordering::Relaxed),
            source_read_bytes: self.bytes.load(Ordering::Relaxed),
            meaning: "文件源成功读取请求字节，包含重复读取，不等于物理磁盘读取量",
        }
    }
}

struct CountedSource {
    region: Region,
    stats: Arc<ReadStats>,
}

impl Source for CountedSource {
    fn len(&self) -> u64 {
        self.region.len()
    }

    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        self.region
            .read_exact_at(offset, output)
            .map_err(|error| match error {
                Error::Io(error) => error,
                error => io::Error::new(io::ErrorKind::InvalidData, error),
            })?;
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes
            .fetch_add(output.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

pub fn load_bundle(path: &Path, budget: u64) -> Result<LoadedBundle> {
    if budget < 16 * MIB {
        return Err(Error::invalid_data("内存软预算至少为 16 MiB"));
    }
    let original = Region::from_file(path)?;
    let io = Arc::new(ReadStats::default());
    let source = Region::new(
        Arc::new(CountedSource {
            region: original.clone(),
            stats: Arc::clone(&io),
        }),
        0,
        original.len(),
    )?;
    let mut prefix = [0_u8; HEADER_SCAN_LENGTH];
    let prefix_length = prefix.len().min(source.len() as usize);
    source.read_exact_at(0, &mut prefix[..prefix_length])?;
    let detection = detect_file_type(&prefix[..prefix_length], source.len());
    if detection.file_type != FileType::BundleFile {
        return Err(Error::unsupported(
            "首版按需读取入口仅支持 UnityFS AssetBundle",
        ));
    }
    let region = source.subregion(detection.data_offset, source.len() - detection.data_offset)?;
    let bundle = UnityFsBundle::open_with_limits(&region, bundle_limits(budget))?;
    let maximum_compressed_block_bytes = bundle
        .blocks
        .iter()
        .map(|block| u64::from(block.compressed_size))
        .max()
        .unwrap_or(0);
    let maximum_uncompressed_block_bytes = bundle
        .blocks
        .iter()
        .map(|block| u64::from(block.uncompressed_size))
        .max()
        .unwrap_or(0);
    let cache_allowance = bundle.entries.iter().try_fold(0_u64, |total, entry| {
        let end = entry
            .offset
            .checked_add(entry.size)
            .ok_or_else(|| Error::invalid_data("entry 范围溢出"))?;
        let blocks = bundle
            .blocks
            .iter()
            .filter(|block| {
                block.compression != CompressionType::None
                    && block.uncompressed_offset < end
                    && block.uncompressed_offset + u64::from(block.uncompressed_size) > entry.offset
            })
            .collect::<Vec<_>>();
        let largest = blocks
            .iter()
            .map(|block| u64::from(block.uncompressed_size))
            .max()
            .unwrap_or(0);
        let total_covered = blocks
            .iter()
            .map(|block| u64::from(block.uncompressed_size))
            .sum::<u64>();
        total
            .checked_add(total_covered.min((ENTRY_REGION_CACHE_BYTES as u64).max(largest)))
            .ok_or_else(|| Error::invalid_data("entry 缓存预算溢出"))
    })?;
    let temporary_block_bytes = maximum_compressed_block_bytes
        .checked_add(maximum_uncompressed_block_bytes)
        .ok_or_else(|| Error::invalid_data("压缩块预算溢出"))?;
    require_budget(
        cache_allowance + temporary_block_bytes,
        budget,
        "解压块和缓存",
    )?;
    let metadata_limit = (budget / 4).min(32 * MIB);
    let mut serialized_metadata_bytes = 0_u64;
    let mut metadata_heap_estimate_bytes = 0_u64;
    let mut serialized_files = Vec::new();
    let mut resources = Vec::new();
    let input_name = path.to_string_lossy();
    for (index, entry) in bundle.entries.iter().enumerate() {
        let entry_region = bundle.entry_region(index)?;
        let entry_path = format!("{input_name}::{}", entry.path);
        // 资源流保持范围引用，索引不触碰其压缩载荷
        if entry.path.ends_with(".resS") || entry.path.ends_with(".resource") {
            resources.push(LoadedResource {
                path: entry_path,
                region: entry_region,
            });
            continue;
        }
        let length = prefix.len().min(entry_region.len() as usize);
        entry_region.read_exact_at(0, &mut prefix[..length])?;
        let detected = detect_file_type(&prefix[..length], entry_region.len());
        if detected.file_type == FileType::ResourceFile {
            resources.push(LoadedResource {
                path: entry_path,
                region: entry_region,
            });
            continue;
        }
        if detected.file_type != FileType::AssetsFile {
            return Err(Error::unsupported(format!(
                "不支持嵌套的 {}：{}",
                detected.file_type, entry.path
            )));
        }
        let header =
            SerializedFileHeader::read(&mut EndianReader::new(entry_region.cursor(), Endian::Big))?;
        serialized_metadata_bytes = serialized_metadata_bytes
            .checked_add(u64::from(header.metadata_size))
            .ok_or_else(|| Error::invalid_data("元数据累计大小溢出"))?;
        require_budget(serialized_metadata_bytes, metadata_limit, "序列化元数据")?;
        let limits = serialized_limits(
            metadata_limit - serialized_metadata_bytes + u64::from(header.metadata_size),
        );
        let file = SerializedFile::open_with_options(
            entry_region,
            SerializedOpenOptions {
                limits,
                bundle_version_hint: Some(bundle.header.common.unity_revision.clone()),
                ..SerializedOpenOptions::default()
            },
        )?;
        metadata_heap_estimate_bytes += metadata_heap_estimate(&file);
        require_budget(
            cache_allowance + temporary_block_bytes + metadata_heap_estimate_bytes,
            budget,
            "加载准入估计",
        )?;
        serialized_files.push(LoadedSerializedFile {
            path: entry_path,
            file,
        });
    }
    let mut classes = BTreeMap::new();
    let objects = serialized_files
        .iter()
        .map(|file| {
            for object in &file.file.objects {
                *classes.entry(object.class_id).or_insert(0) += 1;
            }
            file.file.objects.len()
        })
        .sum();
    let mut unity_versions = serialized_files
        .iter()
        .map(|file| file.file.unity_version_string.clone())
        .collect::<Vec<_>>();
    unity_versions.sort();
    unity_versions.dedup();
    let stats = BundleStats {
        input_bytes: source.len(),
        entries: bundle.entries.len(),
        storage_blocks: bundle.blocks.len(),
        serialized_files: serialized_files.len(),
        resources: resources.len(),
        objects,
        classes,
        unity_versions,
        maximum_compressed_block_bytes,
        maximum_uncompressed_block_bytes,
        uncompressed_entry_bytes: bundle.entries.iter().map(|entry| entry.size).sum(),
        serialized_metadata_bytes,
        metadata_heap_estimate_bytes,
        entry_cache_allowance_bytes: cache_allowance,
        admission_estimate_bytes: cache_allowance
            + temporary_block_bytes
            + metadata_heap_estimate_bytes,
        memory_budget_bytes: budget,
        budget_contract: "加载准入估计与解析上限；不包含编码器工作区和操作系统分配器开销，不是硬RSS限制",
    };
    Ok(LoadedBundle {
        collection: AssetCollection::from_loaded_parts(serialized_files, resources),
        stats,
        io,
        source,
    })
}

fn require_budget(required: u64, budget: u64, purpose: &str) -> Result<()> {
    if required > budget {
        return Err(Error::invalid_data(format!(
            "{purpose}需要约 {required} 字节，超过软预算 {budget}"
        )));
    }
    Ok(())
}

fn bundle_limits(budget: u64) -> BundleParseLimits {
    BundleParseLimits {
        max_blocks_info_size: (budget / 8).min(16 * MIB),
        max_blocks: (budget / 256).min(1_000_000) as usize,
        max_entries: (budget / 1024).min(100_000) as usize,
        max_entry_read_size: budget,
        max_compressed_block_size: budget / 4,
        max_uncompressed_block_size: budget / 4,
        max_lzma_memory_kib: (budget / 4 / 1024).min(u64::from(u32::MAX)) as u32,
        max_zstd_window_log: 26,
        ..BundleParseLimits::default()
    }
}

fn serialized_limits(budget: u64) -> SerializedParseLimits {
    SerializedParseLimits {
        maximum_metadata_bytes: budget,
        maximum_types: (budget / 256).max(1) as usize,
        maximum_objects: (budget / size_of::<ObjectInfo>() as u64).max(1) as usize,
        maximum_script_types: (budget / 32).max(1) as usize,
        maximum_externals: (budget / 128).max(1) as usize,
        maximum_reference_types: (budget / 256).max(1) as usize,
        maximum_type_tree_nodes: (budget / size_of::<TypeTreeNode>() as u64).max(1) as usize,
        maximum_total_type_tree_nodes: (budget / size_of::<TypeTreeNode>() as u64).max(1) as usize,
        maximum_string_buffer_bytes: (budget / 2) as usize,
        maximum_total_string_buffer_bytes: (budget / 2) as usize,
        maximum_string_bytes: (budget / 8).min(MIB) as usize,
        maximum_materialized_metadata_string_bytes: (budget / 2) as usize,
        maximum_resolved_type_tree_string_bytes: budget as usize,
        maximum_type_dependencies: (budget / 4).max(1) as usize,
        ..SerializedParseLimits::default()
    }
}

fn metadata_heap_estimate(file: &SerializedFile) -> u64 {
    let trees = file
        .types
        .iter()
        .chain(&file.reference_types)
        .filter_map(|kind| kind.type_tree.as_ref());
    let mut bytes = file.objects.capacity() * size_of::<ObjectInfo>();
    bytes += file.types.capacity() * size_of::<unity_rs_core::serialized::SerializedType>();
    bytes +=
        file.reference_types.capacity() * size_of::<unity_rs_core::serialized::SerializedType>();
    bytes += file.externals.capacity() * size_of::<unity_rs_core::serialized::ExternalFile>();
    bytes +=
        file.script_types.capacity() * size_of::<unity_rs_core::serialized::ScriptIdentifier>();
    for tree in trees {
        bytes += tree.nodes.capacity() * size_of::<TypeTreeNode>() + tree.string_buffer.capacity();
        bytes += tree
            .nodes
            .iter()
            .map(|node| node.type_name.capacity() + node.field_name.capacity())
            .sum::<usize>();
    }
    bytes += file
        .externals
        .iter()
        .map(|external| external.path.capacity())
        .sum::<usize>();
    bytes as u64
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use super::{CountedSource, ReadStats, require_budget};
    use unity_rs_core::source::{Region, Source};

    #[test]
    fn counts_only_successful_source_reads() {
        let stats = Arc::new(ReadStats::default());
        let source = CountedSource {
            region: Region::from_bytes([1_u8, 2, 3].to_vec()),
            stats: Arc::clone(&stats),
        };
        let mut output = [0_u8; 2];
        source.read_exact_at(1, &mut output).unwrap();
        assert_eq!(output, [2, 3]);
        assert_eq!(
            source.read_exact_at(2, &mut output).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(stats.snapshot().source_read_calls, 1);
        assert_eq!(stats.snapshot().source_read_bytes, 2);
    }

    #[test]
    fn budget_rejects_before_resource_materialization() {
        assert!(require_budget(33, 32, "资源").is_err());
        assert!(require_budget(32, 32, "资源").is_ok());
    }
}
