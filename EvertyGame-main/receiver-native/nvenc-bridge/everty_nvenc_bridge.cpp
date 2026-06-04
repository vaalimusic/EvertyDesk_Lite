#include <d3d11.h>
#include <d3d11_4.h>
#include <dxgi.h>
#include <wrl/client.h>

#include <cstdint>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <algorithm>
#include <unordered_map>
#include <utility>
#include <vector>

#include "NvEncoder/NvEncoderD3D11.h"
#include "nvEncodeAPI.h"

using Microsoft::WRL::ComPtr;

namespace
{
std::mutex g_lastErrorMutex;
std::string g_lastError;

void SetLastErrorText(const std::string& message)
{
    std::lock_guard<std::mutex> lock(g_lastErrorMutex);
    g_lastError = message;
}

void ClearLastErrorText()
{
    SetLastErrorText(std::string());
}

const char* GetLastErrorText()
{
    std::lock_guard<std::mutex> lock(g_lastErrorMutex);
    return g_lastError.c_str();
}

void ThrowIfFailed(HRESULT hr, const char* message)
{
    if (FAILED(hr))
    {
        throw std::runtime_error(message);
    }
}

struct EncodedPacket
{
    std::vector<uint8_t> Payload;
    int64_t TimestampHns = 0;
    bool KeyFrame = false;
};

class NativeNvencSession
{
public:
    NativeNvencSession(ID3D11Device* device, int width, int height, int codec, int bitrateBps, int fps, int gopLength, bool gamePreset)
        : device_(device),
          targetWidth_(width),
          targetHeight_(height),
          codec_(codec),
          bitrateBps_(bitrateBps),
          fps_(fps),
          gopLength_(gopLength),
          gamePreset_(gamePreset)
    {
        if (!device_)
        {
            throw std::runtime_error("D3D11 device is null.");
        }

        if (targetWidth_ <= 0 || targetHeight_ <= 0 || fps_ <= 0 || bitrateBps_ <= 0 || gopLength_ <= 0)
        {
            throw std::runtime_error("Invalid native NVENC session parameters.");
        }

        ThrowIfFailed(device_.As(&videoDevice_), "Failed to query ID3D11VideoDevice.");
        device_->GetImmediateContext(&deviceContext_);
        ThrowIfFailed(deviceContext_.As(&videoContext_), "Failed to query ID3D11VideoContext.");
        ComPtr<ID3D11Multithread> multithread;
        if (SUCCEEDED(deviceContext_.As(&multithread)) && multithread)
        {
            multithread->SetMultithreadProtected(TRUE);
        }

        pendingPackets_.reserve(16);
        CreateEncoder();
    }

    ~NativeNvencSession()
    {
        try
        {
            DestroyEncoder();
        }
        catch (...)
        {
        }
    }

    void EncodeFrame(ID3D11Texture2D* texture, int64_t timestampHns, bool forceIdr)
    {
        if (!texture)
        {
            throw std::runtime_error("Input texture is null.");
        }

        auto* encoderInput = encoder_->GetNextInputFrame();
        auto* destinationTexture = reinterpret_cast<ID3D11Texture2D*>(encoderInput->inputPtr);
        if (!destinationTexture)
        {
            throw std::runtime_error("Encoder input texture is null.");
        }

        BlitScaledFrame(texture, destinationTexture);

        NV_ENC_PIC_PARAMS picParams = { NV_ENC_PIC_PARAMS_VER };
        picParams.inputTimeStamp = static_cast<uint64_t>(timestampHns);
        if (forceIdr)
        {
            picParams.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
        }

        std::vector<NvEncOutputFrame> packets;
        encoder_->EncodeFrame(packets, &picParams);
        for (auto& packet : packets)
        {
            pendingPackets_.push_back(EncodedPacket
            {
                std::move(packet.frame),
                timestampHns,
                forceIdr
            });
        }
    }

    void DrainPackets(void (*callback)(const uint8_t*, int, int64_t, int, void*), void* userData)
    {
        if (!callback)
        {
            throw std::runtime_error("Drain callback is null.");
        }

        for (auto& packet : pendingPackets_)
        {
            callback(
                packet.Payload.data(),
                static_cast<int>(packet.Payload.size()),
                packet.TimestampHns,
                packet.KeyFrame ? 1 : 0,
                userData);
        }

        pendingPackets_.clear();
    }

