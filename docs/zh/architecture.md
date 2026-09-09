# 架构

## 依赖方向

`anmixiu` 是精简的公共 facade。`anmixiu-core` 负责 Element、组件、公共 `Style`、事件、状态
查找、生命周期和调度契约。`anmixiu-reactive` 与 `anmixiu-scene` 是平台中立的叶子 crate。
`anmixiu-runtime` 在其上增加 Tokio 与绑定 owner 的本地 UI future 调度。
`anmixiu-platform` 负责内部 Taffy 适配、共享的 Element 到布局/Scene 投影、可移植输入与显示
模型、winit 事件循环，以及原生窗口与 wgpu surface 的连接。`anmixiu-render` 通过 wgpu 消费
平台中立的 Scene 数据，在 macOS 选择 Metal，在 Windows 选择 D3D12。`anmixiu-text` 在编译时
选择 CoreText 或 DirectWrite。未来桌面与移动后端仍复用相同的公共 UI 模型。

依赖始终从平台实现指向契约。core crate 永远不知道 winit、wgpu、Metal、D3D12、CoreText、
DirectWrite 或某个具体事件循环。Taffy 类型也不会进入公共 Element 或样式 API。

`anmixiu-platform` 与 `anmixiu-render` 禁止 unsafe Rust，并依赖上游窗口和 surface 实现。
原生文本 FFI 仅限 `anmixiu-text` 的目标专属模块；所有共享契约与 facade 都禁止 unsafe Rust。

## 更新流水线

在显式的已挂载 Element 渲染观察器中，读取 `Signal` 会记录一条 owner/source 边。写入会改变值，
并把每个仍然存活的依赖 owner 插入去重 dirty 队列；它不会扫描 Element 树，也不会立即渲染。
窗口随后请求一次 display turn。保留的生命周期树把每个 dirty owner 直接路由到对应的已挂载
Element，只重新渲染匹配的 owner，并复用全部 clean Element 快照；之后，完整 revision key
仍匹配的布局与 Scene 缓存条目也会被复用。clean turn 不提交 GPU 工作，普通 display turn 最多
present 一个 Scene 快照。渲染期间产生的 invalidation 会移动到下一 turn，并受到无限渲染循环保护。

卸载会移除依赖边，并取消尚未完成且绑定到 owner 的 UI future。应用状态和窗口状态只由对应 store
保留；同类型窗口状态的优先级高于应用状态。

## 原生窗口

`Window` 是可移植的创建配置。可选 title 表示“继承应用名称”；显式的空 `SharedString` 则表示
有意设置为空标题。原生适配器把配置解析为 `WindowInfo` 快照，并通过 `WindowHandle` 保留它。
渲染期间读取该快照会订阅已挂载 Element 的 owner，因此原生 resize、scale、focus、visibility 和
presentation mode 变化可以使对应帧失效，而应用代码无需同步查询 AppKit 或 Win32。

每个应用拥有一个有界窗口命令队列，以及一个以生成的 `WindowId` 为键的存活窗口 registry。命令在
执行前从队列移除，因此 mount 和 input 回调可以继续排入 open/update/close 操作，而不会重入
`RefCell` borrow。已经关闭的窗口会被移除，不会作为历史记录保留。其 root host 会卸载，owner
任务和事件订阅会取消，renderer/surface 状态会释放；过期 handle 只保留最终 `Closed` 快照。
只有最后一个窗口关闭后，原生应用循环才退出。

所有窗口共享应用唯一的 Tokio runtime、应用状态与类型化事件 router。每个窗口分别拥有自己的 root
host、响应式 owner registry、窗口状态、frame builder、renderer、viewport、pointer 状态和原生
display 调度。winit 把每个原生事件和合并后的 `RedrawRequested` 路由到对应的原生窗口 ID，再映射到
Anmixiu 不可变的 `WindowId`。因此 `Context::window()` 绑定 owner 且保持稳定，
`AppHandle::active_window()` 则会跟随原生焦点变化。

## Element 身份

