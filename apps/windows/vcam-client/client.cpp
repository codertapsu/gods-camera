// The app's side of the camera's pipe. See gc_vcam_client.h.

#include "gc_vcam_client.h"

#include <atomic>
#include <cstring>
#include <mutex>
#include <new>
#include <string>
#include <thread>
#include <vector>

#include "gc_vcam_win.h"

namespace {

// What LOCAL SERVICE may do with the pipe: read, write and look at it. It may
// not create a further instance of the pipe, which is the one right that
// FILE_GENERIC_WRITE would add.
constexpr wchar_t kSourceRights[] = L"0x12019b";

// Room in the pipe for one 1080p picture, so a write does not wait for the reader.
constexpr DWORD kOutBufferBytes = 4 * 1024 * 1024;
// A write that takes longer than this means the source is stuck.
constexpr DWORD kWriteTimeoutMs = 1000;

// The SID of a Windows service, as text, or empty if this machine has no such service.
std::wstring service_sid(const wchar_t* account) {
    DWORD sid_size = 0;
    DWORD domain_size = 0;
    SID_NAME_USE use{};
    LookupAccountNameW(nullptr, account, nullptr, &sid_size, nullptr, &domain_size, &use);
    if (sid_size == 0) {
        return {};
    }
    std::vector<BYTE> sid(sid_size);
    std::wstring domain(domain_size, L'\0');
    if (!LookupAccountNameW(nullptr, account, sid.data(), &sid_size, domain.data(), &domain_size, &use)) {
        return {};
    }
    return gc::sid_to_string(sid.data());
}

// An event that is closed with its owner.
gc::Handle make_event(bool manual_reset) {
    return gc::Handle(CreateEventW(nullptr, manual_reset ? TRUE : FALSE, FALSE, nullptr));
}

}  // namespace

struct gc_vcam_server {
    std::wstring pipe_name;
    gc::Handle pipe;
    gc::Handle stop;         // set once, when the server is being destroyed
    gc::Handle frame_ready;  // set whenever a picture is waiting to be written
    std::thread worker;

    mutable std::mutex lock;       // guards `pending`, `has_pending` and `wanted`
    std::vector<uint8_t> pending;  // a header and its picture, ready to write
    bool has_pending = false;
    gc_vcam_wanted wanted{};

    std::atomic<bool> watched{false};
    std::atomic<uint64_t> sequence{0};
    std::atomic<uint64_t> sent{0};
    std::atomic<uint64_t> dropped{0};
    std::atomic<uint64_t> connects{0};
    std::atomic<uint64_t> write_failures{0};
};

