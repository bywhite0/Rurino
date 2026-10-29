use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use unity_rs_core::mesh::{MeshBlendShapeVertex, MeshBoneWeight};

use crate::export::ExportResult;
use crate::model_source::SourceModel;

pub struct GlbReport {
    pub bytes: u64,
    pub binary_bytes: u64,
    pub working_bytes: u64,
    pub manifest: Value,
}

#[derive(Clone, Copy)]
enum Flip {
    None,
    Vector,
    Tangent,
}

enum ArrayData<'a> {
    Vectors(&'a [[f32; 3]]),
    Floats(&'a [f32], usize, Flip),
    Indices(&'a [u32]),
    Joints(&'a [MeshBoneWeight]),
    Weights(&'a [MeshBoneWeight]),
    Matrices(&'a [[f32; 16]]),
    MorphIndices(&'a [MeshBlendShapeVertex], Arc<Vec<usize>>),
    MorphValues(&'a [MeshBlendShapeVertex], Arc<Vec<usize>>, usize),
    MorphRecords(&'a [MeshBlendShapeVertex]),
}

struct ArrayPlan<'a> {
    offset: u64,
    bytes: u64,
    data: ArrayData<'a>,
}

struct Planner<'a> {
    arrays: Vec<ArrayPlan<'a>>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    reports: Vec<Value>,
    binary_bytes: u64,
    working_bytes: u64,
    budget: u64,
}

struct GeometryPlan {
    attributes: BTreeMap<String, usize>,
    indices: Vec<usize>,
    targets: Vec<Value>,
    channels: Vec<Value>,
    bind_poses: Option<usize>,
    report: Value,
}

pub fn write_glb(
    source: &SourceModel,
    writer: &mut impl Write,
    maximum_output_bytes: u64,
    working_budget_bytes: u64,
) -> ExportResult<GlbReport> {
    if source.renderers.is_empty() {
        return Err("所选模型没有可导出的Renderer".into());
    }
    let mut plan = Planner {
        arrays: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
        reports: Vec::new(),
        binary_bytes: 0,
        working_bytes: 0,
        budget: working_budget_bytes / 2,
    };
    plan.charge(
        (source.nodes.len() + source.renderers.len() + source.materials.len()) as u64 * 2048,
    )?;
    plan.charge(
        source
            .nodes
            .iter()
            .map(|node| node.name.len() as u64 * 4)
            .sum::<u64>()
            + source
                .materials
                .iter()
                .map(|material| material.material.name.len() as u64 * 4)
                .sum::<u64>(),
    )?;
    let geometries = source
        .meshes
        .iter()
        .enumerate()
        .map(|(index, mesh)| plan_geometry(&mut plan, index, &mesh.geometry))
        .collect::<ExportResult<Vec<_>>>()?;
    let mut nodes = source
        .nodes
        .iter()
        .map(|node| {
            json!({
                "name": node.name,
                "translation": [-node.translation[0], node.translation[1], node.translation[2]],
                "rotation": [node.rotation[0], -node.rotation[1], -node.rotation[2], node.rotation[3]],
                "scale": node.scale,
                "children": node.children,
                "extras": {"unity_active": node.active,
                    "source_game_object": key_json(node.key.file_index, node.key.path_id),
                    "source_transform": key_json(node.transform_key.file_index, node.transform_key.path_id)}
            })
        })
        .collect::<Vec<_>>();
    let mut meshes = Vec::new();
    let mut skins = Vec::new();
    let mut renderer_reports = Vec::new();
    let mut requires_custom_morph_interpolation = false;
    for (index, renderer) in source.renderers.iter().enumerate() {
        let geometry = &geometries[renderer.mesh];
        let mesh = &source.meshes[renderer.mesh].geometry.mesh;
        let primitives = geometry
            .indices
            .iter()
            .enumerate()
            .map(|(slot, accessor)| {
                let mut primitive =
                    json!({"attributes": geometry.attributes, "indices": accessor, "mode": 4});
                if let Some(material) = renderer.materials.get(slot).copied().flatten() {
                    primitive["material"] = json!(material);
                }
                if !geometry.targets.is_empty() {
                    primitive["targets"] = json!(geometry.targets);
                }
                primitive
            })
            .collect::<Vec<_>>();
        if primitives.is_empty() {
            return Err("模型没有可导出的三角子网格".into());
        }
        let gltf_mesh = json!({"name": mesh.name, "primitives": primitives,
            "extras": {"source_mesh": renderer.mesh,
                "source_mesh_object": key_json(source.meshes[renderer.mesh].key.file_index, source.meshes[renderer.mesh].key.path_id),
                "targetNames": geometry.channels}});
        let mut target_weights = vec![0.0_f32; geometry.targets.len()];
        let mut channel_report = Vec::new();
        if let Some(shapes) = &mesh.blend_shapes {
            let mut target = 0;
            for (channel_index, channel) in shapes.channels.iter().enumerate() {
                let source_weight = renderer
                    .blend_shape_weights
                    .get(channel_index)
                    .copied()
                    .unwrap_or(0.0);
                let full_weight = shapes.full_weights[channel.frame_index];
                let faithful_default = channel.frame_count == 1 && full_weight != 0.0;
                if faithful_default {
                    target_weights[target] = source_weight / full_weight;
                    if !target_weights[target].is_finite() {
                        return Err("标准形态权重投影超出float32范围".into());
                    }
                }
                if !faithful_default {
                    requires_custom_morph_interpolation = true;
                }
                channel_report.push(json!({"channel": channel_index, "name": channel.name,
                    "source_weight": source_weight, "first_target": target,
                    "target_count": channel.frame_count, "default_projection_exact": faithful_default,
                    "requires_custom_morph_interpolation": !faithful_default}));
                target += channel.frame_count;
            }
        }
        let mesh_index = meshes.len();
        meshes.push(gltf_mesh);
        let node_index = nodes.len();
        let mut node = json!({"name": format!("{}:renderer-{index}", source.nodes[renderer.node].name),
            "mesh": mesh_index, "extras": {"unity_enabled": renderer.enabled,
                "source_renderer": key_json(renderer.key.file_index, renderer.key.path_id)}});
        if !geometry.targets.is_empty() {
            node["weights"] = json!(target_weights);
        }
        if !renderer.bones.is_empty() {
            if renderer.bones.len() != mesh.bind_poses.len() || mesh.skin.is_none() {
                return Err("蒙皮骨骼、绑定矩阵或顶点权重不完整".into());
            }
            if mesh.skin.as_ref().is_some_and(|weights| {
                weights.iter().any(|weight| {
                    weight
                        .bone_indices
                        .iter()
                        .any(|joint| *joint as usize >= renderer.bones.len())
                })
            }) {
                return Err("蒙皮关节索引超出骨骼表".into());
            }
            let mut skin = json!({"name": format!("{}:skin-{index}", mesh.name),
                "joints": renderer.bones,
                "inverseBindMatrices": geometry.bind_poses.ok_or("蒙皮缺少绑定矩阵accessor")?});
            if let Some(root) = renderer.root_bone {
                skin["skeleton"] = json!(root);
            }
            node["skin"] = json!(skins.len());
            skins.push(skin);
        }
        nodes.push(node);
        nodes[renderer.node]["children"]
            .as_array_mut()
            .ok_or("模型节点children不是数组")?
            .push(json!(node_index));
        renderer_reports.push(json!({"source_renderer": index, "gltf_node": node_index,
            "gltf_mesh": mesh_index, "source_mesh": renderer.mesh,
            "source_material_slots": renderer.materials,
            "source_bones": renderer.bones, "source_root_bone": renderer.root_bone,
            "source_blend_shape_weights": renderer.blend_shape_weights,
            "morph_channels": channel_report, "enabled": renderer.enabled}));
    }
    for node in &mut nodes {
        if node["children"].as_array().is_some_and(Vec::is_empty) {
            node.as_object_mut()
                .ok_or("GLB节点不是对象")?
                .remove("children");
        }
    }
    let roots = source
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| node.parent.is_none().then_some(index))
        .collect::<Vec<_>>();
    let mut document = json!({"asset": {"version": "2.0", "generator": concat!("Rurino ", env!("CARGO_PKG_VERSION"))},
        "scene": 0, "scenes": [{"name": source.name, "nodes": roots}],
        "materials": source.materials.iter().enumerate().map(|(index, material)| {
            json!({"name": material.material.name, "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0},
                "extras": {"source_material": index, "appearance": "identity_placeholder"}})
        }).collect::<Vec<_>>(),
        "buffers": [{"byteLength": plan.binary_bytes}],
        "extras": {"requires_custom_morph_interpolation": requires_custom_morph_interpolation,
            "materials_are_identity_placeholders": true, "runtime_animation_not_baked": true}});
    document["nodes"] = Value::Array(nodes);
    document["meshes"] = Value::Array(meshes);
    document["bufferViews"] = Value::Array(std::mem::take(&mut plan.views));
    document["accessors"] = Value::Array(std::mem::take(&mut plan.accessors));
    if !skins.is_empty() {
        document["skins"] = json!(skins);
    }
    let mut json_bytes = LimitedBuffer {
        bytes: Vec::new(),
        limit: working_budget_bytes.saturating_sub(plan.working_bytes),
    };
    serde_json::to_writer(&mut json_bytes, &document)?;
    let json_length = (json_bytes.bytes.len() as u64).div_ceil(4) * 4;
    let binary_length = plan.binary_bytes.div_ceil(4) * 4;
    let total = 12_u64
        .checked_add(8 + json_length)
        .and_then(|length| length.checked_add(8 + binary_length))
        .ok_or("GLB长度溢出")?;
    if total > maximum_output_bytes || total > u64::from(u32::MAX) {
        return Err(format!("GLB需要{total}字节，超过输出预算{maximum_output_bytes}").into());
    }
    writer.write_all(b"glTF")?;
    writer.write_all(&2_u32.to_le_bytes())?;
    writer.write_all(&(total as u32).to_le_bytes())?;
    writer.write_all(&(json_length as u32).to_le_bytes())?;
    writer.write_all(b"JSON")?;
    writer.write_all(&json_bytes.bytes)?;
    write_padding(writer, json_length - json_bytes.bytes.len() as u64, b' ')?;
    writer.write_all(&(binary_length as u32).to_le_bytes())?;
    writer.write_all(b"BIN\0")?;
    let mut written = 0;
    for array in &plan.arrays {
        write_padding(writer, array.offset - written, 0)?;
        array.data.write(writer, false)?;
        written = array.offset + array.bytes;
    }
    write_padding(writer, binary_length - written, 0)?;
    Ok(GlbReport {
        bytes: total,
        binary_bytes: plan.binary_bytes,
        working_bytes: plan.working_bytes + json_bytes.bytes.len() as u64,
        manifest: json!({"schema": 1, "file": "model.glb", "bytes": total,
            "binary_chunk_bytes": binary_length, "binary_chunk_file_offset": 28 + json_length,
            "source_float_encoding": "IEEE754-f32-little-endian", "source_integer_encoding": "u32-little-endian",
            "coordinate_conversion": {"reflection": [-1,1,1], "position_normal": "negate_x",
                "tangent": "negate_x_and_w", "quaternion": "x_neg_y_neg_z_w",
                "inverse_bind": "source_serialized_row_major_to_gltf_column_major_C_times_M_times_C",
                "source_inverse_bind_order": "serialized_row_major_e00_e01_through_e33",
                "triangles": "reverse_each_triple",
                "uv": "unchanged_all_dimensions", "weights": "unchanged_f32_no_normalization"},
            "arrays": plan.reports, "meshes": geometries.into_iter().map(|geometry| geometry.report).collect::<Vec<_>>(),
            "renderers": renderer_reports, "requires_custom_morph_interpolation": requires_custom_morph_interpolation,
            "morph_frame_records": "u32_index_then_f32_position3_normal3_tangent3_source_order",
            "appearance": "identity_materials_without_textures_or_shader_emulation"}),
    })
}

fn plan_geometry<'a>(
    plan: &mut Planner<'a>,
    mesh_index: usize,
    geometry: &'a unity_rs_core::mesh::MeshGeometry,
) -> ExportResult<GeometryPlan> {
    let mesh = &geometry.mesh;
    let count = mesh.vertices.len();
    let mut attributes = BTreeMap::new();
    if count == 0 {
        return Err("GLB不能导出零顶点网格".into());
    }
    let position = plan.accessor(
        format!("mesh/{mesh_index}/positions"),
        ArrayData::Vectors(&mesh.vertices),
        count,
        3,
        5126,
        Some(34962),
    )?;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for vertex in &mesh.vertices {
        for channel in 0..3 {
            let value = if channel == 0 {
                -vertex[channel]
            } else {
                vertex[channel]
            };
            if !value.is_finite() {
                return Err("GLB位置含非有限值".into());
            }
            min[channel] = min[channel].min(value);
            max[channel] = max[channel].max(value);
        }
    }
    plan.accessors[position]["min"] = json!(min);
    plan.accessors[position]["max"] = json!(max);
    attributes.insert("POSITION".into(), position);
    if let Some(normals) = &mesh.normals {
        attributes.insert(
            "NORMAL".into(),
            plan.accessor(
                format!("mesh/{mesh_index}/normals"),
                ArrayData::Vectors(normals),
                count,
                3,
                5126,
                Some(34962),
            )?,
        );
    }
    if let Some(tangents) = &geometry.channels.tangents {
        let semantic = if tangents.dimension == 4 {
            "TANGENT"
        } else {
            "_UNITY_TANGENT"
        };
        attributes.insert(
            semantic.into(),
            plan.accessor(
                format!("mesh/{mesh_index}/tangents"),
                ArrayData::Floats(&tangents.values, tangents.dimension, Flip::Tangent),
                count,
                tangents.dimension,
                5126,
                Some(34962),
            )?,
        );
    }
    if let Some(colors) = &geometry.channels.colors {
        let semantic = if matches!(colors.dimension, 3 | 4) {
            "COLOR_0"
        } else {
            "_UNITY_COLOR"
        };
        attributes.insert(
            semantic.into(),
            plan.accessor(
                format!("mesh/{mesh_index}/colors"),
                ArrayData::Floats(&colors.values, colors.dimension, Flip::None),
                count,
                colors.dimension,
                5126,
                Some(34962),
            )?,
        );
    }
    for (channel, uv) in geometry.channels.uvs.iter().enumerate() {
        if let Some(uv) = uv {
            let semantic = if uv.dimension == 2 {
                format!("TEXCOORD_{channel}")
            } else {
                format!("_UNITY_TEXCOORD_{channel}")
            };
            attributes.insert(
                semantic,
                plan.accessor(
                    format!("mesh/{mesh_index}/uv/{channel}"),
                    ArrayData::Floats(&uv.values, uv.dimension, Flip::None),
                    count,
                    uv.dimension,
                    5126,
                    Some(34962),
                )?,
            );
        }
    }
    if let Some(weights) = &mesh.skin {
        if weights
            .iter()
            .flat_map(|weight| weight.bone_indices)
            .any(|joint| joint > u32::from(u16::MAX))
        {
            return Err("GLB关节索引超过65535，不能无损转换为JOINTS_0".into());
        }
        attributes.insert(
            "JOINTS_0".into(),
            plan.accessor(
                format!("mesh/{mesh_index}/joint_indices"),
                ArrayData::Joints(weights),
                count,
                4,
                5123,
                Some(34962),
            )?,
        );
        attributes.insert(
            "WEIGHTS_0".into(),
            plan.accessor(
                format!("mesh/{mesh_index}/weights"),
                ArrayData::Weights(weights),
                count,
                4,
                5126,
                Some(34962),
            )?,
        );
    }
    let indices = mesh
        .sub_meshes
        .iter()
        .enumerate()
        .map(|(index, submesh)| {
            if submesh.indices.is_empty() || !submesh.indices.len().is_multiple_of(3) {
                return Err("GLB子网格缺少完整三角形索引".into());
            }
            plan.accessor(
                format!("mesh/{mesh_index}/submesh/{index}/indices"),
                ArrayData::Indices(&submesh.indices),
                submesh.indices.len(),
                1,
                5125,
                Some(34963),
            )
        })
        .collect::<ExportResult<Vec<_>>>()?;
    let bind_poses = if mesh.bind_poses.is_empty() {
        None
    } else {
        Some(plan.accessor(
            format!("mesh/{mesh_index}/bind_poses"),
            ArrayData::Matrices(&mesh.bind_poses),
            mesh.bind_poses.len(),
            16,
            5126,
            None,
        )?)
    };
    let mut targets = Vec::new();
    let mut channels = Vec::new();
    let mut morph_report = Vec::new();
    if let Some(shapes) = &mesh.blend_shapes {
        let mut covered = vec![false; shapes.frames.len()];
        plan.charge(covered.len() as u64)?;
        for (channel_index, channel) in shapes.channels.iter().enumerate() {
            let end = channel
                .frame_index
                .checked_add(channel.frame_count)
                .ok_or("形态通道帧范围溢出")?;
            if channel.frame_count == 0
                || end > shapes.frames.len()
                || end > shapes.full_weights.len()
            {
                return Err("形态通道没有完整的帧或权重范围".into());
            }
            for (frame_index, covered) in covered
                .iter_mut()
                .enumerate()
                .take(end)
                .skip(channel.frame_index)
            {
                if *covered {
                    return Err("形态通道重叠引用同一帧".into());
                }
                *covered = true;
                let frame = &shapes.frames[frame_index];
                let vertices = &shapes.vertices[frame.first_vertex as usize
                    ..(frame.first_vertex + frame.vertex_count) as usize];
                let mut order = (0..vertices.len()).collect::<Vec<_>>();
                plan.charge(order.len() as u64 * std::mem::size_of::<usize>() as u64)?;
                order.sort_unstable_by_key(|index| vertices[*index].index);
                let order = Arc::new(order);
                let record = if vertices.is_empty() {
                    None
                } else {
                    Some(plan.view(
                        format!("mesh/{mesh_index}/morph/{frame_index}/source_records"),
                        ArrayData::MorphRecords(vertices),
                        None,
                    )?)
                };
                let mut target = serde_json::Map::new();
                if !vertices.is_empty() {
                    let index_view = plan.view(
                        format!("mesh/{mesh_index}/morph/{frame_index}/sparse_indices"),
                        ArrayData::MorphIndices(vertices, Arc::clone(&order)),
                        None,
                    )?;
                    for (kind, semantic) in [(0, "POSITION"), (1, "NORMAL"), (2, "TANGENT")] {
                        if kind == 1 && (!frame.has_normals || !attributes.contains_key("NORMAL")) {
                            continue;
                        }
                        if kind == 2 && (!frame.has_tangents || !attributes.contains_key("TANGENT"))
                        {
                            continue;
                        }
                        let values = plan.view(
                            format!("mesh/{mesh_index}/morph/{frame_index}/{semantic}"),
                            ArrayData::MorphValues(vertices, Arc::clone(&order), kind),
                            None,
                        )?;
                        let accessor = plan.accessors.len();
                        let mut accessor_value = json!({"componentType": 5126, "count": count, "type": "VEC3",
                            "sparse": {"count": vertices.len(), "indices": {"bufferView": index_view, "componentType": 5125}, "values": {"bufferView": values}}});
                        if kind == 0 {
                            let mut min = [if vertices.len() == count {
                                f32::INFINITY
                            } else {
                                0.0
                            }; 3];
                            let mut max = [if vertices.len() == count {
                                f32::NEG_INFINITY
                            } else {
                                0.0
                            }; 3];
                            for vertex in vertices {
                                for channel in 0..3 {
                                    let value = if channel == 0 {
                                        -vertex.vertex[channel]
                                    } else {
                                        vertex.vertex[channel]
                                    };
                                    min[channel] = min[channel].min(value);
                                    max[channel] = max[channel].max(value);
                                }
                            }
                            accessor_value["min"] = json!(min);
                            accessor_value["max"] = json!(max);
                        }
                        plan.accessors.push(accessor_value);
                        target.insert(semantic.into(), json!(accessor));
                    }
                }
                if target.is_empty() {
                    let accessor = plan.accessors.len();
                    plan.accessors.push(json!({"componentType": 5126, "count": count, "type": "VEC3", "min": [0,0,0], "max": [0,0,0]}));
                    target.insert("POSITION".into(), json!(accessor));
                }
                targets.push(Value::Object(target));
                channels.push(json!(format!("{}:frame-{frame_index}", channel.name)));
                morph_report.push(json!({"channel": channel_index, "name": channel.name,
                    "name_hash": channel.name_hash, "frame": frame_index, "target": targets.len() - 1,
                    "source_first_vertex": frame.first_vertex, "source_vertex_count": frame.vertex_count,
                    "has_normals": frame.has_normals, "has_tangents": frame.has_tangents,
                    "source_full_weight": shapes.full_weights[frame_index], "source_records_buffer_view": record}));
            }
        }
        if covered.iter().any(|covered| !covered)
            || shapes.full_weights.len() != shapes.frames.len()
        {
            return Err("形态数据含未归属通道的帧或权重，首版拒绝遗漏".into());
        }
    }
    let report = json!({"source_mesh": mesh_index, "name": mesh.name, "vertex_count": count,
        "attributes": attributes, "indices": indices, "bind_poses_accessor": bind_poses,
        "source_bone_name_hashes": mesh.bone_name_hashes, "source_root_bone_name_hash": mesh.root_bone_name_hash,
        "morph_frames": morph_report, "morph_channel_count": mesh.blend_shapes.as_ref().map_or(0, |shape| shape.channels.len())});
    Ok(GeometryPlan {
        attributes,
        indices,
        targets,
        channels,
        bind_poses,
        report,
    })
}

impl<'a> Planner<'a> {
    fn charge(&mut self, bytes: u64) -> ExportResult<()> {
        self.working_bytes = self
            .working_bytes
            .checked_add(bytes)
            .ok_or("GLB规划预算溢出")?;
        if self.working_bytes > self.budget {
            return Err("GLB元数据和排序工作区超过预算".into());
        }
        Ok(())
    }

    fn view(
        &mut self,
        label: String,
        data: ArrayData<'a>,
        target: Option<u32>,
    ) -> ExportResult<usize> {
        self.charge(2048 + label.len() as u64 * 4)?;
        let bytes = data.byte_len(false);
        let offset = self.binary_bytes.div_ceil(4) * 4;
        self.binary_bytes = offset.checked_add(bytes).ok_or("GLB缓冲长度溢出")?;
        let index = self.views.len();
        let mut view = json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.views.push(view);
        let source_hash = data.hash(true)?;
        let converted_hash = data.hash(false)?;
        self.reports.push(
            json!({"label": label, "buffer_view": index, "buffer_byte_offset": offset,
            "byte_length": bytes, "source_byte_length": data.byte_len(true),
            "source_canonical_sha256": source_hash, "glb_sha256": converted_hash,
            "source_order": "original_array_order", "glb_order": data.output_order()}),
        );
        self.arrays.push(ArrayPlan {
            offset,
            bytes,
            data,
        });
        Ok(index)
    }

    fn accessor(
        &mut self,
        label: String,
        data: ArrayData<'a>,
        count: usize,
        dimension: usize,
        component: u32,
        target: Option<u32>,
    ) -> ExportResult<usize> {
        let kind = match dimension {
            1 => "SCALAR",
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            16 => "MAT4",
            _ => return Err("GLB属性维度不受支持".into()),
        };
        let scalar_bytes = if component == 5123 { 2 } else { 4 };
        if data.byte_len(false) != count as u64 * dimension as u64 * scalar_bytes {
            return Err(format!("GLB属性{label}长度与count/dimension不符").into());
        }
        let view = self.view(label, data, target)?;
        let index = self.accessors.len();
        self.accessors.push(
            json!({"bufferView": view, "componentType": component, "count": count, "type": kind}),
        );
        self.reports[view]["accessor"] = json!(index);
        self.reports[view]["count"] = json!(count);
        self.reports[view]["dimension"] = json!(dimension);
        self.reports[view]["component_type"] = json!(component);
        Ok(index)
    }
}

impl ArrayData<'_> {
    fn byte_len(&self, source: bool) -> u64 {
        match self {
            Self::Vectors(values) => values.len() as u64 * 12,
            Self::Floats(values, _, _) => values.len() as u64 * 4,
            Self::Indices(values) => values.len() as u64 * 4,
            Self::Joints(values) => values.len() as u64 * if source { 16 } else { 8 },
            Self::Weights(values) => values.len() as u64 * 16,
            Self::Matrices(values) => values.len() as u64 * 64,
            Self::MorphIndices(values, _) => values.len() as u64 * 4,
            Self::MorphValues(values, _, _) => values.len() as u64 * 12,
            Self::MorphRecords(values) => values.len() as u64 * 40,
        }
    }

    fn output_order(&self) -> &'static str {
        match self {
            Self::Indices(_) => "reversed_triangle_triples",
            Self::MorphIndices(_, _) | Self::MorphValues(_, _, _) => {
                "ascending_sparse_vertex_index"
            }
            _ => "original_array_order",
        }
    }

    fn hash(&self, source: bool) -> ExportResult<String> {
        let mut writer = HashWriter(Sha256::new());
        self.write(&mut writer, source)?;
        Ok(format!("{:x}", writer.0.finalize()))
    }

    fn write(&self, writer: &mut impl Write, source: bool) -> ExportResult<()> {
        match self {
            Self::Vectors(values) => {
                for value in *values {
                    write_floats(
                        writer,
                        value,
                        3,
                        if source { Flip::None } else { Flip::Vector },
                    )?;
                }
            }
            Self::Floats(values, dimension, flip) => write_floats(
                writer,
                values,
                *dimension,
                if source { Flip::None } else { *flip },
            )?,
            Self::Indices(values) => {
                for triangle in values.chunks_exact(3) {
                    for index in if source { [0, 1, 2] } else { [2, 1, 0] } {
                        writer.write_all(&triangle[index].to_le_bytes())?;
                    }
                }
            }
            Self::Joints(values) => {
                for value in *values {
                    for joint in value.bone_indices {
                        if source {
                            writer.write_all(&joint.to_le_bytes())?;
                        } else {
                            writer.write_all(&(joint as u16).to_le_bytes())?;
                        }
                    }
                }
            }
            Self::Weights(values) => {
                for value in *values {
                    write_floats(writer, &value.weights, 4, Flip::None)?;
                }
            }
            Self::Matrices(values) => {
                for value in *values {
                    for index in 0..16 {
                        // Unity序列化按行保存，glTF按列保存。
                        let element = value[if source {
                            index
                        } else {
                            (index % 4) * 4 + index / 4
                        }];
                        if !element.is_finite() {
                            return Err("绑定矩阵包含非有限值".into());
                        }
                        let value = if !source && ((index % 4 == 0) != (index / 4 == 0)) {
                            -element
                        } else {
                            element
                        };
                        writer.write_all(&value.to_le_bytes())?;
                    }
                }
            }
            Self::MorphIndices(values, order) => {
                for index in 0..values.len() {
                    let vertex = &values[if source { index } else { order[index] }];
                    writer.write_all(&vertex.index.to_le_bytes())?;
                }
            }
            Self::MorphValues(values, order, kind) => {
                for index in 0..values.len() {
                    let vertex = &values[if source { index } else { order[index] }];
                    let value = match kind {
                        0 => &vertex.vertex,
                        1 => &vertex.normal,
                        _ => &vertex.tangent,
                    };
                    write_floats(
                        writer,
                        value,
                        3,
                        if source { Flip::None } else { Flip::Vector },
                    )?;
                }
            }
            Self::MorphRecords(values) => {
                for value in *values {
                    writer.write_all(&value.index.to_le_bytes())?;
                    write_floats(writer, &value.vertex, 3, Flip::None)?;
                    write_floats(writer, &value.normal, 3, Flip::None)?;
                    write_floats(writer, &value.tangent, 3, Flip::None)?;
                }
            }
        }
        Ok(())
    }
}

fn write_floats(
    writer: &mut impl Write,
    values: &[f32],
    dimension: usize,
    flip: Flip,
) -> ExportResult<()> {
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err("GLB属性包含非有限浮点数".into());
        }
        let channel = index % dimension;
        let transformed = match flip {
            Flip::None => *value,
            Flip::Vector if channel == 0 => -*value,
            Flip::Tangent if channel == 0 || channel == 3 => -*value,
            _ => *value,
        };
        writer.write_all(&transformed.to_le_bytes())?;
    }
    Ok(())
}

