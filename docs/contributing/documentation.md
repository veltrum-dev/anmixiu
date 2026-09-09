# Documentation workflow

The documentation is a bilingual VitePress site. English pages live directly under `docs/`, and
Simplified Chinese pages mirror the same relative paths under `docs/zh/`.

## Prerequisites

- Bun 1.4.0, matching the version pinned in `package.json` and CI.
- Rust 1.89 or newer when changing examples or public API documentation.

Install the locked dependencies:

```sh
bun install --frozen-lockfile
```

## Local development

Start the development server with hot reload:

```sh
bun run docs:dev
```

Build and preview the production output:

```sh
bun run docs:build
bun run docs:preview
```

`DOCS_BASE` defaults to `/` for local development and preview. GitHub Actions sets it to
`/anmixiu/` while building the artifact for the repository's GitHub Pages project path.

## Bilingual page contract

Every Markdown page must exist in both locales at the same relative path. For example:

```text
docs/guide/getting-started.md
docs/zh/guide/getting-started.md
```

Run the parity check before submitting changes:

```sh
bun run docs:locales
```

Keep headings and information architecture aligned, but translate meaning rather than sentence
shape. Rust identifiers, commands, numeric measurements, and platform contracts must remain exact.

## Code examples

Examples describing public behavior must compile in the Rust workspace or be covered by a focused
doctest. Prefer the repository's `examples/` programs for longer walkthroughs so documentation does
not become a second, untested implementation.

## Continuous delivery

Pull requests install dependencies from `bun.lock`, check locale parity, and build the site. A
successful build from `main` is uploaded as a GitHub Pages artifact and deployed to the
`github-pages` environment.
