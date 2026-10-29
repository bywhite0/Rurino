use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

use serde_json::{Value, json};
use unity_rs_core::endian::{Endian, EndianReader};
use unity_rs_core::loader::AssetCollection;
use unity_rs_core::material::{Material, MaterialReadLimits, read_material};
use unity_rs_core::mesh::{MeshGeometry, MeshReadLimits, read_mesh_geometry_with_collection};
use unity_rs_core::renderer::{
    Renderer, RendererReadLimits, read_mesh_renderer, read_skinned_mesh_renderer,
};
use unity_rs_core::scene::{
    SceneReadLimits, read_game_object, read_mesh_filter, read_transform, resolve_object_reference,
};
use unity_rs_core::scene_hierarchy::SceneObjectKey;
use unity_rs_core::serialized::{ObjectReference, SerializedFile, TypeTree, TypeTreeNode};
use unity_rs_core::source::{Region, RegionCursor};
use unity_rs_core::type_tree::{
    TypeTreeReadLimits, TypeValue, read_type_tree_from_reader, validate_tree_shape,
};

use crate::export::ExportResult;

const MIB: u64 = 1024 * 1024;

pub struct SourceModel {
    pub name: String,
    pub root: usize,
    pub nodes: Vec<SourceNode>,
    pub meshes: Vec<SourceMesh>,
    pub renderers: Vec<SourceRenderer>,
    pub materials: Vec<SourceMaterial>,
    pub metadata: Value,
    pub working_bytes: u64,
}

pub struct SourceNode {
    pub key: SceneObjectKey,
    pub transform_key: SceneObjectKey,
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub active: bool,
    pub metadata: Value,
}

pub struct SourceMesh {
    pub key: SceneObjectKey,
    pub geometry: MeshGeometry,
    pub metadata: Value,
}

pub struct SourceRenderer {
    pub key: SceneObjectKey,
    pub node: usize,
    pub mesh: usize,
    pub materials: Vec<Option<usize>>,
    pub bones: Vec<usize>,
    pub root_bone: Option<usize>,
    pub blend_shape_weights: Vec<f32>,
    pub enabled: bool,
    pub metadata: Value,
}

pub struct SourceMaterial {
    pub key: SceneObjectKey,
    pub material: Material,
    pub metadata: Value,
}

struct PendingSkin {
    renderer: usize,
    bones: Vec<SceneObjectKey>,
    root: Option<SceneObjectKey>,
}

struct ModelReader<'a> {
    collection: &'a AssetCollection,
    model: SourceModel,
    objects: BTreeMap<SceneObjectKey, usize>,
    meshes: BTreeMap<SceneObjectKey, usize>,
    materials: BTreeMap<SceneObjectKey, usize>,
    pending_skins: Vec<PendingSkin>,
    budget: u64,
    used: u64,
    index_bytes: u64,
}

pub fn collect_model(
    collection: &AssetCollection,
    root: SceneObjectKey,
    budget: u64,
) -> ExportResult<SourceModel> {
    if budget < 16 * MIB {
        return Err("模型读取软预算至少为16MiB".into());
    }
    let object_count = collection
        .serialized_files()
        .iter()
        .map(|file| file.file.objects.len() as u64)
        .sum::<u64>();
    let index_bytes = object_count.checked_mul(96).ok_or("模型索引大小溢出")?;
    if index_bytes > budget / 4 {
        return Err("模型对象索引超过预算，请缩小依赖集合".into());
    }
    let objects = collection
        .serialized_files()
        .iter()
        .enumerate()
        .flat_map(|(file_index, file)| {
            file.file
                .objects
                .iter()
                .enumerate()
                .map(move |(index, object)| {
                    (
                        SceneObjectKey {
                            file_index,
                            path_id: object.path_id,
                        },
                        index,
                    )
                })
        })
        .collect::<BTreeMap<_, _>>();
    if objects.len() as u64 != object_count {
        return Err("依赖集合含重复对象身份".into());
    }
    let mut reader = ModelReader {
        collection,
        objects,
        meshes: BTreeMap::new(),
        materials: BTreeMap::new(),
        pending_skins: Vec::new(),
        budget,
        used: index_bytes,
        index_bytes,
        model: SourceModel {
            name: String::new(),
            root: 0,
            nodes: Vec::new(),
            meshes: Vec::new(),
            renderers: Vec::new(),
            materials: Vec::new(),
            metadata: json!({
                "coordinate_frame": "unity_source_local",
                "selection": "selected_game_object_descendants",
                "outside_parent": "保留原引用；所选根的局部坐标作为导出坐标系",
                "runtime_components": "记录引用，不执行动画、骨骼复制、Shader或物理",
                "memory_contract": "保留数据与临时解析的软估算，不是硬RSS上限",
            }),
            working_bytes: 0,
        },
    };
    let mut stack = vec![(root, None)];
    let mut visited = BTreeSet::new();
    while let Some((key, parent)) = stack.pop() {
        if !visited.insert(key) {
            return Err("模型层级重复引用或存在环".into());
        }
        let children = reader.read_node(key, parent)?;
        let parent_index = reader.model.nodes.len() - 1;
        stack.extend(
            children
                .into_iter()
                .rev()
                .map(|child| (child, Some(parent_index))),
        );
        if stack.len() as u64 * 64 > reader.remaining()? / 4 {
            return Err("模型层级遍历超过工作区预算".into());
        }
    }
    reader.resolve_skins()?;
    if reader.model.renderers.is_empty() {
        return Err("所选对象没有可导出的MeshRenderer或SkinnedMeshRenderer".into());
    }
    reader.model.name = reader.model.nodes[0].name.clone();
    reader.model.metadata["root"] = source_identity(collection, root);
    reader.model.metadata["counts"] = json!({
        "nodes": reader.model.nodes.len(), "meshes": reader.model.meshes.len(),
        "renderers": reader.model.renderers.len(), "materials": reader.model.materials.len(),
    });
    reader.model.working_bytes = reader.used.saturating_sub(reader.index_bytes);
    Ok(reader.model)
}

