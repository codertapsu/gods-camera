// The picture the camera shows when the app is not sending: dark, with a
// light ring and a stroke through it. No words, so it needs no translation.
//
// The media source draws it; the probe recognises it.
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

}  // namespace gc_slate
