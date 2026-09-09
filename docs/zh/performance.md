# 性能目标与测量

工程参考预算是：120 Hz 显示器每帧 8.33 ms，普通输入到下一可见帧不超过 16.67 ms。这些是依赖
运行环境的工程目标，不是 API 保证。

下面的数据描述当前 macOS 后端及其 Apple Silicon 测试环境。它们是后端基线，并不表示
Anmixiu 仅支持 macOS；Windows、Linux/FreeBSD、iOS 和 Android 后端落地后会补充跨平台基准。

使用两个 Tokio worker 的 release Counter 在参考 M1 Pro 主机上，空闲物理内存占用约为 31 MB。
反复实时调整窗口大小后，Retina `CAMetalLayer` IOSurface 从约 10 MB 增长到 33 MB，Rust
`MALLOC_SMALL` 保持在约 11–13 MB。观察到的增长属于图形资源高水位，不是堆持续增长的证据。
活动 resize 可能占用单个 CPU 核心的很大一部分，因为每次合并后的尺寸变化都要进行布局、场景构建、
drawable resize 和呈现；停止操作后，空闲 CPU 必须回到零。

Criterion 基准覆盖 Signal 通知与 dirty 去重、Taffy 布局、场景缓存复用、Metal 提交与离屏绘制，
以及 CoreText shaping 和 atlas 工作。运行全部 release 基准：

```sh
cargo bench --workspace
```

每项基准都应记录常规规模和压力规模。比较算法变化时，必须使用分配器 profiler 捕获分配次数与字节数；
只有 Criterion 耗时不能作为分配证据。测试会断言缓存统计和硬容量，确保 warm steady state 的记账
结构不会无界增长。

## 当前发布基线

以下结果记录于 2026-08-26，环境为 Apple M1 Pro（arm64，16 GiB）、macOS 26.5.1、
Rust 1.98.0，使用最终 workspace 和 `cargo bench --workspace`：

| 路径 | 常规 | 压力 |
| --- | ---: | ---: |
| 共享 UI 字符串克隆 | 长标签：9.66–9.67 ns | 1,000 次克隆：0 分配字节 |
| 共享短格式化 | `Count 42`：5.75–5.77 ns | 1,000 个静态标签：0 分配字节 |
| macOS 帧投影 | 100 个缓存按钮：59.4–60.3 µs | hover Scene 重建：82.9–91.9 µs |
| Signal 通知与 dirty 提取 | 1 个 owner：197.5–203.5 ns | 1,000 个：84.6–86.9 µs |
| 重复 dirty 插入 | 10 次写入：207.8–219.9 ns | 100,000 次：1.584–1.586 ms |
| Taffy 未缓存 Flexbox | 100 个节点：25.01–25.17 µs | 5,000 个：1.381–1.391 ms |
| Taffy 完整键缓存命中 | 1,000 个节点：9.78–10.23 ns | — |
| Scene 命令构建 | 100 个：337.8–343.8 ns | 10,000 个：27.37–27.51 µs |
| Scene 缓存命中 | 39.8–41.8 ns | — |
| 使用系统 UI 字体的 CoreText 缓存 shaping | 常规：29.24–29.54 µs | 混合拉丁/CJK：71.69–72.00 µs |
| Metal 提交、完成与回读 | 1 次绘制：388.3–414.4 µs | 1,000 次：825.7–854.7 µs |

Metal 基准包含 256×256 CPU 回读，因此比屏幕上的异步 present 更严格。其显式回读分配为
262,144 B/帧；100 次迭代测得相同的分配与释放字节数。CoreText 的 warm 1,000 次迭代探针同样
测得相同的分配与释放字节数，并使用固定 1 MiB atlas。详细分配数据及 CoreText 优化前后证据保存在
renderer/text crate 的性能记录中。Scene 和 layout 使用仅含安全代码的 crate，因此尚未验证
allocator hook 计数；作为替代，它们的有界条目数量和 steady-state 复用由契约测试保障。
