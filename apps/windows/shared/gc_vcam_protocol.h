// The contract between the God's Camera app and its camera's media source.
//
// The media source is a COM object that Windows loads into the camera service
// (Frame Server), which runs as LOCAL SERVICE in session 0. The app runs as
// the user. Pictures cross from one to the other through a named pipe that
// the app owns:
//
//   app (pipe server)  <----  request: which size is wanted  ----  media source (pipe client)
//   app (pipe server)  ----  frame header + NV12 bytes, again and again  ---->  media source
//
// The source connects when an app starts reading the camera and disconnects
// when the last one stops, so a connected pipe is also how the app knows that
// something is watching.
//
// Everything here is plain C, so the app's side can be reached from Rust too.
#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The class Windows creates in the camera service. It is registered for the
// whole machine under HKLM\SOFTWARE\Classes\CLSID.
#define GC_VCAM_CLSID_STRING L"{1E1EB430-08A7-42F4-9F7D-C53A00B36024}"

// A string attribute the app sets on the virtual camera before starting it:
// the name of the pipe to fetch pictures from. Without it the source looks
// for the pipe of the user it runs as, which is right when a test loads the
// source into its own process and wrong inside the camera service.
#define GC_VCAM_ATTRIBUTE_PIPE_NAME_STRING L"{94A0A41C-B68F-4E28-98AA-B471A72D5025}"

// Attributes the source puts on every sample. They reach a reader in the same
// process; the camera service does not pass them on to other apps.
// UINT64: the frame's sequence number as the app sent it; 0 for a slate.
#define GC_VCAM_SAMPLE_SEQUENCE_STRING L"{86C72A67-9AC2-4973-ACE5-EA51761B53D6}"
// UINT32: one of GC_VCAM_SAMPLE_*.
#define GC_VCAM_SAMPLE_KIND_STRING L"{57360040-62F0-4AB0-BC1C-B33E44317EF5}"
#define GC_VCAM_SAMPLE_FRAME 0u   // a picture from the app, new
#define GC_VCAM_SAMPLE_REPEAT 1u  // the app's last picture again, because the next one is late
#define GC_VCAM_SAMPLE_SLATE 2u   // the source's own picture: the app is not sending

// The name people see in a camera list.
#define GC_VCAM_FRIENDLY_NAME L"God's Camera"

// The pipe's name is this prefix followed by the user's SID as a string, so
// each user of the machine has a pipe of their own.
#define GC_VCAM_PIPE_PREFIX L"\\\\.\\pipe\\godscamera.frames."

#define GC_VCAM_MAGIC_REQUEST 0x52564347u  // "GCVR"
#define GC_VCAM_MAGIC_FRAME 0x46564347u    // "GCVF"
#define GC_VCAM_PROTOCOL_VERSION 1u
#define GC_VCAM_FOURCC_NV12 0x3231564Eu    // "NV12"

// The largest picture either side accepts: 4K.
#define GC_VCAM_MAX_WIDTH 3840u
#define GC_VCAM_MAX_HEIGHT 2160u

// How long the source waits for the next picture before it shows its slate, and
// how long after starting it waits for the first one.
#define GC_VCAM_SLATE_AFTER_MS 500
#define GC_VCAM_FIRST_FRAME_GRACE_MS 300

#pragma pack(push, 1)

// Source to app, once after connecting and again whenever the wanted size changes.
typedef struct gc_vcam_request {
    uint32_t magic;           // GC_VCAM_MAGIC_REQUEST
    uint16_t version;         // GC_VCAM_PROTOCOL_VERSION
    uint16_t size;            // sizeof(gc_vcam_request); later versions may be longer
    uint32_t width;
    uint32_t height;
    uint32_t format;          // GC_VCAM_FOURCC_NV12
    uint32_t frame_rate_num;  // frames a second, as a fraction
    uint32_t frame_rate_den;
} gc_vcam_request;

// App to source, before every picture. The picture's bytes follow at once:
// the luma plane, `width` bytes a row, then the interleaved chroma plane.
typedef struct gc_vcam_frame_header {
    uint32_t magic;         // GC_VCAM_MAGIC_FRAME
    uint16_t version;       // GC_VCAM_PROTOCOL_VERSION
    uint16_t header_size;   // sizeof(gc_vcam_frame_header); later versions may be longer
    uint32_t width;
    uint32_t height;
    uint32_t format;        // GC_VCAM_FOURCC_NV12
    uint32_t payload_size;  // bytes that follow: width * height * 3 / 2 for NV12
    uint64_t sequence;      // goes up by one with every picture the app makes; never 0
    int64_t time_100ns;     // when the picture was made: gc_vcam_now_100ns() in the app
} gc_vcam_frame_header;

#pragma pack(pop)

// Bytes in one NV12 picture.
static inline uint32_t gc_vcam_nv12_size(uint32_t width, uint32_t height) {
    return width * height + (width * height) / 2u;
}

#ifdef _WIN32
// The time both sides stamp and compare: the performance counter in units of
// 100 nanoseconds, which is also the clock Media Foundation's samples use.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
static inline int64_t gc_vcam_now_100ns(void) {
    LARGE_INTEGER counter, frequency;
    QueryPerformanceCounter(&counter);
    QueryPerformanceFrequency(&frequency);
    // Whole seconds and the rest are scaled apart, so nothing overflows.
    return (counter.QuadPart / frequency.QuadPart) * 10000000ll
        + (counter.QuadPart % frequency.QuadPart) * 10000000ll / frequency.QuadPart;
}
#endif

#ifdef __cplusplus
}
#endif
