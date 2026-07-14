//  EVRTCK HLS v2 — реализация конвейерных стадий с ScrollDetector
//
//  Компилируется: vitis_hls -f run_hls.tcl
//  C-симуляция:   vitis_hls -f run_csim.tcl
//
//  Каждая функция — отдельная стадия DATAFLOW-конвейера.
//  Стадии обмениваются данными через hls::stream<> (аппаратные FIFO).
//
//  Изменения относительно v1:
//    - Стадия 3 переименована из TileClassifier → ScrollAndClassify
//    - ScrollDetector: 32 параллельных компаратора (#pragma HLS UNROLL N_DY)
//    - LineBuf: 64 строки × 128 байт, ARRAY_PARTITION complete dim=1
//    - MODE_SCROLL = 3: 2 байта на тайл вместо ~2 КБ (99× улучшение при скролле)
//    - MODE_CLEAN = 0: sentinel для чистых тайлов, позволяет loop total_tiles
//    - Версия пакета: EVCK v2
//    - Убран dirty_count — не нужен благодаря MODE_CLEAN sentinel

#include "evrtck_hls.hpp"
#include <string.h>

// ── Утилиты ──────────────────────────────────────────────────────────────────

static ap_uint<32> tile_beat_offset(
    ap_uint<16> tx, ap_uint<16> ty, int beat,
    ap_uint<32> w
) {
    #pragma HLS INLINE
    ap_uint<32> px_base = (ty * TILE_PX * w + tx * TILE_PX) * 4;
    return (px_base / AXI_BYTES) + beat;
}

// ── Стадия 1: FrameReader ────────────────────────────────────────────────────

static void stage_frame_reader(
    const ap_uint<AXI_W>* cur_ddr,
    const ap_uint<AXI_W>* prev_ddr,
    ap_uint<32>            tiles_x,
    ap_uint<32>            tiles_y,
    ap_uint<32>            width,
    hls::stream<FrameBeat>& out
) {
    #pragma HLS INLINE off

    ap_uint<16> tile_id = 0;
    for (ap_uint<16> ty = 0; ty < tiles_y; ty++) {
        for (ap_uint<16> tx = 0; tx < tiles_x; tx++) {
            for (int b = 0; b < BEATS_PER_TILE; b++) {
                #pragma HLS PIPELINE II=1
                ap_uint<32> off = tile_beat_offset(tx, ty, b, width);
                FrameBeat beat;
                beat.cur     = cur_ddr[off];
                beat.prev    = prev_ddr[off];
                beat.last    = (b == BEATS_PER_TILE - 1) ? 1 : 0;
                beat.tile_id = tile_id;
                out.write(beat);
            }
            tile_id++;
        }
    }
}

// ── Стадия 2: DirtyDetector ──────────────────────────────────────────────────

static void stage_dirty_detector(
    hls::stream<FrameBeat>&   in,
    hls::stream<FrameBeat>&   pass_through,
    hls::stream<DirtyResult>& dirty_out,
    int                       total_tiles
) {
    #pragma HLS INLINE off

    for (int t = 0; t < total_tiles; t++) {
        ap_uint<1>  dirty   = 0;
        ap_uint<16> tile_id = 0;

        for (int b = 0; b < BEATS_PER_TILE; b++) {
            #pragma HLS PIPELINE II=1
            FrameBeat beat = in.read();
            tile_id = beat.tile_id;
            if ((beat.cur ^ beat.prev) != 0) dirty = 1;
            pass_through.write(beat);
        }

        DirtyResult dr;
        dr.tile_id = tile_id;
        dr.dirty   = dirty;
        dirty_out.write(dr);
    }
}

// ── ScrollDetector helpers ────────────────────────────────────────────────────
//
// Этот блок — программная копия evrtck_scroll_csim.cpp.
// На FPGA (в составе stage_scroll_and_classify) синтезируется в:
//   - N_DY=32 параллельных компаратора (UNROLL по di)
//   - II=1 на строку тайла (PIPELINE по row)
//   - ARRAY_PARTITION complete dim=1 на LineBuf → 64 независимых порта BRAM

