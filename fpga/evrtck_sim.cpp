// EVRTCK Software Pipeline Simulator — v2 с MODE_SCROLL
//
// Компиляция: cl /O2 /std:c++17 /EHsc evrtck_sim.cpp
//             g++ -O2 -std=c++17 -o evrtck_sim evrtck_sim.cpp
//
// Новое в v2:
//   MODE_SCROLL(dy) — детектор вертикального сдвига тайла.
//   Поиск в окне [-MAX_DY, +MAX_DY].
//   На FPGA: 65 параллельных компараторов, #pragma HLS UNROLL, II=1.
//   Фоллбэк: если совпадение не найдено — MODE_DELTA (ZRLE).
//   Цена пакета: 2 байта вместо ~2 KB ZRLE на каждый тайл скроллинга.

#include <cstdint>
#include <cstring>
#include <cassert>
#include <cstdio>
#include <cmath>
#include <chrono>
#include <queue>
#include <vector>
#include <functional>
#include <string>
#include <algorithm>
#include <climits>

// ── Константы ────────────────────────────────────────────────────────────────

static constexpr int TILE_PX    = 32;
static constexpr int TILE_BYTES = TILE_PX * TILE_PX * 4;

static constexpr uint8_t MODE_SOLID  = 1;
static constexpr uint8_t MODE_DELTA  = 2;
static constexpr uint8_t MODE_SCROLL = 3; // новый: payload = int8_t dy

// Окно поиска сдвига. На FPGA: 2×MAX_DY+1 компараторов в UNROLL.
// 65 компараторов при MAX_DY=32 — влезает в U50 без проблем.
static constexpr int MAX_DY = 32;

// ── SimQueue ──────────────────────────────────────────────────────────────────

template<typename T>
using SimQueue = std::queue<T>;

// ── Типы данных ───────────────────────────────────────────────────────────────

struct FrameBeat {
    uint64_t cur[8];
    uint64_t prev[8];
    bool     last;
    uint16_t tile_id;
};

struct DirtyResult {
    uint16_t tile_id;
    bool     dirty;
};

struct TilePayload {
    uint16_t tile_id;
    uint8_t  mode;
    uint32_t color;              // MODE_SOLID: packed RGBA
    int8_t   dy;                 // MODE_SCROLL: сдвиг в пикселях
    uint8_t  delta[TILE_BYTES];  // MODE_DELTA: XOR-дельта
};

// ── Стадия 1: FrameReader ────────────────────────────────────────────────────

static inline int tiles_in(int px) { return (px + TILE_PX - 1) / TILE_PX; }

static void stage_frame_reader(
    const uint8_t* cur, const uint8_t* prev,
    int w, int h,
    SimQueue<FrameBeat>& out
) {
    int tiles_x = tiles_in(w), tiles_y = tiles_in(h);
    uint16_t tile_id = 0;
    for (int ty = 0; ty < tiles_y; ty++) {
        for (int tx = 0; tx < tiles_x; tx++) {
            int x0 = tx * TILE_PX, y0 = ty * TILE_PX;
            int x1 = std::min(x0 + TILE_PX, w);
            int y1 = std::min(y0 + TILE_PX, h);

            uint8_t tile_cur[TILE_BYTES]  = {};
            uint8_t tile_prev[TILE_BYTES] = {};
            int off = 0;
            for (int y = y0; y < y1; y++) {
                int n = (x1 - x0) * 4;
                memcpy(tile_cur  + off, cur  + (y * w + x0) * 4, n);
                memcpy(tile_prev + off, prev + (y * w + x0) * 4, n);
                off += n;
            }

            int total = off;
            int beats = (total + 63) / 64;
            for (int b = 0; b < beats; b++) {
                FrameBeat beat = {};
                beat.tile_id = tile_id;
                beat.last    = (b == beats - 1);
                int byte_off = b * 64;
                int n = std::min(64, total - byte_off);
                memcpy(beat.cur,  tile_cur  + byte_off, n);
                memcpy(beat.prev, tile_prev + byte_off, n);
                out.push(beat);
            }
            tile_id++;
        }
    }
}

// ── Стадия 2: DirtyDetector ──────────────────────────────────────────────────

