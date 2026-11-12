#include "source.h"

using Microsoft::WRL::ComPtr;

namespace {

constexpr DWORD kStreamId = 0;

// The pipe name the app put on its virtual camera, if it is one of ours.
std::wstring PipeNameFrom(IMFAttributes* attributes) {
    UINT32 length = 0;
    if (FAILED(attributes->GetStringLength(GC_VCAM_ATTRIBUTE_PIPE_NAME, &length)) || length == 0 || length > 255) {
        return {};
    }
    std::wstring name(static_cast<size_t>(length) + 1, L'\0');
    if (FAILED(attributes->GetString(GC_VCAM_ATTRIBUTE_PIPE_NAME, name.data(), length + 1, nullptr))) {
        return {};
    }
    name.resize(length);
    if (gc::sid_in_pipe_name(name).empty()) {
        DebugLine(L"the pipe name on the camera is not one of this app's; ignoring it");
        return {};
    }
    return name;
}

// Windows asks every camera which combinations of streams and rates it
// supports, as "profiles". The one every camera must have is the legacy
// profile, for apps that know nothing of profiles: here, the one stream at
// any size, up to thirty frames a second.
HRESULT AddSensorProfiles(IMFAttributes* attributes) {
    ComPtr<IMFSensorProfileCollection> profiles;
    ComPtr<IMFSensorProfile> legacy;
    RETURN_IF_FAILED(MFCreateSensorProfileCollection(&profiles));
    RETURN_IF_FAILED(MFCreateSensorProfile(KSCAMERAPROFILE_Legacy, 0, nullptr, &legacy));
    RETURN_IF_FAILED(legacy->AddProfileFilter(kStreamId, L"((RES==;FRT<=30,1;SUT==))"));
    RETURN_IF_FAILED(profiles->AddProfile(legacy.Get()));
    return attributes->SetUnknown(MF_DEVICEMFT_SENSORPROFILE_COLLECTION, profiles.Get());
}

}  // namespace

HRESULT Source::RuntimeClassInitialize(IMFAttributes* activate_attributes) {
    const HRESULT result = Guarded([&]() -> HRESULT { return Initialize(activate_attributes); });
    if (FAILED(result)) {
        // The stream and the source hold on to each other; this lets go.
        Shutdown();
    }
    return result;
}

HRESULT Source::Initialize(IMFAttributes* activate_attributes) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(MFCreateAttributes(&attributes_, 8));
    if (activate_attributes != nullptr) {
        RETURN_IF_FAILED(activate_attributes->CopyAllItems(attributes_.Get()));
    }
    RETURN_IF_FAILED(AddSensorProfiles(attributes_.Get()));

    // Inside the camera service the app says which pipe is its own. A test
    // that loads the source into its own process says nothing, and gets the
    // pipe of the user it runs as.
    std::wstring pipe_name = PipeNameFrom(attributes_.Get());
    const bool named_by_app = !pipe_name.empty();
    if (!named_by_app) {
        pipe_name = gc::pipe_name_for(gc::current_user_sid());
    }

    RETURN_IF_FAILED(MFCreateEventQueue(&events_));
    RETURN_IF_FAILED(Microsoft::WRL::MakeAndInitialize<Stream>(
        &stream_, static_cast<IMFMediaSource*>(this), kStreamId, pipe_name, named_by_app));

    ComPtr<IMFStreamDescriptor> stream_descriptor;
    RETURN_IF_FAILED(stream_->GetStreamDescriptor(&stream_descriptor));
    IMFStreamDescriptor* descriptors[1] = {stream_descriptor.Get()};
    RETURN_IF_FAILED(MFCreatePresentationDescriptor(1, descriptors, &descriptor_));
    return S_OK;
}

HRESULT Source::CheckShutdown() const {
    if (state_ == State::Shutdown) {
        return MF_E_SHUTDOWN;
    }
    return events_ && stream_ && descriptor_ ? S_OK : E_UNEXPECTED;
}

