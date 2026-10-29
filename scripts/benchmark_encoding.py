"""无损编码优化的真实命令行回归与性能测量。"""

import argparse
import hashlib
import importlib.util
import io
import json
import os
import platform
import statistics
import subprocess
import tempfile
import time
import traceback
from collections import Counter
from pathlib import Path

from PIL import Image

if os.name == "nt":
    # 本机WMI查询挂起，使用platform的系统API回退
    platform._wmi = None

BASH = "C:/Program Files/Git/bin/bash.exe"
STRATEGIES = (
    ("baseline", "png", "none"),
    ("png-balanced", "png", "balanced"),
    ("png-small", "png", "small"),
    ("webp", "webp", "none"),
    ("auto-balanced", "auto", "balanced"),
    ("auto-small", "auto", "small"),
    ("png-fast", "png", "none"),
)


def main():
    parser = argparse.ArgumentParser(description="无损体积优化的内容回归和基准")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare", help="固定原包和独立像素oracle")
    prepare.add_argument("--shared", type=Path, required=True)
    prepare.add_argument("--output", type=Path, required=True)
    for name in ("check", "bench", "compare"):
        run = commands.add_parser(name, help="真实回归" if name == "check" else "独立进程性能基准")
        run.add_argument("--suite", type=Path, required=True)
        run.add_argument("--executable", type=Path, required=True)
        run.add_argument("--work", type=Path, required=True)
        run.add_argument("--output", type=Path, required=True)
        run.add_argument("--optimize-seconds", type=int, default=5)
        run.add_argument("--timeout", type=float, default=180)
        run.add_argument("--case", action="append")
        run.add_argument("--strategy", action="append")
        if name in {"bench", "compare"}:
            run.add_argument("--benchmark-script", type=Path, required=True)
            run.add_argument("--repeats", type=int, default=3)
        if name == "compare":
            run.add_argument("--python", type=Path, required=True)
            run.add_argument("--largest-suite", type=Path, required=True)
    commands.add_parser("selftest", help="核验透明RGB、文件名和尺寸检查")
    args = parser.parse_args()
    if args.command == "prepare":
        return prepare_suite(args)
    if args.command == "check":
        return check_suite(args)
    if args.command == "bench":
        return benchmark_suite(args)
    if args.command == "compare":
        return comparison_suite(args)
    return selftest()


def read_bytes(path):
    return subprocess.run(
        [BASH, "-lc", 'cat -- "$(cygpath -u "$1")"', "reader", str(path)],
        capture_output=True,
        check=True,
    ).stdout


def read_json(path):
    return json.loads(read_bytes(path).decode("utf-8-sig"))


def read_rows(path):
    return [json.loads(line) for line in read_bytes(path).decode("utf-8-sig").splitlines() if line.strip()]


