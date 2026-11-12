// The DLL's four entry points, as every in-process COM server has them, and
// the few things the rest of the source shares.

#include "activate.h"
#include "pch.h"

#include <cstdarg>
#include <cwchar>

using Microsoft::WRL::ComPtr;

std::atomic<long> g_live_objects{0};

namespace {

HMODULE g_module = nullptr;
std::atomic<long> g_server_locks{0};
std::atomic<bool> g_pinned{false};

constexpr wchar_t kClassKey[] = L"SOFTWARE\\Classes\\CLSID\\" GC_VCAM_CLSID_STRING;
constexpr wchar_t kClassTitle[] = L"God's Camera media source";

// Makes the activation object when COM asks for the class.
class Factory final
    : public Microsoft::WRL::RuntimeClass<
          Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>, IClassFactory, Microsoft::WRL::FtmBase> {
public:
    IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** object) override {
        if (object == nullptr) {
            return E_POINTER;
        }
        *object = nullptr;
        if (outer != nullptr) {
            return CLASS_E_NOAGGREGATION;
        }
        return Guarded([&]() -> HRESULT {
            ComPtr<Activate> activate;
            RETURN_IF_FAILED(Microsoft::WRL::MakeAndInitialize<Activate>(&activate));
            return activate.CopyTo(riid, object);
        });
    }

    IFACEMETHODIMP LockServer(BOOL lock) override {
        if (lock) {
            g_server_locks.fetch_add(1);
        } else {
            g_server_locks.fetch_sub(1);
        }
        return S_OK;
    }

private:
    LiveObject live_;
};

LSTATUS SetString(HKEY key, const wchar_t* name, const wchar_t* value) {
    const DWORD bytes = static_cast<DWORD>((wcslen(value) + 1) * sizeof(wchar_t));
    return RegSetValueExW(key, name, 0, REG_SZ, reinterpret_cast<const BYTE*>(value), bytes);
}

}  // namespace

void DebugLine(const wchar_t* format, ...) {
    wchar_t line[512];
    const int prefix = swprintf_s(line, L"[GodsCamera] ");
    va_list arguments;
    va_start(arguments, format);
    const int written = _vsnwprintf_s(line + prefix, ARRAYSIZE(line) - prefix - 2, _TRUNCATE, format, arguments);
    va_end(arguments);
    const size_t length = written < 0 ? wcslen(line) : static_cast<size_t>(prefix) + written;
    line[length] = L'\n';
    line[length + 1] = L'\0';
    OutputDebugStringW(line);
}

void PinModule() {
    if (g_pinned.exchange(true)) {
        return;
    }
    HMODULE pinned = nullptr;
    GetModuleHandleExW(
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, reinterpret_cast<LPCWSTR>(&PinModule),
        &pinned);
}

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = module;
    }
    return TRUE;
}

STDAPI DllGetClassObject(REFCLSID clsid, REFIID riid, LPVOID* object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    *object = nullptr;
    if (clsid != CLSID_GodsCameraSource) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    return Guarded([&]() -> HRESULT {
        ComPtr<Factory> factory = Microsoft::WRL::Make<Factory>();
        if (!factory) {
            return E_OUTOFMEMORY;
        }
        return factory.CopyTo(riid, object);
    });
}

STDAPI DllCanUnloadNow() {
    return g_live_objects.load() == 0 && g_server_locks.load() == 0 ? S_OK : S_FALSE;
}

// Tell Windows where the class lives. The camera service runs as LOCAL
// SERVICE and only looks in the machine's part of the registry, so this
// needs an administrator: tools/register-windows-camera.ps1 calls it through
// regsvr32.
STDAPI DllRegisterServer() {
    wchar_t path[MAX_PATH * 2];
    const DWORD length = GetModuleFileNameW(g_module, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path)) {
        return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
    }

    HKEY class_key = nullptr;
    LSTATUS status = RegCreateKeyExW(
        HKEY_LOCAL_MACHINE, kClassKey, 0, nullptr, REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr, &class_key, nullptr);
    if (status != ERROR_SUCCESS) {
        return HRESULT_FROM_WIN32(status);
    }
    status = SetString(class_key, nullptr, kClassTitle);
    HKEY server_key = nullptr;
    if (status == ERROR_SUCCESS) {
        status = RegCreateKeyExW(
            class_key, L"InprocServer32", 0, nullptr, REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr, &server_key, nullptr);
    }
    if (status == ERROR_SUCCESS) {
        status = SetString(server_key, nullptr, path);
    }
    if (status == ERROR_SUCCESS) {
        status = SetString(server_key, L"ThreadingModel", L"Both");
    }
    if (server_key != nullptr) {
        RegCloseKey(server_key);
    }
    RegCloseKey(class_key);
    if (status != ERROR_SUCCESS) {
        RegDeleteTreeW(HKEY_LOCAL_MACHINE, kClassKey);
        return HRESULT_FROM_WIN32(status);
    }
    return S_OK;
}

STDAPI DllUnregisterServer() {
    const LSTATUS status = RegDeleteTreeW(HKEY_LOCAL_MACHINE, kClassKey);
    if (status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND) {
        return S_OK;
    }
    return HRESULT_FROM_WIN32(status);
}
