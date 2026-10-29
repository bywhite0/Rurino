# 模型数据导出与独立回读

Rurino 0.3.0 增加源模型的 GLB 投影和源数据说明。验收对象是解码后的模型数值、层级与引用；材质外观、运行时组件、动画和原版画面另行验证。实测二进制 SHA-256 为 `b01bf530e2c97dd504e069a3b8914cc6c0edb21eb1840320438d6a6f22d87e33`。

## 输出与用法

```text
Rurino model --input <prefab.assetbundle> --catalog <catalog.json> --output <模型目录>
Rurino model --input <prefab.assetbundle> --dependency <依赖.assetbundle> --root-path-id <路径ID> --output <模型目录>
Rurino verify --manifest <模型目录/manifest.jsonl> --level files
```

`model.glb` 保存可交换模型，`model.source.json` 保存来源、原字段、GLB 缓冲引用及明确的表达限制，`manifest.jsonl` 保存文件哈希。catalog 按逻辑名递归选取依赖、循环去重；依赖从根原包的父目录读取。没有 catalog 时可重复指定 `--dependency`。选择根原包中唯一的根 GameObject，有多个根时要求显式 `--root-path-id`。

模型只包含选定子树，不把载入的源 FBX 层级再次拼入。输出目录必须为空，任务失败清理部分成果；默认内存软预算 256 MiB、输出配额 64 MiB。首版模型命令顺序处理，不提供模型断点恢复。

`verify --level files` 仅核对文件哈希。完整数值核验需要原包及独立参考，使用下述验证脚本；`--level decoded` 对 GLB 明确拒绝，避免将文件一致性当作模型数值通过。

## 保存的数据与格式边界

- GLB 保留位置、法线、切线与手性、颜色、UV0–7、三角子网格、局部 TRS、蒙皮／绑定矩阵及稀疏形态。非二维 UV 使用自定义属性，不假装标准二维 TEXCOORD。
- Unity 到 glTF 进行明确的 X 反射、三角形绕序、四元数和切线手性转换。绑定矩阵先将源序列化行顺序转成 glTF 列顺序，再做坐标反射；源哈希仍按原序列化顺序计算。
- 四维位置／法线逐顶点验证第四分量为正零，并在源说明记录恢复位模式。非零或负零会拒绝，避免核心三分量读取静默丢值。
- 骨权重保持解码后的 float32 原值，不重新归一化。非空 variable bone-count 权重、非三角拓扑、非零 baseVertex、缺失必需引用或子树外骨骼会拒绝，未覆盖格式不猜测。
- 形态保留全部稀疏源记录、原顺序、帧、通道和 fullWeight。单帧通道映射默认权重；多帧通道保留数据并标记需要自定义插值，通用 GLB 查看器不承诺 Unity 的多帧混合语义。
- 材质在 GLB 中是名称占位，源说明保存有序属性、浮点位模式、纹理 PPtr／ST、关键词、渲染状态及未解释尾部原字节。没有输出贴图图像或 Shader 仿真，不声明原版外观。
- 原生压缩块、外置流和完整游戏对象仍由原包及依赖归档保留。解码后数值一致不等于输出文件可替代原 bundle。

## 独立验证方法

参考端使用 UnityPy 1.25.4 的 MeshHandler 与原始 TypeTree，直接读取固定依赖闭包，每包先核验大小和 SHA-256；不使用 unity-rs 数据作为独立参考。GLB 端由 pygltflib 1.16.5 解析结构，另一个 accessor 读取器展开 bufferView／sparse 数据，再按既定坐标转换比较完整 float32／uint32 字节，包括零的符号。

对照还覆盖全部节点的局部 TRS、active、组件身份／类别／layer、材质槽、骨骼顺序／rootBone、bindpose、默认形态和原稀疏记录，以及有序材质属性／位模式、纹理 ST、关键词、queue、原对象哈希与尾部 4 字节。验证不会仅信任生产者自己写出的数组哈希。

| 固定样本 | 明确依赖 | 节点 | 网格／顶点 | 属性数组 | 蒙皮顶点 | 形态帧 | 材质属性项 |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| 服装 1001102101 | 153 包 | 254 | 12／23,238 | 74 | 23,238 | 129 | 1,718 |
| 吉他 ppadv010_guitar | 4 包 | 1 | 1／2,443 | 3 | 0 | 0 | 32 |