// Маппинг di → dy: di=0→+1, di=1→-1, di=2→+2, ... di=30→+16, di=31→-16
static ap_int<8> sc_dy_of(int di) {
    #pragma HLS INLINE
    int abs_dy = di / 2 + 1;
    return (ap_int<8>)(di % 2 == 0 ? abs_dy : -abs_dy);
}

// Загружает SCROLL_ROWS строк prev_frame для тайла в позиции (x0, y0) из DDR.
// Строка li = строка кадра (y0 - MAX_DY + li).
// При выходе за границы кадра строка обнуляется.
static void sc_load_linebuf(
    const ap_uint<AXI_W>* prev_ddr,
    int                    x0,
    int                    y0,
    int                    w,
    int                    h,
    ap_uint<8>             lb[SCROLL_ROWS][ROW_BYTES]
) {
    #pragma HLS INLINE
    #pragma HLS ARRAY_PARTITION variable=lb complete dim=1

    // ROW_BYTES=128 = 2 × AXI_BYTES(64) — два beat на строку
    static constexpr int BEATS_ROW = ROW_BYTES / AXI_BYTES;

    LOAD_ROWS:
    for (int li = 0; li < SCROLL_ROWS; li++) {
        int src_y = y0 - MAX_DY + li;

        if (src_y < 0 || src_y >= h) {
            ZERO_ROW: for (int b = 0; b < ROW_BYTES; b++) {
                #pragma HLS UNROLL
                lb[li][b] = 0;
            }
        } else {
            ap_uint<32> byte_off  = ((ap_uint<32>)src_y * w + x0) * 4;
            ap_uint<32> beat_base = byte_off / AXI_BYTES;

            LOAD_ROW: for (int beat = 0; beat < BEATS_ROW; beat++) {
                #pragma HLS PIPELINE II=1
                ap_uint<AXI_W> raw = prev_ddr[beat_base + beat];
                UNPACK_BEAT: for (int b = 0; b < AXI_BYTES; b++) {
                    #pragma HLS UNROLL
                    lb[li][beat * AXI_BYTES + b] = raw((b + 1) * 8 - 1, b * 8);
                }
            }
        }
    }
}

// Детектор скроллинга: N_DY=32 параллельных аппаратных компаратора.
// Возвращает true если cur_tile совпадает с lb сдвинутым на dy.
// Приоритет: минимальный |dy| (di=0 → |dy|=1 → наивысший).
static bool sc_detect(
    const ap_uint<8>  tile_cur[TILE_BYTES],
    const ap_uint<8>  lb[SCROLL_ROWS][ROW_BYTES],
    ap_int<8>&        out_dy
) {
    #pragma HLS INLINE

    bool match[N_DY];
    #pragma HLS ARRAY_PARTITION variable=match complete dim=1

    INIT_MATCH: for (int di = 0; di < N_DY; di++) {
        #pragma HLS UNROLL
        match[di] = true;
    }

    // ROW_LOOP: 32 такта @ II=1 (конвейер на FPGA)
    ROW_LOOP: for (int row = 0; row < TILE_PX; row++) {
        #pragma HLS PIPELINE II=1

        // Строка cur в локальных регистрах (ARRAY_PARTITION → мгновенное чтение)
        ap_uint<8> cur_row[ROW_BYTES];
        #pragma HLS ARRAY_PARTITION variable=cur_row complete dim=1
        LOAD_CUR: for (int b = 0; b < ROW_BYTES; b++) {
            #pragma HLS UNROLL
            cur_row[b] = tile_cur[row * ROW_BYTES + b];
        }

        // CMP_DY: N_DY=32 параллельных компаратора (UNROLL → аппаратные блоки)
        CMP_DY: for (int di = 0; di < N_DY; di++) {
            #pragma HLS UNROLL
            int dy_int = (di % 2 == 0) ? (di / 2 + 1) : -(di / 2 + 1);
            int pi     = row + MAX_DY + dy_int;

            bool differs = false;
            CMP_BYTES: for (int b = 0; b < ROW_BYTES; b++) {
                #pragma HLS UNROLL factor=16
                if (cur_row[b] != lb[pi][b]) differs = true;
            }
            if (differs) match[di] = false;
        }
    }

    // Приоритетный энкодер: обратный цикл, последняя запись побеждает → di=0 wins
    out_dy = 0;
    bool found = false;
    PRIO_ENC: for (int di = N_DY - 1; di >= 0; di--) {
        #pragma HLS UNROLL
        if (match[di]) {
            out_dy = sc_dy_of(di);
            found  = true;
        }
    }
    return found;
}

