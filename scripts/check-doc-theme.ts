import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = process.cwd();
const themePath = resolve(root, "docs/.vitepress/theme/index.ts");
const configPath = resolve(root, "docs/.vitepress/config.mts");
const examplePath = resolve(root, "docs/.vitepress/theme/components/HomeCodeExample.vue");
const codeWindowsPath = resolve(root, "docs/.vitepress/theme/code-windows.css");
const theme = readFileSync(themePath, "utf8");
const config = readFileSync(configPath, "utf8");

function check(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

check(existsSync(examplePath), "the home hero must provide a code example");
const example = readFileSync(examplePath, "utf8");
check(
  theme.includes('"home-hero-image": () => h(HomeCodeExample)'),
  "the code example must be mounted only in the home hero image slot",
);
check(!theme.includes("custom.css"), "the theme must use VitePress default CSS");
check(!config.includes("anmixiu-squad"), "the default theme must not run the accent switcher script");
check(!config.includes("theme-color"), "the default theme must not add custom head metadata");
check(!existsSync(resolve(root, "docs/.vitepress/theme/custom.css")), "custom theme CSS must be removed");
check(example.includes("let count = self.count.get();"), "the hero snippet must use the real Counter example API");
check(example.includes("navigator.clipboard.writeText"), "the hero code example must be copyable");
check(example.includes("home-code-example__traffic"), "the hero example must use a macOS-style title bar");
check(example.includes('class="syntax-keyword"'), "the Rust example must render syntax-highlighted tokens");
check(existsSync(codeWindowsPath), "Markdown code blocks must have the shared macOS window stylesheet");
const codeWindows = readFileSync(codeWindowsPath, "utf8");
check(theme.includes('import "./code-windows.css"'), "the code-window stylesheet must be loaded by the theme");
check(codeWindows.includes(".vp-doc div[class*="), "the code-window style must target fenced Markdown blocks");
check(codeWindows.includes("var(--shiki-dark"), "light pages must keep readable dark-surface syntax colors");

console.log("VitePress default theme contract passed.");