pub fn model_roots(
    collection: &AssetCollection,
    root_file_count: usize,
    budget: u64,
) -> ExportResult<Vec<SceneObjectKey>> {
    let mut roots = Vec::new();
    for (file_index, loaded) in collection
        .serialized_files()
        .iter()
        .take(root_file_count)
        .enumerate()
    {
        for (index, object) in loaded.file.objects.iter().enumerate() {
            if object.class_id != 1 {
                continue;
            }
            let game_object = read_game_object(&loaded.file, index, scene_limits(budget))?;
            for component in game_object.components {
                let target = resolve_object_reference(collection, file_index, component.component)?
                    .ok_or("GameObject含空组件引用")?;
                if !matches!(target.object.class_id, 4 | 224) {
                    continue;
                }
                let transform =
                    read_transform(target.file, target.object_index, scene_limits(budget))?;
                if transform.father.is_null() {
                    roots.push(SceneObjectKey {
                        file_index,
                        path_id: object.path_id,
                    });
                }
            }
        }
    }
    Ok(roots)
}

impl ModelReader<'_> {
    fn remaining(&self) -> ExportResult<u64> {
        self.budget
            .checked_sub(self.used)
            .filter(|value| *value >= MIB)
            .ok_or_else(|| "模型保留数据已耗尽软预算".into())
    }

    fn charge(&mut self, bytes: u64) -> ExportResult<()> {
        self.used = self.used.checked_add(bytes).ok_or("模型预算累计溢出")?;
        self.remaining()?;
        Ok(())
    }

    fn target(&self, key: SceneObjectKey, class: i32) -> ExportResult<(&SerializedFile, usize)> {
        let file = &self
            .collection
            .serialized_files()
            .get(key.file_index)
            .ok_or("模型序列化文件索引越界")?
            .file;
        let index = *self.objects.get(&key).ok_or("模型对象引用缺失")?;
        if file.objects[index].class_id != class {
            return Err(format!("对象{}类型不符，预期{class}", key.path_id).into());
        }
        Ok((file, index))
    }

    fn read_node(
        &mut self,
        key: SceneObjectKey,
        parent: Option<usize>,
    ) -> ExportResult<Vec<SceneObjectKey>> {
        let limits = scene_limits(self.remaining()?);
        let (file, index) = self.target(key, 1)?;
        let game_object = read_game_object(file, index, limits)?;
        let active = match projected_field(file, index, "m_IsActive", self.remaining()?)? {
            TypeValue::Boolean(value) => value,
            value => integer(&value)? != 0,
        };
        let components = game_object
            .components
            .iter()
            .map(|component| {
                resolve_object_reference(self.collection, key.file_index, component.component)?
                    .ok_or_else(|| "GameObject含空组件引用".into())
                    .map(|target| {
                        (
                            SceneObjectKey {
                                file_index: target.file_index,
                                path_id: target.object.path_id,
                            },
                            target.object.class_id,
                        )
                    })
            })
            .collect::<ExportResult<Vec<_>>>()?;
        let transforms = components
            .iter()
            .filter(|component| matches!(component.1, 4 | 224))
            .collect::<Vec<_>>();
        if transforms.len() != 1 {
            return Err("模型GameObject必须有且只有一个Transform".into());
        }
        let transform_key = transforms[0].0;
        let (transform_file, transform_index) = self.target_any_transform(transform_key)?;
        let transform = read_transform(transform_file, transform_index, limits)?;
        if require_reference(
            self.collection,
            transform_key.file_index,
            transform.component.game_object,
            1,
        )? != key
        {
            return Err("Transform所属GameObject不匹配".into());
        }
        let translation = [
            transform.local_position.x,
            transform.local_position.y,
            transform.local_position.z,
        ];
        let rotation = [
            transform.local_rotation.x,
            transform.local_rotation.y,
            transform.local_rotation.z,
            transform.local_rotation.w,
        ];
        let scale = [
            transform.local_scale.x,
            transform.local_scale.y,
            transform.local_scale.z,
        ];
        if !translation
            .iter()
            .chain(&rotation)
            .chain(&scale)
            .all(|value| value.is_finite())
        {
            return Err("节点TRS包含非有限浮点值".into());
        }
        let children = transform
            .children
            .iter()
            .map(|reference| {
                let child_key =
                    require_transform(self.collection, transform_key.file_index, *reference)?;
                let (child_file, child_index) = self.target_any_transform(child_key)?;
                let child = read_transform(child_file, child_index, limits)?;
                if require_transform(self.collection, child_key.file_index, child.father)?
                    != transform_key
                {
                    return Err("Transform父子引用不一致".into());
                }
                require_reference(
                    self.collection,
                    child_key.file_index,
                    child.component.game_object,
                    1,
                )
            })
            .collect::<ExportResult<Vec<_>>>()?;
        let metadata = json!({
            "source": source_identity(self.collection, key), "layer": game_object.layer,
            "parent_transform": reference_json(transform.father),
            "components": components.iter().map(|(component, class)| json!({
                "source": source_identity(self.collection, *component), "class_id": class,
                "semantic_export": matches!(class, 4 | 224 | 23 | 33 | 137),
            })).collect::<Vec<_>>(),
        });
        self.charge(
            (size_of::<SourceNode>()
                + game_object.name.len()
                + children.len() * 8
                + components.len() * 96) as u64
                + serde_json::to_vec(&metadata)?.len() as u64 * 3,
        )?;
        let node = self.model.nodes.len();
        self.model.nodes.push(SourceNode {
            key,
            transform_key,
            name: game_object.name,
            parent,
            children: Vec::new(),
            translation,
            rotation,
            scale,
            active,
            metadata,
        });
        if let Some(parent) = parent {
            self.model.nodes[parent].children.push(node);
        }
        let mesh_filter = components
            .iter()
            .find(|component| component.1 == 33)
            .map(|component| component.0);
        if components
            .iter()
            .filter(|component| component.1 == 33)
            .count()
            > 1
        {
            return Err("同一GameObject含多个MeshFilter，首版不猜测绑定".into());
        }
        for (component, class) in components {
            if class == 23 {
                let (file, index) = self.target(component, 23)?;
                let renderer = read_mesh_renderer(file, index, renderer_limits(self.remaining()?))?;
                let mesh_filter = mesh_filter.ok_or("MeshRenderer没有MeshFilter")?;
                let (file, index) = self.target(mesh_filter, 33)?;
                let filter = read_mesh_filter(file, index, limits)?;
                let mesh =
                    require_reference(self.collection, mesh_filter.file_index, filter.mesh, 43)?;
                self.add_renderer(component, node, mesh, renderer.renderer, Vec::new(), None, Vec::new(), json!({
                    "mesh_filter": source_identity(self.collection, mesh_filter), "trailing_bytes": renderer.trailing_bytes,
                }))?;
            }
            if class == 137 {
                let (file, index) = self.target(component, 137)?;
                let renderer =
                    read_skinned_mesh_renderer(file, index, renderer_limits(self.remaining()?))?;
                let root_reference = object_reference(&projected_field(
                    file,
                    index,
                    "m_RootBone",
                    self.remaining()?,
                )?)?;
                let root_bone = if root_reference.is_null() {
                    None
                } else {
                    Some(require_transform(
                        self.collection,
                        component.file_index,
                        root_reference,
                    )?)
                };
                let mesh =
                    require_reference(self.collection, component.file_index, renderer.mesh, 43)?;
                let bones = renderer
                    .bones
                    .iter()
                    .map(|reference| {
                        require_transform(self.collection, component.file_index, *reference)
                    })
                    .collect::<ExportResult<Vec<_>>>()?;
                if renderer
                    .blend_shape_weights
                    .iter()
                    .any(|value| !value.is_finite())
                {
                    return Err("默认形态权重包含非有限值".into());
                }
                self.add_renderer(component, node, mesh, renderer.renderer, bones, root_bone,
                    renderer.blend_shape_weights, json!({"quality": renderer.quality,
                        "update_when_offscreen": renderer.update_when_offscreen, "trailing_bytes": renderer.trailing_bytes}))?;
            }
        }
        Ok(children)
    }

    fn target_any_transform(&self, key: SceneObjectKey) -> ExportResult<(&SerializedFile, usize)> {
        let file = &self
            .collection
            .serialized_files()
            .get(key.file_index)
            .ok_or("Transform文件缺失")?
            .file;
        let index = *self.objects.get(&key).ok_or("Transform对象缺失")?;
        if !matches!(file.objects[index].class_id, 4 | 224) {
            return Err("骨骼或层级引用不是Transform".into());
        }
        Ok((file, index))
    }

    #[allow(clippy::too_many_arguments)]
    fn add_renderer(
        &mut self,
        key: SceneObjectKey,
        node: usize,
        mesh_key: SceneObjectKey,
        renderer: Renderer,
        bones: Vec<SceneObjectKey>,
        root: Option<SceneObjectKey>,
        blend_shape_weights: Vec<f32>,
        mut metadata: Value,
    ) -> ExportResult<()> {
        if renderer.virtual_geometry != 0 || renderer.virtual_geometry_shadow != 0 {
            return Err("首版模型不支持虚拟几何".into());
        }
        if renderer.static_batch_info.sub_mesh_count != 0 || !renderer.static_batch_root.is_null() {
            return Err("首版模型不支持运行时静态合批映射".into());
        }
        let mesh = self.register_mesh(mesh_key)?;
        let materials = renderer
            .materials
            .iter()
            .map(|reference| {
                if reference.is_null() {
                    return Ok(None);
                }
                let material = require_reference(self.collection, key.file_index, *reference, 21)?;
                self.register_material(material).map(Some)
            })
            .collect::<ExportResult<Vec<_>>>()?;
        if materials.len() < self.model.meshes[mesh].geometry.mesh.sub_meshes.len() {
            return Err("材质槽少于子网格，首版不猜测材质回退".into());
        }
        metadata["source"] = source_identity(self.collection, key);
        metadata["mesh_source"] = source_identity(self.collection, mesh_key);
        metadata["bone_sources"] = json!(
            bones
                .iter()
                .map(|bone| source_identity(self.collection, *bone))
                .collect::<Vec<_>>()
        );
        metadata["root_bone_source"] = root
            .map(|bone| source_identity(self.collection, bone))
            .unwrap_or(Value::Null);
        metadata["cast_shadows"] = json!(renderer.cast_shadows);
        metadata["receive_shadows"] = json!(renderer.receive_shadows);
        self.charge(
            (size_of::<SourceRenderer>()
                + bones.len() * 32
                + materials.len() * 16
                + blend_shape_weights.len() * 4) as u64
                + serde_json::to_vec(&metadata)?.len() as u64 * 3,
        )?;
        let index = self.model.renderers.len();
        self.model.renderers.push(SourceRenderer {
            key,
            node,
            mesh,
            materials,
            bones: Vec::new(),
            root_bone: None,
            blend_shape_weights,
            enabled: renderer.enabled,
            metadata,
        });
        self.pending_skins.push(PendingSkin {
            renderer: index,
            bones,
            root,
        });
        Ok(())
    }

    fn register_mesh(&mut self, key: SceneObjectKey) -> ExportResult<usize> {
        if let Some(index) = self.meshes.get(&key) {
            return Ok(*index);
        }
        let remaining = self.remaining()?;
        let (file, index) = self.target(key, 43)?;
        let variable = projected_field(file, index, "m_VariableBoneCountWeights", remaining)?;
        let variable_data = match &variable {
            TypeValue::Object(fields) => fields
                .iter()
                .find(|field| field.name == "m_Data")
                .map(|field| &field.value),
            TypeValue::Array(_) | TypeValue::ByteArray { .. } => Some(&variable),
            _ => None,
        }
        .ok_or("可变骨权重字段结构未验证")?;
        if variable_data.array_len() != Some(0) {
            return Err("Mesh含可变数量骨权重，核心读者未保留，拒绝不完整模型".into());
        }
        let sub_meshes = projected_field(file, index, "m_SubMeshes", remaining)?;
        let TypeValue::Array(sub_meshes) = sub_meshes else {
            return Err("子网格字段不是数组".into());
        };
        for submesh in &sub_meshes {
            if integer(field(submesh, "baseVertex")?)? != 0
                || integer(field(submesh, "topology")?)? != 0
            {
                return Err("数值无损导出首版仅接受零baseVertex的三角列表".into());
            }
        }
        let metadata = json!({
            "source": source_identity(self.collection, key),
            "sub_meshes": sub_meshes.iter().map(value_json).collect::<ExportResult<Vec<_>>>()?,
            "variable_bone_count_weights": "verified_empty",
            "mesh_compression": value_json(&projected_field(file, index, "m_MeshCompression", remaining)?)?,
            "source_vertex_values": "解码后的原float32数值；原压缩字节仍需源包归档",
        });
        let geometry = read_mesh_geometry_with_collection(
            self.collection,
            file,
            index,
            mesh_limits(remaining),
        )?;
        validate_geometry(&geometry)?;
        let channel_metadata = source_channel_metadata(
            self.collection,
            file,
            index,
            geometry.mesh.vertices.len(),
            remaining,
        )?;
        let mut metadata = metadata;
        metadata["vertex_channels"] = channel_metadata.0;
        metadata["fourth_component_policy"] = channel_metadata.1;
        let bytes = geometry_bytes(&geometry)?;
        self.charge(bytes + serde_json::to_vec(&metadata)?.len() as u64 * 3 + 256)?;
        let index = self.model.meshes.len();
        self.model.meshes.push(SourceMesh {
            key,
            geometry,
            metadata,
        });
        self.meshes.insert(key, index);
        Ok(index)
    }

    fn register_material(&mut self, key: SceneObjectKey) -> ExportResult<usize> {
        if let Some(index) = self.materials.get(&key) {
            return Ok(*index);
        }
        let (file, index) = self.target(key, 21)?;
        let material = read_material(
            file,
            index,
            MaterialReadLimits {
                maximum_object_bytes: self.remaining()?,
                maximum_string_bytes: 1024 * 1024,
                maximum_total_string_bytes: usize::try_from(self.remaining()? / 4)?,
                maximum_array_elements: 100_000,
            },
        )?;
        if material
            .saved_properties
            .floats
            .iter()
            .any(|property| !property.value.is_finite())
            || material
                .saved_properties
                .colors
                .iter()
                .any(|property| property.value.iter().any(|value| !value.is_finite()))
            || material
                .saved_properties
                .texture_environments
                .iter()
                .any(|property| {
                    property
                        .value
                        .scale
                        .iter()
                        .chain(&property.value.offset)
                        .any(|value| !value.is_finite())
                })
        {
            return Err("材质属性包含非有限浮点值".into());
        }
        if material.trailing_bytes > 1024 * 1024 {
            return Err("材质未解释尾部超过1MiB，拒绝无界元数据复制".into());
        }
        let payload = file.object_region(index)?;
        let tail = payload
            .subregion(
                payload
                    .len()
                    .checked_sub(material.trailing_bytes)
                    .ok_or("材质尾部越界")?,
                material.trailing_bytes,
            )?
            .read_to_vec(1024 * 1024)?;
        let metadata = json!({
            "source": source_identity(self.collection, key), "shader": reference_json(material.shader),
            "shader_target": optional_reference(self.collection,key.file_index,material.shader),
            "raw_object_sha256": crate::export::hash_region(&payload)?,
            "trailing_data_hex": tail.iter().map(|byte|format!("{byte:02x}")).collect::<String>(),
            "legacy_shader_keywords": material.legacy_shader_keywords, "valid_keywords": material.valid_keywords,
            "invalid_keywords": material.invalid_keywords, "lightmap_flags": material.lightmap_flags,
            "enable_instancing_variants": material.enable_instancing_variants,
            "custom_render_queue": material.custom_render_queue, "string_tags": material.string_tags,
            "disabled_shader_passes": material.disabled_shader_passes, "trailing_bytes": material.trailing_bytes,
            "integers": material.saved_properties.integers.iter().map(|property| json!({"name": property.name,"value": property.value})).collect::<Vec<_>>(),
            "floats": material.saved_properties.floats.iter().map(|property| json!({"name": property.name,"value": property.value,"bits": property.value.to_bits()})).collect::<Vec<_>>(),
            "colors": material.saved_properties.colors.iter().map(|property| json!({"name": property.name,"value": property.value,"bits": property.value.map(f32::to_bits)})).collect::<Vec<_>>(),
            "textures": material.saved_properties.texture_environments.iter().map(|property| json!({
                "name": property.name,"texture": reference_json(property.value.texture),
                "texture_target": optional_reference(self.collection,key.file_index,property.value.texture),
                "scale": property.value.scale,"offset": property.value.offset,
                "scale_bits": property.value.scale.map(f32::to_bits),
                "offset_bits": property.value.offset.map(f32::to_bits),
            })).collect::<Vec<_>>(),
            "projection": "保存源属性；标准材质占位，不模拟游戏Shader",
        });
        self.charge(serde_json::to_vec(&metadata)?.len() as u64 * 5 + 512)?;
        let index = self.model.materials.len();
        self.model.materials.push(SourceMaterial {
            key,
            material,
            metadata,
        });
        self.materials.insert(key, index);
        Ok(index)
    }

    fn resolve_skins(&mut self) -> ExportResult<()> {
        let transforms = self
            .model
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.transform_key, index))
            .collect::<BTreeMap<_, _>>();
        for skin in self.pending_skins.drain(..) {
            let renderer = &mut self.model.renderers[skin.renderer];
            renderer.bones = skin
                .bones
                .iter()
                .map(|key| {
                    transforms
                        .get(key)
                        .copied()
                        .ok_or_else(|| "蒙皮骨骼位于所选子树之外或缺失".into())
                })
                .collect::<ExportResult<Vec<_>>>()?;
            renderer.root_bone = skin
                .root
                .map(|key| {
                    transforms
                        .get(&key)
                        .copied()
                        .ok_or("rootBone位于所选子树之外")
                })
                .transpose()?;
            let mesh = &self.model.meshes[renderer.mesh].geometry.mesh;
            if let Some(weights) = &mesh.skin {
                if renderer.bones.len() != mesh.bind_poses.len() {
                    return Err("蒙皮骨骼与inverse bind矩阵数量不匹配".into());
                }
                if weights.iter().any(|vertex| {
                    vertex
                        .weights
                        .iter()
                        .zip(vertex.bone_indices)
                        .any(|(weight, index)| {
                            *weight > 0.0 && index as usize >= renderer.bones.len()
                        })
                }) {
                    return Err("蒙皮骨索引越界".into());
                }
            }
            let channel_count = mesh
                .blend_shapes
                .as_ref()
                .map_or(0, |shapes| shapes.channels.len());
            if !renderer.blend_shape_weights.is_empty()
                && renderer.blend_shape_weights.len() != channel_count
            {
                return Err("默认形态权重数量与形态通道不一致".into());
            }
            renderer.metadata["default_blendshape_weights_count"] =
                json!(renderer.blend_shape_weights.len());
            renderer.metadata["empty_default_weights"] =
                json!("保留源空数组，标准模型显示按零权重");
        }
        Ok(())
    }
}