// ── Стадия 3: ScrollAndClassify ──────────────────────────────────────────────
//
// Заменяет stage_tile_classifier. Для каждого тайла:
//   1. Дочитывает beats из потока (cur-пиксели → tile_cur[])
//   2. Если тайл clean → TilePayload(MODE_CLEAN) в out
//   3. Иначе:
//      a. Загружает LineBuf из prev_ddr (64 строки ±MAX_DY вокруг тайла)
//      b. Пробует ScrollDetector (32 параллельных компаратора)
//      c. Пробует Solid check (cur-тайл одного цвета?)
//      d. Fallback: MODE_DELTA (XOR с prev из LineBuf[MAX_DY..])
//   Эмиттирует РОВНО ONE TilePayload на каждый из total_tiles тайлов.
//   Это позволяет downstream-стадиям циклиться по total_tiles без dirty_count.

static void stage_scroll_and_classify(
    hls::stream<FrameBeat>&   beats_in,
    hls::stream<DirtyResult>& dirty_in,
    const ap_uint<AXI_W>*     prev_ddr,   // AXI4-Master: gmem_scroll
    ap_uint<32>               width,
    ap_uint<32>               height,
    ap_uint<32>               tiles_x,
    hls::stream<TilePayload>& out,
    int                       total_tiles
) {
    #pragma HLS INLINE off
    #pragma HLS INTERFACE m_axi port=prev_ddr bundle=gmem_scroll \
        max_read_burst_length=16 latency=4

    ap_uint<8> tile_cur[TILE_BYTES];
    #pragma HLS ARRAY_PARTITION variable=tile_cur cyclic factor=64 dim=1

    ap_uint<8> lb[SCROLL_ROWS][ROW_BYTES];
    #pragma HLS ARRAY_PARTITION variable=lb complete dim=1

    for (int t = 0; t < total_tiles; t++) {
        DirtyResult dr = dirty_in.read();

        // Всегда дочитываем beats (иначе FIFO beats_pass переполнится)
        CONSUME: for (int b = 0; b < BEATS_PER_TILE; b++) {
            #pragma HLS PIPELINE II=1
            FrameBeat beat = beats_in.read();
            // Всегда распаковываем cur (для чистых тайлов — дешевле чем IF)
            UNPACK: for (int byte_i = 0; byte_i < AXI_BYTES; byte_i++) {
                #pragma HLS UNROLL
                tile_cur[b * AXI_BYTES + byte_i] =
                    beat.cur((byte_i + 1) * 8 - 1, byte_i * 8);
            }
        }

        TilePayload tp;
        tp.tile_id = dr.tile_id;
        tp.mode    = MODE_CLEAN;
        tp.color   = 0;
        tp.dy      = 0;

        if (dr.dirty) {
            // Координаты тайла в кадре
            int ty = (int)(dr.tile_id) / (int)tiles_x;
            int tx = (int)(dr.tile_id) % (int)tiles_x;
            int x0 = tx * TILE_PX;
            int y0 = ty * TILE_PX;

            // Загружаем LineBuf из DDR
            sc_load_linebuf(prev_ddr, x0, y0, (int)width, (int)height, lb);

            // ── 1. Scroll detection ───────────────────────────────────────────
            ap_int<8> found_dy = 0;
            bool scroll_ok = sc_detect(tile_cur, lb, found_dy);

            if (scroll_ok) {
                tp.mode = MODE_SCROLL;
                tp.dy   = found_dy;

            } else {
                // ── 2. Solid check (cur-тайл = один цвет?) ───────────────────
                ap_uint<32> first_px = 0;
                FIRST_PX: for (int j = 0; j < 4; j++) {
                    #pragma HLS UNROLL
                    first_px((j + 1) * 8 - 1, j * 8) = tile_cur[j];
                }
                bool solid = true;
                SOLID_CHK: for (int i = 4; i < TILE_BYTES; i += 4) {
                    #pragma HLS PIPELINE II=1
                    ap_uint<32> px = 0;
                    for (int j = 0; j < 4; j++) {
                        #pragma HLS UNROLL
                        px((j + 1) * 8 - 1, j * 8) = tile_cur[i + j];
                    }
                    if (px != first_px) solid = false;
                }

                if (solid) {
                    tp.mode  = MODE_SOLID;
                    tp.color = first_px;

                } else {
                    // ── 3. Delta: XOR с prev из LineBuf[MAX_DY..MAX_DY+TILE_PX) ─
                    tp.mode = MODE_DELTA;
                    DELTA_ROWS: for (int row = 0; row < TILE_PX; row++) {
                        #pragma HLS PIPELINE II=1
                        int li = MAX_DY + row;  // LineBuf[MAX_DY..] = prev tile
                        DELTA_BYTES: for (int b = 0; b < ROW_BYTES; b++) {
                            #pragma HLS UNROLL
                            tp.delta[row * ROW_BYTES + b] =
                                tile_cur[row * ROW_BYTES + b] ^ lb[li][b];
                        }
                    }
                }
            }
        }

        out.write(tp);
    }
}

