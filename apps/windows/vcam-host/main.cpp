// gc-vcam-host: stands in for the God's Camera app while the camera is tested.
//
// It makes the pipe the media source fetches pictures from, asks Windows for
// the virtual camera, and sends a moving test picture whose frame number and
// time of making are written into it as rows of squares, for gc-vcam-probe
// to read back.
//
//   gc-vcam-host [--seconds N] [--no-camera] [--size 1280x720|1920x1080]
//
//   --seconds N   stop after N seconds; without it, run until Ctrl+C
//   --no-camera   only the pipe: for tests that load the media source themselves
//   --size WxH    always send this size; without it, send the size asked for
//
// The last line it prints is one line of JSON.

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <unknwn.h>
#include <windows.h>

#include <ole2.h>

#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mfobjects.h>
#include <mfvirtualcamera.h>

#include <wrl/client.h>

#include <cstdio>
#include <cstring>
#include <cwchar>
#include <memory>
#include <string>

#include "gc_test_pattern.h"
#include "gc_vcam_client.h"
#include "gc_vcam_guids.h"
#include "gc_vcam_protocol.h"
#include "gc_vcam_win.h"

using Microsoft::WRL::ComPtr;

namespace {

constexpr int64_t kFramePeriod = 333333;  // 100 ns units: thirty a second
constexpr int64_t kSecond = 10000000;

HANDLE g_stop = nullptr;

BOOL WINAPI OnConsoleControl(DWORD) {
    SetEvent(g_stop);
    return TRUE;
}

struct Options {
    int seconds = 0;  // 0: until Ctrl+C
    bool camera = true;
    uint32_t width = 0;  // 0: whatever the reading side asks for
    uint32_t height = 0;
};

void PrintUsage() {
    std::fputs(
        "gc-vcam-host [--seconds N] [--no-camera] [--size 1280x720|1920x1080]\n"
        "  --seconds N   stop after N seconds; without it, run until Ctrl+C\n"
        "  --no-camera   only the pipe: for tests that load the media source themselves\n"
        "  --size WxH    always send this size; without it, send the size asked for\n",
        stderr);
}

bool ParseOptions(int argc, wchar_t** argv, Options* options) {
    for (int i = 1; i < argc; ++i) {
        const std::wstring argument = argv[i];
        if (argument == L"--no-camera") {
            options->camera = false;
        } else if (argument == L"--seconds" && i + 1 < argc) {
            options->seconds = _wtoi(argv[++i]);
            if (options->seconds <= 0) {
                return false;
            }
        } else if (argument == L"--size" && i + 1 < argc) {
            const std::wstring size = argv[++i];
            if (size == L"1280x720") {
                options->width = 1280;
                options->height = 720;
            } else if (size == L"1920x1080") {
                options->width = 1920;
                options->height = 1080;
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    return true;
}

// What a failure to make the camera most likely means, in plain words.
const char* Explain(HRESULT result) {
    switch (static_cast<unsigned long>(result)) {
        case 0x80040154ul:  // REGDB_E_CLASSNOTREG
            return "The media source is not registered on this computer. Build it, then run "
                   "tools\\\\register-windows-camera.ps1 in a PowerShell window opened as administrator.";
        case 0x80070005ul:  // E_ACCESSDENIED
            return "Windows refused. Either the media source's file is where the camera service may not read "
                   "it (tools\\\\register-windows-camera.ps1 puts it under Program Files, where it may), or "
                   "camera access is switched off in Settings > Privacy & security > Camera.";
        case 0x8007007Eul:  // ERROR_MOD_NOT_FOUND
        case 0x80070002ul:  // ERROR_FILE_NOT_FOUND
            return "The media source is registered, but the camera service cannot load its file. Run "
                   "tools\\\\register-windows-camera.ps1 again as administrator.";
        case 0x80070490ul:  // ERROR_NOT_FOUND
            return "Windows could not find what it needs to make the camera. The media source may not be "
                   "registered: run tools\\\\register-windows-camera.ps1 as administrator.";
        default:
            return "Windows could not make the camera.";
    }
}

struct Camera {
    ComPtr<IMFVirtualCamera> device;
    bool started_media_foundation = false;
    const char* state = "off";  // "off", "started" or "failed"
    const char* failed_step = "";
    HRESULT result = S_OK;
};

// Ask Windows for the camera. It lives for as long as this process holds it.
void StartCamera(const wchar_t* pipe_name, Camera* camera) {
    camera->state = "failed";
    camera->failed_step = "MFStartup";
    camera->result = MFStartup(MF_VERSION);
    if (FAILED(camera->result)) {
        return;
    }
    camera->started_media_foundation = true;

    camera->failed_step = "MFCreateVirtualCamera";
    camera->result = MFCreateVirtualCamera(
        MFVirtualCameraType_SoftwareCameraSource, MFVirtualCameraLifetime_Session, MFVirtualCameraAccess_CurrentUser,
        GC_VCAM_FRIENDLY_NAME, GC_VCAM_CLSID_STRING, nullptr, 0, &camera->device);
    if (FAILED(camera->result)) {
        return;
    }
    // The media source runs in the camera service, as another account. This
    // tells it whose pipe to fetch pictures from.
    camera->failed_step = "IMFVirtualCamera::SetString";
    camera->result = camera->device->SetString(GC_VCAM_ATTRIBUTE_PIPE_NAME, pipe_name);
    if (FAILED(camera->result)) {
        return;
    }
    camera->failed_step = "IMFVirtualCamera::Start";
    camera->result = camera->device->Start(nullptr);
    if (FAILED(camera->result)) {
        return;
    }
    camera->state = "started";
    camera->failed_step = "";
}

void StopCamera(Camera* camera) {
    if (camera->device) {
        // Remove takes a started camera off the computer, and shuts it down
        // as it does; a camera that never started only needs the shutdown.
        if (std::strcmp(camera->state, "started") != 0 || FAILED(camera->device->Remove())) {
            camera->device->Shutdown();
        }
        camera->device.Reset();
    }
    if (camera->started_media_foundation) {
        MFShutdown();
        camera->started_media_foundation = false;
    }
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
    Options options;
    if (!ParseOptions(argc, argv, &options)) {
        PrintUsage();
        return 1;
    }
    g_stop = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    SetConsoleCtrlHandler(OnConsoleControl, TRUE);

    gc_vcam_server* server = nullptr;
    const uint32_t pipe_error = gc_vcam_server_create(&server);
    if (pipe_error != 0) {
        const char* why = pipe_error == ERROR_ACCESS_DENIED || pipe_error == ERROR_PIPE_BUSY
            ? "Another program already holds the camera's pipe. Is another gc-vcam-host, or the app, running?"
            : "The camera's pipe could not be made.";
        std::printf(
            "{\"tool\":\"gc-vcam-host\",\"ok\":false,\"pipe_error\":%u,\"message\":\"%s\"}\n", pipe_error, why);
        return 1;
    }

    Camera camera;
    const bool com_ready = SUCCEEDED(CoInitializeEx(nullptr, COINIT_MULTITHREADED));
    if (options.camera) {
        StartCamera(gc_vcam_server_pipe_name(server), &camera);
        if (FAILED(camera.result)) {
            std::printf(
                "{\"tool\":\"gc-vcam-host\",\"ok\":false,\"camera\":\"failed\",\"camera_step\":\"%s\","
                "\"camera_hresult\":\"0x%08lX\",\"message\":\"%s\"}\n",
                camera.failed_step, static_cast<unsigned long>(camera.result), Explain(camera.result));
            StopCamera(&camera);
            gc_vcam_server_destroy(server);
            if (com_ready) {
                CoUninitialize();
            }
            return 2;
        }
        std::fputs("The camera \"God's Camera\" is there for as long as this runs.\n", stderr);
    }

    gc::Handle timer(CreateWaitableTimerExW(nullptr, nullptr, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS));
    std::unique_ptr<gc_pattern::Renderer> renderer;
    const int64_t began = gc_vcam_now_100ns();
    const int64_t ends = options.seconds > 0 ? began + options.seconds * kSecond : INT64_MAX;
    int64_t tick = 0;
    uint32_t number = 1;  // of the next picture; also what the squares in it say
    bool was_watched = false;
    uint32_t last_width = 0;
    uint32_t last_height = 0;

    for (;;) {
        // One picture every thirtieth of a second, on a schedule that does not drift.
        const int64_t due = began + tick * kFramePeriod;
        int64_t now = gc_vcam_now_100ns();
        if (now >= ends) {
            break;
        }
        if (due > now) {
            DWORD stopped = WAIT_TIMEOUT;
            if (timer.valid()) {
                LARGE_INTEGER after{};
                after.QuadPart = -(due - now);
                SetWaitableTimer(timer.get(), &after, 0, nullptr, nullptr, FALSE);
                const HANDLE waits[2] = {g_stop, timer.get()};
                stopped = WaitForMultipleObjects(2, waits, FALSE, INFINITE);
            } else {
                stopped = WaitForSingleObject(g_stop, static_cast<DWORD>((due - now) / 10000 + 1));
            }
            if (stopped == WAIT_OBJECT_0) {
                break;
            }
            ++tick;
        } else {
            // Fell behind: skip the pictures that are already late.
            tick = (now - began) / kFramePeriod + 1;
            if (WaitForSingleObject(g_stop, 0) == WAIT_OBJECT_0) {
                break;
            }
        }

        gc_vcam_wanted wanted{};
        const bool watched = gc_vcam_server_watched(server, &wanted) != 0;
        if (watched != was_watched) {
            if (watched) {
                std::fprintf(stderr, "Something is watching: it asked for %u x %u.\n", wanted.width, wanted.height);
            } else {
                std::fputs("Nothing is watching.\n", stderr);
            }
            was_watched = watched;
        }
        if (!watched) {
            continue;
        }
        const uint32_t width = options.width != 0 ? options.width : wanted.width;
        const uint32_t height = options.width != 0 ? options.height : wanted.height;
        if (width < 320 || height < 180) {
            continue;  // too small for the squares to be read back
        }
        if (!renderer || renderer->width() != width || renderer->height() != height) {
            renderer = std::make_unique<gc_pattern::Renderer>(width, height);
        }
        const int64_t made = gc_vcam_now_100ns();
        const auto& picture = renderer->render(number, made);
        if (gc_vcam_server_send(server, picture.data(), width, height, made) != 0) {
            ++number;
            last_width = width;
            last_height = height;
        }
    }

    const double ran = static_cast<double>(gc_vcam_now_100ns() - began) / kSecond;
    // The last picture may still be on its way out: give it a moment before counting.
    gc_vcam_stats stats{};
    gc_vcam_server_get_stats(server, &stats);
    for (int i = 0; i < 20 && stats.frames_sent + stats.frames_dropped + 1 < number; ++i) {
        Sleep(5);
        gc_vcam_server_get_stats(server, &stats);
    }
    StopCamera(&camera);
    gc_vcam_server_destroy(server);
    if (com_ready) {
        CoUninitialize();
    }
    std::printf(
        "{\"tool\":\"gc-vcam-host\",\"ok\":true,\"camera\":\"%s\",\"seconds\":%.1f,\"frames_made\":%u,"
        "\"frames_sent\":%llu,\"frames_dropped\":%llu,\"connects\":%llu,\"write_failures\":%llu,"
        "\"last_size\":\"%ux%u\"}\n",
        camera.state, ran, number - 1, static_cast<unsigned long long>(stats.frames_sent),
        static_cast<unsigned long long>(stats.frames_dropped), static_cast<unsigned long long>(stats.connects),
        static_cast<unsigned long long>(stats.write_failures), last_width, last_height);
    return 0;
}