fn require_reference(
    collection: &AssetCollection,
    source: usize,
    reference: ObjectReference,
    class: i32,
) -> ExportResult<SceneObjectKey> {
    let target =
        resolve_object_reference(collection, source, reference)?.ok_or("模型必需引用为空")?;
    if target.object.class_id != class {
        return Err(format!("引用类型{}不符，预期{class}", target.object.class_id).into());
    }
    Ok(SceneObjectKey {
        file_index: target.file_index,
        path_id: target.object.path_id,
    })
}

fn require_transform(
    collection: &AssetCollection,
    source: usize,
    reference: ObjectReference,
) -> ExportResult<SceneObjectKey> {
    let target = resolve_object_reference(collection, source, reference)?
        .ok_or("骨骼或层级Transform引用为空")?;
    if !matches!(target.object.class_id, 4 | 224) {
        return Err("骨骼或层级引用不是Transform".into());
    }
    Ok(SceneObjectKey {
        file_index: target.file_index,
        path_id: target.object.path_id,
    })
}

fn source_identity(collection: &AssetCollection, key: SceneObjectKey) -> Value {
    json!({"file_index":key.file_index,"serialized_file":collection.serialized_files()[key.file_index].path,"path_id":key.path_id.to_string()})
}

fn reference_json(reference: ObjectReference) -> Value {
    json!({"file_id":reference.file_id,"path_id":reference.path_id.to_string()})
}