`ElementId` 是调用方提供的语义身份，不是遍历索引。调用 `.id(...)` 会把 `ButtonElement` 等具体
builder 转换为 `Stateful<ButtonElement>`；包括 `.on_click(...)` 在内、需要跨多个输入阶段保持
状态的 API，只能在这一有状态 wrapper 上使用。wrapper 会被 `IntoElement` 擦除，但其 ID 会留在
Element 树中。

平台会把具名祖先 ID 组合为 `GlobalElementId`。为了渲染效率，命中区域保留紧凑的帧内 `HitId`，
但会映射回这条语义路径。因此，即便在 mouse-down 与 mouse-up 之间插入无关兄弟节点，也不会改变
click 的目标。同一渲染树中的重复语义路径会成为结构化构建错误。Taffy 的 `LayoutNodeId` 始终是
私有帧内索引，绝不会暴露为应用身份。

## 共享值与条件构建器

每个公共 UI 值都实现 `Element: Styled + Lifecycle`。内置的 `DivElement`、`TextElement` 和
`ButtonElement` 是具体的原语快速路径。自定义 Element 作为子元素时，会成为真实的中性布局盒，并
保留自己的类型化生命周期 host 与响应式 owner；其 `Lifecycle::render` 输出成为该布局盒下的内容。
`ParentElement`、`InteractiveElement` 和 `StatefulInteractiveElement` 仍是可选能力。
异构 `ElementNode` 投影对文档隐藏，只存在于内部 crate 边界之间。

无 key 的挂载身份是 `(parent, sibling position, Rust TypeId)`；`.id(...)` 用调用方提供的语义
身份替代 position。父元素以相同身份的新值重新渲染时，只会更新该 Element 的配置与观察器，不会重复
mount。首次成功 paint 后，`on_mount` 按父到子、从左到右调用；移除时，`on_unmount` 按完全相反
的顺序调用。Signal 写入直接路由到订阅 owner，未变化的兄弟节点不会执行 `render`。

内置元素提供最低限度但可用的默认值，而不是通用节点的别名。`DivElement` 默认为中性的 Flex 列容器；
`TextElement` 继承前景色并使用原生文本度量；`ButtonElement` 则提供可见的中性背景、白色标签、
36 像素最小高度、padding、1 像素 border、hover refinement、8 像素圆角、固有交叉轴尺寸、
居中标签、pointer cursor 和 2 像素 focus ring。border 是仅绘制的内嵌层；hover refinement
可以改变背景、前景与边框颜色，而不会使 Taffy 布局失效。winit 的 `CursorLeft` 事件会在指针离开
原生窗口时清除 hover。`Styled` 覆盖始终具有最终决定权；品牌变体与 theme 属于未来组件层。

应用和窗口 typography 都是按字段可选的默认值。窗口字体族或字号会覆盖对应应用字段，同时另一个字段
仍可继续 fallback。两层都未指定某字段时，平台提供原生 UI 字体和可见默认字号：CoreText 解析
macOS 默认值；Windows 读取当前 non-client message/UI 字体族与逻辑字号，供 DirectWrite 使用。
系统设置变化后，Windows 会刷新这些派生值，并使嵌入旧度量的文本、布局与 Scene 结果失效。省略配置
永远不会产生字面上的零字号计算字体。

颜色支持归一化浮点 `rgb`/`rgba` 构造器，以及 const 整数 `hex(0xRRGGBB)` /
`hex_with_alpha(0xRRGGBBAA)` 构造器。分离的函数消除了前导零值歧义，并让字符串解析离开
render/style 热路径。`Styled` 与 `StyleRefinement` 的颜色 setter 接受 `impl Into<Color>`；
直接 `u32` 值严格表示 `0xRRGGBB`，alpha 必须通过 `hex_with_alpha` 显式指定。

`px(...)` 返回具体 `Pixels` 单位。接受像素的 builder 接受 `impl Into<Pixels>`；裸 `f32`/
`u32` 值仍表示逻辑像素，因此 `.width(320.0)` 等价于 `.width(px(320.0))`。未来的百分比或相对
单位将使用不同的具体类型与具名构造器，而不是让 `px(...)` 返回擦除后的单位容器。