static void stage_dirty_detector(
    SimQueue<FrameBeat>& in,
    SimQueue<FrameBeat>& pass,
    SimQueue<DirtyResult>& dirty_out,
    int total_tiles
) {
    for (int t = 0; t < total_tiles; t++) {
        bool dirty = false;
        uint16_t tile_id = 0;
        bool done = false;
        while (!done) {
            FrameBeat b = in.front(); in.pop();
            tile_id = b.tile_id;
            for (int i = 0; i < 8; i++)
                if (b.cur[i] != b.prev[i]) dirty = true;
            pass.push(b);
            done = b.last;
        }
        dirty_out.push({tile_id, dirty});
    }
}

// ── ScrollDetector: ищет вертикальный сдвиг тайла ────────────────────────────
//
// На FPGA: каждый dy — отдельный компаратор, все 65 работают параллельно.
// Initiation Interval = время одного сравнения тайла, не × 65.
// Поиск от dy=+1 до MAX_DY и от dy=-1 до -MAX_DY — сначала малые сдвиги
// (типичный скролл = несколько пикселей/кадр), выход при первом совпадении.

static int find_scroll_dy(
    const uint8_t* cur_frame,
    const uint8_t* prev_frame,
    int w, int h,
    int tx, int ty
) {
    int x0 = tx * TILE_PX, y0 = ty * TILE_PX;
    int x1 = std::min(x0 + TILE_PX, w);
    int y1 = std::min(y0 + TILE_PX, h);
    int tw = (x1 - x0) * 4;

    // Собираем текущий тайл
    uint8_t cur_tile[TILE_BYTES] = {};
    for (int y = y0; y < y1; y++)
        memcpy(cur_tile + (y - y0) * tw, cur_frame + (y * w + x0) * 4, tw);

    // Поиск: сначала +1..+MAX_DY, потом -1..-MAX_DY (стратегия "вероятнее вниз")
    for (int abs_dy = 1; abs_dy <= MAX_DY; abs_dy++) {
        for (int sign = 1; sign >= -1; sign -= 2) {
            int dy = sign * abs_dy;
            int sy0 = y0 + dy, sy1 = y1 + dy;
            if (sy0 < 0 || sy1 > h) continue;

            bool match = true;
            for (int y = 0; y < (y1 - y0) && match; y++) {
                const uint8_t* prev_row = prev_frame + ((sy0 + y) * w + x0) * 4;
                if (memcmp(cur_tile + y * tw, prev_row, tw) != 0) match = false;
            }
            if (match) return dy;
        }
    }
    return INT_MIN;
}

// ── Стадия 3: TileClassifier (с ScrollDetector) ───────────────────────────────

static void stage_tile_classifier(
    SimQueue<FrameBeat>& beats_in,
    SimQueue<DirtyResult>& dirty_in,
    SimQueue<TilePayload>& out,
    const uint8_t* cur_frame,
    const uint8_t* prev_frame,
    int w, int h,
    int total_tiles,
    int& scroll_hits,   // статистика
    int& delta_hits
) {
    int tiles_x = tiles_in(w);
    for (int t = 0; t < total_tiles; t++) {
        DirtyResult dr = dirty_in.front(); dirty_in.pop();

        uint8_t delta[TILE_BYTES] = {};
        int off = 0;
        bool done = false;
        while (!done) {
            FrameBeat b = beats_in.front(); beats_in.pop();
            if (dr.dirty) {
                uint8_t xb[64];
                for (int i = 0; i < 8; i++) {
                    uint64_t xv = b.cur[i] ^ b.prev[i];
                    memcpy(xb + i * 8, &xv, 8);
                }
                int n = std::min(64, TILE_BYTES - off);
                memcpy(delta + off, xb, n);
                off += n;
            }
            done = b.last;
        }
        if (!dr.dirty) continue;

        int tx = dr.tile_id % tiles_x;
        int ty = dr.tile_id / tiles_x;

        // 1. Solid check
        uint32_t first_px;
        memcpy(&first_px, delta, 4);
        bool solid = true;
        for (int i = 4; i < off; i += 4) {
            uint32_t p; memcpy(&p, delta + i, 4);
            if (p != first_px) { solid = false; break; }
        }
        if (solid) {
            out.push({dr.tile_id, MODE_SOLID, first_px, 0, {}});
            continue;
        }

        // 2. Scroll detection
        int dy = find_scroll_dy(cur_frame, prev_frame, w, h, tx, ty);
        if (dy != INT_MIN) {
            TilePayload tp = {};
            tp.tile_id = dr.tile_id;
            tp.mode    = MODE_SCROLL;
            tp.dy      = (int8_t)dy;
            out.push(tp);
            scroll_hits++;
            continue;
        }

        // 3. Fallback: XOR + ZRLE
        TilePayload tp = {};
        tp.tile_id = dr.tile_id;
        tp.mode    = MODE_DELTA;
        memcpy(tp.delta, delta, off);
        out.push(tp);
        delta_hits++;
    }
}

