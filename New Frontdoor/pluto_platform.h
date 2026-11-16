/*
 * pluto_platform.h — Small native helpers that Raylib does not provide.
 */
#ifndef PLUTO_PLATFORM_H
#define PLUTO_PLATFORM_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Returns 1 when a file was selected, 0 when the dialog was cancelled and -1
 * when file dialogs are not available on this platform. */
int pluto_platform_open_media(char *out_path, size_t cap);

/* Returns 1 when a destination was selected, 0 when cancelled and -1 when
 * file dialogs are not available. */
int pluto_platform_save_file(const char *title, const char *default_name,
                             const char *filter_label, const char *filter_extension,
                             char *out_path, size_t cap);

#ifdef __cplusplus
}
#endif

#endif /* PLUTO_PLATFORM_H */
