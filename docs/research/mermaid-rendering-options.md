# Mermaid rendering options for Pecan

**Status:** research note, 2026-08-27
**Scope:** official Mermaid documentation/source/package metadata; official React, react-markdown, unified, MDN, Graphviz, and candidate renderer repositories. No application code was changed.

## Executive recommendation

For Pecan's current Vite/React chat UI, use the official `mermaid` browser package behind an **app-level dynamic import**, and invoke `mermaid.render()` from a dedicated React component only for fenced blocks whose language is `mermaid`.

Keep the current `react-markdown` pipeline and its `code` component override. A Mermaid fence is already delivered to that override as a `code` element with a `language-mermaid` class, so Pecan does not need a new remark/rehype plugin merely to recognize or replace the fence. `react-markdown` explicitly supports replacing HTML-equivalent elements with components and describes its pipeline as markdown → mdast → remark → hast → rehype → React components ([react-markdown README](https://github.com/remarkjs/react-markdown/blob/fda7fa560bec901a6103e195f9b1979dab543b17/readme.md#architecture), [components API](https://github.com/remarkjs/react-markdown/blob/fda7fa560bec901a6103e195f9b1979dab543b17/readme.md#components)).

Recommended defaults:

- `mermaid.initialize({ startOnLoad: false, securityLevel: "strict" })` once per loaded module.
- Use `render(uniqueId, source)` rather than `run()` so React retains ownership of the outer component and only the diagram component mutates its private container.
- Keep inspectable source, copy affordance, loading state, and a readable error fallback.
- Cache the dynamic import/module initialization; do not import Mermaid at the top of `chat-markdown.tsx`.
- Ignore stale async results after unmount/source changes. Mermaid serializes multiple `render()` calls but does not expose cancellation through the public browser API ([Mermaid source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L397-L432)).
- If long threads commonly contain many diagrams, add viewport-level deferral after the first implementation. `IntersectionObserver` is explicitly intended for asynchronously detecting visibility and lazy-loading content ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Intersection_Observer_API)).
- Do not add ELK, a WASM Graphviz package, or a native Mermaid reimplementation in the first slice. They solve different problems and increase compatibility or payload risk.

This fits the live repository: `web/package.json` currently uses React 19.2.6, react-markdown 10.1.0, and Vite 8; `web/src/main.tsx` uses `createRoot` rather than hydration; and `web/src/components/chat-markdown.tsx` already routes fenced code through `CodeFence` while deliberately leaving diagrams as source.

## 1. Mermaid browser APIs

### `initialize(config)`

`initialize` sets Mermaid's site-wide configuration and should run before `run`; Mermaid's source describes it as configuration for subsequent rendering ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L216-L224)). The official usage guide calls `initialize` the preferred configuration path ([docs](https://mermaid.js.org/config/usage.html#configuration)).

Important integration consequences:

- Configuration is module-global, not per React component. Initialize once with a stable policy.
- Set `startOnLoad: false` for manual React integration. Otherwise Mermaid installs a page-load handler and, by default, calls `run()` over `.mermaid` elements ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L282-L301), [docs](https://mermaid.js.org/config/usage.html#using-mermaid-run)).
- Font metrics affect node dimensions. Mermaid's usage guide warns that rendering before dynamically loaded fonts are ready can produce labels outside their boxes; its default page-load integration waits for all assets, including fonts ([docs](https://mermaid.js.org/config/usage.html#labels-out-of-bounds)). A React renderer should either use a stable system font or wait for the relevant font readiness before first render if exact label bounds matter.

### `run(options)`

`run()` is the preferred replacement for deprecated `init()`. It finds nodes by `querySelector` (default `.mermaid`) or accepts an explicit node list; options include `postRenderCallback` and `suppressErrors` ([official docs](https://mermaid.js.org/config/usage.html#using-mermaid-run), [source/type](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L35-L54)). `init()` is deprecated in Mermaid v10 in favor of `initialize` plus `run` ([docs](https://mermaid.js.org/config/usage.html#calling-mermaid-init-deprecated)).

`run()` is convenient for static DOM pages, but it is not the best primitive for Pecan's React renderer:

- It reads each target's `innerHTML`, renders it, replaces the element's `innerHTML`, and optionally binds returned event handlers ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L173-L205)).
- It marks targets with `data-processed="true"` and skips already marked nodes on later runs ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L173-L183)). A React update that reuses an element can therefore need explicit marker/source management.
- Broad selector scans and external DOM mutation create two owners for the same subtree. Targeted `render()` confines imperative work to the component's private ref.

