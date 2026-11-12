#include "activate.h"

using Microsoft::WRL::ComPtr;

HRESULT Activate::RuntimeClassInitialize() {
    return MFCreateAttributes(&attributes_, 8);
}

// IMFActivate

IFACEMETHODIMP Activate::ActivateObject(REFIID riid, void** object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    *object = nullptr;
    return Guarded([&]() -> HRESULT {
        std::lock_guard<std::mutex> guard(lock_);
        // The same source every time, until it is shut down or detached.
        if (!source_) {
            RETURN_IF_FAILED(Microsoft::WRL::MakeAndInitialize<Source>(&source_, attributes_.Get()));
            DebugLine(L"media source created");
        }
        return source_.CopyTo(riid, object);
    });
}

IFACEMETHODIMP Activate::ShutdownObject() {
    ComPtr<Source> source;
    {
        std::lock_guard<std::mutex> guard(lock_);
        source.Swap(source_);
    }
    if (source) {
        source->Shutdown();
    }
    return S_OK;
}

IFACEMETHODIMP Activate::DetachObject() {
    ComPtr<Source> source;
    std::lock_guard<std::mutex> guard(lock_);
    source.Swap(source_);
    return S_OK;
}

// IMFAttributes

IFACEMETHODIMP Activate::GetItem(REFGUID key, PROPVARIANT* item) {
    return attributes_->GetItem(key, item);
}

IFACEMETHODIMP Activate::GetItemType(REFGUID key, MF_ATTRIBUTE_TYPE* type) {
    return attributes_->GetItemType(key, type);
}

IFACEMETHODIMP Activate::CompareItem(REFGUID key, REFPROPVARIANT item, BOOL* result) {
    return attributes_->CompareItem(key, item, result);
}

IFACEMETHODIMP Activate::Compare(IMFAttributes* theirs, MF_ATTRIBUTES_MATCH_TYPE match, BOOL* result) {
    return attributes_->Compare(theirs, match, result);
}

IFACEMETHODIMP Activate::GetUINT32(REFGUID key, UINT32* item) {
    return attributes_->GetUINT32(key, item);
}

IFACEMETHODIMP Activate::GetUINT64(REFGUID key, UINT64* item) {
    return attributes_->GetUINT64(key, item);
}

IFACEMETHODIMP Activate::GetDouble(REFGUID key, double* item) {
    return attributes_->GetDouble(key, item);
}

IFACEMETHODIMP Activate::GetGUID(REFGUID key, GUID* item) {
    return attributes_->GetGUID(key, item);
}

IFACEMETHODIMP Activate::GetStringLength(REFGUID key, UINT32* length) {
    return attributes_->GetStringLength(key, length);
}

IFACEMETHODIMP Activate::GetString(REFGUID key, LPWSTR item, UINT32 size, UINT32* length) {
    return attributes_->GetString(key, item, size, length);
}

IFACEMETHODIMP Activate::GetAllocatedString(REFGUID key, LPWSTR* item, UINT32* length) {
    return attributes_->GetAllocatedString(key, item, length);
}

IFACEMETHODIMP Activate::GetBlobSize(REFGUID key, UINT32* size) {
    return attributes_->GetBlobSize(key, size);
}

IFACEMETHODIMP Activate::GetBlob(REFGUID key, UINT8* buffer, UINT32 buffer_size, UINT32* size) {
    return attributes_->GetBlob(key, buffer, buffer_size, size);
}

IFACEMETHODIMP Activate::GetAllocatedBlob(REFGUID key, UINT8** buffer, UINT32* size) {
    return attributes_->GetAllocatedBlob(key, buffer, size);
}

IFACEMETHODIMP Activate::GetUnknown(REFGUID key, REFIID riid, LPVOID* object) {
    return attributes_->GetUnknown(key, riid, object);
}

IFACEMETHODIMP Activate::SetItem(REFGUID key, REFPROPVARIANT item) {
    return attributes_->SetItem(key, item);
}

IFACEMETHODIMP Activate::DeleteItem(REFGUID key) {
    return attributes_->DeleteItem(key);
}

IFACEMETHODIMP Activate::DeleteAllItems() {
    return attributes_->DeleteAllItems();
}

IFACEMETHODIMP Activate::SetUINT32(REFGUID key, UINT32 item) {
    return attributes_->SetUINT32(key, item);
}

IFACEMETHODIMP Activate::SetUINT64(REFGUID key, UINT64 item) {
    return attributes_->SetUINT64(key, item);
}

IFACEMETHODIMP Activate::SetDouble(REFGUID key, double item) {
    return attributes_->SetDouble(key, item);
}

IFACEMETHODIMP Activate::SetGUID(REFGUID key, REFGUID item) {
    return attributes_->SetGUID(key, item);
}

IFACEMETHODIMP Activate::SetString(REFGUID key, LPCWSTR item) {
    return attributes_->SetString(key, item);
}

IFACEMETHODIMP Activate::SetBlob(REFGUID key, const UINT8* buffer, UINT32 size) {
    return attributes_->SetBlob(key, buffer, size);
}

IFACEMETHODIMP Activate::SetUnknown(REFGUID key, IUnknown* unknown) {
    return attributes_->SetUnknown(key, unknown);
}

IFACEMETHODIMP Activate::LockStore() {
    return attributes_->LockStore();
}

IFACEMETHODIMP Activate::UnlockStore() {
    return attributes_->UnlockStore();
}

IFACEMETHODIMP Activate::GetCount(UINT32* items) {
    return attributes_->GetCount(items);
}

IFACEMETHODIMP Activate::GetItemByIndex(UINT32 index, GUID* key, PROPVARIANT* item) {
    return attributes_->GetItemByIndex(index, key, item);
}

IFACEMETHODIMP Activate::CopyAllItems(IMFAttributes* destination) {
    return attributes_->CopyAllItems(destination);
}
