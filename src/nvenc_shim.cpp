#ifdef _WIN32

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

#include <d3d11.h>
#include <dxgi.h>
#include <nvEncodeAPI.h>

namespace {

constexpr int kOk = 0;
constexpr int kNoPacket = 1;
constexpr int kErr = -1;
constexpr uint32_t kCodecMaskH264 = 1u << 0;
constexpr uint32_t kCodecMaskH265 = 1u << 1;
constexpr uint32_t kBufferCount = 3;
constexpr uint32_t kInfiniteGop = 0xffff'ffffu;

using NvEncodeApiCreateInstance =
    NVENCSTATUS(NVENCAPI *)(NV_ENCODE_API_FUNCTION_LIST *);
using NvEncodeApiGetMaxSupportedVersion = NVENCSTATUS(NVENCAPI *)(uint32_t *);

void set_error(char *dst, size_t dst_len, const std::string &message) {
    if (!dst || dst_len == 0) {
        return;
    }
    const size_t len = std::min(dst_len - 1, message.size());
    std::memcpy(dst, message.data(), len);
    dst[len] = '\0';
}

std::string status_message(const char *call, NVENCSTATUS status) {
    return std::string(call) + " failed with NVENC status " +
           std::to_string(static_cast<int>(status));
}

bool guid_equal(const GUID &a, const GUID &b) {
    return std::memcmp(&a, &b, sizeof(GUID)) == 0;
}

const GUID *codec_guid(int codec) {
    switch (codec) {
    case 1:
        return &NV_ENC_CODEC_H264_GUID;
    case 2:
        return &NV_ENC_CODEC_HEVC_GUID;
    default:
        return nullptr;
    }
}

struct Api {
    HMODULE library = nullptr;
    NV_ENCODE_API_FUNCTION_LIST fns = {};

    ~Api() {
        if (library) {
            FreeLibrary(library);
        }
    }

    bool load(std::string &err) {
        library = LoadLibraryA("nvEncodeAPI64.dll");
        if (!library) {
            library = LoadLibraryA("nvEncodeAPI.dll");
        }
        if (!library) {
            err = "NvEncodeAPI library not found";
            return false;
        }

        auto get_max = reinterpret_cast<NvEncodeApiGetMaxSupportedVersion>(
            GetProcAddress(library, "NvEncodeAPIGetMaxSupportedVersion"));
        auto create = reinterpret_cast<NvEncodeApiCreateInstance>(
            GetProcAddress(library, "NvEncodeAPICreateInstance"));
        if (!get_max || !create) {
            err = "NvEncodeAPI entry points not found";
            return false;
        }

        uint32_t driver_version = 0;
        NVENCSTATUS status = get_max(&driver_version);
        if (status != NV_ENC_SUCCESS) {
            err = status_message("NvEncodeAPIGetMaxSupportedVersion", status);
            return false;
        }
        const uint32_t current_version =
            (NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION;
        if (current_version > driver_version) {
            err = "NVIDIA driver is older than the bundled NVENC API header";
            return false;
        }

        std::memset(&fns, 0, sizeof(fns));
        fns.version = NV_ENCODE_API_FUNCTION_LIST_VER;
        status = create(&fns);
        if (status != NV_ENC_SUCCESS) {
            err = status_message("NvEncodeAPICreateInstance", status);
            return false;
        }
        if (!fns.nvEncOpenEncodeSessionEx || !fns.nvEncGetEncodeGUIDCount ||
            !fns.nvEncGetEncodeGUIDs || !fns.nvEncGetInputFormatCount ||
            !fns.nvEncGetInputFormats || !fns.nvEncGetEncodePresetConfigEx ||
            !fns.nvEncInitializeEncoder || !fns.nvEncRegisterResource ||
            !fns.nvEncUnregisterResource || !fns.nvEncMapInputResource ||
            !fns.nvEncUnmapInputResource || !fns.nvEncCreateBitstreamBuffer ||
            !fns.nvEncDestroyBitstreamBuffer || !fns.nvEncEncodePicture ||
            !fns.nvEncLockBitstream || !fns.nvEncUnlockBitstream ||
            !fns.nvEncDestroyEncoder) {
            err = "NvEncodeAPI function list is incomplete";
            return false;
        }
        return true;
    }
};

struct DxDevice {
    IDXGIFactory1 *factory = nullptr;
    IDXGIAdapter1 *adapter = nullptr;
    ID3D11Device *device = nullptr;
    ID3D11DeviceContext *context = nullptr;
    std::string adapter_name = "NVIDIA GPU";

