/*
 * pluto_platform.c — Native open/save dialogs for the C frontend.
 *
 * Windows uses the common dialogs from comdlg32.  Other platforms report that
 * dialogs are unavailable so the caller can surface a helpful message; timeline
 * editing and export still work when a path is supplied another way.
 */
#include "pluto_platform.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PLUTO_PATH_W 1024

#ifdef _WIN32
#include <windows.h>
#include <commdlg.h>

static int utf8_to_wide(const char *input, wchar_t *output, int output_count) {
    return MultiByteToWideChar(CP_UTF8, 0, input, -1, output, output_count);
}

static int wide_to_utf8(const wchar_t *input, char *output, int output_count) {
    return WideCharToMultiByte(CP_UTF8, 0, input, -1, output, output_count, NULL, NULL);
}

static void build_filter(const char *label, const char *extension, wchar_t *out, size_t out_count) {
    wchar_t wide_label[128];
    wchar_t wide_pattern[128];
    char pattern[128];
    size_t index = 0;

    if (!label || !label[0]) label = "Media";
    if (!extension || !extension[0] || strcmp(extension, "*") == 0) {
        snprintf(pattern, sizeof(pattern), "*.*");
    } else if (strchr(extension, ';')) {
        snprintf(pattern, sizeof(pattern), "%s", extension);
    } else {
        snprintf(pattern, sizeof(pattern), "*.%s", extension);
    }
    utf8_to_wide(label, wide_label, (int)(sizeof(wide_label) / sizeof(wchar_t)));
    utf8_to_wide(pattern, wide_pattern, (int)(sizeof(wide_pattern) / sizeof(wchar_t)));

    /* "Label\0pattern\0\0" */
    while (wide_label[index] && index + 1 < out_count) { out[index] = wide_label[index]; index++; }
    out[index++] = L'\0';
    {
        size_t p = 0;
        while (wide_pattern[p] && index + 1 < out_count) { out[index++] = wide_pattern[p++]; }
    }
    out[index++] = L'\0';
    if (index < out_count) out[index] = L'\0';
}

int pluto_platform_open_media(char *out_path, size_t cap) {
    OPENFILENAMEW dialog;
    wchar_t file[PLUTO_PATH_W] = {0};
    wchar_t filter[512];
    wchar_t title[128];

    if (!out_path || cap == 0) return -1;
    build_filter("Media files", "mp4;mov;mkv;avi;webm;m4v;mp3;wav;m4a;aac;flac;ogg;png;jpg;jpeg;webp;gif", filter, sizeof(filter) / sizeof(wchar_t));
    utf8_to_wide("Import media", title, (int)(sizeof(title) / sizeof(wchar_t)));

    memset(&dialog, 0, sizeof(dialog));
    dialog.lStructSize = sizeof(dialog);
    dialog.hwndOwner = GetActiveWindow();
    dialog.lpstrFilter = filter;
    dialog.lpstrFile = file;
    dialog.nMaxFile = PLUTO_PATH_W;
    dialog.lpstrTitle = title;
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    if (!GetOpenFileNameW(&dialog)) return 0;
    if (!wide_to_utf8(file, out_path, (int)cap)) return 0;
    return 1;
}

int pluto_platform_save_file(const char *title, const char *default_name,
                             const char *filter_label, const char *filter_extension,
                             char *out_path, size_t cap) {
    OPENFILENAMEW dialog;
    wchar_t file[PLUTO_PATH_W] = {0};
    wchar_t filter[512];
    wchar_t wide_title[128];

    if (!out_path || cap == 0) return -1;
    build_filter(filter_label && filter_label[0] ? filter_label : "Export", filter_extension && filter_extension[0] ? filter_extension : "*", filter, sizeof(filter) / sizeof(wchar_t));
    utf8_to_wide(title && title[0] ? title : "Export", wide_title, (int)(sizeof(wide_title) / sizeof(wchar_t)));
    if (default_name && default_name[0]) {
        utf8_to_wide(default_name, file, PLUTO_PATH_W);
    }

    memset(&dialog, 0, sizeof(dialog));
    dialog.lStructSize = sizeof(dialog);
    dialog.hwndOwner = GetActiveWindow();
    dialog.lpstrFilter = filter;
    dialog.lpstrFile = file;
    dialog.nMaxFile = PLUTO_PATH_W;
    dialog.lpstrTitle = wide_title;
    dialog.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    if (!GetSaveFileNameW(&dialog)) return 0;
    if (!wide_to_utf8(file, out_path, (int)cap)) return 0;
    return 1;
}

#else

int pluto_platform_open_media(char *out_path, size_t cap) {
    (void)out_path;
    (void)cap;
    return -1;
}

int pluto_platform_save_file(const char *title, const char *default_name,
                             const char *filter_label, const char *filter_extension,
                             char *out_path, size_t cap) {
    (void)title;
    (void)default_name;
    (void)filter_label;
    (void)filter_extension;
    (void)out_path;
    (void)cap;
    return -1;
}

#endif
