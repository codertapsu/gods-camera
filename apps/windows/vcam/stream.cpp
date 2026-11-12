#include "stream.h"

using Microsoft::WRL::ComPtr;

namespace {

constexpr int64_t kMillisecond = 10000;  // in 100 ns units
constexpr int64_t kSlateAfter = GC_VCAM_SLATE_AFTER_MS * kMillisecond;
constexpr int64_t kFirstFrameGrace = GC_VCAM_FIRST_FRAME_GRACE_MS * kMillisecond;
constexpr DWORD kSamplesInAllocator = 10;

// The picture sizes the camera offers, the first being the one it starts with.
constexpr struct {
    uint32_t width;
    uint32_t height;
} kSizes[] = {{1280, 720}, {1920, 1080}};

// A media type of one format and size at thirty frames a second. NV12 is
// what the app sends and what most readers take. YUY2 is for older programs
// that know nothing else; the stream makes it from the NV12 picture.
HRESULT MakeType(bool yuy2, uint32_t width, uint32_t height, IMFMediaType** out) {
    ComPtr<IMFMediaType> type;
    RETURN_IF_FAILED(MFCreateMediaType(&type));
    RETURN_IF_FAILED(type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video));
    RETURN_IF_FAILED(type->SetGUID(MF_MT_SUBTYPE, yuy2 ? MFVideoFormat_YUY2 : MFVideoFormat_NV12));
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive));
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE));
    RETURN_IF_FAILED(MFSetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, width, height));
    RETURN_IF_FAILED(MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, 30, 1));
    RETURN_IF_FAILED(MFSetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1));
    // Bits a pixel (twelve for NV12, sixteen for YUY2), thirty times a second.
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_AVG_BITRATE, width * height * (yuy2 ? 16u : 12u) * 30u));
    // High-definition video: BT.709 colours in the studio range.
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235));
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709));
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709));
    RETURN_IF_FAILED(type->SetUINT32(MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709));
    *out = type.Detach();
    return S_OK;
}

// What the camera service looks for on a stream: that it is a capture pin
// carrying colour pictures, and that several apps may share it.
HRESULT SetStreamAttributes(IMFAttributes* attributes, DWORD stream_id) {
    RETURN_IF_FAILED(attributes->SetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, PINNAME_VIDEO_CAPTURE));
    RETURN_IF_FAILED(attributes->SetUINT32(MF_DEVICESTREAM_STREAM_ID, stream_id));
    RETURN_IF_FAILED(attributes->SetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, 1));
    RETURN_IF_FAILED(attributes->SetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color));
    return S_OK;
}

// Write an NV12 picture, rows `width` bytes apart, into a buffer of the
// stream's format whose rows are `pitch` bytes apart.
void WritePicture(const uint8_t* nv12, uint32_t width, uint32_t height, bool yuy2, BYTE* first_row, LONG pitch) {
    if (!yuy2) {
        // Luma, then colour at half the height: both `width` bytes a row.
        const uint32_t rows = height + height / 2;
        for (uint32_t row = 0; row < rows; ++row) {
            std::memcpy(first_row + static_cast<size_t>(row) * pitch, nv12 + static_cast<size_t>(row) * width, width);
        }
        return;
    }
    // NV12 has one row of colour for every two rows of picture. YUY2 wants
    // one for each, so every row of colour is used twice.
    const uint8_t* colour = nv12 + static_cast<size_t>(width) * height;
    for (uint32_t y = 0; y < height; ++y) {
        const uint8_t* luma_row = nv12 + static_cast<size_t>(y) * width;
        const uint8_t* colour_row = colour + static_cast<size_t>(y / 2) * width;
        BYTE* out = first_row + static_cast<size_t>(y) * pitch;
        for (uint32_t x = 0; x < width; x += 2) {
            out[0] = luma_row[x];
            out[1] = colour_row[x];
            out[2] = luma_row[x + 1];
            out[3] = colour_row[x + 1];
            out += 4;
        }
    }
}

