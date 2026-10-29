"""专用解包工具的真实命令行回归。"""

import argparse
import hashlib
import io
import json
import subprocess
import tempfile
import time
import traceback
from pathlib import Path

import UnityPy
from PIL import Image
from UnityPy.export import Texture2DConverter

BASH = "C:/Program Files/Git/bin/bash.exe"


def read_bytes(path):
    return subprocess.run(
        [BASH, "-lc", 'cat -- "$(cygpath -u "$1")"', "reader", str(path)],
        capture_output=True,
        check=True,
    ).stdout


def read_rows(path):
    return [json.loads(line) for line in read_bytes(path).decode("utf-8-sig").splitlines() if line.strip()]


def run(executable, arguments, expected_success, commands):
    started = time.perf_counter()
    process = subprocess.run(
        [str(executable), *map(str, arguments)],
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        timeout=180,
    )
    commands.append(
        {
            "arguments": list(map(str, arguments)),
            "returncode": process.returncode,
            "seconds": time.perf_counter() - started,
            "stdout": process.stdout,
            "stderr": process.stderr,
        }
    )
    if (process.returncode == 0) != expected_success:
        raise AssertionError(
            f"返回码与预期不符：{process.returncode}；{process.stdout[-1500:]}；{process.stderr[-1500:]}"
        )
    return process


def extract(executable, source, output, commands, *options, success=True):
    return run(
        executable,
        [
            "extract",
            "--input",
            source,
            "--output",
            output,
            "--memory-budget",
            "256MiB",
            "--output-budget",
            "64MiB",
            *options,
        ],
        success,
        commands,
    )


def verify(executable, output, level, commands, success=True):
    return run(
        executable,
        ["verify", "--manifest", output / "manifest.jsonl", "--level", level, "--memory-budget", "256MiB"],
        success,
        commands,
    )


def files_in(output):
    return [path for path in output.rglob("*") if path.is_file() and path.name != "manifest.jsonl"]


def latest_rows(path):
    rows = {}
    for row in read_rows(path):
        rows[(row["input_sha256"], row["entry"], row["path_id"], row["policy_key"])] = row
    return list(rows.values())


def quota_cleanup(executable, plain, output, commands):
    run(
        executable,
        [
            "extract",
            "--input",
            plain / "image_card_full_10217031.assetbundle",
            "--output",
            output,
            "--format",
            "png",
            "--class",
            "28",
            "--output-budget",
            "1",
            "--memory-budget",
            "256MiB",
        ],
        False,
        commands,
    )
    remaining = files_in(output)
    if remaining:
        raise AssertionError(f"配额失败遗留文件：{remaining}")
    return {"remaining_payload_files": 0, "recorded_rows": len(read_rows(output / "manifest.jsonl"))}


def resumed_history(executable, plain, output, commands):
    quota_cleanup(executable, plain, output, commands)
    extract(
        executable,
        plain / "image_card_full_10217031.assetbundle",
        output,
        commands,
        "--format",
        "png",
        "--class",
        "28",
        "--resume",
    )
    verify(executable, output, "decoded", commands)
    manifest = output / "manifest.jsonl"
    with manifest.open("ab") as writer:
        writer.write(b'{"schema":')
    extract(
        executable,
        plain / "image_card_full_10217031.assetbundle",
        output,
        commands,
        "--format",
        "png",
        "--class",
        "28",
        "--resume",
    )
    verify(executable, output, "decoded", commands)
    rows = read_rows(manifest)
    active = latest_rows(manifest)
    if not active or any(row["status"] == "decode_error" for row in active):
        raise AssertionError("恢复后最新记录仍有错误")
    return {"parseable_rows": len(rows), "active_rows": len(active), "partial_tail_recovered": True}


