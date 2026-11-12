// The headers every file of the media source needs, in the order Windows wants them.
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
#include <propvarutil.h>

// With this, the GUIDs the next headers declare are also defined here.
#include <initguid.h>

#include <ks.h>
#include <ksmedia.h>
#include <ksproxy.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mfobjects.h>
#include <mfvirtualcamera.h>

#include <wrl/client.h>
#include <wrl/implements.h>

#include <atomic>
#include <cstdint>
#include <cstring>
#include <deque>
#include <mutex>
#include <new>
#include <string>
#include <thread>
#include <vector>

#include "gc_slate.h"
#include "gc_vcam_guids.h"
#include "gc_vcam_protocol.h"
#include "gc_vcam_win.h"

// A line for a debugger or DebugView. Never the contents of a picture, and
// never who the user is.
void DebugLine(const wchar_t* format, ...);

// Keep this DLL in the process for as long as the process lives. Called
// before the first thread of the source's own starts, so that the code a
// thread is running can never be unloaded under it.
void PinModule();

// Counts this DLL's live COM objects, for DllCanUnloadNow.
extern std::atomic<long> g_live_objects;
struct LiveObject {
    LiveObject() { g_live_objects.fetch_add(1); }
    ~LiveObject() { g_live_objects.fetch_sub(1); }
    LiveObject(const LiveObject&) = delete;
    LiveObject& operator=(const LiveObject&) = delete;
};

#define RETURN_IF_FAILED(expression)       \
    do {                                   \
        const HRESULT hr__ = (expression); \
        if (FAILED(hr__)) {                \
            return hr__;                   \
        }                                  \
    } while (0)

// No C++ exception may leave a COM method or a thread: inside the camera
// service it would take every camera on the machine down with it.
template <typename Body>
HRESULT Guarded(Body&& body) noexcept {
    try {
        return body();
    } catch (const std::bad_alloc&) {
        return E_OUTOFMEMORY;
    } catch (...) {
        return E_UNEXPECTED;
    }
}
