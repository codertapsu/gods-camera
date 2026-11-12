// gc-vcam-probe --drive: the media source driven directly, the way Windows'
// camera service drives it, rather than through a source reader.
//
// A source reader never hands the source an allocator, never pauses a stream
// and never asks for another size while pictures flow. The camera service
// does. This walks through those calls on the DLL in this process and checks
// each answer, so they have been run at least once before the service runs
// them where nothing can be seen.

#include "probe.h"

#include <cstdio>

#include "gc_vcam_guids.h"
#include "gc_vcam_protocol.h"

using Microsoft::WRL::ComPtr;

namespace probe {
namespace {

constexpr DWORD kPatience = 2000;  // milliseconds to wait for an event that should come at once

// The steps taken, as the body of a JSON array, and how many went wrong.
struct Report {
    std::string steps;
    int failed = 0;

    void Check(const char* step, bool held, HRESULT result = S_OK) {
        Append(&steps, "%s{\"step\":\"%s\",\"ok\":%s", steps.empty() ? "" : ",", step, held ? "true" : "false");
        if (!held) {
            ++failed;
            Append(&steps, ",\"hresult\":\"0x%08lX\"", static_cast<unsigned long>(result));
        }
        steps += "}";
    }
};

// The next event of one kind from a source or a stream, or nothing if it does
// not come in time. Events of other kinds on the way are passed over.
ComPtr<IMFMediaEvent> NextEvent(IMFMediaEventGenerator* from, MediaEventType wanted, DWORD patience) {
    const ULONGLONG deadline = GetTickCount64() + patience;
    for (;;) {
        ComPtr<IMFMediaEvent> event;
        const HRESULT result = from->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event);
        if (SUCCEEDED(result)) {
            MediaEventType type = MEUnknown;
            event->GetType(&type);
            if (type == wanted) {
                return event;
            }
            continue;
        }
        if (result != MF_E_NO_EVENTS_AVAILABLE || GetTickCount64() > deadline) {
            return nullptr;
        }
        Sleep(1);
    }
}

// The object an event carries: the stream of a new-stream event, the sample of a sample event.
ComPtr<IUnknown> ObjectOf(IMFMediaEvent* event) {
    ComPtr<IUnknown> object;
    PROPVARIANT value;
    PropVariantInit(&value);
    if (event != nullptr && SUCCEEDED(event->GetValue(&value)) && value.vt == VT_UNKNOWN) {
        object = value.punkVal;
    }
    PropVariantClear(&value);
    return object;
}

// The media type of one size and format among those a stream offers.
ComPtr<IMFMediaType> PickType(IMFMediaTypeHandler* handler, uint32_t width, uint32_t height, const GUID& subtype) {
    DWORD count = 0;
    handler->GetMediaTypeCount(&count);
    for (DWORD index = 0; index < count; ++index) {
        ComPtr<IMFMediaType> type;
        if (SUCCEEDED(handler->GetMediaTypeByIndex(index, &type))) {
            const TypeInfo info = Describe(type.Get());
            if (info.width == width && info.height == height && info.subtype == subtype) {
                return type;
            }
        }
    }
    return nullptr;
}

// Ask a stream for `count` pictures, one at a time. Returns how many requests
// were answered properly: with the token that was handed in and a whole
// picture of the expected size. The samples are kept in `keep` if it is given.
int ReadSamples(
    IMFMediaStream* stream, const TypeInfo& type, int count, Findings* findings, std::vector<uint8_t>* scratch,
    std::vector<ComPtr<IMFSample>>* keep) {
    const bool yuy2 = type.subtype == MFVideoFormat_YUY2;
    const DWORD picture_bytes = yuy2 ? type.width * type.height * 2 : gc_vcam_nv12_size(type.width, type.height);
    const int64_t began = gc_vcam_now_100ns();
    int good = 0;
    for (int i = 0; i < count; ++i) {
        // Any COM object will do as a token; the stream must hand back this very one.
        ComPtr<IMFAttributes> token;
        if (FAILED(MFCreateAttributes(&token, 0)) || FAILED(stream->RequestSample(token.Get()))) {
            continue;
        }
        const ComPtr<IMFMediaEvent> event = NextEvent(stream, MEMediaSample, kPatience);
        ComPtr<IMFSample> sample;
        if (!event || FAILED(ObjectOf(event.Get()).As(&sample))) {
            continue;
        }
        const int64_t arrival = gc_vcam_now_100ns();
        ComPtr<IUnknown> handed_back;
        ComPtr<IUnknown> handed_in;
        token.As(&handed_in);
        const bool same_token = SUCCEEDED(sample->GetUnknown(MFSampleExtension_Token, IID_PPV_ARGS(&handed_back)))
            && handed_back.Get() == handed_in.Get();

        ComPtr<IMFMediaBuffer> buffer;
        ComPtr<IMF2DBuffer> rows;
        DWORD length = 0;
        BYTE* first_row = nullptr;
        LONG pitch = 0;
        bool whole_picture = false;
        if (SUCCEEDED(sample->GetBufferByIndex(0, &buffer)) && SUCCEEDED(buffer->GetCurrentLength(&length))
            && length == picture_bytes && SUCCEEDED(buffer.As(&rows)) && SUCCEEDED(rows->Lock2D(&first_row, &pitch))) {
            if (pitch >= static_cast<LONG>(yuy2 ? type.width * 2 : type.width)) {
                ++findings->samples;
                ExamineLocked(findings, first_row, pitch, type, arrival, began, scratch);
                whole_picture = true;
            }
            rows->Unlock2D();
        }
        if (same_token && whole_picture) {
            ++good;
        }
        if (keep != nullptr) {
            keep->push_back(sample);
        }
    }
    return good;
}

// How many bytes the samples of an allocator hold right now, or 0 if nobody
// has made it ready. The stream is to make ready the allocator it is given,
// for the size it runs at, and to empty it when it stops; this is how the
// test sees that it did so to this allocator and not to one of its own.
DWORD BytesInItsSamples(IMFVideoSampleAllocator* allocator) {
    ComPtr<IMFSample> sample;
    ComPtr<IMFMediaBuffer> buffer;
    DWORD bytes = 0;
    if (FAILED(allocator->AllocateSample(&sample)) || FAILED(sample->GetBufferByIndex(0, &buffer))
        || FAILED(buffer->GetMaxLength(&bytes))) {
        return 0;
    }
    return bytes;
}

}  // namespace

