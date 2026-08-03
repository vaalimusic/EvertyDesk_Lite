// Intel oneVPL / Media SDK — аппаратное кодирование напрямую через драйвер.
//
// Зачем этот шим существует
// -------------------------
// Media Foundation не отдаёт HEVC, пока не установлен пакет «HEVC Video
// Extensions» из Microsoft Store. На целевой машине (Intel UHD 610) живо
// подтверждено: MFTEnumEx не находит HEVC-энкодера ни по одному GUID, хотя
// H264(hw) находит. Требовать установку пакета от пользователя нельзя,
// поэтому нужен путь напрямую в драйвер — тот же приём, что уже применён для
// NVIDIA в nvenc_shim.cpp.
//
// D3D12 Video Encode как вендор-нейтральную альтернативу проверили первой и
// отвергли по факту: на том же железе `CheckFeatureSupport` отвечает, что
// аппаратных кодеков нет (Gen9.5 слишком старый). Проба oneVPL на той же
// машине отвечает: libvpl.dll, API 1.35, аппаратная сессия через D3D11 — то
// есть путь открыт, и это поколение API 1.x, а не 2.x.
//
// Почему C++, а не чистый Rust
// ----------------------------
// mfxVideoParam/mfxFrameSurface1 — большие структуры с объединениями и
// reserved-полями. Объявлять их вручную на стороне Rust значит гадать про
// раскладку памяти на железе, которое я не могу отладить. Здесь раскладку
// гарантирует компилятор по официальным заголовкам (vendor/onevpl, MIT).
//
// Почему символы резолвятся динамически
// -------------------------------------
// Линковаться с libmfx.lib нельзя: на машинах без Intel-графики библиотеки
// нет, и процесс не запустился бы вовсе. Поэтому диспетчер грузится через
// LoadLibrary, а отсутствие функций — это штатный отказ, по которому
// вызывающий код откатывается на существующий каскад.

#include <windows.h>

#include <cstdio>
#include <cstring>
#include <cstdint>
#include <vector>

#include "mfxvideo.h"

namespace {

constexpr int kOk = 0;
constexpr int kErr = -1;

// Коды кодеков, согласованные с NvencCodec на стороне Rust.
constexpr int kCodecH264 = 0;
constexpr int kCodecH265 = 1;

void set_err(char *err, size_t err_len, const char *msg) {
    if (!err || err_len == 0) {
        return;
    }
    std::snprintf(err, err_len, "%s", msg);
}

void set_errf(char *err, size_t err_len, const char *fmt, int value) {
    if (!err || err_len == 0) {
        return;
    }
    std::snprintf(err, err_len, fmt, value);
}

// ── Динамическая загрузка диспетчера ────────────────────────────────────────

struct MfxApi {
    HMODULE module = nullptr;

    mfxStatus (MFX_CDECL *Init)(mfxIMPL, mfxVersion *, mfxSession *) = nullptr;
    mfxStatus (MFX_CDECL *Close)(mfxSession) = nullptr;
    mfxStatus (MFX_CDECL *EncodeQuery)(mfxSession, mfxVideoParam *, mfxVideoParam *) = nullptr;
    mfxStatus (MFX_CDECL *EncodeQueryIOSurf)(mfxSession, mfxVideoParam *, mfxFrameAllocRequest *) = nullptr;
    mfxStatus (MFX_CDECL *EncodeInit)(mfxSession, mfxVideoParam *) = nullptr;
    mfxStatus (MFX_CDECL *EncodeClose)(mfxSession) = nullptr;
    mfxStatus (MFX_CDECL *EncodeGetVideoParam)(mfxSession, mfxVideoParam *) = nullptr;
    mfxStatus (MFX_CDECL *EncodeFrameAsync)(mfxSession, mfxEncodeCtrl *, mfxFrameSurface1 *,
                                            mfxBitstream *, mfxSyncPoint *) = nullptr;
    mfxStatus (MFX_CDECL *SyncOperation)(mfxSession, mfxSyncPoint, mfxU32) = nullptr;

