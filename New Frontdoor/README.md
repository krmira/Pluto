# Pluto native frontend (C + Raylib)

This folder contains the native C frontend for Pluto. It renders the full UI
(timeline, library, preview, inspector, export drawer) with Raylib and talks to
the Rust backend through the C ABI in `pluto_bridge.h`.

The backend is loaded at runtime from `pluto_frontdoor_ffi.dll` (see
`pluto_bridge.c`); the frontend never links against Rust at build time.

## Build

### Prerequisites

- Rust (GNU toolchain for the backend): `rustup toolchain install stable-x86_64-pc-windows-gnu`
- FFmpeg/ffprobe on `PATH` (backend media engine)
- Raylib 5.5 MinGW build: the local toolchain lives in `%LOCALAPPDATA%\pluto-tools\raylib-5.5_win64_mingw-w64`

### Build the backend shared library

```sh
cargo build --release -p pluto-frontdoor-ffi
```

The resulting `Backdoor/target/release/pluto_frontdoor_ffi.dll` is what the
frontend loads.

### Build the frontend

With the MinGW gcc on `PATH` and the raylib include/lib folders available:

```sh
gcc -std=c11 -O2 -Wall -Wextra -Wno-unused-parameter \
    -I"$RAYLIB/include" -I. \
    pluto_interface.c pluto_bridge.c pluto_platform.c \
    -L"$RAYLIB/lib" -lraylib -lopengl32 -lgdi32 -lwinmm -lcomdlg32 \
    -o pluto.exe
```

Or simply run the provided script:

```powershell
.\build.ps1
```

## Running

Copy `pluto.exe` next to `pluto_frontdoor_ffi.dll` (or place the DLL beside the
executable) and run. The backend locates the library through, in order:

1. the `PLUTO_BACKEND_DLL` environment variable,
2. the folder of the running executable,
3. development fallbacks such as `Backdoor/target/release/pluto_frontdoor_ffi.dll`.

On first start the backend probes FFmpeg and the timeline is initialized empty.

## Layout

- `pluto_interface.c` — application state, widgets, timeline, preview, inspector, export drawer.
- `pluto_bridge.h/.c` — runtime loader and C ABI wrappers.
- `pluto_platform.h/.c` — Win32 open/save dialogs (media import, export, project save).
- `pluto_bridge.h` structs must stay layout-compatible with
  `Backdoor/frontdoor-ffi/src/lib.rs` (`PlutoClipInfo`, `PlutoMediaInfo`,
  `PlutoExportStatus`).

## Keyboard shortcuts

- `Space` play/pause, `V` select, `H` hand, `C` razor, `T` text tool
- `Ctrl+Z` / `Ctrl+Shift+Z` undo / redo, `Ctrl+E` export drawer, `Del` delete
- Arrow keys seek, `Home`/`End` jump to start/end, `G` guides

## Notes

- The legacy web UI in `Frontdoor/` is kept for reference; the native frontend is
  the primary interface.
- The native frontend mirrors the web UI defaults, effects registry, and export
  settings from `Frontdoor/scripts/app.js`.