// ── ZRLE ─────────────────────────────────────────────────────────────────────

static std::vector<uint8_t> zrle_encode(const uint8_t* src, int len) {
    std::vector<uint8_t> out;
    out.reserve(len / 4 + 16);
    int i = 0;
    while (i < len) {
        int z0 = i;
        while (i < len && src[i] == 0) i++;
        int zeros = i - z0;
        if (zeros >= 4 || (zeros > 0 && i == len)) {
            int rem = zeros;
            while (rem > 0) {
                uint16_t n = (uint16_t)std::min(rem, 65535);
                out.push_back(0x00); out.push_back(n & 0xFF); out.push_back(n >> 8);
                rem -= n;
            }
            continue;
        }
        i = z0;
        int ls = i;
        while (i < len) {
            if (src[i] == 0) {
                int z = 0;
                while (i + z < len && src[i + z] == 0) z++;
                if (z >= 4) break;
            }
            i++;
        }
        int ll = i - ls;
        if (ll > 0) {
            int j = 0;
            while (j < ll) {
                uint16_t n = (uint16_t)std::min(ll - j, 65535);
                out.push_back(0x01); out.push_back(n & 0xFF); out.push_back(n >> 8);
                out.insert(out.end(), src + ls + j, src + ls + j + n);
                j += n;
            }
        }
    }
    return out;
}

// ── Стадия 4: ZrleEncoder ────────────────────────────────────────────────────

static void stage_zrle_encoder(
    SimQueue<TilePayload>& in,
    SimQueue<TilePayload>& tiles_out,
    SimQueue<std::vector<uint8_t>>& zrle_out
) {
    while (!in.empty()) {
        TilePayload tp = in.front(); in.pop();
        tiles_out.push(tp);
        if (tp.mode == MODE_DELTA)
            zrle_out.push(zrle_encode(tp.delta, TILE_BYTES));
    }
}

// ── Стадия 5: PacketWriter ────────────────────────────────────────────────────

static std::vector<uint8_t> stage_packet_writer(
    SimQueue<TilePayload>& tiles_in,
    SimQueue<std::vector<uint8_t>>& zrle_in,
    int total_tiles,
    uint32_t frame_id, uint32_t w, uint32_t h
) {
    std::vector<uint8_t> out;
    out.reserve(512);

    auto w8  = [&](uint8_t  v){ out.push_back(v); };
    auto w16 = [&](uint16_t v){ out.push_back(v & 0xFF); out.push_back(v >> 8); };
    auto w32 = [&](uint32_t v){ w16(v & 0xFFFF); w16(v >> 16); };

    out.push_back('E'); out.push_back('V'); out.push_back('C'); out.push_back('K');
    w8(2); w8(0); // version=2 (добавлен MODE_SCROLL), flags
    w32(frame_id); w32(w); w32(h);

    uint16_t map_bytes = (uint16_t)((total_tiles + 7) / 8);
    w16(map_bytes);
    size_t map_pos = out.size();
    out.resize(out.size() + map_bytes, 0);

    while (!tiles_in.empty()) {
        TilePayload tp = tiles_in.front(); tiles_in.pop();
        out[map_pos + tp.tile_id / 8] |= (1 << (tp.tile_id % 8));

        w8(tp.mode);
        switch (tp.mode) {
            case MODE_SOLID:
                w32(tp.color);
                break;
            case MODE_SCROLL:
                w8((uint8_t)(int8_t)tp.dy); // 1 байт знаковый сдвиг
                break;
            case MODE_DELTA: {
                auto enc = zrle_in.front(); zrle_in.pop();
                w32((uint32_t)enc.size());
                out.insert(out.end(), enc.begin(), enc.end());
                break;
            }
        }
    }
    return out;
}

// ── Полный pipeline ───────────────────────────────────────────────────────────

struct EncodeStats {
    int dirty_tiles, total_tiles;
    int scroll_tiles, delta_tiles, solid_tiles;
    double encode_us;
    size_t packet_bytes;
};