// What DeliverIfDue answers when there is nothing to wait for but the next event.
constexpr int64_t kUntilWoken = -1;
// How soon to look again when a sample could not be had: five milliseconds.
constexpr int64_t kLookAgainSoon = 5 * kMillisecond;

}  // namespace

Stream::~Stream() {
    Shutdown();
}

HRESULT Stream::RuntimeClassInitialize(IMFMediaSource* parent, DWORD stream_id, const std::wstring& pipe_name) {
    if (parent == nullptr) {
        return E_INVALIDARG;
    }
    std::lock_guard<std::mutex> guard(lock_);
    parent_ = parent;
    stream_id_ = stream_id;
    pipe_name_ = pipe_name;
    wake_.reset(CreateEventW(nullptr, FALSE, FALSE, nullptr));
    if (!wake_.valid() || !feed_.Ready()) {
        return E_OUTOFMEMORY;
    }

    // Every size as NV12 first, then every size as YUY2.
    constexpr size_t kTypeCount = ARRAYSIZE(kSizes) * 2;
    ComPtr<IMFMediaType> types[kTypeCount];
    IMFMediaType* raw_types[kTypeCount] = {};
    for (size_t i = 0; i < kTypeCount; ++i) {
        const auto& size = kSizes[i % ARRAYSIZE(kSizes)];
        RETURN_IF_FAILED(MakeType(i >= ARRAYSIZE(kSizes), size.width, size.height, &types[i]));
        raw_types[i] = types[i].Get();
    }

    RETURN_IF_FAILED(MFCreateAttributes(&attributes_, 10));
    RETURN_IF_FAILED(SetStreamAttributes(attributes_.Get(), stream_id_));
    RETURN_IF_FAILED(MFCreateEventQueue(&events_));
    RETURN_IF_FAILED(MFCreateStreamDescriptor(stream_id_, static_cast<DWORD>(kTypeCount), raw_types, &descriptor_));

    ComPtr<IMFMediaTypeHandler> handler;
    RETURN_IF_FAILED(descriptor_->GetMediaTypeHandler(&handler));
    RETURN_IF_FAILED(handler->SetCurrentMediaType(types[0].Get()));
    RETURN_IF_FAILED(SetStreamAttributes(descriptor_.Get(), stream_id_));
    return S_OK;
}

HRESULT Stream::CheckShutdown() const {
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    return events_ ? S_OK : E_UNEXPECTED;
}

// IMFMediaEventGenerator

IFACEMETHODIMP Stream::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return events_->BeginGetEvent(callback, state);
}

IFACEMETHODIMP Stream::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return events_->EndGetEvent(result, event);
}

IFACEMETHODIMP Stream::GetEvent(DWORD flags, IMFMediaEvent** event) {
    // GetEvent may wait for as long as it likes, so the lock is not held over it.
    ComPtr<IMFMediaEventQueue> queue;
    {
        std::lock_guard<std::mutex> guard(lock_);
        RETURN_IF_FAILED(CheckShutdown());
        queue = events_;
    }
    return queue->GetEvent(flags, event);
}

IFACEMETHODIMP Stream::QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status, const PROPVARIANT* payload) {
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return events_->QueueEventParamVar(type, extended_type, status, payload);
}

// IMFMediaStream

IFACEMETHODIMP Stream::GetMediaSource(IMFMediaSource** source) {
    if (source == nullptr) {
        return E_POINTER;
    }
    *source = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return parent_.CopyTo(source);
}

IFACEMETHODIMP Stream::GetStreamDescriptor(IMFStreamDescriptor** descriptor) {
    if (descriptor == nullptr) {
        return E_POINTER;
    }
    *descriptor = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    if (!descriptor_) {
        return E_UNEXPECTED;
    }
    return descriptor_.CopyTo(descriptor);
}

IFACEMETHODIMP Stream::RequestSample(IUnknown* token) {
    return Guarded([&]() -> HRESULT {
        {
            std::lock_guard<std::mutex> guard(lock_);
            RETURN_IF_FAILED(CheckShutdown());
            if (state_ != MF_STREAM_STATE_RUNNING) {
                return MF_E_INVALIDREQUEST;
            }
            requests_.emplace_back(token);
        }
        SetEvent(wake_.get());
        return S_OK;
    });
}

