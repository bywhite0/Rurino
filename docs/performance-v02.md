# 无损编码优化与 UnityPy 对比

Rurino 0.2.0 保留默认流式 PNG，增加 oxipng 无损优化、无损 WebP 和逐图自动择小。所有测量使用同一 release 二进制，SHA-256 为 `39da537dd3a19cca956301b194a437b6c570f5da6633230e03ad3729046aa820`。

## 测量方法

- Windows 11，12 个逻辑处理器；Rust 1.94.1 release，Python 3.14.0、UnityPy 1.25.4、Pillow 13.0.0。
- 每个场景三轮独立进程、顺序测量；时间取墙钟中位数，内存取 Windows 内核峰值工作集的三轮最大值。未清空系统文件缓存。
- 时间包含启动、Python 导入、资源加载、解码和写出；Rurino 另包含原包来源哈希，UnityPy worker 没有额外计算该哈希。独立 oracle 与导出后的内容核验在计时外，优化模式内置的候选回读核验在计时内。
- 单原包运行，`--jobs 1 --memory-budget 256MiB`；优化搜索每图软截止为 5 秒，候选顺序处理。
- Windows Python 直接启动真实解释器，避免虚拟环境启动器低估峰值。本机 WMI 查询挂起，测量脚本在进程内禁用 `platform` 的 WMI 探测，使用其系统 API 回退；没有修改 Python 安装或系统服务。
- Rurino 对比档为 `--format png --profile default --optimize none`，UnityPy 使用 Pillow PNG level 6。二者完整 RGBA 一致，PNG 压缩算法与文件大小不同；数据是端到端表现，不能等同纯解码器速度。

## 当前导出性能

| 场景 | Rurino 耗时 | UnityPy 耗时 | 耗时比 | Rurino 峰值 | UnityPy 峰值 | Rurino／UnityPy 输出字节 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 卡面 PNG | 0.459 s | 1.477 s | 3.22 倍 | 15.06 MiB | 131.63 MiB | 3,108,175／3,000,048 |
| 透明贴纸 PNG | 0.035 s | 1.248 s | 35.98 倍 | 7.61 MiB | 117.48 MiB | 169,234／165,796 |
| 属性图标 5 张 | 0.028 s | 1.255 s | 44.61 倍 | 6.47 MiB | 116.78 MiB | 60,190／59,128 |
| 大图集中单张 2048² PNG | 0.882 s | 9.954 s | 11.28 倍 | 46.06 MiB | 1373.96 MiB | 5,327,936／5,099,135 |

全部 24 次运行均通过尺寸与完整 RGBA 核验。小资源的端到端差异包含 Python 导入开销；大图集收益主要来自按需展开与仅解码所选对象。选择性解码仍扫描原包计算来源 SHA-256，不表示完全跳过全包读取。

默认 `--profile fast` 单独测量：卡面 0.060 s、14.91 MiB、3,297,853 字节，比 `default` 多 6.10% 体积。它是不同压缩档，不混作同档性能排名。

## 体积优化的成本

以下使用相同 `profile=default` 建立基线；耗时包含原包处理、基线编码、可选候选与候选完整 RGBA 核验。自动择小会同时试 PNG 与无损 WebP，只采用验证通过且更小的结果。

| 资源 | 策略 | 输出字节 | 比基线减少 | 耗时 | 峰值工作集 |
| --- | --- | ---: | ---: | ---: | ---: |
| 卡面 | none | 3,108,175 | 0 | 0.469 s | 15.05 MiB |
| 卡面 | PNG balanced | 2,581,822 | 16.93% | 1.987 s | 46.78 MiB |
| 卡面 | PNG small | 2,533,245 | 18.50% | 3.494 s | 47.02 MiB |
| 卡面 | auto small | 2,402,824 | 22.69% | 3.543 s | 46.98 MiB |
| 透明贴纸 | none | 169,234 | 0 | 0.035 s | 7.60 MiB |
| 透明贴纸 | auto small | 139,572 | 17.53% | 0.442 s | 17.41 MiB |
| 属性图标 5 张 | none | 60,190 | 0 | 0.027 s | 6.30 MiB |
| 属性图标 5 张 | auto small | 46,422 | 22.87% | 0.301 s | 9.70 MiB |

