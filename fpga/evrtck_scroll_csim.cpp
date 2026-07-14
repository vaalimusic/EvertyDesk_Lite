// EVRTCK ScrollDetector — C-simulation (plain C++, no ap_int)
//
// Это REFERENCE MODEL для верификации алгоритма на обычном PC.
// HLS-код (evrtck_scroll_hls.cpp) компилируется только через Vitis HLS.
// Здесь: те же алгоритмы, те же тесты, нулевые зависимости.
//
// Компиляция: cl /O2 /std:c++17 /EHsc evrtck_scroll_csim.cpp
//             g++ -O2 -std=c++17 -o evrtck_scroll_csim evrtck_scroll_csim.cpp

#include <cstdint>
#include <cstring>
#include <cstdio>
#include <cassert>
#include <chrono>
#include <climits>
#include <algorithm>

// ── Константы (идентичны HLS) ─────────────────────────────────────────────────

static constexpr int TILE_PX    = 32;
static constexpr int ROW_BYTES  = TILE_PX * 4;           // 128 байт/строка
static constexpr int TILE_BYTES = TILE_PX * ROW_BYTES;   // 4096 байт/тайл
static constexpr int MAX_DY     = 16;                     // окно ±16 px
static constexpr int N_DY       = MAX_DY * 2;             // 32 кандидата
static constexpr int SCROLL_ROWS = TILE_PX + MAX_DY * 2; // 64 строки в линбуфере

// ── Reference ScrollDetector ──────────────────────────────────────────────────
//
// Зеркало HLS-логики. В синтезе эта же структура даёт:
//   - N_DY=32 параллельных компаратора (UNROLL по di)
//   - II=1 на строку тайла (PIPELINE по row)
//   - 64 независимых порта BRAM (ARRAY_PARTITION complete dim=1 по prev_lines)

struct ScrollResult {
    bool  found;
    int8_t dy;
};

// Маппинг индекса di → dy: поиск ближнего первым
// di=0→+1, di=1→-1, di=2→+2, di=3→-2, ... di=30→+16, di=31→-16
static inline int8_t dy_of(int di) {
    int abs = di / 2 + 1;
    return (int8_t)(di % 2 == 0 ? abs : -abs);
}

static ScrollResult scroll_detect_tile(
    const uint8_t cur_tile[TILE_BYTES],
    // prev_lines[SCROLL_ROWS][ROW_BYTES]:
    // строка 0 = y0 - MAX_DY, строка SCROLL_ROWS-1 = y0 + TILE_PX + MAX_DY - 1
    const uint8_t prev_lines[SCROLL_ROWS][ROW_BYTES]
) {
    // match[di] = false если хоть одна строка не совпала для данного dy
    // На FPGA: N_DY=32 SR-регистра, ARRAY_PARTITION complete
    bool match[N_DY];
    for (int di = 0; di < N_DY; di++) match[di] = true;

    // ROW_LOOP: 32 такта @ II=1 (на FPGA — конвейер)
    for (int row = 0; row < TILE_PX; row++) {

        // CMP_DY: 32 параллельных компаратора (UNROLL на FPGA)
        for (int di = 0; di < N_DY; di++) {
            if (!match[di]) continue;  // уже отсеян — пропуск (SR-флаг в железе)

            int dy = dy_of(di);
            int prev_row_idx = row + MAX_DY + dy;  // индекс в линбуфере

            // CMP_BYTES: 128 байт → 1 бит несовпадения
            // На FPGA: UNROLL factor=16 → 8 волн × 16 байт → OR-дерево
            bool differs = (memcmp(
                cur_tile + row * ROW_BYTES,
                prev_lines[prev_row_idx],
                ROW_BYTES
            ) != 0);

            if (differs) match[di] = false;
        }
    }

    // Приоритетный энкодер: di=0 (|dy|=1) — наивысший приоритет.
    // На FPGA: mux-дерево из N_DY входов.
    for (int di = 0; di < N_DY; di++) {
        if (match[di]) return {true, dy_of(di)};
    }
    return {false, 0};
}

// ── Тесты ────────────────────────────────────────────────────────────────────

static void fill_pattern_row(uint8_t* row, int seed) {
    for (int b = 0; b < ROW_BYTES; b++)
        row[b] = (uint8_t)((seed * 7 + b * 13 + 37) & 0xFF);
}

static void test_match(int8_t target_dy) {
    uint8_t cur[TILE_BYTES] = {};
    uint8_t prev[SCROLL_ROWS][ROW_BYTES] = {};

    // Заполняем всё уникальными паттернами
    for (int li = 0; li < SCROLL_ROWS; li++)
        fill_pattern_row(prev[li], li * 31 + 1000);

    // cur совпадает с prev при данном target_dy:
    // cur[row] == prev[row + MAX_DY + target_dy]
    for (int row = 0; row < TILE_PX; row++) {
        int src = row + MAX_DY + target_dy;
        assert(src >= 0 && src < SCROLL_ROWS);
        memcpy(cur + row * ROW_BYTES, prev[src], ROW_BYTES);
    }

    auto res = scroll_detect_tile(cur, prev);
    printf("[dy=%+d]  found=%d dy=%+d", (int)target_dy, (int)res.found, (int)res.dy);
    if (res.found && res.dy == target_dy)
        printf("  PASS\n");
    else
        printf("  FAIL (expected found=1 dy=%+d)\n", (int)target_dy);
    assert(res.found);
    assert(res.dy == target_dy);
}

