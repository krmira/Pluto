/*
 * pluto_bridge.h — C interface to the Pluto Rust backend.
 *
 * The backend is built as a shared library (pluto_frontdoor_ffi.dll on
 * Windows, libpluto_frontdoor_ffi.so on Linux/macOS) and loaded at runtime by
 * pluto_bridge.c.  No import library or link-time Rust dependency is required.
 *
 * All strings are UTF-8.  Buffers returned through `unsigned char**` are owned
 * by the backend and must be released with `pluto_buffer_free`.
 */
#ifndef PLUTO_BRIDGE_H
#define PLUTO_BRIDGE_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PLUTO_ID_SIZE 64
#define PLUTO_TRACK_SIZE 16
#define PLUTO_LABEL_SIZE 128
#define PLUTO_PATH_SIZE 1024
#define PLUTO_KIND_SIZE 16
#define PLUTO_THEME_SIZE 24
#define PLUTO_TRANS_SIZE 16
#define PLUTO_MESSAGE_SIZE 512

typedef struct PlutoClipInfo {
    char id[PLUTO_ID_SIZE];
    char track[PLUTO_TRACK_SIZE];
    double start;
    double dur;
    char label[PLUTO_LABEL_SIZE];
    char linked_audio_id[PLUTO_ID_SIZE];
    char path[PLUTO_PATH_SIZE];
    char media_kind[PLUTO_KIND_SIZE];
    char theme[PLUTO_THEME_SIZE];
    int has_audio;
    double source_offset;
    double source_duration;
    char trans_type[PLUTO_TRANS_SIZE];
    double trans_dur;
} PlutoClipInfo;

typedef struct PlutoMediaInfo {
    char name[256];
    char path[PLUTO_PATH_SIZE];
    char kind[PLUTO_KIND_SIZE];
    double duration;
    int has_audio;
} PlutoMediaInfo;

typedef struct PlutoExportStatus {
    int running;
    int finished;
    int success;
    int canceled;
    double progress;
    char message[PLUTO_MESSAGE_SIZE];
    char output[PLUTO_PATH_SIZE];
} PlutoExportStatus;

/* --- Dynamic loader ------------------------------------------------------ */

/* Loads the backend next to the executable, then from a few development
 * fallback locations.  Safe to call more than once. */
int pluto_bridge_load(void);
int pluto_bridge_available(void);
const char *pluto_bridge_error(void);
void pluto_bridge_unload(void);

/* --- Diagnostics --------------------------------------------------------- */

const char *pluto_last_error(void);

/* --- Lifecycle ----------------------------------------------------------- */

int pluto_backend_init(void);
void pluto_backend_shutdown(void);

/* --- Timeline ------------------------------------------------------------ */

int pluto_timeline_reset(void);
int pluto_timeline_count(void);
double pluto_timeline_duration(void);
int pluto_timeline_snapping(void);
int pluto_timeline_get_clip(int index, PlutoClipInfo *out);
int pluto_timeline_add_clip(const PlutoClipInfo *info, const char *props_json);
int pluto_timeline_set_props(const char *id, const char *props_json);
int pluto_timeline_set_transition(const char *id, const char *type, double dur);
int pluto_timeline_move_clip(const char *id, double start, const char *track, double playhead, double snap_threshold);
int pluto_timeline_trim_clip(const char *id, double start, double end);
int pluto_timeline_split_clip(const char *id, double at, const char *right_id);
int pluto_timeline_remove_clip(const char *id);
int pluto_timeline_set_snapping(int enabled);

/* --- Media --------------------------------------------------------------- */

int pluto_media_probe(const char *path, PlutoMediaInfo *info);
int pluto_media_inspect(const char *path, PlutoMediaInfo *info,
                        unsigned char **thumbnail, int *thumbnail_len,
                        unsigned char **filmstrip, int *filmstrip_len,
                        unsigned char **waveform, int *waveform_len);
int pluto_media_frame(const char *path, double time, int width, int height,
                      unsigned char **data, int *len);
void pluto_buffer_free(unsigned char *data, int len);

/* --- Export -------------------------------------------------------------- */

int pluto_export_start(const char *settings_json, double time, const char *output_path);
int pluto_export_status(PlutoExportStatus *out);
int pluto_export_running(void);
int pluto_export_cancel(void);
int pluto_export_reset(void);

#ifdef __cplusplus
}
#endif

#endif /* PLUTO_BRIDGE_H */