    bool ok() const {
        return module && Init && Close && EncodeQuery && EncodeInit && EncodeClose &&
               EncodeFrameAsync && SyncOperation && EncodeGetVideoParam;
    }
};

template <typename Fn>
void resolve(HMODULE module, const char *name, Fn &slot) {
    slot = reinterpret_cast<Fn>(GetProcAddress(module, name));
}

// Диспетчер загружается один раз на процесс. Рантайм Intel держит внутреннее
// состояние, и повторные LoadLibrary/FreeLibrary в одном процессе — известный
// источник проблем, поэтому модуль намеренно не выгружается.
const MfxApi *mfx_api() {
    static MfxApi api;
    static bool initialised = false;
    if (initialised) {
        return api.ok() ? &api : nullptr;
    }
    initialised = true;

    // Порядок тот же, что в пробе на стороне Rust (src/onevpl.rs): сперва
    // диспетчер oneVPL 2.x, который умеет и легаси-рантайм, затем легаси-
    // диспетчер, затем сам рантайм напрямую.
    static const char *kCandidates[] = {"libvpl.dll", "libmfx.dll", "libmfxhw64.dll"};
    for (const char *name : kCandidates) {
        HMODULE module = LoadLibraryA(name);
        if (!module) {
            continue;
        }
        MfxApi candidate;
        candidate.module = module;
        resolve(module, "MFXInit", candidate.Init);
        resolve(module, "MFXClose", candidate.Close);
        resolve(module, "MFXVideoENCODE_Query", candidate.EncodeQuery);
        resolve(module, "MFXVideoENCODE_QueryIOSurf", candidate.EncodeQueryIOSurf);
        resolve(module, "MFXVideoENCODE_Init", candidate.EncodeInit);
        resolve(module, "MFXVideoENCODE_Close", candidate.EncodeClose);
        resolve(module, "MFXVideoENCODE_GetVideoParam", candidate.EncodeGetVideoParam);
        resolve(module, "MFXVideoENCODE_EncodeFrameAsync", candidate.EncodeFrameAsync);
        resolve(module, "MFXVideoCORE_SyncOperation", candidate.SyncOperation);
        if (candidate.ok()) {
            api = candidate;
            return &api;
        }
        FreeLibrary(module);
    }
    return nullptr;
}

// ── BGRA → NV12 ─────────────────────────────────────────────────────────────
//
// ENCODE ждёт NV12: плоскость яркости, затем чередующиеся U/V в половинном
// разрешении. Конвертация делается здесь, а не на стороне Rust, чтобы не
// гонять лишнюю копию кадра через FFI.
//
// Коэффициенты — BT.601 limited range, те же, что использует остальной
// конвейер; менять их здесь нельзя, иначе картинка «поплывёт» по цвету
// относительно других энкодеров.
void bgra_to_nv12(const uint8_t *bgra, uint32_t width, uint32_t height, uint32_t pitch,
                  uint8_t *y_plane, uint8_t *uv_plane) {
    for (uint32_t row = 0; row < height; ++row) {
        const uint8_t *src = bgra + static_cast<size_t>(row) * width * 4;
        uint8_t *dst_y = y_plane + static_cast<size_t>(row) * pitch;
        for (uint32_t col = 0; col < width; ++col) {
            const int b = src[col * 4 + 0];
            const int g = src[col * 4 + 1];
            const int r = src[col * 4 + 2];
            dst_y[col] = static_cast<uint8_t>(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16);
        }
    }

    // Цветность считается по одному пикселю на блок 2×2, а не усреднением.
    // Это сознательный размен: усреднение дало бы чуть лучший результат на
    // резких границах, но стоило бы вчетверо больше чтений на каждом кадре,
    // а мы кодируем в реальном времени.
    const uint32_t chroma_h = height / 2;
    const uint32_t chroma_w = width / 2;
    for (uint32_t row = 0; row < chroma_h; ++row) {
        const uint8_t *src = bgra + static_cast<size_t>(row) * 2 * width * 4;
        uint8_t *dst_uv = uv_plane + static_cast<size_t>(row) * pitch;
        for (uint32_t col = 0; col < chroma_w; ++col) {
            const int b = src[col * 8 + 0];
            const int g = src[col * 8 + 1];
            const int r = src[col * 8 + 2];
            dst_uv[col * 2 + 0] =
                static_cast<uint8_t>(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128);
            dst_uv[col * 2 + 1] =
                static_cast<uint8_t>(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128);
        }
    }
}

struct EncoderContext {
    const MfxApi *api = nullptr;
    mfxSession session = nullptr;
    bool encode_open = false;

    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t aligned_width = 0;
    uint32_t aligned_height = 0;

    // Одна системная поверхность: наш конвейер синхронный (кадр за кадром),
    // поэтому очередь поверхностей не нужна.
    std::vector<uint8_t> surface_memory;
    mfxFrameSurface1 surface{};