### `render(id, text, container?)`

`render()` is the programmatic API for one definition. It returns a promise for `{ svg, diagramType, bindFunctions? }`; the official example inserts `svg` and then calls `bindFunctions` after insertion ([docs](https://mermaid.js.org/config/usage.html#api-usage), [source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L386-L433)).

Caveats:

1. **Calls are serialized.** The high-level API queues multiple `render()` calls and executes them serially to avoid shared-state races ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaid.ts#L397-L432)). A thread with many diagrams can form a render backlog even when React starts effects concurrently.
2. **Use a unique, DOM-safe ID per rendered instance.** Mermaid uses the ID to construct temporary wrapper, iframe, selector, and SVG IDs ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaidAPI.ts#L495-L555)). Do not use raw diagram text as an ID.
3. **The renderer uses the browser DOM even when only an SVG string is requested.** It selects `document.body`, creates temporary DOM/style/SVG nodes, measures/draws, serializes `innerHTML`, and removes temporary elements ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaidAPI.ts#L465-L655)).
4. **Interactive diagrams require `bindFunctions`.** Call it only after the rendered SVG has been inserted into the component's container ([docs](https://mermaid.js.org/config/usage.html#binding-events)). Under `strict`, click functionality is disabled, so this is mainly relevant if a future trusted-only mode permits interactions.
5. **Errors are asynchronous and source changes can race.** Keep the source visible or offer a source toggle, and only commit a result if the component is still mounted and the render corresponds to the latest source.
6. **React development Strict Mode performs an extra effect setup/cleanup cycle.** The component must tolerate duplicate starts and cleanup correctly ([React `useEffect`](https://react.dev/reference/react/useEffect#caveats)). Mermaid's queue makes calls safe from Mermaid's shared-state perspective, but Pecan must still ignore stale results.

## 2. Security model

Mermaid's `securityLevel` values are materially different:

| Level | Official behavior |
|---|---|
| `strict` (default) | Encodes HTML tags in text and disables click functionality. |
| `loose` | Allows HTML tags and click functionality. |
| `antiscript` | Allows HTML except script elements and enables clicks. |
| `sandbox` | Renders in a sandboxed iframe; prevents JavaScript in that context but can impair links, popups, and other interaction. |

Source: [Mermaid config schema](https://mermaid.js.org/config/schema-docs/config-properties-securitylevel.html).

For chat/agent-authored Mermaid, retain **`strict`**. Do not switch to `loose` or `antiscript` for richer labels. Mermaid's render path sanitizes serialized SVG with DOMPurify for every non-`loose`, non-sandbox render; sandbox mode instead returns iframe-wrapped output ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaidAPI.ts#L622-L646)).

`strict` is necessary but does not make the integration free of browser sink concerns:

- Mermaid's documented integration assigns the returned string with `element.innerHTML = svg` ([docs](https://mermaid.js.org/config/usage.html#api-usage)).
- `innerHTML` is an injection sink; MDN recommends `TrustedHTML` and a Trusted Types-enforcing CSP for untrusted strings ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Element/innerHTML)).
- A CSP with `require-trusted-types-for 'script'` rejects string assignments to `innerHTML` unless a default policy or trusted value is supplied ([MDN](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/require-trusted-types-for)). Mermaid itself currently uses `innerHTML` internally as part of rendering, so a future strict Trusted Types policy must be tested against the exact Mermaid release rather than assuming only Pecan's final insertion needs adaptation.

Practical policy:

- Treat Mermaid source as untrusted external input.
- Pin a Mermaid version through `package-lock.json`; do not use `@latest` CDN imports.
- Keep `securityLevel: "strict"`, preserve the existing safe `defaultUrlTransform` for normal Markdown links, and do not enable raw HTML merely for diagrams.
- Apply a reasonable source-size limit before invoking Mermaid. Mermaid itself has a `maxTextSize` setting, but early rejection also avoids loading/queuing obviously excessive input.
- If Pecan later enables links/click callbacks in diagrams, make that a separate security decision rather than weakening the global default.

`react-markdown` is safe by default because it builds React elements rather than using `dangerouslySetInnerHTML`, but its own documentation warns that plugins and custom components can make a pipeline unsafe and recommends `rehype-sanitize` when complete post-plugin safety is required ([README](https://github.com/remarkjs/react-markdown/blob/fda7fa560bec901a6103e195f9b1979dab543b17/readme.md#security)). The Mermaid subtree is an explicit imperative exception and should stay isolated.

## 3. react-markdown and unified integration choices

### Recommended: custom `code` component

Pecan already has the right interception point in `web/src/components/chat-markdown.tsx`:

1. `react-markdown` parses a fenced block into the normal `pre > code` HTML shape.
2. The language arrives as `className="language-mermaid"` on the `code` component; this is the same pattern react-markdown documents for syntax-highlighting components ([example](https://github.com/remarkjs/react-markdown/blob/fda7fa560bec901a6103e195f9b1979dab543b17/readme.md#use-custom-components-syntax-highlight)).
3. `CodeFence` can branch to a lazy Mermaid component before the generic highlighted-code path.
4. Keep `PreBlock` unwrapped as it is now, so the Mermaid component owns its outer frame and does not create nested `<pre>` elements.

This is the smallest integration and preserves the current GFM and highlighting pipeline.

### When a unified plugin would be justified

A remark plugin operates on mdast; a rehype plugin operates on hast; plugin order matters ([unified guide](https://unifiedjs.com/learn/guide/using-unified/)). Use a plugin only if Pecan needs syntax-tree behavior that the code component cannot cleanly provide, such as:

- static/build-time transformation of Mermaid fences into image/HTML nodes;
- collecting all diagrams and metadata before rendering;
- a custom non-code Markdown construct;
- server rendering where transformed output must be in initial HTML.

The synchronous default `Markdown` component cannot use async plugins. `MarkdownAsync` supports promises on the server, while `MarkdownHooks` supports async plugins on the client via `useEffect`/`useState` and initially renders nothing unless a fallback is supplied ([react-markdown API](https://github.com/remarkjs/react-markdown/blob/fda7fa560bec901a6103e195f9b1979dab543b17/readme.md#api)). Moving the entire chat Markdown pipeline to `MarkdownHooks` only to render Mermaid would unnecessarily make all Markdown depend on an async processing pass.

Do not add `rehype-raw` for Mermaid. react-markdown says raw HTML support costs roughly 60 kB minzipped and is intentionally separate because HTML is dangerous ([README](https://github.com/remarkjs/react-markdown/blob/fda7fa560bec901a6103e195f9b1979dab543b17/readme.md#appendix-a-html-in-markdown)); Mermaid fences are plain code nodes and need no raw HTML parsing.

## 4. Lazy loading, bundle shape, and runtime performance

### App-level lazy loading

Use `import("mermaid")` inside a cached loader that is reached only by a Mermaid fence. Dynamic `import()` loads a module asynchronously on demand; MDN specifically lists avoiding startup cost and memory for rarely needed code as a use case, while noting static imports are preferable for initial dependencies and static analysis/tree shaking ([MDN](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/import)).

This creates the desired product boundary:

- threads without diagrams pay no Mermaid download/parse/evaluation cost;
- the first diagram pays the import and first diagram/layout chunk cost;
- later diagrams reuse the browser module cache and Mermaid's loaded modules;
- normal Markdown remains synchronous and immediately visible.

### Mermaid's own code splitting

The npm package is not a single small renderer. `mermaid@11.17.2` exports `./dist/mermaid.core.mjs` and declares dependencies including D3, Cytoscape, Dagre, KaTeX, DOMPurify, RoughJS, and diagram-specific libraries ([official package metadata](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/package.json)). Mermaid's ESM build enables splitting, and the core build leaves dependencies external for downstream bundlers ([build source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/.esbuild/util.ts#L105-L127)). Diagram definitions and the default Dagre layout are themselves registered through lazy loaders ([diagram registration](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/diagram-api/diagram-orchestration.ts), [layout loader](https://github.com/mermaid-js/mermaid/blob/dcb694ddb58dc5ad3502e7e903cac05fd812eac3/packages/mermaid/src/rendering-util/render.ts)).

Therefore, app-level dynamic import and Mermaid's internal diagram/layout lazy loading are complementary. They also imply a first-render request waterfall: load Mermaid, detect the diagram, then load its definition/layout chunks. If that latency becomes visible, prefetch on likely intent (for example, when a Mermaid fence enters a near-viewport margin) rather than putting Mermaid back into the initial app bundle. `<link rel="modulepreload">` can prefetch, parse, and compile modules, but MDN warns that indiscriminate preloading can starve other work ([MDN](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Attributes/rel/modulepreload)).

### Reproducible package observation (not an API guarantee)

Measured on 2026-08-27 with official `mermaid@11.17.2`, Vite 8.2.2, a production build whose only Mermaid use was `await import("mermaid")`, and a basic flowchart render in Chromium:

- Vite emitted 95 Mermaid-related JavaScript chunks totaling **3,371,248 raw bytes**; these include all lazy alternatives and are not all fetched for one diagram.
- The basic flowchart path fetched 27 Mermaid chunks totaling **788,169 raw bytes**, **207,532 bytes when each chunk was gzip-compressed**, or **179,628 bytes with Brotli**. HTTP headers and the tiny application wrapper are excluded.
- The package's all-in-one `dist/mermaid.min.js` was **3,572,661 raw bytes** and **976,021 gzip bytes**.

These figures are local observations from the pinned package tarball, not stable upstream promises. They justify lazy loading but should be re-measured in Pecan's actual production output after integration.

The official `@mermaid-js/tiny` package is not the obvious answer for Pecan. Mermaid describes it as a CDN-oriented subset without mindmap, architecture, KaTeX, or lazy loading and recommends the full package unless there is a specific size reason ([official README](https://github.com/mermaid-js/mermaid/tree/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/tiny)). Pecan benefits more from loading the full package only on threads that need it.

### Rendering many diagrams

Mermaid queues renders serially, and layout/SVG generation runs on the browser main thread. For long chat histories:

1. Render visible or near-visible diagrams first.
2. Use `IntersectionObserver` with positive `rootMargin` to begin before the diagram becomes visible ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Intersection_Observer_API)).
3. Preserve a source/placeholder height where practical to reduce layout shift.
4. Memoize by exact source plus stable theme/config if messages remount during navigation. Cache SVG strings only within a bounded policy; do not let arbitrary chat history create an unbounded global cache.
5. Do not rely on `requestIdleCallback` for required rendering without a fallback; MDN marks it non-Baseline and recommends a timeout because callbacks can otherwise be delayed for seconds ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Window/requestIdleCallback)).

## 5. SSR and client constraints

Mermaid's distributed build targets the browser, and `render()` uses `document.body` plus live DOM/SVG operations ([build source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/.esbuild/util.ts#L37-L50), [render source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid/src/mermaidAPI.ts#L465-L655)). Importing or invoking it during ordinary Node SSR is therefore not a supported browser-equivalent rendering path.

Official Mermaid's server/build-time route is `@mermaid-js/mermaid-cli`, which converts Mermaid input to SVG/PNG/PDF and exposes a Node API, but does so through Puppeteer/browser infrastructure; its Node API is explicitly not covered by semver ([official CLI](https://github.com/mermaid-js/mermaid-cli#use-nodejs-api), [package source](https://github.com/mermaid-js/mermaid-cli)). It is suitable for build-time or cached server generation, not per-message request rendering without careful process reuse, limits, and isolation.

If Pecan later adds SSR:

- A client-only Mermaid component should render the same source/placeholder markup on the server and during the first client pass, then render SVG in an effect. React requires server and initial client content to match for hydration ([`hydrateRoot`](https://react.dev/reference/react-dom/client/hydrateRoot#caveats)).
- Effects only run on the client, which makes an effect a valid browser-only boundary but means initial HTML contains no diagram unless it was pre-rendered ([React `useEffect`](https://react.dev/reference/react/useEffect#displaying-different-content-on-the-server-and-the-client)).
- Client-only rendering trades away no-JS diagrams, can cause layout shift, and delays accessibility/SEO content. For documentation/static export, use an explicit pre-render pipeline rather than pretending Mermaid's browser API is SSR-safe.

Pecan currently uses `createRoot`, not `hydrateRoot`, so these are future constraints rather than present blockers.

## 6. Parser, layout engine, renderer: do not conflate them

A complete diagram path has distinct stages:

```text
Mermaid source
  → Mermaid syntax detection/parsing
  → diagram-specific semantic model
  → graph/diagram layout and text measurement
  → SVG/Canvas/raster rendering
  → safe DOM insertion and optional event binding
```

Replacing one stage does not replace the whole pipeline.

### Official `@mermaid-js/parser`

The official parser package returns ASTs; it is not an SVG renderer or layout engine ([README](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/parser/README.md), [package metadata](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/parser/package.json)). At the pinned commit its public dispatcher covers a specific Langium-based subset (info, packet, pie, architecture, git graph, event modeling, radar, railroad variants, treemap, tree view, Wardley, and Cynefin), not every classic Mermaid grammar such as flowchart or sequence ([source](https://github.com/mermaid-js/mermaid/blob/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/parser/src/parse.ts#L1-L156)). It is useful for validation/editor tooling for supported grammars, not as a drop-in path to rendered diagrams.

### Layout inside Mermaid

Dagre is Mermaid's default layered layout. Mermaid also lists ELK, tidy-tree, and CoSE Bilkent layouts ([official layouts](https://mermaid.js.org/config/layouts.html)); ELK is a separate package that must be installed and registered with `registerLayoutLoaders` ([official ELK package](https://github.com/mermaid-js/mermaid/tree/b134d15b8d2eb8e5a3d4ef7126eeabd444fbcd54/packages/mermaid-layout-elk)). A layout engine accepts an already-understood diagram/graph model. It does not, by itself, parse Mermaid's many DSLs or reproduce Mermaid's styling and semantics.

### Graphviz and Graphviz WASM

Graphviz consumes the DOT language and provides layout engines such as `dot`, `neato`, and `fdp`; DOT has its own grammar for graphs, nodes, edges, subgraphs, and clusters ([Graphviz DOT language](https://graphviz.org/doc/info/lang.html), [layout engines](https://graphviz.org/docs/layouts/)).

- `@hpcc-js/wasm-graphviz` compiles Graphviz to WebAssembly and turns DOT into SVG ([official repository](https://github.com/hpcc-systems/hpcc-js-wasm), [example](https://github.com/hpcc-systems/hpcc-js-wasm/blob/main/hw-graphviz.html)).
- Viz.js likewise describes itself as a WebAssembly build of Graphviz and renders DOT strings/graph objects ([official API](https://viz-js.com/api/)).

Neither is Mermaid-compatible without a **Mermaid parser plus a faithful Mermaid-to-DOT/model conversion**, and even then sequence, Gantt, pie, mindmap, C4, styling directives, link behavior, accessibility metadata, and other diagram-specific semantics need separate implementations. Graphviz WASM is an alternative layout/render backend for DOT, not a drop-in Mermaid renderer.

## 7. Mermaid-compatible native/WASM candidates

The following are independent compatibility implementations, not official Mermaid packages. Their support claims come from their own repositories and must be validated against Pecan's real fixture corpus.

| Candidate | Stages supplied | Runtime/surface | Main caveat |
|---|---|---|---|
| [Merman](https://github.com/Latias94/merman/tree/93957e34f9050ea69f1d10862b538f8d3b4bcce9) | Mermaid parsing, typed semantic model, layout, SVG; optional diagnostics and raster/text outputs | Native Rust; repository also documents WASM/web bindings | Independent reimplementation; parity and SVG details must be tested per diagram/config. |
| [`mermaid-rs-renderer`](https://github.com/1jehuang/mermaid-rs-renderer/tree/6907e0747822cc2ee7044250a065497ce7d8f3c0) | Native parsing → IR → layout → SVG, optional PNG | Rust library/CLI | Project calls itself early/active development and warns visual output may not match Mermaid CLI in all cases. |
| [`rusty-mermaid`](https://github.com/base58ed/rusty-mermaid/tree/4bdea8c79e0cb7bfe99fe3d8ad5eebb5bdb353a8) | Parsing, Dagre-style layout/custom layouts, scene IR, SVG/raster/GPU backends | Native Rust and WASM-capable architecture | Independent renderer; broad claimed type support is not the same as syntax/config/output parity. |

Merman is the most architecturally explicit candidate if Pecan later needs native server/CLI rendering: its repository separates parser-only (`merman-core`) from lower-level layout/SVG (`merman-render`) and a high-level renderer ([README](https://github.com/Latias94/merman/tree/93957e34f9050ea69f1d10862b538f8d3b4bcce9#output-targets)). That separation also makes clear why a parser-only or Graphviz-only dependency is insufficient.

Before adopting any compatibility implementation, build a conformance set containing at least:

- all Mermaid fence types Pecan intends to advertise;
- invalid/incomplete streamed input;
- nested subgraphs/clusters and long labels;
- theme variables and dark/light rendering;
- Unicode, escaping, HTML-like text, and links;
- accessibility title/description directives;
- large graphs and configured resource limits;
- SVG sanitation and rasterization behavior.

Compare accepted syntax, diagnostics, layout, SVG safety, and visual output against a pinned official Mermaid release. Do not choose based only on self-reported diagram counts or cold-start benchmarks.

## 8. Option comparison

| Option | Syntax fidelity | Initial web cost | SSR/native story | Recommendation for Pecan now |
|---|---:|---:|---:|---|
| Official `mermaid`, static import | Highest | Paid by every thread | Browser-only render API | No |
| Official `mermaid`, dynamic import + `render()` | Highest | Paid only when a diagram appears; internal chunks load by type/layout | Client-only unless separately pre-rendered | **Yes** |
| Official `run()` over `.mermaid` nodes | Highest | Same package cost | Client DOM only | No; conflicts with React ownership/update semantics |
| `@mermaid-js/tiny` | Official subset | Single smaller eager CDN bundle; no lazy loading | Browser | No; unsupported features and upstream recommends full package |
| Mermaid CLI pre-render | Highest for pinned browser environment | No client renderer; image/SVG payload only | Build/server via Puppeteer | Later for export/static docs, with caching/isolation |
| Official parser + custom renderer | Parser fidelity only for its supported subset | Potentially smaller but substantial engineering | Flexible | No |
| Graphviz WASM | DOT fidelity, not Mermaid | WASM payload | Browser/Node WASM | No, unless product changes input language to DOT |
| Independent Rust/WASM Mermaid renderer | Varies | WASM/native dependent | Strong native potential | Evaluate only if native/SSR/offline rendering becomes a hard requirement |

## 9. Suggested implementation contract for a later coding task

This report does not change application code. A narrow implementation should satisfy:

1. A `mermaid` fence takes the existing extracted raw source and renders in a dedicated component; all other fences retain current behavior.
2. The first Mermaid component starts one cached `import("mermaid")`; threads without Mermaid never request it.
3. Mermaid is initialized once with `startOnLoad: false` and `securityLevel: "strict"`.
4. Each render uses a stable unique ID and a private container ref.
5. Loading, parse/render error, source/copy, and successful SVG states are explicit.
6. Cleanup prevents stale queued results from mutating remounted/unmounted components.
7. The implementation calls returned `bindFunctions` after insertion, even if strict mode currently makes most interactions inert.
8. Tests cover success, invalid syntax, source change race, unmount, multiple diagrams, and non-Mermaid fence regression without live network dependencies.
9. A production Vite build records the Mermaid async chunks, and a browser test confirms no Mermaid request on a diagram-free thread.
10. The first slice uses built-in Dagre only. ELK registration and viewport deferral are follow-up optimizations justified by measured diagrams/threads.

## Bottom line

Official Mermaid loaded on demand is the lowest-risk path: it preserves syntax fidelity, fits the existing `react-markdown` component seam, and keeps the substantial renderer off diagram-free threads. `render()` is a better React primitive than `run()`. Strict security must remain enabled, the `innerHTML`/Trusted Types boundary must be acknowledged, and rendering should be treated as client-only unless Pecan deliberately adds a pre-render service.

WASM Graphviz is not a Mermaid renderer. Native/WASM Mermaid reimplementations do exist and may become attractive for Pecan's Rust side, but each replaces parsing, layout, and rendering semantics—not merely a JavaScript runtime—and therefore needs fixture-based compatibility proof before adoption.
