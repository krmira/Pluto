/*
 * pluto_interface.c — Native C frontend for the Pluto video editor.
 *
 * The interface mirrors the editor layout used by the previous web frontend,
 * but it is rendered with Raylib and talks directly to the Rust backend
 * through pluto_bridge.h (timeline-core + pluto-media-engine).
 *
 * Build (Windows, MinGW-w64 + Raylib 5.x):
 *   see build.ps1 / Makefile in this folder.
 */
#include "raylib.h"
#include "pluto_bridge.h"
#include "pluto_platform.h"

#include <ctype.h>
#include <math.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_CLIPS 128
#define MAX_MEDIA 64
#define MAX_TRACKS 6
#define MAX_UNDO 32
#define JSON_CAP 65536

#define RGBA(r, g, b, a) ((Color){(unsigned char)(r), (unsigned char)(g), (unsigned char)(b), (unsigned char)(a)})
#define RGB(r, g, b) RGBA(r, g, b, 255)

/* Palette mirrors styles/app.css. */
static const Color COL_BG = RGBA(11, 11, 13, 255);
static const Color COL_CARD = RGBA(24, 25, 28, 255);
static const Color COL_CARD_TOP = RGBA(28, 29, 32, 255);
static const Color COL_FIELD = RGBA(33, 34, 38, 255);
static const Color COL_FIELD_HOVER = RGBA(42, 43, 49, 255);
static const Color COL_TEXT = RGBA(237, 237, 240, 255);
static const Color COL_MUTED = RGBA(140, 142, 150, 255);
static const Color COL_DIM = RGBA(98, 100, 108, 255);
static const Color COL_RED = RGBA(255, 59, 59, 255);
static const Color COL_GREEN = RGBA(52, 199, 123, 255);
static const Color COL_VIDEO = RGBA(34, 70, 63, 255);
static const Color COL_AUDIO = RGBA(28, 48, 55, 255);
static const Color COL_TITLE = RGBA(101, 84, 232, 255);
static const Color COL_BORDER = RGBA(255, 255, 255, 18);
static const Color COL_LIGHT_BUTTON = RGBA(241, 241, 244, 255);
static const Color COL_LIGHT_TEXT = RGBA(13, 13, 15, 255);

typedef struct { float x, y, w, h; } Rect;

static float Clampf(float v, float lo, float hi) { return v < lo ? lo : v > hi ? hi : v; }
static int Clampi(int v, int lo, int hi) { return v < lo ? lo : v > hi ? hi : v; }
static double Clampd(double v, double lo, double hi) { return v < lo ? lo : v > hi ? hi : v; }
static Rect MakeRect(float x, float y, float w, float h) { Rect r = {x, y, w, h}; return r; }
static int RectHit(Rect r, Vector2 p) { return p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h; }
static Rect RectInset(Rect r, float amount) { return MakeRect(r.x + amount, r.y + amount, r.w - amount * 2, r.h - amount * 2); }
static double Maxd(double a, double b) { return a > b ? a : b; }
static float Minf(float a, float b) { return a < b ? a : b; }
static float Maxf(float a, float b) { return a > b ? a : b; }

static void CircleGradient(Vector2 center, float radius, Color inner, Color outer) {
    DrawCircleGradient((int)center.x, (int)center.y, radius, inner, outer);
}

static void CopyStr(char *dst, size_t cap, const char *src) {
    if (!dst || cap == 0) return;
    if (!src) src = "";
    size_t length = strlen(src);
    if (length >= cap) length = cap - 1;
    memcpy(dst, src, length);
    dst[length] = '\0';
}

static int ContainsCI(const char *haystack, const char *needle) {
    if (!needle || !needle[0]) return 1;
    if (!haystack) return 0;
    size_t n = strlen(needle);
    for (const char *p = haystack; *p; ++p) {
        size_t i = 0;
        while (i < n && p[i] && tolower((unsigned char)p[i]) == tolower((unsigned char)needle[i])) i++;
        if (i == n) return 1;
    }
    return 0;
}

/* ========================================================================== */
/* Fonts and text                                                             */
/* ========================================================================== */

static Font g_ui_font;
static Font g_title_font;

static Font LoadBestFont(int size) {
    const char *paths[] = {
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/segoeuib.ttf",
        "C:/Windows/Fonts/arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        NULL,
    };
    for (int i = 0; paths[i]; i++) {
        if (FileExists(paths[i])) {
            Font font = LoadFontEx(paths[i], size, NULL, 0);
            if (font.texture.id > 0) {
                SetTextureFilter(font.texture, TEXTURE_FILTER_BILINEAR);
                return font;
            }
        }
    }
    Font fallback = GetFontDefault();
    return fallback;
}

static float TxtW(const char *text, float size) {
    return MeasureTextEx(g_ui_font, text ? text : "", size, 0.2f).x;
}

static void Txt(const char *text, float x, float y, float size, Color color) {
    DrawTextEx(g_ui_font, text ? text : "", (Vector2){x, y}, size, 0.2f, color);
}

static void TxtC(const char *text, float cx, float y, float size, Color color) {
    DrawTextEx(g_ui_font, text ? text : "", (Vector2){cx - TxtW(text, size) * 0.5f, y}, size, 0.2f, color);
}

static void TxtR(const char *text, float rx, float y, float size, Color color) {
    DrawTextEx(g_ui_font, text ? text : "", (Vector2){rx - TxtW(text, size), y}, size, 0.2f, color);
}

static void TxtEllipsis(const char *text, float x, float y, float size, float max_width, Color color) {
    char buffer[512];
    CopyStr(buffer, sizeof(buffer), text);
    if (TxtW(buffer, size) <= max_width) {
        Txt(buffer, x, y, size, color);
        return;
    }
    size_t length = strlen(buffer);
    while (length > 1) {
        length--;
        while (length > 0 && ((unsigned char)buffer[length] & 0xC0) == 0x80) length--;
        buffer[length] = '\0';
        char attempt[520];
        snprintf(attempt, sizeof(attempt), "%s…", buffer);
        if (TxtW(attempt, size) <= max_width) {
            Txt(attempt, x, y, size, color);
            return;
        }
    }
    Txt("…", x, y, size, color);
}

static void DrawTrackedLabel(const char *text, float x, float y, float size, float spacing, Color color) {
    float cursor = x;
    char glyph[2] = {0, 0};
    for (const char *p = text; *p; p++) {
        glyph[0] = *p;
        DrawTextEx(g_ui_font, glyph, (Vector2){cursor, y}, size, 0.2f, color);
        cursor += TxtW(glyph, size) + spacing;
    }
}

/* ========================================================================== */
/* Icons                                                                      */
/* ========================================================================== */

typedef enum {
    ICON_PLAY, ICON_PAUSE, ICON_PLUS, ICON_MINUS, ICON_SEARCH, ICON_FILM, ICON_MUSIC,
    ICON_TYPE, ICON_SPARKLES, ICON_SUN, ICON_DROPLET, ICON_SLIDERS, ICON_GRID, ICON_REPEAT,
    ICON_TRASH, ICON_X, ICON_CHECK, ICON_CHEV_LEFT, ICON_CHEV_RIGHT, ICON_CHEV_UP, ICON_CHEV_DOWN,
    ICON_SKIP_BACK, ICON_SKIP_FORWARD, ICON_ZOOM_IN, ICON_ZOOM_OUT, ICON_CURSOR, ICON_HAND,
    ICON_SCISSORS, ICON_MAGNET, ICON_LINK, ICON_RESET, ICON_DOWNLOAD, ICON_FOLDER, ICON_WIN_MIN,
    ICON_WIN_MAX, ICON_WIN_RESTORE, ICON_WIN_CLOSE, ICON_WHEEL, ICON_SCALE, ICON_POSITION,
    ICON_CROP, ICON_DISTORT, ICON_LAYERS, ICON_GLITCH, ICON_PIXEL, ICON_APERTURE, ICON_SUNRISE,
    ICON_IMAGE, ICON_VIDEO, ICON_WAVE, ICON_TRANSITION, ICON_COUNT
} IconId;

static void DrawIcon(IconId icon, float x, float y, float size, Color color) {
    float t = Maxf(1.25f, size * 0.08f);
    float cx = x + size * 0.5f;
    float cy = y + size * 0.5f;
#define PX(n) (x + (n) * size)
#define PY(n) (y + (n) * size)
#define LN(x1, y1, x2, y2) DrawLineEx((Vector2){PX(x1), PY(y1)}, (Vector2){PX(x2), PY(y2)}, t, color)
#define DOT(nx, ny, r) DrawCircleV((Vector2){PX(nx), PY(ny)}, (r) * size, color)
#define RING(nx, ny, r) DrawRing((Vector2){PX(nx), PY(ny)}, (r) * size - t * 0.5f, (r) * size + t * 0.5f, 0, 360, 32, color)
#define BOX(nx, ny, nw, nh, round) DrawRectangleRounded((Rectangle){PX(nx), PY(ny), (nw) * size, (nh) * size}, (round), 5, color)
    switch (icon) {
        case ICON_PLAY:
            DrawTriangle((Vector2){PX(0.34f), PY(0.20f)}, (Vector2){PX(0.34f), PY(0.80f)}, (Vector2){PX(0.78f), PY(0.50f)}, color);
            break;
        case ICON_PAUSE:
            BOX(0.30f, 0.22f, 0.13f, 0.56f, 0.15f);
            BOX(0.57f, 0.22f, 0.13f, 0.56f, 0.15f);
            break;
        case ICON_PLUS:
            LN(0.24f, 0.5f, 0.76f, 0.5f);
            LN(0.5f, 0.24f, 0.5f, 0.76f);
            break;
        case ICON_MINUS:
            LN(0.24f, 0.5f, 0.76f, 0.5f);
            break;
        case ICON_SEARCH:
            RING(0.44f, 0.44f, 0.22f);
            LN(0.60f, 0.60f, 0.80f, 0.80f);
            break;
        case ICON_FILM:
            DrawRectangleRoundedLinesEx((Rectangle){PX(0.14f), PY(0.22f), size * 0.72f, size * 0.56f}, 0.12f, 4, t, color);
            LN(0.32f, 0.22f, 0.32f, 0.78f);
            LN(0.68f, 0.22f, 0.68f, 0.78f);
            break;
        case ICON_MUSIC:
            DOT(0.34f, 0.70f, 0.09f);
            DOT(0.72f, 0.64f, 0.09f);
            LN(0.42f, 0.70f, 0.42f, 0.30f);
            LN(0.80f, 0.64f, 0.80f, 0.24f);
            LN(0.42f, 0.30f, 0.80f, 0.24f);
            break;
        case ICON_TYPE:
            LN(0.22f, 0.24f, 0.78f, 0.24f);
            LN(0.50f, 0.24f, 0.50f, 0.78f);
            LN(0.36f, 0.78f, 0.64f, 0.78f);
            break;
        case ICON_SPARKLES:
            DrawTriangle((Vector2){PX(0.5f), PY(0.12f)}, (Vector2){PX(0.62f), PY(0.42f)}, (Vector2){PX(0.38f), PY(0.42f)}, color);
            DrawTriangle((Vector2){PX(0.5f), PY(0.88f)}, (Vector2){PX(0.62f), PY(0.58f)}, (Vector2){PX(0.38f), PY(0.58f)}, color);
            DrawTriangle((Vector2){PX(0.12f), PY(0.5f)}, (Vector2){PX(0.42f), PY(0.38f)}, (Vector2){PX(0.42f), PY(0.62f)}, color);
            DrawTriangle((Vector2){PX(0.88f), PY(0.5f)}, (Vector2){PX(0.58f), PY(0.38f)}, (Vector2){PX(0.58f), PY(0.62f)}, color);
            break;
        case ICON_SUN:
            RING(0.5f, 0.5f, 0.17f);
            for (int i = 0; i < 8; i++) {
                float a = (float)i * PI / 4.0f;
                LN(0.5f + cosf(a) * 0.28f, 0.5f + sinf(a) * 0.28f, 0.5f + cosf(a) * 0.38f, 0.5f + sinf(a) * 0.38f);
            }
            break;
        case ICON_DROPLET:
            DrawCircleSector((Vector2){PX(0.5f), PY(0.58f)}, size * 0.30f, 0, 180, 24, color);
            DrawTriangle((Vector2){PX(0.5f), PY(0.14f)}, (Vector2){PX(0.22f), PY(0.58f)}, (Vector2){PX(0.78f), PY(0.58f)}, color);
            break;
        case ICON_SLIDERS:
            for (int i = 0; i < 3; i++) {
                float yy = 0.26f + i * 0.24f;
                LN(0.16f, yy, 0.84f, yy);
                DOT(i == 0 ? 0.34f : i == 1 ? 0.66f : 0.46f, yy, 0.065f);
            }
            break;
        case ICON_GRID:
            DrawRectangleRoundedLinesEx((Rectangle){PX(0.16f), PY(0.16f), size * 0.68f, size * 0.68f}, 0.08f, 4, t, color);
            LN(0.16f, 0.39f, 0.84f, 0.39f);
            LN(0.16f, 0.61f, 0.84f, 0.61f);
            LN(0.39f, 0.16f, 0.39f, 0.84f);
            LN(0.61f, 0.16f, 0.61f, 0.84f);
            break;
        case ICON_REPEAT:
            LN(0.20f, 0.34f, 0.72f, 0.34f);
            LN(0.72f, 0.34f, 0.62f, 0.24f);
            LN(0.72f, 0.34f, 0.62f, 0.44f);
            LN(0.80f, 0.66f, 0.28f, 0.66f);
            LN(0.28f, 0.66f, 0.38f, 0.56f);
            LN(0.28f, 0.66f, 0.38f, 0.76f);
            break;
        case ICON_TRASH:
            LN(0.20f, 0.28f, 0.80f, 0.28f);
            LN(0.30f, 0.28f, 0.34f, 0.82f);
            LN(0.70f, 0.28f, 0.66f, 0.82f);
            LN(0.34f, 0.82f, 0.66f, 0.82f);
            LN(0.41f, 0.22f, 0.59f, 0.22f);
            break;
        case ICON_X:
            LN(0.24f, 0.24f, 0.76f, 0.76f);
            LN(0.76f, 0.24f, 0.24f, 0.76f);
            break;
        case ICON_CHECK:
            LN(0.20f, 0.52f, 0.42f, 0.74f);
            LN(0.42f, 0.74f, 0.82f, 0.28f);
            break;
        case ICON_CHEV_LEFT: LN(0.62f, 0.24f, 0.36f, 0.5f); LN(0.36f, 0.5f, 0.62f, 0.76f); break;
        case ICON_CHEV_RIGHT: LN(0.38f, 0.24f, 0.64f, 0.5f); LN(0.64f, 0.5f, 0.38f, 0.76f); break;
        case ICON_CHEV_UP: LN(0.24f, 0.62f, 0.5f, 0.36f); LN(0.5f, 0.36f, 0.76f, 0.62f); break;
        case ICON_CHEV_DOWN: LN(0.24f, 0.38f, 0.5f, 0.64f); LN(0.5f, 0.64f, 0.76f, 0.38f); break;
        case ICON_TRANSITION: {
            /* Two arrows chasing each other around a circle, like a cross-fade. */
            DrawCircleV((Vector2){PX(0.5f), PY(0.5f)}, size * 0.46f, color);
            DrawCircleV((Vector2){PX(0.5f), PY(0.5f)}, size * 0.42f, BLANK);
            DrawTriangle((Vector2){PX(0.68f), PY(0.34f)}, (Vector2){PX(0.68f), PY(0.66f)}, (Vector2){PX(0.5f), PY(0.5f)}, color);
            DrawTriangle((Vector2){PX(0.32f), PY(0.66f)}, (Vector2){PX(0.32f), PY(0.34f)}, (Vector2){PX(0.5f), PY(0.5f)}, color);
            break;
        }
        case ICON_SKIP_BACK:
            DrawTriangle((Vector2){PX(0.72f), PY(0.24f)}, (Vector2){PX(0.72f), PY(0.76f)}, (Vector2){PX(0.30f), PY(0.5f)}, color);
            LN(0.22f, 0.24f, 0.22f, 0.76f);
            break;
        case ICON_SKIP_FORWARD:
            DrawTriangle((Vector2){PX(0.28f), PY(0.24f)}, (Vector2){PX(0.28f), PY(0.76f)}, (Vector2){PX(0.70f), PY(0.5f)}, color);
            LN(0.78f, 0.24f, 0.78f, 0.76f);
            break;
        case ICON_ZOOM_IN:
        case ICON_ZOOM_OUT:
            RING(0.42f, 0.42f, 0.21f);
            LN(0.57f, 0.57f, 0.80f, 0.80f);
            LN(0.31f, 0.42f, 0.53f, 0.42f);
            if (icon == ICON_ZOOM_IN) LN(0.42f, 0.31f, 0.42f, 0.53f);
            break;
        case ICON_CURSOR:
            DrawTriangle((Vector2){PX(0.24f), PY(0.16f)}, (Vector2){PX(0.76f), PY(0.52f)}, (Vector2){PX(0.46f), PY(0.58f)}, color);
            DrawTriangle((Vector2){PX(0.24f), PY(0.16f)}, (Vector2){PX(0.46f), PY(0.58f)}, (Vector2){PX(0.38f), PY(0.82f)}, color);
            break;
        case ICON_HAND:
            BOX(0.30f, 0.36f, 0.40f, 0.44f, 0.18f);
            for (int i = 0; i < 4; i++) BOX(0.30f + i * 0.10f, 0.20f + (i == 0 ? 0.10f : 0.0f), 0.07f, 0.26f, 0.5f);
            break;
        case ICON_SCISSORS:
            RING(0.26f, 0.28f, 0.10f);
            RING(0.26f, 0.74f, 0.10f);
            LN(0.34f, 0.35f, 0.80f, 0.76f);
            LN(0.34f, 0.67f, 0.80f, 0.26f);
            break;
        case ICON_MAGNET:
            DrawRing((Vector2){cx, PY(0.56f)}, size * 0.22f, size * 0.32f, 0, 180, 28, color);
            BOX(0.18f, 0.24f, 0.16f, 0.32f, 0.2f);
            BOX(0.66f, 0.24f, 0.16f, 0.32f, 0.2f);
            break;
        case ICON_LINK:
            RING(0.34f, 0.5f, 0.18f);
            RING(0.66f, 0.5f, 0.18f);
            LN(0.34f, 0.5f, 0.66f, 0.5f);
            break;
        case ICON_RESET:
            DrawRing((Vector2){cx, cy}, size * 0.26f, size * 0.34f, 40, 330, 28, color);
            DrawTriangle((Vector2){PX(0.72f), PY(0.18f)}, (Vector2){PX(0.88f), PY(0.34f)}, (Vector2){PX(0.66f), PY(0.38f)}, color);
            break;
        case ICON_DOWNLOAD:
            LN(0.5f, 0.16f, 0.5f, 0.62f);
            LN(0.5f, 0.62f, 0.34f, 0.46f);
            LN(0.5f, 0.62f, 0.66f, 0.46f);
            LN(0.22f, 0.78f, 0.78f, 0.78f);
            break;
        case ICON_FOLDER:
            LN(0.14f, 0.30f, 0.42f, 0.30f);
            LN(0.42f, 0.30f, 0.50f, 0.40f);
            LN(0.14f, 0.30f, 0.14f, 0.78f);
            LN(0.14f, 0.78f, 0.86f, 0.78f);
            LN(0.86f, 0.78f, 0.86f, 0.40f);
            LN(0.86f, 0.40f, 0.50f, 0.40f);
            break;
        case ICON_WIN_MIN: LN(0.26f, 0.5f, 0.74f, 0.5f); break;
        case ICON_WIN_MAX: DrawRectangleRoundedLinesEx((Rectangle){PX(0.26f), PY(0.26f), size * 0.48f, size * 0.48f}, 0.08f, 3, t, color); break;
        case ICON_WIN_RESTORE:
            DrawRectangleRoundedLinesEx((Rectangle){PX(0.34f), PY(0.20f), size * 0.42f, size * 0.42f}, 0.08f, 3, t, color);
            DrawRectangleRoundedLinesEx((Rectangle){PX(0.22f), PY(0.34f), size * 0.42f, size * 0.42f}, 0.08f, 3, t, color);
            break;
        case ICON_WIN_CLOSE: LN(0.28f, 0.28f, 0.72f, 0.72f); LN(0.72f, 0.28f, 0.28f, 0.72f); break;
        case ICON_WHEEL:
            RING(0.5f, 0.5f, 0.32f);
            for (int i = 0; i < 4; i++) {
                float a = (float)i * PI / 2.0f + PI / 4.0f;
                LN(0.5f + cosf(a) * 0.16f, 0.5f + sinf(a) * 0.16f, 0.5f + cosf(a) * 0.31f, 0.5f + sinf(a) * 0.31f);
            }
            break;
        case ICON_SCALE:
            LN(0.18f, 0.82f, 0.82f, 0.18f);
            LN(0.82f, 0.18f, 0.60f, 0.20f);
            LN(0.82f, 0.18f, 0.80f, 0.40f);
            LN(0.18f, 0.82f, 0.40f, 0.80f);
            LN(0.18f, 0.82f, 0.20f, 0.60f);
            break;
        case ICON_POSITION:
            RING(0.5f, 0.5f, 0.14f);
            LN(0.5f, 0.12f, 0.5f, 0.32f);
            LN(0.5f, 0.68f, 0.5f, 0.88f);
            LN(0.12f, 0.5f, 0.32f, 0.5f);
            LN(0.68f, 0.5f, 0.88f, 0.5f);
            break;
        case ICON_CROP:
            LN(0.24f, 0.14f, 0.24f, 0.70f);
            LN(0.24f, 0.70f, 0.82f, 0.70f);
            LN(0.14f, 0.30f, 0.70f, 0.30f);
            LN(0.70f, 0.30f, 0.70f, 0.86f);
            break;
        case ICON_DISTORT:
            LN(0.22f, 0.30f, 0.62f, 0.14f);
            LN(0.62f, 0.14f, 0.84f, 0.66f);
            LN(0.84f, 0.66f, 0.40f, 0.86f);
            LN(0.40f, 0.86f, 0.22f, 0.30f);
            break;
        case ICON_LAYERS:
            for (int i = 0; i < 3; i++) {
                float yy = 0.30f + i * 0.15f;
                DrawTriangle((Vector2){PX(0.5f), PY(yy - 0.10f)}, (Vector2){PX(0.82f), PY(yy)}, (Vector2){PX(0.5f), PY(yy + 0.10f)}, color);
                DrawTriangle((Vector2){PX(0.5f), PY(yy - 0.10f)}, (Vector2){PX(0.18f), PY(yy)}, (Vector2){PX(0.5f), PY(yy + 0.10f)}, color);
            }
            break;
        case ICON_GLITCH:
            DrawTriangle((Vector2){PX(0.56f), PY(0.12f)}, (Vector2){PX(0.28f), PY(0.56f)}, (Vector2){PX(0.48f), PY(0.56f)}, color);
            DrawTriangle((Vector2){PX(0.44f), PY(0.88f)}, (Vector2){PX(0.72f), PY(0.44f)}, (Vector2){PX(0.52f), PY(0.44f)}, color);
            break;
        case ICON_PIXEL:
            for (int yy = 0; yy < 3; yy++) for (int xx = 0; xx < 3; xx++)
                if ((xx + yy) % 2 == 0) BOX(0.18f + xx * 0.22f, 0.18f + yy * 0.22f, 0.18f, 0.18f, 0.15f);
            break;
        case ICON_APERTURE:
            RING(0.5f, 0.5f, 0.32f);
            for (int i = 0; i < 6; i++) {
                float a = (float)i * PI / 3.0f;
                LN(0.5f + cosf(a) * 0.10f, 0.5f + sinf(a) * 0.10f, 0.5f + cosf(a) * 0.30f, 0.5f + sinf(a) * 0.30f);
            }
            break;
        case ICON_SUNRISE:
            DrawRing((Vector2){cx, PY(0.58f)}, size * 0.16f, size * 0.24f, 180, 360, 24, color);
            LN(0.14f, 0.58f, 0.86f, 0.58f);
            LN(0.50f, 0.14f, 0.50f, 0.26f);
            break;
        case ICON_IMAGE:
            DrawRectangleRoundedLinesEx((Rectangle){PX(0.14f), PY(0.20f), size * 0.72f, size * 0.60f}, 0.10f, 4, t, color);
            DrawTriangle((Vector2){PX(0.20f), PY(0.70f)}, (Vector2){PX(0.42f), PY(0.42f)}, (Vector2){PX(0.60f), PY(0.70f)}, color);
            DrawTriangle((Vector2){PX(0.48f), PY(0.70f)}, (Vector2){PX(0.64f), PY(0.52f)}, (Vector2){PX(0.80f), PY(0.70f)}, color);
            DOT(0.68f, 0.34f, 0.05f);
            break;
        case ICON_VIDEO:
            DrawRectangleRoundedLinesEx((Rectangle){PX(0.12f), PY(0.30f), size * 0.52f, size * 0.40f}, 0.12f, 4, t, color);
            DrawTriangle((Vector2){PX(0.68f), PY(0.42f)}, (Vector2){PX(0.88f), PY(0.32f)}, (Vector2){PX(0.88f), PY(0.68f)}, color);
            break;
        case ICON_WAVE:
            for (int i = 0; i < 9; i++) {
                float xx = 0.14f + i * 0.09f;
                float h = 0.10f + 0.26f * fabsf(sinf(i * 1.7f));
                LN(xx, 0.5f - h, xx, 0.5f + h);
            }
            break;
        default:
            RING(0.5f, 0.5f, 0.30f);
            break;
    }
#undef PX
#undef PY
#undef LN
#undef DOT
#undef RING
#undef BOX
}

