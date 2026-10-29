"""收集锁定 Windows x64 依赖的许可文本与第三方声明。"""

import argparse
import hashlib
import io
import json
import re
import subprocess
import tarfile
from collections import Counter
from pathlib import Path

import tomllib


def git_path(path):
    value = path.as_posix()
    if len(value) >= 3 and value[1:3] == ":/":
        return "/" + value[0].lower() + value[2:]
    return value


def read_git_bash(bash, path):
    return subprocess.run(
        [str(bash), "-lc", 'cat -- "$1"', "reader", git_path(path)],
        capture_output=True,
        check=True,
    ).stdout


def active_packages(metadata):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending = list(metadata["workspace_default_members"])
    selected = set()
    while pending:
        identifier = pending.pop()
        if identifier in selected:
            continue
        selected.add(identifier)
        for dependency in nodes[identifier]["deps"]:
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])
    return sorted(
        (packages[identifier] for identifier in selected if packages[identifier]["source"] is not None),
        key=lambda package: (package["name"], package["version"]),
    )


def main():
    parser = argparse.ArgumentParser(description="生成当前目标依赖许可资料")
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", default="x86_64-pc-windows-msvc")
    parser.add_argument("--bash", type=Path, default=Path("C:/Program Files/Git/bin/bash.exe"))
    args = parser.parse_args()
    metadata_result = subprocess.run(
        ["cargo", "metadata", "--offline", "--locked", "--filter-platform", args.target, "--format-version", "1"],
        cwd=args.repository,
        capture_output=True,
        check=True,
    )
    metadata = json.loads(metadata_result.stdout)
    packages = active_packages(metadata)
    checksums = {
        (package["name"], package["version"]): package["checksum"]
        for package in tomllib.loads(read_git_bash(args.bash, args.repository / "Cargo.lock").decode("utf-8"))[
            "package"
        ]
        if "checksum" in package
    }
    roots = [Path(package["manifest_path"]).parent for package in packages]
    listing = subprocess.run(
        [
            str(args.bash),
            "-lc",
            'find "$@" -type f \\( -iname "*license*" -o -iname "copying*" -o -iname "*notice*" '
            '-o -iname "authors*" -o -iname "*copyright*" \\) -print0',
            "license-scan",
            *(git_path(path) for path in roots),
        ],
        capture_output=True,
        check=True,
    ).stdout
    paths = [path for path in listing.split(b"\0") if path]
    if not paths:
        raise ValueError("依赖源码目录没有任何许可文件")
    process = subprocess.run(
        [str(args.bash), "-lc", "tar -cf - --null -T -"],
        input=b"\0".join(paths) + b"\0",
        capture_output=True,
        check=True,
    )
    blobs = {}
    with tarfile.open(fileobj=io.BytesIO(process.stdout), mode="r:") as stream:
        for member in stream:
            if member.isfile():
                blobs["/" + member.name.lstrip("/")] = stream.extractfile(member).read()
    args.output.mkdir(parents=True, exist_ok=True)
    package_rows = []
    for package, root in zip(packages, roots):
        prefix = git_path(root).rstrip("/") + "/"
        files = []
        for name, data in sorted(blobs.items()):
            if not name.startswith(prefix):
                continue
            relative = name.removeprefix(prefix)
            target = args.output / "licenses" / f"{package['name']}-{package['version']}" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            files.append(
                {
                    "source_relative_path": relative,
                    "path": target.relative_to(args.output).as_posix(),
                    "bytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "basis": "crate 源码内的原始文件",
                }
            )
        package_rows.append(
            {
                "name": package["name"],
                "version": package["version"],
                "license": package["license"],
                "registry_source": package["source"],
                "repository": package["repository"],
                "crate_url": f"https://crates.io/crates/{package['name']}/{package['version']}",
                "exact_source_url": f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download",
                "crate_sha256": checksums[(package["name"], package["version"])],
                "modified_by_this_project": False,
                "license_files": files,
            }
        )
    core = next(row for row in package_rows if row["name"] == "unity-rs-core")
    core_licenses = next(
        blobs[git_path(roots[packages.index(package)]) + "/THIRD_PARTY_LICENSES.txt"]
        for package in packages
        if package["name"] == "unity-rs-core"
    ).decode("utf-8")
    missing = []
    for package in package_rows:
        if package["license_files"]:
            continue
        name = re.escape(package["name"])
        version = re.escape(package["version"])
        match = re.search(
            rf"(?ms)^Package: {name} {version}\n(.*?)(?=^=+\nPackage: |\Z)",
            core_licenses,
        )
        if not match:
            missing.append(f"{package['name']} {package['version']}")
            continue
        text = (f"Package: {package['name']} {package['version']}\n" + match.group(1)).strip() + "\n"
        target = args.output / "licenses" / f"{package['name']}-{package['version']}" / "UPSTREAM-LICENSE-BUNDLE.txt"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8", newline="\n")
        data = text.encode("utf-8")
        package["license_files"].append(
            {
                "path": target.relative_to(args.output).as_posix(),
                "bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
                "basis": f"unity-rs-core {core['version']} 随 crate 提供的 THIRD_PARTY_LICENSES.txt 精确包名版本段落",
            }
        )
    if missing:
        raise ValueError("缺少可核验许可文本：" + "、".join(missing))
    mpl = [row for row in package_rows if "MPL-2.0" in row["license"]]
    notes = [
        "# 第三方许可与来源",
        "",
        f"本清单按 Cargo.lock 和 `cargo metadata --offline --locked --filter-platform {args.target}` "
        f"解析的非开发依赖生成，共 {len(package_rows)} 个第三方 crate；包含目标平台适用的运行与构建依赖。",
        "",
        "Rurino 自有代码使用 MIT 许可。第三方依赖各自的许可表达式与版权声明仍适用，"
        "不将整个依赖图表述为 MIT。多选许可保留 crate 内全部可用许可文本，AND 许可同时保留其附加文本。",
        "",
        "许可文件按依赖源码原始相对路径保存在 `licenses/<crate-version>/`；"
        "机器清单 `third-party-licenses.json` 记录每份文本和每个原始 crate 包的 SHA-256。",
        "",
        "## MPL-2.0 依赖源码",
        "",
        "Symphonia 的四个依赖采用 MPL-2.0，本项目没有修改这些依赖的源码。"
        "分发可执行文件时，应同时提供本声明和许可文本，并使接收者能取得相应 MPL 覆盖源码。"
        "下列链接是锁定版本的完整 crate 源码，包哈希见机器清单；未来修改这些依赖时需要提供对应修改后的源码。",
        "",
        "| 包 | 版本 | 对应源码 |",
        "| --- | --- | --- |",
    ]
    for package in mpl:
        notes.append(f"| {package['name']} | {package['version']} | [完整 crate 源码]({package['exact_source_url']}) |")
    notes.extend(
        [
            "",
            "MPL-2.0 为这些文件保留其许可，不改变独立自有代码的 MIT 声明。"
            "本清单保守列出锁定依赖图，即使某些解码路径未被首版命令直接调用。",
            "",
            "## 内嵌和附加许可",
            "",
            "- `unity-rs-core` 内嵌修正的 texture2ddecoder 代码及其 MIT 文本，"
            "同包的 `THIRD_PARTY_NOTICES.md` 和 `THIRD_PARTY_LICENSES.txt` 已一并保留。",
            "- `FSB_VORBIS_NOTICE.md` 保留 fsbex 0.3.0 生成数据的 MIT 授权与版权声明。",
            "- `unicode-ident` 的 Unicode-3.0、`jpeg-encoder` 的 IJG、"
            "`brotli` 的 BSD/MIT，以及 `zstd-sys` 内嵌 Zstandard 的 BSD 声明按源包原文保留。",
            "- 某个发布 crate 若没有独立 LICENSE 文件，仅在上游 core 的合并许可文件存在"
            "完全相同的包名和版本段落时复用该文本，并在机器清单标出来源。",
            "",
            "## 锁定依赖列表",
            "",
            "| 包 | 版本 | 声明许可 | 源码仓库 | 随附许可文本 |",
            "| --- | --- | --- | --- | --- |",
        ]
    )
    for package in package_rows:
        source = package["repository"] or package["crate_url"]
        links = "、".join(
            f"[{row.get('source_relative_path', Path(row['path']).name)}]({row['path']})"
            for row in package["license_files"]
        )
        notes.append(
            f"| {package['name']} | {package['version']} | {package['license']} | [来源]({source}) | {links} |"
        )
    report = {
        "schema": 1,
        "target": args.target,
        "project_license": "MIT",
        "cargo_lock_sha256": hashlib.sha256(read_git_bash(args.bash, args.repository / "Cargo.lock")).hexdigest(),
        "package_count": len(package_rows),
        "license_file_count": sum(len(row["license_files"]) for row in package_rows),
        "license_expression_counts": dict(sorted(Counter(row["license"] for row in package_rows).items())),
        "packages": package_rows,
        "generation": {
            "cargo_command": [
                "cargo",
                "metadata",
                "--offline",
                "--locked",
                "--filter-platform",
                args.target,
                "--format-version",
                "1",
            ],
            "source_reads": "Git Bash find + tar 批量只读，逐文件原始字节保留",
            "core_supplementary_notices": True,
            "all_dependencies_unmodified_by_project": True,
        },
    }
    (args.output / "THIRD_PARTY_NOTICES.md").write_text("\n".join(notes) + "\n", encoding="utf-8", newline="\n")
    (args.output / "third-party-licenses.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    print(
        json.dumps(
            {
                "packages": report["package_count"],
                "license_files": report["license_file_count"],
                "MPL-2.0": [f"{row['name']} {row['version']}" for row in mpl],
                "fallback_package_count": sum(
                    any("basis" in item and "精确包名版本" in item["basis"] for item in row["license_files"])
                    for row in package_rows
                ),
            },
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
