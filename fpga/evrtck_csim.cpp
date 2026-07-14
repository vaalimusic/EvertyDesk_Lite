//  EVRTCK HLS — C-симуляция / тестбенч
//  Позволяет проверить корректность алгоритма без железа,
//  прямо на хосте через vitis_hls csim или обычный g++.

#include "evrtck_hls.hpp"
#include <cstdio>
#include <cstring>
#include <cassert>

// ── Вспомогательная: заполнить кадр ──────────────────────────────────────────

static void fill_solid(ap_uint<AXI_W>* buf, int w, int h, uint8_t r, uint8_t g, uint8_t b, uint8_t a) {
    int total_bytes = w * h * 4;
    int beats = (total_bytes + AXI_BYTES - 1) / AXI_BYTES;
    for (int i = 0; i < beats; i++) {
        ap_uint<AXI_W> beat = 0;
        for (int j = 0; j < AXI_BYTES; j += 4) {
            beat((j + 0) * 8 + 7, (j + 0) * 8) = r;
            beat((j + 1) * 8 + 7, (j + 1) * 8) = g;
            beat((j + 2) * 8 + 7, (j + 2) * 8) = b;
            beat((j + 3) * 8 + 7, (j + 3) * 8) = a;
        }
        buf[i] = beat;
    }
}

// ── Тест 1: статичный кадр → tiny output ─────────────────────────────────────

static void test_static_frame() {
    constexpr int W = 1920, H = 1080;
    constexpr int BEATS = (W * H * 4 + AXI_BYTES - 1) / AXI_BYTES;

    static ap_uint<AXI_W> cur[BEATS], prev[BEATS];
    static ap_uint<8>     out[1 << 20];
    ap_uint<32> out_size = 0;

    fill_solid(cur,  W, H, 30, 30, 30, 255);
    fill_solid(prev, W, H, 30, 30, 30, 255);  // identical

    evrtck_encode_core(cur, prev, out, out_size, /*frame_id=*/2, W, H);

    // Статичный 1080p кадр: только заголовок + tile_map, нет данных тайлов
    // header=20, map_bytes_field=2, tile_map=ceil(2040/8)=255 → ≤280 байт
    printf("[test_static_frame] out_size = %u (expect ≤280)\n", (unsigned)out_size);
    assert(out_size <= 280);
    printf("PASS\n\n");
}

// ── Тест 2: один изменённый пиксель ──────────────────────────────────────────

static void test_single_pixel_change() {
    constexpr int W = 64, H = 64;
    constexpr int BEATS = (W * H * 4 + AXI_BYTES - 1) / AXI_BYTES;

    static ap_uint<AXI_W> cur[BEATS], prev[BEATS];
    static ap_uint<8>     out[1 << 20];
    ap_uint<32> out_size = 0;

    fill_solid(prev, W, H, 0, 0, 0, 255);
    fill_solid(cur,  W, H, 0, 0, 0, 255);

    // Меняем байт [0] (R канал первого пикселя, тайл 0,0)
    cur[0].range(7, 0) = 200;

    evrtck_encode_core(cur, prev, out, out_size, /*frame_id=*/3, W, H);

    printf("[test_single_pixel_change] out_size = %u\n", (unsigned)out_size);
    // Должен затронуть ровно 1 тайл; ZRLE должен сжать 4092 нуля хорошо
    assert(out_size < 100);
    printf("PASS\n\n");
}

// ── Тест 3: solid-color тайл ─────────────────────────────────────────────────

static void test_solid_tile() {
    constexpr int W = 32, H = 32;  // ровно 1 тайл
    constexpr int BEATS = (W * H * 4 + AXI_BYTES - 1) / AXI_BYTES;

    static ap_uint<AXI_W> cur[BEATS], prev[BEATS];
    static ap_uint<8>     out[1 << 20];
    ap_uint<32> out_size = 0;

    fill_solid(prev, W, H, 0, 0, 0,   0);
    fill_solid(cur,  W, H, 255, 0, 0, 255); // красный

    evrtck_encode_core(cur, prev, out, out_size, /*frame_id=*/1, W, H);

    printf("[test_solid_tile] out_size = %u (expect: header+map+mode+color = 27)\n", (unsigned)out_size);
    // header(20) + map_bytes_field(2) + tile_map(1) + mode(1) + color(4) = 28
    assert(out_size <= 30);
    printf("PASS\n\n");
}

int main() {
    printf("=== EVRTCK HLS C-Simulation ===\n\n");
    test_static_frame();
    test_single_pixel_change();
    test_solid_tile();
    printf("=== All tests PASSED ===\n");
    return 0;
}
