// A test picture that says which frame it is.
//
// The test host draws colour bars with a moving block, and writes two numbers
// into the top of the picture as rows of light and dark squares: the frame's
// number, and the time it was made. The probe reads the squares back from the
// luma plane of what the camera delivers. That measures what a meeting app
// would get, whatever happens to the sample's own attributes on the way.
#pragma once

#include <cstdint>
#include <cstring>
#include <vector>

namespace gc_pattern {

// Eight squares that mark the start of the first row. A picture without them
// is not this pattern: it is the source's slate, or something else.
inline constexpr uint8_t kPreamble[8] = {1, 0, 1, 1, 0, 0, 1, 0};
inline constexpr int kNumberBits = 32;
inline constexpr int kTimeBits = 48;
inline constexpr uint8_t kLight = 235;
inline constexpr uint8_t kDark = 16;

// The eight colour bars, left to right, in BT.709 studio range: luma, blue
// difference, red difference. The probe checks the colours that arrive
// against the last two.
inline constexpr int kBarCount = 8;
inline constexpr uint8_t kBars[kBarCount][3] = {
    {235, 128, 128}, {219, 16, 138}, {188, 154, 16}, {173, 42, 26},
    {78, 214, 230},  {63, 102, 240}, {32, 240, 118}, {16, 128, 128},
};

struct Layout {
    uint32_t block_w;
    uint32_t block_h;
    uint32_t x0;
    uint32_t row1_y;
    uint32_t row2_y;
};

// Where the squares are, for a picture of this size. 48 squares fit across.
inline Layout layout(uint32_t width, uint32_t height) {
    Layout l{};
    l.block_w = width / 64;
    l.block_h = height / 30;
    l.x0 = l.block_w;
    l.row1_y = l.block_h / 2;
    l.row2_y = l.row1_y + l.block_h + l.block_h / 2;
    return l;
}

inline void fill_block(uint8_t* luma, size_t stride, uint32_t x, uint32_t y, uint32_t w, uint32_t h, uint8_t value) {
    for (uint32_t row = 0; row < h; ++row) {
        std::memset(luma + (y + row) * stride + x, value, w);
    }
}

// Write the frame's number and time into the luma plane.
inline void draw_code(uint8_t* luma, size_t stride, uint32_t width, uint32_t height, uint32_t number, int64_t time_100ns) {
    const Layout l = layout(width, height);
    uint32_t x = l.x0;
    for (uint8_t bit : kPreamble) {
        fill_block(luma, stride, x, l.row1_y, l.block_w, l.block_h, bit ? kLight : kDark);
        x += l.block_w;
    }
    for (int bit = kNumberBits - 1; bit >= 0; --bit) {
        fill_block(luma, stride, x, l.row1_y, l.block_w, l.block_h, ((number >> bit) & 1u) ? kLight : kDark);
        x += l.block_w;
    }
    x = l.x0;
    const uint64_t time = static_cast<uint64_t>(time_100ns);
    for (int bit = kTimeBits - 1; bit >= 0; --bit) {
        fill_block(luma, stride, x, l.row2_y, l.block_w, l.block_h, ((time >> bit) & 1ull) ? kLight : kDark);
        x += l.block_w;
    }
}

struct Decoded {
    bool is_pattern = false;
    uint32_t number = 0;
    uint64_t time_low = 0;  // the low 48 bits of the time the frame was made
};

// One square's value, from a few pixels around its middle.
inline bool read_block(const uint8_t* luma, size_t stride, uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    const uint32_t cx = x + w / 2;
    const uint32_t cy = y + h / 2;
    uint32_t sum = 0;
    for (int dy = -1; dy <= 1; ++dy) {
        for (int dx = -1; dx <= 1; ++dx) {
            sum += luma[(cy + dy) * stride + (cx + dx)];
        }
    }
    return sum / 9 > 128;
}

inline Decoded read_code(const uint8_t* luma, size_t stride, uint32_t width, uint32_t height) {
    Decoded decoded;
    const Layout l = layout(width, height);
    if (l.block_w < 4 || l.block_h < 4) {
        return decoded;
    }
    uint32_t x = l.x0;
    for (uint8_t bit : kPreamble) {
        if (read_block(luma, stride, x, l.row1_y, l.block_w, l.block_h) != (bit != 0)) {
            return decoded;
        }
        x += l.block_w;
    }
    for (int bit = 0; bit < kNumberBits; ++bit) {
        decoded.number = (decoded.number << 1) | (read_block(luma, stride, x, l.row1_y, l.block_w, l.block_h) ? 1u : 0u);
        x += l.block_w;
    }
    x = l.x0;
    for (int bit = 0; bit < kTimeBits; ++bit) {
        decoded.time_low = (decoded.time_low << 1) | (read_block(luma, stride, x, l.row2_y, l.block_w, l.block_h) ? 1ull : 0ull);
        x += l.block_w;
    }
    decoded.is_pattern = true;
    return decoded;
}

// The whole time a frame was made, from its low 48 bits and the present time.
// 48 bits of 100 ns wrap after 325 days of uptime, far longer than a frame travels.
inline int64_t full_time(uint64_t time_low, int64_t now_100ns) {
    const uint64_t mask = (1ull << kTimeBits) - 1;
    uint64_t candidate = (static_cast<uint64_t>(now_100ns) & ~mask) | time_low;
    if (candidate > static_cast<uint64_t>(now_100ns) + (1ull << (kTimeBits - 1))) {
        candidate -= (1ull << kTimeBits);
    }
    return static_cast<int64_t>(candidate);
}

// Draws the test picture in NV12. The bars are drawn once; each frame only
// moves the block and rewrites the squares.
class Renderer {
public:
    Renderer(uint32_t width, uint32_t height) : width_(width), height_(height) {
        const size_t luma = static_cast<size_t>(width) * height;
        background_.resize(luma + luma / 2);
        for (uint32_t y = 0; y < height; ++y) {
            for (uint32_t x = 0; x < width; ++x) {
                background_[static_cast<size_t>(y) * width + x] = kBars[x * kBarCount / width][0];
            }
        }
        uint8_t* chroma = background_.data() + luma;
        for (uint32_t y = 0; y < height / 2; ++y) {
            for (uint32_t x = 0; x < width / 2; ++x) {
                const auto& bar = kBars[(x * 2) * kBarCount / width];
                chroma[static_cast<size_t>(y) * width + x * 2] = bar[1];
                chroma[static_cast<size_t>(y) * width + x * 2 + 1] = bar[2];
            }
        }
        // A dark band behind the squares, so they read the same over every bar.
        const Layout l = layout(width, height);
        fill_block(background_.data(), width, 0, 0, width, l.row2_y + l.block_h + l.block_h / 2, 64);
        frame_ = background_;
    }

