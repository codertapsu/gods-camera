#include "frame_feed.h"

#include <aclapi.h>

namespace {

// What the source asks of the pipe: read, write and look at it. Asking for
// "generic write" would also ask to create pipe instances, which the app
// does not grant.
constexpr DWORD kPipeAccess =
    FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;

// The pipe must belong to the user its name says. Another account could
// otherwise make a pipe of that name first and feed the camera its own pictures.
bool OwnerMatchesName(HANDLE pipe, const std::wstring& pipe_name) {
    const std::wstring expected = gc::sid_in_pipe_name(pipe_name);
    if (expected.empty()) {
        return false;
    }
    PSID owner = nullptr;
    PSECURITY_DESCRIPTOR descriptor = nullptr;
    if (GetSecurityInfo(pipe, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION, &owner, nullptr, nullptr, nullptr, &descriptor)
        != ERROR_SUCCESS) {
        return false;
    }
    const std::wstring actual = gc::sid_to_string(owner);
    LocalFree(descriptor);
    return _wcsicmp(actual.c_str(), expected.c_str()) == 0;
}

}  // namespace

FrameFeed::FrameFeed()
    : stop_(CreateEventW(nullptr, TRUE, FALSE, nullptr)), new_frame_(CreateEventW(nullptr, FALSE, FALSE, nullptr)) {}

FrameFeed::~FrameFeed() {
    Stop();
}

void FrameFeed::Start(const std::wstring& pipe_name, uint32_t width, uint32_t height) {
    if (reader_.joinable() && pipe_name == pipe_name_ && width == width_ && height == height_) {
        return;
    }
    Stop();
    pipe_name_ = pipe_name;
    width_ = width;
    height_ = height;
    ResetEvent(stop_.get());
    PinModule();
    reader_ = std::thread([this] {
        try {
            Run();
        } catch (...) {
            DebugLine(L"the pipe reader ended on an error");
        }
    });
}

void FrameFeed::Stop() {
    if (reader_.joinable()) {
        SetEvent(stop_.get());
        reader_.join();
    }
    std::lock_guard<std::mutex> guard(lock_);
    info_ = Info{};
}

FrameFeed::Info FrameFeed::Latest() const {
    std::lock_guard<std::mutex> guard(lock_);
    return info_;
}

bool FrameFeed::WaitUnlessStopping(DWORD milliseconds) const {
    return WaitForSingleObject(stop_.get(), milliseconds) != WAIT_OBJECT_0;
}

bool FrameFeed::ReadExact(HANDLE pipe, OVERLAPPED* operation, void* into, DWORD bytes) const {
    auto* at = static_cast<uint8_t*>(into);
    while (bytes > 0) {
        ResetEvent(operation->hEvent);
        DWORD got = 0;
        if (!ReadFile(pipe, at, bytes, &got, operation)) {
            if (GetLastError() != ERROR_IO_PENDING) {
                return false;
            }
            HANDLE waits[2] = {stop_.get(), operation->hEvent};
            if (WaitForMultipleObjects(2, waits, FALSE, INFINITE) != WAIT_OBJECT_0 + 1) {
                CancelIoEx(pipe, operation);
                GetOverlappedResult(pipe, operation, &got, TRUE);
                return false;
            }
            if (!GetOverlappedResult(pipe, operation, &got, FALSE)) {
                return false;
            }
        }
        if (got == 0) {
            return false;
        }
        at += got;
        bytes -= got;
    }
    return true;
}

bool FrameFeed::WriteExact(HANDLE pipe, OVERLAPPED* operation, const void* from, DWORD bytes) const {
    ResetEvent(operation->hEvent);
    DWORD written = 0;
    if (!WriteFile(pipe, from, bytes, &written, operation)) {
        if (GetLastError() != ERROR_IO_PENDING) {
            return false;
        }
        HANDLE waits[2] = {stop_.get(), operation->hEvent};
        if (WaitForMultipleObjects(2, waits, FALSE, 2000) != WAIT_OBJECT_0 + 1) {
            CancelIoEx(pipe, operation);
            GetOverlappedResult(pipe, operation, &written, TRUE);
            return false;
        }
        if (!GetOverlappedResult(pipe, operation, &written, FALSE)) {
            return false;
        }
    }
    return written == bytes;
}

