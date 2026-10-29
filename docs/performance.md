# 首版性能与无损验证

Rurino 0.1.0 使用 unity-rs-core 0.6.1。固定 7 个原包、14 个场景，各支持路径顺序运行三轮，共 111 次独立进程；Rurino 的 42 次全部通过内容验证。其余运行中，UnityPy 全部通过；官方 unity-rs 有三次因最大图集默认 preload table 项数上限而失败，作为能力限制单列。

## 测量方法

- Windows 11，12 个逻辑处理器；Rust 1.94.1 release，Python 3.14.0、UnityPy 1.25.4、Pillow 13.0.0。
- 输入 SHA-256、PNG 完整 RGBA 和原生字节 oracle 在计时外建立，输出逐次验证并清理。
- 时间为三轮墙钟中位数，包含进程启动与导入；内存为三轮内核峰值工作集的最大值。
- Windows GetProcessMemoryInfo 记录 PeakWorkingSetSize 和 PeakPagefileUsage，后者是进程提交峰值，不是物理 pagefile 写入量。
- 直接启动 Python 真实解释器，绕过 venv 重定向启动器；96 MiB 瞬时分配控制试验确认释放后仍可捕捉内核峰值。
- 未清空系统文件缓存。索引命令的名称与引用解析工作量不同，不能将端到端差异简单归因于语言。

## 主要结果

| 场景 | 工具 | 中位耗时 | 峰值工作集 | 输出字节 | 验证 |
| --- | --- | ---: | ---: | ---: | --- |
| 最大图集索引 | Rurino | 0.0164 s | 7.97 MiB | 0 | 通过 |
| 最大图集索引 | UnityPy | 9.6433 s | 1329.14 MiB | 0 | 通过 |
| 最大图集索引 | 官方 unity-rs | 不作成功对比 | 293.04 MiB | 0 | 默认项数上限 |
| 图集中单张 2048² PNG | Rurino | 0.8762 s | 48.60 MiB | 5,327,936 | 通过 |
| 图集中单张 2048² PNG | UnityPy | 9.9480 s | 1380.47 MiB | 5,099,135 | 通过 |
| 卡面 PNG default | Rurino | 0.4906 s | 17.62 MiB | 3,108,175 | 通过 |
| 卡面 PNG default | 官方 unity-rs | 0.5281 s | 17.27 MiB | 3,108,175 | 通过 |
| 卡面 PNG level 6 | UnityPy | 1.5259 s | 138.14 MiB | 3,000,048 | 通过 |
| 卡面 PNG fast | Rurino | 0.0698 s | 17.51 MiB | 3,297,853 | 通过 |
| Spine 原始文本 | Rurino | 0.0286 s | 10.65 MiB | 2,081,074 | 通过 |
| 五个字体原字节 | Rurino | 0.0520 s | 17.98 MiB | 7,287,848 | 通过 |
| Cubemap 六面全部 mip | Rurino | 0.0558 s | 8.72 MiB | 6,489 | 通过 |

最大图集为 163,712,241 字节，3,216 个对象。Rurino 按需索引的源读取请求量为 57,614 字节、34 次，未读取全部纹理载荷；此数字包含重复源请求，不等于物理磁盘 I/O。UnityPy 展开更多容器内容，官方 Rust CLI 读取 preload 名称元数据，需结合工作量理解时间。

单页探针选择其中一张 2048² ETC2 图集页，追加三轮。Rurino 的完整 RGBA 与独立 oracle 相同；来源 SHA-256 校验扫描 163,712,241 字节，索引与选择性解码另读取 2,244,844 字节，合计 165,957,085 字节。它仍读取全部原包作哈希，但只展开所选像素，不宣称零全包 I/O。

官方 CLI 报告 preload table 含 5,105,420 项，超过默认 1,000,000 项限制。失败前耗时和峰值只保留在原始记录，不作为成功吞吐对比。

卡面 default 输出与官方 Rust PNG 逐字节一致，像素也与 UnityPy 完全一致；UnityPy PNG 更小。fast 仍逐像素一致，但比 default 多约 6.1% 体积，速度与体积需要同时比较。

这些结果仅覆盖固定样本。软内存预算不是操作系统 RSS 硬限制；索引低占用来自延迟读取，不能直接等同全部资源解码的占用。

## 正确性

- Rust 单元测试 16 项通过，覆盖 catalog、路径、Cube 布局、原生字节、透明 RGB、预算及读取计数。
- 真实 CLI 回归 9 组通过，覆盖配额回滚、错误后恢复、尾行修复、策略隔离、并发共享配额、像素哈希、catalog、固定源计划和缺失来源哈希拒绝。
- Cubemap 共 48 张 PNG，即 6 面 × 8 mip，尺寸与完整 RGBA 逐项匹配 UnityPy 独立块布局 oracle。
- 二进制 Spine、Font、VideoClip 和 Cubemap 原生压缩载荷逐字节核验，不经 JSON 字符串重编码。
- Alpha8 派生 RGB=0，与 UnityPy 全 RGBA 相同；源 alpha 不变，不与官方 RGB=255 的结果混比。
- 格式检查、Clippy 全目标无警告检查通过，测试临时 PNG 全部清理。

## 复验

Windows 基准环境需 UnityPy 1.25.4、Pillow 13.0.0、NumPy 2.5.4。prepare 使用共享资料的归档清单与固定原包，不随仓库分发游戏资源。

```text
python scripts/benchmark_unpacker.py selftest
python scripts/benchmark_unpacker.py prepare --shared <资料根> --output <本地suite.json>
python scripts/benchmark_unpacker.py run --suite <本地suite.json> --executable <Rurino.exe> --unity-rs-executable <官方CLI> --python <隔离python> --output <结果.json> --work <临时目录> --repeats 3
python scripts/exercise_cli.py --executable <Rurino.exe> --plain <plain目录> --work <回归临时目录>
```

完整记录见 reports/performance.json，CSV 见 reports/performance-summary.csv，回归见 reports/cli-regression.json。公开记录的路径已替换为占位符，数值及输入、输出、二进制哈希保持原值。reports/suite.json 只保存固定资源身份和 oracle，复验时用 prepare 生成实际路径。

实测二进制 SHA-256 为 a1f0a2d9b8eae64f5b473ce86d4f381f8ac7ff1c8909c7b3c949157f65a5e6e8。单页追加结果见 reports/largest-texture-probe.json，其固定对象参考见 reports/largest-texture-suite.json。
