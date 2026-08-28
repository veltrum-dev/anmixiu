# Browser native reference

This directory is a small, dependency-free comparison page for browser behavior. It deliberately
uses native HTML elements and native `overflow: auto` scroll containers so the browser's own X/Y
scrollbars, trackpad momentum, wheel handling, focus behavior, and default controls are easy to
compare with Anmixiu.

Open [`index.html`](index.html) directly, or serve the directory so browser security policies never
get in the way of future reference fixtures:

```sh
python3 -m http.server 8000 --directory browser-reference
```

Then visit <http://localhost:8000>.

## What is covered

- A vertical native scroll area with 120 rows.
- A two-axis native scroll area with a deliberately wide table and 120 rows.
- Native `scrollBy({ behavior: "smooth" })` buttons, kept separate from wheel/trackpad behavior.
- A small baseline of headings, links, lists, code, buttons, inputs, select, and progress controls.

Keep new browser comparisons in this directory, preferably as a focused section in `index.html`, so
it remains the shared visual baseline for future Anmixiu defaults.
