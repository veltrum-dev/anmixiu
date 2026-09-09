# 核心概念

## Element 是普通 Rust 值

每个公共 UI 值都实现 `Element: Styled + Lifecycle`。内置元素使用具体的原语类型；自定义
Element 作为子元素时，仍保留独立的挂载身份和响应式 owner。

各项能力保持分离：

- `Styled` 提供样式构建器。
- `ParentElement` 提供子元素构建器。
- `InteractiveElement` 提供语义身份。
- `StatefulInteractiveElement` 提供跨输入阶段的处理器。

这样，文本节点不必假装拥有自己并不需要的能力。

## 身份具有语义

`ElementId` 由应用提供。调用 `.id(...)` 会把具体元素转换为 `Stateful<E>`，使交互在不同帧之间
拥有稳定身份。紧凑的布局、绘制与命中测试索引始终属于内部实现。

没有显式 ID 时，挂载身份由父元素、兄弟位置与 Rust 类型共同确定。重新配置相同身份不会再次触发挂载。

## Signal 安排按帧批处理的更新

只有当 Element 在观察器中渲染时，读取 `Signal` 才会建立订阅。写入会修改值、把每个依赖它的已挂载
owner 标记为 dirty，并请求下一帧；它不会扫描 Element 树，也不会同步执行渲染。

```rust
let count = Signal::new(0_u32);
let value = count.get();
count.set(value + 1);
```

owner 卸载时，其订阅会被移除，尚未完成且绑定到该 owner 的 UI 任务也会被取消。

## 生命周期保持同步

`Lifecycle::render` 接收 `&self`。状态变化通过 `Signal` 完成，render 和生命周期钩子不会变成
async。首次成功绘制后，`on_mount` 只执行一次；移除时，`on_unmount` 按树的逆序执行一次。

UI future 通过 `Context::spawn` 创建。它们只在原生 UI 线程恢复执行，并绑定到创建它们的已挂载
owner。

## 样式使用可移植值

公共 `Style` 归 `anmixiu-core` 所有，Taffy 留在布局适配层之后。裸 `f32` 和 `u32` 长度表示
逻辑像素，`px(...)` 则返回显式的 `Pixels` 类型。

颜色使用归一化通道或含义明确的整数格式：

```rust
let opaque = Color::hex(0x33_66_FF);
let translucent = Color::hex_with_alpha(0x33_66_FF_80);
```

直接从 `u32` 转换时只接受 24 位 `0xRRGGBB`；透明度始终需要显式表达。

## 原生后端共享同一个公共模型

macOS 组合 winit、wgpu/Metal 与 CoreText；Windows 10 及以上组合 winit、wgpu/D3D12 与 DirectWrite。
平台中立 crate 永远不会反向依赖这些实现，不支持的平台专属能力也不能静默退化为空操作。

完整的依赖关系与更新流水线见[架构](../architecture)。
