# NotCode Design System

Version 1.0 · shared identity for NotCode Android and NotCode Desktop

NotCode is one product family with two interaction models. Android uses Material 3 and touch-first navigation; Desktop uses compact IDE chrome, keyboard shortcuts and resizable workspaces. The implementation is different by platform, but the brand language is not.

## Brand contract

The following tokens are canonical. Product UI must reference these tokens rather than inventing local colors. The same values must be used in light/dark-capable components where they apply; this first shared contract is a dark graphite workspace.

| Token | Value | Meaning |
|---|---:|---|
| `background` | `#191C20` | Main workspace and editor canvas |
| `sidebar` | `#14171A` | Navigation, rails, status areas |
| `surface` | `#202428` | Panels, dialogs, cards, composer |
| `divider` | `#30363C` | Hairlines and quiet boundaries |
| `text` | `#E4E9E7` | Primary content |
| `muted` | `#A0AAA5` | Secondary content and metadata |
| `accent` | `#B6E6A5` | NotCode action, selection and focus |

Semantic states are not brand colors: error `#F2B8B5`, warning `#F2D18B`, and success `#81C995`. They are only for the meaning of a state. Do not replace the NotCode accent with red, amber or green for ordinary selected or active controls.

## Tokens

The source of truth for generated Desktop CSS is `design-system/tokens.json`. The Android implementation should mirror the same names in Kotlin, preferably through a small `NotCodeColors`, `NotCodeTypography`, `NotCodeSpacing`, `NotCodeShapes`, `NotCodeMotion` token layer rather than scattering `Color(...)` and `dp` literals through composables.

### Colors

Use `background` as the deepest surface. Use `sidebar` for persistent navigation and compact system chrome. Use `surface` for a layer above the workspace. Use `divider` sparingly: it separates structure and should not draw a box around every element. Primary text uses `text`; metadata, hints and inactive icons use `muted`; focus rings, primary buttons, selected tabs and the brand mark use `accent`.

### Typography

The shared typeface is Manrope, already present in the Android project at `app/src/main/res/font/manrope.ttf`. Desktop loads the same asset and falls back to Segoe UI. Code uses Cascadia Code, then Consolas. Use sentence case, not tracked all-caps labels.

The shared scale is caption 11, small 12, body 13, large 15, title 22 and display 32 px on Desktop. Android maps these to the closest Material 3 `sp` values while preserving the hierarchy. Body line height is 1.55; code line height is 22 px on Desktop. Weight choices are regular 400, medium 500, semibold 600 and bold 700.

### Spacing

Base unit is 4 px: 4, 8, 12, 16, 20, 24, 32, 40, 48 and 64. Android maps the same numbers to `dp`. Use 8 px for control internals, 12 px for compact panel padding, 16 px for normal content padding and 24 px for a section transition. Touch targets must be at least 48 dp on Android; Desktop controls may be compact, but keyboard focus must remain visible.

### Shapes and elevation

The shared corner scale is 8, 12, 16, 20 and 28 px, plus a pill for status dots. Use 8 px for controls and small rows, 12 px for cards/composers, 16 px for larger surfaces, 20 px for dialogs and 28 px only for high-level mobile containers. Do not use a different radius for every element.

The visual language is quiet: flat panels by default, a restrained overlay shadow for dialogs, and a translucent accent wash for selected state. Avoid generic card stacks and gradients. Android maps these rules to Material 3 tonal surfaces and elevations without changing the token colors.

### Icons

Icons are 18 px by default, 14 px in dense rows and 22 px for high-level navigation. Use one consistent 1.7 px outline stroke and the same semantic icon for the same action in both products. Desktop uses Lucide; Android should use the closest Material/Lucide-compatible outline glyph. Icons support text; they do not replace a label when the action is unfamiliar.

### Motion

Motion is user-driven and short: fast 100 ms, normal 160 ms and slow 240 ms, easing `cubic-bezier(0.2, 0, 0, 1)`. Use it for panel opening, selection and confirmation feedback, not for decoration or constant ambient movement. Respect reduced-motion settings on Desktop and Android. A loading state may use one small accent pulse; it must not make the interface noisy.

## Component rules

Buttons share a surface, divider, 8 px shape and visible accent focus ring. The primary action uses the accent fill with `onAccent` text; secondary actions stay quiet. Disabled controls reduce contrast without changing hue. Cards and panels use the same surface/divider rules and do not receive independent decorative colors.

Navigation exposes one active destination with a narrow accent indicator. Tabs use an accent top or bottom indicator and preserve the same file/session vocabulary. Empty states explain what is missing and offer the next real action; they must not fill the product with invented demo data.

Dangerous actions show a confirmation that says what will execute, where, and what can be changed. Error copy states the failed operation and a recovery path. Semantic colors are reserved for that meaning.

## Platform adaptation

Android uses Material 3 components, 48 dp touch targets, bottom or drawer navigation and adaptive layouts. Desktop uses a 48 px activity rail, compact 32 px controls, keyboard equivalents, Monaco tabs, draggable splitters and multiple terminal/editor panes. Both platforms keep left-aligned hierarchy, the same surface order, typography family, icon character, radii and state behavior.

## Desktop layout contract

Desktop starts with the graphite workspace, a compact activity rail, an optional 240 px explorer, a flexible editor, an optional 360 px agent panel and a resizable terminal. The layout must remain usable down to the supported minimum window size. The editor is the visual center; AI is a working panel, not a marketing sidebar. Persistent local status is shown as a small badge, not as an extra accent color.

## Android migration note

The current Android source has a legacy color set in `NotCodeApp.kt` (`#101411`, `#171D18`, `#C6F27A` and related values) and a `Night` surface literal. Those are not the shared contract. The next Android design-system pass should replace them with the canonical tokens above and route Material 3 `darkColorScheme`, shapes and typography through the shared token layer. This documentation intentionally does not rewrite Android UI architecture; it defines the visual contract that both products must follow.

## Review checklist

Before merging a component, confirm that it uses the shared token source, has a visible keyboard or touch focus state, uses the established shape, uses semantic colors only for semantic states, supports reduced motion, has a real empty/error state and does not introduce a new brand accent. Confirm the same component has a Material 3 adaptation on Android and a mouse/keyboard adaptation on Desktop.