int Drive(const std::wstring& dll) {
    Report report;
    Findings findings;
    std::vector<uint8_t> scratch;
    std::vector<ComPtr<IMFSample>> held;

    ComPtr<IMFActivate> activate;
    ComPtr<IMFMediaSource> source;
    const char* step = "";
    HRESULT result = OpenInProcess(dll, &activate, &source, &step);
    if (FAILED(result)) {
        std::printf(
            "{\"tool\":\"gc-vcam-probe\",\"ok\":false,\"mode\":\"drive\",\"step\":\"%s\",\"hresult\":\"0x%08lX\","
            "\"message\":\"The media source could not be loaded from the DLL.\"}\n",
            step, static_cast<unsigned long>(result));
        return 2;
    }

    // Everything from here on runs once; the first step that makes the rest
    // pointless ends it.
    do {
        ComPtr<IMFMediaSource> again;
        result = activate->ActivateObject(IID_PPV_ARGS(&again));
        report.Check("the activation object hands out the same source twice", SUCCEEDED(result) && again.Get() == source.Get(), result);
        again.Reset();

        ComPtr<IMFMediaSourceEx> extended;
        ComPtr<IMFSampleAllocatorControl> control;
        ComPtr<IKsControl> controls;
        ComPtr<IMFGetService> services;
        source.As(&extended);
        source.As(&control);
        source.As(&controls);
        source.As(&services);
        report.Check("the source has the interfaces the camera service asks for", extended && control && controls && services);
        if (!extended || !control || !controls || !services) {
            break;
        }

        KSPROPERTY property{};
        ULONG returned = 0;
        result = controls->KsProperty(&property, sizeof(property), nullptr, 0, &returned);
        report.Check("a camera control it does not have is answered as a driver would", result == HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND), result);
        void* nothing = nullptr;
        result = services->GetService(GUID_NULL, __uuidof(IUnknown), &nothing);
        report.Check("a service it does not have is refused", result == MF_E_UNSUPPORTED_SERVICE && nothing == nullptr, result);

        // The camera service gives the source an allocator before it starts it.
        DWORD input = 99;
        MFSampleAllocatorUsage usage = MFSampleAllocatorUsage_DoesNotAllocate;
        result = control->GetAllocatorUsage(0, &input, &usage);
        report.Check("it says it will use the allocator it is given", SUCCEEDED(result) && usage == MFSampleAllocatorUsage_UsesProvidedAllocator && input == 0, result);
        ComPtr<IMFVideoSampleAllocatorEx> allocator;
        result = MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator));
        if (SUCCEEDED(result)) {
            result = control->SetDefaultAllocator(0, allocator.Get());
        }
        report.Check("it takes an allocator", SUCCEEDED(result), result);
        if (FAILED(result)) {
            break;
        }
        report.Check("and leaves it alone until the camera starts", BytesInItsSamples(allocator.Get()) == 0);

        ComPtr<IMFPresentationDescriptor> descriptor;
        ComPtr<IMFStreamDescriptor> stream_descriptor;
        ComPtr<IMFMediaTypeHandler> handler;
        BOOL selected = FALSE;
        result = source->CreatePresentationDescriptor(&descriptor);
        if (SUCCEEDED(result)) {
            result = descriptor->SelectStream(0);
        }
        if (SUCCEEDED(result)) {
            result = descriptor->GetStreamDescriptorByIndex(0, &selected, &stream_descriptor);
        }
        if (SUCCEEDED(result)) {
            result = stream_descriptor->GetMediaTypeHandler(&handler);
        }
        report.Check("it describes itself", SUCCEEDED(result) && selected, result);
        if (FAILED(result)) {
            break;
        }
        const ComPtr<IMFMediaType> type_720 = PickType(handler.Get(), 1280, 720, MFVideoFormat_NV12);
        const ComPtr<IMFMediaType> type_1080 = PickType(handler.Get(), 1920, 1080, MFVideoFormat_NV12);
        const ComPtr<IMFMediaType> type_yuy2 = PickType(handler.Get(), 1280, 720, MFVideoFormat_YUY2);
        report.Check("it offers 1280x720 and 1920x1080 as NV12, and YUY2", type_720 && type_1080 && type_yuy2);
        if (!type_720 || !type_1080 || !type_yuy2) {
            break;
        }

        // Start at the smaller size.
        PROPVARIANT from_now;
        PropVariantInit(&from_now);
        handler->SetCurrentMediaType(type_720.Get());
        result = source->Start(descriptor.Get(), nullptr, &from_now);
        report.Check("it starts", SUCCEEDED(result), result);
        ComPtr<IMFMediaStream> stream;
        ObjectOf(NextEvent(source.Get(), MENewStream, kPatience).Get()).As(&stream);
        report.Check("the source announces its stream", stream != nullptr);
        report.Check("the source says it has started", NextEvent(source.Get(), MESourceStarted, kPatience) != nullptr);
        if (!stream) {
            break;
        }
        report.Check("the stream says it has started", NextEvent(stream.Get(), MEStreamStarted, kPatience) != nullptr);
        ComPtr<IMFMediaStream2> stream_state;
        stream.As(&stream_state);
        report.Check("the stream can be paused and stopped by itself", stream_state != nullptr);
        if (!stream_state) {
            break;
        }

        const TypeInfo info_720 = Describe(type_720.Get());
        const TypeInfo info_1080 = Describe(type_1080.Get());
        const TypeInfo info_yuy2 = Describe(type_yuy2.Get());
        report.Check(
            "thirty requests are each answered with their token and a whole picture",
            ReadSamples(stream.Get(), info_720, 30, &findings, &scratch, nullptr) == 30);

        const DWORD bytes_720 = gc_vcam_nv12_size(1280, 720);
        const DWORD bytes_1080 = gc_vcam_nv12_size(1920, 1080);
        DWORD ready_for = BytesInItsSamples(allocator.Get());
        report.Check("the stream made the given allocator ready for 1280x720", ready_for >= bytes_720 && ready_for < bytes_1080);

        // Pause, stop and start the stream alone, as the camera service may.
        result = stream_state->SetStreamState(MF_STREAM_STATE_PAUSED);
        report.Check("the stream pauses", SUCCEEDED(result), result);
        result = stream->RequestSample(nullptr);
        report.Check("a paused stream turns requests away", result == MF_E_INVALIDREQUEST, result);
        result = stream_state->SetStreamState(MF_STREAM_STATE_RUNNING);
        report.Check(
            "and delivers again when it runs",
            SUCCEEDED(result) && ReadSamples(stream.Get(), info_720, 3, &findings, &scratch, nullptr) == 3, result);
        result = stream_state->SetStreamState(MF_STREAM_STATE_STOPPED);
        MF_STREAM_STATE state = MF_STREAM_STATE_RUNNING;
        stream_state->GetStreamState(&state);
        report.Check("the stream stops by itself", SUCCEEDED(result) && state == MF_STREAM_STATE_STOPPED, result);
        result = stream_state->SetStreamState(MF_STREAM_STATE_RUNNING);
        report.Check(
            "and delivers again when it runs once more",
            SUCCEEDED(result) && ReadSamples(stream.Get(), info_720, 3, &findings, &scratch, nullptr) == 3, result);

        // Another size, and then another format, while pictures flow.
        handler->SetCurrentMediaType(type_1080.Get());
        result = source->Start(descriptor.Get(), nullptr, &from_now);
        report.Check("it takes a larger size while running", SUCCEEDED(result), result);
        report.Check("the source says its stream has changed", NextEvent(source.Get(), MEUpdatedStream, kPatience) != nullptr);
        report.Check(
            "fifteen pictures of the larger size follow",
            ReadSamples(stream.Get(), info_1080, 15, &findings, &scratch, nullptr) == 15);
        ready_for = BytesInItsSamples(allocator.Get());
        report.Check("the given allocator was made ready again, for 1920x1080", ready_for >= bytes_1080);
        handler->SetCurrentMediaType(type_yuy2.Get());
        result = source->Start(descriptor.Get(), nullptr, &from_now);
        report.Check(
            "it takes YUY2 while running, and ten pictures follow",
            SUCCEEDED(result) && ReadSamples(stream.Get(), info_yuy2, 10, &findings, &scratch, nullptr) == 10, result);

        // Stop while a reader still holds two pictures.
        ReadSamples(stream.Get(), info_yuy2, 2, &findings, &scratch, &held);
        result = source->Stop();
        report.Check("it stops", SUCCEEDED(result), result);
        report.Check("the stream says it has stopped", NextEvent(stream.Get(), MEStreamStopped, kPatience) != nullptr);
        report.Check("the source says it has stopped", NextEvent(source.Get(), MESourceStopped, kPatience) != nullptr);
        result = stream->RequestSample(nullptr);
        report.Check("a stopped stream turns requests away", result == MF_E_INVALIDREQUEST, result);
        report.Check("the given allocator is emptied when the camera stops", BytesInItsSamples(allocator.Get()) == 0);
        held.clear();

        // Start once more: the same stream is announced again.
        handler->SetCurrentMediaType(type_720.Get());
        result = source->Start(descriptor.Get(), nullptr, &from_now);
        ComPtr<IMFMediaStream> announced;
        ObjectOf(NextEvent(source.Get(), MENewStream, kPatience).Get()).As(&announced);
        report.Check("a new start announces the same stream again", SUCCEEDED(result) && announced.Get() == stream.Get(), result);
        report.Check(
            "and five pictures follow", ReadSamples(stream.Get(), info_720, 5, &findings, &scratch, nullptr) == 5);

        // Shut down. Everything says so afterwards, and nothing hangs.
        result = source->Shutdown();
        report.Check("it shuts down", SUCCEEDED(result), result);
        ComPtr<IMFPresentationDescriptor> none;
        result = source->CreatePresentationDescriptor(&none);
        report.Check("a shut-down source says so", result == MF_E_SHUTDOWN, result);
        result = stream->RequestSample(nullptr);
        report.Check("a shut-down stream says so", result == MF_E_SHUTDOWN, result);
        activate->ShutdownObject();
        ComPtr<IMFMediaSource> fresh;
        result = activate->ActivateObject(IID_PPV_ARGS(&fresh));
        report.Check("the activation object then makes a new source", SUCCEEDED(result) && fresh && fresh.Get() != source.Get(), result);
        if (fresh) {
            fresh->Shutdown();
        }
        report.Check("every picture was the host's or the slate", findings.others == 0);
        report.Check("every colour was right", findings.colours_wrong == 0);
    } while (false);

    held.clear();
    source->Shutdown();
    activate->ShutdownObject();

    std::string out;
    Append(&out, "{\"tool\":\"gc-vcam-probe\",\"ok\":%s,\"mode\":\"drive\",\"failed\":%d,", report.failed == 0 ? "true" : "false", report.failed);
    Append(
        &out, "\"pictures\":{\"frames\":%llu,\"repeats\":%llu,\"slates\":%llu,\"others\":%llu,\"colours_wrong\":%llu},",
        static_cast<unsigned long long>(findings.new_frames), static_cast<unsigned long long>(findings.repeats),
        static_cast<unsigned long long>(findings.slates), static_cast<unsigned long long>(findings.others),
        static_cast<unsigned long long>(findings.colours_wrong));
    out += "\"steps\":[";
    out += report.steps;
    out += "]}";
    std::puts(out.c_str());
    std::fflush(stdout);
    return report.failed == 0 ? 0 : 2;
}

}  // namespace probe