    std::vector<uint8_t> bitstream_memory;
    mfxBitstream bitstream{};

    // Отданный наружу буфер: живёт до следующего вызова encode, как и в
    // nvenc_shim.
    std::vector<uint8_t> out_bytes;

    ~EncoderContext() {
        if (api && session) {
            if (encode_open) {
                api->EncodeClose(session);
            }
            api->Close(session);
        }
    }
};

mfxU32 codec_id(int codec) {
    return codec == kCodecH265 ? MFX_CODEC_HEVC : MFX_CODEC_AVC;
}

void fill_video_param(mfxVideoParam &param, int codec, uint32_t width, uint32_t height,
                      uint32_t fps, uint32_t bitrate) {
    std::memset(&param, 0, sizeof(param));
    param.mfx.CodecId = codec_id(codec);
    // Целевое качество/скорость. SPEED, а не BALANCED: это стриминг с упором
    // на задержку, лишние миллисекунды на кадр дороже небольшой потери
    // качества при фиксированном битрейте.
    param.mfx.TargetUsage = MFX_TARGETUSAGE_BEST_SPEED;
    param.mfx.RateControlMethod = MFX_RATECONTROL_VBR;
    param.mfx.TargetKbps = static_cast<mfxU16>(bitrate / 1000);
    param.mfx.MaxKbps = static_cast<mfxU16>((bitrate / 1000) * 3 / 2);
    param.mfx.FrameInfo.FrameRateExtN = fps;
    param.mfx.FrameInfo.FrameRateExtD = 1;
    param.mfx.FrameInfo.FourCC = MFX_FOURCC_NV12;
    param.mfx.FrameInfo.ChromaFormat = MFX_CHROMAFORMAT_YUV420;
    param.mfx.FrameInfo.PicStruct = MFX_PICSTRUCT_PROGRESSIVE;
    param.mfx.FrameInfo.CropX = 0;
    param.mfx.FrameInfo.CropY = 0;
    param.mfx.FrameInfo.CropW = static_cast<mfxU16>(width);
    param.mfx.FrameInfo.CropH = static_cast<mfxU16>(height);
    // Выравнивание обязательное: 16 по ширине, 16 по высоте для прогрессивной
    // развёртки. Без него Init отвергает параметры.
    param.mfx.FrameInfo.Width = static_cast<mfxU16>((width + 15) & ~15u);
    param.mfx.FrameInfo.Height = static_cast<mfxU16>((height + 15) & ~15u);

    // Ключевые кадры ставит вызывающая сторона через mfxEncodeCtrl, поэтому
    // собственный периодический IDR рантайму не нужен — иначе он слал бы свои
    // IDR поверх наших, удваивая и без того дорогой трафик.
    param.mfx.GopPicSize = 0;
    // Без B-кадров: они добавляют задержку переупорядочивания, что для
    // интерактивной сессии неприемлемо.
    param.mfx.GopRefDist = 1;
    param.mfx.NumRefFrame = 1;
    param.mfx.IdrInterval = 0;
    param.AsyncDepth = 1; // синхронно: кадр отдали — кадр забрали
    param.IOPattern = MFX_IOPATTERN_IN_SYSTEM_MEMORY;
}

} // namespace

