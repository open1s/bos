# Reference UI mirror

Measured UI source of truth, extracted from the shipped DeepSeek Harness app
(`/Applications/DeepSeek Harness.app/Contents/Resources/app.asar`).

The asar data base is `8 + <u32 little-endian at offset 4>` = **3393624**, NOT
`16 + <json size at offset 8>`; the two-byte error makes every extracted file start
with the previous file's closing brace. Each leaf entry in the header JSON `files`
tree carries `size` and `offset` **relative to that base**.

| path | what it holds |
|---|---|
| `extract-asar.py` | list/read asar entries; `--list` prints `size path` per line |
| `pkgs/` | the `dsh-*` bundles extracted so far (sha256-verified by the asar header); `dsh-compact.md` lists the compaction/command/chat subset re-extracted this round |
| `dsh-css-inline/` | 146 CSS modules inlined in those bundles, with `INDEX.md` |
| `dsh-css-files/` | 41 CSS modules shipped as real files, with `INDEX.md` |
| `dsh-settings-chrome.md` | settings panel/nav/header/footer sheets + token map |
| `dsh-settings-rail.md` | the five measured nav cells |
| `dsh-general-rows.md` | the General pane's row sheets |
| `dsh-shell-chrome.md` | shell chrome sheets |
| `dsh-conversation-chrome.md` | composer/input sheets |
| `dsh-transcript-chrome.md` | transcript/turn sheets |
| `dsh-type-scale.md` | measured type scale |
| `dsh-slot-map.md` | slot registrations |
| `dsh-compact.md` | /compact, the automatic trigger, the marker, and bsh parity |

Extraction recipe for both CSS trees:

```bash
# real files, straight out of the asar
python3 .tmp-ref/extract_css_node.py
# inlined modules, out of the extracted bundles
python3 .tmp-ref/extract_css.py
```
