"""专用解包工具的独立进程基准与内容核验。"""

import argparse
import ctypes
import hashlib
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path


class ProcessMemoryCounters(ctypes.Structure):
    _fields_ = [
        ("cb", ctypes.c_uint32),
        ("PageFaultCount", ctypes.c_uint32),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
    ]


class ProcessIoCounters(ctypes.Structure):
    _fields_ = [
        ("ReadOperationCount", ctypes.c_uint64),
        ("WriteOperationCount", ctypes.c_uint64),
        ("OtherOperationCount", ctypes.c_uint64),
        ("ReadTransferCount", ctypes.c_uint64),
        ("WriteTransferCount", ctypes.c_uint64),
        ("OtherTransferCount", ctypes.c_uint64),
    ]


class ProcessFileTime(ctypes.Structure):
    _fields_ = [("low", ctypes.c_uint32), ("high", ctypes.c_uint32)]


def main():
    parser = argparse.ArgumentParser(description="独立进程解包基准与无损验证")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare", help="固定原包和独立内容基准")
    prepare.add_argument("--shared", type=Path, required=True)
    prepare.add_argument("--output", type=Path, required=True)
    run = commands.add_parser("run", help="顺序测量工具并核验输出")
    run.add_argument("--suite", type=Path, required=True)
    run.add_argument("--executable", type=Path, required=True)
    run.add_argument("--unity-rs-executable", type=Path, required=True)
    run.add_argument("--python", type=Path, default=Path(sys.executable))
    run.add_argument("--output", type=Path, required=True)
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--repeats", type=int, default=3)
    run.add_argument("--timeout", type=float, default=180)
    run.add_argument("--case", action="append", help="仅运行指定场景")
    run.add_argument("--tool", action="append", choices=("Rurino", "unitypy", "unity-rs"))
    worker = commands.add_parser("worker", help=argparse.SUPPRESS)
    worker.add_argument("--suite", type=Path, required=True)
    worker.add_argument("--case", required=True)
    worker.add_argument("--output", type=Path)
    commands.add_parser("selftest", help="验证进程峰值与输出检查")
    args = parser.parse_args()
    if args.command == "prepare":
        return prepare_suite(args.shared.resolve(), args.output.resolve())
    if args.command == "run":
        return run_suite(args)
    if args.command == "worker":
        return unitypy_worker(args)
    return selftest()


