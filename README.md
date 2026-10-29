# Rurino

按需读取、有界输出和可核验无损导出的 Rust AssetBundle 命令行工具。

## 首版能力

- `inspect` 仅读取容器与对象表，输出分类、内存准入估计和源读取请求统计，不解析全部对象名称。
- `plan` 根据文件、目录或 catalog 的精确类型／逻辑名筛选生成固定 SHA-256 计划。
- `extract` 支持原生载荷、Texture2D PNG、Cubemap 六面与 mip、TextAsset、字体和视频原字节。
- `verify` 核对文件 SHA-256，并可重新解码 PNG 核对完整 RGBA。
- 支持有界并发、内存软预算、输出配额、断点恢复；失败对象清理部分输出，状态写入 JSONL。

核心固定为 `unity-rs-core 0.6.1`。按需 entry 及 Region 保留资源区间，避免默认加载器将所有 entry 展开为完整 Vec。

核心程序、CLI、解码与文件写出均由 Rust 实现，运行可执行文件不需要 Python。`scripts/` 中的 Python 仅用于基准测试、独立内容校验和第三方许可清单生成。

## 构建

需要 Rust 1.88 或更新版本及对应平台的 C/C++ 构建工具链。首次构建会下载并编译依赖。

```text
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

## 使用

以下路径为占位符；Windows 二进制名为 `Rurino.exe`。

```text
Rurino inspect --input <资源.assetbundle>
Rurino inspect --input <plain目录> --catalog <catalog.json> --label <逻辑名>
Rurino plan --input <资源.assetbundle> --class 28 --format png --output <plan.jsonl>
Rurino extract --plan <plan.jsonl> --output <目录> --memory-budget 256MiB --output-budget 512MiB --jobs 2
Rurino extract --input <资源.assetbundle> --class 89 --format png --mips all --output <目录>
Rurino verify --manifest <目录/manifest.jsonl> --level decoded
```

`--class`、`--path-id`、`--input` 可重复指定。目录枚举限当前层 `.assetbundle`；catalog 仅选择 ResourceType=1，标签和类型精确相交筛选。常用类 ID：Texture2D=28、TextAsset=49、Cubemap=89、Font=128、VideoClip=329、RenderTexture=84。

PNG 的 `--profile fast|default|best` 对应核心 PNG 编码器的压缩配置，像素一致；`fast` 优先速度，文件可能更大。首版尚未接入 oxipng 优化或 WebP 导出。

`--mips all` 导出全部存储级别，Cubemap 会依次输出六面。默认 `--mips 0` 输出首个存储 mip。Alpha8 派生 PNG 默认填 RGB=0，可用 `--alpha8-rgb 255` 明确选择白色；源 alpha 不变。

同一输出目录继续执行时使用 `--resume`。来源哈希、导出实现、策略和当前输出哈希匹配才复用结果；格式／编码策略使用独立路径。完整清单中的中间损坏报错，末尾未完成行可恢复。

导出前按流计算一次原包 SHA-256，核验计划来源并建立输出身份；这会扫描原包字节，但仅解码被选对象的像素。进度将来源哈希读取和选择性读取分别统计。

## 无损范围

`native` 保留原生像素载荷、文本／字体／视频字节，或尚未支持类型的原始对象片段；其状态为 `raw_verified`，不等于语义解码成功。原始对象片段必须与原始序列化文件、依赖和来源清单一起使用，不能独自还原完整 bundle。

PNG 导出核对全部 RGBA，透明像素 RGB 也参与哈希。纹理原生输出另存 mip／面布局；PNG 某个 mip 的像素无损不能替代原始压缩块和整条 mip 链的保留。首版 PNG 只对明确允许的 8 位源格式承诺；HDR／浮点等未覆盖格式可使用原生载荷保存。

RenderTexture 和 0×0 动态字体纹理没有静态画面，分别标记 `metadata_only`／`expected_empty`。PNG 模式未支持的类型标记 `decoded_not_supported`，不会将 raw 回退冒充图片输出。首版不重建 Shader 执行、模型渲染或动画播放。

内存参数控制加载准入估计、解析上限和每任务解码工作区，包含有界 entry 缓存估计；它是软预算，不是操作系统 RSS 硬限制。并发按原包分配预算；最大块或任务超过额度会拒绝，无法通过增大队列绕过。

## 验证与性能

固定输入身份、三轮结果和测量方法见 [性能报告](docs/performance.md)。原始游戏资源和图片不进入仓库，测试输出使用临时目录，核验后清理。

Windows 基准使用真实工作进程的内核峰值工作集和提交量；会绕过 Python venv 启动器，避免低估峰值。基准脚本依赖独立 Python 环境中的 UnityPy 1.25.4、Pillow 13 和 NumPy，使用方式见报告。

## 许可

项目自身代码采用 [MIT](LICENSE)。第三方组件继续遵循各自许可，清单和原文见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。游戏数据和资源不属于本项目许可授予范围。