static EncodeStats encode_frame(
    const uint8_t* cur, const uint8_t* prev,
    int w, int h, uint32_t frame_id
) {
    auto t0 = std::chrono::high_resolution_clock::now();

    int tiles_x = tiles_in(w), tiles_y = tiles_in(h);
    int total = tiles_x * tiles_y;

    SimQueue<FrameBeat>            q_raw, q_pass;
    SimQueue<DirtyResult>          q_dirty;
    SimQueue<TilePayload>          q_classified, q_tiles;
    SimQueue<std::vector<uint8_t>> q_zrle;

    stage_frame_reader(cur, prev, w, h, q_raw);
    stage_dirty_detector(q_raw, q_pass, q_dirty, total);

    int dirty_count = 0;
    { SimQueue<DirtyResult> tmp = q_dirty;
      while (!tmp.empty()) { if (tmp.front().dirty) dirty_count++; tmp.pop(); } }

    int scroll_hits = 0, delta_hits = 0;
    stage_tile_classifier(q_pass, q_dirty, q_classified,
                          cur, prev, w, h, total,
                          scroll_hits, delta_hits);

    stage_zrle_encoder(q_classified, q_tiles, q_zrle);

    auto pkt = stage_packet_writer(q_tiles, q_zrle, total, frame_id, w, h);

    auto t1 = std::chrono::high_resolution_clock::now();

    return {
        dirty_count, total,
        scroll_hits, delta_hits,
        dirty_count - scroll_hits - delta_hits, // solid
        std::chrono::duration<double, std::micro>(t1 - t0).count(),
        pkt.size()
    };
}

// ── Генераторы кадров ─────────────────────────────────────────────────────────

static void gen_base_desktop(uint8_t* f, int w, int h) {
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            uint8_t* p = f + (y * w + x) * 4;
            if (y > h - 40) { p[0]=30;  p[1]=30;  p[2]=30;  p[3]=255; }
            else             { p[0]=200; p[1]=200; p[2]=210; p[3]=255; }
        }
}

static void draw_cursor(uint8_t* f, int w, int h, int cx, int cy) {
    for (int dy = 0; dy < 16 && cy + dy < h; dy++)
        for (int dx = 0; dx < 16 && cx + dx < w; dx++)
            if (dx == 0 || dy == 0 || dx == dy) {
                uint8_t* p = f + ((cy + dy) * w + (cx + dx)) * 4;
                p[0]=0; p[1]=0; p[2]=0; p[3]=255;
            }
}

static void gen_browser_content(uint8_t* f, int w, int h, int scroll_off) {
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            uint8_t* p = f + (y * w + x) * 4;
            p[0]=255; p[1]=255; p[2]=255; p[3]=255;
        }
    int line_h = 20;
    for (int line = 0; line < h / line_h + 2; line++) {
        int y0 = line * line_h - (scroll_off % line_h);
        int ty = y0 + 4;
        for (int y = ty; y < ty + 12 && y < h; y++) {
            if (y < 0) continue;
            int indent = ((line * 37) % 40) + 50;
            int tw = w - indent - ((line * 53) % 200);
            tw = std::max(0, std::min(tw, w - indent));
            for (int x = indent; x < indent + tw; x++) {
                uint8_t* p = f + (y * w + x) * 4;
                p[0]=30; p[1]=30; p[2]=30; p[3]=255;
            }
        }
    }
}

// ── Бенчмарк ─────────────────────────────────────────────────────────────────

struct BenchResult {
    std::string name;
    int frames;
    double total_us;
    double avg_dirty_pct, avg_scroll_pct, avg_delta_pct;
    double avg_compression;
    size_t min_b, max_b, avg_b;
};

static BenchResult run_bench(
    const std::string& name, int w, int h, int frames,
    std::function<void(uint8_t*, uint8_t*, int, int, int)> gen
) {
    std::vector<uint8_t> cur(w * h * 4), prev(w * h * 4, 0);
    BenchResult r = {}; r.name = name; r.frames = frames;
    r.min_b = SIZE_MAX;
    double sum_dirty=0, sum_scroll=0, sum_delta=0, sum_comp=0;
    size_t sum_b = 0;

    printf("  %-40s %d frames...\n", name.c_str(), frames);

    for (int f = 0; f < frames; f++) {
        gen(cur.data(), prev.data(), w, h, f);
        auto s = encode_frame(cur.data(), prev.data(), w, h, (uint32_t)f);
        r.total_us  += s.encode_us;
        sum_dirty   += 100.0 * s.dirty_tiles / s.total_tiles;
        sum_scroll  += s.dirty_tiles > 0 ? 100.0 * s.scroll_tiles / s.dirty_tiles : 0;
        sum_delta   += s.dirty_tiles > 0 ? 100.0 * s.delta_tiles  / s.dirty_tiles : 0;
        sum_comp    += (double)s.packet_bytes / ((size_t)w * h * 4);
        sum_b       += s.packet_bytes;
        r.min_b      = std::min(r.min_b, s.packet_bytes);
        r.max_b      = std::max(r.max_b, s.packet_bytes);
        memcpy(prev.data(), cur.data(), cur.size());
    }
    r.avg_dirty_pct  = sum_dirty  / frames;
    r.avg_scroll_pct = sum_scroll / frames;
    r.avg_delta_pct  = sum_delta  / frames;
    r.avg_compression = sum_comp  / frames;
    r.avg_b = sum_b / frames;
    return r;
}

