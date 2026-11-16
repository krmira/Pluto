# Pluto native frontend

This is the active desktop frontend. Pluto is moving from the Tauri webview to
**egui** while keeping the existing `Frontdoor` layout, colors, spacing, and
editor behavior as the reference. The web implementation remains in the repo
for comparison during the migration; the native build does not load it.

## Current native surface

- Custom Pluto title bar, library, preview/transport, inspector, and timeline
  panels using the existing theme tokens and icon set.
- FFprobe based media metadata, FFmpeg thumbnails and real audio waveforms,
  imported on a single background worker; GIFs are imported as animated video.
- Timeline clips, lane expansion, drag/drop, snapping, razor split, scrubbing,
  cursor/hand tools, zoom/pan, frame stepping, and undo/redo.
- Native FFmpeg preview for the active video clip, including its canvas,
  transform/color/effect filters, mirror effect, and clip keyframe timing.
- Native image and video export settings for PNG/JPEG/WebP/AVIF and
  MP4/WebM/GIF, including quality, frame rate, audio, looping, metadata,
  color-space, dimensions, and aspect-ratio lock.
- Versioned `.pluto` project files with Ctrl+S/Ctrl+O save and open shortcuts;
  opening restores the timeline and reloads media from its stored paths.
- Transform, color, effect, title, transition, and audio-detach controls write
  to the same clip properties consumed by the exporter.

## Migration status

The egui shell and primary editing path are in place, but full behavior parity
is not complete. The current live preview shows one active video layer at a
time; multi-track compositing, synced audio playback, and final pixel-level
widget polish remain outstanding.
Some multi-selection, grouping, and keyframe operations are implemented but
still need broader interaction testing. The shared FFmpeg engine still handles
full-timeline export, including the stacked video/audio layers and transitions.

## Run

```powershell
cd Frontdoor-egui
cargo run
```

Install FFmpeg and FFprobe on `PATH`, set `PLUTO_FFMPEG_DIR`, or keep them in an
`ffmpeg` directory beside `Pluto.exe`. The Windows Actions workflow caches
those tools, checks the egui and backend crates, and uploads a portable ZIP
with the application and FFmpeg runtime.

If the native window fails to appear, startup and panic details are written to
`%LOCALAPPDATA%\Pluto\startup.log`.

## Visual reference

`src/theme.rs` holds the palette, panel geometry, spacing, radii, and type
scale from `Frontdoor/styles/app.css`. `src/icons.rs` embeds the same icon
shapes. Custom painting preserves the major panel and timeline styling while
egui widgets provide native controls inside the inspector.