`SharedString` 是 Element 文本、按钮标签与具名 `ElementId` 变体使用的公共不可变字符串。它封装
`SmolStr`：静态值使用借用，短值内联存储，长值由 clone 共享。`shared_format!` 直接格式化到这一
存储。渲染后的 Element 树作为主线程 `Rc<ElementNode>` 快照保留，因此平台布局/paint 不必深拷贝树。

`FluentBuilder` 提供 `when`、`when_else`、`when_some` 与 `when_none`。这些方法保留具体
builder 类型，并根据条件应用 closure；它们在支持可选子树、样式和处理器的同时，避免引入擦除后的
条件 Element 类型。

可选的 `anmixiu` `macros` feature 提供 `#[derive(Element)]`。标有 `#[element(style)]` 的
字段代理 `Styled`；再加入 `parent` 标记便同时代理 `ParentElement`。派生不会生成生命周期或交互
实现，因此 `Lifecycle::render` 与其他能力仍保持显式，能力隔离也不会被破坏。

## 类型化事件

`Lifecycle::bind_events` 是默认空操作钩子，在第一帧 paint 后调用一次。其 `EventBindings` 值会
保留每个 RAII `Subscription` 直到卸载。事件 payload 是通过 `TypeId` 路由的普通 Rust 值，而不是
字符串 topic。`EventScope::Owner` 精确匹配事件来源的已挂载 Element owner；
`EventScope::Window` 把交付限制在当前窗口，`EventScope::App` 则通过应用拥有的 router 跨窗口广播。

订阅带有 `EventPriority`。较高优先级先分发；相同优先级保持注册顺序。嵌套 emission 使用 FIFO
队列，具有 pending 硬容量和每个同步 turn 的交付次数上限，因此反馈循环不能独占 UI 线程。调用回调时
不持有 router 的 mutable borrow，所以 handler 可以安全地 emit 或 unsubscribe。panic guard 会
恢复进行中的回调，并丢弃它排入的嵌套工作。取消订阅也会立即取消注册 owner cleanup，使 owner 生命周期
内的动态 churn 保持有界。

router 为诊断提供只读订阅元数据快照；事件 payload 值永远不会作为状态保留。

未来可复用自定义控件的扩展点，是构建在 Element builder 之上的类型化组件 registry。一次注册会把
Rust 组件构造器和显式元数据（名称、版本、支持的属性与生命周期 owner）关联到应用拥有的 registry；
它不会解析 tag 名、修改全局 namespace 或引入 shadow DOM。组件仍返回普通 `Element` 值，并保留
相同的 Signal/owner/lifecycle 契约。这样可以为库作者提供可发现性与命名空间，而无需重建字符串类型的
全局 Element registry。

## 异步边界

每个应用拥有一个 Tokio 多线程 runtime，用于 timer 与 I/O readiness。UI future 使用有界
`async-task` 队列，只由运行 winit 原生事件循环的线程 poll。未来后端会提供等价的原生 UI executor，
而不改变 owner 契约。
`Context::spawn` 把 future 绑定到当前已挂载 Element owner，并返回结构化 `SpawnError`，因此调用
方不会保留或 detach task handle，容量或生命周期拒绝也不会变成框架 panic。Tokio runtime 使用两个
worker：当一个 I/O 任务延迟时仍然保持多线程，又不会按每个逻辑 CPU 增加空闲 UI 线程。生命周期方法与
render 始终保持同步。

## 缓存契约

- 布局：以 root 身份、结构/style/measure revision、逻辑 viewport 与 scale 为键；每个 engine
  只保留一个当前条目。
- Scene：以 node、paint/layout revision 与 scale 为键；使用有界 LRU 容量。
- glyph atlas：以字体身份、字号、scale、glyph 与量化 subpixel phase 为键；页面尺寸和条目容量固定，
  generation 变化会强制 texture 刷新。观察到 repack 的帧会使用单一 atlas generation 重建；如果一帧
  的 glyph 并集无法在有界页面内稳定，则返回结构化错误，而不是提交过期 UV。
