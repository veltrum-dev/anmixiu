import { readdir } from "node:fs/promises";
import { join, relative } from "node:path";

const docsRoot = join(import.meta.dir, "..", "docs");
const chineseRoot = join(docsRoot, "zh");

async function markdownFiles(root: string, excludedDirectories = new Set<string>()): Promise<string[]> {
  const files: string[] = [];

  async function visit(directory: string): Promise<void> {
    const entries = await readdir(directory, { withFileTypes: true });

    for (const entry of entries) {
      if (entry.isDirectory() && excludedDirectories.has(entry.name)) {
        continue;
      }

      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        await visit(path);
      } else if (entry.isFile() && entry.name.endsWith(".md")) {
        files.push(relative(root, path));
      }
    }
  }

  await visit(root);
  return files.sort();
}

const englishFiles = await markdownFiles(docsRoot, new Set([".vitepress", "zh"]));
const chineseFiles = await markdownFiles(chineseRoot);
const englishSet = new Set(englishFiles);
const chineseSet = new Set(chineseFiles);

const missingChinese = englishFiles.filter((path) => !chineseSet.has(path));
const missingEnglish = chineseFiles.filter((path) => !englishSet.has(path));

if (missingChinese.length > 0 || missingEnglish.length > 0) {
  for (const path of missingChinese) {
    console.error(`Missing Simplified Chinese page: docs/zh/${path}`);
  }

  for (const path of missingEnglish) {
    console.error(`Missing English page: docs/${path}`);
  }

  process.exit(1);
}

console.log(`Locale parity verified for ${englishFiles.length} documentation pages.`);
