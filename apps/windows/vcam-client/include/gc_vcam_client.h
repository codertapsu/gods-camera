// The app's side of the camera: the pipe the media source fetches pictures from.
//
// The app makes one server at start-up. While nothing reads the camera the
// server costs nothing; `gc_vcam_server_watched` says when an app has started
// reading and which size it asked for, and from then on the app sends its
// pictures with `gc_vcam_server_send`. Sending never waits: the picture is
// copied and a thread of the server's own writes it. If the next picture
// comes before the last one is written, the older one is dropped.
//
// Plain C, so it can be called from C++, C# and Rust alike.
#pragma once

#include <stdint.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct gc_vcam_server gc_vcam_server;

// What the reading side asked for.
typedef struct gc_vcam_wanted {
    uint32_t width;
    uint32_t height;
    uint32_t format;          // GC_VCAM_FOURCC_NV12
    uint32_t frame_rate_num;
    uint32_t frame_rate_den;
} gc_vcam_wanted;

typedef struct gc_vcam_stats {
    uint64_t frames_sent;     // pictures written to the source
    uint64_t frames_dropped;  // pictures replaced by a newer one before they were written
    uint64_t connects;        // times a source connected
    uint64_t write_failures;  // writes that failed or took too long; the source is then cut off
} gc_vcam_stats;

// Make the server and its pipe for the user this process runs as.
// Returns 0, or a Windows error code: ERROR_ACCESS_DENIED or ERROR_PIPE_BUSY
// when another program already holds the pipe's name.
uint32_t gc_vcam_server_create(gc_vcam_server** server);

// Stop the thread, close the pipe and free everything. Accepts NULL.
void gc_vcam_server_destroy(gc_vcam_server* server);

// The pipe's name. The app hands it to the virtual camera as the attribute
// GC_VCAM_ATTRIBUTE_PIPE_NAME before starting the camera.
const wchar_t* gc_vcam_server_pipe_name(const gc_vcam_server* server);

// 1 while a source is connected and has said what it wants; `wanted` is then
// filled in if it is not NULL. 0 otherwise: nothing is reading the camera.
int gc_vcam_server_watched(const gc_vcam_server* server, gc_vcam_wanted* wanted);

// Hand over one NV12 picture: `width` bytes a row of luma, `height` rows,
// then `height / 2` rows of interleaved chroma, with no padding.
// `time_100ns` is when it was made, from gc_vcam_now_100ns().
// Returns 1 if it was taken, 0 if nothing is watching or the size is not sensible.
int gc_vcam_server_send(gc_vcam_server* server, const uint8_t* nv12, uint32_t width, uint32_t height, int64_t time_100ns);

void gc_vcam_server_get_stats(const gc_vcam_server* server, gc_vcam_stats* stats);

#ifdef __cplusplus
}
#endif
