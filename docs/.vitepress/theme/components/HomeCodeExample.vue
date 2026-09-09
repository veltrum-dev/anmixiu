<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { useData } from "vitepress";

const { lang } = useData();
const copied = ref(false);
let resetTimer: ReturnType<typeof setTimeout> | undefined;

const code = `let count = self.count.get();
let increment = self.count.clone();

div()
    .padding(px(28.0))
    .gap(px(18.0))
    .child(text(shared_format!("Count {count}")))
    .child(
        button("Increment")
            .id("increment")
            .on_click(move || {
                increment.update(|value| *value += 1)
            }),
    )`;

const highlightedCode = `<span class="syntax-keyword">let</span> count = <span class="syntax-keyword">self</span>.count.<span class="syntax-function">get</span>();
<span class="syntax-keyword">let</span> increment = <span class="syntax-keyword">self</span>.count.<span class="syntax-function">clone</span>();

<span class="syntax-function">div</span>()
    .<span class="syntax-function">padding</span>(<span class="syntax-function">px</span>(<span class="syntax-number">28.0</span>))
    .<span class="syntax-function">gap</span>(<span class="syntax-function">px</span>(<span class="syntax-number">18.0</span>))
    .<span class="syntax-function">child</span>(<span class="syntax-function">text</span>(<span class="syntax-macro">shared_format!</span>(<span class="syntax-string">"Count {count}"</span>)))
    .<span class="syntax-function">child</span>(
        <span class="syntax-function">button</span>(<span class="syntax-string">"Increment"</span>)
            .<span class="syntax-function">id</span>(<span class="syntax-string">"increment"</span>)
            .<span class="syntax-function">on_click</span>(<span class="syntax-keyword">move</span> || {
                increment.<span class="syntax-function">update</span>(|value| *value += <span class="syntax-number">1</span>)
            }),
    )`;

const labels = computed(() =>
  lang.value.startsWith("zh")
    ? { region: "Counter 示例代码", copy: "复制", copied: "已复制" }
    : { region: "Counter example code", copy: "Copy", copied: "Copied" },
);

async function copyCode(): Promise<void> {
  await navigator.clipboard.writeText(code);
  copied.value = true;
  if (resetTimer) clearTimeout(resetTimer);
  resetTimer = setTimeout(() => {
    copied.value = false;
  }, 1800);
}

onBeforeUnmount(() => {
  if (resetTimer) clearTimeout(resetTimer);
});
</script>

<template>
  <section class="home-code-example" :aria-label="labels.region">
    <header class="home-code-example__header">
      <span class="home-code-example__traffic" aria-hidden="true">
        <i class="traffic-dot traffic-dot--close" />
        <i class="traffic-dot traffic-dot--minimize" />
        <i class="traffic-dot traffic-dot--maximize" />
      </span>
      <span class="home-code-example__filename">counter.rs</span>
      <button type="button" @click="copyCode">
        {{ copied ? labels.copied : labels.copy }}
      </button>
    </header>
    <pre tabindex="0"><code v-html="highlightedCode" /></pre>
    <span class="visually-hidden" aria-live="polite">{{ copied ? labels.copied : "" }}</span>
  </section>
</template>

<style scoped>
.home-code-example {
  --home-code-bg: var(--vp-code-block-bg);
  --home-code-header-bg: var(--vp-c-bg-elv);
  --home-code-border: var(--vp-c-border);
  --home-code-text: var(--vp-code-block-color);
  --home-code-muted: var(--vp-c-text-3);
  --home-code-button-bg: var(--vp-code-copy-code-bg);
  --home-code-button-border: var(--vp-code-copy-code-border-color);
  --home-code-button-hover-border: var(--vp-code-copy-code-hover-border-color);
  --home-code-button-hover-text: var(--vp-c-text-1);
  --home-code-keyword: var(--vp-c-brand-1);
  --home-code-function: var(--vp-c-brand-2);
  --home-code-macro: var(--vp-c-tip-1);
  --home-code-string: var(--vp-c-success-1);
  --home-code-number: var(--vp-c-warning-1);
  width: min(100%, 520px);
  border: 1px solid var(--home-code-border);
  border-radius: 14px;
  background: var(--home-code-bg);
  box-shadow: var(--vp-shadow-4);
  overflow: hidden;
}

.home-code-example__header {
  display: grid;
  min-height: 44px;
  grid-template-columns: 1fr auto 1fr;
  align-items: center;
  padding: 0 12px;
  border-bottom: 1px solid var(--home-code-border);
  background: var(--home-code-header-bg);
  color: var(--home-code-muted);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
}

.home-code-example__traffic {
  display: flex;
  align-items: center;
  gap: 7px;
  justify-self: start;
}

.traffic-dot {
  width: 11px;
  height: 11px;
  border-radius: 50%;
}

.traffic-dot--close {
  background: #ff5f57;
}

.traffic-dot--minimize {
  background: #febc2e;
}

.traffic-dot--maximize {
  background: #28c840;
}

.home-code-example__filename {
  justify-self: center;
}

button {
  min-height: 30px;
  justify-self: end;
  padding: 0 9px;
  border: 1px solid var(--home-code-button-border);
  border-radius: 6px;
  background: var(--home-code-button-bg);
  color: var(--vp-c-text-2);
  font-size: 11px;
  font-weight: 600;
}

button:hover {
  border-color: var(--home-code-button-hover-border);
  color: var(--home-code-button-hover-text);
}

pre {
  max-height: 320px;
  margin: 0;
  padding: 18px 20px 20px;
  overflow: auto;
  background: var(--home-code-bg);
  color: var(--home-code-text);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  line-height: 1.65;
  text-align: left;
  tab-size: 4;
}

code {
  font-family: inherit;
}

code :deep(.syntax-keyword) {
  color: var(--home-code-keyword);
}

code :deep(.syntax-function) {
  color: var(--home-code-function);
}

code :deep(.syntax-macro) {
  color: var(--home-code-macro);
}

code :deep(.syntax-string) {
  color: var(--home-code-string);
}

code :deep(.syntax-number) {
  color: var(--home-code-number);
}

@media (max-width: 639px) {
  .home-code-example {
    border-radius: 8px;
  }

  pre {
    padding: 14px 16px 16px;
    font-size: 11px;
  }
}
</style>
