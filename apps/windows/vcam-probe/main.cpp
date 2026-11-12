// gc-vcam-probe: reads the camera the way a meeting app would, and says what it got.
//
//   gc-vcam-probe [--in-process <GodsCameraSource.dll>] [--seconds N] [--size 1280x720|1920x1080]
//                 [--format NV12|YUY2] [--pipe-attribute user|<name>]
//   gc-vcam-probe --drive <GodsCameraSource.dll> [--pipe-attribute user|<name>]
//
// Without --in-process it looks for the camera named "God's Camera" among the
// computer's cameras, which only works once the media source is registered
// and something (gc-vcam-host, or the app) has made the camera.
//
// With --in-process it loads the media source's DLL into this process and
// reads it directly. That needs no registration, and tests everything but
// the camera service itself.
//
// With --drive it loads the DLL as well, and then makes the calls the camera
// service makes and a reader does not; see drive.cpp.
//
// With --pipe-attribute, either of the two hands the source a pipe name the
// way the app does through the camera: "user" for this user's own pipe, or
// any text, to see that the source takes no name that is not one of ours.
//
// The pictures gc-vcam-host sends carry their frame number and the time they
// were made as rows of squares. The probe reads them back from what the
// camera delivers, so it measures what an app would really get.
//
// The last line it prints is one line of JSON.

#include "probe.h"

#include <algorithm>
#include <cmath>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <string>
#include <vector>

#include "gc_slate.h"
#include "gc_test_pattern.h"
#include "gc_vcam_guids.h"
#include "gc_vcam_protocol.h"
#include "gc_vcam_win.h"

using Microsoft::WRL::ComPtr;

