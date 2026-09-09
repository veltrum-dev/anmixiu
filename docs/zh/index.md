---
layout: home

hero:
  name: Anmixiu
  text: 原生 Rust UI，普通 Rust 值
  tagline: 具备细粒度响应式系统、原生文本和原生 GPU 渲染的跨平台 GUI 工作空间。
  actions:
    - theme: brand
      text: 快速开始
      link: /zh/guide/getting-started
    - theme: alt
      text: 核心概念
      link: /zh/guide/core-concepts
    - theme: alt
      text: 在 GitHub 上查看
      link: https://github.com/veltrum-dev/anmixiu

features:
  - title: 普通 Rust API
    details: 使用可链式构建器与显式能力组合具体、强类型的元素。
  - title: 细粒度更新
    details: Signal 读取订阅已挂载 owner，写入则安排去重并按帧批处理的更新。
  - title: 原生后端
    details: 当前实现在 macOS 使用 winit、wgpu（Metal）和 CoreText，在 Windows 10 及以上使用 winit、wgpu（D3D12）和 DirectWrite。
---

::: warning 实验性项目
Anmixiu 仍处于实验阶段。稳定版本发布前，公共 API 和平台契约可能发生变化。
:::