// IMFMediaStream2

IFACEMETHODIMP Stream::SetStreamState(MF_STREAM_STATE state) {
    return Guarded([&]() -> HRESULT {
        Tokens dropped;  // let go of after the lock
        std::lock_guard<std::mutex> guard(lock_);
        RETURN_IF_FAILED(CheckShutdown());
        if (state_ == state) {
            return S_OK;
        }
        switch (state) {
            case MF_STREAM_STATE_PAUSED:
                if (state_ != MF_STREAM_STATE_RUNNING) {
                    return MF_E_INVALID_STATE_TRANSITION;
                }
                state_ = MF_STREAM_STATE_PAUSED;
                return S_OK;
            case MF_STREAM_STATE_RUNNING:
                return StartLocked(false, nullptr);
            case MF_STREAM_STATE_STOPPED:
                StopLocked(false, &dropped);
                return S_OK;
            default:
                return MF_E_INVALID_STATE_TRANSITION;
        }
    });
}

IFACEMETHODIMP Stream::GetStreamState(MF_STREAM_STATE* state) {
    if (state == nullptr) {
        return E_INVALIDARG;
    }
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    *state = state_;
    return S_OK;
}

// IKsControl. "Set not found" is what a camera driver answers for a control it does not have.

IFACEMETHODIMP Stream::KsProperty(PKSPROPERTY, ULONG, LPVOID, ULONG, ULONG*) {
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

IFACEMETHODIMP Stream::KsMethod(PKSMETHOD, ULONG, LPVOID, ULONG, ULONG*) {
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

IFACEMETHODIMP Stream::KsEvent(PKSEVENT, ULONG, LPVOID, ULONG, ULONG*) {
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

// For the source

HRESULT Stream::Start(IMFMediaType* type) {
    if (type == nullptr) {
        return E_INVALIDARG;
    }
    return Guarded([&]() -> HRESULT {
        std::lock_guard<std::mutex> guard(lock_);
        RETURN_IF_FAILED(CheckShutdown());
        return StartLocked(true, type);
    });
}

HRESULT Stream::Stop(bool send_event) {
    Tokens dropped;  // let go of after the lock
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    StopLocked(send_event, &dropped);
    return S_OK;
}

void Stream::Shutdown() {
    // Everything that belongs to someone else is let go of after the lock.
    Tokens dropped;
    ComPtr<IMFMediaSource> parent;
    ComPtr<IMFMediaEventQueue> events;
    ComPtr<IMFVideoSampleAllocator> allocator;
    {
        std::lock_guard<std::mutex> guard(lock_);
        if (shut_down_) {
            return;
        }
        shut_down_ = true;
        state_ = MF_STREAM_STATE_STOPPED;
        dropped.swap(requests_);
        feed_.Stop();
        if (events_) {
            events_->Shutdown();
        }
        events.Swap(events_);
        parent.Swap(parent_);
        if (allocator_ && allocator_ready_) {
            allocator_->UninitializeSampleAllocator();
            allocator_ready_ = false;
        }
        allocator.Swap(allocator_);
        attributes_.Reset();
        descriptor_.Reset();
        type_.Reset();
    }
    // The delivery thread sees `shut_down_` and ends.
    if (wake_.valid()) {
        SetEvent(wake_.get());
    }
}

HRESULT Stream::SetAllocator(IUnknown* allocator) {
    if (allocator == nullptr) {
        return E_POINTER;
    }
    ComPtr<IMFVideoSampleAllocator> video_allocator;
    RETURN_IF_FAILED(allocator->QueryInterface(IID_PPV_ARGS(&video_allocator)));
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    if (state_ == MF_STREAM_STATE_RUNNING) {
        return MF_E_INVALIDREQUEST;
    }
    allocator_.Swap(video_allocator);  // the old one is let go of after the lock
    allocator_ready_ = false;
    return S_OK;
}

HRESULT Stream::CopyAttributes(IMFAttributes** attributes) {
    if (attributes == nullptr) {
        return E_POINTER;
    }
    *attributes = nullptr;
    std::lock_guard<std::mutex> guard(lock_);
    RETURN_IF_FAILED(CheckShutdown());
    return attributes_.CopyTo(attributes);
}

// Internals. The lock is held in all of these but the delivery loop.

HRESULT Stream::StartLocked(bool send_event, IMFMediaType* new_type) {
    ComPtr<IMFMediaType> type = new_type != nullptr ? new_type : type_.Get();
    if (!type) {
        ComPtr<IMFMediaTypeHandler> handler;
        RETURN_IF_FAILED(descriptor_->GetMediaTypeHandler(&handler));
        RETURN_IF_FAILED(handler->GetCurrentMediaType(&type));
    }
    GUID subtype{};
    UINT32 width = 0;
    UINT32 height = 0;
    RETURN_IF_FAILED(type->GetGUID(MF_MT_SUBTYPE, &subtype));
    RETURN_IF_FAILED(MFGetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, &width, &height));
    bool offered = false;
    for (const auto& size : kSizes) {
        offered = offered || (size.width == width && size.height == height);
    }
    const bool yuy2 = subtype == MFVideoFormat_YUY2;
    if ((subtype != MFVideoFormat_NV12 && !yuy2) || !offered) {
        return MF_E_INVALIDMEDIATYPE;
    }

    DWORD flags = 0;
    const bool same_type = type_ && (type_.Get() == type.Get() || type_->IsEqual(type.Get(), &flags) == S_OK);
    const bool new_size = width != width_ || height != height_;
    if (new_size || slate_.empty()) {
        std::vector<uint8_t> slate(gc_vcam_nv12_size(width, height));
        gc_slate::draw(slate.data(), width, height);
        slate_.swap(slate);
    }

    // The camera service hands the stream an allocator whose samples it can
    // pass on to other processes. A reader in this process hands over none,
    // and the stream makes its own. Either is made ready when the stream
    // starts and emptied when it stops.
    if (!allocator_) {
        RETURN_IF_FAILED(MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator_)));
        allocator_ready_ = false;
    }
    if (!allocator_ready_ || !same_type) {
        if (allocator_ready_) {
            allocator_->UninitializeSampleAllocator();
            allocator_ready_ = false;
        }
        RETURN_IF_FAILED(allocator_->InitializeSampleAllocator(kSamplesInAllocator, type.Get()));
        allocator_ready_ = true;
    }
    type_ = type;
    width_ = width;
    height_ = height;
    yuy2_ = yuy2;

    RETURN_IF_FAILED(EnsureDeliveryThread());
    feed_.Start(pipe_name_, width_, height_);
    if (state_ == MF_STREAM_STATE_STOPPED || new_size) {
        // Nothing of this size has come from the app yet. It gets a moment
        // to send before the slate shows, after a change of size as at a start.
        started_at_ = gc_vcam_now_100ns();
        last_delivery_ = 0;
        last_sequence_ = 0;
    }
    state_ = MF_STREAM_STATE_RUNNING;
    if (send_event) {
        RETURN_IF_FAILED(events_->QueueEventParamVar(MEStreamStarted, GUID_NULL, S_OK, nullptr));
    }
    SetEvent(wake_.get());
    DebugLine(L"stream running at %u x %u, %s", width_, height_, yuy2_ ? L"YUY2" : L"NV12");
    return S_OK;
}