fn optional_reference(
    collection: &AssetCollection,
    source: usize,
    reference: ObjectReference,
) -> Value {
    if reference.is_null() {
        return json!({"status":"null"});
    }
    match resolve_object_reference(collection, source, reference) {
        Ok(Some(target)) => {
            json!({"status":"resolved_not_exported","class_id":target.object.class_id,
            "source":source_identity(collection,SceneObjectKey {file_index:target.file_index,path_id:target.object.path_id})})
        }
        Ok(None) => json!({"status":"null"}),
        Err(error) => json!({"status":"unresolved_not_exported","reason":error.to_string()}),
    }
}

fn scene_limits(budget: u64) -> SceneReadLimits {
    SceneReadLimits {
        maximum_string_bytes: 1024 * 1024,
        maximum_game_object_components: 4096,
        maximum_transform_children: 100_000,
        maximum_reference_bytes: budget / 4,
    }
}

fn renderer_limits(budget: u64) -> RendererReadLimits {
    RendererReadLimits {
        maximum_object_bytes: budget,
        maximum_materials: 4096,
        maximum_bones: 100_000,
        maximum_blend_shape_weights: 100_000,
        maximum_reference_bytes: budget / 4,
    }
}

fn mesh_limits(budget: u64) -> MeshReadLimits {
    MeshReadLimits {
        maximum_object_bytes: budget / 2,
        maximum_string_bytes: 1024 * 1024,
        maximum_total_string_bytes: usize::try_from(budget / 8).unwrap_or(usize::MAX),
        maximum_vertices: usize::try_from(budget / 64).unwrap_or(usize::MAX),
        maximum_indices: usize::try_from(budget / 8).unwrap_or(usize::MAX),
        maximum_vertex_data_bytes: usize::try_from(budget / 3).unwrap_or(usize::MAX),
        maximum_decoded_bytes: budget / 3,
        maximum_output_bytes: budget / 3,
        maximum_auxiliary_bytes: budget / 3,
        maximum_compressed_data_bytes: budget / 3,
        ..MeshReadLimits::default()
    }
}