/* ========================================================================== */
/* Shared drawing helpers                                                     */
/* ========================================================================== */

static void RoundRect(Rect r, float radius, Color fill) {
    DrawRectangleRounded((Rectangle){r.x, r.y, r.w, r.h}, Clampf(radius / Maxf(1.0f, Minf(r.w, r.h) * 0.5f), 0.0f, 1.0f), 8, fill);
}

static void RoundRectBorder(Rect r, float radius, Color fill, float thickness, Color border) {
    RoundRect(r, radius, fill);
    DrawRectangleRoundedLinesEx((Rectangle){r.x, r.y, r.w, r.h}, Clampf(radius / Maxf(1.0f, Minf(r.w, r.h) * 0.5f), 0.0f, 1.0f), 8, thickness, border);
}

static void DrawSparkleBadge(float x, float y) {
    DrawIcon(ICON_SPARKLES, x, y, 13, RGBA(196, 181, 253, 255));
}

/* ========================================================================== */
/* Data model                                                                 */
/* ========================================================================== */

typedef struct {
    const char *key;
    const char *label;
    IconId icon;
    int count;
    struct {
        const char *key;
        const char *label;
        float min;
        float max;
        float def;
        const char *unit;
        int decimals;
        int choice;
        const char *choices[3];
    } params[4];
} EffectDef;

enum {
    FX_BLUR, FX_GRAIN, FX_VIGNETTE,
    FX_CHROMATIC, FX_GLOW, FX_GLITCH, FX_PIXELATE, FX_SHARPEN, FX_LENS, FX_SCANLINES, FX_SHAKE,
    FX_FILMBURN, FX_HALFTONE, FX_POSTERIZE, FX_FISHEYE, FX_LIGHTLEAK, FX_MIRROR,
    FX_EQ, FX_REVERB, FX_DELAY, FX_COMPRESSOR, FX_LIMITER, FX_DENOISE,
    FX_COUNT
};

static const EffectDef EFFECTS[FX_COUNT] = {
    {"blur", "Blur", ICON_WAVE, 3, {{"mode", "Mode", 0, 2, 0, "", 0, 1, {"Gaussian", "Motion", "Zoom"}},
                                    {"amount", "Amount", 0, 100, 25, "", 0, 0, {0}},
                                    {"angle", "Angle", -180, 180, 0, "°", 0, 0, {0}}}},
    {"grain", "Grain", ICON_GRID, 3, {{"amount", "Amount", 0, 100, 35, "", 0, 0, {0}},
                                      {"size", "Size", 0, 100, 30, "", 0, 0, {0}},
                                      {"roughness", "Roughness", 0, 100, 50, "", 0, 0, {0}}}},
    {"vignette", "Vignette", ICON_APERTURE, 4, {{"amount", "Amount", -100, 100, -40, "", 0, 0, {0}},
                                                {"midpoint", "Midpoint", 0, 100, 50, "", 0, 0, {0}},
                                                {"feather", "Feather", 0, 100, 50, "", 0, 0, {0}},
                                                {"roundness", "Roundness", -100, 100, 0, "", 0, 0, {0}}}},
    {"chromatic", "Chromatic aberration", ICON_GLITCH, 2, {{"amount", "Amount", 0, 100, 30, "", 0, 0, {0}},
                                                           {"angle", "Angle", -180, 180, 0, "°", 0, 0, {0}}}},
    {"glow", "Glow", ICON_SUN, 3, {{"amount", "Amount", 0, 100, 30, "", 0, 0, {0}},
                                   {"threshold", "Threshold", 0, 100, 70, "", 0, 0, {0}},
                                   {"radius", "Radius", 0, 100, 40, "", 0, 0, {0}}}},
    {"glitch", "Glitch", ICON_GLITCH, 3, {{"amount", "Amount", 0, 100, 30, "", 0, 0, {0}},
                                          {"blocks", "Block size", 0, 100, 25, "", 0, 0, {0}},
                                          {"speed", "Speed", 0, 100, 50, "", 0, 0, {0}}}},
    {"pixelate", "Pixelate", ICON_PIXEL, 2, {{"size", "Pixel size", 1, 80, 12, "px", 0, 0, {0}},
                                             {"blend", "Blend", 0, 100, 100, "%", 0, 0, {0}}}},
    {"sharpen", "Sharpen", ICON_SLIDERS, 3, {{"amount", "Amount", 0, 100, 35, "", 0, 0, {0}},
                                             {"radius", "Radius", 0, 20, 5, "px", 0, 0, {0}},
                                             {"threshold", "Threshold", 0, 100, 10, "", 0, 0, {0}}}},
    {"lens", "Lens Distortion", ICON_DISTORT, 2, {{"amount", "Distortion", -100, 100, 0, "", 0, 0, {0}},
                                                  {"scale", "Scale", 50, 150, 100, "%", 0, 0, {0}}}},
    {"scanlines", "Scanlines", ICON_GRID, 2, {{"amount", "Opacity", 0, 100, 25, "", 0, 0, {0}},
                                              {"spacing", "Spacing", 1, 12, 3, "px", 0, 0, {0}}}},
    {"shake", "Camera Shake", ICON_POSITION, 3, {{"amount", "Intensity", 0, 100, 20, "", 0, 0, {0}},
                                                 {"frequency", "Frequency", 1, 30, 8, "Hz", 0, 0, {0}},
                                                 {"smoothness", "Smoothness", 0, 100, 35, "", 0, 0, {0}}}},
    {"filmBurn", "Film Burn", ICON_SUN, 4, {{"amount", "Amount", 0, 100, 22, "", 0, 0, {0}},
                                            {"warmth", "Warmth", 0, 100, 65, "", 0, 0, {0}},
                                            {"edge", "Edge Spread", 0, 100, 38, "", 0, 0, {0}},
                                            {"flicker", "Flicker", 0, 100, 12, "", 0, 0, {0}}}},
    {"halftone", "Halftone", ICON_GRID, 3, {{"size", "Dot Size", 1, 24, 5, "px", 0, 0, {0}},
                                            {"contrast", "Contrast", 0, 100, 48, "", 0, 0, {0}},
                                            {"angle", "Angle", 0, 180, 45, "°", 0, 0, {0}}}},
    {"posterize", "Posterize", ICON_LAYERS, 2, {{"levels", "Color Levels", 2, 16, 6, "", 0, 0, {0}},
                                                {"blend", "Blend", 0, 100, 100, "%", 0, 0, {0}}}},
    {"fisheye", "Fisheye", ICON_APERTURE, 3, {{"amount", "Distortion", -100, 100, 35, "", 0, 0, {0}},
                                              {"scale", "Scale", 50, 150, 100, "%", 0, 0, {0}},
                                              {"blend", "Blend", 0, 100, 100, "%", 0, 0, {0}}}},
    {"lightLeak", "Light Leak", ICON_SUNRISE, 3, {{"amount", "Intensity", 0, 100, 28, "", 0, 0, {0}},
                                                  {"position", "Position", 0, 100, 35, "", 0, 0, {0}},
                                                  {"warmth", "Warmth", 0, 100, 70, "", 0, 0, {0}}}},
    {"mirror", "Mirror", ICON_LAYERS, 2, {{"axis", "Axis", 0, 1, 0, "", 0, 1, {"Horizontal", "Vertical", 0}},
                                          {"blend", "Blend", 0, 100, 100, "%", 0, 0, {0}}}},
    {"eq", "Equalizer", ICON_MUSIC, 3, {{"low", "Low Gain", -20, 20, 0, "dB", 0, 0, {0}},
                                        {"mid", "Mid Gain", -20, 20, 0, "dB", 0, 0, {0}},
                                        {"high", "High Gain", -20, 20, 0, "dB", 0, 0, {0}}}},
    {"reverb", "Reverb", ICON_WAVE, 3, {{"room", "Room Size", 0, 100, 30, "", 0, 0, {0}},
                                        {"damp", "Damping", 0, 100, 50, "", 0, 0, {0}},
                                        {"wet", "Wet/Dry", 0, 100, 25, "", 0, 0, {0}}}},
    {"delay", "Delay", ICON_REPEAT, 3, {{"time", "Time", 1, 2000, 320, "ms", 0, 0, {0}},
                                        {"feedback", "Feedback", 0, 100, 28, "", 0, 0, {0}},
                                        {"mix", "Mix", 0, 100, 22, "", 0, 0, {0}}}},
    {"compressor", "Compressor", ICON_SLIDERS, 4, {{"threshold", "Threshold", -60, 0, -18, "dB", 0, 0, {0}},
                                                    {"ratio", "Ratio", 1, 20, 4, ":1", 1, 0, {0}},
                                                    {"attack", "Attack", 0, 200, 12, "ms", 0, 0, {0}},
                                                    {"release", "Release", 5, 1000, 140, "ms", 0, 0, {0}}}},
    {"limiter", "Limiter", ICON_SLIDERS, 2, {{"ceiling", "Output Ceiling", -12, 0, -1, "dB", 1, 0, {0}},
                                                         {"release", "Release", 1, 500, 80, "ms", 0, 0, {0}}}},
    {"denoise", "Noise Reduction", ICON_GRID, 2, {{"reduction", "Reduction", 0, 100, 35, "", 0, 0, {0}},
                                                   {"sensitivity", "Sensitivity", 0, 100, 50, "", 0, 0, {0}}}},
};

/* Two names were used above for icon readability. */

static const char *PRESET_IDS[] = {"none", "vivid", "noir", "fade", "warm", "cool", "cinema", "retro"};
static const char *PRESET_LABELS[] = {"None", "Vivid", "Noir", "Fade", "Warm", "Cool", "Cinema", "Retro"};
#define PRESET_COUNT 8

typedef struct {
    char id[PLUTO_ID_SIZE];
    char track[PLUTO_TRACK_SIZE];
    double start;
    double dur;
    double source_offset;
    double source_duration;
    char label[PLUTO_LABEL_SIZE];
    char path[PLUTO_PATH_SIZE];
    char media_kind[PLUTO_KIND_SIZE];
    char theme[PLUTO_THEME_SIZE];
    int has_audio;
    char linked_audio_id[PLUTO_ID_SIZE];

    /* Transform */
    float scale_w, scale_h;
    float pos_x, pos_y;
    float rotation;
    float opacity;
    int scale_linked, pos_linked, crop_linked, distort_linked;
    float crop_t, crop_r, crop_b, crop_l;
    float distort_tl, distort_tr, distort_br, distort_bl;

    /* Colors */
    float light[7];
    float color[3];
    float detail[3];
    struct { float hue, amount; } grade[3];
    struct { float h, s, l; } hsl;

    /* Effects */
    int fx_enabled[FX_COUNT];
    float fx[FX_COUNT][4];
    int preset;
    float preset_intensity;

    /* Audio */
    float volume, pan, bass, treble;
    int fade_in, fade_out;
    float fade_in_duration, fade_out_duration;

    /* Text */
    char title[128];
    char subtitle[128];
    char caption[256];
    float text_size;
    int text_align; /* 0 left, 1 center, 2 right */

    /* Transition into the clip that follows on the same track */
    char trans_type[PLUTO_TRANS_SIZE];
    float trans_dur;
} Clip;

typedef struct {
    char name[256];
    char path[PLUTO_PATH_SIZE];
    char kind[PLUTO_KIND_SIZE];
    double duration;
    int has_audio;
    Texture2D thumbnail;
    Texture2D filmstrip;
    Texture2D waveform;
    int has_thumbnail;
    int has_filmstrip;
    int has_waveform;
} MediaItem;

typedef struct {
    Clip *clips;
    int count;
    int selected;
    double time;
} Snapshot;

typedef enum { TOOL_SELECT, TOOL_HAND, TOOL_RAZOR, TOOL_TEXT } Tool;

typedef enum { TAB_TRANSFORM, TAB_COLORS, TAB_EFFECTS, TAB_AUDIO, TAB_TEXT, TAB_COUNT } Tab;

typedef struct {
    Rect rect;
    char *buffer;
    int cap;
    const char *placeholder;
    int active;
    int numeric;
    int history;
    int pushed;
} TextField;

typedef struct {
    int mode;            /* 0 image, 1 video */
    int image_format;    /* png jpg webp avif */
    float image_quality;
    int transparent, metadata, color_space; /* color space 0 sRGB, 1 P3 */
    int video_format;    /* mp4 webm gif */
    int fps_index;       /* 24, 30, 60 */
    int video_quality;   /* low, medium, high, max */
    int include_audio, loop;
    int size_index;      /* presets + custom */
    int width, height;
    char name[64];
    char width_text[16];
    char height_text[16];
} ExportSettings;

typedef struct {
    int active;
    int mode; /* 1 move, 2 trim left, 3 trim right */
    int index;
    Clip backup;
    double grab_time;
    int target_track;
    int moved;
    double press_start;
    double press_dur;
    double press_offset;
    float press_x;
} DragState;

typedef struct {
    int width, height;
    int should_close;

    int backend_ok;
    char backend_error[PLUTO_MESSAGE_SIZE * 2];

    char project_name[64];
    TextField name_field;

    /* Library */
    MediaItem media[MAX_MEDIA];
    int media_count;
    int library_tab;          /* 0 media, 1 audio */
    char search[96];
    TextField search_field;
    float media_scroll;
    float audio_scroll;
    int importing;

    /* Timeline */
    Clip clips[MAX_CLIPS];
    int clip_count;
    int next_clip;
    int selected;
    int tab;
    int tool;
    float pps;
    double playhead;
    int playing;
    int loop;
    int guides;
    int snapping;
    float scroll_x;
    float scroll_y;
    double last_tick;

    DragState drag;
    double last_click_time;
    int last_click_index;
    int dragging_playhead;

    /* Inspector */
    int light_open, color_open, detail_open, grading_open, hsl_open;
    int hsl_channel;
    int fx_open[FX_COUNT];
    int fx_category[3];
    int trans_open;
    int transform_page;       /* 0 scale/position, 1 crop, 2 distort */
    int link_scale, link_pos, link_crop, link_distort;
    int text_page;            /* 0 title, 1 subtitles */
    TextField text_title;
    TextField text_sub;
    TextField text_caption;
    char text_loaded_id[PLUTO_ID_SIZE];
    float inspector_content_h;

    /* Export */
    int drawer_open;
    ExportSettings export_cfg;
    TextField export_name;
    TextField export_width;
    TextField export_height;
    int export_running;
    int export_finished;
    int export_success;
    int export_canceled;
    double export_progress;
    char export_message[PLUTO_MESSAGE_SIZE];
    char export_output[PLUTO_PATH_SIZE];
    double export_polled_at;

    /* History */
    Snapshot undo[MAX_UNDO];
    int undo_count;
    Snapshot redo[MAX_UNDO];
    int redo_count;

    /* Backend property debounce */
    int props_dirty;
    char props_dirty_id[PLUTO_ID_SIZE];
    double props_dirty_since;

    /* Toast */
    char toast[256];
    double toast_until;

    /* Preview caches */
    Texture2D video_tex;
    int video_tex_ok;
    char video_tex_path[PLUTO_PATH_SIZE];
    double video_tex_time;
    double video_request_at;
    Texture2D image_tex;
    int image_tex_ok;
    char image_tex_path[PLUTO_PATH_SIZE];
    Rect preview_frame;
    int preview_dirty;

    float inspector_scroll;
    int busy_message;
} App;

static App g_app;
static App *g_app_ptr = &g_app;
static int g_slider_key = 0;
static TextField *g_focus = NULL;
static char g_props_buf[JSON_CAP];
static char g_settings_buf[JSON_CAP];

static const char *TRACK_IDS[MAX_TRACKS] = {"v4", "v3", "v2", "v1", "a1", "a2"};
static const char *TRACK_LABELS[MAX_TRACKS] = {"V4", "V3", "V2", "V1", "A1", "A2"};
static const char *TRANS_IDS[5] = {"none", "dissolve", "dip", "wipe", "iris"};

static int TrackIndex(const char *id) {
    for (int i = 0; i < MAX_TRACKS; i++) if (strcmp(TRACK_IDS[i], id) == 0) return i;
    return 0;
}

static int TrackIsAudio(int index) { return index >= 4; }

static int TrackIndexForClip(const Clip *clip) { return TrackIndex(clip->track); }

static void Toast(App *app, const char *message) {
    CopyStr(app->toast, sizeof(app->toast), message);
    app->toast_until = GetTime() + 4.0;
}

static void Toastf(App *app, const char *fmt, ...) {
    char buffer[256];
    va_list args;
    va_start(args, fmt);
    vsnprintf(buffer, sizeof(buffer), fmt, args);
    va_end(args);
    Toast(app, buffer);
}

/* ========================================================================== */
/* Small JSON writer                                                          */
/* ========================================================================== */

typedef struct {
    char *data;
    size_t cap;
    size_t len;
    int failed;
} Json;

static void JsonInit(Json *json, char *storage, size_t cap) {
    json->data = storage;
    json->cap = cap;
    json->len = 0;
    json->failed = 0;
    if (cap) storage[0] = '\0';
}

static void JsonRaw(Json *json, const char *text) {
    if (json->failed || !text) return;
    size_t length = strlen(text);
    if (json->len + length + 1 >= json->cap) {
        json->failed = 1;
        return;
    }
    memcpy(json->data + json->len, text, length);
    json->len += length;
    json->data[json->len] = '\0';
}

static void JsonFmt(Json *json, const char *fmt, ...) {
    if (json->failed) return;
    char buffer[512];
    va_list args;
    va_start(args, fmt);
    vsnprintf(buffer, sizeof(buffer), fmt, args);
    va_end(args);
    JsonRaw(json, buffer);
}

static void JsonString(Json *json, const char *text) {
    JsonRaw(json, "\"");
    if (text) {
        for (const unsigned char *p = (const unsigned char *)text; *p; p++) {
            switch (*p) {
                case '"': JsonRaw(json, "\\\""); break;
                case '\\': JsonRaw(json, "\\\\"); break;
                case '\n': JsonRaw(json, "\\n"); break;
                case '\r': JsonRaw(json, "\\r"); break;
                case '\t': JsonRaw(json, "\\t"); break;
                default:
                    if (*p < 0x20) JsonFmt(json, "\\u%04x", *p);
                    else {
                        char glyph[2] = {(char)*p, 0};
                        JsonRaw(json, glyph);
                    }
                    break;
            }
        }
    }
    JsonRaw(json, "\"");
}

/* ========================================================================== */
/* Clip helpers                                                               */
/* ========================================================================== */

static Clip DefaultClip(void) {
    Clip clip;
    memset(&clip, 0, sizeof(clip));
    clip.scale_w = clip.scale_h = 100.0f;
    clip.opacity = 100.0f;
    clip.light[0] = 0;
    clip.light[1] = 0;
    clip.light[2] = 0;
    clip.light[3] = 0;
    clip.light[4] = 0;
    clip.light[5] = 0;
    clip.light[6] = 0;
    clip.color[0] = 0;
    clip.color[1] = 0;
    clip.color[2] = 0;
    clip.detail[0] = 0;
    clip.detail[1] = 0;
    clip.detail[2] = 0;
    for (int i = 0; i < FX_COUNT; i++) {
        clip.fx_enabled[i] = 0;
        for (int p = 0; p < 4; p++) clip.fx[i][p] = EFFECTS[i].params[p].def;
    }
    clip.preset = 0;
    clip.preset_intensity = 100.0f;
    clip.volume = 100.0f;
    clip.pan = 0.0f;
    clip.bass = 0.0f;
    clip.treble = 0.0f;
    clip.fade_in_duration = 1.0f;
    clip.fade_out_duration = 1.0f;
    clip.text_size = 100.0f;
    clip.text_align = 1;
    clip.trans_dur = 1.0f;
    CopyStr(clip.theme, sizeof(clip.theme), "coast");
    return clip;
}

static Clip *SelectedClip(App *app) {
    if (app->selected < 0 || app->selected >= app->clip_count) return NULL;
    return &app->clips[app->selected];
}

static int FindClipIndex(App *app, const char *id) {
    for (int i = 0; i < app->clip_count; i++) if (strcmp(app->clips[i].id, id) == 0) return i;
    return -1;
}

static double TimelineDuration(App *app) {
    double duration = 5.0;
    for (int i = 0; i < app->clip_count; i++) {
        double end = app->clips[i].start + app->clips[i].dur;
        if (end > duration) duration = end;
    }
    return duration;
}

static double TrackEnd(App *app, const char *track) {
    double end = 0.0;
    for (int i = 0; i < app->clip_count; i++) {
        if (strcmp(app->clips[i].track, track) == 0) {
            double candidate = app->clips[i].start + app->clips[i].dur;
            if (candidate > end) end = candidate;
        }
    }
    return end;
}

static Color ThemeBaseColor(const Clip *clip) {
    if (strcmp(clip->theme, "title") == 0) return COL_TITLE;
    if (TrackIsAudio(TrackIndexForClip(clip))) return COL_AUDIO;
    if (strcmp(clip->media_kind, "audio") == 0) return COL_AUDIO;
    return COL_VIDEO;
}

static Color ClipColor(const Clip *clip) {
    Color base = ThemeBaseColor(clip);
    if (strcmp(clip->theme, "title") == 0) return base;
    if (TrackIsAudio(TrackIndexForClip(clip))) return base;
    return RGBA((int)(base.r * 1.25f), (int)(base.g * 1.25f), (int)(base.b * 1.25f), 255);
}

/* ========================================================================== */
/* History                                                                    */
/* ========================================================================== */

static Snapshot SnapshotCapture(App *app) {
    Snapshot snap;
    snap.count = app->clip_count;
    snap.selected = app->selected;
    snap.time = app->playhead;
    snap.clips = NULL;
    if (app->clip_count > 0) {
        snap.clips = (Clip *)malloc(sizeof(Clip) * (size_t)app->clip_count);
        if (snap.clips) memcpy(snap.clips, app->clips, sizeof(Clip) * (size_t)app->clip_count);
        else snap.count = 0;
    }
    return snap;
}

