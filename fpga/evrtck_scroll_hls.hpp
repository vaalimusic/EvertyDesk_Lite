//  EVRTCK ScrollDetector — HLS IP Core
//
//  Target: Xilinx Alveo U50, Vitis HLS 2023.x, 300 MHz
//
//  Архитектура:
//    N_DY параллельных аппаратных компараторов (создаются через UNROLL).
//    Каждый компаратор — конвейер из TILE_PX стадий (одна строка за такт).
//    Предыдущий кадр кэшируется в Line Buffer (BRAM), порезанный через
//    ARRAY_PARTITION complete dim=1 — каждая строка на своём порту памяти,
//    все N_DY компараторов читают параллельно без конкуренции за порты.
//
//  Latency: TILE_PX тактов = 32 такта = 106 нс @ 300 MHz
//  Throughput (II=1): один тайл каждые 32 такта — все тайлы потоком.

#pragma once

#include "ap_int.h"

// ── Параметры ─────────────────────────────────────────────────────────────────

static constexpr int TILE_PX   = 32;
static constexpr int ROW_BYTES = TILE_PX * 4;          // 128 байт/строка
static constexpr int TILE_BYTES = TILE_PX * ROW_BYTES;  // 4096 байт/тайл
static constexpr int MAX_DY    = 16;                    // окно поиска ±16 px
static constexpr int N_DY      = MAX_DY * 2;            // 32 кандидата (без dy=0)
static constexpr int SCROLL_ROWS = TILE_PX + MAX_DY * 2; // 64 строки в line buffer

// ── Типы ─────────────────────────────────────────────────────────────────────

// Входной тайл: текущий кадр (128 байт × 32 строки)
struct CurTile {
    ap_uint<8> data[TILE_BYTES];
};

// Строка line buffer (128 байт пикселей предыдущего кадра)
struct PrevRow {
    ap_uint<8> data[ROW_BYTES];
};

// Результат детектора
struct ScrollResult {
    ap_uint<1>  found;   // 1 = совпадение найдено
    ap_int<8>   dy;      // сдвиг в пикселях (только если found=1)
};

// ── Top-level функция ─────────────────────────────────────────────────────────

/// Определяет вертикальный сдвиг тайла.
///
/// @param cur_tile   128×32 = 4096 байт текущего тайла (RGBA)
/// @param prev_lines SCROLL_ROWS=64 строк предыдущего кадра вокруг позиции тайла:
///                   строки [y0-MAX_DY .. y0+TILE_PX+MAX_DY-1]
/// @param result     {found, dy}: если found=1, cur_tile совпадает с
///                   prev_lines сдвинутым на dy пикселей
void scroll_detect_tile(
    const CurTile&   cur_tile,
    const PrevRow    prev_lines[SCROLL_ROWS],
    ScrollResult&    result
);

/// Загружает нужные строки предыдущего кадра в line buffer из DDR.
/// Вызывается один раз на тайл перед scroll_detect_tile.
void load_prev_lines(
    const ap_uint<256>* prev_frame_ddr,  // AXI4-Master, 512-bit шина
    ap_uint<32>         frame_width,      // ширина кадра в пикселях
    ap_uint<32>         tile_y0,          // y-координата начала тайла
    ap_uint<32>         frame_height,
    PrevRow             out_lines[SCROLL_ROWS]
);
