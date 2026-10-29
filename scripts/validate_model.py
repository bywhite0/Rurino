"""用独立UnityPy源数据核验GLB数值、蒙皮与层级。"""

# ruff: noqa: E402

import argparse
import base64
import hashlib
import importlib.util
import json
import os
import platform
import statistics
import struct
import subprocess
import tarfile
import tempfile
import time
import traceback
from pathlib import Path

platform._wmi = None
import numpy as np
import UnityPy
from fsspec.implementations.memory import MemoryFileSystem
from pygltflib import GLTF2
from UnityPy.helpers.MeshHelper import MeshHandler

BASH = "C:/Program Files/Git/bin/bash.exe"


def read(path):
    return subprocess.run(
        [BASH, "-lc", 'cat -- "$(cygpath -u "$1")"', "reader", str(path)], capture_output=True, check=True
    ).stdout


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def cab_name(value):
    return str(value).replace("\\", "/").rsplit("::", 1)[-1].rsplit("/", 1)[-1].lower()


def key(obj):
    return cab_name(obj.assets_file.name) + ":" + str(obj.path_id)


def pointer(source, value, objects):
    if not value["m_PathID"]:
        return None
    cab = cab_name(source.assets_file.name)
    if value["m_FileID"]:
        cab = cab_name(source.assets_file.externals[value["m_FileID"] - 1].path)
    result = cab + ":" + str(value["m_PathID"])
    if result not in objects:
        raise AssertionError("独立oracle引用未闭合：" + result)
    return result


def packed(values, kind="f32"):
    array = np.asarray(values, dtype={"f32": "<f4", "u32": "<u4"}[kind])
    if array.ndim == 1:
        array = array.reshape(-1, 1)
    encoded = array.tobytes()
    return {
        "count": len(array),
        "dimension": array.shape[1],
        "component": kind,
        "sha256": digest(encoded),
        "data": base64.b64encode(encoded).decode("ascii"),
    }


def array_bytes(value):
    return base64.b64decode(value["data"])


class Glb:
    def __init__(self, data):
        # 外部库独立解析结构，访问器字节再由本读取器核验
        self.parsed = GLTF2.load_from_bytes(data)
        if struct.unpack_from("<4sII", data) != (b"glTF", 2, len(data)):
            raise AssertionError("GLB头或声明长度无效")
        cursor = 12
        chunks = []
        while cursor < len(data):
            length, kind = struct.unpack_from("<II", data, cursor)
            if length % 4 or cursor + 8 + length > len(data):
                raise AssertionError("GLB块未对齐或越界")
            chunks.append((kind, data[cursor + 8 : cursor + 8 + length]))
            cursor += 8 + length
        if [kind for kind, _ in chunks] != [0x4E4F534A, 0x004E4942]:
            raise AssertionError("GLB必须是单JSON和单BIN内嵌块")
        self.tree = json.loads(chunks[0][1])
        self.binary = chunks[1][1]
        if len(self.tree["buffers"]) != 1 or self.tree["buffers"][0].get("uri"):
            raise AssertionError("GLB引用外部缓冲区")
        if self.tree.get("images") or self.tree.get("textures") or self.tree.get("animations"):
            raise AssertionError("首步几何导出包含未授权图像或动画")
        for index in range(len(self.tree["bufferViews"])):
            self.view(index)

    def view(self, index):
        view = self.tree["bufferViews"][index]
        offset, length = view.get("byteOffset", 0), view["byteLength"]
        if view.get("buffer", 0) != 0 or offset % 4 or offset + length > len(self.binary):
            raise AssertionError("bufferView越界、未4字节对齐或buffer无效")
        return self.binary[offset : offset + length]

    def accessor(self, index):
        accessor = self.tree["accessors"][index]
        dimension = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}[accessor["type"]]
        dtype = {5120: "i1", 5121: "u1", 5122: "<i2", 5123: "<u2", 5125: "<u4", 5126: "<f4"}[accessor["componentType"]]
        if accessor.get("normalized"):
            raise AssertionError("首步数值导出不应使用归一化访问器")
        result = np.zeros((accessor["count"], dimension), dtype=dtype)
        if "bufferView" in accessor:
            view = self.tree["bufferViews"][accessor["bufferView"]]
            size = np.dtype(dtype).itemsize * dimension
            stride = view.get("byteStride", size)
            data = self.view(accessor["bufferView"])
            offset = accessor.get("byteOffset", 0)
            if stride < size or offset + max(0, accessor["count"] - 1) * stride + size > len(data):
                raise AssertionError("访问器长度或stride越界")
            for row in range(accessor["count"]):
                result[row] = np.frombuffer(data, dtype=dtype, count=dimension, offset=offset + row * stride)
        if "sparse" in accessor:
            sparse = accessor["sparse"]
            indices = sparse["indices"]
            index_dtype = {5121: "u1", 5123: "<u2", 5125: "<u4"}[indices["componentType"]]
            values = np.frombuffer(
                self.view(indices["bufferView"]),
                dtype=index_dtype,
                count=sparse["count"],
                offset=indices.get("byteOffset", 0),
            )
            if len(values) and (max(values) >= accessor["count"] or np.any(values[1:] <= values[:-1])):
                raise AssertionError("稀疏索引不严格递增或越界")
            replacement = np.frombuffer(
                self.view(sparse["values"]["bufferView"]),
                dtype=dtype,
                count=sparse["count"] * dimension,
                offset=sparse["values"].get("byteOffset", 0),
            ).reshape(-1, dimension)
            result[values] = replacement
        return result


def source_array(value):
    return np.frombuffer(array_bytes(value), dtype="<u4" if value["component"] == "u32" else "<f4").reshape(
        value["count"], value["dimension"]
    )


