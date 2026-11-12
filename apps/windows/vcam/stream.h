// The camera's one video stream.
//
// A reading app asks for one sample at a time. The stream answers each
// request when the next picture is due: the app's newest picture as soon as
// it arrives, its last picture again if the next one is late, and the
// source's own slate when the app has stopped sending. Requests are answered
// from a thread of the stream's own, so a caller is never kept waiting.
#pragma once

#include "frame_feed.h"
#include "pch.h"

class Stream final
    : public Microsoft::WRL::RuntimeClass<
          Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>,
          Microsoft::WRL::ChainInterfaces<IMFMediaStream2, IMFMediaStream, IMFMediaEventGenerator>, IKsControl,
          Microsoft::WRL::FtmBase> {
public:
    Stream() = default;
    ~Stream() override;

    HRESULT RuntimeClassInitialize(IMFMediaSource* parent, DWORD stream_id, const std::wstring& pipe_name);

    // IMFMediaEventGenerator
    IFACEMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override;
    IFACEMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override;
    IFACEMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override;
    IFACEMETHODIMP QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status, const PROPVARIANT* payload) override;

    // IMFMediaStream
    IFACEMETHODIMP GetMediaSource(IMFMediaSource** source) override;
    IFACEMETHODIMP GetStreamDescriptor(IMFStreamDescriptor** descriptor) override;
    IFACEMETHODIMP RequestSample(IUnknown* token) override;

    // IMFMediaStream2
    IFACEMETHODIMP SetStreamState(MF_STREAM_STATE state) override;
    IFACEMETHODIMP GetStreamState(MF_STREAM_STATE* state) override;

    // IKsControl: the stream has no controls of its own.
    IFACEMETHODIMP KsProperty(PKSPROPERTY property, ULONG property_length, LPVOID data, ULONG data_length, ULONG* returned) override;
    IFACEMETHODIMP KsMethod(PKSMETHOD method, ULONG method_length, LPVOID data, ULONG data_length, ULONG* returned) override;
    IFACEMETHODIMP KsEvent(PKSEVENT event, ULONG event_length, LPVOID data, ULONG data_length, ULONG* returned) override;

    // For the source.
    DWORD Id() const { return stream_id_; }
    HRESULT Start(IMFMediaType* type);
    HRESULT Stop(bool send_event);
    void Shutdown();
    HRESULT SetAllocator(IUnknown* allocator);
    HRESULT CopyAttributes(IMFAttributes** attributes);

private:
    // Tokens of samples asked for and not yet sent. They are the caller's
    // objects, so they are let go of only once the lock is released.
    using Tokens = std::deque<Microsoft::WRL::ComPtr<IUnknown>>;

    HRESULT CheckShutdown() const;
    HRESULT StartLocked(bool send_event, IMFMediaType* new_type);
    void StopLocked(bool send_event, Tokens* dropped);
    HRESULT EnsureDeliveryThread();
    void DeliveryLoop();
    int64_t DeliverIfDue(Microsoft::WRL::ComPtr<IMFSample>* sent, Microsoft::WRL::ComPtr<IUnknown>* answered);
    HRESULT FillSample(IMFSample* sample, bool from_app, UINT32* kind, FrameFeed::Info* info);

    LiveObject live_;
    mutable std::mutex lock_;
    Microsoft::WRL::ComPtr<IMFMediaSource> parent_;
    Microsoft::WRL::ComPtr<IMFMediaEventQueue> events_;
    Microsoft::WRL::ComPtr<IMFAttributes> attributes_;
    Microsoft::WRL::ComPtr<IMFStreamDescriptor> descriptor_;
    Microsoft::WRL::ComPtr<IMFMediaType> type_;
    Microsoft::WRL::ComPtr<IMFVideoSampleAllocator> allocator_;
    bool allocator_ready_ = false;  // initialised for `type_`
    DWORD stream_id_ = 0;
    std::wstring pipe_name_;
    MF_STREAM_STATE state_ = MF_STREAM_STATE_STOPPED;
    bool shut_down_ = false;

    uint32_t width_ = 0;
    uint32_t height_ = 0;
    bool yuy2_ = false;                // the reader asked for YUY2 instead of NV12
    int64_t frame_duration_ = 333333;  // 100 ns units: thirty a second

    FrameFeed feed_;
    std::vector<uint8_t> slate_;    // the source's own picture, as NV12 at the current size
    Tokens requests_;
    uint64_t last_sequence_ = 0;    // the app's frame sent last
    int64_t last_delivery_ = 0;     // when the last sample went out; 0 before the first
    int64_t last_sample_time_ = 0;  // the time written on the last sample
    int64_t started_at_ = 0;

    // The delivery thread holds a reference to the stream while it runs and
    // ends when the stream is shut down.
    gc::Handle wake_;               // a request arrived, or the thread should look again
    bool delivery_started_ = false;
};