// IMFMediaEventGenerator

IFACEMETHODIMP Source::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return events_->BeginGetEvent(callback, state);
}

IFACEMETHODIMP Source::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return events_->EndGetEvent(result, event);
}

IFACEMETHODIMP Source::GetEvent(DWORD flags, IMFMediaEvent** event) {
    // GetEvent may wait for as long as it likes, so the lock is not held over it.
    ComPtr<IMFMediaEventQueue> queue;
    {
        std::lock_guard<std::mutex> guard(lock_);
        RETURN_IF_FAILED(CheckShutdown());
        queue = events_;
    }
    return queue->GetEvent(flags, event);
}

IFACEMETHODIMP Source::QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status, const PROPVARIANT* payload) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return events_->QueueEventParamVar(type, extended_type, status, payload);
}

// IMFMediaSource

IFACEMETHODIMP Source::CreatePresentationDescriptor(IMFPresentationDescriptor** descriptor) {
    if (descriptor == nullptr) {
        return E_POINTER;
    }
    *descriptor = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return descriptor_->Clone(descriptor);
}

IFACEMETHODIMP Source::GetCharacteristics(DWORD* characteristics) {
    if (characteristics == nullptr) {
        return E_POINTER;
    }
    *characteristics = 0;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    *characteristics = MFMEDIASOURCE_IS_LIVE;
    return S_OK;
}

IFACEMETHODIMP Source::Pause() {
    // A camera does not pause.
    return MF_E_INVALID_STATE_TRANSITION;
}

IFACEMETHODIMP Source::Shutdown() {
    // Everything is let go of after the lock.
    ComPtr<Stream> stream;
    ComPtr<IMFMediaEventQueue> events;
    ComPtr<IMFPresentationDescriptor> descriptor;
    ComPtr<IMFAttributes> attributes;
    {
        std::lock_guard<std::mutex> guard(lock_);
        state_ = State::Shutdown;
        if (events_) {
            events_->Shutdown();
        }
        events.Swap(events_);
        stream.Swap(stream_);
        descriptor.Swap(descriptor_);
        attributes.Swap(attributes_);
    }
    if (stream) {
        stream->Shutdown();
    }
    return S_OK;
}

IFACEMETHODIMP Source::Start(
    IMFPresentationDescriptor* descriptor, const GUID* time_format, const PROPVARIANT* start_position) {
    if (descriptor == nullptr || start_position == nullptr) {
        return E_INVALIDARG;
    }
    if (time_format != nullptr && *time_format != GUID_NULL) {
        return MF_E_UNSUPPORTED_TIME_FORMAT;
    }
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());

    // The caller's description must be of this source: one stream, this one.
    DWORD count = 0;
    RETURN_IF_FAILED(descriptor->GetStreamDescriptorCount(&count));
    if (count != 1) {
        return E_INVALIDARG;
    }
    BOOL selected = FALSE;
    ComPtr<IMFStreamDescriptor> theirs;
    RETURN_IF_FAILED(descriptor->GetStreamDescriptorByIndex(0, &selected, &theirs));
    DWORD stream_id = 0;
    RETURN_IF_FAILED(theirs->GetStreamIdentifier(&stream_id));
    if (stream_id != stream_->Id()) {
        return MF_E_NOT_FOUND;
    }
    BOOL was_selected = FALSE;
    ComPtr<IMFStreamDescriptor> ours;
    RETURN_IF_FAILED(descriptor_->GetStreamDescriptorByIndex(0, &was_selected, &ours));

    if (selected) {
        RETURN_IF_FAILED(descriptor_->SelectStream(0));
        ComPtr<IMFMediaTypeHandler> handler;
        ComPtr<IMFMediaType> type;
        RETURN_IF_FAILED(theirs->GetMediaTypeHandler(&handler));
        RETURN_IF_FAILED(handler->GetCurrentMediaType(&type));
        // The stream is announced first, then started.
        IUnknown* stream = static_cast<IMFMediaStream2*>(stream_.Get());
        RETURN_IF_FAILED(events_->QueueEventParamUnk(was_selected ? MEUpdatedStream : MENewStream, GUID_NULL, S_OK, stream));
        RETURN_IF_FAILED(stream_->Start(type.Get()));
    } else if (was_selected) {
        RETURN_IF_FAILED(descriptor_->DeselectStream(0));
        RETURN_IF_FAILED(stream_->Stop(false));
    }

    PROPVARIANT started_at;
    RETURN_IF_FAILED(InitPropVariantFromInt64(MFGetSystemTime(), &started_at));
    RETURN_IF_FAILED(events_->QueueEventParamVar(MESourceStarted, GUID_NULL, S_OK, &started_at));
    state_ = State::Started;
    return S_OK;
}

