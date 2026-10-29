# Rurino

按需读取、有界输出和可核验无损导出的 Rust AssetBundle 命令行工具。

## 能力

- `inspect` 仅读取容器与对象表，输出分类、内存准入估计和源读取请求统计，不解析全部对象名称。
- `plan` 根据文件、目录或 catalog 的精确类型／逻辑名筛选生成固定 SHA-256 计划。
- `extract` 支持原生载荷、Texture2D PNG／无损 WebP、Cubemap 六面与 mip、TextAsset、字体和视频原字节。
- `verify` 核对文件 SHA-256，并可重新解码 PNG／WebP 核对尺寸与完整 RGBA。
- 可选 PNG 体积优化，以及 PNG／无损 WebP 的逐图最小候选选择。
- `model` 按选定 prefab 根输出 GLB 与源数据说明，支持完整顶点通道、层级、蒙皮和形态数据的独立数值回读。
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
Rurino extract --input <资源.assetbundle> --class 28 --format png --optimize balanced --output <目录>
Rurino extract --input <资源.assetbundle> --class 28 --format auto --optimize small --jobs 1 --output <目录>
Rurino verify --manifest <目录/manifest.jsonl> --level decoded
Rurino model --input <角色prefab.assetbundle> --catalog <catalog.json> --output <模型目录>
Rurino model --input <prefab.assetbundle> --dependency <依赖.assetbundle> --root-path-id <GameObject路径ID> --output <模型目录>
```

`--class`、`--path-id`、`--input` 可重复指定。目录枚举限当前层 `.assetbundle`；catalog 仅选择 ResourceType=1，标签和类型精确相交筛选。常用类 ID：Texture2D=28、TextAsset=49、Cubemap=89、Font=128、VideoClip=329、RenderTexture=84。

PNG 的 `--profile fast|default|best` 对应基线 PNG 编码器的压缩配置，像素一致；默认 `fast` 优先速度，文件可能更大。

`--optimize none|balanced|small` 默认 `none`，PNG 沿用流式写出；`balanced`／`small` 分别使用 oxipng 2／4 级搜索，并允许无损缩减 RGB、灰度、索引色与位深。候选必须重新解码，与原始显示顺序的完整 RGBA 一致才可采用，PNG 优化不会选取比基线更大的文件。

`--format webp` 输出无损 WebP；`--format auto` 在基线 PNG、所选 PNG 优化候选与无损 WebP 中逐图选小，因此同一任务可能同时产生 `.png` 和 `.webp`。显式 WebP 不承诺小于 PNG。WebP 使用 image-webp 的 VP8L 编码器，其压缩比与 libwebp 的不同配置需要分别测量。

`--optimize-seconds 5` 限制每张图的可选搜索时间，从基线编码与核验完成后开始计时；它是软截止，不会中断正在运行的压缩试验。设为 `0` 跳过可选搜索，显式 WebP 仍执行必要编码和核验。内存不足时 PNG 优化／auto 保留已核验基线；显式 WebP 无法满足预算则报错。每张图的清单记录候选大小、耗时、验证状态、选择结果和跳过原因。

`--mips all` 导出全部存储级别，Cubemap 会依次输出六面。默认 `--mips 0` 输出首个存储 mip。Alpha8 派生 PNG 默认填 RGB=0，可用 `--alpha8-rgb 255` 明确选择白色；源 alpha 不变。

同一输出目录继续执行时使用 `--resume`。来源哈希、导出实现、策略和当前输出哈希匹配才复用结果；格式／编码策略使用独立路径。完整清单中的中间损坏报错，末尾未完成行可恢复。

导出前按流计算一次原包 SHA-256，核验计划来源并建立输出身份；这会扫描原包字节，但仅解码被选对象的像素。进度将来源哈希读取和选择性读取分别统计。

## 无损范围

`native` 保留原生像素载荷、文本／字体／视频字节，或尚未支持类型的原始对象片段；其状态为 `raw_verified`，不等于语义解码成功。原始对象片段必须与原始序列化文件、依赖和来源清单一起使用，不能独自还原完整 bundle。

PNG／WebP 的像素核验覆盖完整 RGBA，透明像素 RGB 也参与哈希。纹理原生输出另存 mip／面布局；某个 mip 的像素无损不能替代原始压缩块和整条 mip 链的保留。图片导出只对明确允许的 8 位源格式承诺；HDR／浮点等未覆盖格式可使用原生载荷保存。

纹理的 `m_ColorSpace` 保存在元数据中。图片导出不做色彩空间转换，也不额外合成 ICC；本工具处理 AssetBundle 纹理，不处理任意已有 PNG 的元数据优化。

RenderTexture 和 0×0 动态字体纹理没有静态画面，分别标记 `metadata_only`／`expected_empty`。图片模式未支持的类型标记 `decoded_not_supported`，不会将 raw 回退冒充图片输出。本工具不重建 Shader 执行、模型渲染或动画播放。

内存参数控制加载准入估计、解析上限和每任务解码工作区，包含有界 entry 缓存、候选缓冲和重新解码的估计；它是软预算，不是操作系统 RSS 硬限制。并发按原包分配预算；最大块或任务超过额度会拒绝，无法通过增大队列绕过。候选顺序编码，oxipng 不启用内部多线程；较大图片可用 `--jobs 1` 将任务预算集中到当前原包。

## 验证与性能

模型命令输出 `model.glb`、`model.source.json` 和 `manifest.jsonl`。catalog 用于解析根原包父目录中的明确依赖闭包；也可重复使用 `--dependency` 指定所需原包。自动选择首个原包中唯一的根 GameObject；有多个根时须指定 `--root-path-id`，`--root-file-index` 限于首个原包的序列化文件。加载依赖不等于导出全部依赖模型，仅输出所选子树。

模型预算默认 256 MiB、输出配额默认 64 MiB；顺序处理，二进制逐数组写出。输出目录必须为空，任务失败清理已写文件。首版模型任务不提供断点恢复，现有图片提取与恢复策略独立使用。

GLB 保留位置、法线、切线及手性、颜色、UV0–7、三角子网格、层级、蒙皮／绑定矩阵和稀疏形态；Unity 坐标转为 glTF 坐标时进行明确的 X 镜像、绕序及矩阵排列转换。非二维 UV 通过自定义属性保留，原语义由源数据说明记录。四维位置／法线只接受已逐顶点验证的第四分量正零，并保存恢复规则；其他值明确拒绝。

材质在 GLB 中为名称占位，完整源属性和引用保存在说明中；本步没有输出贴图、Shader 效果或运行时动画。多帧形态保留全部帧和原权重，注明需要自定义插值，通用查看器不保证按 Unity 方式混合。模型数据保留与原版外观、组件装配及动作效果分别验收。

`verify --level files` 核对模型与说明文件哈希；完整模型数值核验使用 `scripts/validate_model.py` 从原包建立独立 UnityPy 参考，再用独立 GLB 读取器检查。`--level decoded` 对 GLB 明确提示使用该流程，不把文件哈希通过冒充数值通过。固定样本、结果及复验方式见 [模型数据验证](docs/model-data-validation.md)。

当前三轮性能、无损优化收益和 UnityPy 对比见 [0.2.0 性能报告](docs/performance-v02.md)，首版基线见 [0.1.0 报告](docs/performance.md)。原始游戏资源和图片不进入仓库，测试输出使用临时目录，核验后清理。

Windows 基准使用真实工作进程的内核峰值工作集和提交量；会绕过 Python venv 启动器，避免低估峰值。基准脚本依赖独立 Python 环境中的 UnityPy 1.25.4、Pillow 13 和 NumPy，使用方式见报告。

## 许可

项目自身代码采用 [MIT](LICENSE)。第三方组件继续遵循各自许可，清单和原文见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。游戏数据和资源不属于本项目许可授予范围。
