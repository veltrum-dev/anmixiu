# 文档工作流

文档是一个双语 VitePress 站点。英文页面直接位于 `docs/`，简体中文页面则在 `docs/zh/` 下以相同的
相对路径镜像。

## 前置要求

- Bun 1.4.0，与 `package.json` 和 CI 中锁定的版本一致。
- 修改示例或公共 API 文档时，需要 Rust 1.89 或更高版本。

安装锁定的依赖：

```sh
bun install --frozen-lockfile
```

## 本地开发

启动支持热更新的开发服务器：

```sh
bun run docs:dev
```

构建并预览生产输出：

```sh
bun run docs:build
bun run docs:preview
```

本地开发和预览时，`DOCS_BASE` 默认为 `/`。GitHub Actions 构建 GitHub Pages artifact 时会把它
设置为 `/anmixiu/`，匹配仓库的项目路径。

## 双语页面契约

每个 Markdown 页面都必须在两种语言中以相同相对路径存在。例如：

```text
docs/guide/getting-started.md
docs/zh/guide/getting-started.md
```

提交变更前运行语言完整性检查：

```sh
bun run docs:locales
```

标题与信息架构应保持一致，但翻译应传达含义，而不是机械复制句式。Rust 标识符、命令、数字测量值与
平台契约必须保持准确。

## 代码示例

描述公共行为的示例必须能在 Rust workspace 中编译，或者由针对性的 doctest 覆盖。较长的演示应优先
使用仓库 `examples/` 中的程序，避免让文档成为第二份未经测试的实现。

## 持续交付

Pull Request 会根据 `bun.lock` 安装依赖、检查双语页面完整性并构建站点。`main` 的构建成功后，
产物会作为 GitHub Pages artifact 上传，并部署到 `github-pages` environment。
