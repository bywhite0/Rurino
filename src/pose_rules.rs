use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::export::ExportResult;

pub type PropertyBlock = BTreeMap<String, i32>;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub nodes: Vec<Node>,
    pub renderers: Vec<Renderer>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub active: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Renderer {
    pub node: usize,
    pub weights: Vec<f32>,
    pub material_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BonePair {
    pub source: usize,
    pub target: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    SetLocalTrs {
        node: usize,
        #[serde(default)]
        translation: Option<[f32; 3]>,
        #[serde(default)]
        rotation: Option<[f32; 4]>,
        #[serde(default)]
        scale: Option<[f32; 3]>,
    },
    RootMotion {
        node: usize,
        position: [f32; 3],
        rotation: [f32; 4],
    },
    FaceSync {
        upper_chest: BonePair,
        local_rotations: Vec<BonePair>,
    },
    SetBlendShape {
        renderer: usize,
        index: usize,
        weight: f32,
    },
    SetActive {
        node: usize,
        active: bool,
    },
    SetPropertyBlock {
        renderer: usize,
        #[serde(default)]
        material_index: Option<usize>,
        ints: PropertyBlock,
    },
    CryEyeInit {
        controller: String,
        renderer: usize,
        material_index: usize,
        distortion_node: usize,
        property: String,
    },
    CryEye {
        controller: String,
        is_cry: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub nodes: Vec<NodeSnapshot>,
    pub renderers: Vec<RendererSnapshot>,
    pub cry_controllers: BTreeMap<String, CryController>,
    pub traces: Vec<Trace>,
    pub quaternion_adjustments: Vec<QuaternionAdjustment>,
    pub contract: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeSnapshot {
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub active: bool,
    pub active_in_hierarchy: bool,
    pub world_matrix: [f32; 16],
    pub world_rotation: [f32; 4],
}

#[derive(Debug, Clone, Serialize)]
pub struct RendererSnapshot {
    pub node: usize,
    pub default_weights: Vec<f32>,
    pub current_weights: Vec<f32>,
    pub material_count: usize,
    pub property_block: Option<PropertyBlock>,
    pub material_property_blocks: BTreeMap<usize, PropertyBlock>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CryController {
    pub renderer: usize,
    pub material_index: usize,
    pub distortion_node: usize,
    pub property: String,
    pub cached_block: PropertyBlock,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    pub index: usize,
    pub operation: &'static str,
    pub changed_nodes: Vec<usize>,
    pub changed_renderers: Vec<usize>,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuaternionAdjustment {
    pub node: usize,
    pub stage: String,
    pub original: [f32; 4],
    pub normalized: [f32; 4],
    pub original_norm: f32,
}

struct World {
    matrix: [f32; 16],
    rotation: [f32; 4],
    active: bool,
}

pub fn evaluate(scene: &Scene, frame: &Frame) -> ExportResult<Snapshot> {
    validate_scene(scene)?;
    if frame.operations.len() > 100_000 {
        return Err("受控姿态操作超过100000项".into());
    }
    let mut nodes = scene.nodes.clone();
    let mut renderers = scene
        .renderers
        .iter()
        .map(|renderer| RendererSnapshot {
            node: renderer.node,
            default_weights: renderer.weights.clone(),
            current_weights: renderer.weights.clone(),
            material_count: renderer.material_count,
            property_block: None,
            material_property_blocks: BTreeMap::new(),
        })
        .collect::<Vec<_>>();
    let mut cry_controllers = BTreeMap::new();
    let mut adjustments = Vec::new();
    let mut worlds = compute_worlds(&nodes, &mut adjustments, "source")?;
    let mut traces = Vec::with_capacity(frame.operations.len());
    for (index, operation) in frame.operations.iter().enumerate() {
        let trace = match operation {
            Operation::SetLocalTrs {
                node,
                translation,
                rotation,
                scale,
            } => {
                let target = nodes.get_mut(*node).ok_or("局部TRS节点索引越界")?;
                if translation.is_none() && rotation.is_none() && scale.is_none() {
                    return Err("局部TRS操作没有赋值字段".into());
                }
                if let Some(value) = translation {
                    require_finite(value, "局部位置")?;
                    target.translation = *value;
                }
                if let Some(value) = rotation {
                    normalized_quaternion(*value, *node, "local_assignment", &mut adjustments)?;
                    target.rotation = *value;
                }
                if let Some(value) = scale {
                    require_finite(value, "局部缩放")?;
                    target.scale = *value;
                }
                Trace {
                    index,
                    operation: "set_local_trs",
                    changed_nodes: vec![*node],
                    changed_renderers: Vec::new(),
                    detail: json!({"assignment": "source_local", "input_preserved": true,
                        "translation": translation, "rotation": rotation, "scale": scale}),
                }
            }
            Operation::RootMotion {
                node,
                position,
                rotation,
            } => {
                require_finite(position, "根运动世界位置")?;
                let normalized =
                    normalized_quaternion(*rotation, *node, "root_motion_input", &mut adjustments)?;
                set_world_transform(&mut nodes, &worlds, *node, *position, normalized)?;
                Trace {
                    index,
                    operation: "root_motion",
                    changed_nodes: vec![*node],
                    changed_renderers: Vec::new(),
                    detail: json!({"assignment": "absolute_world_position_rotation", "accumulate": false,
                        "position": position, "requested_rotation": rotation, "calculation_rotation": normalized}),
                }
            }
            Operation::FaceSync {
                upper_chest,
                local_rotations,
            } => {
                let source = worlds.get(upper_chest.source).ok_or("脸部上胸源节点越界")?;
                let position = [source.matrix[3], source.matrix[7], source.matrix[11]];
                set_world_transform(
                    &mut nodes,
                    &worlds,
                    upper_chest.target,
                    position,
                    source.rotation,
                )?;
                let mut changed = vec![upper_chest.target];
                for pair in local_rotations {
                    let rotation = nodes
                        .get(pair.source)
                        .ok_or("脸部局部旋转源节点越界")?
                        .rotation;
                    nodes
                        .get_mut(pair.target)
                        .ok_or("脸部局部旋转目标节点越界")?
                        .rotation = rotation;
                    changed.push(pair.target);
                }
                Trace {
                    index,
                    operation: "face_sync",
                    changed_nodes: changed,
                    changed_renderers: Vec::new(),
                    detail: json!({"upper_chest_mode": "world_position_world_rotation", "other_pairs": "local_rotation",
                        "order": "upper_chest_then_declared_pairs", "binding": "explicit_node_indices",
                        "upper_chest": upper_chest, "local_rotations": local_rotations}),
                }
            }
            Operation::SetBlendShape {
                renderer,
                index: weight_index,
                weight,
            } => {
                require_finite(&[*weight], "形态权重")?;
                let target = renderers.get_mut(*renderer).ok_or("形态renderer索引越界")?;
                *target
                    .current_weights
                    .get_mut(*weight_index)
                    .ok_or("形态权重索引越界")? = *weight;
                Trace {
                    index,
                    operation: "set_blend_shape",
                    changed_nodes: Vec::new(),
                    changed_renderers: vec![*renderer],
                    detail: json!({"index": weight_index, "weight": weight, "clamped": false,
                        "morph_evaluation": "data_only_no_frame_interpolation"}),
                }
            }
            Operation::SetActive { node, active } => {
                nodes.get_mut(*node).ok_or("显隐节点索引越界")?.active = *active;
                Trace {
                    index,
                    operation: "set_active",
                    changed_nodes: vec![*node],
                    changed_renderers: Vec::new(),
                    detail: json!({"active": active}),
                }
            }
            Operation::SetPropertyBlock {
                renderer,
                material_index,
                ints,
            } => {
                validate_block(ints)?;
                let target = renderers
                    .get_mut(*renderer)
                    .ok_or("属性块renderer索引越界")?;
                if let Some(material) = material_index {
                    if *material >= target.material_count {
                        return Err("属性块材质槽索引越界".into());
                    }
                    target
                        .material_property_blocks
                        .insert(*material, ints.clone());
                } else {
                    target.property_block = Some(ints.clone());
                }
                Trace {
                    index,
                    operation: "set_property_block",
                    changed_nodes: Vec::new(),
                    changed_renderers: vec![*renderer],
                    detail: json!({"scope": if material_index.is_some() {"material_slot"} else {"renderer"},
                        "material_index": material_index, "ints": ints, "replace": true, "priority": "not_gpu_verified"}),
                }
            }
            Operation::CryEyeInit {
                controller,
                renderer,
                material_index,
                distortion_node,
                property,
            } => {
                require_name(controller, "哭眼控制器键")?;
                require_name(property, "哭眼属性名")?;
                if cry_controllers.contains_key(controller) {
                    return Err("哭眼控制器重复初始化".into());
                }
                let target = renderers.get(*renderer).ok_or("哭眼renderer索引越界")?;
                if *material_index >= target.material_count {
                    return Err("哭眼读取材质槽越界".into());
                }
                let cached_block = target
                    .material_property_blocks
                    .get(material_index)
                    .cloned()
                    .unwrap_or_default();
                nodes
                    .get_mut(*distortion_node)
                    .ok_or("哭眼变形对象索引越界")?
                    .active = false;
                cry_controllers.insert(
                    controller.clone(),
                    CryController {
                        renderer: *renderer,
                        material_index: *material_index,
                        distortion_node: *distortion_node,
                        property: property.clone(),
                        cached_block,
                    },
                );
                Trace {
                    index,
                    operation: "cry_eye_init",
                    changed_nodes: vec![*distortion_node],
                    changed_renderers: Vec::new(),
                    detail: json!({"controller": controller, "get_scope": "material_slot", "material_index": material_index,
                        "missing_slot_block": "empty", "renderer_block_fallback": false,
                        "distortion_active": false}),
                }
            }
            Operation::CryEye { controller, is_cry } => {
                let target = cry_controllers
                    .get_mut(controller)
                    .ok_or("哭眼控制器未初始化")?;
                target
                    .cached_block
                    .insert(target.property.clone(), i32::from(*is_cry));
                renderers[target.renderer].property_block = Some(target.cached_block.clone());
                nodes[target.distortion_node].active = *is_cry;
                Trace {
                    index,
                    operation: "cry_eye",
                    changed_nodes: vec![target.distortion_node],
                    changed_renderers: vec![target.renderer],
                    detail: json!({"controller": controller, "property": target.property, "value": i32::from(*is_cry),
                        "set_scope": "renderer", "slot_blocks_preserved": true, "distortion_active": is_cry,
                        "priority": "not_gpu_verified"}),
                }
            }
        };
        worlds = compute_worlds(&nodes, &mut adjustments, trace.operation)?;
        traces.push(trace);
    }
    Ok(Snapshot {
        nodes: nodes
            .into_iter()
            .zip(worlds)
            .map(|(node, world)| NodeSnapshot {
                parent: node.parent,
                translation: node.translation,
                rotation: node.rotation,
                scale: node.scale,
                active: node.active,
                active_in_hierarchy: world.active,
                world_matrix: world.matrix,
                world_rotation: world.rotation,
            })
            .collect(),
        renderers,
        cry_controllers,
        traces,
        quaternion_adjustments: adjustments,
        contract: json!({
            "arithmetic": "float32_controlled_math_replay",
            "matrix_order": "row_major_translation_at_3_7_11",
            "quaternion_order": "xyzw",
            "quaternion_policy": "保留原局部输入；计算使用归一化四元数，位变化逐项记录",
            "world_position": "full_parent_affine_matrix",
            "world_rotation": "quaternion_hierarchy_independent_of_scale_shear",
            "morph": "numeric_parameters_only_no_frame_interpolation_or_deformation",
            "property_blocks": "renderer_and_material_slot_states_preserved_priority_not_gpu_verified",
            "comparison_tolerance": {"absolute": 0.00001, "relative": 0.00002},
            "unity_transform_bit_exact": false,
        }),
    })
}

fn validate_scene(scene: &Scene) -> ExportResult<()> {
    if scene.nodes.is_empty() {
        return Err("受控姿态场景没有节点".into());
    }
    for (index, node) in scene.nodes.iter().enumerate() {
        if node.parent.is_some_and(|parent| parent >= index) {
            return Err("受控姿态节点必须按父节点在前排序，禁止环或越界父引用".into());
        }
        require_finite(&node.translation, "源位置")?;
        require_finite(&node.rotation, "源旋转")?;
        require_finite(&node.scale, "源缩放")?;
    }
    for renderer in &scene.renderers {
        if renderer.node >= scene.nodes.len() {
            return Err("源renderer所属节点越界".into());
        }
        require_finite(&renderer.weights, "源形态权重")?;
    }
    Ok(())
}

fn compute_worlds(
    nodes: &[Node],
    adjustments: &mut Vec<QuaternionAdjustment>,
    stage: &str,
) -> ExportResult<Vec<World>> {
    let mut worlds: Vec<World> = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let rotation = normalized_quaternion(node.rotation, index, stage, adjustments)?;
        let local = trs_matrix(node.translation, rotation, node.scale);
        let (matrix, world_rotation, active) = if let Some(parent) = node.parent {
            let parent = &worlds[parent];
            (
                multiply_matrix(parent.matrix, local),
                normalized_quaternion(
                    multiply_quaternion(parent.rotation, rotation),
                    index,
                    &format!("{stage}:world_product"),
                    adjustments,
                )?,
                parent.active && node.active,
            )
        } else {
            (local, rotation, node.active)
        };
        require_finite(&matrix, "世界矩阵")?;
        worlds.push(World {
            matrix,
            rotation: world_rotation,
            active,
        });
    }
    Ok(worlds)
}

fn set_world_transform(
    nodes: &mut [Node],
    worlds: &[World],
    node: usize,
    position: [f32; 3],
    rotation: [f32; 4],
) -> ExportResult<()> {
    let target = nodes.get_mut(node).ok_or("世界变换目标节点越界")?;
    if let Some(parent) = target.parent {
        target.translation = transform_point(inverse_affine(worlds[parent].matrix)?, position);
        target.rotation = multiply_quaternion(conjugate(worlds[parent].rotation), rotation);
    } else {
        target.translation = position;
        target.rotation = rotation;
    }
    require_finite(&target.translation, "反变换局部位置")?;
    require_finite(&target.rotation, "反变换局部旋转")?;
    Ok(())
}

fn normalized_quaternion(
    value: [f32; 4],
    node: usize,
    stage: &str,
    adjustments: &mut Vec<QuaternionAdjustment>,
) -> ExportResult<[f32; 4]> {
    require_finite(&value, "四元数")?;
    let norm_squared = value
        .iter()
        .fold(0.0_f32, |sum, component| sum + component * component);
    if !norm_squared.is_finite() || norm_squared <= 1e-20 {
        return Err("四元数退化或其float32范数不可表示".into());
    }
    let norm = norm_squared.sqrt();
    let normalized = value.map(|component| component / norm);
    if value.map(f32::to_bits) != normalized.map(f32::to_bits) {
        adjustments.push(QuaternionAdjustment {
            node,
            stage: stage.to_owned(),
            original: value,
            normalized,
            original_norm: norm,
        });
    }
    Ok(normalized)
}

fn require_finite(values: &[f32], label: &str) -> ExportResult<()> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(format!("{label}包含非有限浮点值").into());
    }
    Ok(())
}

fn require_name(value: &str, label: &str) -> ExportResult<()> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') {
        return Err(format!("{label}为空或超过已支持范围").into());
    }
    Ok(())
}