def prepare_suite(args):
    import importlib.metadata

    import UnityPy

    shared = args.shared.resolve()
    archive = read_json(shared / "archive/manifest.json")
    sources = (
        ("card", "image_card_full_10217031.assetbundle", None),
        ("sticker", "image_sticker_20360121.assetbundle", None),
        ("attribute", "attribute.assetbundle", None),
        ("alpha8", "raid_event_bg_900040.assetbundle", [-7477645124050682085]),
        ("cube", "__reflectionprobe-0.exr.assetbundle", None),
    )
    cases = []
    for name, filename, selected_ids in sources:
        relative = "game/plain/" + filename
        path = shared / "archive" / relative
        raw = read_bytes(path)
        expected_source = archive["files"][relative]
        if len(raw) != expected_source["size"] or digest(raw) != expected_source["sha256"]:
            raise ValueError(f"原包大小或SHA不符：{filename}")
        environment = UnityPy.load(raw)
        expected = []
        for obj in environment.objects:
            if obj.type.value != (89 if name == "cube" else 28):
                continue
            if selected_ids is not None and obj.path_id not in selected_ids:
                continue
            texture = obj.read()
            if texture.m_Width == 0 or texture.m_Height == 0:
                continue
            faces = 6 if name == "cube" else 1
            levels = texture.m_MipCount if name == "cube" else 1
            for face in range(faces):
                for mip in range(levels):
                    image = (
                        texture.image.convert("RGBA") if face == mip == 0 else texture_subresource(texture, face, mip)
                    )
                    expected.append(
                        {
                            "path_id": str(obj.path_id),
                            "face": face,
                            "mip": mip,
                            "size": list(image.size),
                            "rgba_sha256": digest(image.tobytes()),
                            "alpha_range": list(image.getchannel("A").getextrema()),
                            "zero_alpha_pixels": image.getchannel("A").histogram()[0],
                        }
                    )
        if not expected:
            raise ValueError(f"样本没有目标纹理：{filename}")
        cases.append(
            {
                "id": name,
                "source": str(path),
                "source_bytes": len(raw),
                "source_sha256": digest(raw),
                "class_id": 89 if name == "cube" else 28,
                "path_ids": list(map(str, selected_ids or [])),
                "mips": "all" if name == "cube" else "0",
                "expected": expected,
            }
        )
    suite = {
        "schema": 1,
        "oracle": {"UnityPy": importlib.metadata.version("UnityPy"), "Pillow": importlib.metadata.version("Pillow")},
        "cases": cases,
        "notes": [
            "固定原包SHA、尺寸和完整RGBA；透明像素RGB也参与哈希。",
            "独立oracle在计时外产生；不会清理或改写原包。",
            "Cubemap六面和全部mip按已核验ASTC布局取值。",
            "所有候选输出都在任务临时目录，验证后清理。",
        ],
    }
    write_json(args.output, suite)
    print(
        json.dumps(
            {"cases": len(cases), "expected_images": sum(len(case["expected"]) for case in cases)}, ensure_ascii=False
        )
    )
    return 0