fn write_padding(writer: &mut impl Write, count: u64, byte: u8) -> ExportResult<()> {
    if count > 3 {
        return Err("GLB填充长度异常".into());
    }
    writer.write_all(&[byte; 3][..count as usize])?;
    Ok(())
}

fn key_json(file: usize, path: i64) -> Value {
    json!({"file_index": file, "path_id": path.to_string()})
}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.update(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct LimitedBuffer {
    bytes: Vec<u8>,
    limit: u64,
}

impl Write for LimitedBuffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() as u64 + data.len() as u64 > self.limit {
            return Err(std::io::Error::other("GLB JSON超过内存预算"));
        }
        self.bytes
            .try_reserve_exact(data.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unity_rs_core::mesh::{MeshBlendShapeVertex, MeshBoneWeight};

    use super::{ArrayData, Flip, LimitedBuffer, write_floats};
    use std::io::Write;

    #[test]
    fn coordinate_reflection_preserves_float_bits_and_winding() {
        let source = [1.25_f32, -0.0, -3.5, -1.0];
        let mut output = Vec::new();
        write_floats(&mut output, &source, 4, Flip::Tangent).unwrap();
        let expected = [-1.25_f32, -0.0, -3.5, 1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(output, expected);
        let mut indices = Vec::new();
        ArrayData::Indices(&[7, 1, 9, 5, 0, 2])
            .write(&mut indices, false)
            .unwrap();
        assert_eq!(
            indices,
            [9_u32, 1, 7, 2, 0, 5]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn serialized_bind_pose_rows_become_gltf_columns_without_moving_translation() {
        let source = [[
            1.0_f32, 2.0, 3.0, 10.0, 4.0, 5.0, 6.0, 11.0, 7.0, 8.0, 9.0, 12.0, 0.0, -0.0, 0.0, 1.0,
        ]];
        let mut converted = Vec::new();
        ArrayData::Matrices(&source)
            .write(&mut converted, false)
            .unwrap();
        let reflected: [f32; 16] = std::array::from_fn(|index| {
            f32::from_le_bytes(converted[index * 4..index * 4 + 4].try_into().unwrap())
        });
        assert_eq!(reflected[12], -10.0);
        assert_eq!(reflected[13], 11.0);
        assert_eq!(reflected[14], 12.0);
        assert_eq!(reflected[1], -4.0);
        assert_eq!(reflected[4], -2.0);
        assert_eq!(reflected[7].to_bits(), (-0.0_f32).to_bits());
        let restored: [f32; 16] = std::array::from_fn(|index| {
            let row = index / 4;
            let column = index % 4;
            let value = reflected[column * 4 + row];
            if (row == 0) != (column == 0) {
                -value
            } else {
                value
            }
        });
        assert_eq!(restored.map(f32::to_bits), source[0].map(f32::to_bits));
        let mut restored = Vec::new();
        ArrayData::Matrices(&source)
            .write(&mut restored, true)
            .unwrap();
        assert_eq!(
            restored,
            source[0]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn sparse_projection_keeps_source_records_and_all_weight_values() {
        let records = [
            MeshBlendShapeVertex {
                index: 3,
                vertex: [1.0, 2.0, 3.0],
                normal: [4.0, 5.0, 6.0],
                tangent: [7.0, 8.0, 9.0],
            },
            MeshBlendShapeVertex {
                index: 1,
                vertex: [10.0, 11.0, 12.0],
                normal: [13.0, 14.0, 15.0],
                tangent: [16.0, 17.0, 18.0],
            },
        ];
        let order = Arc::new(vec![1, 0]);
        let mut indices = Vec::new();
        ArrayData::MorphIndices(&records, Arc::clone(&order))
            .write(&mut indices, false)
            .unwrap();
        assert_eq!(
            indices,
            [1_u32, 3]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        );
        let mut values = Vec::new();
        ArrayData::MorphValues(&records, order, 2)
            .write(&mut values, false)
            .unwrap();
        assert_eq!(
            values,
            [-16.0_f32, 17.0, 18.0, -7.0, 8.0, 9.0]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
        );
        let mut source = Vec::new();
        ArrayData::MorphRecords(&records)
            .write(&mut source, false)
            .unwrap();
        assert_eq!(source.len(), 80);
        assert_eq!(&source[..4], &3_u32.to_le_bytes());
        let weights = [MeshBoneWeight {
            weights: [0.1, 0.2, 0.3, -0.0],
            bone_indices: [4, 3, 2, 1],
        }];
        let mut output = Vec::new();
        ArrayData::Weights(&weights)
            .write(&mut output, false)
            .unwrap();
        assert_eq!(
            output,
            weights[0]
                .weights
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn json_buffer_limit_rejects_before_appending() {
        let mut buffer = LimitedBuffer {
            bytes: Vec::new(),
            limit: 4,
        };
        buffer.write_all(b"1234").unwrap();
        assert!(buffer.write_all(b"5").is_err());
        assert_eq!(buffer.bytes, b"1234");
    }
}