IFACEMETHODIMP Source::Stop() {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    if (state_ != State::Started) {
        return MF_E_INVALID_STATE_TRANSITION;
    }
    state_ = State::Stopped;
    RETURN_IF_FAILED(stream_->Stop(true));
    RETURN_IF_FAILED(descriptor_->DeselectStream(0));
    PROPVARIANT stopped_at;
    RETURN_IF_FAILED(InitPropVariantFromInt64(MFGetSystemTime(), &stopped_at));
    return events_->QueueEventParamVar(MESourceStopped, GUID_NULL, S_OK, &stopped_at);
}

// IMFMediaSourceEx

IFACEMETHODIMP Source::GetSourceAttributes(IMFAttributes** attributes) {
    if (attributes == nullptr) {
        return E_POINTER;
    }
    *attributes = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    // The camera service wants the store itself, not a copy of it.
    return attributes_.CopyTo(attributes);
}

IFACEMETHODIMP Source::GetStreamAttributes(DWORD stream_id, IMFAttributes** attributes) {
    if (attributes == nullptr) {
        return E_POINTER;
    }
    *attributes = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    if (stream_id != stream_->Id()) {
        return MF_E_NOT_FOUND;
    }
    return stream_->CopyAttributes(attributes);
}

IFACEMETHODIMP Source::SetD3DManager(IUnknown*) {
    // Pictures are handed over in ordinary memory. The caller ignores the answer.
    return E_NOTIMPL;
}

// IMFGetService

IFACEMETHODIMP Source::GetService(REFGUID, REFIID, LPVOID* object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    *object = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return MF_E_UNSUPPORTED_SERVICE;
}

// IKsControl. "Set not found" is what a camera driver answers for a control it does not have.

IFACEMETHODIMP Source::KsProperty(PKSPROPERTY property, ULONG property_length, LPVOID, ULONG, ULONG*) {
    if (property == nullptr || property_length < sizeof(KSPROPERTY)) {
        return E_INVALIDARG;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

IFACEMETHODIMP Source::KsMethod(PKSMETHOD, ULONG, LPVOID, ULONG, ULONG*) {
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

IFACEMETHODIMP Source::KsEvent(PKSEVENT, ULONG, LPVOID, ULONG, ULONG*) {
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

// IMFSampleAllocatorControl

IFACEMETHODIMP Source::SetDefaultAllocator(DWORD output_stream_id, IUnknown* allocator) {
    if (allocator == nullptr) {
        return E_POINTER;
    }
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    if (output_stream_id != stream_->Id()) {
        return MF_E_NOT_FOUND;
    }
    return stream_->SetAllocator(allocator);
}

IFACEMETHODIMP Source::GetAllocatorUsage(DWORD output_stream_id, DWORD* input_stream_id, MFSampleAllocatorUsage* usage) {
    if (input_stream_id == nullptr || usage == nullptr) {
        return E_POINTER;
    }
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    if (output_stream_id != stream_->Id()) {
        return MF_E_NOT_FOUND;
    }
    // The stream fills the samples of the allocator the camera service gives it.
    *usage = MFSampleAllocatorUsage_UsesProvidedAllocator;
    *input_stream_id = output_stream_id;
    return S_OK;
}