static void SnapshotFree(Snapshot *snap) {
    free(snap->clips);
    snap->clips = NULL;
    snap->count = 0;
}

static int SnapshotEquals(const Snapshot *snap, App *app) {
    if (snap->count != app->clip_count) return 0;
    if (snap->count > 0 && !snap->clips) return 0;
    if (snap->count > 0 && memcmp(snap->clips, app->clips, sizeof(Clip) * (size_t)snap->count) != 0) return 0;
    return snap->selected == app->selected;
}

static void UndoClear(Snapshot *stack, int *count) {
    for (int i = 0; i < *count; i++) SnapshotFree(&stack[i]);
    *count = 0;
}

static void PushSnapshot(Snapshot *stack, int *count, Snapshot snap) {
    if (*count == MAX_UNDO) {
        SnapshotFree(&stack[0]);
        memmove(&stack[0], &stack[1], sizeof(Snapshot) * (size_t)(MAX_UNDO - 1));
        (*count)--;
    }
    stack[(*count)++] = snap;
}

static void PushUndo(App *app) {
    Snapshot snap = SnapshotCapture(app);
    if (app->undo_count > 0 && SnapshotEquals(&app->undo[app->undo_count - 1], app)) {
        SnapshotFree(&snap);
        return;
    }
    PushSnapshot(app->undo, &app->undo_count, snap);
    UndoClear(app->redo, &app->redo_count);
}

static void RestoreSnapshot(App *app, const Snapshot *snap) {
    if (snap->count > 0 && !snap->clips) return;
    if (snap->count > 0) memcpy(app->clips, snap->clips, sizeof(Clip) * (size_t)snap->count);
    app->clip_count = snap->count;
    app->selected = snap->selected;
    if (app->selected >= app->clip_count) app->selected = app->clip_count - 1;
    app->playhead = snap->time;
    app->preview_dirty = 1;
}

static void Undo(App *app) {
    if (app->undo_count == 0) return;
    Snapshot current = SnapshotCapture(app);
    Snapshot previous = app->undo[--app->undo_count];
    PushSnapshot(app->redo, &app->redo_count, current);
    RestoreSnapshot(app, &previous);
    SnapshotFree(&previous);
}

static void Redo(App *app) {
    if (app->redo_count == 0) return;
    Snapshot current = SnapshotCapture(app);
    Snapshot next = app->redo[--app->redo_count];
    PushSnapshot(app->undo, &app->undo_count, current);
    RestoreSnapshot(app, &next);
    SnapshotFree(&next);
}

/* ========================================================================== */
/* Serialization                                                              */
/* ========================================================================== */

static void JsonEffects(Json *json, const Clip *clip) {
    JsonRaw(json, "{\"preset\":{\"id\":");
    JsonString(json, PRESET_IDS[clip->preset]);
    JsonFmt(json, ",\"intensity\":%.1f}", clip->preset_intensity);
    for (int i = 0; i < FX_COUNT; i++) {
        JsonFmt(json, ",\"%s\":{\"enabled\":%s", EFFECTS[i].key, clip->fx_enabled[i] ? "true" : "false");
        for (int p = 0; p < EFFECTS[i].count; p++) {
            const char *key = EFFECTS[i].params[p].key;
            float value = clip->fx[i][p];
            if (EFFECTS[i].params[p].choice) {
                int index = Clampi((int)lroundf(value), 0, 2);
                const char *label = EFFECTS[i].params[p].choices[index];
                JsonFmt(json, ",\"%s\":", key);
                JsonString(json, label ? label : "");
            } else {
                JsonFmt(json, ",\"%s\":%.4f", key, value);
            }
        }
        JsonRaw(json, "}");
    }
    JsonRaw(json, "}");
}

static void JsonColors(Json *json, const Clip *clip) {
    JsonRaw(json, "{\"light\":{");
    JsonFmt(json, "\"exposure\":%.4f,\"brightness\":%.2f,\"contrast\":%.2f,\"highlights\":%.2f,\"shadows\":%.2f,\"whites\":%.2f,\"blacks\":%.2f",
           clip->light[0], clip->light[1], clip->light[2], clip->light[3], clip->light[4], clip->light[5], clip->light[6]);
    JsonRaw(json, "},\"color\":{");
    JsonFmt(json, "\"temperature\":%.2f,\"vibrance\":%.2f,\"saturation\":%.2f", clip->color[0], clip->color[1], clip->color[2]);
    JsonRaw(json, "},\"detail\":{");
    JsonFmt(json, "\"sharpness\":%.2f,\"clarity\":%.2f,\"vignette\":%.2f", clip->detail[0], clip->detail[1], clip->detail[2]);
    JsonRaw(json, "},\"grading\":{");
    static const char *GRADE_KEYS[3] = {"shadows", "tint", "highlights"};
    for (int i = 0; i < 3; i++) {
        JsonFmt(json, "%s\"%s\":{\"hue\":%.2f,\"amount\":%.2f}", i ? "," : "", GRADE_KEYS[i], clip->grade[i].hue, clip->grade[i].amount);
    }
    JsonFmt(json, "},\"hsl\":{\"h\":%.2f,\"s\":%.2f,\"l\":%.2f}}", clip->hsl.h, clip->hsl.s, clip->hsl.l);
}

static void JsonTransform(Json *json, const Clip *clip) {
    JsonFmt(json,
            "{\"scale\":{\"w\":%.3f,\"h\":%.3f},\"position\":{\"x\":%.3f,\"y\":%.3f},\"rotation\":%.3f,\"opacity\":%.3f,"
            "\"crop\":{\"left\":%.3f,\"right\":%.3f,\"top\":%.3f,\"bottom\":%.3f},"
            "\"distort\":{\"tl\":%.3f,\"tr\":%.3f,\"br\":%.3f,\"bl\":%.3f}}",
            clip->scale_w, clip->scale_h, clip->pos_x, clip->pos_y, clip->rotation, clip->opacity,
            clip->crop_l, clip->crop_r, clip->crop_t, clip->crop_b,
            clip->distort_tl, clip->distort_tr, clip->distort_br, clip->distort_bl);
}

static void JsonAudio(Json *json, const Clip *clip) {
    JsonFmt(json,
            "{\"volume\":%.3f,\"pan\":%.3f,\"bass\":%.3f,\"treble\":%.3f,"
            "\"fadeIn\":%s,\"fadeOut\":%s,\"fadeInDuration\":%.3f,\"fadeOutDuration\":%.3f}",
            clip->volume, clip->pan, clip->bass, clip->treble,
            clip->fade_in ? "true" : "false", clip->fade_out ? "true" : "false",
            clip->fade_in_duration, clip->fade_out_duration);
}

static void JsonText(Json *json, const Clip *clip) {
    static const char *ALIGN[3] = {"left", "center", "right"};
    JsonRaw(json, "{\"title\":");
    JsonString(json, clip->title);
    JsonRaw(json, ",\"sub\":");
    JsonString(json, clip->subtitle);
    JsonFmt(json, ",\"size\":%.2f,\"caption\":", clip->text_size);
    JsonString(json, clip->caption);
    JsonRaw(json, ",\"align\":");
    JsonString(json, ALIGN[Clampi(clip->text_align, 0, 2)]);
    JsonRaw(json, "}");
}

/* Build the layout payload expected by pluto-media-engine for one clip. */
static const char *BuildClipProps(const Clip *clip) {
    Json json;
    JsonInit(&json, g_props_buf, sizeof(g_props_buf));
    JsonRaw(&json, "{\"transform\":");
    JsonTransform(&json, clip);
    JsonRaw(&json, ",\"colors\":");
    JsonColors(&json, clip);
    JsonRaw(&json, ",\"effects\":");
    JsonEffects(&json, clip);
    JsonRaw(&json, ",\"audio\":");
    JsonAudio(&json, clip);
    JsonRaw(&json, ",\"text\":");
    JsonText(&json, clip);
    JsonRaw(&json, "}");
    if (json.failed) return "{}";
    return g_props_buf;
}

static void BuildClipInfo(const Clip *clip, PlutoClipInfo *info) {
    memset(info, 0, sizeof(*info));
    CopyStr(info->id, sizeof(info->id), clip->id);
    CopyStr(info->track, sizeof(info->track), clip->track);
    info->start = clip->start;
    info->dur = clip->dur;
    CopyStr(info->label, sizeof(info->label), clip->label);
    CopyStr(info->linked_audio_id, sizeof(info->linked_audio_id), clip->linked_audio_id);
    CopyStr(info->path, sizeof(info->path), clip->path);
    CopyStr(info->media_kind, sizeof(info->media_kind), clip->media_kind);
    CopyStr(info->theme, sizeof(info->theme), clip->theme);
    info->has_audio = clip->has_audio;
    info->source_offset = clip->source_offset;
    info->source_duration = clip->source_duration;
    CopyStr(info->trans_type, sizeof(info->trans_type), clip->trans_type);
    info->trans_dur = clip->trans_dur;
}

static const char *BuildSettingsJson(App *app) {
    static const char *IMAGE[4] = {"png", "jpg", "webp", "avif"};
    static const char *VIDEO[3] = {"mp4", "webm", "gif"};
    const char *quality = "high";
    switch (app->export_cfg.video_quality) {
        case 0: quality = "low"; break;
        case 1: quality = "medium"; break;
        case 2: quality = "high"; break;
        default: quality = "max"; break;
    }
    static const int FPS[3] = {24, 30, 60};
    Json json;
    JsonInit(&json, g_settings_buf, sizeof(g_settings_buf));
    JsonFmt(&json, "{\"mode\":\"%s\",", app->export_cfg.mode == 0 ? "image" : "video");
    JsonFmt(&json, "\"image\":{\"format\":\"%s\",\"quality\":%.1f,\"transparent\":%s,\"metadata\":%s,\"colorSpace\":\"%s\"},",
            IMAGE[Clampi(app->export_cfg.image_format, 0, 3)],
            app->export_cfg.image_quality,
            app->export_cfg.transparent ? "true" : "false",
            app->export_cfg.metadata ? "true" : "false",
            app->export_cfg.color_space ? "p3" : "srgb");
    JsonFmt(&json, "\"video\":{\"format\":\"%s\",\"fps\":%d,\"quality\":\"%s\",\"audio\":%s,\"loop\":%s},",
            VIDEO[Clampi(app->export_cfg.video_format, 0, 2)],
            FPS[Clampi(app->export_cfg.fps_index, 0, 2)],
            quality,
            app->export_cfg.include_audio ? "true" : "false",
            app->export_cfg.loop ? "true" : "false");
    JsonFmt(&json, "\"size\":{\"width\":%d,\"height\":%d},", Clampi(app->export_cfg.width, 16, 7680), Clampi(app->export_cfg.height, 16, 4320));
    JsonRaw(&json, "\"output\":{\"name\":");
    JsonString(&json, app->export_cfg.name);
    JsonRaw(&json, ",\"destination\":\"\"}}");
    if (json.failed) return "{}";
    return g_settings_buf;
}

/* ========================================================================== */
/* Backend synchronization                                                    */
/* ========================================================================== */

static int BackendAddClip(App *app, const Clip *clip) {
    if (!app->backend_ok) return 0;
    PlutoClipInfo info;
    BuildClipInfo(clip, &info);
    if (!pluto_timeline_add_clip(&info, BuildClipProps(clip))) {
        Toastf(app, "Timeline backend: %s", pluto_last_error());
        return 0;
    }
    return 1;
}

static void BackendSyncAll(App *app) {
    if (!app->backend_ok) return;
    pluto_timeline_reset();
    for (int i = 0; i < app->clip_count; i++) BackendAddClip(app, &app->clips[i]);
    pluto_timeline_set_snapping(app->snapping);
}

static void BackendFlushProps(App *app, int force) {
    if (!app->backend_ok || !app->props_dirty) return;
    if (!force && GetTime() - app->props_dirty_since < 0.18) return;
    int index = FindClipIndex(app, app->props_dirty_id);
    if (index >= 0) {
        pluto_timeline_set_props(app->props_dirty_id, BuildClipProps(&app->clips[index]));
    }
    app->props_dirty = 0;
}

static void MarkPropsDirty(App *app, const Clip *clip) {
    CopyStr(app->props_dirty_id, sizeof(app->props_dirty_id), clip->id);
    app->props_dirty = 1;
    app->props_dirty_since = GetTime();
    app->preview_dirty = 1;
}

static void RemoveClipAt(App *app, int index) {
    if (index < 0 || index >= app->clip_count) return;
    PushUndo(app);
    if (app->backend_ok) {
        if (!pluto_timeline_remove_clip(app->clips[index].id)) {
            Toastf(app, "Timeline backend: %s", pluto_last_error());
        }
    }
    if (app->video_tex_ok && strcmp(app->video_tex_path, app->clips[index].path) == 0) {
        UnloadTexture(app->video_tex);
        app->video_tex_ok = 0;
        app->video_tex_path[0] = '\0';
    }
    for (int i = index; i < app->clip_count - 1; i++) app->clips[i] = app->clips[i + 1];
    app->clip_count--;
    if (app->selected >= app->clip_count) app->selected = app->clip_count - 1;
    app->preview_dirty = 1;
}

static int AddMediaClip(App *app, int media_index) {
    if (media_index < 0 || media_index >= app->media_count) return -1;
    if (app->clip_count >= MAX_CLIPS) {
        Toast(app, "The timeline is full");
        return -1;
    }
    MediaItem *media = &app->media[media_index];
    int audio = strcmp(media->kind, "audio") == 0;
    const char *track = audio ? "a1" : "v1";
    PushUndo(app);
    Clip clip = DefaultClip();
    snprintf(clip.id, sizeof(clip.id), "clip-%d", ++app->next_clip);
    CopyStr(clip.track, sizeof(clip.track), track);
    clip.start = TrackEnd(app, track);
    clip.dur = (strcmp(media->kind, "image") == 0) ? 5.0 : Maxd(media->duration, 0.5);
    clip.source_duration = media->duration;
    clip.source_offset = 0.0;
    CopyStr(clip.label, sizeof(clip.label), media->name);
    CopyStr(clip.path, sizeof(clip.path), media->path);
    CopyStr(clip.media_kind, sizeof(clip.media_kind), media->kind);
    CopyStr(clip.theme, sizeof(clip.theme), audio ? "audio" : "coast");
    clip.has_audio = media->has_audio;
    app->clips[app->clip_count] = clip;
    app->selected = app->clip_count;
    app->clip_count++;
    app->preview_dirty = 1;
    BackendAddClip(app, &clip);
    if (clip.start < app->playhead && app->playhead < clip.start + clip.dur) {
        /* Keep the playhead where the user placed it. */
    }
    return app->selected;
}

static int AddTitleClip(App *app, const char *track, double start) {
    if (app->clip_count >= MAX_CLIPS) {
        Toast(app, "The timeline is full");
        return -1;
    }
    PushUndo(app);
    Clip clip = DefaultClip();
    snprintf(clip.id, sizeof(clip.id), "clip-%d", ++app->next_clip);
    CopyStr(clip.track, sizeof(clip.track), track && track[0] ? track : "v1");
    clip.start = start < 0 ? app->playhead : start;
    clip.dur = 4.0;
    clip.source_duration = 4.0;
    CopyStr(clip.label, sizeof(clip.label), "Title");
    CopyStr(clip.theme, sizeof(clip.theme), "title");
    CopyStr(clip.media_kind, sizeof(clip.media_kind), "title");
    CopyStr(clip.title, sizeof(clip.title), "Your title");
    CopyStr(clip.subtitle, sizeof(clip.subtitle), "Add a subtitle");
    app->clips[app->clip_count] = clip;
    app->selected = app->clip_count;
    app->clip_count++;
    app->preview_dirty = 1;
    BackendAddClip(app, &clip);
    return app->selected;
}

/* ========================================================================== */
/* Media import                                                               */
/* ========================================================================== */

static Texture2D TextureFromBytes(const unsigned char *data, int len, const char *extension) {
    Texture2D texture = {0};
    if (!data || len <= 0) return texture;
    Image image = LoadImageFromMemory(extension, data, len);
    if (image.data) {
        texture = LoadTextureFromImage(image);
        UnloadImage(image);
        if (texture.id > 0) SetTextureFilter(texture, TEXTURE_FILTER_BILINEAR);
    }
    return texture;
}

static void ImportMedia(App *app) {
    char path[PLUTO_PATH_SIZE] = {0};
    if (app->importing) return;
    int result = pluto_platform_open_media(path, sizeof(path));
    if (result == -1) {
        Toast(app, "File dialogs are unavailable on this platform");
        return;
    }
    if (result == 0 || !path[0]) return;
    if (!app->backend_ok) {
        Toast(app, "The Rust backend is not loaded, so media cannot be inspected");
        return;
    }
    if (app->media_count >= MAX_MEDIA) {
        Toast(app, "The library is full");
        return;
    }
    app->importing = 1;
    PlutoMediaInfo info;
    unsigned char *thumb = NULL, *strip = NULL, *wave = NULL;
    int thumb_len = 0, strip_len = 0, wave_len = 0;
    int ok = pluto_media_inspect(path, &info, &thumb, &thumb_len, &strip, &strip_len, &wave, &wave_len);
    app->importing = 0;
    if (!ok) {
        Toastf(app, "Import failed: %s", pluto_last_error());
        return;
    }
    MediaItem item;
    memset(&item, 0, sizeof(item));
    CopyStr(item.name, sizeof(item.name), info.name[0] ? info.name : path);
    CopyStr(item.path, sizeof(item.path), path);
    CopyStr(item.kind, sizeof(item.kind), info.kind);
    item.duration = info.duration;
    item.has_audio = info.has_audio;
    if (thumb && thumb_len > 0) {
        item.thumbnail = TextureFromBytes(thumb, thumb_len, ".jpg");
        item.has_thumbnail = item.thumbnail.id > 0;
    }
    if (strip && strip_len > 0) {
        item.filmstrip = TextureFromBytes(strip, strip_len, ".png");
        item.has_filmstrip = item.filmstrip.id > 0;
    }
    if (wave && wave_len > 0) {
        item.waveform = TextureFromBytes(wave, wave_len, ".png");
        item.has_waveform = item.waveform.id > 0;
    }
    if (thumb) pluto_buffer_free(thumb, thumb_len);
    if (strip) pluto_buffer_free(strip, strip_len);
    if (wave) pluto_buffer_free(wave, wave_len);
    if (strcmp(item.kind, "image") == 0) {
        Image image = LoadImage(item.path);
        if (image.data) {
            item.thumbnail = LoadTextureFromImage(image);
            UnloadImage(image);
            item.has_thumbnail = item.thumbnail.id > 0;
            if (item.has_thumbnail) SetTextureFilter(item.thumbnail, TEXTURE_FILTER_BILINEAR);
        }
    }
    if (item.name[0] == '\0') CopyStr(item.name, sizeof(item.name), "Media");
    app->media[app->media_count++] = item;
    Toastf(app, "Imported %s", item.name);
}

/* ========================================================================== */
/* Preview texture cache                                                      */
/* ========================================================================== */

static void ReleasePreviewTextures(App *app) {
    if (app->video_tex_ok) {
        UnloadTexture(app->video_tex);
        app->video_tex_ok = 0;
    }
    if (app->image_tex_ok) {
        UnloadTexture(app->image_tex);
        app->image_tex_ok = 0;
    }
    app->video_tex_path[0] = '\0';
    app->image_tex_path[0] = '\0';
}

/* Topmost visual clip under the playhead, following the V1..V4 stacking order. */
static int TopClipAt(App *app, double time) {
    int best = -1;
    int best_track = -1;
    for (int i = 0; i < app->clip_count; i++) {
        Clip *clip = &app->clips[i];
        int track = TrackIndexForClip(clip);
        if (TrackIsAudio(track)) continue;
        if (time < clip->start || time >= clip->start + clip->dur) continue;
        if (track == best_track) {
            if (best < 0 || clip->start > app->clips[best].start) best = i;
        } else if (track > best_track) {
            best = i;
            best_track = track;
        }
    }
    return best;
}

static void EnsurePreviewTexture(App *app) {
    int index = TopClipAt(app, app->playhead);
    if (index < 0) {
        app->preview_dirty = 0;
        return;
    }
    Clip *clip = &app->clips[index];
    if (strcmp(clip->theme, "title") == 0 || !clip->path[0]) {
        app->preview_dirty = 0;
        return;
    }
    if (strcmp(clip->media_kind, "image") == 0) {
        if (app->image_tex_ok && strcmp(app->image_tex_path, clip->path) == 0) return;
        if (app->image_tex_ok) {
            UnloadTexture(app->image_tex);
            app->image_tex_ok = 0;
        }
        Image image = LoadImage(clip->path);
        if (image.data) {
            app->image_tex = LoadTextureFromImage(image);
            UnloadImage(image);
            app->image_tex_ok = app->image_tex.id > 0;
            if (app->image_tex_ok) {
                SetTextureFilter(app->image_tex, TEXTURE_FILTER_BILINEAR);
                CopyStr(app->image_tex_path, sizeof(app->image_tex_path), clip->path);
            }
        }
        return;
    }
    double target = app->playhead - clip->start + clip->source_offset;
    if (app->video_tex_ok && strcmp(app->video_tex_path, clip->path) == 0 && fabs(target - app->video_tex_time) < 0.06) return;
    if (GetTime() - app->video_request_at < (app->playing ? 0.8 : 0.2)) return;
    if (!app->backend_ok) return;
    app->video_request_at = GetTime();
    unsigned char *data = NULL;
    int len = 0;
    if (!pluto_media_frame(clip->path, target, 640, 360, &data, &len) || !data || len <= 0) {
        if (data) pluto_buffer_free(data, len);
        return;
    }
    Texture2D texture = TextureFromBytes(data, len, ".jpg");
    pluto_buffer_free(data, len);
    if (texture.id > 0) {
        if (app->video_tex_ok) UnloadTexture(app->video_tex);
        app->video_tex = texture;
        app->video_tex_ok = 1;
        CopyStr(app->video_tex_path, sizeof(app->video_tex_path), clip->path);
        app->video_tex_time = target;
    }
}

/* ========================================================================== */
/* Immediate-mode widgets                                                     */
/* ========================================================================== */

static int g_block_ui = 0;
static Vector2 g_mouse;
static int g_mouse_pressed;
static int g_mouse_released;
static int g_click_consumed = 0;
static float g_clip_fade = 1.0f;

static int Interactive(Rect r) {
    return !g_block_ui && RectHit(r, g_mouse);
}

static int Clicked(Rect r) {
    if (!Interactive(r) || !g_mouse_pressed || g_click_consumed) return 0;
    g_click_consumed = 1;
    return 1;
}

static int IconButton(Rect r, IconId icon, int active, const char *tooltip) {
    int hover = Interactive(r);
    if (active) RoundRect(r, 7, RGBA(255, 255, 255, 18));
    else if (hover) RoundRect(r, 7, RGBA(255, 255, 255, 9));
    if (hover) {
        SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (tooltip && tooltip[0]) {
            float width = TxtW(tooltip, 11) + 14;
            Rect tip = MakeRect(r.x + r.w * 0.5f - width * 0.5f, r.y + r.h + 6, width, 22);
            if (tip.x < 6) tip.x = 6;
            if (tip.x + tip.w > g_app_ptr->width - 6) tip.x = g_app_ptr->width - 6 - tip.w;
            RoundRect(tip, 5, RGBA(30, 31, 36, 245));
            TxtC(tooltip, tip.x + tip.w * 0.5f, tip.y + 5, 11, COL_TEXT);
        }
    }
    DrawIcon(icon, r.x + (r.w - 16) * 0.5f, r.y + (r.h - 16) * 0.5f, 16, active ? COL_TEXT : (hover ? COL_TEXT : COL_MUTED));
    return Clicked(r);
}