fn geometry_bytes(geometry: &MeshGeometry) -> ExportResult<u64> {
    let mesh = &geometry.mesh;
    let mut bytes = (mesh.vertices.len() * 12
        + mesh.normals.as_ref().map_or(0, |v| v.len() * 12)
        + mesh.uv0.as_ref().map_or(0, |v| v.len() * 8)
        + mesh.bind_poses.len() * 64
        + mesh.bone_name_hashes.len() * 4
        + mesh.skin.as_ref().map_or(0, |v| v.len() * 32)
        + mesh
            .sub_meshes
            .iter()
            .map(|v| v.indices.len() * 4 + 64)
            .sum::<usize>()) as u64;
    for channel in geometry
        .channels
        .uvs
        .iter()
        .flatten()
        .chain(geometry.channels.tangents.iter())
        .chain(geometry.channels.colors.iter())
    {
        bytes = bytes
            .checked_add(channel.values.len() as u64 * 4)
            .ok_or("模型大小溢出")?;
    }
    if let Some(shapes) = &mesh.blend_shapes {
        bytes += (shapes.vertices.len() * 40
            + shapes.frames.len() * 16
            + shapes.full_weights.len() * 4
            + shapes
                .channels
                .iter()
                .map(|v| v.name.len() + 64)
                .sum::<usize>()) as u64;
    }
    Ok(bytes + mesh.name.len() as u64 + 512)
}

