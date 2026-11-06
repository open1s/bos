# BSH Screenshot Replacement Guide

> The images in this directory are **layout placeholders, not screenshots of the running application**. Replace them with genuine BSH captures before presenting the workspace as verified product UI.

## Placeholder inventory

| File | Intended scene | Suggested real capture |
|---|---|---|
| `workspace.png` | Main workspace | Full application window showing project navigation, session list and workspace layout |
| `agent-session.png` | Agent session | A real conversation with streaming response, plan/task state and tool activity |
| `extensions.png` | Extension manager | Installed extensions, lifecycle state and capability/permission UI that exists in the build |
| `settings.png` | Model and settings | Provider/model configuration with all secrets and personal data removed |

All four files are existing PNG layout placeholders. The README links to these exact `.png` paths. The separate `bsh-product-hero.svg` is a conceptual platform illustration, not a substitute for application captures.

## Replacement requirements

- Keep the same filenames so README links do not need to change. If you change the format, update both `README.md` and `README-ZH.md`.
- Prefer lossless PNG or high-quality WebP at **1600 × 900** or higher, 16:9.
- Capture the actual BSH application. Do not compose controls that are not present in the running build.
- Remove API keys, tokens, personal paths, private project names and other sensitive data before committing.
- Use consistent window size, theme, scale and UI language across the gallery.
- Do not label conceptual diagrams as product screenshots or evidence of implemented features.

## Status

The four gallery assets are placeholders and must be replaced with genuine captures before they are presented as real application UI. The product hero is a separate conceptual illustration.
