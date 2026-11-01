# BSH Screenshot Replacement Guide

> The images in this directory are **layout placeholders, not screenshots of the running application**. Replace them with genuine BSH captures before presenting the workspace as a verified product UI.

## Placeholder inventory

| File | Intended scene | Suggested real capture |
|---|---|---|
| `workspace.svg` | Main workspace | Full application window showing project navigation, session list and workspace layout |
| `agent-session.svg` | Agent session | A real conversation with streaming response, plan/task state and tool activity |
| `extensions.svg` | Extension manager | Installed extensions, lifecycle state and capability/permission UI that exists in the build |
| `settings.svg` | Model and settings | Provider/model configuration with all secrets and personal data removed |

## Replacement requirements

- Keep the same filenames so README links do not need to change; SVG files may be replaced by PNG or WebP if the Markdown references are updated.
- Prefer lossless PNG or high-quality WebP at **1600 × 900** or higher, 16:9.
- Capture the actual BSH application. Do not compose controls that are not present in the running build.
- Remove API keys, tokens, personal paths, private project names and other sensitive data before committing.
- Use consistent window size, theme, scale and UI language across the gallery.
- Update both `README.md` and `README-ZH.md` only if the scene inventory or filenames change.

## Status

All four current assets are explicit placeholders. They must not be described as real screenshots, production UI evidence or proof that a feature is currently available.