45 次优化基准全部通过内容核验。卡面自动选择 WebP，贴纸和属性图标选择 PNG；WebP 并不总是更小。卡面 balanced 至 small 增加约 1.51 秒，仅再减少 48,577 字节。归档可选择 small，频繁导出可保留 none，或按体积需求选 balanced。

Alpha8 字体与 Cubemap 另做正确性与体积测量，不混入三轮时间表：Alpha8 从 588 降至 124 字节，Cubemap 的 48 张从 6,489 降至 3,882 字节，分别减少 78.91% 和 40.18%。Cube 自动输出 41 张 PNG、7 张 WebP。

5 个固定原包、56 张图片的自动择小总大小从 3,344,676 降至 2,592,824 字节，减少 22.48%。这是该样本集的加权结果，不代表全部游戏资源的压缩率。这里使用 image-webp 的 VP8L 编码器，不能套用 Pillow/libwebp 其他配置的既有测量。

## 无损与预算验证

- 21 项 Rust 单元测试、35 组新 CLI 回归、9 组原有 CLI 回归全部通过；格式检查和全目标 Clippy 无警告。
- 新回归覆盖 6 种策略 × 5 个原包，独立核验 336 张图片的尺寸与完整 RGBA，包括全透明像素中的非零 RGB；颜色类型覆盖 RGBA、RGB、灰度 alpha 和索引色 PNG。
- 验证自动选择最小有效候选、策略隔离、断点复用、伪造像素哈希／尺寸拒绝、低预算回退和零秒截止。显式 WebP 预算不足会报错，零秒仍执行必要编码与核验。
- 继续覆盖输出配额、对象失败回滚、清单尾行修复、并发共享配额、固定来源计划和 Cubemap 六面全部 mip。
- 内存参数是工作区软估算，优化搜索为软截止；上游编码／解码器不能保证硬 RSS 或立即中断。报告候选耗时包含编码及独立回读核验。
- 临时 PNG／WebP 均已清理，原始游戏资源和导出图片不进入仓库。

## 复验与记录

复验环境需安装 UnityPy、Pillow 和 NumPy。以下路径均为占位符；单页筛选与 oracle 见 [最大图集 suite](../reports/largest-texture-suite.json)，复验时需将原包路径替换为实际位置。

```text
python scripts/benchmark_encoding.py selftest
python scripts/benchmark_encoding.py prepare --shared <资料根> --output <encoding-suite.json>
python scripts/benchmark_encoding.py check --suite <encoding-suite.json> --executable <Rurino.exe> --work <临时目录> --output <回归.json>
python scripts/benchmark_encoding.py bench --suite <encoding-suite.json> --executable <Rurino.exe> --benchmark-script scripts/benchmark_unpacker.py --work <临时目录> --output <性能.json> --repeats 3
python scripts/benchmark_encoding.py compare --suite <encoding-suite.json> --largest-suite <单页suite.json> --executable <Rurino.exe> --benchmark-script scripts/benchmark_unpacker.py --python <隔离Python> --work <临时目录> --output <对比.json> --repeats 3
```

固定身份见 [encoding-suite.json](../reports/encoding-suite.json)；完整记录见 [优化性能](../reports/encoding-performance.json)、[UnityPy 对比](../reports/current-comparison.json)、[编码回归](../reports/encoding-regression.json) 和 [原有 CLI 回归](../reports/cli-regression-v02.json)。公开记录仅替换本地路径，测量数值与内容哈希保留原值。0.1.0 基线保存在 [首版报告](performance.md)。