def profile_isolation(executable, plain, output, commands):
    source = plain / "image_card_full_10217031.assetbundle"
    extract(executable, source, output, commands, "--format", "png", "--class", "28", "--profile", "fast")
    first = latest_rows(output / "manifest.jsonl")
    extract(
        executable,
        source,
        output,
        commands,
        "--format",
        "png",
        "--class",
        "28",
        "--profile",
        "best",
        "--resume",
    )
    rows = latest_rows(output / "manifest.jsonl")
    policies = {row["policy_key"] for row in rows}
    if len(policies) != 2:
        raise AssertionError(f"策略未隔离：{len(policies)}")
    paths = [{entry["output"]["path"] for entry in row["outputs"]} for row in rows]
    if len(paths) != 2 or paths[0] & paths[1]:
        raise AssertionError("不同策略复用了同一输出路径")
    verify(executable, output, "decoded", commands)
    return {"policies": len(policies), "isolated_output_paths": len(set.union(*paths)), "first_records": len(first)}


def jobs_quota(executable, plain, output, commands):
    budget = 1024 * 1024
    run(
        executable,
        [
            "extract",
            "--input",
            plain / "image_card_full_10217031.assetbundle",
            "--input",
            plain / "image_card_full_10213010.assetbundle",
            "--output",
            output,
            "--format",
            "native",
            "--class",
            "28",
            "--jobs",
            "2",
            "--output-budget",
            str(budget),
            "--memory-budget",
            "256MiB",
        ],
        False,
        commands,
    )
    files = files_in(output)
    if any(path.suffix == ".part" for path in files):
        raise AssertionError("并发配额失败遗留临时文件")
    actual_bytes = sum(path.stat().st_size for path in files)
    if actual_bytes > budget:
        raise AssertionError(f"并发输出超额：{actual_bytes}>{budget}")
    rows = latest_rows(output / "manifest.jsonl")
    if len(rows) != 2 or not any(row["status"] == "decode_error" for row in rows):
        raise AssertionError("并发配额没有登记完整的两个对象结果")
    recorded = {output / entry["output"]["path"] for row in rows for entry in row["outputs"]}
    if set(files) != recorded:
        raise AssertionError("并发配额留下未登记输出")
    return {"payload_bytes": actual_bytes, "budget_bytes": budget, "rows": len(rows), "unrecorded_files": 0}


def forged_rgba_descriptor(executable, plain, output, commands):
    extract(
        executable,
        plain / "image_card_full_10217031.assetbundle",
        output,
        commands,
        "--format",
        "png",
        "--class",
        "28",
    )
    verify(executable, output, "decoded", commands)
    manifest = output / "manifest.jsonl"
    rows = read_rows(manifest)
    changed = 0
    for row in rows:
        for entry in row["outputs"]:
            if entry["descriptor"].get("rgba_sha256"):
                entry["descriptor"]["rgba_sha256"] = "0" * 64
                changed += 1
    if not changed:
        raise AssertionError("未找到像素核验描述")
    manifest.write_text("\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n", encoding="utf-8")
    verify(executable, output, "files", commands)
    verify(executable, output, "decoded", commands, success=False)
    return {"forged_pixel_descriptors": changed, "files_verify_passed": True, "decoded_verify_rejected": True}


