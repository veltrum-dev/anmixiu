---
layout: home

hero:
  name: Anmixiu
  text: Native Rust UI, ordinary Rust values
  tagline: A cross-platform GUI workspace with fine-grained reactivity, native text, and native GPU rendering.
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: Core concepts
      link: /guide/core-concepts
    - theme: alt
      text: View on GitHub
      link: https://github.com/veltrum-dev/anmixiu

features:
  - title: Ordinary Rust API
    details: Compose concrete, strongly typed elements with chainable builders and explicit capabilities.
  - title: Fine-grained updates
    details: Signal reads subscribe mounted owners; writes schedule deduplicated, frame-batched updates.
  - title: Native backends
    details: The current implementation uses winit plus wgpu (Metal) and CoreText on macOS, and winit plus wgpu (D3D12) and DirectWrite on Windows 10 or later.
---

> [!CAUTION]
> Anmixiu is experimental. Public APIs and platform contracts may change before a stable release.