static const char *FormatValue(char *buffer, size_t cap, float value, int decimals, const char *unit) {
    char number[32];
    snprintf(number, sizeof(number), "%.*f", decimals, value);
    if (decimals == 0) {
        /* Keep the display compact but still show a single decimal for step sizes. */
        snprintf(number, sizeof(number), "%d", (int)lroundf(value));
    }
    snprintf(buffer, cap, "%s%s", number, unit ? unit : "");
    return buffer;
}

static int SliderEx(Rect r, const char *label, float *value, float min, float max, float def, const char *unit,
                    int decimals, float step) {
    if (max <= min) max = min + 1;
    int key = (int)(intptr_t)value;
    int hover = Interactive(r);
    Rect rail = MakeRect(r.x, r.y + 26, r.w, 4);
    Rect hit = MakeRect(r.x, r.y + 14, r.w, 24);
    if (!g_block_ui && RectHit(hit, g_mouse) && g_mouse_pressed) {
        g_slider_key = key;
    }
    int changed = 0;
    if (g_slider_key == key && !g_block_ui && IsMouseButtonDown(MOUSE_BUTTON_LEFT)) {
        float t = Clampf((g_mouse.x - rail.x - 2) / Maxf(1.0f, rail.w - 4), 0.0f, 1.0f);
        float next = min + t * (max - min);
        if (step > 0) next = min + roundf((next - min) / step) * step;
        next = Clampf(next, min, max);
        if (fabsf(next - def) <= (max - min) * 0.012f) next = def;
        if (fabsf(next - *value) > 0.00001f) {
            *value = next;
            changed = 1;
        }
    }
    if (g_slider_key == key && (!IsMouseButtonDown(MOUSE_BUTTON_LEFT) || g_block_ui)) g_slider_key = 0;

    float t = Clampf((*value - min) / (max - min), 0.0f, 1.0f);
    Color label_color = hover ? COL_TEXT : COL_MUTED;
    TxtEllipsis(label, r.x, r.y, 11.5f, r.w * 0.62f, label_color);
    char buffer[48];
    TxtR(FormatValue(buffer, sizeof(buffer), *value, decimals, unit), r.x + r.w, r.y, 11.5f, hover ? COL_TEXT : COL_MUTED);
    RoundRect(MakeRect(rail.x, rail.y, rail.w, rail.h), 2, RGBA(255, 255, 255, 20));
    if (t > 0.001f) RoundRect(MakeRect(rail.x, rail.y, rail.w * t, rail.h), 2, COL_TEXT);
    DrawCircleV((Vector2){rail.x + rail.w * t, rail.y + 2}, hover || g_slider_key == key ? 6.0f : 5.0f, COL_TEXT);
    if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
    return changed;
}

static int SliderCompact(Rect r, const char *label, float *value, float min, float max, float def, const char *unit, int decimals, float step) {
    Rect rail = MakeRect(r.x + 70, r.y + r.h * 0.5f - 2, r.w - 70 - 54, 4);
    Rect hit = MakeRect(r.x + 60, r.y, r.w - 54, r.h);
    int key = (int)(intptr_t)value;
    int hover = Interactive(hit);
    if (!g_block_ui && RectHit(hit, g_mouse) && g_mouse_pressed) {
        g_slider_key = key;
        PushUndo(g_app_ptr);
    }
    int changed = 0;
    if (g_slider_key == key && !g_block_ui && IsMouseButtonDown(MOUSE_BUTTON_LEFT)) {
        float t = Clampf((g_mouse.x - rail.x - 2) / Maxf(1.0f, rail.w - 4), 0.0f, 1.0f);
        float next = min + t * (max - min);
        if (step > 0) next = min + roundf((next - min) / step) * step;
        next = Clampf(next, min, max);
        if (fabsf(next - def) <= (max - min) * 0.012f) next = def;
        if (fabsf(next - *value) > 0.00001f) {
            *value = next;
            changed = 1;
        }
    }
    if (g_slider_key == key && (!IsMouseButtonDown(MOUSE_BUTTON_LEFT) || g_block_ui)) g_slider_key = 0;

    float t = Clampf((*value - min) / (max - min), 0.0f, 1.0f);
    TxtEllipsis(label, r.x, r.y + (r.h - 12) * 0.5f, 11.5f, 64, hover ? COL_TEXT : COL_MUTED);
    RoundRect(rail, 2, RGBA(255, 255, 255, 20));
    if (t > 0.001f) RoundRect(MakeRect(rail.x, rail.y, rail.w * t, rail.h), 2, COL_TEXT);
    DrawCircleV((Vector2){rail.x + rail.w * t, rail.y + 2}, hover || g_slider_key == key ? 6 : 5, COL_TEXT);
    char buffer[48];
    TxtR(FormatValue(buffer, sizeof(buffer), *value, decimals, unit), r.x + r.w - 4, r.y + (r.h - 12) * 0.5f, 11.5f, hover ? COL_TEXT : COL_MUTED);
    if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
    return changed;
}

static int Switch(Rect r, int *value) {
    int hover = Interactive(r);
    int changed = 0;
    if (Clicked(r)) {
        PushUndo(g_app_ptr);
        *value = !*value;
        changed = 1;
    }
    Rect track = MakeRect(r.x, r.y + (r.h - 20) * 0.5f, 36, 20);
    RoundRect(track, 10, *value ? COL_TEXT : RGBA(255, 255, 255, 24));
    float knob = *value ? track.x + track.w - 16.5f : track.x + 2.5f;
    DrawCircleV((Vector2){knob + 6.5f, track.y + 10}, 7.5f, *value ? RGBA(16, 17, 20, 255) : RGBA(190, 192, 200, 255));
    if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
    return changed;
}

static int SwitchRow(Rect r, const char *label, int *value) {
    int changed = Switch(MakeRect(r.x + r.w - 42, r.y, 42, r.h), value);
    TxtEllipsis(label, r.x, r.y + (r.h - 12) * 0.5f, 12, r.w - 52, COL_TEXT);
    return changed;
}

static int Segmented(Rect r, const char **options, int count, int *value) {
    RoundRect(r, 7, RGBA(255, 255, 255, 8));
    int changed = 0;
    float width = r.w / (float)count;
    for (int i = 0; i < count; i++) {
        Rect item = MakeRect(r.x + i * width, r.y, width - 2, r.h);
        int hover = Interactive(item);
        if (*value == i) RoundRect(RectInset(item, 2), 6, RGBA(255, 255, 255, 30));
        else if (hover) RoundRect(RectInset(item, 2), 6, RGBA(255, 255, 255, 10));
        TxtC(options[i], item.x + item.w * 0.5f, item.y + (item.h - 12) * 0.5f, 12, *value == i ? COL_TEXT : COL_MUTED);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(item) && *value != i) {
            *value = i;
            changed = 1;
        }
    }
    return changed;
}

static void TextInputDraw(TextField *field, int numeric) {
    int hover = Interactive(field->rect);
    int focused = g_focus == field;
    RoundRect(field->rect, 7, focused ? RGBA(255, 255, 255, 16) : (hover ? COL_FIELD_HOVER : COL_FIELD));
    if (focused) DrawRectangleRoundedLinesEx((Rectangle){field->rect.x, field->rect.y, field->rect.w, field->rect.h}, 0.35f, 6, 1, RGBA(94, 155, 255, 200));
    float text_x = field->rect.x + 10;
    float text_y = field->rect.y + (field->rect.h - 13) * 0.5f;
    float available = field->rect.w - 20;
    if (!field->buffer[0]) {
        TxtEllipsis(field->placeholder ? field->placeholder : "", text_x, text_y, 12.5f, available, COL_DIM);
    } else {
        float width = TxtW(field->buffer, 12.5f);
        float offset = Maxf(0.0f, width - available);
        BeginScissorMode((int)field->rect.x + 6, (int)field->rect.y + 2, (int)field->rect.w - 12, (int)field->rect.h - 4);
        Txt(field->buffer, text_x - offset, text_y, 12.5f, COL_TEXT);
        EndScissorMode();
        if (focused && ((int)(GetTime() * 2.0)) % 2 == 0) {
            DrawRectangle((int)(text_x + Minf(width, available) + 1), (int)text_y - 1, 1, 15, COL_TEXT);
        }
    }
    if (hover) SetMouseCursor(MOUSE_CURSOR_IBEAM);
    if (g_mouse_pressed && RectHit(field->rect, g_mouse) && !g_block_ui) {
        g_focus = field;
    }
    if (g_focus == field && !g_block_ui) {
        int pressed = GetCharPressed();
        while (pressed > 0) {
            int allowed = numeric ? (isdigit(pressed) || pressed == '.' || pressed == '-') : (pressed >= 32 && pressed < 127);
            if (allowed) {
                size_t length = strlen(field->buffer);
                if (length + 1 < (size_t)field->cap) {
                    field->buffer[length] = (char)pressed;
                    field->buffer[length + 1] = '\0';
                }
            }
            pressed = GetCharPressed();
        }
        if (IsKeyPressed(KEY_BACKSPACE)) {
            size_t length = strlen(field->buffer);
            if (length > 0) field->buffer[length - 1] = '\0';
        }
        if (IsKeyPressed(KEY_ENTER) || IsKeyPressed(KEY_KP_ENTER) || IsKeyPressed(KEY_ESCAPE)) {
            g_focus = NULL;
        }
    }
}

static int TextInput(TextField *field, const char *placeholder, int numeric) {
    field->placeholder = placeholder;
    field->numeric = numeric;
    TextInputDraw(field, numeric);
    return g_focus == field;
}

static void WheelGradient(Vector2 center, float radius) {
    const int segments = 72;
    for (int i = 0; i < segments; i++) {
        float a0 = (float)i * 360.0f / segments;
        float a1 = (float)(i + 1) * 360.0f / segments + 1.0f;
        Color color = ColorFromHSV(a0, 0.65f, 0.95f);
        DrawRing(center, radius - 7, radius, a0, a1, 2, color);
    }
}

static int ColorWheel(Rect r, const char *label, float *hue, float *amount) {
    int key = (int)(intptr_t)hue;
    Vector2 center = {r.x + r.w * 0.5f, r.y + r.w * 0.5f - 4};
    float radius = r.w * 0.42f;
    int hover = Interactive(MakeRect(r.x, r.y, r.w, r.w + 12));
    if (hover && g_mouse_pressed) {
        g_slider_key = key;
        PushUndo(g_app_ptr);
    }
    int changed = 0;
    if (g_slider_key == key && !g_block_ui && IsMouseButtonDown(MOUSE_BUTTON_LEFT)) {
        float dx = g_mouse.x - center.x;
        float dy = g_mouse.y - center.y;
        float distance = sqrtf(dx * dx + dy * dy);
        float next_amount = Clampf(distance / radius, 0.0f, 1.0f) * 100.0f;
        float next_hue = atan2f(dy, dx) * RAD2DEG;
        if (next_hue < 0) next_hue += 360.0f;
        if (fabsf(next_hue - *hue) > 0.1f || fabsf(next_amount - *amount) > 0.1f) {
            *hue = next_hue;
            *amount = next_amount;
            changed = 1;
        }
    }
    if (g_slider_key == key && (!IsMouseButtonDown(MOUSE_BUTTON_LEFT) || g_block_ui)) g_slider_key = 0;

    WheelGradient(center, radius);
    DrawCircleV(center, radius - 8, RGBA(22, 23, 27, 255));
    if (*amount > 0.5f) {
        float angle = *hue * DEG2RAD;
        Vector2 tip = {center.x + cosf(angle) * radius * (*amount / 100.0f), center.y + sinf(angle) * radius * (*amount / 100.0f)};
        DrawLineEx(center, tip, 2, RGBA(255, 255, 255, 210));
        DrawCircleV(tip, 5, ColorFromHSV(*hue, 0.75f, 1.0f));
    }
    TxtC(label, center.x, center.y + radius + 4, 11, COL_MUTED);
    if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
    (void)changed;
    return 0;
}

static void SectionLabel(const char *text, float x, float y) {
    DrawTrackedLabel(text, x, y, 10, 1.2f, COL_DIM);
}

static int CardHeader(Rect r, IconId icon, const char *title, int open) {
    int hover = Interactive(r);
    if (hover) RoundRect(r, 7, RGBA(255, 255, 255, 6));
    DrawIcon(icon, r.x + 10, r.y + (r.h - 16) * 0.5f, 16, COL_MUTED);
    Txt(title, r.x + 34, r.y + (r.h - 13) * 0.5f, 13, COL_TEXT);
    DrawIcon(open ? ICON_CHEV_DOWN : ICON_CHEV_RIGHT, r.x + r.w - 26, r.y + (r.h - 14) * 0.5f, 14, COL_DIM);
    if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
    return Clicked(r);
}

static void FmtChip(char *buffer, size_t cap, double seconds) {
    int total = (int)lround(seconds);
    snprintf(buffer, cap, "%d:%02d", total / 60, total % 60);
}

/* ========================================================================== */
/* Layout                                                                     */
/* ========================================================================== */

typedef struct {
    Rect top;
    Rect library;
    Rect preview;
    Rect inspector;
    Rect timeline;
    Rect stage;
    Rect transport;
} Layout;

static Layout ComputeLayout(App *app) {
    Layout layout;
    float margin = 10.0f;
    float left = app->width < 1320 ? 250.0f : 270.0f;
    float right = app->width < 1320 ? 350.0f : 400.0f;
    float timeline_h = Clampf(app->height * 0.34f, 250.0f, 340.0f);
    float content_y = margin + 56 + margin;
    float timeline_y = app->height - margin - timeline_h;
    float center_h = timeline_y - margin - content_y;
    layout.top = MakeRect(margin, margin, app->width - margin * 2, 56);
    layout.library = MakeRect(margin, content_y, left, center_h);
    layout.inspector = MakeRect(app->width - margin - right, content_y, right, center_h);
    float preview_x = margin + left + margin;
    layout.preview = MakeRect(preview_x, content_y, layout.inspector.x - margin - preview_x, center_h);
    layout.timeline = MakeRect(margin, timeline_y, app->width - margin * 2, timeline_h);
    layout.transport = MakeRect(layout.preview.x + 14, layout.preview.y + layout.preview.h - 52, layout.preview.w - 28, 44);
    float stage_w = layout.preview.w - 32;
    float stage_h = layout.preview.h - 32 - 52;
    float aspect = 16.0f / 9.0f;
    if (stage_w / aspect > stage_h) stage_w = stage_h * aspect;
    else stage_h = stage_w / aspect;
    layout.stage = MakeRect(layout.preview.x + (layout.preview.w - stage_w) * 0.5f,
                            layout.preview.y + 16 + (layout.preview.h - 32 - 52 - stage_h) * 0.5f,
                            stage_w, stage_h);
    return layout;
}

static void DrawCard(Rect r) {
    RoundRectBorder(r, 12, COL_CARD, 1, COL_BORDER);
}

/* ========================================================================== */
/* Top bar                                                                    */
/* ========================================================================== */

static void TopBar(App *app, Layout layout) {
    Rect top = layout.top;
    RoundRect(top, 12, COL_CARD_TOP);

    float x = top.x + 16;
    CircleGradient((Vector2){x + 14, top.y + 28}, 14, RGBA(140, 170, 255, 255), RGBA(90, 110, 230, 255));
    TxtC("P", x + 14, top.y + 20, 15, RGBA(12, 14, 28, 255));
    Txt("Pluto", x + 36, top.y + 19, 17, COL_TEXT);
    x += 104;
    DrawRectangle((int)x, (int)(top.y + 18), 1, 20, COL_BORDER);
    x += 16;

    app->name_field.rect = MakeRect(x, top.y + 12, 220, 32);
    TextInput(&app->name_field, "Untitled project", 0);

    float right = top.x + top.w - 14;

    Rect export_rect = MakeRect(right - 104, top.y + 12, 104, 32);
    {
        int hover = Interactive(export_rect);
        RoundRect(export_rect, 8, hover ? RGBA(255, 255, 255, 255) : COL_LIGHT_BUTTON);
        DrawIcon(ICON_DOWNLOAD, export_rect.x + 16, export_rect.y + 9, 14, COL_LIGHT_TEXT);
        Txt("Export", export_rect.x + 38, export_rect.y + 9, 13, COL_LIGHT_TEXT);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(export_rect)) {
            app->drawer_open = !app->drawer_open;
            if (app->drawer_open) pluto_export_reset();
        }
    }
    right -= 116;

    Rect redo_rect = MakeRect(right - 34, top.y + 12, 34, 32);
    Rect undo_rect = MakeRect(right - 72, top.y + 12, 34, 32);
    if (IconButton(undo_rect, ICON_RESET, 0, "Undo (Ctrl+Z)")) Undo(app);
    if (IconButton(redo_rect, ICON_REPEAT, 0, "Redo (Ctrl+Shift+Z)")) Redo(app);
    (void)right;
}

/* ========================================================================== */
/* Library                                                                    */
/* ========================================================================== */

static void DrawMediaThumb(Rect r, MediaItem *media) {
    RoundRect(r, 8, RGBA(9, 10, 13, 255));
    if (media->has_thumbnail && media->thumbnail.id > 0) {
        Rectangle source = {0, 0, (float)media->thumbnail.width, (float)media->thumbnail.height};
        Rectangle dest = {r.x, r.y, r.w, r.h};
        BeginScissorMode((int)r.x, (int)r.y, (int)r.w, (int)r.h);
        DrawTexturePro(media->thumbnail, source, dest, (Vector2){0, 0}, 0, WHITE);
        EndScissorMode();
    } else if (strcmp(media->kind, "audio") == 0) {
        CircleGradient((Vector2){r.x + r.w * 0.5f, r.y + r.h * 0.5f}, r.w * 0.5f, RGBA(30, 60, 70, 255), RGBA(12, 20, 26, 255));
        if (media->has_waveform && media->waveform.id > 0) {
            Rectangle source = {0, 0, (float)media->waveform.width, (float)media->waveform.height};
            Rectangle dest = {r.x + 8, r.y + r.h * 0.3f, r.w - 16, r.h * 0.4f};
            DrawTexturePro(media->waveform, source, dest, (Vector2){0, 0}, 0, RGBA(120, 200, 230, 190));
        } else {
            DrawIcon(ICON_WAVE, r.x + r.w * 0.5f - 18, r.y + r.h * 0.5f - 18, 36, RGBA(104, 196, 235, 200));
        }
    } else {
        CircleGradient((Vector2){r.x + r.w * 0.5f, r.y + r.h * 0.5f}, r.w * 0.6f, RGBA(40, 52, 74, 255), RGBA(14, 18, 26, 255));
        DrawIcon(ICON_FILM, r.x + r.w * 0.5f - 16, r.y + r.h * 0.5f - 16, 32, RGBA(150, 165, 190, 180));
    }
    RoundRectBorder(r, 8, BLANK, 1, COL_BORDER);
}

static void LibraryPanel(App *app, Layout layout) {
    Rect panel = layout.library;
    DrawCard(panel);
    Rect head = MakeRect(panel.x + 12, panel.y + 10, panel.w - 24, 34);
    DrawIcon(ICON_FILM, head.x, head.y + 6, 18, COL_MUTED);
    Txt("Library", head.x + 26, head.y + 8, 15, COL_TEXT);
    Rect import_rect = MakeRect(head.x + head.w - 84, head.y, 84, 30);
    {
        int hover = Interactive(import_rect);
        RoundRect(import_rect, 15, hover ? COL_FIELD_HOVER : COL_FIELD);
        DrawIcon(ICON_PLUS, import_rect.x + 12, import_rect.y + 9, 12, COL_TEXT);
        Txt("Import", import_rect.x + 30, import_rect.y + 8, 12, COL_TEXT);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(import_rect)) ImportMedia(app);
    }

    float y = panel.y + 52;
    if (app->importing) {
        Txt("Inspecting media with FFmpeg…", panel.x + 14, y, 11.5f, COL_MUTED);
        y += 20;
    }
    const char *tabs[2] = {"Media", "Audio"};
    Rect seg = MakeRect(panel.x + 12, y, panel.w - 24, 30);
    Segmented(seg, tabs, 2, &app->library_tab);
    y += 38;

    Rect content = MakeRect(panel.x + 12, y, panel.w - 24, panel.y + panel.h - y - 12);
    BeginScissorMode((int)content.x, (int)content.y, (int)content.w, (int)content.h);
    if (app->library_tab == 0) {
        app->search_field.rect = MakeRect(content.x, content.y + (int)app->media_scroll, content.w, 32);
        TextInput(&app->search_field, "Search media", 0);
        CopyStr(app->search, sizeof(app->search), app->search_field.buffer);

        float grid_y = content.y + 42 + app->media_scroll;
        float gap = 10;
        float tile_w = (content.w - gap) * 0.5f;
        float tile_h = tile_w * 9.0f / 16.0f + 26;
        int column = 0;
        int shown = 0;
        for (int i = 0; i < app->media_count; i++) {
            MediaItem *media = &app->media[i];
            if (app->search[0] && !ContainsCI(media->name, app->search)) continue;
            int visible_i = shown;
            float tx = content.x + column * (tile_w + gap);
            float ty = grid_y + (visible_i / 2) * (tile_h + gap);
            Rect tile = MakeRect(tx, ty, tile_w, tile_h);
            if (ty > content.y + content.h || ty + tile_h < content.y) {
                shown++;
                column = (column + 1) % 2;
                continue;
            }
            Rect thumb = MakeRect(tile.x, tile.y, tile.w, tile.w * 9.0f / 16.0f);
            DrawMediaThumb(thumb, media);
            char duration[32];
            FmtChip(duration, sizeof(duration), media->duration);
            Rect chip = MakeRect(thumb.x + 6, thumb.y + 6, TxtW(duration, 10) + 12, 18);
            RoundRect(chip, 5, RGBA(0, 0, 0, 150));
            Txt(duration, chip.x + 6, chip.y + 3, 10, COL_TEXT);

            Rect add = MakeRect(thumb.x + thumb.w - 26, thumb.y + thumb.h - 26, 22, 22);
            int add_hover = Interactive(add);
            RoundRect(add, 6, add_hover ? RGBA(255, 255, 255, 240) : RGBA(20, 21, 24, 210));
            DrawIcon(ICON_PLUS, add.x + 4, add.y + 4, 14, add_hover ? COL_LIGHT_TEXT : COL_TEXT);
            if (Clicked(add)) AddMediaClip(app, i);

            int tile_hover = Interactive(thumb);
            if (tile_hover) {
                DrawRectangleRoundedLinesEx((Rectangle){thumb.x, thumb.y, thumb.w, thumb.h}, 0.09f, 8, 1, RGBA(255, 255, 255, 80));
                if (IsMouseButtonPressed(MOUSE_BUTTON_LEFT) && !RectHit(add, g_mouse)) {
                    if (GetTime() - app->last_click_time < 0.35 && app->last_click_index == i) {
                        AddMediaClip(app, i);
                        app->last_click_time = 0;
                    } else {
                        app->last_click_time = GetTime();
                        app->last_click_index = i;
                    }
                }
            }
            TxtEllipsis(media->name, tile.x + 2, tile.y + thumb.h + 5, 11.5f, tile.w - 4, COL_TEXT);
            shown++;
            column = (column + 1) % 2;
        }
        if (shown == 0) {
            TxtC(app->media_count ? "No matching media" : "No media imported yet", content.x + content.w * 0.5f, content.y + 60, 12, COL_MUTED);
            TxtC(app->media_count ? "Try a different search" : "Use Import to add video, audio or images", content.x + content.w * 0.5f, content.y + 80, 11, COL_DIM);
        }
        app->media_scroll = Clampf(app->media_scroll, -Maxf(0.0f, ((shown + 1) / 2) * (tile_h + gap) - content.h + 52), 0.0f);
    } else {
        Clip *clip = SelectedClip(app);
        if (!clip) {
            TxtC("Select an audio clip to adjust it", content.x + content.w * 0.5f, content.y + 24, 12, COL_MUTED);
        } else {
            float row_y = content.y + 4 + app->audio_scroll;
            SectionLabel("CLIP AUDIO", content.x, row_y);
            row_y += 20;
            if (SliderCompact(MakeRect(content.x, row_y, content.w, 24), "Volume", &clip->volume, 0, 200, 100, "%", 0, 1)) MarkPropsDirty(app, clip);
            row_y += 26;
            if (SliderCompact(MakeRect(content.x, row_y, content.w, 24), "Pan", &clip->pan, -100, 100, 0, "", 0, 1)) MarkPropsDirty(app, clip);
            row_y += 26;
            if (SliderCompact(MakeRect(content.x, row_y, content.w, 24), "Bass", &clip->bass, -20, 20, 0, "dB", 0, 0.1f)) MarkPropsDirty(app, clip);
            row_y += 26;
            if (SliderCompact(MakeRect(content.x, row_y, content.w, 24), "Treble", &clip->treble, -20, 20, 0, "dB", 0, 0.1f)) MarkPropsDirty(app, clip);
            row_y += 32;
            DrawRectangle((int)content.x, (int)row_y, (int)content.w, 1, COL_BORDER);
            row_y += 10;
            if (SwitchRow(MakeRect(content.x, row_y, content.w, 24), "Fade in", &clip->fade_in)) MarkPropsDirty(app, clip);
            row_y += 26;
            if (clip->fade_in) {
                if (SliderCompact(MakeRect(content.x + 10, row_y, content.w - 10, 22), "Duration", &clip->fade_in_duration, 0, 5, 1, "s", 1, 0.1f)) MarkPropsDirty(app, clip);
                row_y += 24;
            }
            if (SwitchRow(MakeRect(content.x, row_y, content.w, 24), "Fade out", &clip->fade_out)) MarkPropsDirty(app, clip);
            row_y += 26;
            if (clip->fade_out) {
                if (SliderCompact(MakeRect(content.x + 10, row_y, content.w - 10, 22), "Duration", &clip->fade_out_duration, 0, 5, 1, "s", 1, 0.1f)) MarkPropsDirty(app, clip);
                row_y += 24;
            }
            row_y += 10;
            if (clip->has_audio) {
                Txt("This clip has embedded audio.", content.x, row_y, 11, COL_DIM);
            } else {
                Txt("No audio stream on this clip.", content.x, row_y, 11, COL_DIM);
            }
        }
    }
    EndScissorMode();

    if (Interactive(content) && fabsf(GetMouseWheelMove()) > 0.0f) {
        if (app->library_tab == 0) app->media_scroll += GetMouseWheelMove() * 40.0f;
        else app->audio_scroll += GetMouseWheelMove() * 40.0f;
    }
}