    void Reconfigure(int bitrateBps, int fps, int gopLength)
    {
        if (bitrateBps <= 0 || fps <= 0 || gopLength <= 0)
        {
            throw std::runtime_error("Invalid native NVENC reconfigure parameters.");
        }

        bitrateBps_ = bitrateBps;
        fps_ = fps;
        gopLength_ = gopLength;

        ApplyLowLatencySettings();

        NV_ENC_RECONFIGURE_PARAMS reconfigureParams = { NV_ENC_RECONFIGURE_PARAMS_VER };
        reconfigureParams.reInitEncodeParams = initializeParams_;
        reconfigureParams.reInitEncodeParams.encodeConfig = &encodeConfig_;
        reconfigureParams.forceIDR = 1;
        reconfigureParams.resetEncoder = 1;

        if (!encoder_->Reconfigure(&reconfigureParams))
        {
            throw std::runtime_error("NVENC reconfigure failed.");
        }
    }

private:
    void CreateEncoder()
    {
        encoder_ = std::make_unique<NvEncoderD3D11>(
            device_.Get(),
            static_cast<uint32_t>(targetWidth_),
            static_cast<uint32_t>(targetHeight_),
            NV_ENC_BUFFER_FORMAT_ARGB,
            0,
            false,
            false);

        initializeParams_ = { NV_ENC_INITIALIZE_PARAMS_VER };
        encodeConfig_ = { NV_ENC_CONFIG_VER };
        initializeParams_.encodeConfig = &encodeConfig_;

        auto codecGuid = codec_ == 0 ? NV_ENC_CODEC_H264_GUID : NV_ENC_CODEC_HEVC_GUID;
        auto presetGuid = gamePreset_ ? NV_ENC_PRESET_P1_GUID : NV_ENC_PRESET_P3_GUID;
        auto tuningInfo = gamePreset_ ? NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY : NV_ENC_TUNING_INFO_LOW_LATENCY;

        encoder_->CreateDefaultEncoderParams(&initializeParams_, codecGuid, presetGuid, tuningInfo);
        initializeParams_.frameRateNum = fps_;
        initializeParams_.frameRateDen = 1;
        initializeParams_.enablePTD = 1;
        initializeParams_.encodeWidth = static_cast<uint32_t>(targetWidth_);
        initializeParams_.encodeHeight = static_cast<uint32_t>(targetHeight_);
        initializeParams_.darWidth = static_cast<uint32_t>(targetWidth_);
        initializeParams_.darHeight = static_cast<uint32_t>(targetHeight_);

        ApplyLowLatencySettings();
        encoder_->CreateEncoder(&initializeParams_);
    }

    void DestroyEncoder()
    {
        if (encoder_)
        {
            encoder_->DestroyEncoder();
            encoder_.reset();
        }
    }

    void ApplyLowLatencySettings()
    {
        initializeParams_.frameRateNum = fps_;
        initializeParams_.frameRateDen = 1;
        initializeParams_.encodeWidth = static_cast<uint32_t>(targetWidth_);
        initializeParams_.encodeHeight = static_cast<uint32_t>(targetHeight_);
        initializeParams_.darWidth = static_cast<uint32_t>(targetWidth_);
        initializeParams_.darHeight = static_cast<uint32_t>(targetHeight_);

        encodeConfig_.gopLength = static_cast<uint32_t>(gopLength_);
        encodeConfig_.frameIntervalP = 1;
        encodeConfig_.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        encodeConfig_.rcParams.averageBitRate = static_cast<uint32_t>(bitrateBps_);
        encodeConfig_.rcParams.maxBitRate = static_cast<uint32_t>(bitrateBps_);
        encodeConfig_.rcParams.vbvBufferSize = static_cast<uint32_t>(std::max(bitrateBps_ / std::max(1, fps_) * 2, 64 * 1024));
        encodeConfig_.rcParams.vbvInitialDelay = encodeConfig_.rcParams.vbvBufferSize;
        encodeConfig_.rcParams.enableLookahead = 0;
        encodeConfig_.rcParams.lookaheadDepth = 0;
        encodeConfig_.rcParams.zeroReorderDelay = 1;
        encodeConfig_.rcParams.enableNonRefP = gamePreset_ ? 1 : 0;
        encodeConfig_.rcParams.strictGOPTarget = 1;
        encodeConfig_.rcParams.multiPass = NV_ENC_MULTI_PASS_DISABLED;

        if (codec_ == 0)
        {
            encodeConfig_.encodeCodecConfig.h264Config.outputAUD = 1;
            encodeConfig_.encodeCodecConfig.h264Config.repeatSPSPPS = 1;
            encodeConfig_.encodeCodecConfig.h264Config.disableSPSPPS = 0;
            encodeConfig_.encodeCodecConfig.h264Config.idrPeriod = static_cast<uint32_t>(gopLength_);
        }
        else
        {
            encodeConfig_.encodeCodecConfig.hevcConfig.outputAUD = 1;
            encodeConfig_.encodeCodecConfig.hevcConfig.repeatSPSPPS = 1;
            encodeConfig_.encodeCodecConfig.hevcConfig.disableSPSPPS = 0;
            encodeConfig_.encodeCodecConfig.hevcConfig.idrPeriod = static_cast<uint32_t>(gopLength_);
        }
    }

