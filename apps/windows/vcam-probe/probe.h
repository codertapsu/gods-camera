// What the two halves of gc-vcam-probe share.
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <unknwn.h>
#include <windows.h>

#include <ole2.h>

// With this, the GUIDs the next headers declare are also defined here.
#include <initguid.h>

#include <ks.h>
#include <ksmedia.h>
#include <ksproxy.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mfobjects.h>
#include <mfreadwrite.h>

#include <wrl/client.h>

#include <cstdint>
#include <string>
#include <vector>

namespace probe {

// Add formatted text to a string. For building the JSON line.
void Append(std::string* out, const char* format, ...);

// What a media type says about the pictures it describes.
struct TypeInfo {
    uint32_t width = 0;
    uint32_t height = 0;
    GUID subtype{};
    double fps = 0;
    LONG stride = 0;
};

TypeInfo Describe(IMFMediaType* type);

// A stretch of samples of one kind, for the timeline.
struct Stretch {
    const char* kind = "";
    int why = -1;  // for a slate: what its row of squares says, packed; -1 if it has none
    double from_ms = 0;
    double to_ms = 0;
    uint64_t samples = 0;
    uint32_t first_number = 0;
    uint32_t last_number = 0;
};

// What the probe has seen so far.
struct Findings {
    uint64_t samples = 0;
    uint64_t ticks = 0;  // answers without a picture
    uint64_t new_frames = 0;
    uint64_t repeats = 0;
    uint64_t slates = 0;
    uint64_t others = 0;
    uint64_t gap_events = 0;
    uint64_t missing = 0;
    uint64_t restarts = 0;
    bool have_number = false;
    uint32_t first_number = 0;
    uint32_t last_number = 0;
    double first_sample_ms = -1;
    int64_t last_arrival = 0;
    std::vector<double> delays_ms;     // from the making of a picture to its arrival here, new pictures only
    std::vector<double> intervals_ms;  // from one sample to the next
    std::vector<Stretch> timeline;
    uint64_t colours_checked = 0;  // pictures of the host's whose colour bars were looked at
    uint64_t colours_wrong = 0;    // and those where a bar had the wrong colour
    // What the source itself says of each sample. Only an in-process reader sees it.
    uint64_t said_frame = 0;
    uint64_t said_repeat = 0;
    uint64_t said_slate = 0;
};

// Examine a locked, whole picture whose rows are `pitch` bytes apart: which
// of the host's frames it is, or the slate, or neither.
void ExamineLocked(
    Findings* findings, const BYTE* first_row, LONG pitch, const TypeInfo& type, int64_t arrival, int64_t began,
    std::vector<uint8_t>* scratch);

// Load the media source's DLL and make the source as COM would, without the
// registry. If `pipe_attribute` is not empty it is set on the activation
// object first, which is how the app's pipe name reaches the source inside
// the camera service.
HRESULT OpenInProcess(
    const std::wstring& dll, const std::wstring& pipe_attribute, IMFActivate** activate_out,
    IMFMediaSource** source_out, const char** step);

// Drive the media source in the DLL directly, the way the camera service
// does, and print what held and what did not. Returns the exit code.
int Drive(const std::wstring& dll, const std::wstring& pipe_attribute);

}  // namespace probe