/* ========================================================================== */
/* Preview                                                                    */
/* ========================================================================== */

static void FormatTC(char *buffer, size_t cap, double time) {
    int frames = (int)floor(time * 30.0 + 1e-6);
    int seconds = frames / 30;
    snprintf(buffer, cap, "%02d:%02d:%02d:%02d",
             (seconds / 3600) % 100, (seconds / 60) % 60, seconds % 60, frames % 30);
}

static void GradeOverlay(App *app, Clip *clip, Rect stage) {
    float exposure = clip->light[0];
    float brightness = clip->light[1] / 100.0f;
    float contrast = clip->light[2] / 100.0f + clip->detail[1] / 200.0f;
    float saturation = clip->color[2] / 100.0f;
    (void)saturation;
    float temp = clip->color[0] / 100.0f;
    if (fabsf(exposure) > 0.01f || fabsf(brightness) > 0.01f) {
        float amount = Clampf(exposure * 0.09f + brightness * 0.25f, -0.65f, 0.65f);
        Color overlay = amount >= 0 ? RGBA(255, 250, 240, (int)(amount * 190)) : RGBA(0, 0, 0, (int)(-amount * 190));
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, overlay);
    }
    if (contrast > 0.02f) {
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, RGBA(0, 0, 0, (int)Clampf(contrast * 40, 0, 60)));
    }
    if (fabsf(temp) > 0.02f) {
        Color warm = temp > 0 ? RGBA(255, 150, 70, (int)Clampf(fabsf(temp) * 70, 0, 80))
                              : RGBA(70, 150, 255, (int)Clampf(fabsf(temp) * 70, 0, 80));
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, warm);
    }
    if (clip->grade[0].amount > 1.0f || clip->grade[1].amount > 1.0f || clip->grade[2].amount > 1.0f) {
        static const int order[3] = {0, 2, 1};
        static const Color colors[3] = {{120, 80, 200, 0}, {255, 170, 90, 0}, {160, 220, 255, 0}};
        for (int i = 0; i < 3; i++) {
            int slot = order[i];
            if (clip->grade[slot].amount <= 1.0f) continue;
            Color c = colors[slot];
            c.a = (unsigned char)Clampf(clip->grade[slot].amount * 1.1f, 0, 80);
            DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, c);
        }
    }
    if (clip->detail[2] < -1.0f) {
        float alpha = Clampf(-clip->detail[2] / 100.0f, 0.0f, 1.0f);
        float band = stage.h * 0.28f;
        DrawRectangleGradientV((int)stage.x, (int)stage.y, (int)stage.w, (int)band, RGBA(0, 0, 0, (int)(alpha * 170)), RGBA(0, 0, 0, 0));
        DrawRectangleGradientV((int)stage.x, (int)(stage.y + stage.h - band), (int)stage.w, (int)band, RGBA(0, 0, 0, 0), RGBA(0, 0, 0, (int)(alpha * 170)));
        DrawRectangleGradientH((int)stage.x, (int)stage.y, (int)band, (int)stage.h, RGBA(0, 0, 0, (int)(alpha * 120)), RGBA(0, 0, 0, 0));
        DrawRectangleGradientH((int)(stage.x + stage.w - band), (int)stage.y, (int)band, (int)stage.h, RGBA(0, 0, 0, 0), RGBA(0, 0, 0, (int)(alpha * 120)));
    }
}

static void EffectOverlay(App *app, Clip *clip, Rect stage) {
    double seed = app->playhead;
    if (clip->fx_enabled[FX_GRAIN]) {
        float amount = clip->fx[FX_GRAIN][0] / 100.0f;
        for (int i = 0; i < 220; i++) {
            float rx = (float)((i * 73 + (int)(seed * 37)) % 997) / 997.0f;
            float ry = (float)((i * 149 + (int)(seed * 61)) % 991) / 991.0f;
            float size = 0.6f + (i % 3) * 0.5f;
            DrawRectangle((int)(stage.x + rx * stage.w), (int)(stage.y + ry * stage.h), (int)size, (int)size,
                          RGBA(255, 255, 255, (int)(amount * 60)));
        }
    }
    if (clip->fx_enabled[FX_SCANLINES]) {
        float spacing = Maxf(2.0f, clip->fx[FX_SCANLINES][1]);
        int alpha = (int)(clip->fx[FX_SCANLINES][0] * 1.3f);
        for (float y = stage.y; y < stage.y + stage.h; y += spacing) {
            DrawRectangle((int)stage.x, (int)y, (int)stage.w, 1, RGBA(0, 0, 0, Clampi(alpha, 0, 140)));
        }
    }
    if (clip->fx_enabled[FX_VIGNETTE]) {
        float amount = fabsf(clip->fx[FX_VIGNETTE][0]) / 100.0f;
        float band = stage.h * (0.18f + clip->fx[FX_VIGNETTE][3] / 400.0f);
        DrawRectangleGradientV((int)stage.x, (int)stage.y, (int)stage.w, (int)band, RGBA(0, 0, 0, (int)(amount * 170)), RGBA(0, 0, 0, 0));
        DrawRectangleGradientV((int)stage.x, (int)(stage.y + stage.h - band), (int)stage.w, (int)band, RGBA(0, 0, 0, 0), RGBA(0, 0, 0, (int)(amount * 170)));
        DrawRectangleGradientH((int)stage.x, (int)stage.y, (int)band, (int)stage.h, RGBA(0, 0, 0, (int)(amount * 130)), RGBA(0, 0, 0, 0));
        DrawRectangleGradientH((int)(stage.x + stage.w - band), (int)stage.y, (int)band, (int)stage.h, RGBA(0, 0, 0, 0), RGBA(0, 0, 0, (int)(amount * 130)));
    }
    if (clip->fx_enabled[FX_LIGHTLEAK]) {
        float amount = clip->fx[FX_LIGHTLEAK][0] / 100.0f;
        float position = clip->fx[FX_LIGHTLEAK][1] / 100.0f;
        float x = stage.x + position * stage.w;
        CircleGradient((Vector2){x, stage.y + stage.h * 0.2f}, stage.w * 0.6f, RGBA(255, 150, 80, (int)(amount * 75)), RGBA(255, 100, 40, 0));
    }
    if (clip->fx_enabled[FX_FILMBURN]) {
        float amount = clip->fx[FX_FILMBURN][0] / 100.0f;
        DrawRectangleGradientV((int)stage.x, (int)stage.y, (int)stage.w, (int)(stage.h * 0.4f), RGBA(255, 130, 30, (int)(amount * 120)), RGBA(255, 90, 20, 0));
        DrawRectangleGradientH((int)(stage.x + stage.w - stage.w * 0.35f), (int)stage.y, (int)(stage.w * 0.35f), (int)stage.h, RGBA(255, 90, 20, 0), RGBA(255, 140, 40, (int)(amount * 90)));
    }
    if (clip->fx_enabled[FX_GLOW]) {
        float amount = clip->fx[FX_GLOW][0] / 100.0f;
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, RGBA(255, 245, 210, (int)(amount * 22)));
    }
    if (clip->fx_enabled[FX_CHROMATIC]) {
        float amount = clip->fx[FX_CHROMATIC][0] * 0.06f;
        DrawRectangleLinesEx((Rectangle){stage.x - amount, stage.y, stage.w + amount * 2, stage.h}, 2, RGBA(255, 60, 60, 30));
        DrawRectangleLinesEx((Rectangle){stage.x + amount, stage.y, stage.w - amount * 2, stage.h}, 2, RGBA(60, 120, 255, 30));
    }
    if (clip->fx_enabled[FX_HALFTONE] || clip->fx_enabled[FX_PIXELATE] || clip->fx_enabled[FX_POSTERIZE]) {
        /* These are approximated in the live preview; the export applies the exact filter chain. */
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, RGBA(0, 0, 0, 12));
    }
    if (clip->fx_enabled[FX_MIRROR]) {
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, RGBA(255, 255, 255, 6));
    }
}

static void DrawTitleClip(App *app, Clip *clip, Rect stage) {
    (void)app;
    DrawRectangleGradientV((int)stage.x, (int)stage.y, (int)stage.w, (int)stage.h, RGBA(27, 16, 56, 255), RGBA(122, 47, 107, 255));
    CircleGradient((Vector2){stage.x + stage.w * 0.68f, stage.y + stage.h * 0.32f}, stage.w * 0.14f, RGBA(255, 226, 168, 255), RGBA(255, 208, 138, 0));
    DrawTriangle((Vector2){stage.x, stage.y + stage.h},
                 (Vector2){stage.x + stage.w * 0.35f, stage.y + stage.h * 0.62f},
                 (Vector2){stage.x + stage.w * 0.62f, stage.y + stage.h}, RGBA(42, 23, 64, 255));
    DrawTriangle((Vector2){stage.x + stage.w * 0.32f, stage.y + stage.h},
                 (Vector2){stage.x + stage.w * 0.68f, stage.y + stage.h * 0.7f},
                 (Vector2){stage.x + stage.w, stage.y + stage.h}, RGBA(26, 14, 44, 255));

    float base = stage.h * 0.085f * (clip->text_size / 100.0f) * (clip->scale_h / 100.0f);
    float cx = stage.x + stage.w * 0.5f + clip->pos_x * stage.w / 1920.0f;
    float cy = stage.y + stage.h * 0.5f + clip->pos_y * stage.h / 1080.0f;
    int alpha = (int)Clampf(clip->opacity * 2.55f, 0, 255);
    float title_y = cy - base * 0.7f;
    const char *title = clip->title[0] ? clip->title : clip->label;
    float title_w = TxtW(title, base);
    float x = clip->text_align == 0 ? stage.x + 60 : clip->text_align == 2 ? stage.x + stage.w - 60 - title_w : cx - title_w * 0.5f;
    Txt(title, x, title_y, base, RGBA(255, 255, 255, alpha));
    if (clip->subtitle[0]) {
        float sub = base * 0.36f;
        float sub_w = TxtW(clip->subtitle, sub);
        float sx = clip->text_align == 0 ? stage.x + 60 : clip->text_align == 2 ? stage.x + stage.w - 60 - sub_w : cx - sub_w * 0.5f;
        Txt(clip->subtitle, sx, title_y + base * 1.1f, sub, RGBA(255, 235, 240, alpha * 0.85f));
    }
}

static void DrawMediaClip(App *app, Clip *clip, Rect stage) {
    Texture2D *texture = NULL;
    if (strcmp(clip->media_kind, "image") == 0) {
        if (app->image_tex_ok) texture = &app->image_tex;
    } else if (app->video_tex_ok && strcmp(app->video_tex_path, clip->path) == 0) {
        texture = &app->video_tex;
    }
    if (!texture || texture->id == 0) {
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, RGBA(18, 19, 23, 255));
        DrawIcon(ICON_FILM, stage.x + stage.w * 0.5f - 24, stage.y + stage.h * 0.5f - 24, 48, RGBA(70, 74, 84, 255));
        TxtC(app->playing ? "Decoding preview…" : "Adjust the playhead to refresh the preview", stage.x + stage.w * 0.5f, stage.y + stage.h * 0.5f + 34, 11.5f, COL_DIM);
        return;
    }
    float crop_l = Clampf(clip->crop_l / 1920.0f, 0, 0.9f);
    float crop_r = Clampf(clip->crop_r / 1920.0f, 0, 0.9f);
    float crop_t = Clampf(clip->crop_t / 1080.0f, 0, 0.9f);
    float crop_b = Clampf(clip->crop_b / 1080.0f, 0, 0.9f);
    Rectangle source = {
        texture->width * crop_l,
        texture->height * crop_t,
        texture->width * Maxf(0.05f, 1.0f - crop_l - crop_r),
        texture->height * Maxf(0.05f, 1.0f - crop_t - crop_b)
    };
    float shake_x = 0, shake_y = 0;
    if (clip->fx_enabled[FX_SHAKE]) {
        float amount = clip->fx[FX_SHAKE][0] * 0.18f;
        float freq = clip->fx[FX_SHAKE][1] * (1.0f - clip->fx[FX_SHAKE][2] / 120.0f);
        shake_x = sinf((float)(app->playhead * freq * 2.0f * PI)) * amount;
        shake_y = cosf((float)(app->playhead * freq * 2.0f * PI)) * amount;
    }
    float w = stage.w * (clip->scale_w / 100.0f);
    float h = stage.h * (clip->scale_h / 100.0f);
    Vector2 center = {
        stage.x + stage.w * 0.5f + clip->pos_x * stage.w / 1920.0f + shake_x,
        stage.y + stage.h * 0.5f + clip->pos_y * stage.h / 1080.0f + shake_y
    };
    Rectangle dest = {center.x, center.y, w, h};
    int alpha = (int)Clampf(clip->opacity * 2.55f * Maxf(0.0f, g_clip_fade), 0, 255);
    Color tint = {255, 255, 255, (unsigned char)alpha};

    BeginScissorMode((int)stage.x, (int)stage.y, (int)stage.w, (int)stage.h);
    if (clip->fx_enabled[FX_BLUR] && clip->fx[FX_BLUR][1] > 1.0f) {
        float amount = clip->fx[FX_BLUR][1] * 0.06f;
        Color ghost = {255, 255, 255, (unsigned char)(alpha * 0.22f)};
        for (int i = 0; i < 4; i++) {
            float offset = (i - 1.5f) * amount;
            Rectangle copy = {dest.x + offset, dest.y + (clip->fx[FX_BLUR][0] == 1 ? offset * 0.5f : 0), dest.width, dest.height};
            DrawTexturePro(*texture, source, copy, (Vector2){copy.width * 0.5f, copy.height * 0.5f}, clip->rotation, ghost);
        }
    }
    DrawTexturePro(*texture, source, dest, (Vector2){w * 0.5f, h * 0.5f}, clip->rotation, tint);
    if (clip->fx_enabled[FX_MIRROR] && clip->fx[FX_MIRROR][1] > 1.0f) {
        float blend = Clampf(clip->fx[FX_MIRROR][1] / 100.0f, 0.0f, 1.0f);
        Color mirror_tint = {255, 255, 255, (unsigned char)(alpha * blend * 0.6f)};
        Rectangle mirrored = source;
        if (clip->fx[FX_MIRROR][0] < 0.5f) {
            mirrored.x += mirrored.width;
            mirrored.width = -mirrored.width;
        } else {
            mirrored.y += mirrored.height;
            mirrored.height = -mirrored.height;
        }
        DrawTexturePro(*texture, mirrored, dest, (Vector2){w * 0.5f, h * 0.5f}, clip->rotation, mirror_tint);
    }
    EndScissorMode();

    GradeOverlay(app, clip, stage);
    EffectOverlay(app, clip, stage);
}

static int NextClipIndex(App *app, int index);
static void SetClipTransition(App *app, Clip *clip, const char *type, float dur);

/* Cross-fade / dip preview: blends the drawn clip with its neighbour transition. */
static void ComputeClipFade(App *app, int index, double time) {
    g_clip_fade = 1.0f;
    Clip *clip = &app->clips[index];
    int previous_index = -1;
    for (int i = 0; i < app->clip_count; i++) {
        if (i == index) continue;
        Clip *other = &app->clips[i];
        if (strcmp(other->track, clip->track) != 0) continue;
        if (other->start + other->dur > clip->start - 0.05 && other->start + other->dur < clip->start + 0.05) {
            previous_index = i;
            break;
        }
    }
    if (previous_index >= 0) {
        Clip *previous = &app->clips[previous_index];
        double duration = previous->trans_dur > 0 ? (double)previous->trans_dur : 1.0;
        double half = duration * 0.5;
        if (previous->trans_type[0] && time < clip->start + half) {
            double amount = (time - (clip->start - half)) / duration;
            if (amount < 0.0) amount = 0.0;
            if (amount > 1.0) amount = 1.0;
            g_clip_fade = (float)(1.0 - amount);
        }
    }
    int next_index = NextClipIndex(app, index);
    if (next_index >= 0) {
        double duration = clip->trans_dur > 0 ? (double)clip->trans_dur : 1.0;
        double half = duration * 0.5;
        double end = clip->start + clip->dur;
        if (clip->trans_type[0] && time > end - half) {
            double amount = (time - (end - half)) / duration;
            if (amount < 0.0) amount = 0.0;
            if (amount > 1.0) amount = 1.0;
            float fade_out = (float)(1.0 - amount);
            if (fade_out < g_clip_fade) g_clip_fade = fade_out;
        }
    }
}

static void PreviewPanel(App *app, Layout layout) {
    Rect panel = layout.preview;
    DrawCard(panel);
    Rect stage = layout.stage;
    app->preview_frame = stage;

    RoundRect(MakeRect(stage.x - 2, stage.y - 2, stage.w + 4, stage.h + 4), 10, RGBA(0, 0, 0, 160));
    BeginScissorMode((int)stage.x, (int)stage.y, (int)stage.w, (int)stage.h);
    int top = TopClipAt(app, app->playhead);
    if (top >= 0) {
        Clip *clip = &app->clips[top];
        ComputeClipFade(app, top, app->playhead);
        if (strcmp(clip->theme, "title") == 0) DrawTitleClip(app, clip, stage);
        else DrawMediaClip(app, clip, stage);
    } else {
        DrawRectangleRec((Rectangle){stage.x, stage.y, stage.w, stage.h}, RGBA(14, 15, 18, 255));
        DrawIcon(ICON_FILM, stage.x + stage.w * 0.5f - 28, stage.y + stage.h * 0.5f - 44, 56, RGBA(58, 61, 70, 255));
        TxtC("No media at the playhead", stage.x + stage.w * 0.5f, stage.y + stage.h * 0.5f + 22, 14, COL_MUTED);
        TxtC("Import media from the library, then drag it onto the timeline.", stage.x + stage.w * 0.5f, stage.y + stage.h * 0.5f + 44, 11, COL_DIM);
    }
    if (app->guides) {
        for (int i = 1; i < 3; i++) {
            DrawRectangle((int)(stage.x + stage.w * i / 3.0f), (int)stage.y, 1, (int)stage.h, RGBA(255, 255, 255, 34));
            DrawRectangle((int)stage.x, (int)(stage.y + stage.h * i / 3.0f), (int)stage.w, 1, RGBA(255, 255, 255, 34));
        }
        DrawRectangleLinesEx((Rectangle){stage.x + stage.w * 0.5f - 12, stage.y + stage.h * 0.5f - 12, 24, 24}, 1, RGBA(255, 255, 255, 50));
    }
    EndScissorMode();

    Clip *selected = SelectedClip(app);
    if (selected && !TrackIsAudio(TrackIndexForClip(selected)) && top >= 0 && strcmp(selected->theme, "title") == 0) {
        DrawRectangleLinesEx((Rectangle){stage.x, stage.y, stage.w, stage.h}, 1, RGBA(160, 140, 255, 150));
    }

    /* Transport */
    Rect transport = layout.transport;
    char now[32], total[32];
    FormatTC(now, sizeof(now), app->playhead);
    FormatTC(total, sizeof(total), TimelineDuration(app));
    Txt(now, transport.x + 2, transport.y + 14, 13, COL_TEXT);
    Txt("/", transport.x + 86, transport.y + 14, 13, COL_DIM);
    Txt(total, transport.x + 100, transport.y + 14, 13, COL_MUTED);

    float cx = transport.x + transport.w * 0.5f;
    Rect start_rect = MakeRect(cx - 104, transport.y + 4, 34, 34);
    Rect back_rect = MakeRect(cx - 64, transport.y + 4, 34, 34);
    Rect play_rect = MakeRect(cx - 22, transport.y + 1, 44, 44);
    Rect fwd_rect = MakeRect(cx + 30, transport.y + 4, 34, 34);
    Rect end_rect = MakeRect(cx + 70, transport.y + 4, 34, 34);
    if (IconButton(start_rect, ICON_SKIP_BACK, 0, "Go to start")) app->playhead = 0;
    if (IconButton(back_rect, ICON_CHEV_LEFT, 0, "Previous frame")) app->playhead = Maxd(0, app->playhead - 1.0 / 30.0);
    {
        int hover = Interactive(play_rect);
        DrawCircleV((Vector2){play_rect.x + 22, play_rect.y + 22}, 22, hover ? COL_TEXT : COL_LIGHT_BUTTON);
        DrawIcon(app->playing ? ICON_PAUSE : ICON_PLAY, play_rect.x + 10, play_rect.y + 10, 24, COL_LIGHT_TEXT);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(play_rect)) app->playing = !app->playing;
    }
    if (IconButton(fwd_rect, ICON_CHEV_RIGHT, 0, "Next frame")) app->playhead = Minf((float)app->playhead + 1.0f / 30.0f, (float)TimelineDuration(app));
    if (IconButton(end_rect, ICON_SKIP_FORWARD, 0, "Go to end")) app->playhead = TimelineDuration(app);

    float right = transport.x + transport.w;
    Rect guides_rect = MakeRect(right - 34, transport.y + 4, 34, 34);
    Rect loop_rect = MakeRect(right - 74, transport.y + 4, 34, 34);
    if (IconButton(loop_rect, ICON_REPEAT, app->loop, "Loop playback")) app->loop = !app->loop;
    if (IconButton(guides_rect, ICON_GRID, app->guides, "Guides")) app->guides = !app->guides;
    char chip[64];
    snprintf(chip, sizeof(chip), "1920×1080 · 30 fps");
    Rect chip_rect = MakeRect(right - 94 - TxtW(chip, 11) - 20, transport.y + 10, TxtW(chip, 11) + 20, 22);
    RoundRect(chip_rect, 11, RGBA(255, 255, 255, 10));
    Txt(chip, chip_rect.x + 10, chip_rect.y + 5, 11, COL_MUTED);
}