def cubemap_all_subresources(executable, plain, output, commands):
    source = plain / "__reflectionprobe-0.exr.assetbundle"
    environment = UnityPy.load(read_bytes(source))
    texture = next(obj.read() for obj in environment.objects if obj.type.name == "Cubemap")
    if texture.m_TextureFormat != 50:
        raise AssertionError("固定 Cubemap 样本的格式发生变化")
    extract(executable, source, output, commands, "--format", "png", "--class", "89", "--mips", "all")
    verify(executable, output, "decoded", commands)
    rows = latest_rows(output / "manifest.jsonl")
    entries = [entry for row in rows for entry in row["outputs"]]
    expected = 6 * texture.m_MipCount
    if len(entries) != expected:
        raise AssertionError(f"Cubemap 输出数不符：{len(entries)} != {expected}")
    payload = texture.get_image_data()
    if texture.m_CompleteImageSize * 6 != len(payload):
        raise AssertionError("Cubemap 面跨度与载荷长度不符")
    fingerprints = {}
    for face in range(6):
        offset = face * texture.m_CompleteImageSize
        for mip in range(texture.m_MipCount):
            width = max(1, texture.m_Width >> mip)
            height = max(1, texture.m_Height >> mip)
            length = ((width + 5) // 6) * ((height + 5) // 6) * 16
            image = Texture2DConverter.parse_image_data(
                payload[offset : offset + length],
                width,
                height,
                texture.m_TextureFormat,
                texture.object_reader.version,
                texture.object_reader.platform,
                getattr(texture, "m_PlatformBlob", None),
            ).convert("RGBA")
            fingerprints[(face, mip)] = (image.size, hashlib.sha256(image.tobytes()).hexdigest())
            offset += length
    observed = set()
    for entry in entries:
        description = entry["descriptor"]
        key = (description["face"], description["mip"])
        observed.add(key)
        with Image.open(io.BytesIO(read_bytes(output / entry["output"]["path"]))) as opened:
            image = opened.convert("RGBA")
            actual = (image.size, hashlib.sha256(image.tobytes()).hexdigest())
        if actual != fingerprints[key]:
            raise AssertionError(f"Cubemap 面或mip像素不符：{key}")
    if observed != set(fingerprints):
        raise AssertionError("Cubemap 存在遗漏或重复的子资源")
    return {"faces": 6, "mips": texture.m_MipCount, "pngs": len(entries), "independent_rgba_verified": len(entries)}


def catalog_exact_selection(executable, plain, output, commands):
    shared = plain.parents[2]
    catalog = shared / "archive/game/catalog.json"
    process = run(
        executable,
        [
            "inspect",
            "--input",
            plain,
            "--catalog",
            catalog,
            "--label",
            "image_card_full_10217031",
            "--type",
            "png",
            "--memory-budget",
            "256MiB",
        ],
        True,
        commands,
    )
    events = [json.loads(line) for line in process.stdout.splitlines() if line.strip()]
    bundles = [row for row in events if row.get("event") == "bundle"]
    if len(bundles) != 1 or not bundles[0]["input"].endswith("image_card_full_10217031.assetbundle"):
        raise AssertionError("catalog 精确标签没有仅命中指定卡面")
    run(
        executable,
        [
            "inspect",
            "--input",
            plain,
            "--catalog",
            catalog,
            "--label",
            "image_card_full_10217031",
            "--type",
            "prefab",
            "--memory-budget",
            "256MiB",
        ],
        False,
        commands,
    )
    run(
        executable,
        [
            "inspect",
            "--input",
            plain,
            "--catalog",
            catalog,
            "--label",
            "__missing_regression_label__",
            "--memory-budget",
            "256MiB",
        ],
        False,
        commands,
    )
    return {"selected_bundles": len(bundles), "mismatched_type_rejected": True, "missing_label_rejected": True}


def plan_rejects_changed_source(executable, plain, output, commands):
    original = read_bytes(plain / "image_card_full_10217031.assetbundle")
    source = output / "source.assetbundle"
    source.write_bytes(original)
    plan = output / "plan.jsonl"
    run(
        executable,
        ["plan", "--input", source, "--output", plan, "--format", "native", "--class", "28"],
        True,
        commands,
    )
    planned = read_rows(plan)
    if len(planned) != 1 or planned[0]["input_sha256"] != hashlib.sha256(original).hexdigest():
        raise AssertionError("任务计划没有记录正确源哈希")
    accepted = output / "accepted"
    run(
        executable,
        ["extract", "--plan", plan, "--output", accepted, "--jobs", "1", "--output-budget", "4MiB"],
        True,
        commands,
    )
    verify(executable, accepted, "files", commands)
    changed = bytearray(original)
    changed[-1] ^= 1
    source.write_bytes(changed)
    rejected = output / "rejected"
    process = run(
        executable,
        ["extract", "--plan", plan, "--output", rejected, "--jobs", "1", "--output-budget", "4MiB"],
        False,
        commands,
    )
    if files_in(rejected):
        raise AssertionError("源哈希变化后仍发布了载荷")
    if "SHA" not in process.stderr and "哈希" not in process.stderr:
        raise AssertionError(f"源变化拒绝原因不明确：{process.stderr}")
    return {
        "plan_rows": len(planned),
        "unchanged_source_accepted": True,
        "changed_source_rejected": True,
        "payload_files_after_rejection": 0,
    }


def plan_rejects_empty_source_hash(executable, plain, output, commands):
    plan = output / "plan.jsonl"
    run(
        executable,
        [
            "plan",
            "--input",
            plain / "image_card_full_10217031.assetbundle",
            "--output",
            plan,
            "--format",
            "native",
            "--class",
            "28",
        ],
        True,
        commands,
    )
    rows = read_rows(plan)
    rows[0]["input_sha256"] = ""
    plan.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    rejected = output / "rejected"
    process = run(
        executable,
        ["extract", "--plan", plan, "--output", rejected, "--jobs", "1", "--output-budget", "4MiB"],
        False,
        commands,
    )
    if rejected.exists() and files_in(rejected):
        raise AssertionError("缺少源哈希的计划仍发布了载荷")
    if "SHA" not in process.stderr and "哈希" not in process.stderr:
        raise AssertionError(f"缺少源哈希的拒绝原因不明确：{process.stderr}")
    return {"empty_hash_rejected": True, "payload_files_after_rejection": 0}


def main():
    parser = argparse.ArgumentParser(description="真实 CLI 配额、恢复与无损导出回归")
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--plain", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)
    checks = [
        ("quota_cleanup", quota_cleanup),
        ("resume_error_and_partial_tail", resumed_history),
        ("profile_output_isolation", profile_isolation),
        ("jobs2_shared_quota", jobs_quota),
        ("decoded_verify_forged_descriptor", forged_rgba_descriptor),
        ("cubemap_all_faces_mips", cubemap_all_subresources),
        ("catalog_exact_label_selection", catalog_exact_selection),
        ("plan_changed_source_hash", plan_rejects_changed_source),
        ("plan_empty_source_hash", plan_rejects_empty_source_hash),
    ]
    results = []
    for name, operation in checks:
        commands = []
        started = time.perf_counter()
        with tempfile.TemporaryDirectory(prefix=f"{name}-", dir=args.work) as folder:
            output = Path(folder).resolve()
            if output.parent != args.work.resolve():
                raise ValueError("临时目录超出测试工作区")
            try:
                details = operation(args.executable, args.plain, output, commands)
                result = {"case": name, "passed": True, "details": details}
            except Exception as error:
                result = {
                    "case": name,
                    "passed": False,
                    "error": f"{type(error).__name__}: {error}",
                    "traceback": traceback.format_exc(),
                }
        result["seconds"] = time.perf_counter() - started
        result["commands"] = commands
        results.append(result)
        print(
            json.dumps(
                {key: value for key, value in result.items() if key not in ("commands", "traceback")},
                ensure_ascii=False,
            ),
            flush=True,
        )
    report = {
        "executable": str(args.executable.resolve()),
        "executable_sha256": hashlib.sha256(read_bytes(args.executable)).hexdigest(),
        "source_scope": str(args.plain.resolve()),
        "source_sha256": {
            name: hashlib.sha256(read_bytes(args.plain / name)).hexdigest()
            for name in (
                "image_card_full_10217031.assetbundle",
                "image_card_full_10213010.assetbundle",
                "__reflectionprobe-0.exr.assetbundle",
            )
        },
        "cases": results,
        "passed": sum(row["passed"] for row in results),
        "total": len(results),
        "temporary_png_count_after_cleanup": sum(1 for _ in args.work.rglob("*.png")),
        "notes": [
            "卡面只用两个原包，Cube只用一个原包；没有全量图片输出。",
            "测试读取原包、PNG和manifest均经Git Bash cat，写入仅限任务临时输出和测试清单。",
            "临时输出在每个用例完成后清理，原包未修改。",
        ],
    }
    (args.work / "cli-regression.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {
                "passed": report["passed"],
                "total": report["total"],
                "remaining_pngs": report["temporary_png_count_after_cleanup"],
            },
            ensure_ascii=False,
        )
    )
    raise SystemExit(0 if report["passed"] == report["total"] else 1)


if __name__ == "__main__":
    main()