fn validate_geometry(geometry: &MeshGeometry) -> ExportResult<()> {
    let mesh = &geometry.mesh;
    if mesh.vertices.is_empty() || mesh.sub_meshes.is_empty() {
        return Err("网格必须包含顶点和子网格".into());
    }
    if mesh.sub_meshes.iter().any(|submesh| {
        submesh.indices.len() % 3 != 0
            || submesh
                .indices
                .iter()
                .any(|index| *index as usize >= mesh.vertices.len())
    }) {
        return Err("网格三角索引数量或范围无效".into());
    }
    if !mesh
        .vertices
        .iter()
        .flatten()
        .chain(mesh.normals.iter().flatten().flatten())
        .chain(mesh.bind_poses.iter().flatten())
        .all(|value| value.is_finite())
    {
        return Err("网格或bind矩阵包含非有限浮点值".into());
    }
    for channel in geometry
        .channels
        .uvs
        .iter()
        .flatten()
        .chain(geometry.channels.tangents.iter())
        .chain(geometry.channels.colors.iter())
    {
        if !(1..=4).contains(&channel.dimension)
            || channel.values.len() != mesh.vertices.len() * channel.dimension
            || channel.values.iter().any(|value| !value.is_finite())
        {
            return Err("网格顶点通道维度或数值无效".into());
        }
    }
    if mesh.skin.as_ref().is_some_and(|skin| {
        skin.len() != mesh.vertices.len()
            || skin.iter().any(|vertex| {
                vertex
                    .weights
                    .iter()
                    .any(|weight| !weight.is_finite() || *weight < 0.0)
            })
    }) {
        return Err("蒙皮权重数量或数值无效".into());
    }
    if let Some(shapes) = &mesh.blend_shapes {
        if shapes.full_weights.len() != shapes.frames.len()
            || shapes.full_weights.iter().any(|value| !value.is_finite())
        {
            return Err("形态帧权重无效".into());
        }
        let mut covered = vec![false; shapes.frames.len()];
        for channel in &shapes.channels {
            let end = channel
                .frame_index
                .checked_add(channel.frame_count)
                .ok_or("形态通道帧数溢出")?;
            if channel.frame_count == 0 || end > covered.len() {
                return Err("形态通道帧范围无效".into());
            }
            for flag in &mut covered[channel.frame_index..end] {
                if *flag {
                    return Err("形态帧属于重复通道".into());
                }
                *flag = true;
            }
        }
        if covered.iter().any(|flag| !*flag) {
            return Err("形态帧没有对应通道".into());
        }
        for frame in &shapes.frames {
            let start = frame.first_vertex as usize;
            let end = start
                .checked_add(frame.vertex_count as usize)
                .ok_or("形态帧长度溢出")?;
            let vertices = shapes.vertices.get(start..end).ok_or("形态帧索引越界")?;
            let mut indices = BTreeSet::new();
            for vertex in vertices {
                if vertex.index as usize >= mesh.vertices.len()
                    || !indices.insert(vertex.index)
                    || vertex
                        .vertex
                        .iter()
                        .chain(&vertex.normal)
                        .chain(&vertex.tangent)
                        .any(|value| !value.is_finite())
                {
                    return Err("形态稀疏索引重复、越界或数值无效".into());
                }
            }
        }
    }
    Ok(())
}

fn field<'a>(value: &'a TypeValue, name: &str) -> ExportResult<&'a TypeValue> {
    let TypeValue::Object(fields) = value else {
        return Err("模型边界字段不是结构体".into());
    };
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
        .ok_or_else(|| format!("模型边界缺少{name}").into())
}

fn integer(value: &TypeValue) -> ExportResult<i64> {
    match value {
        TypeValue::Signed(value) => Ok(*value),
        TypeValue::Unsigned(value) => Ok(i64::try_from(*value)?),
        _ => Err("模型边界字段不是整数".into()),
    }
}

fn object_reference(value: &TypeValue) -> ExportResult<ObjectReference> {
    Ok(ObjectReference {
        file_id: i32::try_from(integer(field(value, "m_FileID")?)?)?,
        path_id: integer(field(value, "m_PathID")?)?,
    })
}

fn value_json(value: &TypeValue) -> ExportResult<Value> {
    match value {
        TypeValue::Signed(value) => Ok(json!(value)),
        TypeValue::Unsigned(value) => Ok(json!(value)),
        TypeValue::Character(value) => Ok(json!(value)),
        TypeValue::Boolean(value) => Ok(json!(value)),
        TypeValue::Float32(value) if value.is_finite() => Ok(json!(value)),
        TypeValue::Float(value) if value.is_finite() => Ok(json!(value)),
        TypeValue::String(value) => Ok(json!(value)),
        TypeValue::Object(fields) => fields
            .iter()
            .map(|field| Ok((field.name.clone(), value_json(&field.value)?)))
            .collect::<ExportResult<serde_json::Map<_, _>>>()
            .map(Value::Object),
        TypeValue::Array(values) => values
            .iter()
            .map(value_json)
            .collect::<ExportResult<Vec<_>>>()
            .map(Value::Array),
        TypeValue::ByteArray { bytes, .. } => Ok(json!(bytes)),
        _ => Err("模型边界字段含未覆盖的数值类型".into()),
    }
}

