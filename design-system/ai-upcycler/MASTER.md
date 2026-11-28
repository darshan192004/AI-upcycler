# AI Upcycler — MASTER design contract (sub-project B)

Single source of truth for the desktop, gallery, task timeline, and phone chat
surfaces. Read this before building any sub-project B UI. Derived from
`docs/superpowers/specs/2026-11-27-privatelm-style-extension-design.md` §12 and
the Task 1 dependency gate.

## Colour tokens

| Token | Value | Use |
|---|---|---|
| Background | `#0F172A` | App background (dark default) |
| Card | `#1B2336` | Panels, bubbles, cards |
| Border | `#475569` | Dividers and control borders |
| Muted text | `#94A3B8` | Secondary copy, timestamps, captions |
| Accent | `#22C55E` | Running/ready state, focus ring, primary action |
| Destructive | `#EF4444` | Error, delete, failed state |

- Dark is the default and only shipped default; never ship a light-only state.
- Body text must reach **>= 4.5:1** contrast against its actual background in
  both dark and light states. Verify muted text on card and on background before
  use; never use the accent or destructive token as a large-text-only shortcut.
- Desktop focus ring: `2px #22C55E` outline with `2px` offset on every
  interactive control, including modal controls.

## Typography

- UI font: **Fira Sans** (Regular 400, Medium 500).
- Model ids, tokens, and fingerprints: **Fira Code** (Regular 400).
- No other font families. Remote font loading (`fonts.googleapis.com`,
  `fonts.gstatic.com`) is prohibited; fonts are vendored under `assets/fonts/`,
  mirrored to `public/fonts/` for the web app and to the Android `res/font/`
  directory. Licences: `assets/fonts/OFL.txt` and
  `assets/fonts/OFL-FiraCode.txt`.

## Spacing and touch targets

- 4 dp half-step, 8 dp rhythm: 4 / 8 / 16 / 24 / 32 / 48 dp. Use the same scale
  in CSS (0.25 / 0.5 / 1 / 1.5 / 2 / 3 rem) so desktop and Android agree.
- Android touch targets are **48 dp minimum**; keep pressed states inside the
  existing bounds so layout never shifts. Respect safe-area/status-bar clearance.

## Motion

- Hover/press transitions are **150–300 ms** (150 ms for micro-interactions,
  300 ms maximum). Never animate multi-second generation with a spinner; stream
  tokens and use a stable skeleton with `aria-busy` for short waits.
- `prefers-reduced-motion: reduce` disables CSS animation and transition; in
  Compose, expose a reduced-motion off switch that turns all motion off and
  keep every state readable without animation.

## State, icons, and accessibility

- State indicators always pair an **icon with text** (for example a check icon
  plus "Ready", a cross plus "Failed"). Colour is never the only indicator.
- Decorative icons are hidden from accessibility trees (`aria-hidden="true"` on
  the web, `contentDescription = null` / `null` semantics in Compose).
- Icon-only controls still carry an accessible name. Disabled states are
  visibly disabled. Chat lists use stable message/session keys, never indexes.

## Parity

`src/index.css`, `mobile-worker/android/app/src/main/res/values/colors.xml`,
and this file carry the same six hex values. Any change to a token must be made
in all three, then the Task 1 verification commands must be rerun.