def texture_subresource(texture, face, mip):
    from UnityPy.export import Texture2DConverter

    if texture.m_TextureFormat != 50:
        raise ValueError("当前Cube oracle仅支持固定ASTC6x6样本")
    payload = texture.get_image_data()
    offset = face * texture.m_CompleteImageSize
    for level in range(mip):
        width = max(1, texture.m_Width >> level)
        height = max(1, texture.m_Height >> level)
        offset += ((width + 5) // 6) * ((height + 5) // 6) * 16
    width = max(1, texture.m_Width >> mip)
    height = max(1, texture.m_Height >> mip)
    length = ((width + 5) // 6) * ((height + 5) // 6) * 16
    return Texture2DConverter.parse_image_data(
        payload[offset : offset + length],
        width,
        height,
        texture.m_TextureFormat,
        texture.object_reader.version,
        texture.object_reader.platform,
        getattr(texture, "m_PlatformBlob", None),
    ).convert("RGBA")


def selected_cases(suite, requested):
    cases = [case for case in suite["cases"] if requested is None or case["id"] in requested]
    if requested and set(requested) != {case["id"] for case in cases}:
        raise ValueError("存在未知样本")
    for case in cases:
        if digest(read_bytes(case["source"])) != case["source_sha256"]:
            raise ValueError("运行前原包SHA与固定清单不符")
    return cases


def selected_strategies(requested, performance=False):
    allowed = STRATEGIES
    if performance and requested is None:
        allowed = tuple(
            row for row in STRATEGIES if row[0] in {"baseline", "png-fast", "png-balanced", "png-small", "auto-small"}
        )
    if not performance and requested is None:
        allowed = tuple(row for row in STRATEGIES if row[0] != "png-fast")
    chosen = [row for row in allowed if requested is None or row[0] in requested]
    if requested and set(requested) != {row[0] for row in chosen}:
        raise ValueError("存在未知编码配置")
    return chosen


def command_for(args, case, strategy, output):
    command = [
        str(args.executable),
        "extract",
        "--input",
        case["source"],
        "--output",
        str(output),
        "--format",
        strategy[1],
        "--optimize",
        strategy[2],
        "--optimize-seconds",
        str(args.optimize_seconds),
        "--class",
        str(case["class_id"]),
        "--profile",
        "fast" if strategy[0] == "png-fast" else "default",
        "--mips",
        case["mips"],
        "--memory-budget",
        "256MiB",
        "--output-budget",
        "64MiB",
        "--jobs",
        "1",
    ]
    for path_id in case["path_ids"]:
        command.extend(["--path-id", path_id])
    return command


def run_process(command, timeout):
    started = time.perf_counter()
    process = subprocess.run(
        command, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout
    )
    return {
        "command": command,
        "returncode": process.returncode,
        "wall_seconds": time.perf_counter() - started,
        "stdout": process.stdout[-100000:],
        "stderr": process.stderr[-12000:],
    }


def validate_outputs(case, root, strategy=None):
    rows = read_rows(root / "manifest.jsonl")
    active = {}
    for row in rows:
        active[(row["input_sha256"], row["entry"], row["path_id"], row["policy_key"])] = row
    expected = {
        (row["path_id"], row["face"], row["mip"]): (row["size"], row["rgba_sha256"]) for row in case["expected"]
    }
    images = []
    observed = set()
    formats = Counter()
    modes = Counter()
    for row in active.values():
        if row["status"] != "decoded_verified":
            raise AssertionError(f"纹理未完成有效解码：{row['status']}，{row.get('metadata')}")
        for entry in row["outputs"]:
            descriptor = entry["descriptor"]
            output = entry["output"]
            key = (row["path_id"], descriptor.get("face") or 0, descriptor.get("mip") or 0)
            if key not in expected or key in observed:
                raise AssertionError(f"输出遗漏、重复或身份不符：{key}")
            path = root / output["path"]
            encoded = read_bytes(path)
            if len(encoded) != output["bytes"] or digest(encoded) != output["sha256"]:
                raise AssertionError("编码文件与清单哈希不符")
            with Image.open(io.BytesIO(encoded)) as decoded:
                image = decoded.convert("RGBA")
                decoded_format = decoded.format
                mode = decoded.mode
            if (list(image.size), digest(image.tobytes())) != expected[key]:
                raise AssertionError(f"完整RGBA与独立oracle不符：{key}")
            if descriptor.get("rgba_sha256") != expected[key][1]:
                raise AssertionError("清单像素描述与独立oracle不符")
            if [descriptor.get("width"), descriptor.get("height")] != expected[key][0]:
                raise AssertionError("清单像素尺寸与独立oracle不符")
            encoding = descriptor.get("encoding")
            if encoding is not None:
                valid = [candidate for candidate in encoding["candidates"] if candidate["status"] == "verified"]
                chosen = [candidate for candidate in valid if candidate["selected"]]
                if len(chosen) != 1 or not all(candidate["rgba_verified"] for candidate in valid):
                    raise AssertionError("有效编码候选未完整验证或选中项不唯一")
                if encoding["selected_bytes"] != len(encoded) or chosen[0]["bytes"] != len(encoded):
                    raise AssertionError("候选体积和实际输出不符")
                if encoding["rgba_sha256"] != expected[key][1]:
                    raise AssertionError("编码报告RGBA哈希不符")
                if strategy and strategy[1] == "auto" and len(encoded) != min(row["bytes"] for row in valid):
                    raise AssertionError("auto没有选取最小有效候选")
                if strategy and strategy[1] in {"png", "auto"} and len(encoded) > encoding["baseline_bytes"]:
                    raise AssertionError("优化或auto结果大于自身PNG基线")
            suffix = path.suffix.lower()
            if (decoded_format, suffix) not in {("PNG", ".png"), ("WEBP", ".webp")}:
                raise AssertionError(f"格式和扩展名不一致：{decoded_format}/{suffix}")
            observed.add(key)
            formats[decoded_format] += 1
            modes[mode] += 1
            images.append(
                {
                    "identity": list(key),
                    "path": output["path"],
                    "bytes": len(encoded),
                    "sha256": digest(encoded),
                    "format": decoded_format,
                    "mode": mode,
                    "rgba_sha256": expected[key][1],
                    "encoding": descriptor.get("encoding"),
                }
            )
    if observed != set(expected):
        raise AssertionError("输出子资源集合不完整")
    actual = {entry["path"] for entry in images}
    other_images = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and path.suffix.lower() in {".png", ".webp"}
    }
    if actual != other_images:
        raise AssertionError("存在未登记的图像")
    return {
        "valid": True,
        "images": images,
        "image_count": len(images),
        "output_bytes": sum(row["bytes"] for row in images),
        "format_counts": dict(formats),
        "mode_counts": dict(modes),
        "metadata": [{"path_id": row["path_id"], "metadata": row["metadata"]} for row in active.values()],
    }


def verify_process(args, output, expected_success=True):
    result = run_process(
        [str(args.executable), "verify", "--manifest", str(output / "manifest.jsonl"), "--level", "decoded"],
        args.timeout,
    )
    if (result["returncode"] == 0) != expected_success:
        raise AssertionError(f"verify返回码与预期不符：{result}")
    return result


def check_suite(args):
    suite = read_json(args.suite)
    cases = selected_cases(suite, args.case)
    strategies = selected_strategies(args.strategy)
    args.work.mkdir(parents=True, exist_ok=True)
    results = []
    for case in cases:
        case_rows = []
        for strategy in strategies:
            with tempfile.TemporaryDirectory(prefix=f"encoding-{case['id']}-", dir=args.work) as directory:
                output = Path(directory)
                result = run_process(command_for(args, case, strategy, output), args.timeout)
                try:
                    if result["returncode"] != 0:
                        raise AssertionError(f"编码命令失败：{result['stderr']}；{result['stdout']}")
                    validation = validate_outputs(case, output, strategy)
                    verification = verify_process(args, output)
                    result.update({"passed": True, "validation": validation, "verification": verification})
                except Exception as error:
                    result.update(
                        {
                            "passed": False,
                            "error": f"{type(error).__name__}: {error}",
                            "traceback": traceback.format_exc(),
                        }
                    )
                result.update({"case": case["id"], "strategy": strategy[0]})
                results.append(result)
                case_rows.append(result)
            print(
                json.dumps(
                    {"case": case["id"], "strategy": strategy[0], "passed": result["passed"]}, ensure_ascii=False
                ),
                flush=True,
            )
        assert_size_choices(case_rows)
    ancillary = []
    if cases and not args.strategy:
        ancillary.append(check_strategy_isolation(args, cases[0]))
        ancillary.append(check_forged_descriptor(args, cases[0]))
        ancillary.append(check_forged_dimensions(args, cases[0]))
        ancillary.append(check_budget_fallback(args, cases[0]))
        ancillary.append(check_zero_deadline(args, cases[0]))
    report = {
        "schema": 1,
        "executable_sha256": digest(read_bytes(args.executable)),
        "suite_sha256": digest(read_bytes(args.suite)),
        "cases": results,
        "ancillary": ancillary,
        "passed": sum(row["passed"] for row in results) + sum(row["passed"] for row in ancillary),
        "total": len(results) + len(ancillary),
        "remaining_image_files": sum(
            1 for path in args.work.rglob("*") if path.is_file() and path.suffix.lower() in {".png", ".webp"}
        ),
    }
    write_json(args.output, report)
    return int(report["passed"] != report["total"])


def assert_size_choices(rows):
    valid = {row["strategy"]: row["validation"] for row in rows if row["passed"]}
    if "baseline" not in valid:
        return
    baseline = valid["baseline"]["output_bytes"]
    for name in ("png-balanced", "png-small", "auto-balanced", "auto-small"):
        if name in valid and valid[name]["output_bytes"] > baseline:
            raise AssertionError(f"{name}输出比保留的PNG基线更大")
    for name, png_name in (("auto-balanced", "png-balanced"), ("auto-small", "png-small")):
        if name in valid and png_name in valid and valid[name]["output_bytes"] > valid[png_name]["output_bytes"]:
            raise AssertionError("auto未保留更小的PNG候选")


def check_strategy_isolation(args, case):
    commands = []
    try:
        with tempfile.TemporaryDirectory(prefix="encoding-policy-", dir=args.work) as directory:
            output = Path(directory)
            first = run_process(command_for(args, case, STRATEGIES[0], output), args.timeout)
            commands.append(first)
            if first["returncode"] != 0:
                raise AssertionError("基线策略导出失败")
            baseline = read_rows(output / "manifest.jsonl")
            later = [*command_for(args, case, STRATEGIES[1], output), "--resume"]
            second = run_process(later, args.timeout)
            commands.append(second)
            if second["returncode"] != 0:
                raise AssertionError("策略切换恢复失败")
            combined = read_rows(output / "manifest.jsonl")
            if len({row["policy_key"] for row in combined}) < 2:
                raise AssertionError("编码优化配置没有隔离policy key")
            baseline_paths = {entry["output"]["path"] for row in baseline for entry in row["outputs"]}
            new_paths = {entry["output"]["path"] for row in combined[len(baseline) :] for entry in row["outputs"]}
            if baseline_paths & new_paths:
                raise AssertionError("编码策略覆盖了已有输出")
            verify_process(args, output)
        return {"case": "strategy_resume_isolation", "passed": True, "commands": commands}
    except Exception as error:
        return {"case": "strategy_resume_isolation", "passed": False, "error": str(error), "commands": commands}


def check_forged_descriptor(args, case):
    commands = []
    try:
        with tempfile.TemporaryDirectory(prefix="encoding-verify-", dir=args.work) as directory:
            output = Path(directory)
            result = run_process(command_for(args, case, STRATEGIES[4], output), args.timeout)
            commands.append(result)
            if result["returncode"] != 0:
                raise AssertionError("自动编码导出失败")
            verify_process(args, output)
            manifest = output / "manifest.jsonl"
            rows = read_rows(manifest)
            for row in rows:
                for entry in row["outputs"]:
                    entry["descriptor"]["rgba_sha256"] = "0" * 64
            manifest.write_text("\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n", encoding="utf-8")
            rejected = verify_process(args, output, False)
            commands.append(rejected)
        return {"case": "forged_decoded_descriptor", "passed": True, "commands": commands}
    except Exception as error:
        return {"case": "forged_decoded_descriptor", "passed": False, "error": str(error), "commands": commands}


def check_forged_dimensions(args, case):
    commands = []
    try:
        with tempfile.TemporaryDirectory(prefix="encoding-size-", dir=args.work) as directory:
            output = Path(directory)
            result = run_process(command_for(args, case, STRATEGIES[3], output), args.timeout)
            commands.append(result)
            if result["returncode"] != 0:
                raise AssertionError("WebP导出失败")
            verify_process(args, output)
            manifest = output / "manifest.jsonl"
            rows = read_rows(manifest)
            for row in rows:
                for entry in row["outputs"]:
                    entry["descriptor"]["width"] += 1
            manifest.write_text("\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n", encoding="utf-8")
            commands.append(verify_process(args, output, False))
        return {"case": "forged_decoded_dimensions", "passed": True, "commands": commands}
    except Exception as error:
        return {"case": "forged_decoded_dimensions", "passed": False, "error": str(error), "commands": commands}


def check_budget_fallback(args, case):
    commands = []
    try:
        with tempfile.TemporaryDirectory(prefix="encoding-budget-", dir=args.work) as directory:
            output = Path(directory)
            command = command_for(args, case, STRATEGIES[5], output)
            command[command.index("--memory-budget") + 1] = "64MiB"
            result = run_process(command, args.timeout)
            commands.append(result)
            if result["returncode"] != 0:
                raise AssertionError("受限预算下auto没有回退PNG")
            validation = validate_outputs(case, output, STRATEGIES[5])
            for image in validation["images"]:
                encoding = image["encoding"]
                if image["format"] != "PNG" or not encoding:
                    raise AssertionError("预算回退没有生成带报告的PNG")
                if not any(candidate["status"] == "skipped_budget" for candidate in encoding["candidates"]):
                    raise AssertionError("预算回退没有登记被跳过候选")
            verify_process(args, output)
        with tempfile.TemporaryDirectory(prefix="encoding-webp-budget-", dir=args.work) as directory:
            output = Path(directory)
            command = command_for(args, case, STRATEGIES[3], output)
            command[command.index("--memory-budget") + 1] = "64MiB"
            result = run_process(command, args.timeout)
            commands.append(result)
            if result["returncode"] == 0:
                raise AssertionError("显式WebP预算不足没有报错")
            if any(path.suffix.lower() in {".png", ".webp", ".part"} for path in output.rglob("*")):
                raise AssertionError("显式WebP预算失败遗留图像或临时文件")
        return {"case": "optimization_budget_fallback", "passed": True, "commands": commands}
    except Exception as error:
        return {"case": "optimization_budget_fallback", "passed": False, "error": str(error), "commands": commands}


def check_zero_deadline(args, case):
    commands = []
    try:
        for strategy in (STRATEGIES[5], STRATEGIES[3]):
            with tempfile.TemporaryDirectory(prefix="encoding-deadline-", dir=args.work) as directory:
                output = Path(directory)
                command = command_for(args, case, strategy, output)
                command[command.index("--optimize-seconds") + 1] = "0"
                result = run_process(command, args.timeout)
                commands.append(result)
                if result["returncode"] != 0:
                    raise AssertionError("0秒优化预算导出失败")
                validation = validate_outputs(case, output, strategy)
                for image in validation["images"]:
                    encoding = image["encoding"]
                    if strategy[1] == "auto":
                        if image["format"] != "PNG" or not any(
                            candidate["status"] == "skipped_timeout" for candidate in encoding["candidates"]
                        ):
                            raise AssertionError("auto的0秒预算没有跳过可选优化")
                    if strategy[1] == "webp" and image["format"] != "WEBP":
                        raise AssertionError("显式WebP在0秒预算下静默回退格式")
                verify_process(args, output)
        return {"case": "zero_optional_search_deadline", "passed": True, "commands": commands}
    except Exception as error:
        return {"case": "zero_optional_search_deadline", "passed": False, "error": str(error), "commands": commands}


def benchmark_suite(args):
    if args.repeats < 1:
        raise ValueError("重复次数必须为正")
    module = importlib.util.spec_from_file_location("rurino_benchmark", args.benchmark_script)
    if module is None or module.loader is None:
        raise ValueError("无法加载基准脚本")
    benchmark = importlib.util.module_from_spec(module)
    module.loader.exec_module(benchmark)
    suite = read_json(args.suite)
    requested = args.case or ["card", "sticker", "attribute"]
    cases = selected_cases(suite, requested)
    strategies = selected_strategies(args.strategy, True)
    args.work.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    rows = []
    with args.output.with_suffix(".jsonl").open("w", encoding="utf-8", newline="\n") as journal:
        for case in cases:
            for strategy in strategies:
                for repeat in range(args.repeats):
                    with tempfile.TemporaryDirectory(prefix="encoding-bench-", dir=args.work) as directory:
                        output = Path(directory)
                        command = command_for(args, case, strategy, output)
                        metrics = benchmark.measure_process(command, args.timeout, output)
                        validation = (
                            validate_outputs(case, output, strategy) if metrics["returncode"] == 0 else {"valid": False}
                        )
                        row = {
                            "case": case["id"],
                            "strategy": strategy[0],
                            "repeat": repeat + 1,
                            "command": command,
                            **metrics,
                            "validation": validation,
                        }
                        rows.append(row)
                        journal.write(json.dumps(row, ensure_ascii=False) + "\n")
                        journal.flush()
                    print(
                        f"{case['id']} {strategy[0]} {repeat + 1}/{args.repeats}："
                        f"{metrics['wall_seconds']:.3f}s，峰值{metrics['peak_working_set_bytes'] / 1048576:.2f}MiB，"
                        f"{'通过' if validation['valid'] else '失败'}",
                        flush=True,
                    )
    summaries = []
    for case in cases:
        baseline_rows = [row for row in rows if row["case"] == case["id"] and row["strategy"] == "baseline"]
        baseline_bytes = baseline_rows[0]["validation"]["output_bytes"] if baseline_rows else None
        for strategy in strategies:
            selected = [row for row in rows if row["case"] == case["id"] and row["strategy"] == strategy[0]]
            sizes = [row["validation"].get("output_bytes", 0) for row in selected]
            summaries.append(
                {
                    "case": case["id"],
                    "strategy": strategy[0],
                    "runs": len(selected),
                    "all_valid": all(row["validation"]["valid"] for row in selected),
                    "wall_seconds_median": statistics.median(row["wall_seconds"] for row in selected),
                    "peak_working_set_bytes_max": max(row["peak_working_set_bytes"] for row in selected),
                    "peak_commit_bytes_max": max(row["peak_commit_bytes"] for row in selected),
                    "output_bytes": sizes,
                    "reduction_percent": (1 - sizes[0] / baseline_bytes) * 100 if baseline_bytes else None,
                }
            )
    report = {
        "schema": 1,
        "executable_sha256": digest(read_bytes(args.executable)),
        "suite_sha256": digest(read_bytes(args.suite)),
        "summary": summaries,
        "runs": rows,
        "notes": [
            "每次独立进程，Windows内核峰值；输出验证在计时外。",
            "PNG优化和auto使用profile=default，与baseline一致；png-fast单列。",
            "同一输入顺序测量，未清空系统文件缓存，不是纯编解码器性能排名。",
            "本机WMI查询挂起，脚本禁用platform的WMI探测并使用其系统API回退。",
        ],
    }
    write_json(args.output, report)
    return int(not all(row["validation"]["valid"] for row in rows))


def comparison_suite(args):
    module = importlib.util.spec_from_file_location("rurino_comparison", args.benchmark_script)
    if module is None or module.loader is None:
        raise ValueError("无法加载独立进程基准模块")
    benchmark = importlib.util.module_from_spec(module)
    module.loader.exec_module(benchmark)
    cases = selected_cases(read_json(args.suite), args.case or ["card", "sticker", "attribute"])
    largest = read_json(args.largest_suite)
    largest_case = next(case for case in largest["cases"] if case["id"] == "largest_atlas-png-one")
    source = largest["inputs"][largest_case["input"]]
    cases.append(
        {
            "id": "largest_atlas-png-one",
            "source": source["path"],
            "source_sha256": source["sha256"],
            "source_bytes": source["bytes"],
            "class_id": 28,
            "path_ids": largest_case["path_ids"],
            "mips": "0",
            "expected": [
                {
                    "path_id": largest_case["path_ids"][0],
                    "face": 0,
                    "mip": 0,
                    "size": [row["width"], row["height"]],
                    "rgba_sha256": row["rgba_sha256"],
                }
                for row in largest_case["expected"]
            ],
        }
    )
    if digest(read_bytes(source["path"])) != source["sha256"]:
        raise AssertionError("最大图集原包SHA不符")
    args.work.mkdir(parents=True, exist_ok=True)
    worker_suite = {
        "inputs": {
            case["id"]: {"path": case["source"], "bytes": case["source_bytes"], "sha256": case["source_sha256"]}
            for case in cases
        },
        "cases": [
            {
                "id": case["id"],
                "input": case["id"],
                "operation": "png",
                "classes": [case["class_id"]],
                "path_ids": case["path_ids"],
                "mips": case["mips"],
            }
            for case in cases
        ],
    }
    worker_path = args.work / "comparison-worker-suite.json"
    write_json(worker_path, worker_suite)
    worker_python = benchmark.resolve_python(args.python)
    environment = os.environ.copy()
    environment["PYTHONPATH"] = worker_python["site_packages"]
    environment["PYTHONNOUSERSITE"] = "1"
    rows = []
    for case in cases:
        for tool in ("Rurino", "UnityPy"):
            for repeat in range(args.repeats):
                with tempfile.TemporaryDirectory(prefix="encoding-compare-", dir=args.work) as directory:
                    output = Path(directory)
                    command = command_for(args, case, STRATEGIES[0], output)
                    if tool == "UnityPy":
                        command = [
                            worker_python["executable"],
                            "-c",
                            "import platform,runpy,sys;platform._wmi=None;"
                            "sys.argv=sys.argv[1:];runpy.run_path(sys.argv[0],run_name='__main__')",
                            str(args.benchmark_script),
                            "worker",
                            "--suite",
                            str(worker_path),
                            "--case",
                            case["id"],
                            "--output",
                            str(output),
                        ]
                    metrics = benchmark.measure_process(
                        command, args.timeout, output, environment if tool == "UnityPy" else None
                    )
                    validation = {"valid": False}
                    if metrics["returncode"] == 0:
                        if tool == "Rurino":
                            validation = validate_outputs(case, output, STRATEGIES[0])
                        if tool == "UnityPy":
                            actual = []
                            for path in output.glob("*.png"):
                                with Image.open(io.BytesIO(read_bytes(path))) as decoded:
                                    image = decoded.convert("RGBA")
                                    actual.append((image.width, image.height, digest(image.tobytes())))
                            expected = [
                                (entry["size"][0], entry["size"][1], entry["rgba_sha256"]) for entry in case["expected"]
                            ]
                            validation = {
                                "valid": Counter(actual) == Counter(expected),
                                "image_count": len(actual),
                                "output_bytes": sum(path.stat().st_size for path in output.glob("*.png")),
                            }
                    rows.append(
                        {
                            "case": case["id"],
                            "tool": tool,
                            "repeat": repeat + 1,
                            "command": command,
                            **metrics,
                            "validation": validation,
                        }
                    )
                print(
                    f"{case['id']} {tool} {repeat + 1}/{args.repeats}：{metrics['wall_seconds']:.3f}s，"
                    f"峰值{metrics['peak_working_set_bytes'] / 1048576:.2f}MiB，"
                    f"{'通过' if validation['valid'] else '失败'}",
                    flush=True,
                )
    summaries = []
    for case in cases:
        for tool in ("Rurino", "UnityPy"):
            selected = [row for row in rows if row["case"] == case["id"] and row["tool"] == tool]
            summaries.append(
                {
                    "case": case["id"],
                    "tool": tool,
                    "runs": len(selected),
                    "all_valid": all(row["validation"]["valid"] for row in selected),
                    "wall_seconds_median": statistics.median(row["wall_seconds"] for row in selected),
                    "peak_working_set_bytes_max": max(row["peak_working_set_bytes"] for row in selected),
                    "peak_commit_bytes_max": max(row["peak_commit_bytes"] for row in selected),
                    "output_bytes": [row["validation"].get("output_bytes", 0) for row in selected],
                }
            )
    report = {
        "schema": 1,
        "executable_sha256": digest(read_bytes(args.executable)),
        "machine": {"platform": platform.platform(), "logical_cpus": os.cpu_count()},
        "python_worker": worker_python,
        "summary": summaries,
        "runs": rows,
        "notes": [
            "Rurino为profile=default、optimize=none；UnityPy PNG compress_level=6。",
            "每工具每样本三次独立进程，时间含启动和导入，像素核验在计时外。",
            "Windows内核PeakWorkingSetSize累计峰值；Python直接运行base executable避免launcher偏差。",
            "未清空操作系统文件缓存；测试顺序固定，没有并发基准。",
            "PNG编码算法不同，体积差异单列，不推断为纯解码器性能。",
            "本机WMI查询挂起，两侧基准脚本均禁用platform的WMI探测并使用系统API回退。",
        ],
        "remaining_image_files": sum(
            1 for path in args.work.rglob("*") if path.is_file() and path.suffix.lower() in {".png", ".webp"}
        ),
    }
    write_json(args.output, report)
    return int(not all(row["validation"]["valid"] for row in rows))


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def selftest():
    sample = Image.new("RGBA", (2, 2), (10, 20, 30, 0))
    changed = sample.copy()
    changed.putpixel((0, 0), (11, 20, 30, 0))
    if digest(sample.tobytes()) == digest(changed.tobytes()):
        raise AssertionError("透明RGB哈希无效")
    if list(sample.size) != [2, 2]:
        raise AssertionError("尺寸oracle异常")
    rows = [
        {"strategy": "baseline", "passed": True, "validation": {"output_bytes": 100}},
        {"strategy": "png-balanced", "passed": True, "validation": {"output_bytes": 90}},
        {"strategy": "auto-balanced", "passed": True, "validation": {"output_bytes": 80}},
    ]
    assert_size_choices(rows)
    try:
        assert_size_choices(
            [
                {"strategy": "baseline", "passed": True, "validation": {"output_bytes": 100}},
                {"strategy": "png-small", "passed": True, "validation": {"output_bytes": 101}},
            ]
        )
    except AssertionError:
        print("透明像素RGB与只采用更小结果的控制测试通过")
        return 0
    raise AssertionError("未拒绝比基线更大的优化输出")


if __name__ == "__main__":
    raise SystemExit(main())