// ── Стадия 4: ZrleEncoder ────────────────────────────────────────────────────
//
// Потоковый ZRLE-кодировщик. Читает total_tiles TilePayload.
//   MODE_CLEAN / MODE_SCROLL: перенаправляет напрямую в tile_order (bypass)
//   MODE_SOLID:               перенаправляет в tile_order (bypass)
//   MODE_DELTA:               кодирует ZRLE → zrle_out/zrle_sizes,
//                             в tile_order пишет header с enc_len в color-поле.
//
// ZRLE формат: 0x00 + count:u16 = zero-run; 0x01 + len:u16 + data = literal.

static void stage_zrle_encoder(
    hls::stream<TilePayload>&  in,
    hls::stream<TileHeader>&   tile_order,   // ВСЕ тайлы (bypass + delta header)
    hls::stream<ap_uint<8>>&   zrle_out,     // байты только для MODE_DELTA тайлов
    hls::stream<ap_uint<32>>&  zrle_sizes,
    int                        total_tiles
) {
    #pragma HLS INLINE off

    for (int t = 0; t < total_tiles; t++) {
        TilePayload tp = in.read();

        TileHeader hdr;
        hdr.tile_id = tp.tile_id;
        hdr.mode    = tp.mode;
        hdr.color   = tp.color;
        hdr.dy      = tp.dy;

        if (tp.mode != MODE_DELTA) {
            // CLEAN / SOLID / SCROLL — bypass, ZRLE не нужен
            tile_order.write(hdr);
            continue;
        }

        // ZRLE encoding
        ap_uint<16> zero_run = 0;
        ap_uint<8>  lit_buf[65535];
        ap_uint<16> lit_len  = 0;
        ap_uint<32> out_size = 0;

        auto flush_zeros = [&]() {
            if (zero_run == 0) return;
            zrle_out.write(0x00);
            zrle_out.write((ap_uint<8>)(zero_run >> 8));
            zrle_out.write((ap_uint<8>)(zero_run & 0xFF));
            out_size += 3;
            zero_run = 0;
        };
        auto flush_lits = [&]() {
            if (lit_len == 0) return;
            zrle_out.write(0x01);
            zrle_out.write((ap_uint<8>)(lit_len >> 8));
            zrle_out.write((ap_uint<8>)(lit_len & 0xFF));
            for (ap_uint<16> i = 0; i < lit_len; i++) {
                #pragma HLS PIPELINE II=1
                zrle_out.write(lit_buf[i]);
            }
            out_size += 3 + lit_len;
            lit_len = 0;
        };

        ZRLE_LOOP: for (int i = 0; i < TILE_BYTES; i++) {
            #pragma HLS PIPELINE II=1
            ap_uint<8> b = tp.delta[i];
            if (b == 0) {
                if (lit_len > 0) flush_lits();
                zero_run++;
                if (zero_run == 65535) flush_zeros();
            } else {
                if (zero_run >= 4) flush_zeros();
                lit_buf[lit_len++] = b;
                if (lit_len == 65535) flush_lits();
            }
        }
        flush_zeros();
        flush_lits();

        zrle_sizes.write(out_size);

        // Пишем header ПОСЛЕ кодирования: color = enc_len
        hdr.color = out_size;
        tile_order.write(hdr);
    }
}