// 只读取选定字段，避免把顶点大数组物化成动态树。
fn projected_field(
    file: &SerializedFile,
    object_index: usize,
    name: &str,
    budget: u64,
) -> ExportResult<TypeValue> {
    let object = &file.objects[object_index];
    let tree = file
        .types
        .get(object.serialized_type_index.ok_or("模型对象缺少类型索引")?)
        .and_then(|kind| kind.type_tree.as_ref())
        .ok_or("模型完整性检查需要内嵌TypeTree")?;
    validate_tree_shape(&tree.nodes)?;
    if tree.nodes.len() > 4096 {
        return Err("模型字段TypeTree超过4096节点".into());
    }
    let ends = tree
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (index + 1..tree.nodes.len())
                .find(|next| tree.nodes[*next].level <= node.level)
                .unwrap_or(tree.nodes.len())
        })
        .collect::<Vec<_>>();
    let region = file.object_region(object_index)?;
    let endian = if file.header.endianness == 0 {
        Endian::Little
    } else {
        Endian::Big
    };
    let mut reader = EndianReader::new(region.cursor(), endian);
    let mut visits = 0_u64;
    let (child, start, end) = locate_field(
        &tree.nodes,
        &ends,
        0,
        &name.split('.').collect::<Vec<_>>(),
        &mut reader,
        object.byte_start,
        &mut visits,
    )?;
    let level = tree.nodes[child].level;
    let nodes = tree.nodes[child..ends[child]]
        .iter()
        .map(|node| TypeTreeNode {
            level: node.level - level,
            ..node.clone()
        })
        .collect();
    Ok(read_type_tree_from_reader(
        &TypeTree {
            nodes,
            string_buffer: Vec::new(),
        },
        EndianReader::new(region.subregion(start, end - start)?.cursor(), endian),
        object.byte_start + start,
        TypeTreeReadLimits {
            maximum_values: 100_000,
            maximum_array_elements: 100_000,
            maximum_string_bytes: 1024 * 1024,
            maximum_materialized_bytes: usize::try_from((budget / 8).min(16 * MIB))?,
            ..TypeTreeReadLimits::default()
        },
    )?)
}

fn locate_field(
    nodes: &[TypeTreeNode],
    ends: &[usize],
    parent: usize,
    path: &[&str],
    reader: &mut EndianReader<RegionCursor>,
    absolute: u64,
    visits: &mut u64,
) -> ExportResult<(usize, u64, u64)> {
    let mut child = parent + 1;
    while child < ends[parent] {
        if nodes[child].level != nodes[parent].level + 1 {
            return Err("模型字段层级无效".into());
        }
        if nodes[child].field_name == path[0] {
            if path.len() > 1 {
                return locate_field(nodes, ends, child, &path[1..], reader, absolute, visits);
            }
            let start = reader.position()?;
            skip_field(nodes, ends, child, reader, absolute, visits)?;
            return Ok((child, start, reader.position()?));
        }
        skip_field(nodes, ends, child, reader, absolute, visits)?;
        child = ends[child];
    }
    Err(format!("模型完整性检查缺少字段{}", path.join(".")).into())
}

fn projected_region(file: &SerializedFile, index: usize, path: &str) -> ExportResult<Region> {
    let object = &file.objects[index];
    let tree = file.types[object.serialized_type_index.ok_or("模型类型索引缺失")?]
        .type_tree
        .as_ref()
        .ok_or("模型需要TypeTree")?;
    validate_tree_shape(&tree.nodes)?;
    if tree.nodes.len() > 4096 {
        return Err("模型字段TypeTree超过4096节点".into());
    }
    let ends = tree
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (index + 1..tree.nodes.len())
                .find(|next| tree.nodes[*next].level <= node.level)
                .unwrap_or(tree.nodes.len())
        })
        .collect::<Vec<_>>();
    let region = file.object_region(index)?;
    let mut reader = EndianReader::new(
        region.cursor(),
        if file.header.endianness == 0 {
            Endian::Little
        } else {
            Endian::Big
        },
    );
    let (_, start, end) = locate_field(
        &tree.nodes,
        &ends,
        0,
        &path.split('.').collect::<Vec<_>>(),
        &mut reader,
        object.byte_start,
        &mut 0,
    )?;
    Ok(region.subregion(start, end - start)?)
}

fn source_channel_metadata(
    collection: &AssetCollection,
    file: &SerializedFile,
    index: usize,
    vertex_count: usize,
    budget: u64,
) -> ExportResult<(Value, Value)> {
    let channels = projected_field(file, index, "m_VertexData.m_Channels", budget)?;
    let TypeValue::Array(values) = &channels else {
        return Err("顶点通道不是数组".into());
    };
    let descriptors = values
        .iter()
        .map(|value| {
            Ok((
                usize::try_from(integer(field(value, "stream")?)?)?,
                usize::try_from(integer(field(value, "offset")?)?)?,
                u8::try_from(integer(field(value, "format")?)?)?,
                u8::try_from(integer(field(value, "dimension")?)?)? & 15,
            ))
        })
        .collect::<ExportResult<Vec<_>>>()?;
    if descriptors.len() > 64
        || descriptors
            .iter()
            .any(|channel| channel.0 >= 64 || channel.3 > 4)
    {
        return Err("顶点通道数量或维度超过已验证范围".into());
    }
    let mut policy = json!({});
    if !descriptors.iter().take(2).any(|channel| channel.3 == 4) {
        return Ok((value_json(&channels)?, policy));
    }
    let endian = if file.header.endianness == 0 {
        Endian::Little
    } else {
        Endian::Big
    };
    let raw = projected_region(file, index, "m_VertexData.m_DataSize")?;
    let byte_count = usize::try_from(EndianReader::new(raw.cursor(), endian).read_i32()?)
        .map_err(|_| "顶点字节数为负")?;
    let data = if byte_count > 0 {
        raw.subregion(4, byte_count as u64)?
    } else {
        let stream = projected_field(file, index, "m_StreamData", budget)?;
        let TypeValue::String(path) = field(&stream, "path")? else {
            return Err("外置顶点路径不是字符串".into());
        };
        let resource = collection.resource(path).ok_or("外置顶点流缺失")?;
        resource.region.subregion(
            u64::try_from(integer(field(&stream, "offset")?)?)?,
            u64::try_from(integer(field(&stream, "size")?)?)?,
        )?
    };
    let mut stream_offsets = Vec::new();
    let mut next = 0_u64;
    for stream in 0..=descriptors
        .iter()
        .map(|channel| channel.0)
        .max()
        .unwrap_or(0)
    {
        let stride = descriptors
            .iter()
            .filter(|channel| channel.0 == stream && channel.3 > 0)
            .try_fold(0_u64, |stride, channel| {
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                    stride + format_bytes(channel.2)? * u64::from(channel.3),
                )
            })?;
        stream_offsets.push((next, stride));
        next = next
            .checked_add(
                (vertex_count as u64)
                    .checked_mul(stride)
                    .ok_or("顶点流跨度溢出")?,
            )
            .ok_or("顶点流跨度溢出")?
            .div_ceil(16)
            * 16;
    }
    for (channel_index, name) in [(0, "position"), (1, "normal")] {
        let Some(channel) = descriptors
            .get(channel_index)
            .filter(|channel| channel.3 == 4)
        else {
            continue;
        };
        let (offset, stride) = stream_offsets[channel.0];
        let size = format_bytes(channel.2)?;
        for vertex in 0..vertex_count {
            let position = offset + (vertex as u64) * stride + channel.1 as u64 + 3 * size;
            let mut reader = EndianReader::new(data.subregion(position, size)?.cursor(), endian);
            let positive_zero = match channel.2 {
                0 => reader.read_u32()? == 0,
                1 => reader.read_u16()? == 0,
                2 | 3 => reader.read_u8()? == 0,
                4 | 5 => reader.read_u16()? == 0,
                _ => return Err("四维位置或法线整数格式未覆盖".into()),
            };
            if !positive_zero {
                return Err(
                    format!("源{name}第四分量不是正零，核心三分量读取会丢失，拒绝导出").into(),
                );
            }
        }
        policy[name] = json!({"source_dimension":4,"restore_w_bits":0,"verified_vertices":vertex_count,
            "canonicalization":"标准三分量加已验证正零，可逆恢复源四分量"});
    }
    Ok((value_json(&channels)?, policy))
}

