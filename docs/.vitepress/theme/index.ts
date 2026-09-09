import { h } from "vue";
import type { Theme } from "vitepress";
import DefaultTheme from "vitepress/theme";
import HomeCodeExample from "./components/HomeCodeExample.vue";
import "./code-windows.css";

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      "home-hero-image": () => h(HomeCodeExample),
    });
  },
} satisfies Theme;