extern "C" {

// Какие кодеки реально поддерживает аппаратный энкодер этой машины.
// Возвращает битовую маску: бит 0 — H264, бит 1 — H265.
int everty_onevpl_supported_codecs(uint32_t *mask, char *err, size_t err_len) {
    if (mask) {
        *mask = 0;
    }
    const MfxApi *api = mfx_api();
    if (!api) {
        set_err(err, err_len, "диспетчер oneVPL/MSDK не найден");
        return kErr;
    }

    mfxVersion version{};
    version.Major = 1;
    version.Minor = 0;
    mfxSession session = nullptr;
    mfxStatus rc = api->Init(MFX_IMPL_HARDWARE_ANY, &version, &session);
    if (rc != MFX_ERR_NONE) {
        rc = api->Init(MFX_IMPL_HARDWARE_ANY | MFX_IMPL_VIA_D3D11, &version, &session);
    }
    if (rc != MFX_ERR_NONE || !session) {
        set_errf(err, err_len, "MFXInit не создал аппаратную сессию (%d)", static_cast<int>(rc));
        return kErr;
    }

    uint32_t found = 0;
    for (int codec : {kCodecH264, kCodecH265}) {
        mfxVideoParam in{};
        mfxVideoParam out{};
        // Разрешение для запроса берём заведомо валидное и типовое: Query
        // проверяет поддержку кодека, а не конкретный размер кадра.
        fill_video_param(in, codec, 1280, 720, 30, 4000000);
        out.mfx.CodecId = in.mfx.CodecId;
        const mfxStatus q = api->EncodeQuery(session, &in, &out);
        // MFX_WRN_* (положительные) означают «поддерживается, но параметры
        // подправлены» — для вопроса «умеет ли кодек» это тоже «да».
        if (q >= MFX_ERR_NONE) {
            found |= (codec == kCodecH265) ? 0x2u : 0x1u;
        }
    }

    api->Close(session);
    if (mask) {
        *mask = found;
    }
    if (found == 0) {
        set_err(err, err_len, "аппаратная сессия есть, но ни H264, ни H265 не поддержаны");
        return kErr;
    }
    return kOk;
}

int everty_onevpl_create(int codec, uint32_t width, uint32_t height, uint32_t fps,
                         uint32_t bitrate, void **out_ctx, char *err, size_t err_len) {
    if (!out_ctx) {
        set_err(err, err_len, "out_ctx = null");
        return kErr;
    }
    *out_ctx = nullptr;

    const MfxApi *api = mfx_api();
    if (!api) {
        set_err(err, err_len, "диспетчер oneVPL/MSDK не найден");
        return kErr;
    }

    EncoderContext *ctx = new EncoderContext();
    ctx->api = api;
    ctx->width = width;
    ctx->height = height;

    mfxVersion version{};
    version.Major = 1;
    version.Minor = 0;
    mfxStatus rc = api->Init(MFX_IMPL_HARDWARE_ANY, &version, &ctx->session);
    if (rc != MFX_ERR_NONE) {
        rc = api->Init(MFX_IMPL_HARDWARE_ANY | MFX_IMPL_VIA_D3D11, &version, &ctx->session);
    }
    if (rc != MFX_ERR_NONE || !ctx->session) {
        set_errf(err, err_len, "MFXInit не создал аппаратную сессию (%d)", static_cast<int>(rc));
        delete ctx;
        return kErr;
    }

    mfxVideoParam param{};
    fill_video_param(param, codec, width, height, fps, bitrate);

    // Query нормализует параметры под возможности железа. Его вердикт
    // используем как есть: спорить с рантаймом о том, что он умеет, смысла
    // нет, а Init на неподправленных параметрах часто отказывает.
    mfxVideoParam corrected = param;
    rc = api->EncodeQuery(ctx->session, &param, &corrected);
    if (rc < MFX_ERR_NONE) {
        set_errf(err, err_len, "MFXVideoENCODE_Query отверг параметры (%d)", static_cast<int>(rc));
        delete ctx;
        return kErr;
    }

    rc = api->EncodeInit(ctx->session, &corrected);
    if (rc < MFX_ERR_NONE) {
        set_errf(err, err_len, "MFXVideoENCODE_Init (%d)", static_cast<int>(rc));
        delete ctx;
        return kErr;
    }
    ctx->encode_open = true;

    ctx->aligned_width = corrected.mfx.FrameInfo.Width;
    ctx->aligned_height = corrected.mfx.FrameInfo.Height;

    // NV12: Y во всю площадь + UV в половину высоты.
    const size_t luma = static_cast<size_t>(ctx->aligned_width) * ctx->aligned_height;
    ctx->surface_memory.assign(luma + luma / 2, 0);
    std::memset(&ctx->surface, 0, sizeof(ctx->surface));
    ctx->surface.Info = corrected.mfx.FrameInfo;
    ctx->surface.Data.Y = ctx->surface_memory.data();
    ctx->surface.Data.UV = ctx->surface_memory.data() + luma;
    ctx->surface.Data.Pitch = static_cast<mfxU16>(ctx->aligned_width);

    // Буфер битстрима: с запасом на ключевой кадр, который на порядок толще
    // обычного. Нехватка проявляется как MFX_ERR_NOT_ENOUGH_BUFFER, и тогда
    // кадр теряется целиком — дешевле переплатить памятью.
    const size_t bitstream_size = luma * 2 + (1u << 20);
    ctx->bitstream_memory.assign(bitstream_size, 0);
    std::memset(&ctx->bitstream, 0, sizeof(ctx->bitstream));
    ctx->bitstream.Data = ctx->bitstream_memory.data();
    ctx->bitstream.MaxLength = static_cast<mfxU32>(bitstream_size);

    *out_ctx = ctx;
    return kOk;
}

int everty_onevpl_encode(void *raw_ctx, const uint8_t *bgra, size_t bgra_len, int force_key,
                         const uint8_t **out_data, size_t *out_len, int *out_key, char *err,
                         size_t err_len) {
    if (!raw_ctx || !bgra || !out_data || !out_len || !out_key) {
        set_err(err, err_len, "нулевой аргумент");
        return kErr;
    }
    EncoderContext *ctx = static_cast<EncoderContext *>(raw_ctx);
    *out_data = nullptr;
    *out_len = 0;
    *out_key = 0;

    const size_t expected = static_cast<size_t>(ctx->width) * ctx->height * 4;
    if (bgra_len < expected) {
        set_err(err, err_len, "кадр короче ожидаемого BGRA");
        return kErr;
    }

    // Дождаться, пока энкодер отпустит поверхность. MSDK помечает занятую
    // поверхность счётчиком Data.Locked, и писать в неё до его обнуления
    // нельзя — рантайм ещё читает оттуда пиксели. Первая версия шима писала
    // в поверхность безусловно, из-за чего энкодер получал наполовину
    // перезаписанный кадр: битстрим выходил куцым и, судя по всему, местами
    // некорректным.
    for (int spin = 0; ctx->surface.Data.Locked != 0 && spin < 1000; ++spin) {
        Sleep(1);
    }
    if (ctx->surface.Data.Locked != 0) {
        set_err(err, err_len, "поверхность не освободилась (Data.Locked)");
        return kErr;
    }

    bgra_to_nv12(bgra, ctx->width, ctx->height, ctx->surface.Data.Pitch, ctx->surface.Data.Y,
                 ctx->surface.Data.UV);

    mfxEncodeCtrl ctrl{};
    mfxEncodeCtrl *ctrl_ptr = nullptr;
    if (force_key) {
        ctrl.FrameType = MFX_FRAMETYPE_I | MFX_FRAMETYPE_IDR | MFX_FRAMETYPE_REF;
        ctrl_ptr = &ctrl;
    }

    ctx->bitstream.DataOffset = 0;
    ctx->bitstream.DataLength = 0;

    mfxSyncPoint sync = nullptr;
    mfxStatus rc = MFX_ERR_NONE;
    // MFX_WRN_DEVICE_BUSY — штатный ответ «железо занято, повтори». Ограничиваем
    // число повторов: бесконечный цикл здесь означал бы намертво вставший
    // конвейер захвата.
    for (int attempt = 0; attempt < 100; ++attempt) {
        rc = ctx->api->EncodeFrameAsync(ctx->session, ctrl_ptr, &ctx->surface, &ctx->bitstream,
                                        &sync);
        if (rc == MFX_WRN_DEVICE_BUSY) {
            Sleep(1);
            continue;
        }
        break;
    }

    if (rc == MFX_ERR_MORE_DATA) {
        // Рантайм принял кадр, но пакета пока нет. Не ошибка.
        return kOk;
    }
    if (rc < MFX_ERR_NONE || !sync) {
        set_errf(err, err_len, "EncodeFrameAsync (%d)", static_cast<int>(rc));
        return kErr;
    }

    rc = ctx->api->SyncOperation(ctx->session, sync, 10000);
    if (rc < MFX_ERR_NONE) {
        set_errf(err, err_len, "SyncOperation (%d)", static_cast<int>(rc));
        return kErr;
    }

    const uint8_t *data = ctx->bitstream.Data + ctx->bitstream.DataOffset;
    const size_t len = ctx->bitstream.DataLength;
    ctx->out_bytes.assign(data, data + len);
    ctx->bitstream.DataOffset = 0;
    ctx->bitstream.DataLength = 0;

    *out_data = ctx->out_bytes.data();
    *out_len = ctx->out_bytes.size();
    *out_key = (ctx->bitstream.FrameType & (MFX_FRAMETYPE_I | MFX_FRAMETYPE_IDR)) ? 1 : 0;
    return kOk;
}

void everty_onevpl_destroy(void *raw_ctx) {
    delete static_cast<EncoderContext *>(raw_ctx);
}

} // extern "C"
