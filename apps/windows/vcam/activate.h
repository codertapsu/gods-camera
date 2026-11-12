// The class Windows creates by its CLSID inside the camera service.
//
// It is an "activation object": a bag of attributes that can make the media
// source when asked. The camera service fills the bag first, with its own
// attributes and with those the app set on its virtual camera, and then asks
// for the source.
#pragma once

#include "pch.h"
#include "source.h"

class Activate final
    : public Microsoft::WRL::RuntimeClass<
          Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>,
          Microsoft::WRL::ChainInterfaces<IMFActivate, IMFAttributes>, Microsoft::WRL::FtmBase> {
public:
    Activate() = default;
    HRESULT RuntimeClassInitialize();

    // IMFActivate
    IFACEMETHODIMP ActivateObject(REFIID riid, void** object) override;
    IFACEMETHODIMP ShutdownObject() override;
    IFACEMETHODIMP DetachObject() override;

    // IMFAttributes: all handed on to an ordinary attribute store.
    IFACEMETHODIMP GetItem(REFGUID key, PROPVARIANT* item) override;
    IFACEMETHODIMP GetItemType(REFGUID key, MF_ATTRIBUTE_TYPE* type) override;
    IFACEMETHODIMP CompareItem(REFGUID key, REFPROPVARIANT item, BOOL* result) override;
    IFACEMETHODIMP Compare(IMFAttributes* theirs, MF_ATTRIBUTES_MATCH_TYPE match, BOOL* result) override;
    IFACEMETHODIMP GetUINT32(REFGUID key, UINT32* item) override;
    IFACEMETHODIMP GetUINT64(REFGUID key, UINT64* item) override;
    IFACEMETHODIMP GetDouble(REFGUID key, double* item) override;
    IFACEMETHODIMP GetGUID(REFGUID key, GUID* item) override;
    IFACEMETHODIMP GetStringLength(REFGUID key, UINT32* length) override;
    IFACEMETHODIMP GetString(REFGUID key, LPWSTR item, UINT32 size, UINT32* length) override;
    IFACEMETHODIMP GetAllocatedString(REFGUID key, LPWSTR* item, UINT32* length) override;
    IFACEMETHODIMP GetBlobSize(REFGUID key, UINT32* size) override;
    IFACEMETHODIMP GetBlob(REFGUID key, UINT8* buffer, UINT32 buffer_size, UINT32* size) override;
    IFACEMETHODIMP GetAllocatedBlob(REFGUID key, UINT8** buffer, UINT32* size) override;
    IFACEMETHODIMP GetUnknown(REFGUID key, REFIID riid, LPVOID* object) override;
    IFACEMETHODIMP SetItem(REFGUID key, REFPROPVARIANT item) override;
    IFACEMETHODIMP DeleteItem(REFGUID key) override;
    IFACEMETHODIMP DeleteAllItems() override;
    IFACEMETHODIMP SetUINT32(REFGUID key, UINT32 item) override;
    IFACEMETHODIMP SetUINT64(REFGUID key, UINT64 item) override;
    IFACEMETHODIMP SetDouble(REFGUID key, double item) override;
    IFACEMETHODIMP SetGUID(REFGUID key, REFGUID item) override;
    IFACEMETHODIMP SetString(REFGUID key, LPCWSTR item) override;
    IFACEMETHODIMP SetBlob(REFGUID key, const UINT8* buffer, UINT32 size) override;
    IFACEMETHODIMP SetUnknown(REFGUID key, IUnknown* unknown) override;
    IFACEMETHODIMP LockStore() override;
    IFACEMETHODIMP UnlockStore() override;
    IFACEMETHODIMP GetCount(UINT32* items) override;
    IFACEMETHODIMP GetItemByIndex(UINT32 index, GUID* key, PROPVARIANT* item) override;
    IFACEMETHODIMP CopyAllItems(IMFAttributes* destination) override;

private:
    LiveObject live_;
    std::mutex lock_;  // guards `source_`
    Microsoft::WRL::ComPtr<IMFAttributes> attributes_;
    Microsoft::WRL::ComPtr<Source> source_;
};