void Stream::StopLocked(bool send_event, Tokens* dropped) {
    state_ = MF_STREAM_STATE_STOPPED;
    // Requests that were not answered are dropped, as a stopped camera's are.
    dropped->swap(requests_);
    // Leaving the pipe is how the app learns that nothing is watching.
    feed_.Stop();
    // A stopped camera keeps no pictures in memory. Samples a reader still
    // holds stay good until it lets go of them.
    if (allocator_ && allocator_ready_) {
        allocator_->UninitializeSampleAllocator();
        allocator_ready_ = false;
    }
    if (send_event && events_) {
        events_->QueueEventParamVar(MEStreamStopped, GUID_NULL, S_OK, nullptr);
    }
    DebugLine(L"stream stopped");
}

HRESULT Stream::EnsureDeliveryThread() {
    if (delivery_started_) {
        return S_OK;
    }
    PinModule();
    // The thread keeps the stream alive while it runs, and is left to end by
    // itself: nothing ever waits for it, so nothing can wait for it wrongly.
    ComPtr<Stream> self(this);
    std::thread([self]() {
        try {
            self->DeliveryLoop();
        } catch (...) {
            DebugLine(L"the delivery thread ended on an error");
        }
    }).detach();
    delivery_started_ = true;
    return S_OK;
}

