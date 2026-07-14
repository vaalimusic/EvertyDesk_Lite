//  EVRTCK HLS — реализация конвейерных стадий
//
//  Компилируется: vitis_hls -f run_hls.tcl
//  C-симуляция:   vitis_hls -f run_csim.tcl
//
//  Каждая функция — отдельная стадия DATAFLOW-конвейера.
//  Стадии обмениваются данными через hls::stream<> (аппаратные FIFO).
//  Все стадии работают одновременно: пока ZrleEncoder сжимает тайл N,
//  DirtyDetector уже анализирует тайл N+1.

#include "evrtck_hls.hpp"
#include <string.h>

// ── Утилиты ──────────────────────────────────────────────────────────────────

// Смещение в DDR для AXI-beat (beat_idx) тайла (tx, ty) кадра шириной w пикселей.
static ap_uint<32> tile_beat_offset(
    ap_uint<16> tx, ap_uint<16> ty, int beat,
    ap_uint<32> w
) {
    #pragma HLS INLINE
    // Первый пиксель тайла в кадре: (ty*TILE_PX*w + tx*TILE_PX) * 4 байта
    // Делим на AXI_BYTES, чтобы получить beat-индекс в массиве ap_uint<AXI_W>
    ap_uint<32> px_base = (ty * TILE_PX * w + tx * TILE_PX) * 4;
    return (px_base / AXI_BYTES) + beat;
    // Примечание: для тайлов, чья строка не кратна AXI_BYTES, нужен scatter-gather.
    // Для w кратного TILE_PX (типично 1920, 2560) это совпадает точно.
}

// ── Стадия 1: FrameReader ────────────────────────────────────────────────────
// Читает тайлы из DDR попарно (cur + prev) и льёт beats в поток.

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
// XOR каждого beat; если хоть один бит ненулевой — тайл dirty.