    uint32_t width() const { return width_; }
    uint32_t height() const { return height_; }

    // The picture for frame `number`, made at `time_100ns`.
    const std::vector<uint8_t>& render(uint32_t number, int64_t time_100ns) {
        const uint32_t side = height_ / 6;
        const Layout l = layout(width_, height_);
        const uint32_t top = l.row2_y + l.block_h * 2;
        // Put back what the block covered, then draw it at its new place.
        copy_luma_rect(block_x_, block_y_, side);
        const uint32_t travel_x = width_ - side;
        const uint32_t travel_y = height_ - side - top;
        block_x_ = triangle(number * 7u, travel_x);
        block_y_ = top + triangle(number * 5u, travel_y);
        fill_block(frame_.data(), width_, block_x_, block_y_, side, side, 235);
        draw_code(frame_.data(), width_, width_, height_, number, time_100ns);
        return frame_;
    }

private:
    // A back-and-forth sweep over 0..range.
    static uint32_t triangle(uint32_t t, uint32_t range) {
        if (range == 0) {
            return 0;
        }
        const uint32_t phase = t % (2 * range);
        return phase < range ? phase : 2 * range - phase;
    }

    void copy_luma_rect(uint32_t x, uint32_t y, uint32_t side) {
        for (uint32_t row = 0; row < side && y + row < height_; ++row) {
            const size_t at = static_cast<size_t>(y + row) * width_ + x;
            std::memcpy(frame_.data() + at, background_.data() + at, side);
        }
    }

    uint32_t width_;
    uint32_t height_;
    uint32_t block_x_ = 0;
    uint32_t block_y_ = 0;
    std::vector<uint8_t> background_;
    std::vector<uint8_t> frame_;
};

}  // namespace gc_pattern