namespace {

// Wait until a cancelled or finished overlapped operation has let go of its buffer.
void settle(HANDLE pipe, OVERLAPPED* operation) {
    DWORD ignored = 0;
    CancelIoEx(pipe, operation);
    GetOverlappedResult(pipe, operation, &ignored, TRUE);
}

bool accept_request(gc_vcam_server* server, const gc_vcam_request& request) {
    const bool sensible = request.magic == GC_VCAM_MAGIC_REQUEST && request.version == GC_VCAM_PROTOCOL_VERSION
        && request.size == sizeof(gc_vcam_request) && request.format == GC_VCAM_FOURCC_NV12
        && request.width >= 16 && request.width <= GC_VCAM_MAX_WIDTH && request.height >= 16
        && request.height <= GC_VCAM_MAX_HEIGHT && request.width % 2 == 0 && request.height % 2 == 0
        && request.frame_rate_num > 0 && request.frame_rate_den > 0;
    if (!sensible) {
        return false;
    }
    {
        std::lock_guard<std::mutex> guard(server->lock);
        server->wanted.width = request.width;
        server->wanted.height = request.height;
        server->wanted.format = request.format;
        server->wanted.frame_rate_num = request.frame_rate_num;
        server->wanted.frame_rate_den = request.frame_rate_den;
    }
    server->watched.store(true);
    return true;
}

// Write a whole buffer, giving up when the server stops or the source does not take it.
bool write_all(gc_vcam_server* server, OVERLAPPED* operation, const std::vector<uint8_t>& bytes) {
    ResetEvent(operation->hEvent);
    DWORD written = 0;
    if (!WriteFile(server->pipe.get(), bytes.data(), static_cast<DWORD>(bytes.size()), &written, operation)) {
        if (GetLastError() != ERROR_IO_PENDING) {
            return false;
        }
        HANDLE waits[2] = {server->stop.get(), operation->hEvent};
        const DWORD which = WaitForMultipleObjects(2, waits, FALSE, kWriteTimeoutMs);
        if (which != WAIT_OBJECT_0 + 1) {
            settle(server->pipe.get(), operation);
            return false;
        }
        if (!GetOverlappedResult(server->pipe.get(), operation, &written, FALSE)) {
            return false;
        }
    }
    return written == bytes.size();
}

// Talk to one connected source until it leaves. Returns true if the server is stopping.
bool serve(gc_vcam_server* server, OVERLAPPED* read, OVERLAPPED* write) {
    gc_vcam_request request{};
    auto* into = reinterpret_cast<uint8_t*>(&request);
    DWORD have = 0;
    bool reading = false;
    std::vector<uint8_t> outgoing;
    for (;;) {
        if (have == sizeof(request)) {
            have = 0;
            if (!accept_request(server, request)) {
                return false;  // not a source this server understands
            }
        }
        if (!reading) {
            // Always keep one read going: it brings the next request, and it
            // is how the source's leaving is noticed.
            ResetEvent(read->hEvent);
            DWORD got = 0;
            if (ReadFile(server->pipe.get(), into + have, sizeof(request) - have, &got, read)) {
                if (got == 0) {
                    return false;
                }
                have += got;
                continue;
            }
            if (GetLastError() != ERROR_IO_PENDING) {
                return false;  // the source has gone
            }
            reading = true;
        }

        HANDLE waits[3] = {server->stop.get(), read->hEvent, server->frame_ready.get()};
        const DWORD which = WaitForMultipleObjects(3, waits, FALSE, INFINITE);
        if (which == WAIT_OBJECT_0) {
            settle(server->pipe.get(), read);
            return true;
        }
        if (which == WAIT_OBJECT_0 + 1) {
            DWORD got = 0;
            reading = false;
            if (!GetOverlappedResult(server->pipe.get(), read, &got, FALSE) || got == 0) {
                return false;
            }
            have += got;
            continue;
        }
        if (which != WAIT_OBJECT_0 + 2) {
            settle(server->pipe.get(), read);
            return false;
        }

        // A picture is waiting: take it and write it.
        {
            std::lock_guard<std::mutex> guard(server->lock);
            if (!server->has_pending) {
                continue;
            }
            outgoing.swap(server->pending);
            server->has_pending = false;
        }
        if (!write_all(server, write, outgoing)) {
            settle(server->pipe.get(), read);
            if (WaitForSingleObject(server->stop.get(), 0) == WAIT_OBJECT_0) {
                return true;
            }
            server->write_failures.fetch_add(1);
            return false;
        }
        server->sent.fetch_add(1);
    }
}

void run(gc_vcam_server* server) {
    gc::Handle connect_event = make_event(true);
    gc::Handle read_event = make_event(true);
    gc::Handle write_event = make_event(true);
    OVERLAPPED connect{};
    OVERLAPPED read{};
    OVERLAPPED write{};
    connect.hEvent = connect_event.get();
    read.hEvent = read_event.get();
    write.hEvent = write_event.get();

    for (;;) {
        ResetEvent(connect.hEvent);
        bool connected = false;
        if (ConnectNamedPipe(server->pipe.get(), &connect)) {
            connected = true;
        } else {
            const DWORD error = GetLastError();
            if (error == ERROR_PIPE_CONNECTED) {
                connected = true;  // the source connected before this call
            } else if (error == ERROR_IO_PENDING) {
                HANDLE waits[2] = {server->stop.get(), connect.hEvent};
                if (WaitForMultipleObjects(2, waits, FALSE, INFINITE) == WAIT_OBJECT_0) {
                    settle(server->pipe.get(), &connect);
                    return;
                }
                DWORD ignored = 0;
                connected = GetOverlappedResult(server->pipe.get(), &connect, &ignored, FALSE) != FALSE;
            }
        }
        if (connected) {
            server->connects.fetch_add(1);
            const bool stopping = serve(server, &read, &write);
            server->watched.store(false);
            {
                std::lock_guard<std::mutex> guard(server->lock);
                server->has_pending = false;
            }
            if (stopping) {
                return;
            }
        } else if (WaitForSingleObject(server->stop.get(), 200) == WAIT_OBJECT_0) {
            return;
        }
        DisconnectNamedPipe(server->pipe.get());
    }
}

}  // namespace