static void stage_dirty_detector(
    hls::stream<FrameBeat>&   in,
    hls::stream<FrameBeat>&   pass_through,  // прокидываем байты дальше
    hls::stream<DirtyResult>& dirty_out,
    int                       total_tiles
) {
    #pragma HLS INLINE off

    for (int t = 0; t < total_tiles; t++) {
        ap_uint<1>  dirty    = 0;
        ap_uint<16> tile_id  = 0;

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

// ── Стадия 3: TileClassifier ─────────────────────────────────────────────────
// Для грязных тайлов: проверяет solid-color, иначе строит XOR-delta.
// Чистые тайлы пропускаются (не пишутся в выход).

static void stage_tile_classifier(
    hls::stream<FrameBeat>&   beats_in,
    hls::stream<DirtyResult>& dirty_in,
    hls::stream<TilePayload>& out,
    int                       total_tiles
) {
    #pragma HLS INLINE off

    ap_uint<8> tile_buf[TILE_BYTES];
    #pragma HLS ARRAY_PARTITION variable=tile_buf cyclic factor=64

    for (int t = 0; t < total_tiles; t++) {
        // Получаем dirty-флаг для этого тайла
        DirtyResult dr = dirty_in.read();

        // Всегда дочитываем beats (иначе FIFO переполнится)
        for (int b = 0; b < BEATS_PER_TILE; b++) {
            #pragma HLS PIPELINE II=1
            FrameBeat beat = beats_in.read();
            if (dr.dirty) {
                // Разворачиваем AXI-beat в байты и XOR с prev
                ap_uint<AXI_W> delta_beat = beat.cur ^ beat.prev;
                for (int byte_i = 0; byte_i < AXI_BYTES; byte_i++) {
                    #pragma HLS UNROLL
                    tile_buf[b * AXI_BYTES + byte_i] =
                        delta_beat((byte_i + 1) * 8 - 1, byte_i * 8);
                }
            }
        }

        if (!dr.dirty) continue;

        // Проверка solid-color (все 4-байтных пикселя одинаковы)
        ap_uint<32> first_px = 0;
        for (int i = 0; i < 4; i++) {
            #pragma HLS UNROLL
            first_px((i + 1) * 8 - 1, i * 8) = tile_buf[i];
        }
        bool solid = true;
        for (int i = 4; i < TILE_BYTES; i += 4) {
            #pragma HLS PIPELINE II=1
            ap_uint<32> px = 0;
            for (int j = 0; j < 4; j++) {
                #pragma HLS UNROLL
                px((j + 1) * 8 - 1, j * 8) = tile_buf[i + j];
            }
            if (px != first_px) { solid = false; }
        }

        TilePayload tp;
        tp.tile_id = dr.tile_id;
        if (solid) {
            tp.mode  = MODE_SOLID;
            tp.color = first_px;
        } else {
            tp.mode = MODE_DELTA;
            memcpy(tp.delta, tile_buf, TILE_BYTES);
        }
        out.write(tp);
    }
}

// ── Стадия 4: ZrleEncoder ────────────────────────────────────────────────────
// Потоковый ZRLE: II=1 на байт. Детерминированная задержка без буфера обратного
// давления — важно: выход в аппаратный FIFO, не в память.

static void stage_zrle_encoder(
    hls::stream<TilePayload>& in,
    hls::stream<TilePayload>& pass_solid,  // solid тайлы идут напрямую
    hls::stream<ap_uint<8>>&  zrle_out,    // сжатые байты delta тайлов
    hls::stream<ap_uint<32>>& zrle_sizes,  // размер каждого сжатого тайла
    int                       dirty_tiles
) {
    #pragma HLS INLINE off

    for (int t = 0; t < dirty_tiles; t++) {
        TilePayload tp = in.read();

        if (tp.mode == MODE_SOLID) {
            pass_solid.write(tp);
            continue;
        }

        // ZRLE encoding: потоковый, II=1
        ap_uint<16> zero_run = 0;
        ap_uint<8>  lit_buf[65535];  // в реальном ядре — BRAM FIFO
        ap_uint<16> lit_len  = 0;
        ap_uint<32> out_size = 0;

        auto flush_zeros = [&]() {
            if (zero_run == 0) return;
            zrle_out.write(0x00);
            zrle_out.write(zero_run >> 8);
            zrle_out.write(zero_run & 0xFF);
            out_size += 3;
            zero_run = 0;
        };
        auto flush_lits = [&]() {
            if (lit_len == 0) return;
            zrle_out.write(0x01);
            zrle_out.write(lit_len >> 8);
            zrle_out.write(lit_len & 0xFF);
            for (ap_uint<16> i = 0; i < lit_len; i++) {
                #pragma HLS PIPELINE II=1
                zrle_out.write(lit_buf[i]);
            }
            out_size += 3 + lit_len;
            lit_len = 0;
        };

        for (int i = 0; i < TILE_BYTES; i++) {
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
    }
}

// ── Стадия 5: PacketWriter ───────────────────────────────────────────────────
// Собирает EVRTCK заголовок + tile_map + данные тайлов и пишет в DDR.

static void stage_packet_writer(
    ap_uint<8>*               out_buf,
    ap_uint<32>&              out_size,
    hls::stream<DirtyResult>& dirty_map_in,   // snapshot dirty-map
    hls::stream<TilePayload>& solids_in,
    hls::stream<ap_uint<8>>&  zrle_in,
    hls::stream<ap_uint<32>>& zrle_sizes_in,
    ap_uint<32>               frame_id,
    ap_uint<32>               width,
    ap_uint<32>               height,
    ap_uint<32>               tiles_x,
    ap_uint<32>               tiles_y,
    int                       total_tiles,
    int                       dirty_count
) {
    #pragma HLS INLINE off

    ap_uint<32> pos = 0;

    auto write_byte = [&](ap_uint<8> b) {
        #pragma HLS INLINE
        out_buf[pos++] = b;
    };
    auto write_u16 = [&](ap_uint<16> v) {
        #pragma HLS INLINE
        write_byte(v & 0xFF);
        write_byte(v >> 8);
    };
    auto write_u32 = [&](ap_uint<32> v) {
        #pragma HLS INLINE
        write_byte(v & 0xFF);
        write_byte((v >> 8) & 0xFF);
        write_byte((v >> 16) & 0xFF);
        write_byte((v >> 24) & 0xFF);
    };

    // EVCK header
    write_byte('E'); write_byte('V'); write_byte('C'); write_byte('K');
    write_byte(1);    // VERSION
    write_byte(0);    // flags
    write_u32(frame_id);
    write_u32(width);
    write_u32(height);

    ap_uint<16> map_bytes = (total_tiles + 7) / 8;
    write_u16(map_bytes);

    // Tile dirty-map: читаем из потока (DirtyDetector уже вычислил)
    ap_uint<32> map_start = pos;
    for (int i = 0; i < map_bytes; i++) {
        #pragma HLS PIPELINE II=1
        write_byte(0);  // заполним ниже
    }

    // Пишем данные тайлов и одновременно строим tile_map
    ap_uint<8> tile_map[8192] = {};  // max 65536 тайлов / 8 — хранится в BRAM
    #pragma HLS ARRAY_PARTITION variable=tile_map cyclic factor=8

    for (int t = 0; t < dirty_count; t++) {
        TilePayload tp;

        // Определяем: solid или delta
        // (в реальной реализации — отдельный мультиплексор-арбитр)
        if (!solids_in.empty()) {
            tp = solids_in.read();
        }

        ap_uint<16> id = tp.tile_id;
        tile_map[id / 8] |= (1 << (id % 8));

        write_byte(tp.mode);
        if (tp.mode == MODE_SOLID) {
            write_u32(tp.color);
        } else {
            ap_uint<32> enc_len = zrle_sizes_in.read();
            write_u32(enc_len);
            for (ap_uint<32> i = 0; i < enc_len; i++) {
                #pragma HLS PIPELINE II=1
                write_byte(zrle_in.read());
            }
        }
    }

    // Записываем tile_map поверх зарезервированного места
    for (int i = 0; i < map_bytes; i++) {
        #pragma HLS PIPELINE II=1
        out_buf[map_start + i] = tile_map[i];
    }

    out_size = pos;
}

// ── Top-level: DATAFLOW orchestrator ─────────────────────────────────────────

void evrtck_encode_core(
    const ap_uint<AXI_W>* current_frame,
    const ap_uint<AXI_W>* prev_frame,
    ap_uint<8>*            out_buf,
    ap_uint<32>&           out_size,
    ap_uint<32>            frame_id,
    ap_uint<32>            width,
    ap_uint<32>            height
) {
    #pragma HLS INTERFACE m_axi     port=current_frame bundle=gmem0 depth=8294400
    #pragma HLS INTERFACE m_axi     port=prev_frame    bundle=gmem1 depth=8294400
    #pragma HLS INTERFACE m_axi     port=out_buf       bundle=gmem2 depth=8388608
    #pragma HLS INTERFACE s_axilite port=out_size
    #pragma HLS INTERFACE s_axilite port=frame_id
    #pragma HLS INTERFACE s_axilite port=width
    #pragma HLS INTERFACE s_axilite port=height
    #pragma HLS INTERFACE s_axilite port=return

    #pragma HLS DATAFLOW

    ap_uint<32> tiles_x = (width  + TILE_PX - 1) / TILE_PX;
    ap_uint<32> tiles_y = (height + TILE_PX - 1) / TILE_PX;
    int total_tiles = tiles_x * tiles_y;

    // Потоки между стадиями
    hls::stream<FrameBeat>   beats_raw("beats_raw");
    hls::stream<FrameBeat>   beats_pass("beats_pass");
    hls::stream<DirtyResult> dirty_flags("dirty_flags");
    hls::stream<DirtyResult> dirty_map_snap("dirty_map_snap");
    hls::stream<TilePayload> classified("classified");
    hls::stream<TilePayload> solid_tiles("solid_tiles");
    hls::stream<ap_uint<8>>  zrle_bytes("zrle_bytes");
    hls::stream<ap_uint<32>> zrle_sizes("zrle_sizes");

    #pragma HLS STREAM variable=beats_raw    depth=128
    #pragma HLS STREAM variable=beats_pass   depth=128
    #pragma HLS STREAM variable=dirty_flags  depth=2040
    #pragma HLS STREAM variable=classified   depth=16
    #pragma HLS STREAM variable=solid_tiles  depth=16
    #pragma HLS STREAM variable=zrle_bytes   depth=4096
    #pragma HLS STREAM variable=zrle_sizes   depth=16

    // Счётчик dirty тайлов нужен packet_writer — считаем отдельно
    // (в реальном ядре: dirty_count передаётся через s_axilite или отдельный stream)
    int dirty_count = 0; // TODO: вычислить в dirty_detector, передать через stream

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

    stage_tile_classifier(
        beats_pass, dirty_flags,
        classified,
        total_tiles
    );

    stage_zrle_encoder(
        classified,
        solid_tiles, zrle_bytes, zrle_sizes,
        dirty_count
    );

    stage_packet_writer(
        out_buf, out_size,
        dirty_map_snap,
        solid_tiles, zrle_bytes, zrle_sizes,
        frame_id, width, height,
        tiles_x, tiles_y,
        total_tiles, dirty_count
    );
}
