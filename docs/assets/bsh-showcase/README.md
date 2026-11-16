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

## Capture workflow (macOS development build)

The repository's BSH UI workflow documents launching the raw binary for section-specific captures. From the repository root, after building the app:

```bash
pkill -f "BSH.app" || true
pkill -f target/debug/bsh || true
sleep 2
nohup ./target/debug/bsh > /tmp/bsh_ui.log 2>&1 & disown
sleep 6
pgrep -fl target/debug/bsh
```

Use the app's own navigation to reach each real screen. If the local OCU setup is available, the documented capture call is:

```bash
ocu call get_app_state --args '{"app":"bsh","text_limit":200}'
```

Save the returned screenshot payload as the matching PNG file in this directory. If OCU is unavailable, use macOS screenshot capture manually. Confirm the image shows the actual running BSH process; do not recreate UI elements in an image editor.

Before committing, inspect all four captures at full resolution and remove credentials, tokens, personal paths, private repository names, account identifiers and unrelated desktop content. Keep the app window size and visual theme consistent. Verify each image opens and that the README renders all four images.

The command above is a capture recipe from the repository's development notes, not a claim that the binary or OCU is available in every environment. Confirm the process is running before capture; if launch fails, resolve the build/runtime issue rather than substituting a concept image.