    void EnsureVideoProcessor(UINT sourceWidth, UINT sourceHeight)
    {
        if (sourceWidth == sourceWidth_ &&
            sourceHeight == sourceHeight_ &&
            sourceBgraTexture_ &&
            videoProcessorInputView_ &&
            videoProcessorEnumerator_ &&
            videoProcessor_)
        {
            return;
        }

        outputViews_.clear();
        videoProcessorInputView_.Reset();
        sourceBgraTexture_.Reset();
        videoProcessor_.Reset();
        videoProcessorEnumerator_.Reset();

        D3D11_VIDEO_PROCESSOR_CONTENT_DESC contentDesc = {};
        contentDesc.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
        contentDesc.InputFrameRate.Numerator = static_cast<UINT>(std::max(1, fps_));
        contentDesc.InputFrameRate.Denominator = 1;
        contentDesc.InputWidth = sourceWidth;
        contentDesc.InputHeight = sourceHeight;
        contentDesc.OutputFrameRate.Numerator = static_cast<UINT>(std::max(1, fps_));
        contentDesc.OutputFrameRate.Denominator = 1;
        contentDesc.OutputWidth = static_cast<UINT>(targetWidth_);
        contentDesc.OutputHeight = static_cast<UINT>(targetHeight_);
        contentDesc.Usage = D3D11_VIDEO_USAGE_OPTIMAL_SPEED;

        ThrowIfFailed(
            videoDevice_->CreateVideoProcessorEnumerator(&contentDesc, &videoProcessorEnumerator_),
            "Failed to create D3D11 video processor enumerator.");
        ThrowIfFailed(
            videoDevice_->CreateVideoProcessor(videoProcessorEnumerator_.Get(), 0, &videoProcessor_),
            "Failed to create D3D11 video processor.");

        D3D11_TEXTURE2D_DESC textureDesc = {};
        textureDesc.Width = sourceWidth;
        textureDesc.Height = sourceHeight;
        textureDesc.MipLevels = 1;
        textureDesc.ArraySize = 1;
        textureDesc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        textureDesc.SampleDesc.Count = 1;
        textureDesc.Usage = D3D11_USAGE_DEFAULT;
        textureDesc.BindFlags = D3D11_BIND_RENDER_TARGET;

        ThrowIfFailed(
            device_->CreateTexture2D(&textureDesc, nullptr, &sourceBgraTexture_),
            "Failed to create D3D11 BGRA staging texture.");

        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC inputViewDesc = {};
        inputViewDesc.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
        inputViewDesc.Texture2D.ArraySlice = 0;
        inputViewDesc.Texture2D.MipSlice = 0;

        ThrowIfFailed(
            videoDevice_->CreateVideoProcessorInputView(sourceBgraTexture_.Get(), videoProcessorEnumerator_.Get(), &inputViewDesc, &videoProcessorInputView_),
            "Failed to create D3D11 video processor input view.");

        sourceWidth_ = sourceWidth;
        sourceHeight_ = sourceHeight;
    }

    ID3D11VideoProcessorOutputView* GetOutputView(ID3D11Texture2D* destinationTexture)
    {
        auto it = outputViews_.find(destinationTexture);
        if (it != outputViews_.end())
        {
            return it->second.Get();
        }

        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC outputViewDesc = {};
        outputViewDesc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
        ComPtr<ID3D11VideoProcessorOutputView> outputView;
        ThrowIfFailed(
            videoDevice_->CreateVideoProcessorOutputView(destinationTexture, videoProcessorEnumerator_.Get(), &outputViewDesc, &outputView),
            "Failed to create D3D11 video processor output view.");
        auto* rawView = outputView.Get();
        outputViews_.emplace(destinationTexture, std::move(outputView));
        return rawView;
    }

