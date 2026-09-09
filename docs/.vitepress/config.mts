import { defineConfig, type DefaultTheme } from "vitepress";

const repository = "https://github.com/veltrum-dev/anmixiu";

const englishSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "Guide",
    items: [
      { text: "Getting started", link: "/guide/getting-started" },
      { text: "Core concepts", link: "/guide/core-concepts" },
    ],
  },
  {
    text: "Project",
    items: [
      { text: "Architecture", link: "/architecture" },
      { text: "Performance", link: "/performance" },
      { text: "Documentation workflow", link: "/contributing/documentation" },
      { text: "Legal and distribution", link: "/legal" },
    ],
  },
];

const chineseSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "指南",
    items: [
      { text: "快速开始", link: "/zh/guide/getting-started" },
      { text: "核心概念", link: "/zh/guide/core-concepts" },
    ],
  },
  {
    text: "项目",
    items: [
      { text: "架构", link: "/zh/architecture" },
      { text: "性能", link: "/zh/performance" },
      { text: "文档工作流", link: "/zh/contributing/documentation" },
      { text: "法律与分发", link: "/zh/legal" },
    ],
  },
];

export default defineConfig({
  base: process.env.DOCS_BASE ?? "/",
  title: "Anmixiu",
  description: "A native, cross-platform GUI workspace written in Rust.",
  lastUpdated: true,
  locales: {
    root: {
      label: "English",
      lang: "en-US",
      title: "Anmixiu",
      description: "A native, cross-platform GUI workspace written in Rust.",
      themeConfig: {
        nav: [
          { text: "Guide", link: "/guide/getting-started" },
          { text: "Architecture", link: "/architecture" },
          { text: "Performance", link: "/performance" },
        ],
        sidebar: englishSidebar,
        outline: { label: "On this page", level: [2, 3] },
        docFooter: { prev: "Previous page", next: "Next page" },
        editLink: {
          pattern: `${repository}/edit/main/docs/:path`,
          text: "Edit this page on GitHub",
        },
        lastUpdated: { text: "Last updated" },
        footer: {
          message: "Released under the MIT License.",
          copyright: "Copyright © Veltrum contributors",
        },
      },
    },
    zh: {
      label: "简体中文",
      lang: "zh-CN",
      link: "/zh/",
      title: "Anmixiu",
      description: "使用 Rust 编写的原生跨平台 GUI 工作空间。",
      themeConfig: {
        nav: [
          { text: "指南", link: "/zh/guide/getting-started" },
          { text: "架构", link: "/zh/architecture" },
          { text: "性能", link: "/zh/performance" },
        ],
        sidebar: chineseSidebar,
        outline: { label: "本页内容", level: [2, 3] },
        docFooter: { prev: "上一页", next: "下一页" },
        editLink: {
          pattern: `${repository}/edit/main/docs/:path`,
          text: "在 GitHub 上编辑此页",
        },
        lastUpdated: { text: "最后更新" },
        darkModeSwitchLabel: "外观",
        lightModeSwitchTitle: "切换到浅色主题",
        darkModeSwitchTitle: "切换到深色主题",
        sidebarMenuLabel: "菜单",
        returnToTopLabel: "返回顶部",
        langMenuLabel: "切换语言",
        skipToContentLabel: "跳到正文",
        footer: {
          message: "基于 MIT 许可证发布。",
          copyright: "版权所有 © Veltrum 贡献者",
        },
      },
    },
  },
  markdown: {
    codeCopyButtonTitle: "Copy code / 复制代码",
    container: {
      tipLabel: "TIP",
      warningLabel: "WARNING",
      dangerLabel: "DANGER",
      infoLabel: "INFO",
      detailsLabel: "Details",
    },
  },
  themeConfig: {
    search: {
      provider: "local",
      options: {
        locales: {
          zh: {
            translations: {
              button: { buttonText: "搜索文档", buttonAriaLabel: "搜索文档" },
              modal: {
                noResultsText: "没有找到相关结果",
                resetButtonTitle: "清除查询",
                footer: {
                  selectText: "选择",
                  navigateText: "切换",
                  closeText: "关闭",
                },
              },
            },
          },
        },
      },
    },
    socialLinks: [{ icon: "github", link: repository }],
  },
});
