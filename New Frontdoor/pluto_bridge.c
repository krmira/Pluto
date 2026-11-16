/*
 * pluto_bridge.c — Runtime loader for the Pluto Rust backend.
 *
 * Keeping the backend behind LoadLibrary/dlopen means the native C frontend can
 * be built with any C toolchain (MSVC, MinGW, clang) without matching the Rust
 * toolchain's CRT or producing an import library.
 */
#include "pluto_bridge.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
typedef HMODULE lib_handle;
static lib_handle backend_lib = NULL;
#define PLUTO_LIB_NAME "pluto_frontdoor_ffi.dll"
static void *open_library(const char *path) { return (void *)LoadLibraryA(path); }
static void *find_symbol(void *library, const char *name) { return (void *)GetProcAddress((HMODULE)library, name); }
static void close_library(void *library) { FreeLibrary((HMODULE)library); }
#else
#include <dlfcn.h>
#include <unistd.h>
typedef void *lib_handle;
static lib_handle backend_lib = NULL;
#ifdef __APPLE__
#define PLUTO_LIB_NAME "libpluto_frontdoor_ffi.dylib"
#else
#define PLUTO_LIB_NAME "libpluto_frontdoor_ffi.so"
#endif
static void *open_library(const char *path) { return dlopen(path, RTLD_NOW | RTLD_LOCAL); }
static void *find_symbol(void *library, const char *name) { return dlsym(library, name); }
static void close_library(void *library) { dlclose(library); }
#endif

static char bridge_error[PLUTO_PATH_SIZE * 4];
static int bridge_attempted = 0;

/* --- Backend symbol table ------------------------------------------------ */

typedef int (*fn_int_void)(void);
typedef void (*fn_void_void)(void);
typedef const char *(*fn_cstr_void)(void);
typedef int (*fn_int_i)(int);
typedef int (*fn_int_ii)(int, int);
typedef double (*fn_double_void)(void);
typedef int (*fn_int_cstr)(const char *);
typedef int (*fn_int_cstr_cstr)(const char *, const char *);
typedef int (*fn_int_cstr_cstr_double)(const char *, const char *, double);
typedef int (*fn_int_cstr_int_double)(const char *, int, double);
typedef int (*fn_int_index_info)(int, PlutoClipInfo *);
typedef int (*fn_int_info_cstr)(const PlutoClipInfo *, const char *);
typedef int (*fn_int_cstr_double_cstr_double_double)(const char *, double, const char *, double, double);
typedef int (*fn_int_cstr_double_double)(const char *, double, double);
typedef int (*fn_int_cstr_double_cstr)(const char *, double, const char *);
typedef int (*fn_int_media)(const char *, PlutoMediaInfo *);
typedef int (*fn_int_inspect)(const char *, PlutoMediaInfo *, unsigned char **, int *, unsigned char **, int *, unsigned char **, int *);
typedef int (*fn_int_frame)(const char *, double, int, int, unsigned char **, int *);
typedef void (*fn_void_buffer)(unsigned char *, int);
typedef int (*fn_int_export_start)(const char *, double, const char *);
typedef int (*fn_int_status)(PlutoExportStatus *);

static fn_int_void p_pluto_backend_init;
static fn_void_void p_pluto_backend_shutdown;
static fn_cstr_void p_pluto_last_error;
static fn_int_void p_pluto_timeline_reset;
static fn_int_void p_pluto_timeline_count;
static fn_double_void p_pluto_timeline_duration;
static fn_int_void p_pluto_timeline_snapping;
static fn_int_index_info p_pluto_timeline_get_clip;
static fn_int_info_cstr p_pluto_timeline_add_clip;
static fn_int_cstr_cstr p_pluto_timeline_set_props;
static fn_int_cstr_cstr_double p_pluto_timeline_set_transition;
static fn_int_cstr_double_cstr_double_double p_pluto_timeline_move_clip;
static fn_int_cstr_double_double p_pluto_timeline_trim_clip;
static fn_int_cstr_double_cstr p_pluto_timeline_split_clip;
static fn_int_cstr p_pluto_timeline_remove_clip;
static fn_int_i p_pluto_timeline_set_snapping;
static fn_int_media p_pluto_media_probe;
static fn_int_inspect p_pluto_media_inspect;
static fn_int_frame p_pluto_media_frame;
static fn_void_buffer p_pluto_buffer_free;
static fn_int_export_start p_pluto_export_start;
static fn_int_status p_pluto_export_status;
static fn_int_void p_pluto_export_running;
static fn_int_void p_pluto_export_cancel;
static fn_int_void p_pluto_export_reset;