namespace probe {

constexpr int64_t kSecond = 10000000;
constexpr DWORD kVideo = static_cast<DWORD>(MF_SOURCE_READER_FIRST_VIDEO_STREAM);

struct Options {
    std::wstring dll;  // empty: find the camera among the computer's devices
    bool drive = false;
    std::wstring pipe_attribute;  // empty: hand the source no pipe name
    int seconds = 10;
    uint32_t width = 1280;
    uint32_t height = 720;
    bool yuy2 = false;
};

void PrintUsage() {
    std::fputs(
        "gc-vcam-probe [--in-process <GodsCameraSource.dll>] [--seconds N] [--size 1280x720|1920x1080]\n"
        "              [--format NV12|YUY2] [--pipe-attribute user|<name>]\n"
        "gc-vcam-probe --drive <GodsCameraSource.dll> [--pipe-attribute user|<name>]\n"
        "  --in-process DLL  load the media source into this process; needs no registration\n"
        "  --drive DLL       load it, and make the calls the camera service makes and a reader does not\n"
        "  --seconds N       how long to read; 10 if not given\n"
        "  --size WxH        the picture size to ask the camera for; 1280x720 if not given\n"
        "  --format F        the picture format to ask for; NV12 if not given\n"
        "  --pipe-attribute  with a DLL: hand the source a pipe name as the app does through the camera;\n"
        "                    'user' for this user's own pipe, or any text\n",
        stderr);
}

bool ParseOptions(int argc, wchar_t** argv, Options* options) {
    for (int i = 1; i < argc; ++i) {
        const std::wstring argument = argv[i];
        if (argument == L"--in-process" && i + 1 < argc) {
            options->dll = argv[++i];
        } else if (argument == L"--drive" && i + 1 < argc) {
            options->dll = argv[++i];
            options->drive = true;
        } else if (argument == L"--pipe-attribute" && i + 1 < argc) {
            options->pipe_attribute = argv[++i];
            if (options->pipe_attribute == L"user") {
                options->pipe_attribute = gc::pipe_name_for(gc::current_user_sid());
            }
        } else if (argument == L"--seconds" && i + 1 < argc) {
            options->seconds = _wtoi(argv[++i]);
            if (options->seconds <= 0 || options->seconds > 3600) {
                return false;
            }
        } else if (argument == L"--size" && i + 1 < argc) {
            unsigned width = 0;
            unsigned height = 0;
            if (swscanf_s(argv[++i], L"%ux%u", &width, &height) != 2 || width < 64 || height < 64) {
                return false;
            }
            options->width = width;
            options->height = height;
        } else if (argument == L"--format" && i + 1 < argc) {
            const std::wstring format = argv[++i];
            if (format != L"NV12" && format != L"YUY2") {
                return false;
            }
            options->yuy2 = format == L"YUY2";
        } else {
            return false;
        }
    }
    return true;
}

void Append(std::string* out, const char* format, ...) {
    char text[512];
    va_list arguments;
    va_start(arguments, format);
    vsnprintf_s(text, _TRUNCATE, format, arguments);
    va_end(arguments);
    out->append(text);
}

// One line of JSON that says why the probe could not do its work.
int Fail(const char* mode, const char* step, HRESULT result, const char* message) {
    std::printf(
        "{\"tool\":\"gc-vcam-probe\",\"ok\":false,\"mode\":\"%s\",\"step\":\"%s\",\"hresult\":\"0x%08lX\","
        "\"message\":\"%s\"}\n",
        mode, step, static_cast<unsigned long>(result), message);
    std::fflush(stdout);
    return 2;
}

// "NV12" for the NV12 subtype, and the like; a number in hex for anything unusual.
std::string FormatName(const GUID& subtype) {
    const DWORD code = subtype.Data1;
    char text[16];
    const char a = static_cast<char>(code & 0xFF);
    const char b = static_cast<char>((code >> 8) & 0xFF);
    const char c = static_cast<char>((code >> 16) & 0xFF);
    const char d = static_cast<char>((code >> 24) & 0xFF);
    const auto printable = [](char x) { return x >= 0x20 && x < 0x7F && x != '"' && x != '\\'; };
    if (printable(a) && printable(b) && printable(c) && printable(d)) {
        sprintf_s(text, "%c%c%c%c", a, b, c, d);
    } else {
        sprintf_s(text, "0x%08lX", static_cast<unsigned long>(code));
    }
    return text;
}

TypeInfo Describe(IMFMediaType* type) {
    TypeInfo info;
    UINT32 width = 0;
    UINT32 height = 0;
    UINT32 numerator = 0;
    UINT32 denominator = 0;
    MFGetAttributeSize(type, MF_MT_FRAME_SIZE, &width, &height);
    MFGetAttributeRatio(type, MF_MT_FRAME_RATE, &numerator, &denominator);
    type->GetGUID(MF_MT_SUBTYPE, &info.subtype);
    info.width = width;
    info.height = height;
    info.fps = denominator != 0 ? static_cast<double>(numerator) / denominator : 0.0;
    info.stride = static_cast<LONG>(MFGetAttributeUINT32(type, MF_MT_DEFAULT_STRIDE, width));
    return info;
}

// Load the media source's DLL and make the source as COM would, without the registry.
HRESULT OpenInProcess(
    const std::wstring& dll, const std::wstring& pipe_attribute, IMFActivate** activate_out,
    IMFMediaSource** source_out, const char** step) {
    wchar_t full[MAX_PATH * 2];
    const DWORD length = GetFullPathNameW(dll.c_str(), ARRAYSIZE(full), full, nullptr);
    *step = "LoadLibrary";
    if (length == 0 || length >= ARRAYSIZE(full)) {
        return HRESULT_FROM_WIN32(ERROR_BAD_PATHNAME);
    }
    const HMODULE module = LoadLibraryW(full);
    if (module == nullptr) {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    using GetClassObject = HRESULT(STDAPICALLTYPE*)(REFCLSID, REFIID, LPVOID*);
    const auto get_class_object = reinterpret_cast<GetClassObject>(
        reinterpret_cast<void*>(GetProcAddress(module, "DllGetClassObject")));
    if (get_class_object == nullptr) {
        return HRESULT_FROM_WIN32(ERROR_PROC_NOT_FOUND);
    }
    *step = "DllGetClassObject";
    ComPtr<IClassFactory> factory;
    HRESULT result = get_class_object(CLSID_GodsCameraSource, IID_PPV_ARGS(&factory));
    if (FAILED(result)) {
        return result;
    }
    *step = "IClassFactory::CreateInstance";
    ComPtr<IMFActivate> activate;
    result = factory->CreateInstance(nullptr, IID_PPV_ARGS(&activate));
    if (FAILED(result)) {
        return result;
    }
    if (!pipe_attribute.empty()) {
        // What the camera service does with the attributes the app set on its camera.
        *step = "IMFActivate::SetString";
        result = activate->SetString(GC_VCAM_ATTRIBUTE_PIPE_NAME, pipe_attribute.c_str());
        if (FAILED(result)) {
            return result;
        }
    }
    *step = "IMFActivate::ActivateObject";
    result = activate->ActivateObject(IID_PPV_ARGS(source_out));
    if (FAILED(result)) {
        return result;
    }
    *activate_out = activate.Detach();
    return S_OK;
}

// Text from Windows as it may stand inside a JSON string.
std::string JsonText(const wchar_t* text) {
    char utf8[512] = {};
    WideCharToMultiByte(CP_UTF8, 0, text, -1, utf8, static_cast<int>(sizeof(utf8)) - 1, nullptr, nullptr);
    std::string out;
    for (const char* c = utf8; *c != 0; ++c) {
        if (*c == '"' || *c == '\\') {
            out += '\\';
            out += *c;
        } else if (static_cast<unsigned char>(*c) >= 0x20) {
            out += *c;
        }
    }
    return out;
}

// Find the camera by its name among the computer's video capture devices.
// Windows may add words of its own after the name the app gave, so a name
// that begins with ours is ours.
HRESULT OpenDevice(
    IMFMediaSource** source_out, UINT32* device_count, bool* found, std::string* found_name, const char** step) {
    *step = "MFEnumDeviceSources";
    ComPtr<IMFAttributes> attributes;
    HRESULT result = MFCreateAttributes(&attributes, 1);
    if (FAILED(result)) {
        return result;
    }
    result = attributes->SetGUID(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID);
    if (FAILED(result)) {
        return result;
    }
    IMFActivate** devices = nullptr;
    UINT32 count = 0;
    result = MFEnumDeviceSources(attributes.Get(), &devices, &count);
    if (FAILED(result)) {
        return result;
    }
    *device_count = count;
    ComPtr<IMFActivate> ours;
    for (UINT32 i = 0; i < count; ++i) {
        wchar_t* name = nullptr;
        UINT32 length = 0;
        if (!ours && SUCCEEDED(devices[i]->GetAllocatedString(MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &name, &length))) {
            if (wcsncmp(name, GC_VCAM_FRIENDLY_NAME, wcslen(GC_VCAM_FRIENDLY_NAME)) == 0) {
                ours = devices[i];
                *found_name = JsonText(name);
            }
        }
        CoTaskMemFree(name);
        devices[i]->Release();
    }
    CoTaskMemFree(devices);
    *found = ours != nullptr;
    if (!ours) {
        return S_OK;
    }
    *step = "IMFActivate::ActivateObject";
    return ours->ActivateObject(IID_PPV_ARGS(source_out));
}

// What the camera service needs of a source, checked on the source itself.
// Returns a JSON object.
std::string CheckSource(IMFActivate* activate, IMFMediaSource* source) {
    std::string missing;
    const auto has = [&](IUnknown* object, REFIID iid, const char* name) {
        ComPtr<IUnknown> found;
        if (FAILED(object->QueryInterface(iid, &found))) {
            missing += missing.empty() ? "" : ",";
            missing += std::string("\"") + name + "\"";
        }
    };
    has(activate, __uuidof(IMFAttributes), "IMFActivate as IMFAttributes");
    has(source, __uuidof(IMFMediaEventGenerator), "IMFMediaEventGenerator");
    has(source, __uuidof(IMFMediaSourceEx), "IMFMediaSourceEx");
    has(source, __uuidof(IMFGetService), "IMFGetService");
    has(source, __uuidof(IKsControl), "IKsControl");
    has(source, __uuidof(IMFSampleAllocatorControl), "IMFSampleAllocatorControl");

    bool same_store = false;
    bool sensor_profile = false;
    bool stream_category = false;
    bool shared = false;
    bool colour = false;
    bool allocator = false;
    ComPtr<IMFMediaSourceEx> extended;
    if (SUCCEEDED(source->QueryInterface(IID_PPV_ARGS(&extended)))) {
        ComPtr<IMFAttributes> first;
        ComPtr<IMFAttributes> second;
        if (SUCCEEDED(extended->GetSourceAttributes(&first)) && SUCCEEDED(extended->GetSourceAttributes(&second))) {
            same_store = first.Get() == second.Get();
            ComPtr<IUnknown> profiles;
            sensor_profile = SUCCEEDED(first->GetUnknown(MF_DEVICEMFT_SENSORPROFILE_COLLECTION, IID_PPV_ARGS(&profiles)));
        }
        ComPtr<IMFAttributes> stream;
        if (SUCCEEDED(extended->GetStreamAttributes(0, &stream))) {
            GUID category{};
            stream_category = SUCCEEDED(stream->GetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, &category))
                && category == PINNAME_VIDEO_CAPTURE;
            shared = MFGetAttributeUINT32(stream.Get(), MF_DEVICESTREAM_FRAMESERVER_SHARED, 0) == 1;
            colour = MFGetAttributeUINT32(stream.Get(), MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, 0)
                == static_cast<UINT32>(MFFrameSourceTypes_Color);
        }
    }
    ComPtr<IMFSampleAllocatorControl> control;
    if (SUCCEEDED(source->QueryInterface(IID_PPV_ARGS(&control)))) {
        DWORD input = 99;
        MFSampleAllocatorUsage usage = MFSampleAllocatorUsage_DoesNotAllocate;
        allocator = SUCCEEDED(control->GetAllocatorUsage(0, &input, &usage))
            && usage == MFSampleAllocatorUsage_UsesProvidedAllocator && input == 0;
    }
    const auto word = [](bool value) { return value ? "true" : "false"; };
    std::string out;
    Append(&out, "{\"interfaces_missing\":[%s],\"source_attributes_are_one_store\":%s,", missing.c_str(), word(same_store));
    Append(&out, "\"legacy_sensor_profile\":%s,\"stream_is_capture_pin\":%s,", word(sensor_profile), word(stream_category));
    Append(&out, "\"stream_is_shared\":%s,\"stream_is_colour\":%s,", word(shared), word(colour));
    Append(&out, "\"uses_provided_allocator\":%s}", word(allocator));
    return out;
}

double Percentile(std::vector<double> values, double fraction) {
    if (values.empty()) {
        return 0.0;
    }
    std::sort(values.begin(), values.end());
    const size_t index = static_cast<size_t>(std::ceil(fraction * static_cast<double>(values.size())));
    return values[std::min(values.size() - 1, index == 0 ? 0 : index - 1)];
}

void Note(Findings* findings, const char* kind, double at_ms, uint32_t number, int why = -1) {
    if (findings->timeline.empty() || std::strcmp(findings->timeline.back().kind, kind) != 0
        || findings->timeline.back().why != why) {
        Stretch stretch;
        stretch.kind = kind;
        stretch.why = why;
        stretch.from_ms = at_ms;
        stretch.first_number = number;
        findings->timeline.push_back(stretch);
    }
    Stretch& stretch = findings->timeline.back();
    stretch.to_ms = at_ms;
    stretch.last_number = number;
    ++stretch.samples;
}

// Look at one picture: which of the host's frames it is, or the slate, or
// neither. Returns true if it is one of the host's.
bool Examine(
    Findings* findings, const uint8_t* luma, size_t stride, const TypeInfo& type, int64_t arrival, int64_t began) {
    const double at_ms = static_cast<double>(arrival - began) / 10000.0;
    const gc_pattern::Decoded decoded = gc_pattern::read_code(luma, stride, type.width, type.height);
    if (decoded.is_pattern) {
        if (findings->have_number && decoded.number == findings->last_number) {
            ++findings->repeats;
        } else {
            if (findings->have_number) {
                if (decoded.number < findings->last_number) {
                    ++findings->restarts;  // another host has taken over, counting from one again
                } else if (decoded.number > findings->last_number + 1) {
                    ++findings->gap_events;
                    findings->missing += decoded.number - findings->last_number - 1;
                }
            } else {
                findings->first_number = decoded.number;
            }
            ++findings->new_frames;
            const int64_t made = gc_pattern::full_time(decoded.time_low, arrival);
            findings->delays_ms.push_back(static_cast<double>(arrival - made) / 10000.0);
        }
        findings->have_number = true;
        findings->last_number = decoded.number;
        Note(findings, "frames", at_ms, decoded.number);
        return true;
    }
    if (gc_slate::looks_like(luma, stride, type.width, type.height)) {
        ++findings->slates;
        gc_slate::Why why;
        const bool says = gc_slate::read_why(luma, stride, type.width, type.height, &why);
        Note(findings, "slate", at_ms, 0, says ? static_cast<int>(gc_slate::why_bits(why)) : -1);
    } else {
        ++findings->others;
        Note(findings, "other", at_ms, 0);
    }
    return false;
}

// Whether the eight colour bars of the host's picture arrived with the
// colours the host drew: one spot in the middle of each bar, in the lower
// half of the picture where nothing else is drawn over the colour.
bool ColoursMatch(const BYTE* first_row, LONG pitch, const TypeInfo& type) {
    const bool yuy2 = type.subtype == MFVideoFormat_YUY2;
    const uint32_t y = (type.height * 3 / 4) & ~1u;
    for (int bar = 0; bar < gc_pattern::kBarCount; ++bar) {
        const uint32_t x = (type.width * (2 * bar + 1) / (2 * gc_pattern::kBarCount)) & ~1u;
        int blue = 0;
        int red = 0;
        if (yuy2) {
            // Two pixels in four bytes: luma, blue difference, luma, red difference.
            const BYTE* pair = first_row + static_cast<size_t>(y) * pitch + static_cast<size_t>(x) * 2;
            blue = pair[1];
            red = pair[3];
        } else {
            // After the luma: one row of colour for two rows of picture, blue and red side by side.
            const BYTE* pair = first_row + static_cast<size_t>(type.height + y / 2) * pitch + x;
            blue = pair[0];
            red = pair[1];
        }
        if (std::abs(blue - gc_pattern::kBars[bar][1]) > 4 || std::abs(red - gc_pattern::kBars[bar][2]) > 4) {
            return false;
        }
    }
    return true;
}

// Examine a locked, whole picture whose rows are `pitch` bytes apart. NV12
// begins with its luma, which is what the squares are read from. YUY2 keeps
// luma in every other byte, so it is picked out first. Any other format is
// not one the camera should ever send.
void ExamineLocked(
    Findings* findings, const BYTE* first_row, LONG pitch, const TypeInfo& type, int64_t arrival, int64_t began,
    std::vector<uint8_t>* scratch) {
    bool from_host = false;
    if (type.subtype == MFVideoFormat_NV12) {
        from_host = Examine(findings, first_row, static_cast<size_t>(pitch), type, arrival, began);
    } else if (type.subtype == MFVideoFormat_YUY2) {
        scratch->resize(static_cast<size_t>(type.width) * type.height);
        for (uint32_t y = 0; y < type.height; ++y) {
            const BYTE* row = first_row + static_cast<size_t>(y) * pitch;
            uint8_t* luma = scratch->data() + static_cast<size_t>(y) * type.width;
            for (uint32_t x = 0; x < type.width; ++x) {
                luma[x] = row[static_cast<size_t>(x) * 2];
            }
        }
        from_host = Examine(findings, scratch->data(), type.width, type, arrival, began);
    } else {
        ++findings->others;
        Note(findings, "other", static_cast<double>(arrival - began) / 10000.0, 0);
    }
    if (from_host) {
        ++findings->colours_checked;
        findings->colours_wrong += ColoursMatch(first_row, pitch, type) ? 0 : 1;
    }
}

// If reading hangs, say so and end: a camera that hangs its reader is the
// very thing this tool is there to catch.
DWORD WINAPI Watchdog(LPVOID parameter) {
    const DWORD milliseconds = static_cast<DWORD>(reinterpret_cast<uintptr_t>(parameter));
    Sleep(milliseconds);
    std::printf(
        "{\"tool\":\"gc-vcam-probe\",\"ok\":false,\"step\":\"reading\",\"hresult\":\"0x%08lX\","
        "\"message\":\"The camera stopped answering: reading it hung.\"}\n",
        static_cast<unsigned long>(HRESULT_FROM_WIN32(ERROR_TIMEOUT)));
    std::fflush(stdout);
    TerminateProcess(GetCurrentProcess(), 3);
    return 0;
}

}  // namespace probe