// ── Стадия 5: PacketWriter ───────────────────────────────────────────────────
//
// Формирует EVRTCK v2 пакет: header + tile_map + tile_data.
// Читает total_tiles из tile_order, пропускает MODE_CLEAN.
// Для MODE_DELTA дополнительно читает из zrle_in.

static void stage_packet_writer(
    ap_uint<8>*               out_buf,
    ap_uint<32>&              out_size,
    hls::stream<TileHeader>&  tile_order,      // все тайлы (total_tiles)
    hls::stream<ap_uint<8>>&  zrle_in,         // байты только для DELTA тайлов
    hls::stream<ap_uint<32>>& zrle_sizes_in,
    ap_uint<32>               frame_id,
    ap_uint<32>               width,
    ap_uint<32>               height,
    int                       total_tiles
) {
    #pragma HLS INLINE off

    ap_uint<32> pos = 0;

    auto write_byte = [&](ap_uint<8> b)  { out_buf[pos++] = b; };
    auto write_u16  = [&](ap_uint<16> v) {
        write_byte(v & 0xFF); write_byte(v >> 8);
    };
    auto write_u32  = [&](ap_uint<32> v) {
        write_byte(v & 0xFF);
        write_byte((v >> 8)  & 0xFF);
        write_byte((v >> 16) & 0xFF);
        write_byte((v >> 24) & 0xFF);
    };

    // EVCK v2 header (16 + map_bytes)
    write_byte('E'); write_byte('V'); write_byte('C'); write_byte('K');
    write_byte(2);   // VERSION = 2 (добавлен MODE_SCROLL)
    write_byte(0);   // flags
    write_u32(frame_id);
    write_u32(width);
    write_u32(height);

    ap_uint<16> map_bytes = (total_tiles + 7) / 8;
    write_u16(map_bytes);

    // Резервируем место под tile_map (заполним позже)
    ap_uint<32> map_start = pos;
    RESERVE_MAP: for (int i = 0; i < map_bytes; i++) {
        #pragma HLS PIPELINE II=1
        write_byte(0);
    }

    // tile_map: BRAM-буфер, один бит на тайл
    ap_uint<8> tile_map[8192] = {};
    #pragma HLS ARRAY_PARTITION variable=tile_map cyclic factor=8

    // Проходим ВСЕ total_tiles; чистые тайлы пропускаем
    WRITE_TILES: for (int t = 0; t < total_tiles; t++) {
        TileHeader hdr = tile_order.read();

        if (hdr.mode == MODE_CLEAN) continue;

        // Отмечаем тайл в tile_map
        ap_uint<16> id = hdr.tile_id;
        tile_map[id / 8] |= (ap_uint<8>)(1 << (id % 8));

        write_byte(hdr.mode);

        if (hdr.mode == MODE_SOLID) {
            write_u32(hdr.color);                       // 5 байт итого

        } else if (hdr.mode == MODE_SCROLL) {
            write_byte((ap_uint<8>)hdr.dy);             // 2 байта итого

        } else {
            // MODE_DELTA: hdr.color = enc_len
            ap_uint<32> enc_len = zrle_sizes_in.read();
            write_u32(enc_len);                         // 5 байт header
            WRITE_ZRLE: for (ap_uint<32> i = 0; i < enc_len; i++) {
                #pragma HLS PIPELINE II=1
                write_byte(zrle_in.read());
            }
        }
    }

    // Записываем tile_map поверх зарезервированного места
    FLUSH_MAP: for (int i = 0; i < map_bytes; i++) {
        #pragma HLS PIPELINE II=1
        out_buf[map_start + i] = tile_map[i];
    }

    out_size = pos;
}