def prepare_suite(shared, output):
    import importlib.metadata

    import UnityPy

    archive = json.loads((shared / "archive/manifest.json").read_text(encoding="utf-8"))
    names = {
        "card": "image_card_full_10217031.assetbundle",
        "spine": "ingame_chara_sd_spine_1033_025.assetbundle",
        "attribute": "attribute.assetbundle",
        "cube": "__reflectionprobe-0.exr.assetbundle",
        "font": "raid_event_bg_900040.assetbundle",
        "video": "__sc_ppadv734_104finalemovie_tex0.mp4.assetbundle",
        "largest_atlas": "profpartscard.assetbundle",
    }
    inputs = {}
    for name, filename in names.items():
        key = "game/plain/" + filename
        path = shared / "archive" / key
        expected = archive["files"][key]
        if path.stat().st_size != expected["size"] or hash_file(path) != expected["sha256"]:
            raise ValueError(f"基准原包不匹配：{filename}")
        inputs[name] = {"path": str(path), "bytes": expected["size"], "sha256": expected["sha256"]}
    cases = [
        {
            "id": name + "-inspect",
            "input": name,
            "operation": "inspect",
            "tools": list(("Rurino", "unitypy", "unity-rs")),
        }
        for name in ("card", "spine", "attribute", "font", "largest_atlas")
    ]
    cases.extend(
        [
            {"id": "card-png", "input": "card", "operation": "png", "classes": [28], "mips": "0"},
            {
                "id": "card-png-fast",
                "input": "card",
                "operation": "png",
                "classes": [28],
                "mips": "0",
                "profile": "fast",
                "tools": ["Rurino"],
            },
            {"id": "attribute-png", "input": "attribute", "operation": "png", "classes": [28], "mips": "0"},
            {"id": "spine-text-native", "input": "spine", "operation": "native", "classes": [49]},
            {"id": "font-native", "input": "font", "operation": "native", "classes": [128]},
            {"id": "video-native", "input": "video", "operation": "native", "classes": [329]},
            {
                "id": "cube-all-png",
                "input": "cube",
                "operation": "png",
                "classes": [89],
                "mips": "all",
                "tools": ["Rurino", "unitypy"],
            },
            {
                "id": "cube-native",
                "input": "cube",
                "operation": "native",
                "classes": [89],
                "tools": ["Rurino", "unitypy"],
            },
        ]
    )
    for case in cases:
        case.setdefault("tools", ["Rurino", "unitypy", "unity-rs"])
        if case["operation"] == "inspect":
            if case["input"] == "largest_atlas":
                case["expected_objects"] = 3216
                case["expected_basis"] = "独立55包元数据扫描；本次prepare不加载最大图集"
            else:
                case["expected_objects"] = len(UnityPy.load(inputs[case["input"]]["path"]).objects)
            continue
        environment = UnityPy.load(inputs[case["input"]]["path"])
        selected = [obj for obj in environment.objects if obj.type.value in case["classes"]]
        expected = []
        for obj in selected:
            decoded = obj.read()
            if case["operation"] == "png":
                mip_count = max(1, decoded.m_MipCount) if case["mips"] == "all" else 1
                faces = decoded.m_ImageCount if obj.type.value == 89 else 1
                for face in range(faces):
                    for mip in range(mip_count):
                        image = decoded.image.convert("RGBA") if face == mip == 0 else texture_mip(decoded, mip, face)
                        expected.append(
                            {
                                "path_id": str(obj.path_id),
                                "face": face,
                                "mip": mip,
                                "width": image.width,
                                "height": image.height,
                                "rgba_sha256": hashlib.sha256(image.tobytes()).hexdigest(),
                            }
                        )
            else:
                payload = native_payload(decoded, obj.type.value)
                expected.append(
                    {
                        "path_id": str(obj.path_id),
                        "bytes": len(payload),
                        "sha256": hashlib.sha256(payload).hexdigest(),
                    }
                )
        case["expected"] = expected
    alpha_environment = UnityPy.load(inputs["font"]["path"])
    alpha_object = next(
        obj for obj in alpha_environment.objects if obj.type.value == 28 and obj.path_id == -7477645124050682085
    )
    alpha_image = alpha_object.read().image.convert("RGBA")
    cases.append(
        {
            "id": "alpha8-png",
            "input": "font",
            "operation": "png",
            "classes": [28],
            "path_ids": [str(alpha_object.path_id)],
            "mips": "0",
            "tools": ["Rurino", "unitypy"],
            "expected": [
                {
                    "path_id": str(alpha_object.path_id),
                    "face": 0,
                    "mip": 0,
                    "width": alpha_image.width,
                    "height": alpha_image.height,
                    "rgba_sha256": hashlib.sha256(alpha_image.tobytes()).hexdigest(),
                }
            ],
            "semantic_note": "Alpha8原生只存alpha；本工具派生RGB选择0，与UnityPy一致；"
            "官方Rust填255，不能混为同一RGBA。",
        }
    )
    suite = {
        "schema": 1,
        "inputs": inputs,
        "cases": cases,
        "oracle": {"UnityPy": importlib.metadata.version("UnityPy"), "Pillow": importlib.metadata.version("Pillow")},
        "notes": [
            "输入SHA和独立oracle在计时外生成；每个case使用独立进程，运行期间不刷新系统磁盘缓存。",
            "inspect比较容器加载和对象计数；工具内部可能执行不同的名称和依赖索引工作。",
            "最大图集只运行inspect，禁止批量像素输出；其对象数依据前期独立元数据扫描。",
            "Cubemap六面和全部mip使用明确ASTC块布局oracle；官方unity-rs 0.6.1不支持该语义，未混入raw回退。",
            "每次运行完成后先核验输出，再清理该次临时目录；不会保留整套图片。",
        ],
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    write_json(output, suite)
    print(json.dumps({"suite": str(output), "cases": len(cases), "inputs": len(inputs)}, ensure_ascii=False))
    return 0


def run_suite(args):
    if os.name != "nt":
        raise RuntimeError("本基准使用Windows内核峰值指标，请在Windows执行")
    if args.repeats < 1 or args.timeout <= 0:
        raise ValueError("重复次数与超时必须为正数")
    suite = json.loads(args.suite.read_text(encoding="utf-8"))
    cases = [case for case in suite["cases"] if args.case is None or case["id"] in args.case]
    if args.case and set(args.case) != {case["id"] for case in cases}:
        raise ValueError("存在未知case")
    for source in suite["inputs"].values():
        if hash_file(Path(source["path"])) != source["sha256"]:
            raise ValueError("运行前输入SHA不符")
    args.work.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.worker_python = resolve_python(args.python)
    rows = []
    journal = args.output.with_suffix(".jsonl")
    with journal.open("w", encoding="utf-8", newline="\n") as log:
        for case in cases:
            for tool in case["tools"]:
                if args.tool and tool not in args.tool:
                    continue
                for repeat in range(args.repeats):
                    with tempfile.TemporaryDirectory(prefix="unpack-bench-", dir=args.work) as directory:
                        target = Path(directory)
                        command = tool_command(args, suite, case, tool, target)
                        environment = os.environ.copy()
                        if tool == "unitypy":
                            environment["PYTHONPATH"] = args.worker_python["site_packages"]
                            environment["PYTHONNOUSERSITE"] = "1"
                        metrics = measure_process(command, args.timeout, target, environment)
                        validation = (
                            validate_result(case, target, metrics["stdout"], tool)
                            if metrics["returncode"] == 0
                            else {"valid": False, "reason": "子进程未成功完成"}
                        )
                        row = {
                            "case": case["id"],
                            "operation": case["operation"],
                            "tool": tool,
                            "repeat": repeat + 1,
                            "command": command,
                            "input_bytes": suite["inputs"][case["input"]]["bytes"],
                            **metrics,
                            "observed_limit": (
                                "AssetBundle preload table 超过官方CLI默认元数据项上限"
                                if tool == "unity-rs"
                                and case["id"] == "largest_atlas-inspect"
                                and "exceeding limit" in metrics["stderr"]
                                else None
                            ),
                            "validation": validation,
                        }
                        rows.append(row)
                        log.write(json.dumps(row, ensure_ascii=False) + "\n")
                        log.flush()
                        print(
                            f"{case['id']} {tool} {repeat + 1}/{args.repeats}："
                            f"{metrics['wall_seconds']:.3f}s，"
                            f"峰值 {metrics['peak_working_set_bytes'] / 1048576:.2f}MiB，"
                            f"校验 {'通过' if validation['valid'] else '失败'}",
                            flush=True,
                        )
    summary = []
    for case in cases:
        for tool in case["tools"]:
            selected = [row for row in rows if row["case"] == case["id"] and row["tool"] == tool]
            if not selected:
                continue
            summary.append(
                {
                    "case": case["id"],
                    "tool": tool,
                    "runs": len(selected),
                    "all_valid": all(row["validation"]["valid"] for row in selected),
                    "wall_seconds_median": statistics.median(row["wall_seconds"] for row in selected),
                    "wall_seconds_min": min(row["wall_seconds"] for row in selected),
                    "peak_working_set_bytes_max": max(row["peak_working_set_bytes"] for row in selected),
                    "peak_commit_bytes_max": max(row["peak_commit_bytes"] for row in selected),
                    "output_bytes": [row["validation"].get("output_bytes", 0) for row in selected],
                }
            )
    report = {
        "schema": 1,
        "suite": str(args.suite.resolve()),
        "suite_sha256": hash_file(args.suite),
        "machine": {"platform": platform.platform(), "python": sys.version, "logical_cpus": os.cpu_count()},
        "executables": {
            "Rurino": {"path": str(args.executable), "sha256": hash_file(args.executable)},
            "unity-rs": {"path": str(args.unity_rs_executable), "sha256": hash_file(args.unity_rs_executable)},
            "python": {"path": str(args.python), "sha256": hash_file(args.python)},
        },
        "memory_metrics": {
            "peak_working_set": "Windows GetProcessMemoryInfo.PeakWorkingSetSize，内核累计峰值",
            "peak_commit": "Windows GetProcessMemoryInfo.PeakPagefileUsage，进程提交峰值，不是实际磁盘pagefile读写量",
            "private_usage_sampled_max": "约10ms采样的PrivateUsage最大值，仅作附加指标",
            "process_scope": "各工具单个直接子进程；case工具不再启动子进程",
            "python_launcher": args.worker_python,
        },
        "cache_policy": "未清空操作系统文件缓存；各case顺序执行，报告重复次数与中位数",
        "comparison_limits": [
            "不同工具加载索引与名称元数据的工作量不同，inspect墙钟和读取量分别报告。",
            "失败或能力限制的case不作为成功吞吐对比；其耗时和峰值仅记录失败前的消耗。",
            "Python的进程启动与包导入包含在端到端计时中，不直接解释为纯解码器性能。",
            "PNG standard使用压缩级别6对应default；fast为独立产品配置，不能与不同压缩比混合排名。",
        ],
        "summary": summary,
        "runs": rows,
        "suite_notes": suite["notes"],
    }
    write_json(args.output, report)
    return int(not all(row["validation"]["valid"] for row in rows))


def tool_command(args, suite, case, tool, output):
    source = suite["inputs"][case["input"]]["path"]
    if tool == "unitypy":
        command = [
            args.worker_python["executable"],
            str(Path(__file__).resolve()),
            "worker",
            "--suite",
            str(args.suite),
            "--case",
            case["id"],
        ]
        if case["operation"] != "inspect":
            command.extend(["--output", str(output)])
        return command
    if tool == "unity-rs":
        if case["operation"] == "inspect":
            return [str(args.unity_rs_executable), "info", source]
        command = [str(args.unity_rs_executable), "export", source, str(output), "--filename", "asset-name-path-id"]
        for class_id in case["classes"]:
            command.extend(["--class", str(class_id)])
        if case["operation"] == "png":
            command.extend(["--image-format", "png", "--png-compression", "default", "--png-filter", "auto"])
        return command
    if case["operation"] == "inspect":
        return [str(args.executable), "inspect", "--input", source]
    command = [
        str(args.executable),
        "extract",
        "--input",
        source,
        "--output",
        str(output),
        "--format",
        case["operation"],
        "--memory-budget",
        "268435456",
        "--output-budget",
        "67108864",
    ]
    for class_id in case["classes"]:
        command.extend(["--class", str(class_id)])
    for path_id in case.get("path_ids", []):
        command.extend(["--path-id", path_id])
    if case["operation"] == "png":
        command.extend(["--mips", case["mips"], "--profile", case.get("profile", "default")])
    return command


def unitypy_worker(args):
    import UnityPy

    suite = json.loads(args.suite.read_text(encoding="utf-8"))
    case = next(case for case in suite["cases"] if case["id"] == args.case)
    environment = UnityPy.load(suite["inputs"][case["input"]]["path"])
    if case["operation"] == "inspect":
        print(
            json.dumps(
                {
                    "event": "summary",
                    "objects": len(environment.objects),
                    "files": len({id(obj.assets_file) for obj in environment.objects}),
                }
            )
        )
        return 0
    args.output.mkdir(parents=True, exist_ok=True)
    count = 0
    for obj in environment.objects:
        if obj.type.value not in case["classes"]:
            continue
        if case.get("path_ids") and str(obj.path_id) not in case["path_ids"]:
            continue
        decoded = obj.read()
        if case["operation"] == "png":
            mip_count = max(1, decoded.m_MipCount) if case["mips"] == "all" else 1
            faces = decoded.m_ImageCount if obj.type.value == 89 else 1
            for face in range(faces):
                for mip in range(mip_count):
                    image = decoded.image.convert("RGBA") if face == mip == 0 else texture_mip(decoded, mip, face)
                    image.save(args.output / f"{obj.path_id}-face{face}-mip{mip}.png", format="PNG", compress_level=6)
                    count += 1
        else:
            payload = native_payload(decoded, obj.type.value)
            (args.output / f"{obj.path_id}.bin").write_bytes(payload)
            count += 1
    print(json.dumps({"event": "summary", "outputs": count}))
    return 0


def native_payload(decoded, class_id):
    if class_id in (28, 89):
        return decoded.get_image_data()
    if class_id == 49:
        return decoded.m_Script.encode("utf-8", errors="surrogateescape")
    if class_id == 128:
        return bytes(decoded.m_FontData)
    if class_id == 329:
        from UnityPy.helpers.ResourceReader import get_resource_data

        source = decoded.m_ExternalResources
        return get_resource_data(source.m_Source, decoded.object_reader.assets_file, source.m_Offset, source.m_Size)
    raise ValueError(f"没有该类的原生载荷基准：{class_id}")


def texture_mip(texture, mip, face):
    from UnityPy.export import Texture2DConverter

    block_sizes = {50: 6, 52: 8, 47: 4}
    if texture.m_TextureFormat not in block_sizes:
        raise ValueError("手工mip oracle只覆盖已实测的ASTC和ETC2格式")
    block = block_sizes[texture.m_TextureFormat]
    width = texture.m_Width
    height = texture.m_Height
    offset = face * texture.m_CompleteImageSize
    for _ in range(mip):
        offset += ((width + block - 1) // block) * ((height + block - 1) // block) * 16
        width = max(1, width // 2)
        height = max(1, height // 2)
    length = ((width + block - 1) // block) * ((height + block - 1) // block) * 16
    return Texture2DConverter.parse_image_data(
        texture.get_image_data()[offset : offset + length],
        width,
        height,
        texture.m_TextureFormat,
        texture.object_reader.version,
        texture.object_reader.platform,
        texture.m_PlatformBlob,
        True,
    ).convert("RGBA")


def validate_result(case, output, stdout, tool):
    if case["operation"] == "inspect":
        lines = []
        for line in stdout.splitlines():
            if not line.lstrip().startswith("{"):
                continue
            lines.append(json.loads(line))
        counts = [row["objects"] for row in lines if "objects" in row]
        counts.extend(row["stats"]["objects"] for row in lines if "stats" in row and "objects" in row["stats"])
        valid = case["expected_objects"] in counts
        if tool == "unity-rs":
            counts = [int(value) for value in re.findall(r"^\s*objects:\s*(\d+)\s*$", stdout, re.MULTILINE)]
            valid = counts == [case["expected_objects"]]
        return {"valid": valid, "expected_objects": case["expected_objects"], "reported_counts": counts}
    files = [
        path
        for path in output.rglob("*")
        if path.is_file()
        and path.name not in {"stdout.log", "stderr.log", "manifest.jsonl"}
        and path.suffix.lower() not in {".json", ".jsonl"}
    ]
    manifest = output / "manifest.jsonl"
    if tool == "Rurino" and manifest.exists():
        records = [json.loads(line) for line in manifest.read_text(encoding="utf-8").splitlines() if line.strip()]
        recorded = []
        for row in records:
            outcome = row.get("outcome", row)
            for written in outcome.get("outputs", []):
                saved = written.get("output", written)
                path = output / saved["path"]
                if not path.is_file() or path.stat().st_size != saved["bytes"] or hash_file(path) != saved["sha256"]:
                    return {"valid": False, "reason": "清单输出文件或哈希不符", "path": str(path)}
                recorded.append(path)
        if recorded:
            files = recorded
    if case["operation"] == "png":
        from PIL import Image

        files = [path for path in files if path.suffix.lower() == ".png"]
        actual = []
        for path in files:
            with Image.open(path) as image:
                rgba = image.convert("RGBA")
                actual.append((rgba.width, rgba.height, hashlib.sha256(rgba.tobytes()).hexdigest()))
        expected = [(row["width"], row["height"], row["rgba_sha256"]) for row in case["expected"]]
    else:
        actual = [(path.stat().st_size, hash_file(path)) for path in files]
        expected = [(row["bytes"], row["sha256"]) for row in case["expected"]]
    return {
        "valid": Counter(actual) == Counter(expected),
        "expected_outputs": len(expected),
        "actual_outputs": len(actual),
        "output_bytes": sum(path.stat().st_size for path in files),
        "mismatch_expected": list((Counter(expected) - Counter(actual)).elements())[:8],
        "mismatch_actual": list((Counter(actual) - Counter(expected)).elements())[:8],
    }


def measure_process(command, timeout, directory, environment=None):
    if os.name != "nt":
        raise RuntimeError("需要Windows进程计数器")
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi.GetProcessMemoryInfo.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(ProcessMemoryCounters),
        ctypes.c_uint32,
    ]
    psapi.GetProcessMemoryInfo.restype = ctypes.c_int
    kernel.GetProcessIoCounters.argtypes = [ctypes.c_void_p, ctypes.POINTER(ProcessIoCounters)]
    kernel.GetProcessIoCounters.restype = ctypes.c_int
    kernel.GetProcessTimes.argtypes = [ctypes.c_void_p, *([ctypes.POINTER(ProcessFileTime)] * 4)]
    kernel.GetProcessTimes.restype = ctypes.c_int
    kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    kernel.WaitForSingleObject.restype = ctypes.c_uint32
    peak_working = 0
    peak_commit = 0
    private_sampled = 0
    io_counts = ProcessIoCounters()
    created = ProcessFileTime()
    exited = ProcessFileTime()
    system_time = ProcessFileTime()
    user_time = ProcessFileTime()
    started = time.perf_counter()
    with (directory / "stdout.log").open("wb") as stdout, (directory / "stderr.log").open("wb") as stderr:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment)
        timed_out = False
        handle = ctypes.c_void_p(int(process._handle))
        while True:
            counters = ProcessMemoryCounters()
            counters.cb = ctypes.sizeof(counters)
            if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                peak_working = max(peak_working, counters.PeakWorkingSetSize)
                peak_commit = max(peak_commit, counters.PeakPagefileUsage)
                private_sampled = max(private_sampled, counters.PrivateUsage)
            if process.poll() is not None:
                break
            if time.perf_counter() - started > timeout:
                process.kill()
                timed_out = True
            if kernel.WaitForSingleObject(handle, 10) == 0xFFFFFFFF:
                raise ctypes.WinError(ctypes.get_last_error())
        wall = time.perf_counter() - started
        if not kernel.GetProcessIoCounters(handle, ctypes.byref(io_counts)):
            raise ctypes.WinError(ctypes.get_last_error())
        if not kernel.GetProcessTimes(
            handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(system_time), ctypes.byref(user_time)
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        process.wait()
    if peak_working == 0 or peak_commit == 0:
        raise RuntimeError("未取得有效的Windows内核峰值")
    return {
        "returncode": process.returncode,
        "timed_out": timed_out,
        "wall_seconds": wall,
        "kernel_process_seconds": (filetime_value(exited) - filetime_value(created)) / 10_000_000,
        "cpu_seconds": (filetime_value(system_time) + filetime_value(user_time)) / 10_000_000,
        "peak_working_set_bytes": peak_working,
        "peak_commit_bytes": peak_commit,
        "private_usage_sampled_max_bytes": private_sampled,
        "read_transfer_bytes": io_counts.ReadTransferCount,
        "write_transfer_bytes": io_counts.WriteTransferCount,
        "read_operations": io_counts.ReadOperationCount,
        "write_operations": io_counts.WriteOperationCount,
        "stdout": (directory / "stdout.log").read_text(encoding="utf-8", errors="replace")[-100000:],
        "stderr": (directory / "stderr.log").read_text(encoding="utf-8", errors="replace")[-12000:],
    }


def filetime_value(value):
    return (value.high << 32) | value.low


def resolve_python(executable):
    result = subprocess.run(
        [
            str(executable),
            "-c",
            "import json,sys,sysconfig; "
            "print(json.dumps({'executable':sys._base_executable,'site_packages':sysconfig.get_path('purelib'),"
            "'version':sys.version}))",
        ],
        capture_output=True,
        check=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(result.stdout)


def hash_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def selftest():
    if os.name != "nt":
        raise RuntimeError("峰值控制测试仅适用于Windows")
    with tempfile.TemporaryDirectory(prefix="unpack-bench-control-") as folder:
        metrics = measure_process(
            [
                resolve_python(Path(sys.executable))["executable"],
                "-c",
                "import time; value=bytearray(96*1024*1024); "
                "value[::4096]=b'x'*(len(value)//4096); del value; time.sleep(0.05)",
            ],
            10,
            Path(folder),
        )
        if metrics["returncode"] != 0 or metrics["peak_working_set_bytes"] < 96 * 1024 * 1024:
            raise AssertionError("未捕捉短时分配峰值")
        from PIL import Image

        image = Image.new("RGBA", (2, 2), (10, 20, 30, 0))
        image.save(Path(folder) / "sample.png")
        expected = {
            "operation": "png",
            "expected": [{"width": 2, "height": 2, "rgba_sha256": hashlib.sha256(image.tobytes()).hexdigest()}],
        }
        if not validate_result(expected, Path(folder), "", "Rurino")["valid"]:
            raise AssertionError("透明像素RGB一致性检查失败")
        image.putpixel((0, 0), (11, 20, 30, 0))
        image.save(Path(folder) / "sample.png")
        if validate_result(expected, Path(folder), "", "Rurino")["valid"]:
            raise AssertionError("未识别透明像素RGB变化")
    print("Windows内核峰值和完整RGBA校验控制测试通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
