# Assets

| File | Purpose |
|------|---------|
| `yapper-tray.rgba` | Tray icon (raw RGBA): 4-byte width LE, 4-byte height LE, then `w*h*4` pixels. Loaded by `tray.rs` when present. |

Generate or regenerate:

```bash
# from repo root — simple 32×32 mic glyph
python3 - <<'PY'
# see install / scripts, or copy from tray::build_mic_icon_rgba
PY
```

Fallback: if the file is missing, the app draws a procedural mic glyph in memory.
