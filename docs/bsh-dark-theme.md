# BSH dark theme: macOS graphite

The dark theme is a native-macOS reading of the shell: soft graphite surfaces,
a softened system blue, real window vibrancy, fine white hairlines, and depth from
shadows rather than decoration. The light theme is intentionally untouched.

## Design intent

- **One soft accent.** A desaturated system blue (`#6BA6FF`) carries every
  interactive state: links, focus, selection, the composer edge, the running
  tab.
- **Soft graphite, not black.** The page is `#202024`; the composer and
  controls are `#2E2E34`; the sidebar is `#26262B` lifted at the top and sunk
  at the floor. Pure black is reserved for overlays.
- **Hairlines, not borders.** Separators are white at 10% (`#FFFFFF1A`), the
  way macOS draws vibrancy edges, instead of opaque grey lines.
- **Depth without noise.** The composer is raised by a wide, low-opacity
  shadow and one inset white lip. The page gets a 0.8% top lift and 1.2% floor
  sink. Nothing else glows.

## Palette

Eight-digit hex is `#RRGGBBAA`; the alpha pairs are deliberate vibrancy.

| Token | Value | Role |
|---|---|---|
| `background` | `#202024` | page, soft graphite |
| `sidebar.background` | `#26262B` | sidebar (the gradient starts here) |
| `secondary.background` | `#2E2E34` | raised card, composer, popover |
| `secondary.hover.background` | `#3A3A40` | hover on a raised control |
| `foreground` | `#E6E6EC` | label primary, softened off white |
| `muted.foreground` | `#A3A3AB` | label secondary |
| `border` | `#FFFFFF1A` | hairline, 10% white |
| `input.border` | `#FFFFFF21` | field edge, 13% |
| `primary.background` | `#6BA6FF` | soft system blue |
| `primary.hover.background` | `#8AB9FF` | blue, lifted |
| `selection.background` | `#6BA6FF3D` | text selection, 24% |
| `danger.background` | `#E5675F` | softened system red |
| `success.background` | `#6FCF97` | softened system green |
| `warning.background` | `#E5C07B` | softened system yellow |
| `info.background` | `#7FD1DE` | softened system teal |
| `scrollbar.thumb.background` | `#FFFFFF26` | 15% white |

The tertiary `faint` token is derived, not stored: the muted label mixed 30%
into the page, so the hierarchy is primary / secondary / tertiary without a
fourth colour.

### A translucent token must composite

The palette stores RGB. A kit colour that carries alpha, such as
`sidebar.accent.background: #FFFFFF12`, used to lose that alpha when copied
into the palette, so `theme::selected_row()` became pure white. That is what
turned the selected preset card, the settings selects and the inset buttons
white in dark mode. `sync` now flattens those tokens with `hx_over`: the
selected row over the sidebar, the soft border over the page. Any new
translucent token that feeds a palette accessor has to use `hx_over`, not
`hx`.

## Effects

### Window vibrancy (macOS only)

The window opens opaque. `theme::apply`, `theme::toggle` and
`theme::apply_here` call `set_vibrancy`, which sets
`WindowBackgroundAppearance::Blurred` for dark and `Opaque` for light. The
platform backs a blurred window with an `NSVisualEffectView`; the shell then
makes the frame transparent in dark and paints the page on the main column, so
only the sidebar reveals the material. Light never pays for a blur it hides.

### Sidebar

`render_sidebar` draws a vertical gradient from `sidebar_bg` lifted 2.5% to
sunk 8%, at 91% alpha, plus a 380px floor glow of soft blue at 5%. The
blue-white wave and its light asset are removed from dark; light keeps both.

### Composer

Dark raises the card to `#2E2E34` with a 64px / 30% black shadow, a 2px / 35%
contact shadow and an inset white 5% top lip. A 2px soft-blue edge at 25%
replaces the light theme's cyan gradient ring. Light keeps the measured white
card, `shadow_sm` and accent-to-cyan ring.

### Settings hero

The 62px section tile in the settings banner is a glass wash in dark: 5% white
at the top fading to 2%, almost invisible against the banner, so only the 29px
glyph reads. Light keeps the original tint-to-white disc.

### Page depth and selection

The main column's dark background is a 180° gradient from page lifted 0.8% to
page sunk 1.2%. A selected session row lifts 6% towards white (macOS selection),
and the New Session button uses the raised surface instead of the page.

## Where it lives

| Concern | File |
|---|---|
| Palette values | `crates/bsh/src/assets/bsh-theme.json` (`BSH Dark`) |
| Token sync, faint, `lift`/`sink`, vibrancy | `crates/bsh/src/theme.rs` |
| Sidebar gradient, floor glow, selected row | `crates/bsh/src/ui/sidebar.rs` |
| Composer fill, shadow, glass lip, focus edge | `crates/bsh/src/ui/composer.rs` |
| Page gradient / main column paint | `crates/bsh/src/ui/header.rs` |
| Transparent dark frame | `crates/bsh/src/ui/shell.rs` |
| Window creation | `crates/bsh/src/ui/mod.rs` |

## Verification

- `the_dark_theme_is_macos_graphite` parses the theme asset and pins the page,
  sidebar, label, system blue, hairline, red and green values.
- `the_theme_set_parses_and_carries_both_modes` keeps the set loadable.
- Composer, sidebar and theme test groups pass; `cargo clippy -p bsh
  --all-targets -- -D warnings` is clean.
- Captured in both modes at 1280×820. Light is byte-for-byte unchanged: the
  parent palette compares with zero changed keys; only `BSH Dark` differs.
- The changes are paint and colour only: no control moved, no measured layout
  number changed.

## Maintenance

1. Change the JSON value and the test that pins it; do not hard-code a second
   copy in a view.
2. Gate an effect by `theme::is_dark()` and leave the light branch as it was.
3. Use `theme::lift`/`theme::sink` for graphite depth instead of new hex
   literals, so a future palette edit moves the derived surfaces with it.
4. Keep the accent single. A second saturated colour is what turns a macOS
   surface into a dashboard.
