# DSH UI reference (vendored contracts)

An offline copy of the **DeepSeek Harness (DSH) web frontend's public contracts**,
used as the spec the `bsh` GUI is cloned against. Nothing here is built or
shipped — it is reference material, not a dependency.

## Provenance

Fetched **2026-10-10** from the public npm scope `@deepseek-ai/*` (published by
DeepSeek, MIT — see [LICENSE-dsh](./LICENSE-dsh)). Two upstream sources were
used for the surrounding design narrative:

- `https://github.com/deepseek-ai/deepseek-harness` — `docs/architecture.md`
  (profiles/bundles, seams, turn flow, plugin tree),
  `packages/bundle/web-app/README.md` (the browser GUI surface).
- the npm packages below, resolved through `registry.npmmirror.com`.

| Package | Version | What it is |
| --- | --- | --- |
| `@deepseek-ai/dsh-web-app` | 0.0.1-rc.1 | the web profile patch layer (glue plugin, no dist) |
| `@deepseek-ai/dsh-host-frontend-static` | 0.0.1-rc.3 | the SPA dist server |
| `@deepseek-ai/dsh-client-runtime` | 0.1.1-rc.2 | client plugin runtime |
| `@deepseek-ai/dsh-client-modules` | 0.2.0-rc.2 | client module loading |
| `@deepseek-ai/dsh-client-connection` | 0.2.0-rc.2 | RPC/stream connection |
| `@deepseek-ai/dsh-client-locale` | 0.2.0-rc.2 | locale seam |
| `@deepseek-ai/dsh-client-ui-layout` | 0.2.0-rc.2 | **three-column AppFrame, columns, theme presenter** |
| `@deepseek-ai/dsh-client-ui-sidebar` | 0.2.0-rc.2 | left sidebar |
| `@deepseek-ai/dsh-client-ui-sidebar-right` | 0.2.0-rc.2 | right docking column |
| `@deepseek-ai/dsh-client-ui-conversation` | 0.2.0-rc.2 | **conversation surface, composer, records** |
| `@deepseek-ai/dsh-client-ui-tool` | 0.2.0-rc.2 | tool-call rendering |
| `@deepseek-ai/dsh-client-ui-theme` | 0.2.0-rc.2 | theme seam + tokens |
| `@deepseek-ai/dsh-client-ui-settings` / `-settings-general` | 0.2.0-rc.2 | settings framework + general page |
| `@deepseek-ai/dsh-client-ui-plan` | 0.2.0-rc.2 | plan surface |
| `@deepseek-ai/dsh-client-ui-goal` | 0.2.0-rc.2 | goal surface |
| `@deepseek-ai/dsh-client-ui-workspace` | 0.2.0-rc.2 | workspace/file surface |
| `@deepseek-ai/dsh-client-ui-trajectory` | 0.2.0-rc.2 | trajectory/replay surface |
| `@deepseek-ai/dsh-client-ui-subagent` | 0.2.0-rc.2 | subagent surface |
| `@deepseek-ai/dsh-client-ui-skill` | 0.2.0-rc.2 | skill surface |
| `@deepseek-ai/dsh-client-ui-agent-preset` | 0.2.0-rc.2 | agent preset page |
| `@deepseek-ai/dsh-client-ui-deliverables` | 0.2.0-rc.2 | deliverables surface |

Each `<package>/README.md` below is the upstream reference for that module; the
`.d.ts` files under `contract/` are its published TypeScript contracts
(`lib/types/**`), which is the most precise statement of the UI seam DSH holds
itself to.

### Refreshing this copy

```sh
# resolve + download one package (npmmirror follows redirects to its CDN)
curl -sSL -o pkg.tgz \
  "$(curl -sS https://registry.npmmirror.com/@deepseek-ai%2Fdsh-client-ui-layout \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); v=d["dist-tags"].get("next") or d["dist-tags"]["latest"]; print(d["versions"][v]["dist"]["tarball"])')"
tar xzf pkg.tgz
```

`registry.npmjs.org` is not reachable from this environment; `registry.npmmirror.com`
is, and serves the same tarballs.

## What we clone, and what we deliberately do not

Clone **the contract**, not the runtime:

| DSH module | `bsh` surface | Notes |
| --- | --- | --- |
| `client-ui-layout` AppFrame | `#app-frame` / `#sidebar` / `#center` / `#right-panel` | same column geometry and collapse rules (see the requirements doc, §12.6) |
| `client-ui-sidebar` | file tree + session list (`#sidebar`) | 280px default, 56px rail, Mod+B |
| `client-ui-sidebar-right` | right docking column | per-session track, 45% → 70% |
| `client-ui-conversation` | transcript + composer (`#transcript`, `#composer`) | node-per-record rendering, `textContent` only |
| `client-ui-tool` | tool timeline cards | status, duration, diff |
| `client-ui-theme` | `style.css` tokens | `--dsh-*` tokens ported to our variable names |
| `client-ui-settings*` | settings center | card registry |
| `dsh-client-connection` | Tauri IPC + events | we use IPC/events, not WebSocket RPC |
| `dsh-client-*-hmr` | *not cloned* | dev-only reload driver |

Not cloned: the React component runtime and the Cordis client-plugin loader. The
`bsh` GUI is a hand-written vanilla-JS document behind Tauri IPC — the clone is
of **structure, geometry, tokens, and seams**, which is what makes it look and
behave like DSH while keeping the renderer native-fast and dependency-free.