/* ========================================================================== */
/* Inspector                                                                  */
/* ========================================================================== */

static void PanelTitle(float x, float y, IconId icon, const char *title) {
    DrawIcon(icon, x, y + 1, 22, COL_MUTED);
    Txt(title, x + 30, y + 4, 17, COL_TEXT);
}

static float InspectorSlider(App *app, Rect area, float y, const char *label, float *value,
                             float min, float max, float def, const char *unit, int decimals, float step) {
    (void)app;
    if (SliderCompact(MakeRect(area.x, y, area.w, 24), label, value, min, max, def, unit, decimals, step)) return 1;
    return 0;
}

static int SmallToggle(Rect r, const char *label, int active) {
    int hover = Interactive(r);
    RoundRect(r, 6, active ? RGBA(255, 255, 255, 34) : (hover ? COL_FIELD_HOVER : COL_FIELD));
    TxtC(label, r.x + r.w * 0.5f, r.y + (r.h - 11) * 0.5f, 11, active ? COL_TEXT : COL_MUTED);
    if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
    return Clicked(r);
}

static float DrawTransformPanel(App *app, Clip *clip, Rect area, float y) {
    float start = y;
    PanelTitle(area.x, y, ICON_SCALE, "Transform");
    y += 34;
    SectionLabel("SCALE", area.x, y);
    y += 18;
    Rect link_scale = MakeRect(area.x + area.w - 78, y - 4, 78, 20);
    if (SmallToggle(link_scale, app->link_scale ? "Linked" : "Unlinked", app->link_scale)) app->link_scale = !app->link_scale;
    y += 4;
    float before_w = clip->scale_w, before_h = clip->scale_h;
    if (InspectorSlider(app, area, y, "Width", &clip->scale_w, 10, 400, 100, "%", 0, 1)) MarkPropsDirty(app, clip);
    y += 27;
    if (InspectorSlider(app, area, y, "Height", &clip->scale_h, 10, 400, 100, "%", 0, 1)) MarkPropsDirty(app, clip);
    y += 27;
    if (app->link_scale) {
        if (clip->scale_w != before_w) clip->scale_h = clip->scale_w;
        else if (clip->scale_h != before_h) clip->scale_w = clip->scale_h;
    }
    y += 6;
    SectionLabel("POSITION", area.x, y);
    y += 18;
    Rect link_pos = MakeRect(area.x + area.w - 78, y - 4, 78, 20);
    if (SmallToggle(link_pos, "Reset", 0)) {
        clip->pos_x = clip->pos_y = 0;
        MarkPropsDirty(app, clip);
    }
    y += 4;
    float before_x = clip->pos_x, before_y = clip->pos_y;
    if (InspectorSlider(app, area, y, "X", &clip->pos_x, -960, 960, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
    y += 27;
    if (InspectorSlider(app, area, y, "Y", &clip->pos_y, -540, 540, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
    y += 27;
    (void)before_x;
    (void)before_y;
    y += 4;
    SectionLabel("ROTATION", area.x, y);
    y += 18;
    Rect rot_reset = MakeRect(area.x + area.w - 62, y - 4, 62, 20);
    if (SmallToggle(rot_reset, "Reset", 0)) {
        clip->rotation = 0;
        MarkPropsDirty(app, clip);
    }
    y += 4;
    if (InspectorSlider(app, area, y, "Angle", &clip->rotation, -180, 180, 0, "°", 1, 0.5f)) MarkPropsDirty(app, clip);
    y += 32;
    SectionLabel("OPACITY", area.x, y);
    y += 18;
    if (InspectorSlider(app, area, y, "Opacity", &clip->opacity, 0, 100, 100, "%", 0, 1)) MarkPropsDirty(app, clip);
    y += 34;

    int page = app->transform_page;
    Rect crop_btn = MakeRect(area.x, y, (area.w - 8) * 0.5f, 28);
    Rect distort_btn = MakeRect(area.x + (area.w - 8) * 0.5f + 8, y, (area.w - 8) * 0.5f, 28);
    if (SmallToggle(crop_btn, "Crop", page == 1)) app->transform_page = page == 1 ? 0 : 1;
    if (SmallToggle(distort_btn, "Distort", page == 2)) app->transform_page = page == 2 ? 0 : 2;
    y += 36;
    if (app->transform_page == 1) {
        if (InspectorSlider(app, area, y, "Left", &clip->crop_l, 0, 900, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
        y += 26;
        if (InspectorSlider(app, area, y, "Right", &clip->crop_r, 0, 900, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
        y += 26;
        if (InspectorSlider(app, area, y, "Top", &clip->crop_t, 0, 500, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
        y += 26;
        if (InspectorSlider(app, area, y, "Bottom", &clip->crop_b, 0, 500, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
        y += 30;
        if (SmallToggle(MakeRect(area.x, y, area.w, 24), "Reset crop", 0)) {
            clip->crop_l = clip->crop_r = clip->crop_t = clip->crop_b = 0;
            MarkPropsDirty(app, clip);
        }
        y += 32;
    } else if (app->transform_page == 2) {
        static const char *names[4] = {"Top left", "Top right", "Bottom right", "Bottom left"};
        float *values[4] = {&clip->distort_tl, &clip->distort_tr, &clip->distort_br, &clip->distort_bl};
        for (int i = 0; i < 4; i++) {
            if (InspectorSlider(app, area, y, names[i], values[i], -400, 400, 0, "px", 0, 1)) MarkPropsDirty(app, clip);
            y += 26;
        }
        y += 4;
        if (SmallToggle(MakeRect(area.x, y, area.w, 24), "Reset corners", 0)) {
            clip->distort_tl = clip->distort_tr = clip->distort_br = clip->distort_bl = 0;
            MarkPropsDirty(app, clip);
        }
        y += 32;
    }
    (void)start;
    return y - start;
}

static void GroupHeader(App *app, Rect area, float *y, const char *label, IconId icon, int *open) {
    Rect head = MakeRect(area.x, *y, area.w, 30);
    if (CardHeader(head, icon, label, *open)) *open = !*open;
    *y += 34;
}

static float DrawColorsPanel(App *app, Clip *clip, Rect area, float y) {
    float start = y;
    PanelTitle(area.x, y, ICON_DROPLET, "Colors");
    y += 34;

    GroupHeader(app, area, &y, "Light", ICON_SUN, &app->light_open);
    if (app->light_open) {
        static const char *labels[7] = {"Exposure", "Brightness", "Contrast", "Highlights", "Shadows", "Whites", "Blacks"};
        for (int i = 0; i < 7; i++) {
            float min = i == 0 ? -5.0f : -100.0f;
            float max = i == 0 ? 5.0f : 100.0f;
            int decimals = i == 0 ? 2 : 0;
            float step = i == 0 ? 0.05f : 1.0f;
            if (InspectorSlider(app, area, y, labels[i], &clip->light[i], min, max, 0, i == 0 ? "EV" : "", decimals, step)) MarkPropsDirty(app, clip);
            y += 26;
        }
        y += 6;
    }

    GroupHeader(app, area, &y, "Color", ICON_DROPLET, &app->color_open);
    if (app->color_open) {
        static const char *labels[3] = {"Temperature", "Vibrance", "Saturation"};
        for (int i = 0; i < 3; i++) {
            if (InspectorSlider(app, area, y, labels[i], &clip->color[i], -100, 100, 0, "", 0, 1)) MarkPropsDirty(app, clip);
            y += 26;
        }
        y += 6;
    }

    GroupHeader(app, area, &y, "Detail", ICON_SLIDERS, &app->detail_open);
    if (app->detail_open) {
        static const char *labels[3] = {"Sharpness", "Clarity", "Vignette"};
        for (int i = 0; i < 3; i++) {
            if (InspectorSlider(app, area, y, labels[i], &clip->detail[i], -100, 100, 0, "", 0, 1)) MarkPropsDirty(app, clip);
            y += 26;
        }
        y += 6;
    }

    GroupHeader(app, area, &y, "Color grading", ICON_WHEEL, &app->grading_open);
    if (app->grading_open) {
        static const char *labels[3] = {"Shadows", "Tint", "Highlights"};
        float wheel_w = (area.w - 16) / 3.0f;
        for (int i = 0; i < 3; i++) {
            Rect wheel = MakeRect(area.x + i * (wheel_w + 8), y, wheel_w, wheel_w + 18);
            if (ColorWheel(MakeRect(wheel.x, y, wheel_w, wheel_w), labels[i], &clip->grade[i].hue, &clip->grade[i].amount)) MarkPropsDirty(app, clip);
        }
        y += wheel_w + 26;
        if (SmallToggle(MakeRect(area.x, y, area.w, 24), "Reset grading", 0)) {
            for (int i = 0; i < 3; i++) {
                clip->grade[i].hue = 0;
                clip->grade[i].amount = 0;
            }
            MarkPropsDirty(app, clip);
        }
        y += 32;
    }

    GroupHeader(app, area, &y, "HSL", ICON_GRID, &app->hsl_open);
    if (app->hsl_open) {
        static const char *channels[9] = {"All", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"};
        for (int i = 0; i < 9; i++) {
            Rect chip = MakeRect(area.x + (i % 3) * (area.w - 12) / 3.0f, y + (i / 3) * 28, (area.w - 12) / 3.0f - 4, 24);
            if (SmallToggle(chip, channels[i], app->hsl_channel == i)) app->hsl_channel = i;
        }
        y += 3 * 28 + 6;
        if (InspectorSlider(app, area, y, "Hue", &clip->hsl.h, -100, 100, 0, "", 0, 1)) MarkPropsDirty(app, clip);
        y += 26;
        if (InspectorSlider(app, area, y, "Saturation", &clip->hsl.s, -100, 100, 0, "", 0, 1)) MarkPropsDirty(app, clip);
        y += 26;
        if (InspectorSlider(app, area, y, "Lightness", &clip->hsl.l, -100, 100, 0, "", 0, 1)) MarkPropsDirty(app, clip);
        y += 26;
    }
    return y - start;
}

static float DrawEffectsPanel(App *app, Clip *clip, Rect area, float y, int audio_only) {
    float start = y;
    PanelTitle(area.x, y, ICON_SPARKLES, "Effects");
    y += 34;

    if (!audio_only) {
        int trans_index = 0;
        for (int i = 0; i < 5; i++) {
            if (clip->trans_type[0] && strcmp(clip->trans_type, TRANS_IDS[i]) == 0) trans_index = i;
        }
        int has_next = 0;
        char next_label[PLUTO_LABEL_SIZE] = "";
        double boundary = clip->start + clip->dur;
        for (int i = 0; i < app->clip_count; i++) {
            if (app->clips[i].id == clip->id) continue;
            Clip *other = &app->clips[i];
            if (strcmp(other->track, clip->track) != 0) continue;
            if (other->start < boundary + 0.05 && other->start + other->dur > boundary) {
                has_next = 1;
                CopyStr(next_label, sizeof(next_label), other->label);
                break;
            }
        }

        Rect trans_head = MakeRect(area.x, y, area.w, 30);
        if (CardHeader(trans_head, ICON_TRANSITION, "Transitions", app->trans_open)) app->trans_open = !app->trans_open;
        y += 34;
        if (app->trans_open) {
            Txt(has_next ? "Preview transition into the next clip" : "Place another clip right after this one to add a transition", area.x + 12, y, 11, COL_DIM);
            y += 16;
            Rect trans_seg = MakeRect(area.x, y, area.w, 30);
            if (Segmented(trans_seg, TRANS_IDS, 5, &trans_index)) {
                SetClipTransition(app, clip, TRANS_IDS[trans_index], clip->trans_dur > 0.2f ? clip->trans_dur : 1.0f);
            }
            y += 40;
            if (trans_index != 0) {
                if (InspectorSlider(app, area, y, "Duration", &clip->trans_dur, 0.2f, 3.0f, 1.0f, "s", 1, 0.1f)) SetClipTransition(app, clip, TRANS_IDS[trans_index], clip->trans_dur);
                y += 32;
            }
            y += 6;
        }
        y += 6;
    }

    if (!audio_only) {
        SectionLabel("FILTER PRESETS", area.x, y);
        y += 18;
        float tile_w = (area.w - 24) / 4.0f;
        float tile_h = 48;
        for (int i = 0; i < PRESET_COUNT; i++) {
            Rect tile = MakeRect(area.x + (i % 4) * (tile_w + 8), y + (i / 4) * (tile_h + 6), tile_w, tile_h);
            int hover = Interactive(tile);
            int selected = clip->preset == i;
            Color base = i == 0 ? RGBA(32, 33, 37, 255) : i == 1 ? RGBA(64, 110, 130, 255) : i == 2 ? RGBA(50, 50, 54, 255)
                        : i == 3 ? RGBA(118, 108, 118, 255) : i == 4 ? RGBA(150, 92, 58, 255) : i == 5 ? RGBA(52, 88, 130, 255)
                        : i == 6 ? RGBA(74, 64, 92, 255) : RGBA(128, 98, 62, 255);
            RoundRect(tile, 7, base);
            if (selected) DrawRectangleRoundedLinesEx((Rectangle){tile.x, tile.y, tile.w, tile.h}, 0.16f, 6, 1.5f, COL_TEXT);
            else if (hover) DrawRectangleRoundedLinesEx((Rectangle){tile.x, tile.y, tile.w, tile.h}, 0.16f, 6, 1, RGBA(255, 255, 255, 70));
            TxtC(PRESET_LABELS[i], tile.x + tile.w * 0.5f, tile.y + tile.h - 17, 10.5f, COL_TEXT);
            if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
            if (Clicked(tile) && clip->preset != i) {
                PushUndo(app);
                clip->preset = i;
                MarkPropsDirty(app, clip);
            }
        }
        y += 2 * (tile_h + 6) + 4;
        if (clip->preset != 0) {
            if (InspectorSlider(app, area, y, "Intensity", &clip->preset_intensity, 0, 100, 100, "%", 0, 1)) MarkPropsDirty(app, clip);
            y += 28;
        }
        y += 4;
    }

    for (int category = 0; category < 3; category++) {
        if (audio_only && category != 2) continue;
        if (!audio_only && category == 2) continue;
        const char *title = category == 0 ? "Filters" : category == 1 ? "Video effects" : "Audio effects";
        Rect head = MakeRect(area.x, y, area.w, 30);
        RoundRect(head, 7, RGBA(255, 255, 255, 5));
        if (CardHeader(MakeRect(area.x + 2, y, area.w - 4, 30), category == 0 ? ICON_SLIDERS : category == 1 ? ICON_IMAGE : ICON_MUSIC, title, app->fx_category[category])) {
            app->fx_category[category] = !app->fx_category[category];
        }
        y += 34;
        if (!app->fx_category[category]) continue;
        for (int i = 0; i < FX_COUNT; i++) {
            int belongs = category == 0 ? i <= FX_VIGNETTE : category == 1 ? (i >= FX_CHROMATIC && i <= FX_MIRROR) : (i >= FX_EQ);
            if (!belongs) continue;
            Rect row = MakeRect(area.x, y, area.w, 30);
            int hover = Interactive(row);
            if (hover) RoundRect(row, 6, RGBA(255, 255, 255, 5));
            TxtEllipsis(EFFECTS[i].label, row.x + 8, row.y + 8, 12, row.w - 96, COL_TEXT);
            Rect sw = MakeRect(row.x + row.w - 46, row.y + 4, 38, 22);
            if (Switch(sw, &clip->fx_enabled[i])) MarkPropsDirty(app, clip);
            Rect chev = MakeRect(row.x + row.w - 74, row.y + 3, 24, 24);
            DrawIcon(app->fx_open[i] ? ICON_CHEV_DOWN : ICON_CHEV_RIGHT, chev.x + 5, chev.y + 5, 14, COL_DIM);
            if (Clicked(MakeRect(row.x, row.y, row.w - 80, row.h))) app->fx_open[i] = !app->fx_open[i];
            y += 32;
            if (app->fx_open[i]) {
                if (clip->fx_enabled[i]) {
                    for (int p = 0; p < EFFECTS[i].count; p++) {
                        if (EFFECTS[i].params[p].choice) {
                            const char *choices[3] = {EFFECTS[i].params[p].choices[0], EFFECTS[i].params[p].choices[1], EFFECTS[i].params[p].choices[2]};
                            int count = choices[2] ? 3 : 2;
                            int value = Clampi((int)lroundf(clip->fx[i][p]), 0, count - 1);
                            char label[64];
                            snprintf(label, sizeof(label), "%s", EFFECTS[i].params[p].label);
                            Txt(label, area.x + 12, y + 8, 11, COL_MUTED);
                            Rect seg = MakeRect(area.x + area.w - 180, y, 168, 26);
                            if (Segmented(seg, choices, count, &value)) {
                                clip->fx[i][p] = (float)value;
                                MarkPropsDirty(app, clip);
                            }
                            y += 32;
                        } else {
                            char label[64];
                            snprintf(label, sizeof(label), "%s", EFFECTS[i].params[p].label);
                            if (InspectorSlider(app, area, y, label, &clip->fx[i][p], EFFECTS[i].params[p].min, EFFECTS[i].params[p].max, EFFECTS[i].params[p].def, EFFECTS[i].params[p].unit, EFFECTS[i].params[p].decimals, 1)) MarkPropsDirty(app, clip);
                            y += 26;
                        }
                    }
                } else {
                    Txt("Enable this effect to change its settings.", area.x + 12, y, 11, COL_DIM);
                    y += 22;
                }
                y += 4;
            }
        }
        y += 6;
    }
    return y - start;
}

static float DrawTextPanel(App *app, Clip *clip, Rect area, float y) {
    float start = y;
    PanelTitle(area.x, y, ICON_TYPE, "Text");
    y += 34;
    const char *pages[2] = {"Title", "Subtitles"};
    Rect seg = MakeRect(area.x, y, area.w, 30);
    Segmented(seg, pages, 2, &app->text_page);
    y += 40;

    if (app->text_page == 0) {
        SectionLabel("TITLE", area.x, y);
        y += 18;
        app->text_title.buffer = clip->title;
        app->text_title.cap = (int)sizeof(clip->title);
        app->text_title.rect = MakeRect(area.x, y, area.w, 34);
        TextInput(&app->text_title, "Main title", 0);
        y += 42;
        SectionLabel("SUBTITLE", area.x, y);
        y += 18;
        app->text_sub.buffer = clip->subtitle;
        app->text_sub.cap = (int)sizeof(clip->subtitle);
        app->text_sub.rect = MakeRect(area.x, y, area.w, 34);
        TextInput(&app->text_sub, "Supporting line", 0);
        y += 42;
        SectionLabel("TITLE SIZE", area.x, y);
        y += 18;
        if (InspectorSlider(app, area, y, "Size", &clip->text_size, 40, 200, 100, "%", 0, 1)) MarkPropsDirty(app, clip);
        y += 32;
        SectionLabel("ALIGNMENT", area.x, y);
        y += 18;
        int align = clip->text_align;
        const char *options[3] = {"Left", "Center", "Right"};
        Rect align_rect = MakeRect(area.x, y, area.w, 30);
        if (Segmented(align_rect, options, 3, &align)) {
            clip->text_align = align;
            MarkPropsDirty(app, clip);
        }
        y += 40;
        if (strcmp(clip->theme, "title") != 0) {
            Txt("Tip: use the Text tool on a video track to create a title clip.", area.x, y, 11, COL_DIM);
            y += 24;
        }
    } else {
        SectionLabel("CAPTION", area.x, y);
        y += 18;
        app->text_caption.buffer = clip->caption;
        app->text_caption.cap = (int)sizeof(clip->caption);
        app->text_caption.rect = MakeRect(area.x, y, area.w, 34);
        TextInput(&app->text_caption, "Caption shown near the bottom", 0);
        y += 42;
        Txt("Captions are burned in during export for every clip that has one.", area.x, y, 11, COL_DIM);
        y += 24;
    }
    return y - start;
}

static void InspectorPanel(App *app, Layout layout) {
    Rect panel = layout.inspector;
    DrawCard(panel);
    /* Tabs */
    Rect tabs = MakeRect(panel.x + 12, panel.y + 12, panel.w - 24, 32);
    const char *labels[4] = {"Transform", "Colors", "Effects", "Text"};
    Segmented(tabs, labels, 4, &app->tab);

    Rect content = MakeRect(panel.x + 14, panel.y + 54, panel.w - 28, panel.h - 66);
    BeginScissorMode((int)content.x, (int)content.y, (int)content.w, (int)content.h);

    Clip *clip = SelectedClip(app);
    int track = clip ? TrackIndexForClip(clip) : -1;
    int is_audio = clip && TrackIsAudio(track);
    int is_title = clip && strcmp(clip->theme, "title") == 0;
    int valid = clip && (!is_audio || app->tab == TAB_EFFECTS);

    float content_h = 0;
    if (!clip) {
        DrawIcon(ICON_FILM, content.x + content.w * 0.5f - 15, content.y + 40, 30, COL_DIM);
        TxtC("Nothing to adjust", content.x + content.w * 0.5f, content.y + 82, 13, COL_TEXT);
        TxtC(app->tab == TAB_TEXT ? "Choose the Text tool and click a video track." : "Select a clip on the timeline to edit its properties.",
             content.x + content.w * 0.5f, content.y + 102, 11, COL_DIM);
    } else if (is_audio && app->tab != TAB_EFFECTS) {
        DrawIcon(ICON_MUSIC, content.x + content.w * 0.5f - 15, content.y + 40, 30, COL_DIM);
        TxtC("Audio clip selected", content.x + content.w * 0.5f, content.y + 82, 13, COL_TEXT);
        TxtC("Use the Library Audio tab for level, pan and fades.", content.x + content.w * 0.5f, content.y + 102, 11, COL_DIM);
        TxtC("Open Effects for the audio filter chain.", content.x + content.w * 0.5f, content.y + 120, 11, COL_DIM);
    } else if (is_title && app->tab == TAB_EFFECTS) {
        DrawIcon(ICON_TYPE, content.x + content.w * 0.5f - 15, content.y + 40, 30, COL_DIM);
        TxtC("Text clips are edited in the Text tab", content.x + content.w * 0.5f, content.y + 82, 12, COL_TEXT);
    } else {
        float y = content.y - app->inspector_scroll;
        if (app->tab == TAB_TRANSFORM) content_h = DrawTransformPanel(app, clip, content, y);
        else if (app->tab == TAB_COLORS) content_h = DrawColorsPanel(app, clip, content, y);
        else if (app->tab == TAB_EFFECTS) content_h = DrawEffectsPanel(app, clip, content, y, is_audio);
        else content_h = DrawTextPanel(app, clip, content, y);
        valid = 1;
    }
    EndScissorMode();
    (void)valid;
    app->inspector_content_h = content_h;

    if (Interactive(content)) {
        float wheel = GetMouseWheelMove();
        if (fabsf(wheel) > 0.0f) {
            app->inspector_scroll -= wheel * 46.0f;
        }
    }
    float max_scroll = Maxf(0.0f, app->inspector_content_h - content.h + 8);
    app->inspector_scroll = Clampf(app->inspector_scroll, 0.0f, max_scroll);

    if (max_scroll > 0.0f) {
        float ratio = content.h / Maxf(content.h, app->inspector_content_h);
        float bar_h = Maxf(30.0f, content.h * ratio);
        float bar_y = content.y + (content.h - bar_h) * (app->inspector_scroll / max_scroll);
        RoundRect(MakeRect(panel.x + panel.w - 7, bar_y, 3, bar_h), 2, RGBA(255, 255, 255, 40));
    }
}

/* ========================================================================== */
/* Timeline                                                                   */
/* ========================================================================== */

#define TIMELINE_LABELS 64.0f
#define TIMELINE_RULER 24.0f
#define TIMELINE_ROW 40.0f
#define TIMELINE_BAR 44.0f

static MediaItem *FindMediaByPath(App *app, const char *path) {
    for (int i = 0; i < app->media_count; i++) {
        if (strcmp(app->media[i].path, path) == 0) return &app->media[i];
    }
    return NULL;
}

static double SnapTime(App *app, double time, int skip_index) {
    if (!app->snapping) return time;
    double threshold = 8.0 / app->pps;
    double best = time;
    double best_delta = threshold;
    double candidates[2] = {app->playhead, 0.0};
    for (int i = 0; i < 2; i++) {
        double delta = fabs(candidates[i] - time);
        if (delta < best_delta) {
            best = candidates[i];
            best_delta = delta;
        }
    }
    for (int i = 0; i < app->clip_count; i++) {
        if (i == skip_index) continue;
        double edges[2] = {app->clips[i].start, app->clips[i].start + app->clips[i].dur};
        for (int e = 0; e < 2; e++) {
            double delta = fabs(edges[e] - time);
            if (delta < best_delta) {
                best = edges[e];
                best_delta = delta;
            }
        }
    }
    return best;
}

static void BackendReconcileGeometry(App *app) {
    if (!app->backend_ok) return;
    int count = pluto_timeline_count();
    for (int i = 0; i < count; i++) {
        PlutoClipInfo info;
        if (!pluto_timeline_get_clip(i, &info)) continue;
        int index = FindClipIndex(app, info.id);
        if (index < 0) continue;
        app->clips[index].start = info.start;
        app->clips[index].dur = info.dur;
        CopyStr(app->clips[index].track, sizeof(app->clips[index].track), info.track);
    }
    app->preview_dirty = 1;
}

/* The clip that immediately follows `index` on the same track, if any. */
static int NextClipIndex(App *app, int index) {
    Clip *clip = &app->clips[index];
    double end = clip->start + clip->dur;
    for (int i = 0; i < app->clip_count; i++) {
        if (i == index) continue;
        Clip *other = &app->clips[i];
        if (strcmp(other->track, clip->track) != 0) continue;
        if (other->start < end + 0.05 && other->start + other->dur > end) return i;
    }
    return -1;
}

static void SetClipTransition(App *app, Clip *clip, const char *type, float dur) {
    PushUndo(app);
    if (type && type[0] && strcmp(type, "none") != 0) {
        CopyStr(clip->trans_type, sizeof(clip->trans_type), type);
        clip->trans_dur = dur > 0.2f ? dur : 1.0f;
        MarkPropsDirty(app, clip);
        pluto_timeline_set_transition(clip->id, clip->trans_type, (double)clip->trans_dur);
    } else {
        clip->trans_type[0] = '\0';
        clip->trans_dur = 1.0f;
        MarkPropsDirty(app, clip);
        pluto_timeline_set_transition(clip->id, "", (double)clip->trans_dur);
    }
    app->preview_dirty = 1;
}

static void SplitSelectedClip(App *app, int index, double at) {
    if (index < 0 || index >= app->clip_count) return;
    Clip *clip = &app->clips[index];
    if (at <= clip->start + 0.05 || at >= clip->start + clip->dur - 0.05) return;
    if (app->clip_count >= MAX_CLIPS) {
        Toast(app, "The timeline is full");
        return;
    }
    PushUndo(app);
    Clip right = *clip;
    snprintf(right.id, sizeof(right.id), "clip-%d", ++app->next_clip);
    right.start = at;
    right.dur = clip->start + clip->dur - at;
    right.source_offset = clip->source_offset + (at - clip->start);
    CopyStr(right.label, sizeof(right.label), clip->label);
    right.trans_type[0] = '\0';
    clip->dur = at - clip->start;
    int insert = index + 1;
    memmove(&app->clips[insert + 1], &app->clips[insert], sizeof(Clip) * (size_t)(app->clip_count - insert));
    app->clips[insert] = right;
    app->clip_count++;
    app->selected = insert;
    if (app->backend_ok) {
        if (!pluto_timeline_split_clip(clip->id, at, right.id)) {
            Toastf(app, "Timeline backend: %s", pluto_last_error());
        } else {
            BuildClipProps(&right);
            pluto_timeline_set_props(right.id, g_props_buf);
        }
    }
    app->preview_dirty = 1;
}

static void DrawTimelineRuler(App *app, Rect area, double duration) {
    double step = 1.0;
    double candidates[7] = {0.5, 1, 2, 5, 10, 30, 60};
    for (int i = 0; i < 7; i++) {
        if (candidates[i] * app->pps >= 68) {
            step = candidates[i];
            break;
        }
        step = candidates[i];
    }
    double start_time = app->scroll_x / app->pps;
    double end_time = start_time + area.w / app->pps;
    double first = floor(start_time / step) * step;
    for (double t = first; t <= end_time + step; t += step) {
        if (t < 0) continue;
        float x = area.x + (float)(t * app->pps - app->scroll_x);
        if (x < area.x - 1 || x > area.x + area.w) continue;
        DrawRectangle((int)x, (int)area.y + 14, 1, 8, RGBA(255, 255, 255, 40));
        char label[32];
        int seconds = (int)lround(t);
        snprintf(label, sizeof(label), "%d:%02d", seconds / 60, seconds % 60);
        Txt(label, x + 4, area.y + 2, 10, COL_DIM);
    }
    (void)duration;
}

static void TimelinePanel(App *app, Layout layout) {
    Rect panel = layout.timeline;
    DrawCard(panel);

    /* Toolbar */
    Rect bar = MakeRect(panel.x + 10, panel.y + 7, panel.w - 20, 34);
    float x = bar.x;
    const IconId tool_icons[4] = {ICON_CURSOR, ICON_HAND, ICON_SCISSORS, ICON_TYPE};
    const char *tool_tips[4] = {"Select (V)", "Hand (H)", "Razor (C)", "Text Tool (T)"};
    for (int i = 0; i < 4; i++) {
        Rect r = MakeRect(x, bar.y, 32, 32);
        if (IconButton(r, tool_icons[i], app->tool == i, tool_tips[i])) app->tool = i;
        x += 34;
    }
    x += 8;
    Rect delete_rect = MakeRect(x, bar.y, 32, 32);
    if (IconButton(delete_rect, ICON_TRASH, 0, "Delete (Del)")) {
        if (app->selected >= 0) RemoveClipAt(app, app->selected);
    }
    x += 44;

    float right = bar.x + bar.w;
    Rect snap_rect = MakeRect(right - 34, bar.y, 32, 32);
    if (IconButton(snap_rect, ICON_MAGNET, app->snapping, "Snapping")) {
        app->snapping = !app->snapping;
        if (app->backend_ok) pluto_timeline_set_snapping(app->snapping);
    }
    right -= 46;
    Rect zoom_in = MakeRect(right - 32, bar.y, 32, 32);
    if (IconButton(zoom_in, ICON_ZOOM_IN, 0, "Zoom in")) app->pps = Clampf(app->pps * 1.18f, 6, 120);
    right -= 36;
    Rect zoom_out = MakeRect(right - 32, bar.y, 32, 32);
    if (IconButton(zoom_out, ICON_ZOOM_OUT, 0, "Zoom out")) app->pps = Clampf(app->pps / 1.18f, 6, 120);
    right -= 36;
    SliderEx(MakeRect(right - 190, bar.y + 1, 182, 32), "Zoom", &app->pps, 6, 120, 24, " px/s", 0, 1);

    /* Body */
    Rect body = MakeRect(panel.x + 10, panel.y + TIMELINE_BAR + 2, panel.w - 20, panel.h - TIMELINE_BAR - 12);
    Rect labels = MakeRect(body.x, body.y, TIMELINE_LABELS, body.h);
    Rect scroll = MakeRect(body.x + TIMELINE_LABELS, body.y, body.w - TIMELINE_LABELS, body.h);
    Rect ruler = MakeRect(scroll.x, scroll.y, scroll.w, TIMELINE_RULER);
    double duration = TimelineDuration(app);

    BeginScissorMode((int)scroll.x, (int)scroll.y, (int)scroll.w, (int)scroll.h);
    DrawRectangle((int)scroll.x, (int)scroll.y, (int)scroll.w, (int)scroll.h, RGBA(10, 11, 13, 255));
    DrawTimelineRuler(app, ruler, duration);
    DrawRectangle((int)scroll.x, (int)(ruler.y + TIMELINE_RULER - 1), (int)scroll.w, 1, RGBA(255, 255, 255, 16));

    for (int row = 0; row < MAX_TRACKS; row++) {
        Rect lane = MakeRect(scroll.x, ruler.y + TIMELINE_RULER + row * TIMELINE_ROW, scroll.w, TIMELINE_ROW);
        DrawRectangle((int)lane.x, (int)lane.y, (int)lane.w, (int)lane.h, row % 2 ? RGBA(13, 14, 17, 255) : RGBA(15, 16, 19, 255));
        DrawRectangle((int)lane.x, (int)(lane.y + lane.h - 1), (int)lane.w, 1, RGBA(255, 255, 255, 10));
    }
    /* Vertical grid lines */
    double step = 1.0;
    double candidates[7] = {0.5, 1, 2, 5, 10, 30, 60};
    for (int i = 0; i < 7; i++) {
        if (candidates[i] * app->pps >= 68) { step = candidates[i]; break; }
        step = candidates[i];
    }
    for (double t = floor(0.0) * step; t * app->pps < app->scroll_x + scroll.w; t += step) {
        float gx = scroll.x + (float)(t * app->pps - app->scroll_x);
        if (gx < scroll.x) continue;
        DrawRectangle((int)gx, (int)(ruler.y + TIMELINE_RULER), 1, (int)(scroll.h - TIMELINE_RULER), RGBA(255, 255, 255, 7));
    }

    /* Clips */
    for (int i = 0; i < app->clip_count; i++) {
        Clip *clip = &app->clips[i];
        int row = TrackIndexForClip(clip);
        float cx = scroll.x + (float)(clip->start * app->pps - app->scroll_x);
        float cw = Maxf(10.0f, (float)(clip->dur * app->pps));
        float cy = ruler.y + TIMELINE_RULER + row * TIMELINE_ROW + 3;
        float ch = TIMELINE_ROW - 7;
        if (cx > scroll.x + scroll.w || cx + cw < scroll.x) continue;
        Rect rect = MakeRect(cx, cy, cw, ch);
        Color base = ClipColor(clip);
        RoundRect(rect, 6, base);
        BeginScissorMode((int)Maxf(rect.x, scroll.x), (int)rect.y, (int)Minf(rect.w, scroll.x + scroll.w - rect.x), (int)rect.h);
        MediaItem *media = FindMediaByPath(app, clip->path);
        if (media) {
            Texture2D tex = {0};
            int has = 0;
            if (TrackIsAudio(row) && media->has_waveform) { tex = media->waveform; has = 1; }
            else if (!TrackIsAudio(row) && media->has_filmstrip) { tex = media->filmstrip; has = 1; }
            if (has) {
                Rectangle source = {0, 0, (float)tex.width, (float)tex.height};
                Rectangle dest = {rect.x, rect.y, rect.w, rect.h};
                DrawTexturePro(tex, source, dest, (Vector2){0, 0}, 0, TrackIsAudio(row) ? RGBA(120, 200, 230, 90) : RGBA(255, 255, 255, 95));
            }
        }
        DrawRectangleGradientV((int)rect.x, (int)rect.y, (int)rect.w, (int)rect.h, RGBA(0, 0, 0, 40), RGBA(0, 0, 0, 130));
        if (strcmp(clip->theme, "title") == 0) {
            DrawRectangle((int)rect.x, (int)rect.y, 3, (int)rect.h, RGBA(190, 175, 255, 255));
        }
        char label[160];
        snprintf(label, sizeof(label), "%s", clip->label);
        TxtEllipsis(label, rect.x + 8, rect.y + rect.h - 18, 11, rect.w - 16, RGBA(255, 255, 255, 230));
        if (clip->preset != 0) {
            DrawSparkleBadge(rect.x + rect.w - 18, rect.y + 5);
        }
        EndScissorMode();
        if (app->selected == i) {
            DrawRectangleRoundedLinesEx((Rectangle){rect.x - 1, rect.y - 1, rect.w + 2, rect.h + 2}, 0.14f, 8, 1.5f, COL_TEXT);
            Rect left_handle = MakeRect(rect.x + 1, rect.y + rect.h * 0.5f - 9, 4, 18);
            Rect right_handle = MakeRect(rect.x + rect.w - 5, rect.y + rect.h * 0.5f - 9, 4, 18);
            RoundRect(left_handle, 2, RGBA(255, 255, 255, 190));
            RoundRect(right_handle, 2, RGBA(255, 255, 255, 190));
        } else {
            DrawRectangleRoundedLinesEx((Rectangle){rect.x, rect.y, rect.w, rect.h}, 0.14f, 8, 1, RGBA(255, 255, 255, 30));
        }
    }

    /* Transition junction markers at clip boundaries. */
    for (int i = 0; i < app->clip_count; i++) {
        int next_i = NextClipIndex(app, i);
        if (next_i < 0) continue;
        Clip *clip = &app->clips[i];
        double boundary = clip->start + clip->dur;
        float jx = scroll.x + (float)(boundary * app->pps - app->scroll_x);
        if (jx < scroll.x - 4 || jx > scroll.x + scroll.w + 4) continue;
        Rect junction = MakeRect(jx - 9, ruler.y + TIMELINE_RULER + TrackIndexForClip(clip) * TIMELINE_ROW + TIMELINE_ROW * 0.5f - 9, 18, 18);
        int hover = Interactive(junction);
        RoundRect(junction, 9, hover ? RGBA(255, 255, 255, 28) : RGBA(255, 255, 255, 10));
        DrawIcon(ICON_TRANSITION, junction.x + 9, junction.y + 9, 10, COL_DIM);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(junction)) {
            if (clip->trans_type[0] == '\0') SetClipTransition(app, clip, "dissolve", 1.0f);
            else SetClipTransition(app, clip, "none", 1.0f);
        }
    }

    /* Playhead */
    float play_x = scroll.x + (float)(app->playhead * app->pps - app->scroll_x);
    if (play_x >= scroll.x - 2 && play_x <= scroll.x + scroll.w + 2) {
        DrawRectangle((int)play_x, (int)ruler.y, 1, (int)scroll.h, COL_RED);
        DrawTriangle((Vector2){play_x - 5, ruler.y}, (Vector2){play_x + 5, ruler.y}, (Vector2){play_x, ruler.y + 9}, COL_RED);
    }

    /* Empty hint */
    if (app->clip_count == 0) {
        TxtC("Drop media here to start your timeline", scroll.x + scroll.w * 0.5f, ruler.y + TIMELINE_RULER + TIMELINE_ROW * 2.5f, 12, COL_DIM);
    }
    EndScissorMode();

    /* Track labels */
    for (int row = 0; row < MAX_TRACKS; row++) {
        Rect lr = MakeRect(labels.x, ruler.y + TIMELINE_RULER + row * TIMELINE_ROW, labels.w, TIMELINE_ROW);
        DrawRectangle((int)lr.x, (int)lr.y, (int)lr.w, (int)lr.h, row % 2 ? RGBA(16, 17, 20, 255) : RGBA(18, 19, 22, 255));
        DrawRectangle((int)(lr.x + lr.w - 1), (int)lr.y, 1, (int)lr.h, RGBA(255, 255, 255, 12));
        IconId icon = TrackIsAudio(row) ? ICON_MUSIC : ICON_FILM;
        DrawIcon(icon, lr.x + 10, lr.y + 12, 14, COL_DIM);
        Txt(TRACK_LABELS[row], lr.x + 31, lr.y + 13, 11.5f, COL_MUTED);
    }
    DrawRectangle((int)labels.x, (int)labels.y, (int)labels.w, (int)TIMELINE_RULER, RGBA(15, 16, 18, 255));
    DrawRectangle((int)labels.x, (int)(labels.y + TIMELINE_RULER - 1), (int)labels.w, 1, RGBA(255, 255, 255, 16));

    /* ---- Input ---------------------------------------------------------- */
    int row_hit = -1;
    int clip_hit = -1;
    double time_hit = 0;
    Rect tracks_area = MakeRect(scroll.x, ruler.y + TIMELINE_RULER, scroll.w, TIMELINE_ROW * MAX_TRACKS);
    if (!g_block_ui && RectHit(tracks_area, g_mouse)) {
        row_hit = Clampi((int)((g_mouse.y - tracks_area.y) / TIMELINE_ROW), 0, MAX_TRACKS - 1);
        time_hit = (g_mouse.x - scroll.x + app->scroll_x) / app->pps;
        for (int i = 0; i < app->clip_count; i++) {
            Clip *clip = &app->clips[i];
            if (TrackIndexForClip(clip) != row_hit) continue;
            if (time_hit >= clip->start && time_hit <= clip->start + clip->dur) {
                clip_hit = i;
                break;
            }
        }
    }

    if (!g_block_ui && g_mouse_pressed && RectHit(scroll, g_mouse)) {
        if (RectHit(ruler, g_mouse) || g_mouse.y < ruler.y + TIMELINE_RULER) {
            app->playhead = Maxd(0, (g_mouse.x - scroll.x + app->scroll_x) / app->pps);
            app->drag.active = 0;
            app->dragging_playhead = 1;
            app->playing = 0;
        } else if (row_hit >= 0) {
            if (app->tool == TOOL_TEXT && !TrackIsAudio(row_hit)) {
                AddTitleClip(app, TRACK_IDS[row_hit], time_hit);
            } else if (app->tool == TOOL_RAZOR) {
                if (clip_hit >= 0) SplitSelectedClip(app, clip_hit, time_hit);
            } else if (clip_hit >= 0) {
                Clip *clip = &app->clips[clip_hit];
                float cx = scroll.x + (float)(clip->start * app->pps - app->scroll_x);
                float cw = Maxf(10.0f, (float)(clip->dur * app->pps));
                app->selected = clip_hit;
                app->drag.active = 1;
                app->drag.index = clip_hit;
                app->drag.backup = *clip;
                app->drag.grab_time = time_hit;
                app->drag.target_track = row_hit;
                app->drag.moved = 0;
                if (g_mouse.x - cx < 7) app->drag.mode = 2;
                else if (cx + cw - g_mouse.x < 7) app->drag.mode = 3;
                else app->drag.mode = 1;
                app->drag.press_start = clip->start;
                app->drag.press_dur = clip->dur;
                app->drag.press_offset = clip->source_offset;
                app->drag.press_x = g_mouse.x;
                PushUndo(app);
                if (app->drag.mode == 1) app->dragging_playhead = 0;
            } else {
                app->selected = -1;
                app->drag.active = 0;
            }
        }
    }

    /* Wheel scroll and zoom */
    if (!g_block_ui && RectHit(scroll, g_mouse)) {
        float wheel = GetMouseWheelMove();
        if (fabsf(wheel) > 0.0f) {
            if (IsKeyDown(KEY_LEFT_CONTROL) || IsKeyDown(KEY_RIGHT_CONTROL)) {
                double anchor = (g_mouse.x - scroll.x + app->scroll_x) / app->pps;
                app->pps = Clampf(app->pps * (wheel > 0 ? 1.1f : 0.9f), 6, 120);
                app->scroll_x = (float)(anchor * app->pps - (g_mouse.x - scroll.x));
            } else {
                app->scroll_x = Maxf(0.0f, app->scroll_x - wheel * 60.0f);
            }
        }
    }

    /* Drag update */
    if (app->drag.active) {
        int i = app->drag.index;
        if (i < 0 || i >= app->clip_count) {
            app->drag.active = 0;
        } else {
            Clip *clip = &app->clips[i];
            double cursor = (g_mouse.x - scroll.x + app->scroll_x) / app->pps;
            if (app->drag.mode == 1) {
                double start = app->drag.press_start + (cursor - app->drag.grab_time);
                start = Maxd(0, SnapTime(app, start, i));
                clip->start = start;
                int row = Clampi((int)((g_mouse.y - tracks_area.y) / TIMELINE_ROW), 0, MAX_TRACKS - 1);
                int target = row;
                if (TrackIsAudio(TrackIndex(clip->track)) != TrackIsAudio(target)) target = TrackIndex(clip->track);
                CopyStr(clip->track, sizeof(clip->track), TRACK_IDS[target]);
                app->drag.target_track = target;
                app->drag.moved = 1;
            } else if (app->drag.mode == 2) {
                double original_end = app->drag.press_start + app->drag.press_dur;
                double start = Clampd(app->drag.press_start + (cursor - app->drag.grab_time), 0.0, original_end - 0.2);
                if (app->snapping) start = SnapTime(app, start, i);
                if (start > original_end - 0.2) start = original_end - 0.2;
                double max_back = app->drag.backup.source_duration > 0.0 ? app->drag.press_offset : 0.0;
                if (start < app->drag.press_start - max_back) start = app->drag.press_start - max_back;
                double delta = start - app->drag.press_start;
                clip->start = start;
                clip->dur = app->drag.press_dur - delta;
                clip->source_offset = app->drag.press_offset + delta;
                app->drag.moved = 1;
            } else if (app->drag.mode == 3) {
                double end = app->drag.press_start + app->drag.press_dur + (cursor - app->drag.grab_time);
                if (app->snapping) end = SnapTime(app, end, i);
                double limit = app->drag.press_start + 0.2;
                if (app->drag.backup.source_duration > 0.0) {
                    double available = app->drag.backup.source_duration - app->drag.press_offset;
                    if (available > 0.2) limit = app->drag.press_start + available;
                }
                if (end > limit) end = limit;
                clip->dur = Maxd(0.2, end - clip->start);
                app->drag.moved = 1;
            }
            app->preview_dirty = 1;
            if (g_mouse_released || !IsMouseButtonDown(MOUSE_BUTTON_LEFT)) {
                Clip *src = &app->clips[i];
                int ok = 1;
                if (app->backend_ok) {
                    if (app->drag.mode == 1) {
                        ok = pluto_timeline_move_clip(src->id, src->start, src->track, app->playhead, app->snapping ? 0.15 : 0.0);
                    } else {
                        ok = pluto_timeline_trim_clip(src->id, src->start, src->start + src->dur);
                    }
                }
                if (!ok) {
                    Toastf(app, "Timeline: %s", pluto_last_error());
                    *src = app->drag.backup;
                } else {
                    if (app->drag.mode != 1 && app->backend_ok) {
                        BuildClipProps(src);
                        pluto_timeline_set_props(src->id, g_props_buf);
                    }
                    BackendReconcileGeometry(app);
                }
                app->drag.active = 0;
            }
        }
    }

    if (app->dragging_playhead) {
        if (IsMouseButtonDown(MOUSE_BUTTON_LEFT) && !g_block_ui) {
            app->playhead = Maxd(0, (g_mouse.x - scroll.x + app->scroll_x) / app->pps);
            app->playing = 0;
        } else {
            app->dragging_playhead = 0;
        }
    }

    float max_scroll = (float)Maxd(0.0, duration * app->pps + 80 - scroll.w);
    app->scroll_x = Clampf(app->scroll_x, 0.0f, max_scroll);
}

/* ========================================================================== */
/* Export drawer                                                              */
/* ========================================================================== */

static const char *ImageFormatNames[4] = {"PNG", "JPG", "WebP", "AVIF"};
static const char *VideoFormatNames[3] = {"MP4", "WebM", "GIF"};
static const char *ImageExtensions[4] = {"png", "jpg", "webp", "avif"};
static const char *VideoExtensions[3] = {"mp4", "webm", "gif"};
static const char *SizePresetNames[5] = {"Original", "4K", "1080p", "720p", "Custom"};

static const char *ExportExtension(App *app) {
    return app->export_cfg.mode == 0 ? ImageExtensions[Clampi(app->export_cfg.image_format, 0, 3)]
                                     : VideoExtensions[Clampi(app->export_cfg.video_format, 0, 2)];
}

static void ApplySizePreset(App *app, int preset) {
    app->export_cfg.size_index = preset;
    switch (preset) {
        case 0: app->export_cfg.width = 1920; app->export_cfg.height = 1080; break;
        case 1: app->export_cfg.width = 3840; app->export_cfg.height = 2160; break;
        case 2: app->export_cfg.width = 1920; app->export_cfg.height = 1080; break;
        case 3: app->export_cfg.width = 1280; app->export_cfg.height = 720; break;
        default: break;
    }
    snprintf(app->export_cfg.width_text, sizeof(app->export_cfg.width_text), "%d", app->export_cfg.width);
    snprintf(app->export_cfg.height_text, sizeof(app->export_cfg.height_text), "%d", app->export_cfg.height);
}

static void DrawWrapped(const char *text, float x, float y, float width, float size, Color color) {
    char buffer[1024];
    CopyStr(buffer, sizeof(buffer), text);
    char line[1024] = {0};
    float line_y = y;
    char *cursor = buffer;
    while (*cursor) {
        char word[256];
        int word_len = 0;
        while (*cursor == ' ') cursor++;
        while (*cursor && *cursor != ' ' && word_len < (int)sizeof(word) - 1) word[word_len++] = *cursor++;
        word[word_len] = '\0';
        char attempt[sizeof(line) + sizeof(word) + 2];
        if (line[0]) snprintf(attempt, sizeof(attempt), "%s %s", line, word);
        else snprintf(attempt, sizeof(attempt), "%s", word);
        if (TxtW(attempt, size) > width && line[0]) {
            Txt(line, x, line_y, size, color);
            line_y += size + 4;
            CopyStr(line, sizeof(line), word);
        } else {
            CopyStr(line, sizeof(line), attempt);
        }
    }
    if (line[0]) Txt(line, x, line_y, size, color);
}

static int EnsureExportPath(App *app) {
    if (app->export_output[0]) return 1;
    char ext[16];
    const char *extension = ExportExtension(app);
    char default_name[128];
    const char *name = app->export_cfg.name[0] ? app->export_cfg.name : "pluto-export";
    snprintf(default_name, sizeof(default_name), "%s.%s", name, extension);
    snprintf(ext, sizeof(ext), "%s", extension);
    char path[PLUTO_PATH_SIZE] = {0};
    int result = pluto_platform_save_file("Export from Pluto", default_name, "Export file", ext, path, sizeof(path));
    if (result != 1 || !path[0]) return 0;
    CopyStr(app->export_output, sizeof(app->export_output), path);
    return 1;
}

static void StartExport(App *app) {
    if (app->export_running) return;
    if (!app->backend_ok) {
        Toast(app, "The Rust backend is not loaded");
        return;
    }
    if (!EnsureExportPath(app)) return;
    BackendFlushProps(app, 1);
    pluto_export_reset();
    const char *settings = BuildSettingsJson(app);
    if (!pluto_export_start(settings, app->playhead, app->export_output)) {
        Toastf(app, "Export failed to start: %s", pluto_last_error());
        return;
    }
    app->export_running = 1;
    app->export_finished = 0;
    app->export_success = 0;
    app->export_canceled = 0;
    app->export_progress = 0.0;
    app->export_message[0] = '\0';
    app->export_output[0] = '\0';
    Toast(app, "Export started");
}

static void PollExport(App *app) {
    if (!app->export_running) {
        if (pluto_bridge_available() && pluto_export_running()) app->export_running = 1;
        else return;
    }
    PlutoExportStatus status;
    if (!pluto_export_status(&status)) return;
    app->export_progress = status.progress;
    app->export_finished = status.finished;
    app->export_success = status.success;
    app->export_canceled = status.canceled;
    CopyStr(app->export_message, sizeof(app->export_message), status.message);
    if (status.output[0]) CopyStr(app->export_output, sizeof(app->export_output), status.output);
    if (!status.running) {
        app->export_running = 0;
        if (status.success) app->export_progress = 1.0;
    }
}

static void ExportDrawer(App *app, Layout layout) {
    g_block_ui = 0;
    Rect scrim = MakeRect(0, 0, (float)app->width, (float)app->height);
    DrawRectangleRec((Rectangle){scrim.x, scrim.y, scrim.w, scrim.h}, RGBA(5, 6, 8, 150));
    float width = Minf(430.0f, app->width - 60.0f);
    Rect panel = MakeRect(app->width - width - 10, 10, width, app->height - 20);
    if (g_mouse_pressed && !RectHit(panel, g_mouse)) {
        app->drawer_open = 0;
        g_click_consumed = 1;
        return;
    }
    RoundRect(panel, 14, RGBA(17, 18, 21, 252));
    RoundRectBorder(panel, 14, BLANK, 1, COL_BORDER);
    BeginScissorMode((int)panel.x, (int)panel.y, (int)panel.w, (int)panel.h);

    float x = panel.x + 20;
    float w = panel.w - 40;
    float y = panel.y + 18;
    DrawIcon(ICON_DOWNLOAD, x, y + 2, 22, COL_MUTED);
    Txt("Export", x + 32, y + 3, 20, COL_TEXT);
    Rect close_rect = MakeRect(panel.x + panel.w - 46, y, 28, 28);
    if (IconButton(close_rect, ICON_X, 0, "Close export")) app->drawer_open = 0;
    y += 46;

    const char *modes[2] = {"Image", "Video"};
    Rect mode_rect = MakeRect(x, y, w, 34);
    if (Segmented(mode_rect, modes, 2, &app->export_cfg.mode)) {
        app->export_output[0] = '\0';
        app->export_finished = 0;
    }
    y += 46;

    SectionLabel("FORMAT", x, y);
    y += 18;
    Rect format_rect = MakeRect(x, y, w, 32);
    if (app->export_cfg.mode == 0) {
        int value = app->export_cfg.image_format;
        if (Segmented(format_rect, ImageFormatNames, 4, &value)) {
            app->export_cfg.image_format = value;
            app->export_output[0] = '\0';
        }
    } else {
        int value = app->export_cfg.video_format;
        if (Segmented(format_rect, VideoFormatNames, 3, &value)) {
            app->export_cfg.video_format = value;
            app->export_output[0] = '\0';
        }
    }
    y += 44;

    if (app->export_cfg.mode == 0) {
        if (SliderCompact(MakeRect(x, y, w, 24), "Quality", &app->export_cfg.image_quality, 1, 100, 90, "%", 0, 1)) {
            app->export_finished = 0;
        }
        y += 30;
        if (SwitchRow(MakeRect(x, y, w, 24), "Transparent background", &app->export_cfg.transparent)) app->export_finished = 0;
        y += 30;
        if (SwitchRow(MakeRect(x, y, w, 24), "Embed metadata", &app->export_cfg.metadata)) app->export_finished = 0;
        y += 34;
        SectionLabel("COLOR SPACE", x, y);
        y += 18;
        const char *spaces[2] = {"sRGB", "Display P3"};
        Rect space_rect = MakeRect(x, y, w, 30);
        Segmented(space_rect, spaces, 2, &app->export_cfg.color_space);
        y += 42;
    } else {
        SectionLabel("FRAME RATE", x, y);
        y += 18;
        const char *fps_names[3] = {"24", "30", "60"};
        Rect fps_rect = MakeRect(x, y, w, 30);
        Segmented(fps_rect, fps_names, 3, &app->export_cfg.fps_index);
        y += 40;
        SectionLabel("QUALITY", x, y);
        y += 18;
        const char *qualities[4] = {"Low", "Medium", "High", "Max"};
        Rect quality_rect = MakeRect(x, y, w, 30);
        Segmented(quality_rect, qualities, 4, &app->export_cfg.video_quality);
        y += 40;
        if (SwitchRow(MakeRect(x, y, w, 24), "Include audio", &app->export_cfg.include_audio)) app->export_finished = 0;
        y += 30;
        if (SwitchRow(MakeRect(x, y, w, 24), "Loop GIF", &app->export_cfg.loop)) app->export_finished = 0;
        y += 34;
    }

    DrawRectangle((int)x, (int)y, (int)w, 1, COL_BORDER);
    y += 14;
    SectionLabel("SIZE", x, y);
    y += 18;
    int preset = app->export_cfg.size_index;
    Rect preset_rect = MakeRect(x, y, w, 30);
    if (Segmented(preset_rect, SizePresetNames, 5, &preset)) ApplySizePreset(app, preset);
    y += 40;
    app->export_width.rect = MakeRect(x, y, (w - 30) * 0.5f, 32);
    app->export_height.rect = MakeRect(x + (w - 30) * 0.5f + 30, y, (w - 30) * 0.5f, 32);
    TextInput(&app->export_width, "Width", 1);
    TextInput(&app->export_height, "Height", 1);
    TxtC("×", x + w * 0.5f, y + 9, 13, COL_DIM);
    if (app->export_width.buffer[0]) {
        int parse = atoi(app->export_width.buffer);
        if (parse >= 16 && parse <= 7680 && parse != app->export_cfg.width) {
            app->export_cfg.width = parse;
            app->export_cfg.size_index = 4;
        }
    }
    if (app->export_height.buffer[0]) {
        int parse = atoi(app->export_height.buffer);
        if (parse >= 16 && parse <= 4320 && parse != app->export_cfg.height) {
            app->export_cfg.height = parse;
            app->export_cfg.size_index = 4;
        }
    }
    y += 46;

    SectionLabel("FILE NAME", x, y);
    y += 18;
    app->export_name.rect = MakeRect(x, y, w - 64, 32);
    TextInput(&app->export_name, "pluto-export", 0);
    Rect ext_chip = MakeRect(x + w - 58, y, 58, 32);
    RoundRect(ext_chip, 7, COL_FIELD);
    char extension[24];
    snprintf(extension, sizeof(extension), ".%s", ExportExtension(app));
    TxtC(extension, ext_chip.x + ext_chip.w * 0.5f, ext_chip.y + 9, 11, COL_MUTED);
    y += 42;

    SectionLabel("DESTINATION", x, y);
    y += 18;
    Rect dest_rect = MakeRect(x, y, w, 48);
    {
        int hover = Interactive(dest_rect);
        RoundRect(dest_rect, 8, hover ? COL_FIELD_HOVER : COL_FIELD);
        DrawIcon(ICON_FOLDER, dest_rect.x + 12, dest_rect.y + 14, 18, COL_MUTED);
        if (app->export_output[0]) {
            TxtEllipsis(app->export_output, dest_rect.x + 40, dest_rect.y + 8, 10.5f, dest_rect.w - 110, COL_MUTED);
            TxtEllipsis(app->export_output, dest_rect.x + 40, dest_rect.y + 24, 11.5f, dest_rect.w - 110, COL_TEXT);
        } else {
            Txt("Choose where to save the export", dest_rect.x + 40, dest_rect.y + 16, 11.5f, COL_MUTED);
        }
        TxtR("Change", dest_rect.x + dest_rect.w - 14, dest_rect.y + 17, 11.5f, hover ? COL_TEXT : COL_MUTED);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(dest_rect)) {
            app->export_output[0] = '\0';
            EnsureExportPath(app);
            app->export_finished = 0;
        }
    }
    y += 58;

    Rect go_rect = MakeRect(x, y, app->export_running ? w - 52 : w, 44);
    if (app->export_running) {
        RoundRect(go_rect, 9, RGBA(255, 255, 255, 22));
        RoundRect(MakeRect(go_rect.x, go_rect.y, go_rect.w * (float)Clampd(app->export_progress, 0, 1), go_rect.h), 9, RGBA(255, 255, 255, 46));
        char label[64];
        snprintf(label, sizeof(label), "Exporting… %d%%", (int)(app->export_progress * 100.0));
        TxtC(label, go_rect.x + go_rect.w * 0.5f, go_rect.y + 14, 13, COL_TEXT);
        Rect cancel_rect = MakeRect(x + w - 44, y, 44, 44);
        RoundRect(cancel_rect, 9, COL_FIELD);
        if (IconButton(cancel_rect, ICON_X, 0, "Cancel export")) {
            pluto_export_cancel();
            Toast(app, "Canceling export…");
        }
    } else {
        int hover = Interactive(go_rect);
        RoundRect(go_rect, 9, hover ? RGBA(255, 255, 255, 255) : COL_LIGHT_BUTTON);
        DrawIcon(ICON_DOWNLOAD, go_rect.x + 18, go_rect.y + 13, 18, COL_LIGHT_TEXT);
        char label[96];
        snprintf(label, sizeof(label), "Export %s", app->export_cfg.mode == 0 ? "image" : "video");
        TxtC(label, go_rect.x + go_rect.w * 0.5f + 10, go_rect.y + 14, 14, COL_LIGHT_TEXT);
        if (hover) SetMouseCursor(MOUSE_CURSOR_POINTING_HAND);
        if (Clicked(go_rect)) StartExport(app);
    }
    y += 54;

    char summary[PLUTO_PATH_SIZE + PLUTO_MESSAGE_SIZE + 64];
    if (app->export_running) {
        snprintf(summary, sizeof(summary), "%d×%d · %s · %.1f s timeline",
                 app->export_cfg.width, app->export_cfg.height,
                 app->export_cfg.mode == 0 ? ImageFormatNames[app->export_cfg.image_format] : VideoFormatNames[app->export_cfg.video_format],
                 TimelineDuration(app));
        DrawWrapped(summary, x, y, w, 11, COL_MUTED);
        y += 30;
    } else if (app->export_finished) {
        if (app->export_success) {
            snprintf(summary, sizeof(summary), "Exported to %s", app->export_output);
            DrawWrapped(summary, x, y, w, 11, COL_GREEN);
            y += 34;
        } else {
            snprintf(summary, sizeof(summary), "%s", app->export_canceled ? "Export canceled." : (app->export_message[0] ? app->export_message : "Export failed."));
            DrawWrapped(summary, x, y, w, 11, COL_RED);
            y += 34;
        }
    } else {
        Txt("Image exports render the current frame. Video exports render the whole timeline.", x, y, 11, COL_DIM);
        y += 32;
    }

    SectionLabel("PROJECT", x, y);
    y += 18;
    TxtEllipsis(app->project_name[0] ? app->project_name : "Untitled project", x, y, 12, w, COL_TEXT);
    y += 20;
    char detail[128];
    snprintf(detail, sizeof(detail), "%d clip%s · %.1f s · 1920×1080 · 30 fps",
             app->clip_count, app->clip_count == 1 ? "" : "s", TimelineDuration(app));
    TxtEllipsis(detail, x, y, 11, w, COL_MUTED);

    EndScissorMode();
}

/* ========================================================================== */
/* Application lifecycle                                                      */
/* ========================================================================== */

static void UpdateShortcuts(App *app) {
    if (g_focus) return;
    int ctrl = IsKeyDown(KEY_LEFT_CONTROL) || IsKeyDown(KEY_RIGHT_CONTROL);
    if (ctrl && IsKeyPressed(KEY_Z)) {
        if (IsKeyDown(KEY_LEFT_SHIFT) || IsKeyDown(KEY_RIGHT_SHIFT)) Redo(app);
        else Undo(app);
        BackendSyncAll(app);
        return;
    }
    if (ctrl && IsKeyPressed(KEY_Y)) {
        Redo(app);
        BackendSyncAll(app);
        return;
    }
    if (IsKeyPressed(KEY_SPACE)) app->playing = !app->playing;
    if (IsKeyPressed(KEY_V)) app->tool = TOOL_SELECT;
    if (IsKeyPressed(KEY_H)) app->tool = TOOL_HAND;
    if (IsKeyPressed(KEY_C)) app->tool = TOOL_RAZOR;
    if (IsKeyPressed(KEY_T)) app->tool = TOOL_TEXT;
    if (IsKeyPressed(KEY_LEFT)) app->playhead = Maxd(0, app->playhead - 1.0 / 30.0);
    if (IsKeyPressed(KEY_RIGHT)) app->playhead = Minf((float)app->playhead + 1.0f / 30.0f, (float)TimelineDuration(app));
    if (IsKeyPressed(KEY_HOME)) app->playhead = 0;
    if (IsKeyPressed(KEY_END)) app->playhead = TimelineDuration(app);
    if (IsKeyPressed(KEY_DELETE)) {
        if (app->selected >= 0) RemoveClipAt(app, app->selected);
    }
    if (ctrl && IsKeyPressed(KEY_E)) app->drawer_open = !app->drawer_open;
    if (IsKeyPressed(KEY_G)) app->guides = !app->guides;
    if (IsKeyPressed(KEY_ESCAPE) && app->drawer_open) app->drawer_open = 0;
}

static void UpdateApp(App *app) {
    static double last = 0.0;
    double now = GetTime();
    double dt = last > 0.0 ? now - last : 0.0;
    last = now;
    if (dt > 0.1) dt = 0.1;

    if (app->playing && !g_focus) {
        double duration = TimelineDuration(app);
        app->playhead += dt;
        if (app->playhead >= duration) {
            if (app->loop) app->playhead = 0;
            else {
                app->playhead = duration;
                app->playing = 0;
            }
        }
    }
    BackendFlushProps(app, 0);
    EnsurePreviewTexture(app);
    PollExport(app);
    UpdateShortcuts(app);
    if (app->toast_until > 0 && now > app->toast_until) app->toast[0] = '\0';
}

static void DrawToast(App *app) {
    if (!app->toast[0]) return;
    float width = Minf(app->width - 40, TxtW(app->toast, 12) + 28);
    Rect rect = MakeRect((app->width - width) * 0.5f, 76, width, 30);
    RoundRect(rect, 8, RGBA(28, 30, 36, 245));
    RoundRectBorder(rect, 8, BLANK, 1, COL_BORDER);
    TxtEllipsis(app->toast, rect.x + 14, rect.y + 9, 12, rect.w - 28, COL_TEXT);
}

static void InitExportSettings(App *app) {
    ExportSettings *cfg = &app->export_cfg;
    memset(cfg, 0, sizeof(*cfg));
    cfg->mode = 0;
    cfg->image_format = 0;
    cfg->image_quality = 90;
    cfg->transparent = 0;
    cfg->metadata = 1;
    cfg->color_space = 0;
    cfg->video_format = 0;
    cfg->fps_index = 1;
    cfg->video_quality = 2;
    cfg->include_audio = 1;
    cfg->loop = 1;
    cfg->size_index = 2;
    cfg->width = 1920;
    cfg->height = 1080;
    CopyStr(cfg->name, sizeof(cfg->name), "untitled");
    snprintf(cfg->width_text, sizeof(cfg->width_text), "%d", cfg->width);
    snprintf(cfg->height_text, sizeof(cfg->height_text), "%d", cfg->height);
    app->export_name.buffer = cfg->name;
    app->export_name.cap = (int)sizeof(cfg->name);
    app->export_width.buffer = cfg->width_text;
    app->export_width.cap = (int)sizeof(cfg->width_text);
    app->export_height.buffer = cfg->height_text;
    app->export_height.cap = (int)sizeof(cfg->height_text);
}

static void InitApp(App *app) {
    memset(app, 0, sizeof(*app));
    app->width = GetScreenWidth();
    app->height = GetScreenHeight();
    app->selected = -1;
    app->tab = TAB_TRANSFORM;
    app->tool = TOOL_SELECT;
    app->pps = 24.0f;
    app->snapping = 1;
    app->guides = 1;
    app->trans_open = 1;
    app->library_tab = 0;
    app->light_open = 1;
    app->color_open = 0;
    app->detail_open = 0;
    app->grading_open = 0;
    app->hsl_open = 0;
    app->fx_category[0] = 1;
    app->fx_category[1] = 0;
    app->fx_category[2] = 0;
    app->trans_open = 1;
    for (int i = 0; i < FX_COUNT; i++) app->fx_open[i] = 0;
    CopyStr(app->project_name, sizeof(app->project_name), "Untitled project");
    app->name_field.buffer = app->project_name;
    app->name_field.cap = (int)sizeof(app->project_name);
    app->search_field.buffer = app->search;
    app->search_field.cap = (int)sizeof(app->search);
    InitExportSettings(app);

    if (pluto_bridge_load()) {
        if (pluto_backend_init()) {
            app->backend_ok = 1;
        } else {
            CopyStr(app->backend_error, sizeof(app->backend_error), pluto_last_error());
        }
    } else {
        CopyStr(app->backend_error, sizeof(app->backend_error), pluto_bridge_error());
        Toast(app, "Rust backend not found — build pluto-frontdoor-ffi");
    }
    if (!app->backend_ok) strncpy(app->toast, app->backend_error, sizeof(app->toast) - 1);
    app->toast_until = GetTime() + 6.0;
}

static void ShutdownApp(App *app) {
    ReleasePreviewTextures(app);
    for (int i = 0; i < app->media_count; i++) {
        MediaItem *media = &app->media[i];
        if (media->has_thumbnail) UnloadTexture(media->thumbnail);
        if (media->has_filmstrip) UnloadTexture(media->filmstrip);
        if (media->has_waveform) UnloadTexture(media->waveform);
    }
    UndoClear(app->undo, &app->undo_count);
    UndoClear(app->redo, &app->redo_count);
    pluto_export_cancel();
    pluto_backend_shutdown();
    pluto_bridge_unload();
}

int main(void) {
    SetConfigFlags(FLAG_WINDOW_HIGHDPI | FLAG_MSAA_4X_HINT | FLAG_VSYNC_HINT);
    InitWindow(1440, 900, "Pluto");
    SetWindowMinSize(1100, 700);
    SetTargetFPS(60);
    SetExitKey(KEY_NULL);

    g_ui_font = LoadBestFont(32);
    g_title_font = g_ui_font;

    InitApp(&g_app);

    while (!WindowShouldClose() && !g_app.should_close) {
        g_app.width = GetScreenWidth();
        g_app.height = GetScreenHeight();
        g_mouse = GetMousePosition();
        g_mouse_pressed = IsMouseButtonPressed(MOUSE_BUTTON_LEFT);
        g_mouse_released = IsMouseButtonReleased(MOUSE_BUTTON_LEFT);
        g_click_consumed = 0;

        UpdateApp(&g_app);
        Layout layout = ComputeLayout(&g_app);

        BeginDrawing();
        ClearBackground(COL_BG);

        g_block_ui = g_app.drawer_open ? 1 : 0;
        TopBar(&g_app, layout);
        LibraryPanel(&g_app, layout);
        PreviewPanel(&g_app, layout);
        InspectorPanel(&g_app, layout);
        TimelinePanel(&g_app, layout);

        g_block_ui = 0;
        if (g_app.drawer_open) ExportDrawer(&g_app, layout);
        DrawToast(&g_app);

        /* Release an unfocused text field when clicking empty canvas. */
        if (g_mouse_pressed && g_focus && !RectHit(g_focus->rect, g_mouse)) {
            g_focus = NULL;
        }

        EndDrawing();
    }

    ShutdownApp(&g_app);
    UnloadFont(g_ui_font);
    CloseWindow();
    return 0;
}