void Stream::DeliveryLoop() {
    // An ordinary wait is only as fine as the system's clock tick, about 16 ms,
    // which is half a frame. This timer is good to well under a millisecond.
    gc::Handle timer(CreateWaitableTimerExW(nullptr, nullptr, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS));
    if (!timer.valid()) {
        timer.reset(CreateWaitableTimerExW(nullptr, nullptr, 0, TIMER_ALL_ACCESS));
    }
    const HANDLE waits[3] = {wake_.get(), feed_.NewFrameEvent(), timer.get()};
    const DWORD wait_count = timer.valid() ? 3 : 2;
    for (;;) {
        int64_t wait = kUntilWoken;
        // What a delivery hands back is the caller's, and is let go of after the lock.
        ComPtr<IMFSample> sent;
        ComPtr<IUnknown> answered;
        {
            std::lock_guard<std::mutex> guard(lock_);
            if (shut_down_) {
                return;
            }
            if (state_ == MF_STREAM_STATE_RUNNING && !requests_.empty()) {
                wait = DeliverIfDue(&sent, &answered);
            }
        }
        sent.Reset();
        answered.Reset();

        DWORD timeout = INFINITE;
        if (wait != kUntilWoken) {
            if (timer.valid()) {
                LARGE_INTEGER after{};
                after.QuadPart = -(wait < 1 ? 1 : wait);  // negative: from now
                SetWaitableTimer(timer.get(), &after, 0, nullptr, nullptr, FALSE);
            } else {
                timeout = static_cast<DWORD>((wait + kMillisecond - 1) / kMillisecond);
            }
        } else if (timer.valid()) {
            CancelWaitableTimer(timer.get());
        }
        WaitForMultipleObjects(wait_count, waits, FALSE, timeout);
    }
}