fn validate_block(values: &PropertyBlock) -> ExportResult<()> {
    if values.len() > 4096 {
        return Err("受控整数属性块超过4096项".into());
    }
    values
        .keys()
        .try_for_each(|key| require_name(key, "属性块键"))
}

fn conjugate(value: [f32; 4]) -> [f32; 4] {
    [-value[0], -value[1], -value[2], value[3]]
}

fn multiply_quaternion(left: [f32; 4], right: [f32; 4]) -> [f32; 4] {
    [
        left[3] * right[0] + left[0] * right[3] + left[1] * right[2] - left[2] * right[1],
        left[3] * right[1] - left[0] * right[2] + left[1] * right[3] + left[2] * right[0],
        left[3] * right[2] + left[0] * right[1] - left[1] * right[0] + left[2] * right[3],
        left[3] * right[3] - left[0] * right[0] - left[1] * right[1] - left[2] * right[2],
    ]
}

fn trs_matrix(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> [f32; 16] {
    let [x, y, z, w] = rotation;
    [
        (1.0 - 2.0 * (y * y + z * z)) * scale[0],
        2.0 * (x * y - z * w) * scale[1],
        2.0 * (x * z + y * w) * scale[2],
        translation[0],
        2.0 * (x * y + z * w) * scale[0],
        (1.0 - 2.0 * (x * x + z * z)) * scale[1],
        2.0 * (y * z - x * w) * scale[2],
        translation[1],
        2.0 * (x * z - y * w) * scale[0],
        2.0 * (y * z + x * w) * scale[1],
        (1.0 - 2.0 * (x * x + y * y)) * scale[2],
        translation[2],
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

fn multiply_matrix(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    let mut result = [0.0_f32; 16];
    for row in 0..4 {
        for column in 0..4 {
            result[row * 4 + column] = (0..4).fold(0.0, |sum, inner| {
                sum + left[row * 4 + inner] * right[inner * 4 + column]
            });
        }
    }
    result
}

fn transform_point(matrix: [f32; 16], point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0] * point[0] + matrix[1] * point[1] + matrix[2] * point[2] + matrix[3],
        matrix[4] * point[0] + matrix[5] * point[1] + matrix[6] * point[2] + matrix[7],
        matrix[8] * point[0] + matrix[9] * point[1] + matrix[10] * point[2] + matrix[11],
    ]
}

fn inverse_affine(matrix: [f32; 16]) -> ExportResult<[f32; 16]> {
    let mut augmented = [[0.0_f32; 8]; 4];
    for (row, values) in augmented.iter_mut().enumerate() {
        values[..4].copy_from_slice(&matrix[row * 4..row * 4 + 4]);
        values[row + 4] = 1.0;
    }
    for column in 0..4 {
        let pivot = (column..4)
            .max_by(|left, right| {
                augmented[*left][column]
                    .abs()
                    .total_cmp(&augmented[*right][column].abs())
            })
            .ok_or("父矩阵求逆没有主元")?;
        if augmented[pivot][column] == 0.0 || !augmented[pivot][column].is_finite() {
            return Err("父节点世界矩阵奇异，无法设置世界变换".into());
        }
        augmented.swap(column, pivot);
        let divisor = augmented[column][column];
        augmented[column]
            .iter_mut()
            .for_each(|value| *value /= divisor);
        let source = augmented[column];
        for (row_index, row) in augmented.iter_mut().enumerate() {
            if row_index == column {
                continue;
            }
            let factor = row[column];
            for index in 0..8 {
                row[index] -= factor * source[index];
            }
        }
    }
    let result = std::array::from_fn(|index| augmented[index / 4][4 + index % 4]);
    require_finite(&result, "父矩阵逆")?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{BonePair, Frame, Node, Operation, Renderer, Scene, evaluate};

    fn node(parent: Option<usize>, translation: [f32; 3]) -> Node {
        Node {
            parent,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            active: true,
        }
    }

    fn near(left: &[f32], right: &[f32]) {
        assert_eq!(left.len(), right.len());
        for (left, right) in left.iter().zip(right) {
            assert!((left - right).abs() < 0.00002, "{left} != {right}");
        }
    }

    #[test]
    fn computes_parent_affine_position_and_separate_world_rotation() {
        let mut parent = node(None, [3.0, 4.0, 5.0]);
        parent.rotation = [0.0, 0.0, 0.5_f32.sqrt(), 0.5_f32.sqrt()];
        parent.scale = [2.0, 3.0, 4.0];
        let scene = Scene {
            nodes: vec![parent, node(Some(0), [1.0, 2.0, 3.0])],
            renderers: Vec::new(),
        };
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: Vec::new(),
            },
        )
        .unwrap();
        let matrix = snapshot.nodes[1].world_matrix;
        near(&[matrix[3], matrix[7], matrix[11]], &[-3.0, 6.0, 17.0]);
        near(
            &snapshot.nodes[1].world_rotation,
            &snapshot.nodes[0].world_rotation,
        );
        assert_eq!(snapshot.nodes[0].rotation, scene.nodes[0].rotation);
    }

    #[test]
    fn root_motion_is_absolute_under_a_rotated_nonuniform_parent() {
        let mut parent = node(None, [5.0, 0.0, 0.0]);
        parent.rotation = [0.0, 0.0, 0.5_f32.sqrt(), 0.5_f32.sqrt()];
        parent.scale = [2.0, 3.0, 4.0];
        let scene = Scene {
            nodes: vec![parent, node(Some(0), [0.0; 3])],
            renderers: Vec::new(),
        };
        let operation = Operation::RootMotion {
            node: 1,
            position: [8.0, 6.0, 4.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: vec![operation.clone(), operation],
            },
        )
        .unwrap();
        let matrix = snapshot.nodes[1].world_matrix;
        near(&[matrix[3], matrix[7], matrix[11]], &[8.0, 6.0, 4.0]);
        near(&snapshot.nodes[1].translation, &[3.0, -1.0, 1.0]);
        near(&snapshot.nodes[1].world_rotation, &[0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn face_sync_distinguishes_world_chest_and_sequential_local_rotations() {
        let mut source = node(None, [2.0, 3.0, 4.0]);
        source.rotation = [0.0, 0.0, 0.5_f32.sqrt(), 0.5_f32.sqrt()];
        let mut target_parent = node(None, [10.0, 0.0, 0.0]);
        target_parent.scale = [2.0, 3.0, 4.0];
        let scene = Scene {
            nodes: vec![
                source,
                target_parent,
                node(Some(1), [0.0; 3]),
                node(Some(2), [0.0; 3]),
            ],
            renderers: Vec::new(),
        };
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: vec![Operation::FaceSync {
                    upper_chest: BonePair {
                        source: 0,
                        target: 2,
                    },
                    local_rotations: vec![BonePair {
                        source: 2,
                        target: 3,
                    }],
                }],
            },
        )
        .unwrap();
        let matrix = snapshot.nodes[2].world_matrix;
        near(&[matrix[3], matrix[7], matrix[11]], &[2.0, 3.0, 4.0]);
        near(&snapshot.nodes[3].rotation, &snapshot.nodes[2].rotation);
        near(&snapshot.nodes[3].world_rotation, &[0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn keeps_unclamped_weights_and_source_defaults() {
        let scene = Scene {
            nodes: vec![node(None, [0.0; 3])],
            renderers: vec![Renderer {
                node: 0,
                weights: vec![20.0, 30.0],
                material_count: 1,
            }],
        };
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: vec![
                    Operation::SetBlendShape {
                        renderer: 0,
                        index: 0,
                        weight: -25.0,
                    },
                    Operation::SetBlendShape {
                        renderer: 0,
                        index: 1,
                        weight: 150.0,
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(snapshot.renderers[0].default_weights, vec![20.0, 30.0]);
        assert_eq!(snapshot.renderers[0].current_weights, vec![-25.0, 150.0]);
    }

    #[test]
    fn cry_reads_indexed_cache_and_writes_renderer_scope_without_removing_slots() {
        let scene = Scene {
            nodes: vec![node(None, [0.0; 3]), node(Some(0), [0.0; 3])],
            renderers: vec![Renderer {
                node: 0,
                weights: Vec::new(),
                material_count: 2,
            }],
        };
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: vec![
                    Operation::SetPropertyBlock {
                        renderer: 0,
                        material_index: None,
                        ints: [("renderer_seed".into(), 7)].into(),
                    },
                    Operation::SetPropertyBlock {
                        renderer: 0,
                        material_index: Some(1),
                        ints: [("slot_seed".into(), 9)].into(),
                    },
                    Operation::CryEyeInit {
                        controller: "eye".into(),
                        renderer: 0,
                        material_index: 1,
                        distortion_node: 1,
                        property: "_UseCryParameters".into(),
                    },
                    Operation::SetPropertyBlock {
                        renderer: 0,
                        material_index: Some(1),
                        ints: [("later_slot".into(), 11)].into(),
                    },
                    Operation::CryEye {
                        controller: "eye".into(),
                        is_cry: true,
                    },
                ],
            },
        )
        .unwrap();
        let renderer = &snapshot.renderers[0];
        assert_eq!(
            renderer.property_block.as_ref().unwrap().get("slot_seed"),
            Some(&9)
        );
        assert_eq!(
            renderer
                .property_block
                .as_ref()
                .unwrap()
                .get("_UseCryParameters"),
            Some(&1)
        );
        assert!(
            !renderer
                .property_block
                .as_ref()
                .unwrap()
                .contains_key("renderer_seed")
        );
        assert_eq!(
            renderer.material_property_blocks[&1].get("later_slot"),
            Some(&11)
        );
        assert!(snapshot.nodes[1].active);
        assert_eq!(
            snapshot.traces.last().unwrap().detail["priority"],
            "not_gpu_verified"
        );
    }

    #[test]
    fn absent_indexed_block_does_not_inherit_renderer_block() {
        let scene = Scene {
            nodes: vec![node(None, [0.0; 3])],
            renderers: vec![Renderer {
                node: 0,
                weights: Vec::new(),
                material_count: 1,
            }],
        };
        let snapshot = evaluate(
            &scene,
            &Frame {
                operations: vec![
                    Operation::SetPropertyBlock {
                        renderer: 0,
                        material_index: None,
                        ints: [("renderer_seed".into(), 7)].into(),
                    },
                    Operation::CryEyeInit {
                        controller: "eye".into(),
                        renderer: 0,
                        material_index: 0,
                        distortion_node: 0,
                        property: "cry".into(),
                    },
                ],
            },
        )
        .unwrap();
        assert!(snapshot.cry_controllers["eye"].cached_block.is_empty());
        assert!(!snapshot.nodes[0].active);
    }

    #[test]
    fn reports_quaternion_normalization_without_mutating_local_input() {
        let mut source = node(None, [0.0; 3]);
        source.rotation = [0.0, 0.0, 0.0, 2.0];
        let snapshot = evaluate(
            &Scene {
                nodes: vec![source],
                renderers: Vec::new(),
            },
            &Frame {
                operations: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(snapshot.nodes[0].rotation, [0.0, 0.0, 0.0, 2.0]);
        assert_eq!(snapshot.nodes[0].world_rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(snapshot.quaternion_adjustments[0].original_norm, 2.0);
    }

    #[test]
    fn refuses_singular_parent_degenerate_quaternion_and_invalid_indices() {
        let mut parent = node(None, [0.0; 3]);
        parent.scale[0] = 0.0;
        let scene = Scene {
            nodes: vec![parent, node(Some(0), [0.0; 3])],
            renderers: Vec::new(),
        };
        assert!(
            evaluate(
                &scene,
                &Frame {
                    operations: vec![Operation::RootMotion {
                        node: 1,
                        position: [0.0; 3],
                        rotation: [0.0, 0.0, 0.0, 1.0]
                    }]
                }
            )
            .is_err()
        );
        let mut invalid = node(None, [0.0; 3]);
        invalid.rotation = [0.0; 4];
        assert!(
            evaluate(
                &Scene {
                    nodes: vec![invalid],
                    renderers: Vec::new()
                },
                &Frame {
                    operations: Vec::new()
                }
            )
            .is_err()
        );
        assert!(
            evaluate(
                &scene,
                &Frame {
                    operations: vec![Operation::SetActive {
                        node: 3,
                        active: true
                    }]
                }
            )
            .is_err()
        );
    }
}
