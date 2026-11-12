// The media source: what the camera service sees as the camera.
//
// It has one video stream. It does no work of its own until it is started:
// the camera service makes it, reads its description and throws it away many
// times over, and none of that may cost anything.
#pragma once

#include "pch.h"
#include "stream.h"

class Source final
    : public Microsoft::WRL::RuntimeClass<
          Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>,
          Microsoft::WRL::ChainInterfaces<IMFMediaSourceEx, IMFMediaSource, IMFMediaEventGenerator>, IMFGetService,
          IKsControl, IMFSampleAllocatorControl, Microsoft::WRL::FtmBase> {
public:
    Source() = default;

    // `activate_attributes` are what the camera service set on the object
    // that made this source, among them what the app set on its virtual camera.
    HRESULT RuntimeClassInitialize(IMFAttributes* activate_attributes);

    // IMFMediaEventGenerator
    IFACEMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override;
    IFACEMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override;
    IFACEMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override;
    IFACEMETHODIMP QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status, const PROPVARIANT* payload) override;

    // IMFMediaSource
    IFACEMETHODIMP CreatePresentationDescriptor(IMFPresentationDescriptor** descriptor) override;
    IFACEMETHODIMP GetCharacteristics(DWORD* characteristics) override;
    IFACEMETHODIMP Pause() override;
    IFACEMETHODIMP Shutdown() override;
    IFACEMETHODIMP Start(IMFPresentationDescriptor* descriptor, const GUID* time_format, const PROPVARIANT* start_position) override;
    IFACEMETHODIMP Stop() override;

    // IMFMediaSourceEx
    IFACEMETHODIMP GetSourceAttributes(IMFAttributes** attributes) override;
    IFACEMETHODIMP GetStreamAttributes(DWORD stream_id, IMFAttributes** attributes) override;
    IFACEMETHODIMP SetD3DManager(IUnknown* manager) override;

    // IMFGetService
    IFACEMETHODIMP GetService(REFGUID service, REFIID riid, LPVOID* object) override;

    // IKsControl: the camera has no controls.
    IFACEMETHODIMP KsProperty(PKSPROPERTY property, ULONG property_length, LPVOID data, ULONG data_length, ULONG* returned) override;
    IFACEMETHODIMP KsMethod(PKSMETHOD method, ULONG method_length, LPVOID data, ULONG data_length, ULONG* returned) override;
    IFACEMETHODIMP KsEvent(PKSEVENT event, ULONG event_length, LPVOID data, ULONG data_length, ULONG* returned) override;

    // IMFSampleAllocatorControl
    IFACEMETHODIMP SetDefaultAllocator(DWORD output_stream_id, IUnknown* allocator) override;
    IFACEMETHODIMP GetAllocatorUsage(DWORD output_stream_id, DWORD* input_stream_id, MFSampleAllocatorUsage* usage) override;

private:
    enum class State { Stopped, Started, Shutdown };

    HRESULT Initialize(IMFAttributes* activate_attributes);
    HRESULT CheckShutdown() const;

    LiveObject live_;
    mutable std::mutex lock_;
    State state_ = State::Stopped;
    Microsoft::WRL::ComPtr<IMFMediaEventQueue> events_;
    Microsoft::WRL::ComPtr<IMFPresentationDescriptor> descriptor_;
    Microsoft::WRL::ComPtr<IMFAttributes> attributes_;
    Microsoft::WRL::ComPtr<Stream> stream_;
};
