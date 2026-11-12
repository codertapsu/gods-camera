// Small Windows helpers both sides of the camera's pipe need.
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <sddl.h>

#include <string>
#include <vector>

#include "gc_vcam_protocol.h"

namespace gc {

// A handle that closes itself.
class Handle {
public:
    Handle() = default;
    explicit Handle(HANDLE handle) : handle_(handle) {}
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
    Handle(Handle&& other) noexcept : handle_(other.release()) {}
    Handle& operator=(Handle&& other) noexcept {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }
    ~Handle() { reset(); }

    bool valid() const { return handle_ != nullptr && handle_ != INVALID_HANDLE_VALUE; }
    HANDLE get() const { return handle_; }
    HANDLE release() {
        HANDLE handle = handle_;
        handle_ = nullptr;
        return handle;
    }
    void reset(HANDLE handle = nullptr) {
        if (valid()) {
            CloseHandle(handle_);
        }
        handle_ = handle;
    }

private:
    HANDLE handle_ = nullptr;
};

// A SID as text, such as S-1-5-21-..., or an empty string if it cannot be written.
inline std::wstring sid_to_string(PSID sid) {
    LPWSTR text = nullptr;
    if (sid == nullptr || !ConvertSidToStringSidW(sid, &text)) {
        return {};
    }
    std::wstring out(text);
    LocalFree(text);
    return out;
}

// The SID of the user this process runs as, as text. Empty on failure.
inline std::wstring current_user_sid() {
    HANDLE raw = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw)) {
        return {};
    }
    Handle token(raw);
    DWORD size = 0;
    GetTokenInformation(token.get(), TokenUser, nullptr, 0, &size);
    if (size == 0) {
        return {};
    }
    std::vector<BYTE> buffer(size);
    if (!GetTokenInformation(token.get(), TokenUser, buffer.data(), size, &size)) {
        return {};
    }
    return sid_to_string(reinterpret_cast<TOKEN_USER*>(buffer.data())->User.Sid);
}

// The name of the pipe that belongs to the user with this SID.
inline std::wstring pipe_name_for(const std::wstring& sid) {
    return std::wstring(GC_VCAM_PIPE_PREFIX) + sid;
}

// The SID at the end of a pipe's name, or an empty string if the name is not
// one of ours. The media source opens whatever name it is handed, inside the
// camera service, so the check is strict: the exact prefix, then a SID and
// nothing else. No other pipe, file or network path can pass.
inline std::wstring sid_in_pipe_name(const std::wstring& pipe_name) {
    const std::wstring prefix(GC_VCAM_PIPE_PREFIX);
    if (pipe_name.size() <= prefix.size() || pipe_name.size() > prefix.size() + 184
        || pipe_name.compare(0, prefix.size(), prefix) != 0) {
        return {};
    }
    const std::wstring sid = pipe_name.substr(prefix.size());
    if (sid.compare(0, 4, L"S-1-") != 0) {
        return {};
    }
    for (size_t i = 4; i < sid.size(); ++i) {
        const wchar_t c = sid[i];
        if (!(c >= L'0' && c <= L'9') && c != L'-') {
            return {};
        }
    }
    PSID parsed = nullptr;
    if (!ConvertStringSidToSidW(sid.c_str(), &parsed)) {
        return {};
    }
    LocalFree(parsed);
    return sid;
}

}  // namespace gc