// Answer the oldest request if a picture is due. Returns how long to wait
// before looking again, in 100 ns units, or kUntilWoken.
int64_t Stream::DeliverIfDue(ComPtr<IMFSample>* sent, ComPtr<IUnknown>* answered) {
    const int64_t now = gc_vcam_now_100ns();
    const FrameFeed::Info latest = feed_.Latest();
    const bool usable = latest.sequence != 0 && latest.width == width_ && latest.height == height_;
    const bool fresh = usable && latest.sequence != last_sequence_;
    const bool sending = usable && now - latest.arrived_100ns < kSlateAfter;

    int64_t due = now;
    if (last_delivery_ == 0) {
        // Just started: the app gets a moment to connect before the slate shows.
        due = fresh ? now : started_at_ + kFirstFrameGrace;
    } else if (fresh) {
        // Never much faster than the frame rate, however fast the app sends.
        due = last_delivery_ + frame_duration_ * 3 / 4;
    } else if (sending) {
        // The app's next picture may be a little late: wait half a frame
        // longer before showing the last one again.
        due = last_delivery_ + frame_duration_ * 3 / 2;
    } else {
        // The app is silent: the slate, at the plain frame rate.
        due = last_delivery_ + frame_duration_;
    }
    if (now < due) {
        return due - now;
    }

    ComPtr<IMFSample> sample;
    if (FAILED(allocator_->AllocateSample(&sample))) {
        return kLookAgainSoon;  // every sample is still with the reader
    }
    UINT32 kind = GC_VCAM_SAMPLE_SLATE;
    FrameFeed::Info info;
    if (FAILED(FillSample(sample.Get(), fresh || sending, &kind, &info))) {
        sent->Swap(sample);
        return kLookAgainSoon;
    }

    // Media Foundation's clock now, less the picture's age: the time the app
    // made it. Times on samples never run backwards.
    int64_t time = MFGetSystemTime();
    if (kind == GC_VCAM_SAMPLE_FRAME) {
        const int64_t age = now - info.time_100ns;
        if (age > 0 && age < 10000 * kMillisecond) {
            time -= age;
        }
    }
    if (time <= last_sample_time_) {
        time = last_sample_time_ + 1;
    }
    last_sample_time_ = time;
    sample->SetSampleTime(time);
    sample->SetSampleDuration(frame_duration_);
    sample->SetUINT64(MFSampleExtension_DeviceTimestamp, static_cast<UINT64>(time));
    sample->SetUINT64(GC_VCAM_SAMPLE_SEQUENCE, kind == GC_VCAM_SAMPLE_SLATE ? 0 : info.sequence);
    sample->SetUINT32(GC_VCAM_SAMPLE_KIND, kind);

    answered->Swap(requests_.front());
    requests_.pop_front();
    if (*answered) {
        sample->SetUnknown(MFSampleExtension_Token, answered->Get());
    }
    events_->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.Get());
    sent->Swap(sample);
    if (kind == GC_VCAM_SAMPLE_FRAME) {
        last_sequence_ = info.sequence;
        last_delivery_ = now;
    } else {
        // The slate and the repeats go by the clock. Counting from the moment
        // each was due, not the moment it left, keeps them at the frame rate.
        last_delivery_ = last_delivery_ != 0 && now - due < frame_duration_ ? due : now;
    }
    // A further request is answered when the next picture comes, or is due.
    return requests_.empty() ? kUntilWoken : frame_duration_ * 3 / 4;
}

// Put the picture that is due into a sample: the app's newest, or the slate.
HRESULT Stream::FillSample(IMFSample* sample, bool from_app, UINT32* kind, FrameFeed::Info* info) {
    ComPtr<IMFMediaBuffer> buffer;
    RETURN_IF_FAILED(sample->GetBufferByIndex(0, &buffer));
    ComPtr<IMF2DBuffer2> rows;
    BYTE* first_row = nullptr;
    BYTE* buffer_start = nullptr;
    LONG pitch = 0;
    DWORD length = 0;
    const bool by_rows = SUCCEEDED(buffer.As(&rows))
        && SUCCEEDED(rows->Lock2DSize(MF2DBuffer_LockFlags_Write, &first_row, &pitch, &buffer_start, &length));
    // NV12: a byte a pixel, and half as many rows again for the colour.
    // YUY2: two bytes a pixel, colour and all.
    const LONG row_bytes = static_cast<LONG>(yuy2_ ? width_ * 2 : width_);
    const uint32_t picture_rows = yuy2_ ? height_ : height_ + height_ / 2;
    if (!by_rows) {
        DWORD capacity = 0;
        RETURN_IF_FAILED(buffer->Lock(&first_row, &capacity, nullptr));
        pitch = row_bytes;
        length = capacity;
    }

    HRESULT result = S_OK;
    const auto write = [&](const uint8_t* nv12) { WritePicture(nv12, width_, height_, yuy2_, first_row, pitch); };
    if (pitch < row_bytes || length < static_cast<uint64_t>(pitch) * picture_rows) {
        // Also turns away a picture stored bottom row first, which neither format is.
        result = MF_E_BUFFERTOOSMALL;
    } else if (from_app && feed_.WithLatest(width_, height_, info, write)) {
        *kind = info->sequence != last_sequence_ ? GC_VCAM_SAMPLE_FRAME : GC_VCAM_SAMPLE_REPEAT;
    } else {
        write(slate_.data());
        *kind = GC_VCAM_SAMPLE_SLATE;
        *info = FrameFeed::Info{};
    }

    if (by_rows) {
        rows->Unlock2D();
    } else {
        buffer->Unlock();
    }
    if (SUCCEEDED(result)) {
        buffer->SetCurrentLength(static_cast<DWORD>(row_bytes) * picture_rows);
    }
    return result;
}