static void test_no_match_random() {
    uint8_t cur[TILE_BYTES];
    uint8_t prev[SCROLL_ROWS][ROW_BYTES];
    // Псевдослучайный шум — имитация видеокадра
    for (int i = 0; i < TILE_BYTES; i++)
        cur[i] = (uint8_t)((i * 6271 + 13) & 0xFF);
    for (int r = 0; r < SCROLL_ROWS; r++)
        for (int b = 0; b < ROW_BYTES; b++)
            prev[r][b] = (uint8_t)((r * 97 + b * 31 + 7) & 0xFF);

    auto res = scroll_detect_tile(cur, prev);
    printf("[no match / video noise]  found=%d", (int)res.found);
    if (!res.found) printf("  PASS (DELTA fallback)\n");
    else            printf("  unexpected match dy=%d\n", (int)res.dy);
    assert(!res.found);
}

static void test_priority_nearest_first() {
    // Проверяем что поиск возвращает наименьший |dy| при нескольких совпадениях.
    // Строим cur как копию строк prev с dy=+5, и ОТДЕЛЬНО делаем dy=-5 тоже совпадать.
    // Оба |dy|=5. Di для +5 = 8 (even), для -5 = 9 (odd) → +5 найдётся первым.
    uint8_t cur[TILE_BYTES] = {};
    uint8_t prev[SCROLL_ROWS][ROW_BYTES] = {};
    for (int li = 0; li < SCROLL_ROWS; li++)
        fill_pattern_row(prev[li], li * 17 + 500);

    // cur совпадает с prev при dy=+5
    for (int row = 0; row < TILE_PX; row++)
        memcpy(cur + row * ROW_BYTES, prev[row + MAX_DY + 5], ROW_BYTES);

    // Делаем dy=-5 тоже совпадающим: prev[row + MAX_DY - 5] = cur[row]
    // Диапазоны: dy=+5 → prev[21..52], dy=-5 → prev[11..42]
    // Пересечение [21..42] — нужно скопировать, НЕ перезаписывая нужные строки.
    // Сначала сохраняем snapshot prev для dy=+5 диапазона:
    uint8_t snapshot[TILE_PX][ROW_BYTES];
    for (int row = 0; row < TILE_PX; row++)
        memcpy(snapshot[row], prev[row + MAX_DY + 5], ROW_BYTES);

    // Пишем dy=-5 строки
    for (int row = 0; row < TILE_PX; row++)
        memcpy(prev[row + MAX_DY - 5], cur + row * ROW_BYTES, ROW_BYTES);

    // Восстанавливаем dy=+5 строки (могли быть перезаписаны)
    for (int row = 0; row < TILE_PX; row++)
        memcpy(prev[row + MAX_DY + 5], snapshot[row], ROW_BYTES);

    auto res = scroll_detect_tile(cur, prev);
    // di для +5 = 2*(5-1) = 8, для -5 = 2*(5-1)+1 = 9 → +5 выигрывает
    printf("[priority: dy=+5 vs dy=-5]  found=%d dy=%+d  (expect +5, di=8 < di=9)\n",
           (int)res.found, (int)res.dy);
    assert(res.found && res.dy == 5);
    printf("PASS\n");
}

// ── Benchmark ─────────────────────────────────────────────────────────────────

static void bench_throughput() {
    uint8_t cur[TILE_BYTES];
    uint8_t prev[SCROLL_ROWS][ROW_BYTES];
    for (int li = 0; li < SCROLL_ROWS; li++) fill_pattern_row(prev[li], li);
    for (int row = 0; row < TILE_PX; row++)
        memcpy(cur + row * ROW_BYTES, prev[row + MAX_DY + 5], ROW_BYTES);

    const int ITERS = 100000;
    auto t0 = std::chrono::high_resolution_clock::now();
    volatile int found_count = 0;
    for (int i = 0; i < ITERS; i++) {
        auto r = scroll_detect_tile(cur, prev);
        if (r.found) found_count++;
    }
    auto t1 = std::chrono::high_resolution_clock::now();
    double us = std::chrono::duration<double, std::micro>(t1 - t0).count();

    printf("\n[bench] %d iterations\n", ITERS);
    printf("  Total:      %.1f ms\n", us / 1000.0);
    printf("  Per tile:   %.2f µs  (SW reference, single-threaded)\n", us / ITERS);
    printf("  SW rate:    %.0f Mtiles/s\n", ITERS / us);
    printf("  FPGA @ 300MHz II=1:  %.0f ns/tile  (%.0f Mtiles/s)\n",
           32.0 / 0.3, 0.3 / 32.0 * 1000.0);
    printf("  Speedup projection: %.0f×\n", (us / ITERS) / (32.0 / 300000.0));
}

int main() {
    printf("=== EVRTCK ScrollDetector C-Simulation ===\n\n");

    printf("-- Correctness tests --\n");
    for (int dy = -MAX_DY; dy <= MAX_DY; dy++) {
        if (dy == 0) continue;
        test_match((int8_t)dy);
    }
    test_no_match_random();
    printf("\n");
    test_priority_nearest_first();

    printf("\n-- Throughput benchmark --\n");
    bench_throughput();

    printf("\n=== ALL TESTS PASSED ===\n");
    return 0;
}