- renderer 资源：wgpu 保留 pipeline 集，以及以 atlas id 和 generation 为键、有硬容量的 glyph
  atlas LRU。合成纹理在物理尺寸、格式或嵌套深度变化时替换，并受 256 MiB 硬预算约束。

## 背景效果与合成

`Style::backdrop_blur` 仅影响 paint，并以逻辑像素存储 Gaussian sigma。共享投影会在 Element 的
border 和 background 命令之前，立即发出有序 `DrawCommand::BackdropBlur`。其语义输入是当前
Scene 中所有在它之前的命令；包括 Element 自身 fill、text 与 descendant 在内的后续命令保持未过滤。
非正数或非有限样式值不会发出效果；平台 renderer 会把更大的有限 sigma 限制在共享的 64 逻辑像素上限。

effect 命令会选择 compositor 路径。没有 effect 的 Scene 保持 direct surface render pass，且不
创建中间 color texture。wgpu renderer 使用有界 scene texture、一对可复用 blur texture，以及每层
嵌套 filter 对应的一张透明 texture。它保留 Scene 顺序、裁剪替换与合成 pass，将每类 effect 限制为
64 个、filter 嵌套限制为 8 层，并在分配 GPU texture 前拒绝超过 256 MiB 的计划。

## 原生缩放与刷新

winit 以精确物理像素报告 client size，并单独报告 scale 变化。Anmixiu 用两者派生唯一逻辑
viewport，把 wgpu surface 配置为相同物理尺寸，并且只在 `RedrawRequested` 时渲染。`Occluded`
会停止最小化或隐藏窗口的工作。surface timeout/occlusion 会跳过一帧，outdated surface 会重新配置，
lost surface 会重新创建。Anmixiu 自身不拥有 `CAMetalLayer` 或 DXGI swap chain。

文本放置使用位置感知的原生 glyph cache。布局在 rasterization 前完成；最终 glyph 在当前 DPR 下的
位置会选择四种水平 subpixel mask 之一。geometry 使用物理像素 `floor(x)` / `round(y)` origin，
选中的 mask 则保留原生小数 advance，因此 wgpu 可以一对一采样 atlas texel，而不破坏
kerning。垂直放置会先 round 共享 line baseline，再应用每个 glyph 的整数 bearing；不会独立 round
每个 glyph top，所以混合 script 与 fallback font 仍处于同一 baseline。

CoreText rasterize 到 RGB32 CGContext，遵循平台 antialiasing/smoothing，并由文本后端提取与
renderer 无关的 A8 mask。glyph UV/quad 保留透明的两像素安全边界，避免低 DPI coverage 被裁剪。
scale 与最终定位后的 origin 都属于有界 text/atlas cache 契约。

DirectWrite 对完整文本布局进行 shaping，由原生 engine 提供 script fallback、双向顺序与 glyph
advance。每个 run 的 font-file 身份、face index、simulation、em size、scale、glyph id 与量化 X/Y
subpixel phase 共同构成有界 atlas key。DirectWrite 生成 ClearType coverage，后端将其缩减为带透明
两像素边界、与 renderer 无关的 A8 mask。renderer 把页面上传为 R8 texture，并且只把它作为
opacity mask 采样。

## 未来平台

仓库特意不为尚未支持的操作系统建立占位 crate 或公共 API。Linux 与 FreeBSD 将同时编译 Wayland 和
X11 支持并在运行时选择，优先使用 Vulkan，无法使用时回退到 GL。长期移动端路线包括原生 iOS 与 Android
集成。这些后端应复用平台中立的 `anmixiu-core`、`anmixiu-reactive`、`anmixiu-scene` 和
`anmixiu-runtime` 契约，同时把窗口与生命周期、输入、文本和渲染工作映射到各平台的原生 API。