    ~DxDevice() { release(); }

    void release() {
        if (context) {
            context->Release();
            context = nullptr;
        }
        if (device) {
            device->Release();
            device = nullptr;
        }
        if (adapter) {
            adapter->Release();
            adapter = nullptr;
        }
        if (factory) {
            factory->Release();
            factory = nullptr;
        }
    }

    bool create(std::string &err) {
        HRESULT hr = CreateDXGIFactory1(__uuidof(IDXGIFactory1),
                                        reinterpret_cast<void **>(&factory));
        if (FAILED(hr)) {
            err = "CreateDXGIFactory1 failed";
            return false;
        }

        for (UINT i = 0;; ++i) {
            IDXGIAdapter1 *candidate = nullptr;
            hr = factory->EnumAdapters1(i, &candidate);
            if (hr == DXGI_ERROR_NOT_FOUND) {
                break;
            }
            if (FAILED(hr) || !candidate) {
                continue;
            }

            DXGI_ADAPTER_DESC1 desc = {};
            candidate->GetDesc1(&desc);
            const bool software =
                (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) != 0;
            if (!software && desc.VendorId == 0x10DE) {
                adapter = candidate;
                adapter_name = wide_to_utf8(desc.Description);
                break;
            }
            candidate->Release();
        }

        if (!adapter) {
            err = "NVIDIA DXGI adapter not found";
            return false;
        }

        D3D_FEATURE_LEVEL requested[] = {
            D3D_FEATURE_LEVEL_12_1, D3D_FEATURE_LEVEL_12_0,
            D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0,
        };
        D3D_FEATURE_LEVEL selected = D3D_FEATURE_LEVEL_11_0;
        hr = D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, nullptr,
                               D3D11_CREATE_DEVICE_BGRA_SUPPORT, requested,
                               ARRAYSIZE(requested), D3D11_SDK_VERSION,
                               &device, &selected, &context);
        if (FAILED(hr) || !device || !context) {
            err = "D3D11CreateDevice failed for NVIDIA adapter";
            return false;
        }
        return true;
    }

    static std::string wide_to_utf8(const wchar_t *text) {
        if (!text || !text[0]) {
            return "NVIDIA GPU";
        }
        int needed =
            WideCharToMultiByte(CP_UTF8, 0, text, -1, nullptr, 0, nullptr, nullptr);
        if (needed <= 1) {
            return "NVIDIA GPU";
        }
        std::string out(static_cast<size_t>(needed), '\0');
        WideCharToMultiByte(CP_UTF8, 0, text, -1, out.data(), needed, nullptr,
                            nullptr);
        out.resize(static_cast<size_t>(needed - 1));
        return out;
    }
};

struct Surface {
    ID3D11Texture2D *texture = nullptr;
    NV_ENC_REGISTERED_PTR registered = nullptr;
    NV_ENC_OUTPUT_PTR bitstream = nullptr;
};

struct NvencContext {
    Api api;
    DxDevice dx;
    void *encoder = nullptr;
    int codec = 0;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;
    uint32_t frame_idx = 0;
    std::vector<Surface> surfaces;
    std::vector<uint8_t> packet;

    ~NvencContext() { destroy(); }