    void BlitScaledFrame(ID3D11Texture2D* sourceTexture, ID3D11Texture2D* destinationTexture)
    {
        D3D11_TEXTURE2D_DESC sourceDesc = {};
        sourceTexture->GetDesc(&sourceDesc);
        D3D11_TEXTURE2D_DESC destDesc = {};
        destinationTexture->GetDesc(&destDesc);

        if (sourceDesc.Width == destDesc.Width &&
            sourceDesc.Height == destDesc.Height &&
            sourceDesc.Format == destDesc.Format)
        {
            deviceContext_->CopyResource(destinationTexture, sourceTexture);
            return;
        }

        EnsureVideoProcessor(sourceDesc.Width, sourceDesc.Height);
        deviceContext_->CopyResource(sourceBgraTexture_.Get(), sourceTexture);

        RECT sourceRect = { 0, 0, static_cast<LONG>(sourceDesc.Width), static_cast<LONG>(sourceDesc.Height) };
        RECT destRect = { 0, 0, targetWidth_, targetHeight_ };
        videoContext_->VideoProcessorSetStreamSourceRect(videoProcessor_.Get(), 0, TRUE, &sourceRect);
        videoContext_->VideoProcessorSetStreamDestRect(videoProcessor_.Get(), 0, TRUE, &destRect);

        D3D11_VIDEO_PROCESSOR_STREAM stream = {};
        stream.Enable = TRUE;
        stream.pInputSurface = videoProcessorInputView_.Get();

        ThrowIfFailed(
            videoContext_->VideoProcessorBlt(videoProcessor_.Get(), GetOutputView(destinationTexture), 0, 1, &stream),
            "Failed to scale frame with D3D11 video processor.");
    }

    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> deviceContext_;
    ComPtr<ID3D11VideoDevice> videoDevice_;
    ComPtr<ID3D11VideoContext> videoContext_;
    std::unique_ptr<NvEncoderD3D11> encoder_;
    NV_ENC_INITIALIZE_PARAMS initializeParams_ = {};
    NV_ENC_CONFIG encodeConfig_ = {};
    std::vector<EncodedPacket> pendingPackets_;
    std::unordered_map<ID3D11Texture2D*, ComPtr<ID3D11VideoProcessorOutputView>> outputViews_;
    ComPtr<ID3D11VideoProcessorEnumerator> videoProcessorEnumerator_;
    ComPtr<ID3D11VideoProcessor> videoProcessor_;
    ComPtr<ID3D11Texture2D> sourceBgraTexture_;
    ComPtr<ID3D11VideoProcessorInputView> videoProcessorInputView_;
    UINT sourceWidth_ = 0;
    UINT sourceHeight_ = 0;
    int targetWidth_ = 0;
    int targetHeight_ = 0;
    int codec_ = 0;
    int bitrateBps_ = 0;
    int fps_ = 0;
    int gopLength_ = 0;
    bool gamePreset_ = false;
};
} // namespace

extern "C"
{
__declspec(dllexport) void* create_session(void* d3d11Device, int width, int height, int codec, int bitrateBps, int fps, int gopLength, int gamePreset)
{
    try
    {
        ClearLastErrorText();
        return new NativeNvencSession(
            reinterpret_cast<ID3D11Device*>(d3d11Device),
            width,
            height,
            codec,
            bitrateBps,
            fps,
            gopLength,
            gamePreset != 0);
    }
    catch (const std::exception& ex)
    {
        SetLastErrorText(ex.what());
        return nullptr;
    }
    catch (...)
    {
        SetLastErrorText("Unknown native NVENC session creation failure.");
        return nullptr;
    }
}

__declspec(dllexport) int encode_frame(void* session, void* texture, long long timestampHns, int forceIdr)
{
    try
    {
        ClearLastErrorText();
        auto* nativeSession = reinterpret_cast<NativeNvencSession*>(session);
        nativeSession->EncodeFrame(reinterpret_cast<ID3D11Texture2D*>(texture), timestampHns, forceIdr != 0);
        return 0;
    }
    catch (const std::exception& ex)
    {
        SetLastErrorText(ex.what());
        return -1;
    }
    catch (...)
    {
        SetLastErrorText("Unknown native NVENC encode failure.");
        return -1;
    }
}

__declspec(dllexport) int drain_packets(void* session, void (*callback)(const uint8_t*, int, long long, int, void*), void* userData)
{
    try
    {
        ClearLastErrorText();
        auto* nativeSession = reinterpret_cast<NativeNvencSession*>(session);
        nativeSession->DrainPackets(callback, userData);
        return 0;
    }
    catch (const std::exception& ex)
    {
        SetLastErrorText(ex.what());
        return -1;
    }
    catch (...)
    {
        SetLastErrorText("Unknown native NVENC drain failure.");
        return -1;
    }
}

__declspec(dllexport) int reconfigure(void* session, int bitrateBps, int fps, int gopLength)
{
    try
    {
        ClearLastErrorText();
        auto* nativeSession = reinterpret_cast<NativeNvencSession*>(session);
        nativeSession->Reconfigure(bitrateBps, fps, gopLength);
        return 0;
    }
    catch (const std::exception& ex)
    {
        SetLastErrorText(ex.what());
        return -1;
    }
    catch (...)
    {
        SetLastErrorText("Unknown native NVENC reconfigure failure.");
        return -1;
    }
}

__declspec(dllexport) void destroy_session(void* session)
{
    auto* nativeSession = reinterpret_cast<NativeNvencSession*>(session);
    delete nativeSession;
}

__declspec(dllexport) const char* get_last_error()
{
    return GetLastErrorText();
}
}