extern "C" uint32_t gc_vcam_server_create(gc_vcam_server** out) {
    if (out == nullptr) {
        return ERROR_INVALID_PARAMETER;
    }
    *out = nullptr;
    auto* server = new (std::nothrow) gc_vcam_server();
    if (server == nullptr) {
        return ERROR_OUTOFMEMORY;
    }
    const std::wstring user = gc::current_user_sid();
    server->stop = make_event(true);
    server->frame_ready = make_event(false);
    if (user.empty() || !server->stop.valid() || !server->frame_ready.valid()) {
        delete server;
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    server->pipe_name = gc::pipe_name_for(user);

    // Whose the pipe is: this user's, also when the app runs as administrator
    // (Windows would otherwise write the Administrators group as the owner).
    // The media source checks the owner before it believes the pipe.
    //
    // Who may open it: this user, fully; and the camera service, to read and
    // write. The service runs as LOCAL SERVICE; its own identity and the
    // system's are named as well, in case a version of Windows runs it more
    // narrowly or does part of its work as the system. Nobody else, and
    // nobody over the network.
    const std::wstring rights(kSourceRights);
    std::wstring sddl = L"O:" + user + L"D:P(A;;GA;;;" + user + L")(A;;" + rights + L";;;LS)(A;;" + rights + L";;;SY)";
    const std::wstring frame_server = service_sid(L"NT SERVICE\\FrameServer");
    if (!frame_server.empty()) {
        sddl += L"(A;;" + rights + L";;;" + frame_server + L")";
    }
    PSECURITY_DESCRIPTOR descriptor = nullptr;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.c_str(), SDDL_REVISION_1, &descriptor, nullptr)) {
        const DWORD error = GetLastError();
        delete server;
        return error;
    }
    SECURITY_ATTRIBUTES attributes{sizeof(SECURITY_ATTRIBUTES), descriptor, FALSE};
    // FILE_FLAG_FIRST_PIPE_INSTANCE: if another program already made a pipe
    // of this name, fail instead of quietly sharing the name with it.
    HANDLE pipe = CreateNamedPipeW(
        server->pipe_name.c_str(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 1, kOutBufferBytes, 4096, 0,
        &attributes);
    const DWORD error = pipe == INVALID_HANDLE_VALUE ? GetLastError() : ERROR_SUCCESS;
    LocalFree(descriptor);
    if (pipe == INVALID_HANDLE_VALUE) {
        delete server;
        return error;
    }
    server->pipe.reset(pipe);
    server->worker = std::thread(run, server);
    *out = server;
    return ERROR_SUCCESS;
}

extern "C" void gc_vcam_server_destroy(gc_vcam_server* server) {
    if (server == nullptr) {
        return;
    }
    SetEvent(server->stop.get());
    if (server->worker.joinable()) {
        server->worker.join();
    }
    delete server;
}

extern "C" const wchar_t* gc_vcam_server_pipe_name(const gc_vcam_server* server) {
    return server == nullptr ? L"" : server->pipe_name.c_str();
}

extern "C" int gc_vcam_server_watched(const gc_vcam_server* server, gc_vcam_wanted* wanted) {
    if (server == nullptr || !server->watched.load()) {
        return 0;
    }
    if (wanted != nullptr) {
        std::lock_guard<std::mutex> guard(server->lock);
        *wanted = server->wanted;
    }
    return 1;
}

extern "C" int gc_vcam_server_send(
    gc_vcam_server* server, const uint8_t* nv12, uint32_t width, uint32_t height, int64_t time_100ns) {
    if (server == nullptr || nv12 == nullptr || !server->watched.load()) {
        return 0;
    }
    if (width < 16 || height < 16 || width > GC_VCAM_MAX_WIDTH || height > GC_VCAM_MAX_HEIGHT || width % 2 != 0
        || height % 2 != 0) {
        return 0;
    }
    gc_vcam_frame_header header{};
    header.magic = GC_VCAM_MAGIC_FRAME;
    header.version = GC_VCAM_PROTOCOL_VERSION;
    header.header_size = sizeof(header);
    header.width = width;
    header.height = height;
    header.format = GC_VCAM_FOURCC_NV12;
    header.payload_size = gc_vcam_nv12_size(width, height);
    header.sequence = server->sequence.fetch_add(1) + 1;
    header.time_100ns = time_100ns;
    {
        std::lock_guard<std::mutex> guard(server->lock);
        if (server->has_pending) {
            server->dropped.fetch_add(1);  // the newer picture replaces the one not yet written
        }
        server->pending.resize(sizeof(header) + header.payload_size);
        std::memcpy(server->pending.data(), &header, sizeof(header));
        std::memcpy(server->pending.data() + sizeof(header), nv12, header.payload_size);
        server->has_pending = true;
    }
    SetEvent(server->frame_ready.get());
    return 1;
}

extern "C" void gc_vcam_server_get_stats(const gc_vcam_server* server, gc_vcam_stats* stats) {
    if (server == nullptr || stats == nullptr) {
        return;
    }
    stats->frames_sent = server->sent.load();
    stats->frames_dropped = server->dropped.load();
    stats->connects = server->connects.load();
    stats->write_failures = server->write_failures.load();
}