    void destroy() {
        if (!encoder) {
            return;
        }

        NV_ENC_PIC_PARAMS eos = {};
        eos.version = NV_ENC_PIC_PARAMS_VER;
        eos.encodePicFlags = NV_ENC_PIC_FLAG_EOS;
        api.fns.nvEncEncodePicture(encoder, &eos);

        for (auto &surface : surfaces) {
            if (surface.registered) {
                api.fns.nvEncUnregisterResource(encoder, surface.registered);
                surface.registered = nullptr;
            }
            if (surface.bitstream) {
                api.fns.nvEncDestroyBitstreamBuffer(encoder, surface.bitstream);
                surface.bitstream = nullptr;
            }
            if (surface.texture) {
                surface.texture->Release();
                surface.texture = nullptr;
            }
        }
        surfaces.clear();
        api.fns.nvEncDestroyEncoder(encoder);
        encoder = nullptr;
    }
};

bool open_session(NvencContext &ctx, std::string &err) {
    NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS params = {};
    params.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
    params.device = ctx.dx.device;
    params.deviceType = NV_ENC_DEVICE_TYPE_DIRECTX;
    params.apiVersion = NVENCAPI_VERSION;

    NVENCSTATUS status =
        ctx.api.fns.nvEncOpenEncodeSessionEx(&params, &ctx.encoder);
    if (status != NV_ENC_SUCCESS || !ctx.encoder) {
        err = status_message("nvEncOpenEncodeSessionEx", status);
        return false;
    }
    return true;
}

bool codec_supported(NvencContext &ctx, const GUID &codec) {
    uint32_t guid_count = 0;
    if (ctx.api.fns.nvEncGetEncodeGUIDCount(ctx.encoder, &guid_count) !=
            NV_ENC_SUCCESS ||
        guid_count == 0) {
        return false;
    }

    std::vector<GUID> guids(guid_count);
    uint32_t written = 0;
    if (ctx.api.fns.nvEncGetEncodeGUIDs(ctx.encoder, guids.data(), guid_count,
                                        &written) != NV_ENC_SUCCESS) {
        return false;
    }
    if (std::none_of(guids.begin(), guids.begin() + written,
                     [&](const GUID &candidate) {
                         return guid_equal(candidate, codec);
                     })) {
        return false;
    }

    uint32_t format_count = 0;
    if (ctx.api.fns.nvEncGetInputFormatCount(ctx.encoder, codec,
                                             &format_count) != NV_ENC_SUCCESS ||
        format_count == 0) {
        return false;
    }
    std::vector<NV_ENC_BUFFER_FORMAT> formats(format_count);
    written = 0;
    if (ctx.api.fns.nvEncGetInputFormats(ctx.encoder, codec, formats.data(),
                                         format_count, &written) !=
        NV_ENC_SUCCESS) {
        return false;
    }
    return std::any_of(formats.begin(), formats.begin() + written,
                       [](NV_ENC_BUFFER_FORMAT fmt) {
                           return fmt == NV_ENC_BUFFER_FORMAT_ARGB;
                       });
}

bool init_encoder(NvencContext &ctx, int codec, uint32_t width, uint32_t height,
                  uint32_t fps, uint32_t bitrate, std::string &err) {
    const GUID *guid = codec_guid(codec);
    if (!guid) {
        err = "unsupported NVENC codec";
        return false;
    }
    if (!codec_supported(ctx, *guid)) {
        err = "requested codec/input format is not supported by this NVENC device";
        return false;
    }

    NV_ENC_PRESET_CONFIG preset = {};
    preset.version = NV_ENC_PRESET_CONFIG_VER;
    preset.presetCfg.version = NV_ENC_CONFIG_VER;
    NVENCSTATUS status = ctx.api.fns.nvEncGetEncodePresetConfigEx(
        ctx.encoder, *guid, NV_ENC_PRESET_P1_GUID,
        NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY, &preset);
    if (status != NV_ENC_SUCCESS) {
        err = status_message("nvEncGetEncodePresetConfigEx", status);
        return false;
    }

    NV_ENC_CONFIG config = preset.presetCfg;
    config.version = NV_ENC_CONFIG_VER;
    config.profileGUID = NV_ENC_CODEC_PROFILE_AUTOSELECT_GUID;
    config.gopLength = kInfiniteGop;
    config.frameIntervalP = 1;
    config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
    config.rcParams.multiPass = NV_ENC_MULTI_PASS_DISABLED;
    config.rcParams.averageBitRate = bitrate;
    config.rcParams.maxBitRate = bitrate;
    config.rcParams.vbvBufferSize = std::max<uint32_t>(bitrate / fps, 64 * 1024);
    config.rcParams.vbvInitialDelay = config.rcParams.vbvBufferSize;

    if (codec == 1) {
        config.encodeCodecConfig.h264Config.outputAUD = 1;
        config.encodeCodecConfig.h264Config.repeatSPSPPS = 1;
        config.encodeCodecConfig.h264Config.idrPeriod = kInfiniteGop;
    } else if (codec == 2) {
        config.encodeCodecConfig.hevcConfig.outputAUD = 1;
        config.encodeCodecConfig.hevcConfig.repeatSPSPPS = 1;
        config.encodeCodecConfig.hevcConfig.idrPeriod = kInfiniteGop;
    }

    NV_ENC_INITIALIZE_PARAMS init = {};
    init.version = NV_ENC_INITIALIZE_PARAMS_VER;
    init.encodeGUID = *guid;
    init.presetGUID = NV_ENC_PRESET_P1_GUID;
    init.encodeWidth = width;
    init.encodeHeight = height;
    init.darWidth = width;
    init.darHeight = height;
    init.frameRateNum = fps;
    init.frameRateDen = 1;
    init.enableEncodeAsync = 0;
    init.enablePTD = 1;
    init.maxEncodeWidth = width;
    init.maxEncodeHeight = height;
    init.tuningInfo = NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
    init.encodeConfig = &config;

    status = ctx.api.fns.nvEncInitializeEncoder(ctx.encoder, &init);
    if (status != NV_ENC_SUCCESS) {
        err = status_message("nvEncInitializeEncoder", status);
        return false;
    }
    return true;
}

bool create_surfaces(NvencContext &ctx, std::string &err) {
    ctx.surfaces.resize(kBufferCount);
    for (auto &surface : ctx.surfaces) {
        D3D11_TEXTURE2D_DESC desc = {};
        desc.Width = ctx.width;
        desc.Height = ctx.height;
        desc.MipLevels = 1;
        desc.ArraySize = 1;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 1;
        desc.Usage = D3D11_USAGE_DEFAULT;
        desc.BindFlags = D3D11_BIND_RENDER_TARGET;

        HRESULT hr = ctx.dx.device->CreateTexture2D(&desc, nullptr,
                                                    &surface.texture);
        if (FAILED(hr) || !surface.texture) {
            err = "CreateTexture2D failed for NVENC input surface";
            return false;
        }

        NV_ENC_REGISTER_RESOURCE reg = {};
        reg.version = NV_ENC_REGISTER_RESOURCE_VER;
        reg.resourceType = NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
        reg.width = ctx.width;
        reg.height = ctx.height;
        reg.pitch = 0;
        reg.subResourceIndex = 0;
        reg.resourceToRegister = surface.texture;
        reg.bufferFormat = NV_ENC_BUFFER_FORMAT_ARGB;
        reg.bufferUsage = NV_ENC_INPUT_IMAGE;
        NVENCSTATUS status =
            ctx.api.fns.nvEncRegisterResource(ctx.encoder, &reg);
        if (status != NV_ENC_SUCCESS) {
            err = status_message("nvEncRegisterResource", status);
            return false;
        }
        surface.registered = reg.registeredResource;

        NV_ENC_CREATE_BITSTREAM_BUFFER bitstream = {};
        bitstream.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
        status = ctx.api.fns.nvEncCreateBitstreamBuffer(ctx.encoder, &bitstream);
        if (status != NV_ENC_SUCCESS) {
            err = status_message("nvEncCreateBitstreamBuffer", status);
            return false;
        }
        surface.bitstream = bitstream.bitstreamBuffer;
    }
    return true;
}

bool create_context(int codec, uint32_t width, uint32_t height, uint32_t fps,
                    uint32_t bitrate, NvencContext **out, std::string &err) {
    if (!out) {
        err = "output context pointer is null";
        return false;
    }
    *out = nullptr;
    if (width == 0 || height == 0 || fps == 0) {
        err = "invalid encoder dimensions or FPS";
        return false;
    }

    NvencContext *ctx = new NvencContext();
    ctx->codec = codec;
    ctx->width = width;
    ctx->height = height;
    ctx->fps = fps;

    if (!ctx->api.load(err) || !ctx->dx.create(err) || !open_session(*ctx, err) ||
        !init_encoder(*ctx, codec, width, height, fps, bitrate, err) ||
        !create_surfaces(*ctx, err)) {
        delete ctx;
        return false;
    }
    *out = ctx;
    return true;
}

bool supported_codecs(uint32_t *mask, std::string *name, std::string &err) {
    if (!mask) {
        err = "codec mask pointer is null";
        return false;
    }
    *mask = 0;
    NvencContext ctx;
    if (!ctx.api.load(err) || !ctx.dx.create(err) || !open_session(ctx, err)) {
        return false;
    }
    if (codec_supported(ctx, NV_ENC_CODEC_H264_GUID)) {
        *mask |= kCodecMaskH264;
    }
    if (codec_supported(ctx, NV_ENC_CODEC_HEVC_GUID)) {
        *mask |= kCodecMaskH265;
    }
    if (name) {
        *name = ctx.dx.adapter_name;
    }
    return true;
}

bool lock_packet(NvencContext &ctx, Surface &surface, bool force_key,
                 const uint8_t **data, size_t *len, int *key,
                 std::string &err) {
    NV_ENC_LOCK_BITSTREAM lock = {};
    lock.version = NV_ENC_LOCK_BITSTREAM_VER;
    lock.outputBitstream = surface.bitstream;
    lock.doNotWait = 0;

    NVENCSTATUS status = ctx.api.fns.nvEncLockBitstream(ctx.encoder, &lock);
    if (status != NV_ENC_SUCCESS) {
        err = status_message("nvEncLockBitstream", status);
        return false;
    }

    const auto *bytes = static_cast<const uint8_t *>(lock.bitstreamBufferPtr);
    if (bytes && lock.bitstreamSizeInBytes > 0) {
        ctx.packet.assign(bytes, bytes + lock.bitstreamSizeInBytes);
    } else {
        ctx.packet.clear();
    }
    if (data) {
        *data = ctx.packet.data();
    }
    if (len) {
        *len = ctx.packet.size();
    }
    if (key) {
        *key = force_key || lock.pictureType == NV_ENC_PIC_TYPE_IDR ||
               lock.pictureType == NV_ENC_PIC_TYPE_I;
    }

    ctx.api.fns.nvEncUnlockBitstream(ctx.encoder, surface.bitstream);
    return true;
}

int encode_frame(NvencContext &ctx, const uint8_t *bgra, size_t bgra_len,
                 bool force_key, const uint8_t **data, size_t *len, int *key,
                 std::string &err) {
    const size_t expected = static_cast<size_t>(ctx.width) * ctx.height * 4;
    if (!bgra || bgra_len < expected) {
        err = "BGRA frame is smaller than encoder dimensions";
        return kErr;
    }

    Surface &surface = ctx.surfaces[ctx.frame_idx % ctx.surfaces.size()];
    ctx.dx.context->UpdateSubresource(surface.texture, 0, nullptr, bgra,
                                      ctx.width * 4, 0);

    NV_ENC_MAP_INPUT_RESOURCE mapped = {};
    mapped.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
    mapped.registeredResource = surface.registered;
    NVENCSTATUS status =
        ctx.api.fns.nvEncMapInputResource(ctx.encoder, &mapped);
    if (status != NV_ENC_SUCCESS) {
        err = status_message("nvEncMapInputResource", status);
        return kErr;
    }

    NV_ENC_PIC_PARAMS pic = {};
    pic.version = NV_ENC_PIC_PARAMS_VER;
    pic.inputWidth = ctx.width;
    pic.inputHeight = ctx.height;
    pic.inputPitch = ctx.width;
    pic.frameIdx = ctx.frame_idx;
    pic.inputTimeStamp = ctx.frame_idx;
    pic.inputDuration = 1;
    pic.inputBuffer = mapped.mappedResource;
    pic.outputBitstream = surface.bitstream;
    pic.bufferFmt = mapped.mappedBufferFmt;
    pic.pictureStruct = NV_ENC_PIC_STRUCT_FRAME;
    if (force_key) {
        pic.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR |
                             NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
    }

    status = ctx.api.fns.nvEncEncodePicture(ctx.encoder, &pic);
    if (status == NV_ENC_ERR_NEED_MORE_INPUT) {
        ctx.api.fns.nvEncUnmapInputResource(ctx.encoder, mapped.mappedResource);
        ++ctx.frame_idx;
        return kNoPacket;
    }
    if (status != NV_ENC_SUCCESS) {
        ctx.api.fns.nvEncUnmapInputResource(ctx.encoder, mapped.mappedResource);
        err = status_message("nvEncEncodePicture", status);
        return kErr;
    }

    const bool ok =
        lock_packet(ctx, surface, force_key, data, len, key, err);
    ctx.api.fns.nvEncUnmapInputResource(ctx.encoder, mapped.mappedResource);
    ++ctx.frame_idx;
    return ok ? kOk : kErr;
}

} // namespace

