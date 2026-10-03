# Peek frontend refinement — 3 October 2026

Web: retained the existing landscape and glass-bubble visual identity. Adapted free UIArc Button and Segmented Control styles to the existing Solid UI, including visible focus, larger touch targets and reduced-motion support. Demo choices now support arrow keys, Home and End with roving focus; focus ends automatic cycling. Ting delivery and credential-storage copy matches explicit IAM5 approval.

Native macOS: refined all six settings panes (General, Voice, Permissions, Testing, Startup, Diagnostics) with shared headings, readable explanatory spacing, larger platform controls and capsule shapes. Account/organization details gain stronger hierarchy and bounded API labels. Menu-bar spacing and title hierarchy are aligned. SwiftUI retains native appearance, focus, keyboard and accessibility behavior. Bubble geometry, audio/capture controls, hotkeys, permission actions, queued-answer recovery and credential logic are unchanged.

## Verification

- Native package: all 515 tests pass after these changes; the isolated SwiftUI preview builds.
- Web: all 19 tests, TypeScript, production build and generated documentation/link validation pass.
- Actual native General and Permissions windows captured before and after using the real public views in an artifact-only harness. Its coordinator was never started, service registration was absent, and support/cache/socket locations were isolated. Permissions therefore shows a truthful unavailable-background-service state. No approval or OS permission was granted.
- Browser: desktop 1440×1000 and mobile 390×844 captures opened and inspected; mobile document width equals 390. ArrowRight, Home and End change both selected demo and focus.

Evidence directory: `/Users/codanium/Documents/silicon/.codex-artifacts/iam5-app-updates-20261003/ui-audit/`

| Surface            | Before                                  | After                                  |
| ------------------ | --------------------------------------- | -------------------------------------- |
| Web desktop        | `01-peek-desktop-before.jpg`            | `19-peek-desktop-after.jpg`            |
| Web mobile         | `02-peek-mobile-before.jpg`             | `15-peek-mobile-after.jpg`             |
| Native General     | `10-peek-native-general-before.jpg`     | `12-peek-native-general-after.jpg`     |
| Native Permissions | `11-peek-native-permissions-before.jpg` | `13-peek-native-permissions-after.jpg` |

Native captures use macOS dark appearance at Retina scale; the window’s new dimensions are intentional. This pass does not claim live IAM/Ting, a signed installed app, real microphone/hotkey hardware interaction, every appearance or full assistive-technology verification. Source/license provenance is in `web/UIARC.md`.
