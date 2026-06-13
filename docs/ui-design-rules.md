# EvertyDesk Lite UI rules

These rules are intentionally small and practical. The desktop client is an
operator tool, not a landing page.

## Main Screen

- One primary action per card. On the connect card, `Connect` is the only main
  CTA.
- Checks, diagnostics, and advanced actions stay in status/menus/settings, not
  next to the primary action.
- Recent sessions are shortcuts, not a second toolbar.
- The hero/header uses brand signal and state only: product name, short
  subtitle, online status.

## Spacing

- Page margin: 24-28 px.
- Gap between major blocks: 14-18 px.
- Card padding: 16 px.
- Control height: 38-46 px.
- Avoid nested cards and decorative plates inside colored headers.

## Shape And Color

- Radius: 14 px for cards/hero, 10-11 px for controls, pills only for status.
- Palette: white surfaces, black text, green accent. Avoid teal-on-teal blocks.
- Use green for success/primary action only.
- Do not place a dark or white rounded square inside the hero unless it is an
  actual actionable control.

## Stability UI

- Renderer fallback status should be visible in logs/status text.
- If a backend is experimental, it must be opt-in or behind a clear setting.
- A broken video path should recover with a keyframe instead of leaving the
  operator with a corrupted stream.