// ── Top-level: DATAFLOW orchestrator ─────────────────────────────────────────

void evrtck_encode_core(
    const ap_uint<AXI_W>* current_frame,
    const ap_uint<AXI_W>* prev_frame,
    const ap_uint<AXI_W>* prev_frame_scroll,
    ap_uint<8>*            out_buf,
    ap_uint<32>&           out_size,
    ap_uint<32>            frame_id,
    ap_uint<32>            width,
    ap_uint<32>            height
) {
    #pragma HLS INTERFACE m_axi     port=current_frame       bundle=gmem0 depth=8294400
    #pragma HLS INTERFACE m_axi     port=prev_frame          bundle=gmem1 depth=8294400
    #pragma HLS INTERFACE m_axi     port=prev_frame_scroll   bundle=gmem3 depth=8294400
    #pragma HLS INTERFACE m_axi     port=out_buf             bundle=gmem2 depth=8388608
    #pragma HLS INTERFACE s_axilite port=out_size
    #pragma HLS INTERFACE s_axilite port=frame_id
    #pragma HLS INTERFACE s_axilite port=width
    #pragma HLS INTERFACE s_axilite port=height
    #pragma HLS INTERFACE s_axilite port=return

    #pragma HLS DATAFLOW

    ap_uint<32> tiles_x    = (width  + TILE_PX - 1) / TILE_PX;
    ap_uint<32> tiles_y    = (height + TILE_PX - 1) / TILE_PX;
    int         total_tiles = (int)(tiles_x * tiles_y);

    // Потоки между стадиями
    hls::stream<FrameBeat>   beats_raw("beats_raw");
    hls::stream<FrameBeat>   beats_pass("beats_pass");
    hls::stream<DirtyResult> dirty_flags("dirty_flags");
    hls::stream<TilePayload> classified("classified");
    hls::stream<TileHeader>  tile_order("tile_order");
    hls::stream<ap_uint<8>>  zrle_bytes("zrle_bytes");
    hls::stream<ap_uint<32>> zrle_sizes("zrle_sizes");

    // beats_raw / beats_pass: глубина = 2 тайла (ping-pong)
    #pragma HLS STREAM variable=beats_raw   depth=128
    #pragma HLS STREAM variable=beats_pass  depth=128
    // dirty_flags: по одному элементу на тайл; глубина = 16 тайлов lookahead
    #pragma HLS STREAM variable=dirty_flags depth=16
    // classified: TilePayload ~4KB; глубина=2 достаточна при II=1 везде
    #pragma HLS STREAM variable=classified  depth=2
    // tile_order: TileHeader = 8 байт; глубина=16 (буфер для solid/scroll burst)
    #pragma HLS STREAM variable=tile_order  depth=16
    #pragma HLS STREAM variable=zrle_bytes  depth=4096
    #pragma HLS STREAM variable=zrle_sizes  depth=16

    // ── 5-стадийный конвейер ──────────────────────────────────────────────────

    stage_frame_reader(
        current_frame, prev_frame,
        tiles_x, tiles_y, width,
        beats_raw
    );

    stage_dirty_detector(
        beats_raw, beats_pass,
        dirty_flags,
        total_tiles
    );

    stage_scroll_and_classify(
        beats_pass, dirty_flags,
        prev_frame_scroll,
        width, height, tiles_x,
        classified,
        total_tiles
    );

    stage_zrle_encoder(
        classified,
        tile_order, zrle_bytes, zrle_sizes,
        total_tiles
    );

    stage_packet_writer(
        out_buf, out_size,
        tile_order,
        zrle_bytes, zrle_sizes,
        frame_id, width, height,
        total_tiles
    );
}