两个样本的全部已比较数据通过。服装有切线及额外 UV1；吉他的源四维法线 W 为正零，恢复策略另行核验。**真实样本没有顶点色和非二维 UV，因此不把这两条实现路径列为已完成真实资源验收。** 本轮也未采样运行时动画、姿态约束或 Shader 画面。

独立回读发现初稿把核心 bindpose 的原始连续字段误当列主序，造成部分骨矩阵转置。原始对象中 `e00,e01,e02,e03,e10...` 的字节顺序验证了实际布局；修正写出排列后，沿用未修改的独立参考重新通过全部矩阵。单元测试加入非对称矩阵、平移和负零位模式检查。

## 回归与标准格式

- 27 项 Rust 测试、格式检查、全目标 Clippy 无警告通过。
- 8 组模型黑盒回归通过：独立回读、拒绝覆盖、显式根、低内存、输出配额／回滚、缺失依赖、错误根和单 bit 篡改拒绝；部分检查合并在同一用例中。
- 原有 9 组图片／原生资源 CLI 回归通过，保留配额、恢复、清单、catalog 与像素核验行为。
- Khronos gltf-validator 2.0.0-dev.3.10 对两个样本均报告 0 错误。初稿叶节点空 children 数组经标准校验发现并修正为省略字段。
- 吉他无警告；服装有 12 条 `NODE_SKINNED_MESH_NON_ROOT`，说明蒙皮节点位于层级内、其父节点变换不能按静态网格编辑方式理解。源骨骼和数值层级仍保留，不为消除警告改变原结构。占位材质导致的 unused UV／tangent 等信息另保留在报告。
- 所有临时 GLB、源说明和测试图片已清理，固定原包未修改；仓库只保存来源、数组哈希与完成的指标，原始数值 oracle 留在本地。

## 性能

Windows 11、12 个逻辑处理器，Rust 1.94.1 release；每样本三次独立进程，端到端导出耗时取中位数，内存取 Windows 内核累计峰值工作集的最大值。包括 catalog 解析、来源哈希、依赖加载、模型读取和文件写出；独立回读在计时外。未清空系统文件缓存，未与其他工具进行同范围性能排名。

| 样本 | 耗时中位数 | 峰值工作集 | GLB 字节 | GLB＋源说明字节 |
| --- | ---: | ---: | ---: | ---: |
| 服装 | 0.361 s | 51.05 MiB | 4,329,716 | 6,224,311 |
| 吉他 | 0.223 s | 19.48 MiB | 104,596 | 132,707 |

6 次性能运行全部通过独立回读。内存参数是加载、索引、源数组、写出计划和 JSON 的软估计，不是操作系统 RSS 硬限制。GLB 二进制逐数组写出，源数组与有界计划／JSON仍驻留。

## 复验

独立 Python 环境需要 UnityPy 1.25.4、NumPy 2.5.4、pygltflib 1.16.5 及 UnityPy 所需依赖。Windows 上脚本进程内禁用挂起的 WMI 平台探测，使用系统 API 回退；这不修改 Python 安装或系统服务。

```text
python scripts/validate_model.py prepare --shared <资料根> --dependency-report <固定依赖报告.json> --output <本地oracle.json>
python scripts/validate_model.py check --shared <资料根> --oracle <本地oracle.json> --executable <Rurino.exe> --work <临时目录> --output <回归.json>
python scripts/validate_model.py bench --shared <资料根> --oracle <本地oracle.json> --executable <Rurino.exe> --benchmark-script scripts/benchmark_unpacker.py --work <临时目录> --output <性能.json> --repeats 3
```

prepare 消费固定原包闭包报告，输出含完整源数值的本地 oracle；它不随仓库分发。独立格式校验另外使用 Khronos gltf-validator 的 validateBytes，报告保留版本和消息，不需要运行产品时安装 Node。

终态记录见 [固定输入摘要](../reports/model-suite.json)、[模型回归](../reports/model-regression.json)、[三轮性能](../reports/model-performance.json)、[GLB 标准校验](../reports/model-gltf-validation.json) 和 [既有 CLI 回归](../reports/cli-regression-v03.json)。公开记录仅替换本地路径，数值和文件／内容哈希保持原值。
