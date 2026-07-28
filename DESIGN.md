# Design plan — popup & settings

Written before any markup, per §5 of the implementation plan.

## Concept

**Interlinear.** The one idea the UI is built around is the interlinear gloss — the
convention in language textbooks and translated manuscripts where the source line sits
directly above its translation, bound together as a single unit rather than presented as
two separate blocks. The popup is not a tooltip with two paragraphs in it; it is one
glossed pair.

## Palette

Six named values per theme. Cool paper ground (not cream, not near-black), and — the
substantive choice — **two accents that mean something**: every piece of source-language
material is cool, every piece of target-language material is warm. Colour carries the
direction of the translation instead of just decorating it.

| Token | Light | Dark | Role |
|---|---|---|---|
| `--bg` | `#F4F6FA` | `#171B24` | window ground (cool grey-blue) |
| `--surface` | `#FFFFFF` | `#1F2531` | the card |
| `--text` | `#171B24` | `#E7EBF2` | primary |
| `--text-dim` | `#5A6478` | `#98A2B6` | secondary / original text |
| `--source` | `#3D62B8` | `#7C9CE8` | **cool pole** — the language you had |
| `--target` | `#A96A05` | `#E8A33D` | **warm pole** — the language you wanted |
| `--danger` | `#B3261E` | `#F2857C` | error state |

Deliberately not: warm cream + terracotta, near-black + neon, newspaper hairlines.

## Type

- **Payload** — `Iowan Old Style / Palatino / Georgia / ui-serif`. Used for exactly one
  thing: the translated text. It is the only reason the window opened, so it is the only
  thing set in the reading face.
- **UI** — `Segoe UI Variable Text / system-ui`. Buttons, settings labels, the original
  text.
- **Meta** — `ui-monospace / Cascadia Mono / Consolas`. Used consistently and only for
  machine-ish facts: the `SELECTION → VIETNAMESE` route line, character counts, hotkey
  chips, section labels in Settings.

## Layout

- **Popup**: a single dense card, 420px wide, four stacked rows — route line, glossed
  pair, footer. No panels, no tabs, no decorative header. It resizes to its content and
  never scrolls unless the source text is long (then the original clamps to 4 lines with
  a fade, the translation never clamps).
- **Settings**: one column, ~560px, four labelled groups — Provider / Translation /
  Hotkeys / Behaviour / Advanced. No sidebar navigation for 13 fields.

## Signature detail

**The gloss rail.** The original/translated pair is joined by a 3px vertical rail down
its left edge whose gradient runs `--source` at the top to `--target` at the bottom — the
translation itself, drawn. It replaces the horizontal rule that would otherwise separate
the two blocks, and it is the single element that makes the popup recognisable.

The rail is also the **loading state**: instead of a spinner, a lit band pours down the
rail from the cool end to the warm end while the request is in flight. Under
`prefers-reduced-motion` the animation is dropped and the rail renders as a static
half-lit gradient with a "Translating…" label.

## Quality floor

- 2px `--source` focus ring at 2px offset on every interactive element — never removed.
  This app is driven by the keyboard, so focus is a primary state, not a fallback.
- `prefers-reduced-motion` honoured by the rail, the popup's entrance, and the crop
  overlay.
- Errors name the cause and the fix: *"Invalid API key — check it in Settings → Provider"*,
  not *"Translation failed"*.
- Labels use the terminology the user already typed: "API key", "Base URL", "Model name",
  "Copy translation".
