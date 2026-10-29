use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use serde_json::{Value, json};
use unity_rs_core::loader::{AssetCollection, AssetCollectionParts, AssetLoadLimits};
use unity_rs_core::scene_hierarchy::SceneObjectKey;

use crate::export::{
    ExportResult, ExportedFile, ObjectExport, OutputContent, OutputDescriptor, OutputSink,
};
use crate::model_glb::write_glb;
use crate::model_source::{SourceModel, collect_model, model_roots};
use crate::{FileSink, MIB, ObjectRecord, hash_bytes, hash_file, loading, write_json_line};

pub struct ModelOptions {
    pub input: PathBuf,
    pub catalog: Option<PathBuf>,
    pub dependencies: Vec<PathBuf>,
    pub root_path_id: Option<i64>,
    pub root_file_index: usize,
    pub output: PathBuf,
    pub memory_budget: u64,
    pub output_budget: u64,
}

pub fn extract_model(options: ModelOptions) -> ExportResult<()> {
    let started = Instant::now();
    if options.output.exists() && fs::read_dir(&options.output)?.next().is_some() {
        return Err("模型导出要求新的或空的输出目录".into());
    }
    if !options.input.is_file()
        || options
            .input
            .extension()
            .is_none_or(|value| value != "assetbundle")
    {
        return Err("模型根输入必须是现存 .assetbundle 文件".into());
    }
    let dependencies = match &options.catalog {
        Some(catalog) => crate::catalog::collect_dependencies(
            &options.input,
            catalog,
            options.memory_budget / 3,
        )?,
        None => options.dependencies.clone(),
    };
    let mut files = vec![options.input.canonicalize()?];
    for dependency in dependencies {
        let dependency = dependency.canonicalize()?;
        if !files.contains(&dependency) {
            files.push(dependency);
        }
    }
    let mut parts = AssetCollectionParts::default();
    let mut retained_estimate = 0_u64;
    let mut sources = Vec::new();
    let mut root_files = 0;
    for (index, input) in files.iter().enumerate() {
        let available = options.memory_budget.saturating_sub(retained_estimate);
        let loaded = loading::load_bundle(input, available)?;
        let digest = crate::export::hash_region(&loaded.source)?;
        retained_estimate = retained_estimate
            .checked_add(loaded.stats.admission_estimate_bytes)
            .ok_or("模型依赖加载预算溢出")?;
        if retained_estimate + 16 * MIB > options.memory_budget {
            return Err("模型依赖加载后剩余工作区不足".into());
        }
        sources.push(json!({
            "input": input, "sha256": digest, "bytes": loaded.source.len(),
            "first_file_index": parts.serialized_files.len(),
            "serialized_files": loaded.collection.serialized_files().len(),
            "io": loaded.io.snapshot(), "loader": loaded.stats,
        }));
        let incoming = loaded.collection.into_parts();
        if index == 0 {
            root_files = incoming.serialized_files.len();
        }
        parts.serialized_files.extend(incoming.serialized_files);
        parts.resources.extend(incoming.resources);
        parts.diagnostics.extend(incoming.diagnostics);
    }
    if options.root_file_index >= root_files {
        return Err("root-file-index 必须属于首个根原包".into());
    }
    let object_count = parts
        .serialized_files
        .iter()
        .map(|file| file.file.objects.len() as u64)
        .sum::<u64>();
    let index_estimate = object_count
        .checked_mul(64)
        .and_then(|value| {
            value.checked_add(
                parts
                    .serialized_files
                    .iter()
                    .map(|file| file.path.len() as u64 * 4 + 192)
                    .chain(
                        parts
                            .resources
                            .iter()
                            .map(|file| file.path.len() as u64 * 4 + 192),
                    )
                    .sum::<u64>(),
            )
        })
        .ok_or("模型引用索引预算溢出")?;
    retained_estimate = retained_estimate
        .checked_add(index_estimate)
        .ok_or("模型工作区预算溢出")?;
    if retained_estimate + 16 * MIB > options.memory_budget {
        return Err("模型依赖索引超过软预算".into());
    }
    let mut collection = AssetCollection::from_parts(parts);
    let limits = AssetLoadLimits {
        maximum_reference_index_entries: usize::try_from(
            object_count + collection.serialized_files().len() as u64 * 2,
        )?,
        maximum_resource_index_entries: collection.resources().len().saturating_mul(2),
        ..AssetLoadLimits::default()
    };
    collection.rebuild_reference_index(limits)?;
    collection.rebuild_resource_index(limits)?;
    let root = select_root(
        &collection,
        options.root_file_index,
        options.root_path_id,
        options.memory_budget - retained_estimate,
    )?;
    let source = collect_model(&collection, root, options.memory_budget - retained_estimate)?;
    let source_budget = retained_estimate
        .checked_add(source.working_bytes)
        .ok_or("模型源数组预算溢出")?;
    let writer_budget = options.memory_budget.saturating_sub(source_budget);
    if writer_budget < 8 * MIB {
        return Err("模型源数组加载后 GLB 写出工作区不足".into());
    }
    let policy = hash_bytes(&serde_json::to_vec(&json!({
        "tool": env!("CARGO_PKG_VERSION"), "core": "0.6.1",
        "collector": hash_bytes(include_bytes!("model_source.rs")),
        "writer": hash_bytes(include_bytes!("model_glb.rs")),
        "root_file_index": root.file_index, "root_path_id": root.path_id.to_string(),
        "sources": sources.iter().map(|source| &source["sha256"]).collect::<Vec<_>>(),
    }))?);
    fs::create_dir_all(&options.output)?;
    let used = AtomicU64::new(0);
    let mut sink = FileSink {
        root: &options.output,
        prefix: PathBuf::new(),
        output_budget: options.output_budget,
        used: &used,
        published: Vec::new(),
    };
    let result = (|| {
        let glb_descriptor = descriptor("model.glb", "model_glb", &source.name);
        let (glb_file, glb_report) = sink.write_content(&glb_descriptor, |writer| {
            write_glb(&source, writer, options.output_budget, writer_budget)
        })?;
        let report = json!({
            "schema": 1, "kind": "source_model_data", "tool": env!("CARGO_PKG_VERSION"),
            "status": "exported_requires_independent_readback", "policy_key": policy,
            "scope": "源模型数据与可交换投影；没有宣称运行时装配、动画或Shader画面一致",
            "input": options.input.canonicalize()?,
            "catalog": match &options.catalog {
                Some(path) => Some(json!({"path": path, "sha256": hash_file(path)?})),
                None => None,
            },
            "sources": sources,
            "serialized_files": collection.serialized_files().iter().enumerate().map(|(index, file)| {
                json!({"index": index, "path": file.path, "unity_version": file.file.unity_version_string})
            }).collect::<Vec<_>>(),
            "source": source_metadata(&source), "glb": glb_report.manifest,
            "output": glb_file,
            "budget": {"soft": true, "loader_and_index_estimate_bytes": retained_estimate,
                "source_estimate_bytes": source.working_bytes,
                "glb_working_bytes": glb_report.working_bytes,
                "memory_budget_bytes": options.memory_budget},
        });
        let report_bytes = serde_json::to_vec_pretty(&report)?;
        if report_bytes.len() as u64 > writer_budget / 2 {
            return Err("模型源报告超过元数据软预算".into());
        }
        let report_descriptor = descriptor("model.source.json", "model_source", &source.name);
        let report_file = sink.write(&report_descriptor, OutputContent::Encoded(&report_bytes))?;
        let record = ObjectRecord {
            schema: 1,
            input: options.input.to_string_lossy().into_owned(),
            input_sha256: sources[0]["sha256"]
                .as_str()
                .ok_or("来源哈希缺失")?
                .to_owned(),
            entry: collection.serialized_files()[root.file_index].path.clone(),
            path_id: root.path_id.to_string(),
            class_id: 1,
            policy_key: policy.clone(),
            outcome: ObjectExport {
                kind: "model".into(),
                status: "data_exported".into(),
                name: source.name.clone(),
                metadata: json!({"validation": "requires_independent_readback", "nodes": source.nodes.len(),
                    "meshes": source.meshes.len(), "renderers": source.renderers.len(), "materials": source.materials.len()}),
                outputs: vec![
                    ExportedFile {
                        descriptor: glb_descriptor,
                        output: glb_file,
                    },
                    ExportedFile {
                        descriptor: report_descriptor,
                        output: report_file,
                    },
                ],
            },
        };
        let mut manifest = BufWriter::new(File::create(options.output.join("manifest.jsonl"))?);
        write_json_line(&mut manifest, &record)?;
        manifest.flush()?;
        println!(
            "{}",
            json!({"event": "model_exported", "nodes": source.nodes.len(),
            "meshes": source.meshes.len(), "renderers": source.renderers.len(),
            "materials": source.materials.len(), "output_bytes": used.load(std::sync::atomic::Ordering::Relaxed),
            "glb_bytes": glb_report.bytes, "binary_bytes": glb_report.binary_bytes,
            "policy_key": policy, "elapsed_seconds": started.elapsed().as_secs_f64(),
            "validation": "requires_independent_readback"})
        );
        Ok(())
    })();
    if result.is_err() {
        for (path, _) in &sink.published {
            fs::remove_file(path)?;
        }
        let manifest = options.output.join("manifest.jsonl");
        if manifest.is_file() {
            fs::remove_file(manifest)?;
        }
    }
    result
}