void FrameFeed::Run() {
    gc::Handle io_event(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    std::vector<uint8_t> back;
    bool said_missing = false;
    int failed_attempts = 0;
    while (WaitUnlessStopping(0)) {
        // SECURITY_IDENTIFICATION: the app may learn who connected, and no more.
        // The camera service must not lend its identity to whoever owns the pipe.
        gc::Handle pipe(CreateFileW(
            pipe_name_.c_str(), kPipeAccess, 0, nullptr, OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, nullptr));
        if (!pipe.valid()) {
            if (!said_missing) {
                DebugLine(L"the app's pipe cannot be opened (error %u); showing the slate", GetLastError());
                said_missing = true;
            }
            // The app needs a moment to listen again after a reader has left,
            // so the first tries come quickly. After that, four a second.
            const DWORD wait = failed_attempts < 10 ? 20 : 250;
            ++failed_attempts;
            if (!WaitUnlessStopping(wait)) {
                break;
            }
            continue;
        }
        said_missing = false;
        failed_attempts = 0;
        if (!OwnerMatchesName(pipe.get(), pipe_name_)) {
            DebugLine(L"the pipe does not belong to the user in its name; ignoring it");
            if (!WaitUnlessStopping(2000)) {
                break;
            }
            continue;
        }

        OVERLAPPED operation{};
        operation.hEvent = io_event.get();
        gc_vcam_request request{};
        request.magic = GC_VCAM_MAGIC_REQUEST;
        request.version = GC_VCAM_PROTOCOL_VERSION;
        request.size = sizeof(request);
        request.width = width_;
        request.height = height_;
        request.format = GC_VCAM_FOURCC_NV12;
        request.frame_rate_num = 30;
        request.frame_rate_den = 1;
        bool healthy = WriteExact(pipe.get(), &operation, &request, sizeof(request));
        if (healthy) {
            DebugLine(L"connected to the app; asked for %u x %u", width_, height_);
        }

        while (healthy) {
            gc_vcam_frame_header header{};
            if (!ReadExact(pipe.get(), &operation, &header, sizeof(header))) {
                break;
            }
            const bool sensible = header.magic == GC_VCAM_MAGIC_FRAME && header.version == GC_VCAM_PROTOCOL_VERSION
                && header.header_size >= sizeof(header) && header.header_size <= 4096
                && header.format == GC_VCAM_FOURCC_NV12 && header.width >= 16 && header.width <= GC_VCAM_MAX_WIDTH
                && header.height >= 16 && header.height <= GC_VCAM_MAX_HEIGHT && header.width % 2 == 0
                && header.height % 2 == 0 && header.payload_size == gc_vcam_nv12_size(header.width, header.height)
                && header.sequence != 0;
            if (!sensible) {
                DebugLine(L"the app sent something that is not a frame; disconnecting");
                break;
            }
            // A later version of the app may send a longer header: skip what this one does not know.
            uint8_t extra[4096];
            const DWORD more = header.header_size - static_cast<DWORD>(sizeof(header));
            if (more > 0 && !ReadExact(pipe.get(), &operation, extra, more)) {
                break;
            }
            back.resize(header.payload_size);
            if (!ReadExact(pipe.get(), &operation, back.data(), header.payload_size)) {
                break;
            }
            {
                std::lock_guard<std::mutex> guard(lock_);
                front_.swap(back);
                info_.sequence = header.sequence;
                info_.time_100ns = header.time_100ns;
                info_.arrived_100ns = gc_vcam_now_100ns();
                info_.width = header.width;
                info_.height = header.height;
            }
            SetEvent(new_frame_.get());
        }
        pipe.reset();
        if (!WaitUnlessStopping(100)) {
            break;
        }
    }
}
