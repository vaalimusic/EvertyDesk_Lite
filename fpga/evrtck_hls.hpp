//  EVRTCK HLS — EvertyDesk Remote Transport Codec, FPGA IP Core
//
//  Target:  Xilinx Alveo U50 / U200 (Vitis HLS 2023.x)
//  Clock:   300 MHz  →  3.33 ns / такт
//
//  Pipeline (все стадии параллельны через DATAFLOW):
//
//    DDR ──► [FrameReader] ──► [DirtyDetector] ──► [TileClassifier]
//                                                         │
//                                              ┌──────────┴──────────┐
//                                         [SolidPath]          [DeltaPath]
//                                              └──────────┬──────────┘
//                                                    [ZrleEncoder]
//                                                         │
//                                                  [PacketWriter] ──► DDR
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

static constexpr int TILE_PX   = 32;                     // пикселей на сторону тайла
static constexpr int TILE_BYTES = TILE_PX * TILE_PX * 4; // байт (RGBA)
static constexpr int AXI_W     = 512;                    // ширина AXI шины, бит
static constexpr int AXI_BYTES = AXI_W / 8;             // 64 байта/такт
static constexpr int BEATS_PER_TILE = TILE_BYTES / AXI_BYTES; // 64 такта/тайл

// Режимы тайла — идентичны программной EVRTCK
static constexpr ap_uint<8> MODE_SOLID = 1;
static constexpr ap_uint<8> MODE_DELTA = 2;

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

// Данные тайла после классификации
struct TilePayload {
    ap_uint<16>              tile_id;
    ap_uint<8>               mode;
    ap_uint<32>              color;   // MODE_SOLID: packed RGBA
    ap_uint<8>               delta[TILE_BYTES]; // MODE_DELTA: XOR delta
};

// Выходной байт пакета
using OutByte = ap_axiu<8, 1, 1, 1>;

// ── Интерфейс ядра (top-level функция) ───────────────────────────────────────

/// Точка входа Vitis HLS.
/// current_frame / prev_frame — AXI4-Master (чтение из DDR).
/// out_buf — AXI4-Master (запись готового EVRTCK-пакета).
void evrtck_encode_core(
    const ap_uint<AXI_W>* current_frame,  // #pragma HLS INTERFACE m_axi port=current_frame
    const ap_uint<AXI_W>* prev_frame,     // #pragma HLS INTERFACE m_axi port=prev_frame
    ap_uint<8>*            out_buf,        // #pragma HLS INTERFACE m_axi port=out_buf
    ap_uint<32>&           out_size,       // #pragma HLS INTERFACE s_axilite
    ap_uint<32>            frame_id,       // #pragma HLS INTERFACE s_axilite
    ap_uint<32>            width,          // #pragma HLS INTERFACE s_axilite
    ap_uint<32>            height          // #pragma HLS INTERFACE s_axilite
);
