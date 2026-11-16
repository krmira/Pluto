# Pluto backend

## What is connected

- `timeline-core/` stores and validates the clip snapshot sent by the Frontdoor, including track, timing, trim, link, transform, effect, and audio properties. The pointer-driven drag preview remains in the Frontdoor; each committed edit is synchronized to this Rust state.
- `desktop/src-tauri/` provides native media selection and FFprobe metadata, cancellable FFmpeg export, export progress, and the Tauri commands used by the interface.
- `Frontdoor/scripts/tauri-timeline.js` is the invoke bridge. `window.plutoTimeline` exposes timeline state and editing commands; `window.plutoNative` handles import, export, progress, and file URLs.
- Video and image effects, transforms, titles, audio adjustments, and audio effects are sent with clip state and applied during FFmpeg export. The Frontdoor also previews them while editing.

## GitHub Actions build

`.github/workflows/rust-tauri.yml` installs FFmpeg on the Windows runner, stages `ffmpeg.exe` and `ffprobe.exe` for the bundle, builds Pluto, and uploads the executable with those tools as the `Pluto-Windows` artifact. Download the artifact as a folder so the executable keeps its adjacent `ffmpeg/` directory.

For local development, install FFmpeg and FFprobe on `PATH`, or set `PLUTO_FFMPEG_DIR` to the directory containing both executables.

## C ABI crate

`frontdoor-ffi/` implements the `pluto_frontdoor_ffi` shared library that the
native C frontend loads at runtime via `LoadLibrary`/`dlopen` (see
`New Frontdoor/pluto_bridge.c`). The C side never links against Rust; it resolves
symbols such as `pluto_timeline_add_clip`, `pluto_media_inspect`, and
`pluto_export_start` through the loader.

The `PlutoClipInfo` layout in `New Frontdoor/pluto_bridge.h` must stay
layout-compatible with the `#[repr(C)]` struct in this crate; changing one
requires changing the other.

## Native C frontend

`New Frontdoor/` contains the primary user interface: a Raylib based C frontend
with an immediate-mode UI, timeline editor, library, preview, inspector, and
export drawer, bridged to this backend through the FFI. Build scripts live in
that folder (`build.ps1` or the `Makefile`).

## GitHub Actions build
