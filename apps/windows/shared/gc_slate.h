// The picture the camera shows when the app is not sending: dark, with a
// light ring and a stroke through it. No words, so it needs no translation.
//
// The media source draws it; the probe recognises it.
//
// Along the bottom edge the slate also carries a row of small, dim squares
// that say why it is showing. The media source runs inside Windows' camera
// service, where nothing it writes can be read without an administrator's
// tools; the picture is the one thing that always gets out. The probe reads
// the squares back.
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace gc_slate {

inline constexpr uint8_t kDark = 28;
inline constexpr uint8_t kLight = 170;

struct Mark {
    int64_t cx;
    int64_t cy;
    int64_t outer;   // the ring's outer radius
    int64_t inner;   // and its inner one
    int64_t stroke;  // half the width of the stroke through it
};

inline Mark mark(uint32_t width, uint32_t height) {
    Mark m{};
    m.cx = width / 2;
    m.cy = height / 2;
    m.outer = height / 7;
    m.inner = m.outer - m.outer / 5;
    m.stroke = m.outer / 9 + 1;
    return m;
}

// Draw the slate into an NV12 picture whose rows are `width` bytes apart.
inline void draw(uint8_t* nv12, uint32_t width, uint32_t height) {
    const size_t luma = static_cast<size_t>(width) * height;
    std::memset(nv12, kDark, luma);
    std::memset(nv12 + luma, 128, luma / 2);  // no colour anywhere
    const Mark m = mark(width, height);
    for (int64_t y = m.cy - m.outer; y <= m.cy + m.outer; ++y) {
        for (int64_t x = m.cx - m.outer; x <= m.cx + m.outer; ++x) {
            if (x < 0 || y < 0 || x >= static_cast<int64_t>(width) || y >= static_cast<int64_t>(height)) {
                continue;
            }
            const int64_t dx = x - m.cx;
            const int64_t dy = y - m.cy;
            const int64_t distance = dx * dx + dy * dy;
            if (distance > m.outer * m.outer) {
                continue;
            }
            const bool ring = distance >= m.inner * m.inner;
            const bool stroke = dx - dy < m.stroke && dy - dx < m.stroke;
            if (ring || stroke) {
                nv12[static_cast<size_t>(y) * width + static_cast<size_t>(x)] = kLight;
            }
        }
    }
}

// Whether a luma plane shows the slate: light where the mark is, dark beside
// it and in the corners.
inline bool looks_like(const uint8_t* luma, size_t stride, uint32_t width, uint32_t height) {
    if (width < 64 || height < 64) {
        return false;
    }
    const Mark m = mark(width, height);
    const auto at = [&](int64_t x, int64_t y) { return luma[static_cast<size_t>(y) * stride + static_cast<size_t>(x)]; };
    const auto close_to = [](uint8_t value, uint8_t wanted) { return value + 14 >= wanted && value <= wanted + 14; };
    const int64_t right = static_cast<int64_t>(width) - 9;
    const int64_t bottom = static_cast<int64_t>(height) - 9;
    return close_to(at(m.cx, m.cy), kLight)                                     // on the stroke
        && close_to(at(m.cx + (m.outer + m.inner) / 2, m.cy), kLight)           // on the ring
        && close_to(at(m.cx + m.inner / 2, m.cy - m.inner / 2), kDark)          // inside the ring, off the stroke
        && close_to(at(8, 8), kDark) && close_to(at(right, 8), kDark) && close_to(at(8, bottom), kDark)
        && close_to(at(right, bottom), kDark);
}

// Why the slate is showing.
enum Cause : uint8_t {
    kStarting = 0,      // nothing has been tried yet
    kNoPipe = 1,        // the app's pipe does not exist: the app is not running
    kRefused = 2,       // Windows refused to open the app's pipe
    kBusy = 3,          // another reader has the app's pipe
    kOpenFailed = 4,    // the pipe could not be opened for another reason; see `error`
    kNotTheUsers = 5,   // the pipe does not belong to the user its name says
    kNothingSent = 6,   // connected, and the app sends nothing
    kAnotherSize = 7,   // connected, and the app sends pictures of a size that was not asked for
};

struct Why {
    uint8_t cause = kStarting;   // a Cause
    bool named_by_app = false;   // the pipe's name came from the app, with the camera
    uint16_t error = 0;          // the Windows error code that goes with the cause, if any

    bool operator==(const Why& other) const {
        return cause == other.cause && named_by_app == other.named_by_app && error == other.error;
    }
};

// The row of squares: four that mark it, one for `named_by_app`, three for
// the cause, sixteen for the error. Most significant first.
inline constexpr uint8_t kWhyMarker[4] = {1, 0, 1, 1};
inline constexpr int kWhySquares = 24;
inline constexpr uint8_t kWhyLit = 64;  // dim: it is there to be read by a program

struct WhyRow {
    uint32_t x0;
    uint32_t y0;
    uint32_t square_w;
    uint32_t square_h;
};

inline WhyRow why_row(uint32_t width, uint32_t height) {
    WhyRow row{};
    row.square_w = width / 64;
    row.square_h = height / 45;
    row.x0 = row.square_w;
    row.y0 = height - 2 * row.square_h;
    return row;
}

inline uint32_t why_bits(const Why& why) {
    return (static_cast<uint32_t>(why.named_by_app ? 1 : 0) << 19) | (static_cast<uint32_t>(why.cause & 7u) << 16)
        | why.error;
}

// Write the reason into a slate that `draw` has drawn.
inline void draw_why(uint8_t* nv12, uint32_t width, uint32_t height, const Why& why) {
    const WhyRow row = why_row(width, height);
    const uint32_t bits = why_bits(why);
    for (int i = 0; i < kWhySquares; ++i) {
        const bool lit = i < 4 ? kWhyMarker[i] != 0 : ((bits >> (kWhySquares - 1 - i)) & 1u) != 0;
        for (uint32_t y = 0; y < row.square_h; ++y) {
            std::memset(
                nv12 + static_cast<size_t>(row.y0 + y) * width + row.x0 + static_cast<size_t>(i) * row.square_w,
                lit ? kWhyLit : kDark, row.square_w);
        }
    }
}

// Read the reason back from a picture that `looks_like` the slate. Returns
// false if the row is not there.
inline bool read_why(const uint8_t* luma, size_t stride, uint32_t width, uint32_t height, Why* why) {
    const WhyRow row = why_row(width, height);
    if (row.square_w < 4 || row.square_h < 4) {
        return false;
    }
    uint32_t bits = 0;
    for (int i = 0; i < kWhySquares; ++i) {
        const size_t x = row.x0 + static_cast<size_t>(i) * row.square_w + row.square_w / 2;
        const size_t y = row.y0 + row.square_h / 2;
        const bool lit = luma[y * stride + x] > (kDark + kWhyLit) / 2;
        if (i < 4) {
            if (lit != (kWhyMarker[i] != 0)) {
                return false;
            }
        } else {
            bits = (bits << 1) | (lit ? 1u : 0u);
        }
    }
    why->named_by_app = ((bits >> 19) & 1u) != 0;
    why->cause = static_cast<uint8_t>((bits >> 16) & 7u);
    why->error = static_cast<uint16_t>(bits & 0xFFFFu);
    return true;
}

}  // namespace gc_slate