using namespace probe;

int wmain(int argc, wchar_t** argv) {
    Options options;
    if (!ParseOptions(argc, argv, &options)) {
        PrintUsage();
        return 1;
    }
    const bool in_process = !options.dll.empty();
    const char* mode = options.drive ? "drive" : in_process ? "in-process" : "device";

    HRESULT result = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    if (FAILED(result)) {
        return Fail(mode, "CoInitializeEx", result, "COM could not be started.");
    }
    result = MFStartup(MF_VERSION);
    if (FAILED(result)) {
        return Fail(mode, "MFStartup", result, "Media Foundation could not be started.");
    }
    CloseHandle(CreateThread(
        nullptr, 0, Watchdog, reinterpret_cast<LPVOID>(static_cast<uintptr_t>((options.seconds + 20) * 1000)), 0,
        nullptr));
    if (options.drive) {
        const int code = Drive(options.dll, options.pipe_attribute);
        MFShutdown();
        CoUninitialize();
        return code;
    }

    // Find or make the source.
    ComPtr<IMFActivate> activate;
    ComPtr<IMFMediaSource> source;
    const char* step = "";
    UINT32 device_count = 0;
    bool found = false;
    std::string device_name;
    std::string checks = "null";
    if (in_process) {
        result = OpenInProcess(options.dll, options.pipe_attribute, &activate, &source, &step);
        if (FAILED(result)) {
            return Fail(mode, step, result, "The media source could not be loaded from the DLL.");
        }
        found = true;
        checks = CheckSource(activate.Get(), source.Get());
    } else {
        result = OpenDevice(&source, &device_count, &found, &device_name, &step);
        if (FAILED(result)) {
            return Fail(mode, step, result, "The camera was found but could not be opened.");
        }
        if (!found) {
            std::printf(
                "{\"tool\":\"gc-vcam-probe\",\"ok\":false,\"mode\":\"device\",\"device_found\":false,"
                "\"video_devices\":%u,\"message\":\"No camera named God's Camera. The media source must be "
                "registered, and gc-vcam-host or the app must be running.\"}\n",
                device_count);
            return 2;
        }
    }

    // Read it as an app would: through a source reader, taking the camera's
    // own format with nothing converting in between.
    ComPtr<IMFAttributes> reader_attributes;
    ComPtr<IMFSourceReader> reader;
    result = MFCreateAttributes(&reader_attributes, 1);
    if (SUCCEEDED(result)) {
        result = reader_attributes->SetUINT32(MF_READWRITE_DISABLE_CONVERTERS, TRUE);
    }
    if (SUCCEEDED(result)) {
        result = MFCreateSourceReaderFromMediaSource(source.Get(), reader_attributes.Get(), &reader);
    }
    if (FAILED(result)) {
        return Fail(mode, "MFCreateSourceReaderFromMediaSource", result, "The camera could not be read.");
    }

    std::string native_types;
    ComPtr<IMFMediaType> chosen;
    for (DWORD index = 0;; ++index) {
        ComPtr<IMFMediaType> type;
        if (FAILED(reader->GetNativeMediaType(kVideo, index, &type))) {
            break;
        }
        const TypeInfo info = Describe(type.Get());
        Append(
            &native_types, "%s\"%ux%u %s %.2f\"", native_types.empty() ? "" : ",", info.width, info.height,
            FormatName(info.subtype).c_str(), info.fps);
        const GUID wanted = options.yuy2 ? MFVideoFormat_YUY2 : MFVideoFormat_NV12;
        if (!chosen && info.width == options.width && info.height == options.height && info.subtype == wanted) {
            chosen = type;
        }
    }
    if (!chosen) {
        std::printf(
            "{\"tool\":\"gc-vcam-probe\",\"ok\":false,\"mode\":\"%s\",\"device_found\":true,\"native_types\":[%s],"
            "\"message\":\"The camera does not offer %ux%u %s.\"}\n",
            mode, native_types.c_str(), options.width, options.height, options.yuy2 ? "YUY2" : "NV12");
        return 2;
    }
    reader->SetStreamSelection(static_cast<DWORD>(MF_SOURCE_READER_ALL_STREAMS), FALSE);
    reader->SetStreamSelection(kVideo, TRUE);
    result = reader->SetCurrentMediaType(kVideo, nullptr, chosen.Get());
    if (FAILED(result)) {
        return Fail(mode, "IMFSourceReader::SetCurrentMediaType", result, "The camera refused the picture size.");
    }
    ComPtr<IMFMediaType> current;
    result = reader->GetCurrentMediaType(kVideo, &current);
    if (FAILED(result)) {
        return Fail(mode, "IMFSourceReader::GetCurrentMediaType", result, "The camera did not say what it sends.");
    }
    TypeInfo negotiated = Describe(current.Get());

    Findings findings;
    std::vector<uint8_t> scratch;
    const int64_t began = gc_vcam_now_100ns();
    const int64_t ends = began + options.seconds * kSecond;
    HRESULT read_result = S_OK;
    DWORD last_flags = 0;
    while (gc_vcam_now_100ns() < ends) {
        DWORD stream_index = 0;
        DWORD flags = 0;
        LONGLONG timestamp = 0;
        ComPtr<IMFSample> sample;
        read_result = reader->ReadSample(kVideo, 0, &stream_index, &flags, &timestamp, &sample);
        const int64_t arrival = gc_vcam_now_100ns();
        last_flags = flags;
        if (FAILED(read_result) || (flags & (MF_SOURCE_READERF_ERROR | MF_SOURCE_READERF_ENDOFSTREAM)) != 0) {
            break;
        }
        if ((flags & (MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED | MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED)) != 0) {
            ComPtr<IMFMediaType> changed;
            if (SUCCEEDED(reader->GetCurrentMediaType(kVideo, &changed))) {
                negotiated = Describe(changed.Get());
            }
        }
        if (!sample) {
            ++findings.ticks;
            continue;
        }
        ++findings.samples;
        if (findings.first_sample_ms < 0) {
            findings.first_sample_ms = static_cast<double>(arrival - began) / 10000.0;
        } else {
            findings.intervals_ms.push_back(static_cast<double>(arrival - findings.last_arrival) / 10000.0);
        }
        findings.last_arrival = arrival;

        UINT32 said = 0;
        if (SUCCEEDED(sample->GetUINT32(GC_VCAM_SAMPLE_KIND, &said))) {
            findings.said_frame += said == GC_VCAM_SAMPLE_FRAME ? 1 : 0;
            findings.said_repeat += said == GC_VCAM_SAMPLE_REPEAT ? 1 : 0;
            findings.said_slate += said == GC_VCAM_SAMPLE_SLATE ? 1 : 0;
        }

        // The probe only needs the picture's first `height` rows: in NV12
        // they are the luma, in YUY2 they hold it in every other byte.
        ComPtr<IMFMediaBuffer> buffer;
        if (FAILED(sample->GetBufferByIndex(0, &buffer))) {
            ++findings.others;
            continue;
        }
        const LONG row_bytes = static_cast<LONG>(negotiated.subtype == MFVideoFormat_YUY2 ? negotiated.width * 2 : negotiated.width);
        ComPtr<IMF2DBuffer> rows;
        BYTE* first_row = nullptr;
        LONG pitch = 0;
        if (SUCCEEDED(buffer.As(&rows)) && SUCCEEDED(rows->Lock2D(&first_row, &pitch))) {
            if (pitch >= row_bytes) {
                ExamineLocked(&findings, first_row, pitch, negotiated, arrival, began, &scratch);
            } else {
                ++findings.others;
            }
            rows->Unlock2D();
        } else {
            BYTE* bytes = nullptr;
            DWORD length = 0;
            const LONG stride = negotiated.stride >= row_bytes ? negotiated.stride : row_bytes;
            // All the rows of the picture: NV12 has half as many again for its colour.
            const uint64_t picture_rows = negotiated.subtype == MFVideoFormat_YUY2
                ? negotiated.height
                : negotiated.height + negotiated.height / 2;
            if (SUCCEEDED(buffer->Lock(&bytes, nullptr, &length))) {
                if (length >= static_cast<uint64_t>(stride) * picture_rows) {
                    ExamineLocked(&findings, bytes, stride, negotiated, arrival, began, &scratch);
                } else {
                    ++findings.others;
                }
                buffer->Unlock();
            } else {
                ++findings.others;
            }
        }
    }
    const double ran = static_cast<double>(gc_vcam_now_100ns() - began) / kSecond;

    // Letting go of the reader shuts the source down, as an app's closing does.
    reader.Reset();
    if (activate) {
        activate->ShutdownObject();
    }
    source.Reset();
    activate.Reset();
    MFShutdown();
    CoUninitialize();

    std::string out;
    const bool read_ok = SUCCEEDED(read_result) && (last_flags & MF_SOURCE_READERF_ERROR) == 0;
    Append(&out, "{\"tool\":\"gc-vcam-probe\",\"ok\":%s,\"mode\":\"%s\",\"device_found\":true,", read_ok ? "true" : "false", mode);
    if (!in_process) {
        Append(&out, "\"device_name\":\"%s\",\"video_devices\":%u,", device_name.c_str(), device_count);
    }
    if (!read_ok) {
        Append(&out, "\"read_hresult\":\"0x%08lX\",\"read_flags\":%lu,", static_cast<unsigned long>(read_result), last_flags);
    }
    Append(&out, "\"native_types\":[%s],", native_types.c_str());
    Append(
        &out, "\"negotiated\":{\"width\":%u,\"height\":%u,\"format\":\"%s\",\"fps\":%.2f},", negotiated.width,
        negotiated.height, FormatName(negotiated.subtype).c_str(), negotiated.fps);
    Append(
        &out, "\"seconds\":%.2f,\"samples\":%llu,\"samples_per_second\":%.2f,\"first_sample_ms\":%.1f,", ran,
        static_cast<unsigned long long>(findings.samples), ran > 0 ? static_cast<double>(findings.samples) / ran : 0.0,
        findings.first_sample_ms);
    Append(
        &out, "\"frames\":{\"new\":%llu,\"repeats\":%llu,\"first_number\":%u,\"last_number\":%u,",
        static_cast<unsigned long long>(findings.new_frames), static_cast<unsigned long long>(findings.repeats),
        findings.first_number, findings.last_number);
    Append(
        &out, "\"gaps\":%llu,\"missing\":%llu,\"restarts\":%llu},", static_cast<unsigned long long>(findings.gap_events),
        static_cast<unsigned long long>(findings.missing), static_cast<unsigned long long>(findings.restarts));
    Append(
        &out, "\"slates\":%llu,\"others\":%llu,\"answers_without_picture\":%llu,",
        static_cast<unsigned long long>(findings.slates), static_cast<unsigned long long>(findings.others),
        static_cast<unsigned long long>(findings.ticks));
    Append(
        &out, "\"colours\":{\"checked\":%llu,\"wrong\":%llu},",
        static_cast<unsigned long long>(findings.colours_checked),
        static_cast<unsigned long long>(findings.colours_wrong));
    Append(
        &out, "\"delay_ms\":{\"median\":%.2f,\"p95\":%.2f,\"max\":%.2f},", Percentile(findings.delays_ms, 0.5),
        Percentile(findings.delays_ms, 0.95), Percentile(findings.delays_ms, 1.0));
    Append(
        &out, "\"interval_ms\":{\"median\":%.2f,\"p95\":%.2f,\"max\":%.2f},", Percentile(findings.intervals_ms, 0.5),
        Percentile(findings.intervals_ms, 0.95), Percentile(findings.intervals_ms, 1.0));
    if (in_process) {
        Append(
            &out, "\"source_says\":{\"frames\":%llu,\"repeats\":%llu,\"slates\":%llu},",
            static_cast<unsigned long long>(findings.said_frame), static_cast<unsigned long long>(findings.said_repeat),
            static_cast<unsigned long long>(findings.said_slate));
        Append(&out, "\"source_checks\":");
        out += checks;
        out += ",";
    }
    out += "\"timeline\":[";
    for (size_t i = 0; i < findings.timeline.size(); ++i) {
        const Stretch& stretch = findings.timeline[i];
        Append(
            &out, "%s{\"kind\":\"%s\",\"from_ms\":%.0f,\"to_ms\":%.0f,\"samples\":%llu", i == 0 ? "" : ",", stretch.kind,
            stretch.from_ms, stretch.to_ms, static_cast<unsigned long long>(stretch.samples));
        if (std::strcmp(stretch.kind, "frames") == 0) {
            Append(&out, ",\"first_number\":%u,\"last_number\":%u", stretch.first_number, stretch.last_number);
        }
        if (stretch.why >= 0) {
            // What the slate's own row of squares says about why it is showing.
            static const char* const kCauses[8] = {
                "the source has not tried the app's pipe yet",
                "the app's pipe does not exist",
                "Windows refused to open the app's pipe",
                "the app's pipe is busy with another reader",
                "the app's pipe could not be opened",
                "the pipe does not belong to the user in its name",
                "connected, and the app sends nothing",
                "connected, and the app sends another size than asked for",
            };
            Append(
                &out, ",\"why\":\"%s\",\"error\":%d,\"pipe_named_by_app\":%s", kCauses[(stretch.why >> 16) & 7],
                stretch.why & 0xFFFF, ((stretch.why >> 19) & 1) != 0 ? "true" : "false");
        }
        out += "}";
    }
    out += "]}";
    std::puts(out.c_str());
    std::fflush(stdout);
    return read_ok ? 0 : 2;
}
