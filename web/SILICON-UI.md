# Silicon UI in the peek website

The site's interface components come from [Silicon UI](https://ui.teamofsilicons.com) (MIT, the continuation of Arc by
Team of Silicons; `licenses/silicon-ui-MIT.txt`). They are plain source in `components/silicon-ui/`, installed with
the shadcn CLI from the registry configured in `components.json`:

```sh
npx shadcn@latest add @silicon-ui/<name>
```

| Component | Used for |
|---|---|
| `silicon-foundation` | Design tokens and focus rules (`foundation.css`, imported once in `app/layout.tsx`) |
| `blocks/site-header`, `blocks/site-footer` | The landing page and 404 header and footer |
| `button`, `copy-button`, `code-block` | Actions, install commands, and every code sample (landing and docs) |
| `segmented-control` | The hero demo's Show / Speak / Ask / Slider switch |
| `card`, `badge`, `empty-state` | Shortcut and fact cards, docs group labels, the 404 page |
| `popover`, `accordion`, `password-field`, `alert`, `user-menu` | The Silicon Accounts sign-in menu |
| `command-palette`, `drawer`, `breadcrumb` | Docs search (⌘K or /), the mobile docs menu, the docs breadcrumb |

## Local changes to registry source

Both are additive; reinstalling a component overwrites them, so reapply them after an update.

- `blocks/site-header/site-header.tsx`: an `accessory` prop renders extra content (the account menu) at the start of the
  actions, kept in the bar on narrow layouts.
- `command-palette/command-palette.tsx`: a `filter` prop replaces the substring match (the docs use ranked full-text
  search over `/search.json`), and `emptyTitle` / `emptyDescription` reword the empty result.

The hero's bubble (`components/peek/Hero.tsx`) is not built from Silicon UI: it reproduces Peek.app's own curved arcs,
glass and springs. Native macOS controls use SwiftUI.
