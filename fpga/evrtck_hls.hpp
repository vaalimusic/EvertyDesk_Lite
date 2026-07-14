//  EVRTCK HLS — EvertyDesk Remote Transport Codec, FPGA IP Core
//
//  Target:  Xilinx Alveo U50 / U200 (Vitis HLS 2023.x)
//  Clock:   300 MHz  →  3.33 ns / такт
//
//  Pipeline (все стадии параллельны через DATAFLOW):
//
//    DDR ──► [FrameReader] ──► [DirtyDetector] ──► [ScrollAndClassify] ◄── prev_frame DDR
//                                                         │
//                                    ┌───────────┬────────┴────────┐
//                               SCROLL(2B)   SOLID(5B)        DELTA+ZRLE
//                                    └───────────┴────────┬────────┘
//                                                   [ZrleEncoder]
//                                                         │
//                                                  [PacketWriter] ──► DDR
//
//  ScrollDetector (внутри ScrollAndClassify):
//    - MAX_DY=16, N_DY=32 параллельных компаратора (#pragma HLS UNROLL)
//    - LineBuf [SCROLL_ROWS][ROW_BYTES] в BRAM (#pragma HLS ARRAY_PARTITION complete dim=1)
//    - Early Bypass: SCROLL-тайл = 2 байта, ZrleEncoder не задействован
//
//  Пропускная способность (расчётная @ 300 MHz, 512-bit AXI шина):
//    Чтение тайла 32×32×4 = 4096 байт: 64 такта @ 300 MHz = 213 нс
//    1080p = 2040 тайлов: при полном конвейере ~435 µs на кадр (>2000 fps)
//    На один Alveo U50: 8 параллельных ядер → 16 000 fps суммарно → 266+ VM @ 60fps

#pragma once

#include <ap_int.h>
#include <ap_axi_sdata.h>
#include <hls_stream.h>

// ── Константы ────────────────────────────────────────────────────────────────

static constexpr int TILE_PX      = 32;                       // пикселей на сторону тайла
static constexpr int TILE_BYTES   = TILE_PX * TILE_PX * 4;   // байт (RGBA)
static constexpr int AXI_W        = 512;                      // ширина AXI шины, бит
static constexpr int AXI_BYTES    = AXI_W / 8;               // 64 байта/такт
static constexpr int BEATS_PER_TILE = TILE_BYTES / AXI_BYTES; // 64 такта/тайл

// ScrollDetector — окно поиска ±MAX_DY строк
static constexpr int ROW_BYTES    = TILE_PX * 4;              // 128 байт/строка
static constexpr int MAX_DY       = 16;                       // окно ±16 px
static constexpr int N_DY         = MAX_DY * 2;               // 32 кандидата
static constexpr int SCROLL_ROWS  = TILE_PX + N_DY;           // 64 строки в LineBuf

// Режимы тайла — идентичны программной EVRTCK v2
static constexpr ap_uint<8> MODE_CLEAN  = 0;  // тайл не изменился (sentinel)
static constexpr ap_uint<8> MODE_SOLID  = 1;
static constexpr ap_uint<8> MODE_DELTA  = 2;
static constexpr ap_uint<8> MODE_SCROLL = 3;  // вертикальный сдвиг dy

// ── Типы потоков ─────────────────────────────────────────────────────────────

// Один AXI-beat текущего + предыдущего кадра, помечен last
struct FrameBeat {
    ap_uint<AXI_W> cur;
    ap_uint<AXI_W> prev;
    ap_uint<1>     last;  // последний beat тайла
    ap_uint<16>    tile_id;
};

// Результат детектора грязных тайлов
struct DirtyResult {
    ap_uint<16> tile_id;
    ap_uint<1>  dirty;
};

// Данные тайла после классификации (все режимы)
struct TilePayload {
    ap_uint<16>  tile_id;
    ap_uint<8>   mode;                 // MODE_CLEAN/SOLID/DELTA/SCROLL
    ap_uint<32>  color;                // MODE_SOLID: packed RGBA
    ap_int<8>    dy;                   // MODE_SCROLL: сдвиг в пикселях
    ap_uint<8>   delta[TILE_BYTES];    // MODE_DELTA: XOR delta
};

// Малый заголовок тайла — используется в tile_order stream (без 4KB delta)
struct TileHeader {
    ap_uint<16>  tile_id;
    ap_uint<8>   mode;
    ap_uint<32>  color;                // SOLID: цвет; DELTA: enc_len
    ap_int<8>    dy;                   // SCROLL: сдвиг
};

// Выходной байт пакета
using OutByte = ap_axiu<8, 1, 1, 1>;

// ── Интерфейс ядра (top-level функция) ───────────────────────────────────────

/// Точка входа Vitis HLS.
/// current_frame / prev_frame — AXI4-Master (чтение из DDR).
/// prev_frame_scroll — тот же DDR-регион prev_frame, но отдельный AXI-Master
///   bundle=gmem3, чтобы ScrollDetector читал параллельно с FrameReader.
/// out_buf — AXI4-Master (запись готового EVRTCK v2-пакета).
void evrtck_encode_core(
    const ap_uint<AXI_W>* current_frame,       // bundle=gmem0
    const ap_uint<AXI_W>* prev_frame,          // bundle=gmem1
    const ap_uint<AXI_W>* prev_frame_scroll,   // bundle=gmem3 (same DDR, 2nd AXI port)
    ap_uint<8>*            out_buf,             // bundle=gmem2
    ap_uint<32>&           out_size,
    ap_uint<32>            frame_id,
    ap_uint<32>            width,
    ap_uint<32>            height
);