fn format_bytes(format: u8) -> ExportResult<u64> {
    match format {
        0 | 10 | 11 => Ok(4),
        1 | 4 | 5 | 8 | 9 => Ok(2),
        2 | 3 | 6 | 7 => Ok(1),
        _ => Err("顶点源格式未覆盖".into()),
    }
}

fn skip_field(
    nodes: &[TypeTreeNode],
    ends: &[usize],
    index: usize,
    reader: &mut EndianReader<RegionCursor>,
    absolute: u64,
    visits: &mut u64,
) -> ExportResult<()> {
    *visits += 1;
    if *visits > 2_000_000 {
        return Err("模型字段扫描超过节点访问上限".into());
    }
    let node = &nodes[index];
    let end = ends[index];
    if node.type_name == "ReferencedObject" || node.type_name == "ManagedReferencesRegistry" {
        return Err("模型字段扫描不解释托管引用注册表".into());
    }
    let array = nodes
        .get(index + 1)
        .filter(|child| child.type_name == "Array" && child.level == node.level + 1);
    let mut align = node.meta_flags & 0x4000 != 0;
    if node.type_name == "string" || node.type_name == "TypelessData" {
        let count = u64::try_from(reader.read_i32()?).map_err(|_| "字段字节数为负")?;
        skip_bytes(reader, count)?;
        if node.type_name == "string" && count != 0 {
            align = true;
        }
    } else if let Some(array) = array {
        if nodes
            .get(index + 2)
            .is_none_or(|size| size.field_name != "size")
            || nodes
                .get(index + 3)
                .is_none_or(|data| data.level != node.level + 2)
        {
            return Err("模型数组schema未覆盖".into());
        }
        let count = usize::try_from(reader.read_i32()?).map_err(|_| "模型数组长度为负")?;
        if let Some(size) = fixed_size(nodes, ends, index + 3) {
            skip_bytes(
                reader,
                (count as u64).checked_mul(size).ok_or("模型数组跨度溢出")?,
            )?;
        } else {
            if count > 100_000 {
                return Err("动态模型数组过大".into());
            }
            for _ in 0..count {
                skip_field(nodes, ends, index + 3, reader, absolute, visits)?;
            }
        }
        align |= array.meta_flags & 0x4000 != 0;
    } else if index + 1 == end {
        let bytes = match node.type_name.as_str() {
            "bool" | "SInt8" | "UInt8" => 1,
            "char" => u64::try_from(node.byte_size)?,
            "short" | "SInt16" | "UInt16" | "unsigned short" => 2,
            "int" | "SInt32" | "UInt32" | "unsigned int" | "float" | "Type*" => 4,
            "long long" | "SInt64" | "UInt64" | "unsigned long long" | "FileSize" | "double" => 8,
            _ => return Err(format!("模型字段扫描未覆盖叶类型{}", node.type_name).into()),
        };
        skip_bytes(reader, bytes)?;
    } else {
        let mut child = index + 1;
        while child < end {
            skip_field(nodes, ends, child, reader, absolute, visits)?;
            child = ends[child];
        }
    }
    if align {
        let position = reader.position()?;
        let aligned = (absolute + position).div_ceil(4) * 4 - absolute;
        skip_bytes(reader, aligned - position)?;
    }
    Ok(())
}

fn fixed_size(nodes: &[TypeTreeNode], ends: &[usize], index: usize) -> Option<u64> {
    let node = &nodes[index];
    if node.meta_flags & 0x4000 != 0
        || node.type_name == "string"
        || node.type_name == "TypelessData"
        || node.type_name == "Array"
    {
        return None;
    }
    if index + 1 == ends[index] {
        return u64::try_from(node.byte_size).ok();
    }
    let mut size = 0_u64;
    let mut child = index + 1;
    while child < ends[index] {
        size = size.checked_add(fixed_size(nodes, ends, child)?)?;
        child = ends[child];
    }
    Some(size)
}

fn skip_bytes(reader: &mut EndianReader<RegionCursor>, bytes: u64) -> ExportResult<()> {
    let end = reader
        .position()?
        .checked_add(bytes)
        .ok_or("模型字段跨度溢出")?;
    if end > reader.len()? {
        return Err("模型字段超出对象载荷".into());
    }
    reader.set_position(end)?;
    Ok(())
}