def bits_equal(actual, expected, label):
    expected = np.asarray(expected)
    actual = np.asarray(actual, dtype=expected.dtype)
    if actual.shape != expected.shape or actual.tobytes() != expected.tobytes():
        mismatch = None
        if actual.shape == expected.shape and actual.dtype.itemsize == 4:
            locations = np.argwhere(actual.view(np.uint32) != expected.view(np.uint32))
            mismatch = locations[:6].tolist()
        raise AssertionError(f"逐f32/u32位不一致：{label}；{actual.shape}/{expected.shape}；{mismatch}")


def mirror_vector(value, tangent=False):
    result = np.array(value, dtype="<f4", copy=True)
    result[:, 0] = -result[:, 0]
    if tangent and result.shape[1] == 4:
        result[:, 3] = -result[:, 3]
    return result


def read_closure(group, shared):
    rows = group["closure"]["files"]
    process = subprocess.Popen(
        [BASH, "-lc", "tar --hard-dereference --null -T - -cf -"],
        cwd=shared,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    process.stdin.write(("\0".join(row["path"] for row in rows) + "\0").encode("utf-8"))
    process.stdin.close()
    expected = {row["path"]: row for row in rows}
    environment = UnityPy.Environment(fs=MemoryFileSystem(), path="/rurino-independent")
    root_cabs = set()
    with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
        for index, member in enumerate(archive):
            if not member.isfile():
                continue
            data = archive.extractfile(member).read()
            entry = expected[member.name]
            if len(data) != entry["bytes"] or digest(data) != entry["sha256"]:
                raise AssertionError("原始闭包文件hash不符：" + member.name)
            environment.load_file(data, name=member.name)
            if index == 0:
                root_cabs = {cab_name(obj.assets_file.name) for obj in environment.objects}
    if process.wait() != 0:
        raise RuntimeError(process.stderr.read().decode("utf-8", errors="replace"))
    return environment, root_cabs


def mesh_oracle(obj):
    source = obj.read()
    tree = obj.read_typetree()
    handler = MeshHandler(source)
    handler.process()
    attributes = {}
    for semantic, attribute in (
        ("POSITION", "m_Vertices"),
        ("NORMAL", "m_Normals"),
        ("TANGENT", "m_Tangents"),
        ("COLOR", "m_Colors"),
        ("JOINTS", "m_BoneIndices"),
        ("WEIGHTS", "m_BoneWeights"),
        *[("UV" + str(index), "m_UV" + str(index)) for index in range(8)],
    ):
        values = getattr(handler, attribute, None)
        if values is not None and len(values):
            attributes[semantic] = packed(values, "u32" if semantic == "JOINTS" else "f32")
    # UnityPy三角列表未总是应用baseVertex，在源边界独立补齐
    triangles = handler.get_triangles()
    submeshes = []
    for index, values in enumerate(triangles):
        base = tree["m_SubMeshes"][index].get("baseVertex", 0)
        submeshes.append(packed([[int(vertex) + base] for tri in values for vertex in tri], "u32"))
    bindposes = [
        [matrix[f"e{row}{column}"] for column in range(4) for row in range(4)] for matrix in tree.get("m_BindPose", [])
    ]
    shape = tree.get("m_Shapes", {"vertices": [], "shapes": [], "channels": [], "fullWeights": []})
    frames = []
    for index, frame in enumerate(shape["shapes"]):
        vertices = shape["vertices"][frame["firstVertex"] : frame["firstVertex"] + frame["vertexCount"]]
        frames.append(
            {
                "frame_index": index,
                "first_vertex": frame["firstVertex"],
                "vertex_count": frame["vertexCount"],
                "has_normals": frame["hasNormals"],
                "has_tangents": frame["hasTangents"],
                "indices": [vertex["index"] for vertex in vertices],
                "position": packed([[vertex["vertex"][axis] for axis in "xyz"] for vertex in vertices]),
                "normal": packed([[vertex["normal"][axis] for axis in "xyz"] for vertex in vertices]),
                "tangent": packed([[vertex["tangent"][axis] for axis in "xyz"] for vertex in vertices]),
            }
        )
    return {
        "identity": key(obj),
        "name": source.m_Name,
        "vertices": handler.m_VertexCount,
        "attributes": attributes,
        "submeshes": submeshes,
        "bindposes": packed(bindposes) if bindposes else None,
        "channels": shape["channels"],
        "frames": frames,
        "full_weights": shape["fullWeights"],
        "vertex_channels": tree.get("m_VertexData", {}).get("m_Channels", []),
    }


def material_oracle(obj):
    tree = obj.read_typetree()
    properties = tree["m_SavedProperties"]
    raw = obj.get_raw_data()
    return {
        "identity": key(obj),
        "name": tree["m_Name"],
        "raw_object_sha256": digest(raw),
        "shader": tree["m_Shader"],
        "floats": properties["m_Floats"],
        "integers": properties.get("m_Ints", []),
        "colors": properties["m_Colors"],
        "textures": properties["m_TexEnvs"],
        "valid_keywords": tree.get("m_ValidKeywords", []),
        "invalid_keywords": tree.get("m_InvalidKeywords", []),
        "lightmap_flags": tree.get("m_LightmapFlags"),
        "enable_instancing_variants": tree.get("m_EnableInstancingVariants"),
        "custom_render_queue": tree.get("m_CustomRenderQueue"),
        "disabled_shader_passes": tree.get("disabledShaderPasses", []),
        "source_tail_four_bytes_hex": raw[-4:].hex(),
    }


def validate_materials(case, source, identity):
    materials = {row["identity"]: row for row in case["materials"]}
    compared = 0
    for row in source["materials"]:
        expected = materials[identity(row["key"])]
        actual = row["metadata"]
        if actual["raw_object_sha256"] != expected["raw_object_sha256"]:
            raise AssertionError("材质源对象hash不符")
        shader = actual["shader"]
        if (shader["file_id"], str(shader["path_id"])) != (
            expected["shader"]["m_FileID"],
            str(expected["shader"]["m_PathID"]),
        ):
            raise AssertionError("材质原Shader引用不符")
        for field in (
            "valid_keywords",
            "invalid_keywords",
            "lightmap_flags",
            "enable_instancing_variants",
            "custom_render_queue",
            "disabled_shader_passes",
        ):
            if actual[field] != expected[field]:
                raise AssertionError("材质渲染状态不符：" + field)
        for field, count in (("floats", 1), ("integers", 1), ("colors", 4)):
            values = expected[field]
            if len(actual[field]) != len(values):
                raise AssertionError("材质属性数量不符：" + field)
            for property_row, (name, value) in zip(actual[field], values):
                if property_row["name"] != name:
                    raise AssertionError("材质属性原顺序不符")
                if field == "integers":
                    if property_row["value"] != value:
                        raise AssertionError("材质整型属性不符")
                else:
                    expected_values = [value] if count == 1 else [value[axis] for axis in "rgba"]
                    actual_values = [property_row["value"]] if count == 1 else property_row["value"]
                    bits_equal(actual_values, np.asarray(expected_values, dtype="<f4"), "材质属性/" + name)
                    expected_bits = np.asarray(expected_values, dtype="<f4").view(np.uint32).tolist()
                    actual_bits = [property_row["bits"]] if count == 1 else property_row["bits"]
                    if actual_bits != expected_bits:
                        raise AssertionError("材质属性显式位模式不符")
                compared += 1
        if len(actual["textures"]) != len(expected["textures"]):
            raise AssertionError("材质纹理槽数量不符")
        for property_row, (name, value) in zip(actual["textures"], expected["textures"]):
            texture = property_row["texture"]
            if property_row["name"] != name or (texture["file_id"], str(texture["path_id"])) != (
                value["m_Texture"]["m_FileID"],
                str(value["m_Texture"]["m_PathID"]),
            ):
                raise AssertionError("材质纹理槽原顺序或引用不符")
            for field, native in (("scale", "m_Scale"), ("offset", "m_Offset")):
                expected_values = np.asarray([value[native][axis] for axis in "xy"], dtype="<f4")
                bits_equal(property_row[field], expected_values, "材质纹理ST")
                if property_row[field + "_bits"] != expected_values.view(np.uint32).tolist():
                    raise AssertionError("材质纹理ST位模式不符")
            compared += 1
        # 固定样本原TypeTree在SavedProperties之后有4字节，独立取原对象尾部比较
        if actual["trailing_bytes"] != 4 or actual["trailing_data_hex"] != expected["source_tail_four_bytes_hex"]:
            raise AssertionError("材质源尾部4字节没有完整保留")
    return compared


def prepare(args):
    prior = json.loads(read(args.dependency_report))
    results = []
    for group in prior["groups"]:
        if args.root and group["root"] not in args.root:
            continue
        environment, root_cabs = read_closure(group, args.shared)
        objects = {key(obj): obj for obj in environment.objects}
        trees = {
            identity: obj.read_typetree()
            for identity, obj in objects.items()
            if cab_name(obj.assets_file.name) in root_cabs and obj.type.value in (1, 4, 23, 33, 137)
        }
        nodes = []
        renderers = []
        transform_game_objects = {
            identity: pointer(objects[identity], tree["m_GameObject"], objects)
            for identity, tree in trees.items()
            if objects[identity].type.value == 4
        }
        for identity, tree in trees.items():
            obj = objects[identity]
            if obj.type.value == 4:
                nodes.append(
                    {
                        "identity": transform_game_objects[identity],
                        "transform_identity": identity,
                        "name": trees[transform_game_objects[identity]]["m_Name"],
                        "parent_transform": pointer(obj, tree["m_Father"], objects),
                        "children_transform": [pointer(obj, value, objects) for value in tree["m_Children"]],
                        "translation": [tree["m_LocalPosition"][axis] for axis in "xyz"],
                        "rotation": [tree["m_LocalRotation"][axis] for axis in "xyzw"],
                        "scale": [tree["m_LocalScale"][axis] for axis in "xyz"],
                        "active": trees[transform_game_objects[identity]]["m_IsActive"],
                        "layer": trees[transform_game_objects[identity]]["m_Layer"],
                        "components": [
                            {"identity": target, "class_id": objects[target].type.value}
                            for pair in trees[transform_game_objects[identity]]["m_Component"]
                            for target in [
                                pointer(objects[transform_game_objects[identity]], pair.get("component", pair), objects)
                            ]
                        ],
                    }
                )
            if obj.type.value not in (23, 137):
                continue
            game_object = pointer(obj, tree["m_GameObject"], objects)
            mesh = tree.get("m_Mesh")
            if obj.type.value == 23:
                for component in trees[game_object]["m_Component"]:
                    candidate = pointer(objects[game_object], component.get("component", component), objects)
                    if candidate in trees and objects[candidate].type.value == 33:
                        mesh = trees[candidate]["m_Mesh"]
            if mesh is None:
                raise AssertionError("Renderer没有MeshFilter")
            renderers.append(
                {
                    "identity": identity,
                    "node": game_object,
                    "mesh": pointer(obj, mesh, objects),
                    "bones": [pointer(obj, value, objects) for value in tree.get("m_Bones", [])],
                    "root_bone": pointer(obj, tree["m_RootBone"], objects) if "m_RootBone" in tree else None,
                    "materials": [pointer(obj, value, objects) for value in tree["m_Materials"]],
                    "blendshape_weights": tree.get("m_BlendShapeWeights", []),
                    "enabled": tree["m_Enabled"],
                }
            )
        mesh_keys = sorted({renderer["mesh"] for renderer in renderers})
        meshes = [mesh_oracle(objects[identity]) for identity in mesh_keys]
        material_keys = sorted({identity for row in renderers for identity in row["materials"] if identity})
        materials = [material_oracle(objects[identity]) for identity in material_keys]
        result = {
            "root": group["root"],
            "closure": group["closure"],
            "root_cabs": sorted(root_cabs),
            "nodes": nodes,
            "renderers": renderers,
            "meshes": meshes,
            "materials": materials,
            "totals": {
                "nodes": len(nodes),
                "renderers": len(renderers),
                "meshes": len(meshes),
                "vertices": sum(mesh["vertices"] for mesh in meshes),
                "morph_frames": sum(len(mesh["frames"]) for mesh in meshes),
                "morph_channels": sum(len(mesh["channels"]) for mesh in meshes),
            },
        }
        results.append(result)
        print(json.dumps({"root": result["root"], **result["totals"]}, ensure_ascii=False), flush=True)
    write_json(
        args.output,
        {
            "schema": 1,
            "oracle": "UnityPy direct MeshHandler + raw typetree",
            "unitypy": UnityPy.__version__,
            "cases": results,
        },
    )


def validate_model(case, directory, root_identity=None):
    sidecar = json.loads(read(directory / "model.source.json"))
    glb_bytes = read(directory / "model.glb")
    glb = Glb(glb_bytes)
    files = {row["index"]: cab_name(row["path"]) for row in sidecar["serialized_files"]}

    def identity(value):
        return files[value["file_index"]] + ":" + str(value["path_id"])

    all_nodes = {row["identity"]: row for row in case["nodes"]}
    transforms = {row["transform_identity"]: row["identity"] for row in case["nodes"]}
    selected = set(all_nodes)
    if root_identity:
        selected = set()
        pending = [root_identity]
        while pending:
            current = pending.pop()
            if current in selected:
                raise AssertionError("源oracle层级重复或环")
            selected.add(current)
            pending.extend(transforms[value] for value in all_nodes[current]["children_transform"])
    expected_nodes = {name: row for name, row in all_nodes.items() if name in selected}
    expected_renderers = {row["identity"]: row for row in case["renderers"] if row["node"] in selected}
    expected_meshes = {
        row["identity"]: row
        for row in case["meshes"]
        if row["identity"] in {x["mesh"] for x in expected_renderers.values()}
    }
    source = sidecar["source"]
    material_properties_compared = validate_materials(case, source, identity)
    expected_source_hashes = {row["sha256"] for row in case["closure"]["files"]}
    actual_source_hashes = [row["sha256"] for row in sidecar["sources"]]
    if set(actual_source_hashes) != expected_source_hashes or len(actual_source_hashes) != len(expected_source_hashes):
        raise AssertionError("模型依赖闭包遗漏、重复或文件hash不符")
    source_node_keys = [identity(row["key"]) for row in source["nodes"]]
    source_mesh_keys = [identity(row["key"]) for row in source["meshes"]]
    source_renderer_keys = [identity(row["key"]) for row in source["renderers"]]
    if set(source_node_keys) != set(expected_nodes) or len(source_node_keys) != len(expected_nodes):
        raise AssertionError("源sidecar节点集合重复、遗漏或越过选定根")
    if set(source_mesh_keys) != set(expected_meshes) or len(source_mesh_keys) != len(expected_meshes):
        raise AssertionError("源sidecar Mesh集合重复或遗漏")
    if set(source_renderer_keys) != set(expected_renderers) or len(source_renderer_keys) != len(expected_renderers):
        raise AssertionError("源sidecar Renderer集合重复或遗漏")
    gltf_nodes = glb.tree["nodes"]
    actual_nodes = {
        identity(row["extras"]["source_game_object"]): index
        for index, row in enumerate(gltf_nodes)
        if "source_game_object" in row.get("extras", {})
    }
    actual_renderers = {
        identity(row["extras"]["source_renderer"]): index
        for index, row in enumerate(gltf_nodes)
        if "source_renderer" in row.get("extras", {})
    }
    if set(actual_nodes) != set(expected_nodes) or set(actual_renderers) != set(expected_renderers):
        raise AssertionError("GLB实际节点或Renderer身份集合不符")
    if len(actual_nodes) + len(actual_renderers) != len(gltf_nodes):
        raise AssertionError("GLB存在重复节点或未登记节点")
    for row in source["nodes"]:
        expected = expected_nodes[identity(row["key"])]
        metadata = row["metadata"]
        components = [
            {"identity": identity(item["source"]), "class_id": item["class_id"]} for item in metadata["components"]
        ]
        if metadata["layer"] != expected["layer"] or components != expected["components"]:
            raise AssertionError("源组件身份、顺序、类型或图层未完整保留")
    parents = {}
    for index, node in enumerate(gltf_nodes):
        for child in node.get("children", []):
            if child in parents or child >= len(gltf_nodes):
                raise AssertionError("GLB子节点重复父节点或越界")
            parents[child] = index
    root_nodes = {
        row["identity"] for row in expected_nodes.values() if transforms.get(row["parent_transform"]) not in selected
    }
    scene_roots = glb.tree["scenes"][glb.tree.get("scene", 0)]["nodes"]
    if set(scene_roots) != {actual_nodes[name] for name in root_nodes}:
        raise AssertionError("GLB场景根与选定源根不符")
    for name, expected in expected_nodes.items():
        node = gltf_nodes[actual_nodes[name]]
        if node.get("name") != expected["name"] or bool(node["extras"]["unity_active"]) != bool(expected["active"]):
            raise AssertionError("GLB节点名称或active不符")
        translation = np.asarray(expected["translation"], dtype="<f4")
        translation[0] = -translation[0]
        rotation = np.asarray(expected["rotation"], dtype="<f4")
        rotation[1:3] = -rotation[1:3]
        bits_equal(node.get("translation", [0, 0, 0]), translation, name + "/translation")
        bits_equal(node.get("rotation", [0, 0, 0, 1]), rotation, name + "/rotation")
        bits_equal(node.get("scale", [1, 1, 1]), np.asarray(expected["scale"], dtype="<f4"), name + "/scale")
        expected_children = {
            actual_nodes[transforms[value]] for value in expected["children_transform"] if transforms[value] in selected
        }
        expected_children.update(
            actual_renderers[value] for value, row in expected_renderers.items() if row["node"] == name
        )
        if set(node.get("children", [])) != expected_children:
            raise AssertionError("GLB层级子对象与源层级不符：" + name)
    checked_arrays = 0
    checked_morphs = 0
    checked_skin_vertices = 0
    for name, renderer in expected_renderers.items():
        node_index = actual_renderers[name]
        node = gltf_nodes[node_index]
        if parents.get(node_index) != actual_nodes[renderer["node"]]:
            raise AssertionError("Renderer实例节点父对象不符")
        for field, default in (("translation", [0, 0, 0]), ("rotation", [0, 0, 0, 1]), ("scale", [1, 1, 1])):
            if field in node and node[field] != default:
                raise AssertionError("Renderer子节点含额外TRS变换")
        if bool(node["extras"]["unity_enabled"]) != bool(renderer["enabled"]):
            raise AssertionError("Renderer enabled未保留")
        mesh = expected_meshes[renderer["mesh"]]
        gltf_mesh = glb.tree["meshes"][node["mesh"]]
        source_mesh_index = gltf_mesh["extras"]["source_mesh"]
        if source_mesh_keys[source_mesh_index] != renderer["mesh"]:
            raise AssertionError("Renderer Mesh绑定身份不符")
        source_mesh_metadata = source["meshes"][source_mesh_index]["metadata"]
        if source_mesh_metadata.get("vertex_channels") != mesh["vertex_channels"]:
            raise AssertionError("原始顶点通道维度与格式元数据没有保留")
        primitives = gltf_mesh["primitives"]
        if len(primitives) != len(mesh["submeshes"]):
            raise AssertionError("子网格数量不符")
        native_attributes = mesh["attributes"]
        mapping = {
            "POSITION": "POSITION",
            "NORMAL": "NORMAL",
            "TANGENT": "TANGENT",
            "COLOR": "COLOR_0",
            "JOINTS": "JOINTS_0",
            "WEIGHTS": "WEIGHTS_0",
        }
        mapping.update(
            {
                "UV" + str(index): (
                    "TEXCOORD_"
                    if native_attributes.get("UV" + str(index), {}).get("dimension", 2) == 2
                    else "_UNITY_TEXCOORD_"
                )
                + str(index)
                for index in range(8)
            }
        )
        attributes = primitives[0]["attributes"]
        for semantic, value in native_attributes.items():
            expected = source_array(value)
            if semantic == "JOINTS":
                padded = np.zeros((mesh["vertices"], 4), dtype="<u4")
                padded[:, : expected.shape[1]] = expected
                expected = padded
            if semantic == "WEIGHTS":
                padded = np.zeros((mesh["vertices"], 4), dtype="<f4")
                padded[:, : expected.shape[1]] = expected
                expected = padded
            if semantic in {"POSITION", "NORMAL", "TANGENT"}:
                if semantic == "NORMAL" and expected.shape[1] == 4:
                    if "_UNITY_NORMAL" in attributes:
                        bits_equal(
                            glb.accessor(attributes["_UNITY_NORMAL"]), mirror_vector(expected), name + "/full_normal"
                        )
                    elif np.any(expected[:, 3].view(np.uint32)):
                        raise AssertionError("非零或负零的normal第四分量被丢弃")
                    else:
                        policy = source_mesh_metadata.get("fourth_component_policy", {}).get("normal", {})
                        if (
                            policy.get("source_dimension") != 4
                            or policy.get("restore_w_bits") != 0
                            or policy.get("verified_vertices") != mesh["vertices"]
                        ):
                            raise AssertionError("normal第四分量的+0可恢复规则未明确保存")
                    expected = expected[:, :3]
                expected = mirror_vector(expected, semantic == "TANGENT")
            if mapping[semantic] not in attributes:
                raise AssertionError("GLB遗漏源属性：" + semantic)
            bits_equal(glb.accessor(attributes[mapping[semantic]]), expected, name + "/" + semantic)
            checked_arrays += 1
        if "JOINTS" in native_attributes and "WEIGHTS" not in native_attributes:
            # Unity单索引顶点隐式权重为1，其余槽位补0
            if native_attributes["JOINTS"]["dimension"] != 1:
                raise AssertionError("无权重通道却有多关节影响")
            weights = np.zeros((mesh["vertices"], 4), dtype="<f4")
            weights[:, 0] = 1
            bits_equal(glb.accessor(attributes["WEIGHTS_0"]), weights, name + "/implicit_weights")
        for index, (primitive, original_indices) in enumerate(zip(primitives, mesh["submeshes"])):
            if primitive["attributes"] != attributes or primitive.get("mode", 4) != 4:
                raise AssertionError("子网格复用不同属性集合或不是三角形")
            indices = source_array(original_indices).reshape(-1, 3)[:, ::-1].copy().reshape(-1, 1)
            bits_equal(glb.accessor(primitive["indices"]), indices, name + f"/submesh/{index}")
            expected_material = renderer["materials"][index] if index < len(renderer["materials"]) else None
            if expected_material is not None:
                if "material" not in primitive:
                    raise AssertionError("子网格材质槽遗漏")
                material = glb.tree["materials"][primitive["material"]]
                material_index = material["extras"]["source_material"]
                if identity(source["materials"][material_index]["key"]) != expected_material:
                    raise AssertionError("子网格材质身份或槽位顺序不符")
                if material.get("name") != next(
                    row["name"] for row in case["materials"] if row["identity"] == expected_material
                ):
                    raise AssertionError("材质名称不符")
            elif "material" in primitive:
                raise AssertionError("无材质源槽位被补成另一材质")
        if renderer["bones"]:
            skin = glb.tree["skins"][node["skin"]]
            expected_joints = [actual_nodes[transforms[value]] for value in renderer["bones"]]
            if skin["joints"] != expected_joints:
                raise AssertionError("SMR骨骼原顺序未保留")
            if renderer["root_bone"] and skin.get("skeleton") != actual_nodes[transforms[renderer["root_bone"]]]:
                raise AssertionError("根骨骼绑定不符")
            matrices = source_array(mesh["bindposes"]).copy()
            for index in range(16):
                if (index % 4 == 0) != (index // 4 == 0):
                    matrices[:, index] = -matrices[:, index]
            bits_equal(glb.accessor(skin["inverseBindMatrices"]), matrices, name + "/bindposes")
            joints = glb.accessor(attributes["JOINTS_0"])
            weights = glb.accessor(attributes["WEIGHTS_0"])
            if np.any(joints >= len(skin["joints"])) or np.any(weights < 0):
                raise AssertionError("GLB关节索引或权重无效")
            checked_skin_vertices += len(joints)
        if not renderer["bones"] and "skin" in node:
            raise AssertionError("静态对象出现蒙皮")
        expected_targets = []
        expected_weights = []
        for channel_index, channel in enumerate(mesh["channels"]):
            for frame_index in range(channel["frameIndex"], channel["frameIndex"] + channel["frameCount"]):
                frame = mesh["frames"][frame_index]
                expected_targets.append(frame)
                source_weight = (
                    renderer["blendshape_weights"][channel_index]
                    if channel_index < len(renderer["blendshape_weights"])
                    else 0
                )
                full_weight = mesh["full_weights"][frame_index]
                expected_weights.append(
                    np.float32(source_weight) / np.float32(full_weight)
                    if channel["frameCount"] == 1 and full_weight != 0
                    else np.float32(0)
                )
        if len(primitives[0].get("targets", [])) != len(expected_targets):
            raise AssertionError("morph target数量不符")
        if expected_targets:
            bits_equal(
                node.get("weights", gltf_mesh.get("weights")),
                np.asarray(expected_weights, dtype="<f4"),
                name + "/morph_default",
            )
            for target, frame in zip(primitives[0]["targets"], expected_targets):
                for semantic, field in (("POSITION", "position"), ("NORMAL", "normal"), ("TANGENT", "tangent")):
                    enabled = (
                        semantic == "POSITION"
                        or (semantic == "NORMAL" and frame["has_normals"] and "NORMAL" in attributes)
                        or (semantic == "TANGENT" and frame["has_tangents"] and "TANGENT" in attributes)
                    )
                    if not enabled:
                        if semantic in target:
                            raise AssertionError("GLB产生源中没有的morph属性")
                        continue
                    expected = np.zeros((mesh["vertices"], 3), dtype="<f4")
                    if frame["indices"]:
                        expected[frame["indices"]] = mirror_vector(source_array(frame[field]))
                    bits_equal(
                        glb.accessor(target[semantic]), expected, name + f"/morph/{frame['frame_index']}/{semantic}"
                    )
                checked_morphs += 1
            report_frames = sidecar["glb"]["meshes"][source_mesh_index]["morph_frames"]
            for frame in mesh["frames"]:
                report_frame = next(row for row in report_frames if row["frame"] == frame["frame_index"])
                positions, normals, tangents = (
                    source_array(frame[field]) for field in ("position", "normal", "tangent")
                )
                records = b"".join(
                    struct.pack("<I", index)
                    + positions[row].tobytes()
                    + normals[row].tobytes()
                    + tangents[row].tobytes()
                    for row, index in enumerate(frame["indices"])
                )
                view_index = report_frame["source_records_buffer_view"]
                actual_records = b"" if view_index is None else glb.view(view_index)
                if actual_records != records:
                    raise AssertionError("morph源稀疏记录原顺序未完整保留")
                if (
                    np.float32(report_frame["source_full_weight"]).tobytes()
                    != np.float32(mesh["full_weights"][frame["frame_index"]]).tobytes()
                ):
                    raise AssertionError("morph fullWeight未保留")
    return {
        "valid": True,
        "glb_sha256": digest(glb_bytes),
        "glb_bytes": len(glb_bytes),
        "nodes": len(expected_nodes),
        "renderers": len(expected_renderers),
        "meshes": len(expected_meshes),
        "vertices": sum(mesh["vertices"] for mesh in expected_meshes.values()),
        "attribute_arrays_compared": checked_arrays,
        "morph_targets_compared": checked_morphs,
        "skin_vertices_compared": checked_skin_vertices,
        "independent_parser": "pygltflib 1.16.5 + separate byte accessor decoder",
        "numeric_contract": "source UnityPy float32 bytes including sign of zero",
        "material_properties_compared": material_properties_compared,
    }


def run_cli(executable, arguments, success=True):
    started = time.perf_counter()
    process = subprocess.run(
        [str(executable), *map(str, arguments)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=180,
    )
    result = {
        "arguments": list(map(str, arguments)),
        "returncode": process.returncode,
        "wall_seconds": time.perf_counter() - started,
        "stdout": process.stdout,
        "stderr": process.stderr,
    }
    if (process.returncode == 0) != success:
        raise AssertionError("真实CLI返回码不符：" + json.dumps(result, ensure_ascii=False))
    return result


def model_arguments(args, case, output):
    return [
        "model",
        "--input",
        args.shared / case["closure"]["files"][0]["path"],
        "--catalog",
        args.shared / "archive/game/catalog.json",
        "--output",
        output,
        "--memory-budget",
        "256MiB",
        "--output-budget",
        "64MiB",
    ]


def check(args):
    suite = json.loads(read(args.oracle))
    args.work.mkdir(parents=True, exist_ok=True)
    cases = suite["cases"]
    results = []
    for case in cases:
        with tempfile.TemporaryDirectory(prefix="model-readback-", dir=args.work) as temporary:
            output = Path(temporary) / "output"
            commands = []
            try:
                commands.append(run_cli(args.executable, model_arguments(args, case, output)))
                validation = validate_model(case, output)
                commands.append(
                    run_cli(args.executable, ["verify", "--manifest", output / "manifest.jsonl", "--level", "files"])
                )
                commands.append(
                    run_cli(
                        args.executable,
                        ["verify", "--manifest", output / "manifest.jsonl", "--level", "decoded"],
                        False,
                    )
                )
                # 既有成果默认拒绝覆盖，拒绝后文件字节不能变化
                before = {
                    name: digest(read(output / name)) for name in ("model.glb", "model.source.json", "manifest.jsonl")
                }
                commands.append(run_cli(args.executable, model_arguments(args, case, output), False))
                after = {name: digest(read(output / name)) for name in before}
                if before != after:
                    raise AssertionError("拒绝覆盖仍改写已有输出")
                results.append(
                    {
                        "case": case["root"] + "/independent_readback",
                        "passed": True,
                        "validation": validation,
                        "overwrite_rejected_without_change": True,
                        "commands": commands,
                    }
                )
            except Exception as error:
                results.append(
                    {
                        "case": case["root"] + "/independent_readback",
                        "passed": False,
                        "error": str(error),
                        "traceback": traceback.format_exc(),
                        "commands": commands,
                    }
                )
        print(
            json.dumps({"case": results[-1]["case"], "passed": results[-1]["passed"]}, ensure_ascii=False), flush=True
        )
    for label, edits in (
        ("memory_admission_rejection", {"--memory-budget": "1"}),
        ("output_quota_rollback", {"--output-budget": "1"}),
        ("wrong_root_rejection", {"--root-path-id": "123456789"}),
    ):
        with tempfile.TemporaryDirectory(prefix="model-rejection-", dir=args.work) as temporary:
            output = Path(temporary) / "output"
            command = model_arguments(args, cases[0], output)
            for option, value in edits.items():
                if option in command:
                    command[command.index(option) + 1] = value
                else:
                    command.extend([option, value])
            try:
                result = run_cli(args.executable, command, False)
                leftovers = (
                    [str(path.relative_to(output)) for path in output.rglob("*") if path.is_file()]
                    if output.exists()
                    else []
                )
                if leftovers:
                    raise AssertionError("失败后遗留输出：" + str(leftovers))
                results.append({"case": label, "passed": True, "commands": [result], "remaining_files": 0})
            except Exception as error:
                results.append({"case": label, "passed": False, "error": str(error)})
        print(json.dumps({"case": label, "passed": results[-1]["passed"]}, ensure_ascii=False), flush=True)
    with tempfile.TemporaryDirectory(prefix="model-missing-dependency-", dir=args.work) as temporary:
        output = Path(temporary) / "output"
        try:
            result = run_cli(
                args.executable,
                [
                    "model",
                    "--input",
                    args.shared / cases[0]["closure"]["files"][0]["path"],
                    "--output",
                    output,
                    "--memory-budget",
                    "256MiB",
                    "--output-budget",
                    "64MiB",
                ],
                False,
            )
            if output.exists() and any(path.is_file() for path in output.rglob("*")):
                raise AssertionError("缺失依赖后遗留输出")
            results.append({"case": "missing_dependency_rejection", "passed": True, "commands": [result]})
        except Exception as error:
            results.append({"case": "missing_dependency_rejection", "passed": False, "error": str(error)})
    with tempfile.TemporaryDirectory(prefix="model-explicit-root-", dir=args.work) as temporary:
        output = Path(temporary) / "output"
        try:
            case = cases[0]
            root = next(node for node in case["nodes"] if not node["parent_transform"])
            result = run_cli(
                args.executable,
                [*model_arguments(args, case, output), "--root-path-id", root["identity"].rsplit(":", 1)[1]],
            )
            validation = validate_model(case, output, root["identity"])
            results.append(
                {"case": "explicit_root_selection", "passed": True, "commands": [result], "validation": validation}
            )
        except Exception as error:
            results.append(
                {
                    "case": "explicit_root_selection",
                    "passed": False,
                    "error": str(error),
                    "traceback": traceback.format_exc(),
                }
            )
    with tempfile.TemporaryDirectory(prefix="model-tamper-", dir=args.work) as temporary:
        output = Path(temporary) / "output"
        try:
            result = run_cli(args.executable, model_arguments(args, cases[1], output))
            content = bytearray(read(output / "model.glb"))
            glb = Glb(content)
            position = glb.tree["accessors"][glb.tree["meshes"][0]["primitives"][0]["attributes"]["POSITION"]]
            view = glb.tree["bufferViews"][position["bufferView"]]
            json_length = struct.unpack_from("<I", content, 12)[0]
            byte_offset = 28 + json_length + view.get("byteOffset", 0) + position.get("byteOffset", 0)
            content[byte_offset] ^= 1
            (output / "model.glb").write_bytes(content)
            rejected = run_cli(
                args.executable, ["verify", "--manifest", output / "manifest.jsonl", "--level", "files"], False
            )
            independently_rejected = False
            try:
                validate_model(cases[1], output)
            except AssertionError:
                independently_rejected = True
            if not independently_rejected:
                raise AssertionError("独立回读没有发现单bit顶点篡改")
            results.append(
                {
                    "case": "tampered_vertex_rejection",
                    "passed": True,
                    "commands": [result, rejected],
                    "independent_rejection": True,
                }
            )
        except Exception as error:
            results.append(
                {
                    "case": "tampered_vertex_rejection",
                    "passed": False,
                    "error": str(error),
                    "traceback": traceback.format_exc(),
                }
            )
    report = {
        "schema": 1,
        "executable_sha256": digest(read(args.executable)),
        "oracle_sha256": digest(read(args.oracle)),
        "cases": results,
        "passed": sum(row["passed"] for row in results),
        "total": len(results),
        "remaining_export_files": [
            str(path) for path in args.work.rglob("*") if path.is_file() and path.suffix in {".glb", ".part"}
        ],
    }
    write_json(args.output, report)
    print(
        json.dumps(
            {
                "passed": report["passed"],
                "total": report["total"],
                "remaining_export_files": report["remaining_export_files"],
            },
            ensure_ascii=False,
        )
    )
    return int(report["passed"] != report["total"])


def benchmark(args):
    definition = importlib.util.spec_from_file_location("model_measure", args.benchmark_script)
    module = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(module)
    suite = json.loads(read(args.oracle))
    args.work.mkdir(parents=True, exist_ok=True)
    rows = []
    for case in suite["cases"]:
        for repeat in range(args.repeats):
            with tempfile.TemporaryDirectory(prefix="model-benchmark-", dir=args.work) as temporary:
                root = Path(temporary)
                output = root / "output"
                command = [str(args.executable), *map(str, model_arguments(args, case, output))]
                metrics = module.measure_process(command, 180, root)
                if metrics["returncode"] != 0:
                    raise AssertionError("模型基准命令失败：" + metrics["stderr"])
                validation = validate_model(case, output)
                rows.append(
                    {
                        "case": case["root"],
                        "repeat": repeat + 1,
                        "command": command,
                        **metrics,
                        "validation": validation,
                    }
                )
            print(
                json.dumps(
                    {
                        "case": case["root"],
                        "repeat": repeat + 1,
                        "seconds": metrics["wall_seconds"],
                        "peak_mib": metrics["peak_working_set_bytes"] / 1048576,
                    },
                    ensure_ascii=False,
                ),
                flush=True,
            )
    summaries = []
    for case in suite["cases"]:
        selected = [row for row in rows if row["case"] == case["root"]]
        summaries.append(
            {
                "case": case["root"],
                "runs": len(selected),
                "wall_seconds_median": statistics.median(row["wall_seconds"] for row in selected),
                "peak_working_set_bytes_max": max(row["peak_working_set_bytes"] for row in selected),
                "peak_commit_bytes_max": max(row["peak_commit_bytes"] for row in selected),
                "glb_bytes": [row["validation"]["glb_bytes"] for row in selected],
                "all_valid": all(row["validation"]["valid"] for row in selected),
            }
        )
    report = {
        "executable_sha256": digest(read(args.executable)),
        "oracle_sha256": digest(read(args.oracle)),
        "platform": platform.platform(),
        "logical_cpus": os.cpu_count(),
        "runs": rows,
        "summary": summaries,
        "notes": [
            "独立进程端到端导出，Windows内核累计峰值，数值回读在计时外。",
            "未清空系统文件缓存；源数据和GLB真实accessor逐float32位比较。",
            "本机WMI挂起，进程内platform探测使用系统API回退。",
        ],
        "remaining_export_files": [
            str(path) for path in args.work.rglob("*") if path.is_file() and path.suffix in {".glb", ".part"}
        ],
    }
    write_json(args.output, report)
    return 0


def selftest():
    for actual, expected in (
        (np.asarray([-0.0], dtype="<f4"), np.asarray([0.0], dtype="<f4")),
        (np.asarray([1.0], dtype="<f4"), np.asarray([1.0000001192092896], dtype="<f4")),
    ):
        try:
            bits_equal(actual, expected, "独立比较器负向控制")
        except AssertionError:
            continue
        raise AssertionError("逐位比较器没有拒绝符号零或单ULP差异")
    source = np.asarray([[1.0, 2.0, 3.0, -1.0]], dtype="<f4")
    bits_equal(mirror_vector(mirror_vector(source, True), True), source, "镜像转换可逆")
    print("符号零、单ULP与坐标镜像控制通过")
    return 0


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("selftest")
    prepare_parser = commands.add_parser("prepare")
    prepare_parser.add_argument("--shared", type=Path, required=True)
    prepare_parser.add_argument(
        "--dependency-report",
        type=Path,
        required=True,
    )
    prepare_parser.add_argument("--output", type=Path, required=True)
    prepare_parser.add_argument("--root", action="append")
    check_parser = commands.add_parser("check")
    check_parser.add_argument("--oracle", type=Path, required=True)
    check_parser.add_argument("--executable", type=Path, required=True)
    check_parser.add_argument("--shared", type=Path, required=True)
    check_parser.add_argument("--work", type=Path, required=True)
    check_parser.add_argument("--output", type=Path, required=True)
    benchmark_parser = commands.add_parser("bench")
    benchmark_parser.add_argument("--oracle", type=Path, required=True)
    benchmark_parser.add_argument("--executable", type=Path, required=True)
    benchmark_parser.add_argument("--shared", type=Path, required=True)
    benchmark_parser.add_argument("--work", type=Path, required=True)
    benchmark_parser.add_argument("--output", type=Path, required=True)
    benchmark_parser.add_argument("--benchmark-script", type=Path, required=True)
    benchmark_parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    if args.command == "prepare":
        return prepare(args)
    if args.command == "check":
        return check(args)
    if args.command == "bench":
        return benchmark(args)
    if args.command == "selftest":
        return selftest()


if __name__ == "__main__":
    raise SystemExit(main())