static int resolve_symbols(void *library) {
    int missing = 0;
#define RESOLVE(name)                                                          \
    do {                                                                       \
        *(void **)(&p_##name) = find_symbol(library, #name);                   \
        if (!p_##name) { missing++; }                                          \
    } while (0)
    RESOLVE(pluto_backend_init);
    RESOLVE(pluto_backend_shutdown);
    RESOLVE(pluto_last_error);
    RESOLVE(pluto_timeline_reset);
    RESOLVE(pluto_timeline_count);
    RESOLVE(pluto_timeline_duration);
    RESOLVE(pluto_timeline_snapping);
    RESOLVE(pluto_timeline_get_clip);
    RESOLVE(pluto_timeline_add_clip);
    RESOLVE(pluto_timeline_set_props);
    RESOLVE(pluto_timeline_set_transition);
    RESOLVE(pluto_timeline_move_clip);
    RESOLVE(pluto_timeline_trim_clip);
    RESOLVE(pluto_timeline_split_clip);
    RESOLVE(pluto_timeline_remove_clip);
    RESOLVE(pluto_timeline_set_snapping);
    RESOLVE(pluto_media_probe);
    RESOLVE(pluto_media_inspect);
    RESOLVE(pluto_media_frame);
    RESOLVE(pluto_buffer_free);
    RESOLVE(pluto_export_start);
    RESOLVE(pluto_export_status);
    RESOLVE(pluto_export_running);
    RESOLVE(pluto_export_cancel);
    RESOLVE(pluto_export_reset);
#undef RESOLVE
    return missing == 0;
}

static int add_candidate(char *buffer, size_t cap, const char *directory, const char *name) {
    if (!directory || !directory[0]) {
        return snprintf(buffer, cap, "%s", name);
    }
    size_t length = strlen(directory);
    if (length > 0 && (directory[length - 1] == '\\' || directory[length - 1] == '/')) {
        return snprintf(buffer, cap, "%s%s", directory, name);
    }
#ifdef _WIN32
    return snprintf(buffer, cap, "%s\\%s", directory, name);
#else
    return snprintf(buffer, cap, "%s/%s", directory, name);
#endif
}

int pluto_bridge_load(void) {
    char executable_dir[PLUTO_PATH_SIZE] = {0};
    char candidate[PLUTO_PATH_SIZE * 2];
    char tried[PLUTO_PATH_SIZE * 2] = {0};
    void *library = NULL;
    const char *env_path;
    bridge_attempted = 1;

    if (backend_lib) {
        return 1;
    }

    env_path = getenv("PLUTO_BACKEND_DLL");
    if (env_path && env_path[0]) {
        library = open_library(env_path);
        if (!library) {
            snprintf(tried, sizeof(tried), "PLUTO_BACKEND_DLL=%s", env_path);
        }
    }

#ifdef _WIN32
    if (!library) {
        char module_path[PLUTO_PATH_SIZE];
        DWORD length = GetModuleFileNameA(NULL, module_path, (DWORD)sizeof(module_path));
        if (length > 0 && length < sizeof(module_path)) {
            char *slash = strrchr(module_path, '\\');
            if (!slash) slash = strrchr(module_path, '/');
            if (slash) *slash = '\0';
            snprintf(executable_dir, sizeof(executable_dir), "%s", module_path);
        }
    }
#else
    {
        char module_path[PLUTO_PATH_SIZE];
        ssize_t length = readlink("/proc/self/exe", module_path, sizeof(module_path) - 1);
        if (length > 0) {
            module_path[length] = '\0';
            char *slash = strrchr(module_path, '/');
            if (slash) *slash = '\0';
            snprintf(executable_dir, sizeof(executable_dir), "%s", module_path);
        }
    }
#endif

    if (!library && executable_dir[0]) {
        add_candidate(candidate, sizeof(candidate), executable_dir, PLUTO_LIB_NAME);
        library = open_library(candidate);
        if (!library) {
            snprintf(tried, sizeof(tried), "%s", candidate);
        }
    }
    if (!library) {
        /* Development fallbacks: run from the repository root or src folder. */
        static const char *fallbacks[] = {
            "pluto_frontdoor_ffi.dll",
            "Backdoor/target/release/pluto_frontdoor_ffi.dll",
            "Backdoor/target/debug/pluto_frontdoor_ffi.dll",
            "Backdoor/target/x86_64-pc-windows-gnu/release/pluto_frontdoor_ffi.dll",
            "Backdoor/target/x86_64-pc-windows-gnu/debug/pluto_frontdoor_ffi.dll",
            "../Backdoor/target/release/pluto_frontdoor_ffi.dll",
            "../Backdoor/target/debug/pluto_frontdoor_ffi.dll",
            "../Backdoor/target/x86_64-pc-windows-gnu/release/pluto_frontdoor_ffi.dll",
            "../Backdoor/target/x86_64-pc-windows-gnu/debug/pluto_frontdoor_ffi.dll",
            "libpluto_frontdoor_ffi.so",
            "../Backdoor/target/release/libpluto_frontdoor_ffi.so",
            "../Backdoor/target/debug/libpluto_frontdoor_ffi.so",
            "libpluto_frontdoor_ffi.dylib",
            "../Backdoor/target/release/libpluto_frontdoor_ffi.dylib",
            NULL,
        };
        int index = 0;
        while (!library && fallbacks[index]) {
#ifdef _WIN32
            if (strstr(fallbacks[index], ".dll")) {
                library = open_library(fallbacks[index]);
            }
#else
            if (!strstr(fallbacks[index], ".dll")) {
                library = open_library(fallbacks[index]);
            }
#endif
            if (!library) {
                snprintf(tried, sizeof(tried), "%s", fallbacks[index]);
            }
            index++;
        }
    }

    if (!library) {
        snprintf(bridge_error, sizeof(bridge_error),
                 "Pluto backend library was not found.\n"
                 "Build it with: cargo build --release -p pluto-frontdoor-ffi\n"
                 "Last path tried: %s", tried[0] ? tried : PLUTO_LIB_NAME);
        return 0;
    }
    if (!resolve_symbols(library)) {
        close_library(library);
        snprintf(bridge_error, sizeof(bridge_error),
                 "The Pluto backend loaded but is missing required functions. Rebuild pluto-frontdoor-ffi.");
        return 0;
    }
    backend_lib = (lib_handle)library;
    bridge_error[0] = '\0';
    return 1;
}

int pluto_bridge_available(void) {
    return backend_lib != NULL;
}

const char *pluto_bridge_error(void) {
    if (!bridge_attempted && !backend_lib) {
        return "The Pluto backend has not been loaded yet.";
    }
    if (!bridge_error[0]) {
        return "No backend error.";
    }
    return bridge_error;
}

void pluto_bridge_unload(void) {
    if (backend_lib) {
        close_library((void *)backend_lib);
        backend_lib = NULL;
    }
}

/* --- Wrappers ------------------------------------------------------------ */

const char *pluto_last_error(void) {
    if (p_pluto_last_error) return p_pluto_last_error();
    return pluto_bridge_error();
}

int pluto_backend_init(void) { return p_pluto_backend_init ? p_pluto_backend_init() : 0; }
void pluto_backend_shutdown(void) { if (p_pluto_backend_shutdown) p_pluto_backend_shutdown(); }

int pluto_timeline_reset(void) { return p_pluto_timeline_reset ? p_pluto_timeline_reset() : 0; }
int pluto_timeline_count(void) { return p_pluto_timeline_count ? p_pluto_timeline_count() : 0; }
double pluto_timeline_duration(void) { return p_pluto_timeline_duration ? p_pluto_timeline_duration() : 0.0; }
int pluto_timeline_snapping(void) { return p_pluto_timeline_snapping ? p_pluto_timeline_snapping() : 1; }
int pluto_timeline_get_clip(int index, PlutoClipInfo *out) { return p_pluto_timeline_get_clip ? p_pluto_timeline_get_clip(index, out) : 0; }
int pluto_timeline_add_clip(const PlutoClipInfo *info, const char *props_json) { return p_pluto_timeline_add_clip ? p_pluto_timeline_add_clip(info, props_json) : 0; }
int pluto_timeline_set_props(const char *id, const char *props_json) { return p_pluto_timeline_set_props ? p_pluto_timeline_set_props(id, props_json) : 0; }
int pluto_timeline_set_transition(const char *id, const char *type, double dur) { return p_pluto_timeline_set_transition ? p_pluto_timeline_set_transition(id, type, dur) : 0; }
int pluto_timeline_move_clip(const char *id, double start, const char *track, double playhead, double snap_threshold) {
    return p_pluto_timeline_move_clip ? p_pluto_timeline_move_clip(id, start, track, playhead, snap_threshold) : 0;
}
int pluto_timeline_trim_clip(const char *id, double start, double end) { return p_pluto_timeline_trim_clip ? p_pluto_timeline_trim_clip(id, start, end) : 0; }
int pluto_timeline_split_clip(const char *id, double at, const char *right_id) { return p_pluto_timeline_split_clip ? p_pluto_timeline_split_clip(id, at, right_id) : 0; }
int pluto_timeline_remove_clip(const char *id) { return p_pluto_timeline_remove_clip ? p_pluto_timeline_remove_clip(id) : 0; }
int pluto_timeline_set_snapping(int enabled) { return p_pluto_timeline_set_snapping ? p_pluto_timeline_set_snapping(enabled) : 0; }

int pluto_media_probe(const char *path, PlutoMediaInfo *info) { return p_pluto_media_probe ? p_pluto_media_probe(path, info) : 0; }
int pluto_media_inspect(const char *path, PlutoMediaInfo *info,
                        unsigned char **thumbnail, int *thumbnail_len,
                        unsigned char **filmstrip, int *filmstrip_len,
                        unsigned char **waveform, int *waveform_len) {
    if (!p_pluto_media_inspect) {
        if (thumbnail) *thumbnail = NULL;
        if (thumbnail_len) *thumbnail_len = 0;
        if (filmstrip) *filmstrip = NULL;
        if (filmstrip_len) *filmstrip_len = 0;
        if (waveform) *waveform = NULL;
        if (waveform_len) *waveform_len = 0;
        return 0;
    }
    return p_pluto_media_inspect(path, info, thumbnail, thumbnail_len, filmstrip, filmstrip_len, waveform, waveform_len);
}
int pluto_media_frame(const char *path, double time, int width, int height, unsigned char **data, int *len) {
    if (!p_pluto_media_frame) {
        if (data) *data = NULL;
        if (len) *len = 0;
        return 0;
    }
    return p_pluto_media_frame(path, time, width, height, data, len);
}
void pluto_buffer_free(unsigned char *data, int len) { if (p_pluto_buffer_free) p_pluto_buffer_free(data, len); }

int pluto_export_start(const char *settings_json, double time, const char *output_path) {
    return p_pluto_export_start ? p_pluto_export_start(settings_json, time, output_path) : 0;
}
int pluto_export_status(PlutoExportStatus *out) {
    if (!p_pluto_export_status) {
        if (out) memset(out, 0, sizeof(*out));
        return 0;
    }
    return p_pluto_export_status(out);
}
int pluto_export_running(void) { return p_pluto_export_running ? p_pluto_export_running() : 0; }
int pluto_export_cancel(void) { return p_pluto_export_cancel ? p_pluto_export_cancel() : 0; }
int pluto_export_reset(void) { return p_pluto_export_reset ? p_pluto_export_reset() : 0; }
