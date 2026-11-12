// Fetches pictures from the app through its pipe and keeps the newest one.
#pragma once

#include "pch.h"

class FrameFeed {
public:
    struct Info {
        uint64_t sequence = 0;      // 0 means there is no picture yet
        int64_t time_100ns = 0;     // when the app made it
        int64_t arrived_100ns = 0;  // when it arrived here
        uint32_t width = 0;
        uint32_t height = 0;
    };

    FrameFeed();
    ~FrameFeed();
    FrameFeed(const FrameFeed&) = delete;
    FrameFeed& operator=(const FrameFeed&) = delete;

    // False if Windows could not give the feed what it needs to run at all.
    bool Ready() const { return stop_.valid() && new_frame_.valid(); }

    // Connect to the app's pipe and ask for pictures of this size. Keeps
    // trying for as long as it runs, so the app may come and go. Calling it
    // again with another size starts over.
    void Start(const std::wstring& pipe_name, uint32_t width, uint32_t height);
    // Disconnect, which also tells the app that nothing is watching any more.
    void Stop();

    // Set whenever a new picture has arrived. Resets itself when waited on.
    HANDLE NewFrameEvent() const { return new_frame_.get(); }

    // What the newest picture is, without copying it.
    Info Latest() const;

    // Hand the newest picture, as NV12 with rows `width` bytes apart, to
    // `use`, if it has the size asked for. Returns false if there is none.
    // The picture stays put while `use` runs, so `use` must be quick.
    template <typename Use>
    bool WithLatest(uint32_t width, uint32_t height, Info* info, Use&& use) const {
        std::lock_guard<std::mutex> guard(lock_);
        if (info_.sequence == 0 || info_.width != width || info_.height != height) {
            return false;
        }
        use(front_.data());
        *info = info_;
        return true;
    }

private:
    void Run();
    bool ReadExact(HANDLE pipe, OVERLAPPED* operation, void* into, DWORD bytes) const;
    bool WriteExact(HANDLE pipe, OVERLAPPED* operation, const void* from, DWORD bytes) const;
    bool WaitUnlessStopping(DWORD milliseconds) const;

    std::wstring pipe_name_;
    uint32_t width_ = 0;
    uint32_t height_ = 0;
    gc::Handle stop_;       // set while the reader thread should end
    gc::Handle new_frame_;
    std::thread reader_;

    mutable std::mutex lock_;      // guards `front_` and `info_`
    std::vector<uint8_t> front_;   // the newest whole picture
    Info info_;
};