extern "C" {

int everty_nvenc_supported_codecs(uint32_t *mask, char *name, size_t name_len,
                                  char *err, size_t err_len) {
    std::string error;
    std::string adapter_name;
    if (!supported_codecs(mask, &adapter_name, error)) {
        set_error(err, err_len, error);
        return kErr;
    }
    set_error(name, name_len, adapter_name);
    return kOk;
}

int everty_nvenc_create(int codec, uint32_t width, uint32_t height, uint32_t fps,
                        uint32_t bitrate, void **out, char *err,
                        size_t err_len) {
    std::string error;
    NvencContext *ctx = nullptr;
    if (!create_context(codec, width, height, fps, bitrate, &ctx, error)) {
        set_error(err, err_len, error);
        return kErr;
    }
    *out = ctx;
    return kOk;
}

int everty_nvenc_encode(void *ctx_ptr, const uint8_t *bgra, size_t bgra_len,
                        int force_key, const uint8_t **data, size_t *len,
                        int *key, char *err, size_t err_len) {
    if (!ctx_ptr) {
        set_error(err, err_len, "NVENC context is null");
        return kErr;
    }
    if (data) {
        *data = nullptr;
    }
    if (len) {
        *len = 0;
    }
    if (key) {
        *key = 0;
    }

    std::string error;
    int status = encode_frame(*static_cast<NvencContext *>(ctx_ptr), bgra,
                              bgra_len, force_key != 0, data, len, key, error);
    if (status == kErr) {
        set_error(err, err_len, error);
    }
    return status;
}

void everty_nvenc_destroy(void *ctx_ptr) {
    delete static_cast<NvencContext *>(ctx_ptr);
}

} // extern "C"

#endif // _WIN32