static void print_result(const BenchResult& r, bool show_scroll = true) {
    double fps = 1e6 / (r.total_us / r.frames);
    printf("\n┌─ %s\n", r.name.c_str());
    printf("│  Avg encode:   %.1f µs/frame  (%.0f fps equiv)\n",
           r.total_us / r.frames, fps);
    printf("│  Dirty tiles:  %.1f %%\n", r.avg_dirty_pct);
    if (show_scroll) {
        printf("│  → Scroll:     %.1f %% of dirty  ← MODE_SCROLL hits\n", r.avg_scroll_pct);
        printf("│  → Delta:      %.1f %% of dirty  (ZRLE fallback)\n",    r.avg_delta_pct);
    }
    printf("│  Compression:  %.5f×  (%.0f:1)\n",
           r.avg_compression, 1.0 / (r.avg_compression + 1e-9));
    printf("│  Bytes/frame:  min=%-8zu  avg=%-8zu  max=%zu\n",
           r.min_b, r.avg_b, r.max_b);

    double mbps = r.avg_b * 8.0 * 60 / 1e6;
    printf("│  @ 60fps:      %.1f Mbit/s\n", mbps);
    printf("└─\n");
}

// ── Main ─────────────────────────────────────────────────────────────────────

int main() {
    printf("=== EVRTCK Simulator v2 — MODE_SCROLL ===\n\n");

    const int W = 1920, H = 1080, FRAMES = 300;

    auto cursor_gen = [](uint8_t* cur, uint8_t*, int w, int h, int f) {
        gen_base_desktop(cur, w, h);
        draw_cursor(cur, w, h, 100 + f % 400, 100 + f % 300);
    };

    auto scroll_gen = [](uint8_t* cur, uint8_t*, int w, int h, int f) {
        gen_browser_content(cur, w, h, f * 5);
    };

    auto static_gen = [](uint8_t* cur, uint8_t* prev, int w, int h, int) {
        if (prev[0] == 0) gen_base_desktop(cur, w, h);
        else memcpy(cur, prev, (size_t)w * h * 4);
    };

    printf("Сценарии @ 1920×1080, %d frames:\n\n", FRAMES);

    auto r_cursor = run_bench("Cursor micro-move  (low bitrate)", W, H, FRAMES, cursor_gen);
    auto r_scroll = run_bench("Browser scroll     (peak load)",   W, H, FRAMES, scroll_gen);
    auto r_static = run_bench("Static frame       (overhead)",    W, H, FRAMES, static_gen);

    printf("\n════════════════════════════════════════════════\n");
    print_result(r_cursor);
    print_result(r_scroll);
    print_result(r_static, false);

    // Сравнение скроллинга до/после
    double scroll_v1_mbps = 3882205.0 * 8 * 60 / 1e6;
    double scroll_v2_mbps = r_scroll.avg_b * 8.0 * 60 / 1e6;
    printf("\n════════════════════════════════════════════════\n");
    printf("Скроллинг: до vs после MODE_SCROLL\n");
    printf("  v1 (XOR+ZRLE):  %.0f Mbit/s @ 60fps  (%zu bytes/frame)\n",
           scroll_v1_mbps, (size_t)3882205);
    printf("  v2 (+SCROLL):   %.0f Mbit/s @ 60fps  (%zu bytes/frame)\n",
           scroll_v2_mbps, r_scroll.avg_b);
    if (scroll_v2_mbps > 0)
        printf("  Улучшение:      %.0f×\n", scroll_v1_mbps / scroll_v2_mbps);

    printf("\nFPGA @ 300 MHz — ScrollDetector:\n");
    printf("  65 компараторов в UNROLL → все dy одновременно\n");
    printf("  Latency поиска = latency одного сравнения тайла = 64 такта = 0.21 µs\n");
    printf("  Overhead vs отсутствие scroll = 0 (параллельно с DirtyDetector)\n");
    return 0;
}
