use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::export::{ExportResult, OutputDescriptor};
use crate::pose_rules::{Frame, Node, Operation, Renderer, Scene, evaluate};
use crate::{FileSink, MIB, hash_bytes, hash_file, safe_relative};

pub struct PoseOptions {
    pub model: Option<PathBuf>,
    pub scene: Option<PathBuf>,
    pub state: PathBuf,
    pub output: PathBuf,
    pub memory_budget: u64,
    pub output_budget: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticInput {
    schema: u32,
    scene: Scene,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateInput {
    schema: u32,
    source_sha256: String,
    frames: Vec<NamedFrame>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedFrame {
    name: String,
    operations: Vec<Operation>,
}

#[derive(Deserialize)]
struct ModelInput {
    schema: u32,
    kind: String,
    source: ModelScene,
    glb: ModelGeometry,
    output: ModelOutput,
}

#[derive(Deserialize)]
struct ModelScene {
    nodes: Vec<ModelNode>,
    renderers: Vec<ModelRenderer>,
}

#[derive(Deserialize)]
struct ModelNode {
    key: Value,
    name: String,
    parent: Option<usize>,
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
    active: bool,
}

#[derive(Deserialize)]
struct ModelRenderer {
    key: Value,
    node: usize,
    mesh: usize,
    materials: Vec<Option<usize>>,
    blend_shape_weights: Vec<f32>,
    enabled: bool,
}

#[derive(Deserialize)]
struct ModelGeometry {
    meshes: Vec<ModelMesh>,
}

#[derive(Deserialize)]
struct ModelMesh {
    morph_channel_count: usize,
}

#[derive(Deserialize)]
struct ModelOutput {
    path: String,
    bytes: u64,
    sha256: String,
}

pub fn export_pose(options: PoseOptions) -> ExportResult<()> {
    let started = Instant::now();
    let input = options
        .model
        .as_ref()
        .or(options.scene.as_ref())
        .ok_or("姿态求值需要 --model 或 --scene")?;
    if options.output.exists() {
        return Err("姿态输出文件已存在，请使用新的路径".into());
    }
    if options.memory_budget < 16 * MIB {
        return Err("姿态数值求值软预算至少为16MiB".into());
    }
    let input_bytes = input.metadata()?.len();
    let state_bytes = options.state.metadata()?.len();
    let input_estimate = input_bytes
        .checked_add(state_bytes)
        .and_then(|value| value.checked_mul(12))
        .ok_or("姿态输入预算溢出")?;
    if input_estimate > options.memory_budget / 2 {
        return Err("姿态来源与控制输入超过解析软预算".into());
    }
    let input = fs::read(input)?;
    let source_sha256 = hash_bytes(&input);
    let state_raw = fs::read(&options.state)?;
    let state: StateInput = serde_json::from_slice(&state_raw)?;
    if state.schema != 1 || state.source_sha256 != source_sha256 {
        return Err("姿态控制schema或来源SHA-256与输入不符".into());
    }
    if state.frames.is_empty() || state.frames.len() > 256 {
        return Err("姿态控制需要1至256个独立frame".into());
    }
    let mut names = HashSet::new();
    if state.frames.iter().any(|frame| {
        frame.name.is_empty() || frame.name.len() > 256 || !names.insert(frame.name.as_str())
    }) {
        return Err("frame名称必须非空、唯一且不超过256字节".into());
    }
    let (scene, source) = match &options.model {
        Some(path) => {
            let model: ModelInput = serde_json::from_slice(&input)?;
            if model.schema != 1 || model.kind != "source_model_data" {
                return Err("姿态输入不是受支持的模型源数据说明".into());
            }
            if !safe_relative(&model.output.path) {
                return Err("模型输出引用超出来源目录".into());
            }
            let glb = path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(&model.output.path);
            if glb.metadata()?.len() != model.output.bytes
                || hash_file(&glb)? != model.output.sha256
            {
                return Err("模型GLB来源SHA-256或大小不符".into());
            }
            let renderers = model
                .source
                .renderers
                .iter()
                .map(|renderer| {
                    let count = model
                        .glb
                        .meshes
                        .get(renderer.mesh)
                        .ok_or("源Renderer引用的mesh索引越界")?
                        .morph_channel_count;
                    if count > 100_000 {
                        return Err("源形态通道超过100000".into());
                    }
                    if !renderer.blend_shape_weights.is_empty()
                        && renderer.blend_shape_weights.len() != count
                    {
                        return Err("源默认形态权重数量与通道不符".into());
                    }
                    Ok(Renderer {
                        node: renderer.node,
                        weights: if renderer.blend_shape_weights.is_empty() {
                            vec![0.0; count]
                        } else {
                            renderer.blend_shape_weights.clone()
                        },
                        material_count: renderer.materials.len(),
                    })
                })
                .collect::<ExportResult<Vec<_>>>()?;
            let source = json!({
                "kind": "model_source", "sha256": source_sha256,
                "glb_sha256": model.output.sha256,
                "nodes": model.source.nodes.iter().map(|node| json!({"key": node.key, "name": node.name})).collect::<Vec<_>>(),
                "renderers": model.source.renderers.iter().map(|renderer| json!({
                    "key": renderer.key, "source_default_blendshape_weights": renderer.blend_shape_weights,
                    "empty_defaults_for_computation": renderer.blend_shape_weights.is_empty(),
                    "source_enabled": renderer.enabled, "source_material_slots": renderer.materials,
                })).collect::<Vec<_>>(),
            });
            (
                Scene {
                    nodes: model
                        .source
                        .nodes
                        .into_iter()
                        .map(|node| Node {
                            parent: node.parent,
                            translation: node.translation,
                            rotation: node.rotation,
                            scale: node.scale,
                            active: node.active,
                        })
                        .collect(),
                    renderers,
                },
                source,
            )
        }
        None => {
            let scene: SyntheticInput = serde_json::from_slice(&input)?;
            if scene.schema != 1 {
                return Err("合成场景schema不受支持".into());
            }
            (
                scene.scene,
                json!({"kind": "synthetic_scene", "sha256": source_sha256}),
            )
        }
    };
    let frame_estimate = (scene.nodes.len() as u64)
        .checked_mul(768)
        .and_then(|value| value.checked_add(scene.renderers.len() as u64 * 1024))
        .and_then(|value| {
            value.checked_add(
                scene
                    .renderers
                    .iter()
                    .map(|renderer| renderer.weights.len() as u64 * 16)
                    .sum::<u64>(),
            )
        })
        .ok_or("姿态frame预算溢出")?;
    let estimated = state
        .frames
        .iter()
        .try_fold(input_estimate, |used, frame| {
            let adjustment_estimate = (scene.nodes.len() as u64)
                .checked_mul(frame.operations.len() as u64)
                .and_then(|value| value.checked_mul(192))
                .ok_or("四元数调整记录预算溢出")?;
            let controllers = frame
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::CryEyeInit { .. }))
                .count() as u64;
            let block_estimate = frame
                .operations
                .iter()
                .filter_map(|operation| match operation {
                    Operation::SetPropertyBlock { ints, .. } => {
                        Some(ints.keys().map(|key| key.len() as u64 + 96).sum::<u64>())
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            used.checked_add(frame_estimate)
                .and_then(|value| value.checked_add(frame.operations.len() as u64 * 2048))
                .and_then(|value| value.checked_add(adjustment_estimate))
                .and_then(|value| {
                    controllers
                        .checked_add(2)
                        .and_then(|count| count.checked_mul(block_estimate))
                        .and_then(|bytes| value.checked_add(bytes))
                })
                .ok_or("姿态快照预算溢出")
        })?;
    if estimated > options.memory_budget / 3 {
        return Err("姿态快照集合超过内存软预算，请拆分frame".into());
    }
    let mut frames = Vec::with_capacity(state.frames.len());
    for frame in state.frames {
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: frame.operations,
            },
        )?;
        frames.push(json!({"name": frame.name, "snapshot": snapshot}));
    }
    let report = json!({
        "schema": 1, "kind": "controlled_pose_states", "tool": env!("CARGO_PKG_VERSION"),
        "source": source, "state_sha256": hash_bytes(&state_raw),
        "coordinate_frame": "unity_source_local", "rules_version": 1,
        "rules_implementation_sha256": hash_bytes(include_bytes!("pose_rules.rs")),
        "frames_independent": true, "frames": frames,
        "limits": {
            "scope": "受控数学输入与已核验组件规则；不代表原版进程实际调用或GPU画面",
            "morph_interpolation": "只保存源通道权重；不猜多帧形态或动画插值",
            "quaternion": "保留局部原值；计算归一化调整由快照显式记录",
            "property_block": "保留renderer与材质槽作用域；不宣称最终GPU有效值",
            "memory": "JSON与快照集合软估算，不是操作系统RSS硬限制",
        },
    });
    let root = options
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let filename = options
        .output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("姿态输出文件名不是有效UTF-8")?;
    if options.output.with_extension("part").exists() {
        return Err("姿态临时输出已存在，请先检查或使用新路径".into());
    }
    fs::create_dir_all(root)?;
    let used = AtomicU64::new(0);
    let mut sink = FileSink {
        root,
        prefix: PathBuf::new(),
        output_budget: options.output_budget,
        used: &used,
        published: Vec::new(),
    };
    let descriptor = OutputDescriptor {
        relative_name: filename.to_owned(),
        kind: "controlled_pose".into(),
        name: String::new(),
        face: None,
        mip: None,
        width: None,
        height: None,
        rgba_sha256: None,
        source_payload_sha256: None,
        encoding: None,
    };
    let output = sink
        .write_content(&descriptor, |writer| {
            serde_json::to_writer_pretty(writer, &report)?;
            Ok(())
        })?
        .0;
    println!(
        "{}",
        json!({"event": "pose_evaluated", "frames": report["frames"].as_array().map_or(0, Vec::len),
            "nodes": scene.nodes.len(), "renderers": scene.renderers.len(), "output_bytes": output.bytes,
            "sha256": output.sha256, "source_sha256": source_sha256,
            "elapsed_seconds": started.elapsed().as_secs_f64(), "validation": "controlled_numeric_only"})
    );
    Ok(())
}