fn select_root(
    collection: &AssetCollection,
    file_index: usize,
    selected: Option<i64>,
    budget: u64,
) -> ExportResult<SceneObjectKey> {
    let file = &collection.serialized_files()[file_index].file;
    if let Some(path_id) = selected {
        if !file
            .objects
            .iter()
            .any(|object| object.path_id == path_id && object.class_id == 1)
        {
            return Err("指定 root-path-id 不是根文件中的 GameObject".into());
        }
        return Ok(SceneObjectKey {
            file_index,
            path_id,
        });
    }
    let roots = model_roots(collection, file_index + 1, budget)?
        .into_iter()
        .filter(|root| root.file_index == file_index)
        .collect::<Vec<_>>();
    if roots.len() != 1 {
        return Err(format!(
            "根原包有 {} 个独立 GameObject，请明确 --root-path-id",
            roots.len()
        )
        .into());
    }
    Ok(roots[0])
}

fn descriptor(path: &str, kind: &str, name: &str) -> OutputDescriptor {
    OutputDescriptor {
        relative_name: path.into(),
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

fn source_metadata(source: &SourceModel) -> Value {
    json!({
        "name": source.name, "root": source.root, "metadata": source.metadata,
        "nodes": source.nodes.iter().map(|node| json!({
            "key": {"file_index": node.key.file_index, "path_id": node.key.path_id.to_string()},
            "transform_key": {"file_index": node.transform_key.file_index, "path_id": node.transform_key.path_id.to_string()},
            "name": node.name, "parent": node.parent, "children": node.children,
            "translation": node.translation, "rotation": node.rotation, "scale": node.scale,
            "active": node.active,
            "metadata": node.metadata,
        })).collect::<Vec<_>>(),
        "meshes": source.meshes.iter().map(|mesh| json!({
            "key": {"file_index": mesh.key.file_index, "path_id": mesh.key.path_id.to_string()},
            "name": mesh.geometry.mesh.name, "vertices": mesh.geometry.mesh.vertices.len(),
            "metadata": mesh.metadata,
        })).collect::<Vec<_>>(),
        "renderers": source.renderers.iter().map(|renderer| json!({
            "key": {"file_index": renderer.key.file_index, "path_id": renderer.key.path_id.to_string()},
            "node": renderer.node, "mesh": renderer.mesh, "materials": renderer.materials,
            "bones": renderer.bones, "root_bone": renderer.root_bone,
            "blend_shape_weights": renderer.blend_shape_weights, "enabled": renderer.enabled,
            "metadata": renderer.metadata,
        })).collect::<Vec<_>>(),
        "materials": source.materials.iter().map(|material| json!({
            "key": {"file_index": material.key.file_index, "path_id": material.key.path_id.to_string()},
            "name": material.material.name, "metadata": material.metadata,
            "shader": {"file_id": material.material.shader.file_id,
                "path_id": material.material.shader.path_id.to_string()},
        })).collect::<Vec<_>>(),
    })
}
