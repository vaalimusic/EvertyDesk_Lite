/*
* Copyright 2017-2024 NVIDIA Corporation.  All rights reserved.
*
* Please refer to the NVIDIA end user license agreement (EULA) associated
* with this source code for terms and conditions that govern your use of
* this software. Any use, reproduction, disclosure, or distribution of
* this software and related documentation outside the terms of the EULA
* is strictly prohibited.
*
*/

/**
*  This sample application illustrates the encoding and streaming of a video
*  with one thread while another thread receives and decodes the video.
*  HDR video streaming is also demonstrated in this application.
*/

#include <cuda.h>
#include <iostream>
#include <iomanip>
#include <exception>
#include <stdexcept>
#include <memory>
#include <functional>
#include <stdint.h>
#include "NvDecoder/NvDecoder.h"
#include "NvEncoder/NvEncoderCuda.h"
#include "../Utils/NvEncoderCLIOptions.h"
#include "../Utils/NvCodecUtils.h"
#include "../Utils/FFmpegStreamer.h"
#include "../Utils/FFmpegDemuxer.h"
#include "../Utils/ColorSpace.h"
#include "../Common/AppEncUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

std::mutex g_initMutex;
std::condition_variable g_cvInit;
bool g_ClientInit = false;

void ShowEncoderBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Encode-Decode Sample Application\n";
    oss << "=======================================\n\n";

    oss << "Usage: AppEncDec -i <input_file> [options]\n\n";

    // Brief table of core arguments
    oss << "Common Arguments:\n";
    oss << std::left << std::setw(25) << "Argument"
        << std::setw(12) << "Type"
        << "Default Value\n";
    oss << std::string(50, '-') << "\n";

    oss << std::left << std::setw(25) << "-i <path>"
        << std::setw(12) << "Required"
        << "N/A\n";
    oss << std::left << std::setw(25) << "-o <path>"
        << std::setw(12) << "Optional"
        << "codec-based (out.h264/hevc/av1)\n";
    oss << std::left << std::setw(25) << "-s <WxH>"
        << std::setw(12) << "Required"
        << "N/A\n";
    oss << std::left << std::setw(25) << "-if <format>"
        << std::setw(12) << "Optional"
        << "iyuv\n";
    oss << std::left << std::setw(25) << "-of <format>"
        << std::setw(12) << "Optional"
        << "native\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view encoder capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

void ShowEncoderDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Encode-Decode Sample Application - Detailed Help\n";
    oss << "===================================================\n\n";

    oss << "Usage: AppEncDec -i <input_file> [options]\n\n";

    // Full table of all arguments
    oss << "All Arguments:\n";
    oss << std::left << std::setw(25) << "Argument"
        << std::setw(12) << "Type"
        << std::setw(20) << "Default Value"
        << "Example\n";
    oss << std::string(80, '-') << "\n";

    // Required arguments
    oss << std::left << std::setw(25) << "-i <path>"
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "-i input.yuv\n";
    oss << std::left << std::setw(25) << "-s <WxH>"
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "-s 1920x1080\n";

    // Optional arguments
    oss << std::left << std::setw(25) << "-o <path>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "codec-based"
        << "-o output.h264\n";
    oss << std::left << std::setw(25) << "-if <format>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "iyuv"
        << "-if yuv444\n";
    oss << std::left << std::setw(25) << "-of <format>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "native"
        << "-of bgra\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-o" << ": Output file path\n";
    oss << std::left << std::setw(25) << "-s" << ": Input resolution in WxH format\n";
    oss << std::left << std::setw(25) << "-if" << ": Input format (iyuv/nv12/nv16/p010/p210/bgra/bgra64)\n";
    oss << std::left << std::setw(25) << "-of" << ": Output format (native/bgra/bgra64)\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print basic usage information\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print detailed usage information\n";
    oss << std::left << std::setw(25) << "-ec/--encode-caps" << ": Print encode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* This sample demonstrates simultaneous encoding and decoding\n";
    oss << "* Supports HDR video streaming with VUI parameters\n";
    oss << "* Uses separate threads for encode and decode operations\n";
    oss << "* Native output format is nv12 for SDR and p010 for HDR\n";
    oss << "* Supports color space conversion to BGRA/BGRA64\n";
    oss << std::endl;

    oss << NvEncoderInitParam().GetHelpMessage(false, false, true, false, false, false, false, false) << std::endl;
    oss << "\nTo view encode capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

void ShowHelpAndExit(const char *szBadOption = NULL)
{
    if (szBadOption)
    {
        std::ostringstream oss;
        oss << "Error parsing \"" << szBadOption << "\"\n";
        oss << "Use -h/--help for basic usage or -A/--advanced-options for detailed information\n";
        throw std::invalid_argument(oss.str());
    }
}

enum OutputFormat
{
    native = 0, bgra, bgra64
};

std::vector<std::string> vstrOutputFormatName =
{
    "native", "bgra", "bgra64"
};

void ParseCommandLine(int argc, char *argv[], char *szInputFileName, int &nWidth, int &nHeight,
    NV_ENC_BUFFER_FORMAT &eInputFormat, OutputFormat &eOutputFormat, char *szOutputFileName,
    NvEncoderInitParam &initParam, int &iGpu)
{
    std::ostringstream oss;

    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:\n";
        ShowEncoderBriefHelp();
    }

    for (int i = 1; i < argc; i++)
    {
        if (!_stricmp(argv[i], "-h") || !_stricmp(argv[i], "--help")) {
            ShowEncoderBriefHelp();
        }
        if (!_stricmp(argv[i], "-A") || !_stricmp(argv[i], "--advanced-options")) {
            ShowEncoderDetailedHelp();
        }
        if (!_stricmp(argv[i], "-ec") || !_stricmp(argv[i], "--encode-caps")) {
            ShowEncoderCapability();
        }
        if (!_stricmp(argv[i], "-i")) {
            if (++i == argc) {
                ShowHelpAndExit("-i");
            }
            sprintf(szInputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-o")) {
            if (++i == argc) {
                ShowHelpAndExit("-o");
            }
            sprintf(szOutputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-s")) {
            if (++i == argc || 2 != sscanf(argv[i], "%dx%d", &nWidth, &nHeight)) {
                ShowHelpAndExit("-s");
            }
            continue;
        }
        std::vector<std::string> vszFileFormatName = {
            "iyuv", "nv12", "nv16", "p010", "p210", "bgra", "bgra64"
        };
        NV_ENC_BUFFER_FORMAT aFormat[] = {
            NV_ENC_BUFFER_FORMAT_IYUV,
            NV_ENC_BUFFER_FORMAT_NV12,
            NV_ENC_BUFFER_FORMAT_NV16,
            NV_ENC_BUFFER_FORMAT_YUV420_10BIT,
            NV_ENC_BUFFER_FORMAT_P210,
            NV_ENC_BUFFER_FORMAT_ARGB,
            NV_ENC_BUFFER_FORMAT_UNDEFINED,
        };
        if (!_stricmp(argv[i], "-if")) {
            if (++i == argc) {
                ShowHelpAndExit("-if");
            }
            auto it = std::find(vszFileFormatName.begin(), vszFileFormatName.end(), argv[i]);
            if (it == vszFileFormatName.end()) {
                ShowHelpAndExit("-if");
            }
            eInputFormat = aFormat[it - vszFileFormatName.begin()];
            continue;
        }
        if (!_stricmp(argv[i], "-of")) {
            if (++i == argc) {
                ShowHelpAndExit("-of");
            }
            auto it = find(vstrOutputFormatName.begin(), vstrOutputFormatName.end(), argv[i]);
            if (it == vstrOutputFormatName.end()) {
                ShowHelpAndExit("-of");
            }
            eOutputFormat = (OutputFormat)(it - vstrOutputFormatName.begin());
            continue;
        }
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        // Regard as encoder parameter
        if (argv[i][0] != '-') {
            ShowHelpAndExit(argv[i]);
        }
        oss << argv[i] << " ";
        while (i + 1 < argc && argv[i + 1][0] != '-') {
            oss << argv[++i] << " ";
        }
    }
    // Set VUI parameters for HDR
    std::function<void(NV_ENC_INITIALIZE_PARAMS *pParams)> funcInit = [](NV_ENC_INITIALIZE_PARAMS *pParam)
    {
        if (pParam->encodeGUID == NV_ENC_CODEC_HEVC_GUID)
        {
            NV_ENC_CONFIG_HEVC_VUI_PARAMETERS &hevcVUIParameters = pParam->encodeConfig->encodeCodecConfig.hevcConfig.hevcVUIParameters;
            hevcVUIParameters.videoSignalTypePresentFlag = 1;
            hevcVUIParameters.colourDescriptionPresentFlag = 1;
            hevcVUIParameters.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_FCC;
        }
        else if (pParam->encodeGUID == NV_ENC_CODEC_H264_GUID)
        {
            NV_ENC_CONFIG_H264_VUI_PARAMETERS &h264VUIParameters = pParam->encodeConfig->encodeCodecConfig.h264Config.h264VUIParameters;
            h264VUIParameters.videoSignalTypePresentFlag = 1;
            h264VUIParameters.colourDescriptionPresentFlag = 1;
            h264VUIParameters.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_FCC;
        }
		else
		{

		}
    };
    initParam = NvEncoderInitParam(oss.str().c_str(), (eInputFormat == NV_ENC_BUFFER_FORMAT_UNDEFINED) ? &funcInit : NULL);
}

void EncodeProc(CUdevice cuDevice, int nWidth, int nHeight, NV_ENC_BUFFER_FORMAT eFormat, NvEncoderInitParam *pEncodeCLIOptions,
    bool bBgra64, const char *szInFilePath, const char *szMediaPath, std::exception_ptr &encExceptionPtr)
{
    CUdeviceptr dpFrame = 0, dpBgraFrame = 0;
    CUcontext cuContext = NULL;

    try
    {
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));
        NvEncoderCuda enc(cuContext, nWidth, nHeight, eFormat, 3, false, false, false);
        NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
        NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
        initializeParams.encodeConfig = &encodeConfig;
        enc.CreateDefaultEncoderParams(&initializeParams, pEncodeCLIOptions->GetEncodeGUID(), pEncodeCLIOptions->GetPresetGUID(), pEncodeCLIOptions->GetTuningInfo());

        pEncodeCLIOptions->SetInitParams(&initializeParams, eFormat);

        enc.CreateEncoder(&initializeParams);

        std::ifstream fpIn(szInFilePath, std::ifstream::in | std::ifstream::binary);
        if (!fpIn)
        {
            std::cout << "Unable to open input file: " << szInFilePath << std::endl;
            return;
        }

        int nHostFrameSize = bBgra64 ? nWidth * nHeight * 8 : enc.GetFrameSize();
        std::unique_ptr<uint8_t[]> pHostFrame(new uint8_t[nHostFrameSize]);
        CUdeviceptr dpBgraFrame = 0;
        ck(cuMemAlloc(&dpBgraFrame, nWidth * nHeight * 8));
        int nFrame = 0;
        std::streamsize nRead = 0;

        // Wait for client sync before streamer init
        std::unique_lock<std::mutex> lock(g_initMutex);
        g_cvInit.wait(lock, [] { return g_ClientInit; });

        FFmpegStreamer streamer(pEncodeCLIOptions->IsCodecH264() ? AV_CODEC_ID_H264 : pEncodeCLIOptions->IsCodecHEVC() ? AV_CODEC_ID_HEVC : AV_CODEC_ID_AV1, nWidth, nHeight, 25, szMediaPath);
        do {
            std::vector<NvEncOutputFrame> vPacket;
            nRead = fpIn.read(reinterpret_cast<char*>(pHostFrame.get()), nHostFrameSize).gcount();
            if (nRead == nHostFrameSize)
            {
                const NvEncInputFrame* encoderInputFrame = enc.GetNextInputFrame();

                if (bBgra64)
                {
                    // Color space conversion
                    ck(cuMemcpyHtoD(dpBgraFrame, pHostFrame.get(), nHostFrameSize));
                    Bgra64ToP016((uint8_t *)dpBgraFrame, nWidth * 8, (uint8_t *)encoderInputFrame->inputPtr, encoderInputFrame->pitch, nWidth, nHeight);
                }
                else
                {
                    NvEncoderCuda::CopyToDeviceFrame(cuContext, pHostFrame.get(), 0, (CUdeviceptr)encoderInputFrame->inputPtr,
                        (int)encoderInputFrame->pitch,
                        enc.GetEncodeWidth(),
                        enc.GetEncodeHeight(),
                        CU_MEMORYTYPE_HOST,
                        encoderInputFrame->bufferFormat,
                        encoderInputFrame->chromaOffsets,
                        encoderInputFrame->numChromaPlanes);
                }
                enc.EncodeFrame(vPacket);
            }
            else
            {
                enc.EndEncode(vPacket);
            }
            for (NvEncOutputFrame &packet : vPacket) {
                streamer.Stream(packet.frame.data(), (int)packet.frame.size(), nFrame++);
            }
        } while (nRead == nHostFrameSize);
        ck(cuMemFree(dpBgraFrame));
        dpBgraFrame = 0;

        enc.DestroyEncoder();
        fpIn.close();

        std::cout << std::flush << "Total frames encoded: " << nFrame << std::endl << std::flush;
    }
    catch (const std::exception& )
    {
        encExceptionPtr = std::current_exception();
        ck(cuMemFree(dpBgraFrame));
        dpBgraFrame = 0;
        ck(cuMemFree(dpFrame));
        dpFrame = 0;
    }
}

void DecodeProc(CUdevice cuDevice, const char *szMediaUri, OutputFormat eOutputFormat, const char *szOutFilePath, std::exception_ptr &decExceptionPtr)
{
    CUdeviceptr dpRgbFrame = 0;
    try
    {
        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));

        // Notify the streamer thread:
        // demuxer() has a wait() loop with the listener waiting for connection
        // from the streamer/server side. This will wait (forever) until the
        // server sends connection request. So, notify streamer just before
        // starting demuxer(tcp listen).
        // Note: If both EncodeProc and DecodeProc threads are running on
        // same cpu core, the thread switch may happen after mutex release 
        // but before demux init. In such cases, encoder may start streaming
        // and may cause connection failure.

        std::unique_lock<std::mutex> lock(g_initMutex);
        g_ClientInit = true;
        g_cvInit.notify_one();
        lock.unlock();

        FFmpegDemuxer demuxer(szMediaUri);
        // Output host frame for native format; otherwise output device frame for CUDA processing
        NvDecoder dec(cuContext, eOutputFormat != native, FFmpeg2NvCodecId(demuxer.GetVideoCodec()), true);

        uint8_t *pVideo = NULL;
        int nVideoBytes = 0;
        int nFrame = 0;
        std::ofstream fpOut(szOutFilePath, std::ios::out | std::ios::binary);
        if (!fpOut)
        {
            std::ostringstream err;
            err << "Unable to open output file: " << szOutFilePath << std::endl;
            throw std::invalid_argument(err.str());
        }

        const char *szTail = "\xe0\x00\x00\x00\x01\xce\x8c\x4d\x9d\x10\x8e\x25\xe9\xfe";
        int nWidth = demuxer.GetWidth(), nHeight = demuxer.GetHeight();
        std::unique_ptr<uint8_t[]> pRgbFrame;
        int nRgbFramePitch = 0, nRgbFrameSize = 0;
        if (eOutputFormat != native) {
            nRgbFramePitch = nWidth * (eOutputFormat == bgra ? 4 : 8);
            nRgbFrameSize = nRgbFramePitch * nHeight;
            pRgbFrame.reset(new uint8_t[nRgbFrameSize]);
            ck(cuMemAlloc(&dpRgbFrame, nRgbFrameSize));
        }
        do {
            demuxer.Demux(&pVideo, &nVideoBytes);
            uint8_t *pFrame;
            int nFrameReturned = 0;
            if ((demuxer.GetVideoCodec() == AV_CODEC_ID_H264) || (demuxer.GetVideoCodec() == AV_CODEC_ID_HEVC))
            {
                nFrameReturned = dec.Decode(nVideoBytes > 0 ? pVideo + 6 : NULL,
                    // Cut head and tail generated by FFmpegDemuxer for H264/HEVC
                    nVideoBytes - (nVideoBytes > 20 && !memcmp(pVideo + nVideoBytes - 14, szTail, 14) ? 20 : 6),
                    CUVID_PKT_ENDOFPICTURE);
            }
            else
            {
                nFrameReturned = dec.Decode(pVideo, nVideoBytes);
            }

            int iMatrix = dec.GetVideoFormatInfo().video_signal_description.matrix_coefficients;
            if (!nFrame && nFrameReturned) {
                LOG(INFO) << "Color matrix coefficient: " << iMatrix;
            }
            for (int i = 0; i < nFrameReturned; i++) {
                pFrame = dec.GetFrame();
                if (eOutputFormat == native) {
                    fpOut.write(reinterpret_cast<char*>(pFrame), dec.GetFrameSize());
                }
                else {
                    // Color space conversion
                    if (dec.GetBitDepth() == 8) {
                        if (eOutputFormat == bgra) {
                            Nv12ToColor32<BGRA32>(pFrame, nWidth, (uint8_t *)dpRgbFrame, nRgbFramePitch, nWidth, nHeight, iMatrix);
                        }
                        else {
                            Nv12ToColor64<BGRA64>(pFrame, nWidth, (uint8_t *)dpRgbFrame, nRgbFramePitch, nWidth, nHeight, iMatrix);
                        }
                    }
                    else {
                        if (eOutputFormat == bgra) {
                            P016ToColor32<BGRA32>(pFrame, nWidth * 2, (uint8_t *)dpRgbFrame, nRgbFramePitch, nWidth, nHeight, iMatrix);
                        }
                        else {
                            P016ToColor64<BGRA64>(pFrame, nWidth * 2, (uint8_t *)dpRgbFrame, nRgbFramePitch, nWidth, nHeight, iMatrix);
                        }
                    }
                    ck(cuMemcpyDtoH(pRgbFrame.get(), dpRgbFrame, nRgbFrameSize));
                    fpOut.write(reinterpret_cast<char*>(pRgbFrame.get()), nRgbFrameSize);
                }
                nFrame++;
            }
        } while (nVideoBytes);
        if (eOutputFormat != native)
        {
            ck(cuMemFree(dpRgbFrame));
            dpRgbFrame = 0;
            pRgbFrame.reset(nullptr);
        }
        fpOut.close();

        std::cout << "Total frame decoded: " << nFrame << std::endl
            << "Saved in file " << szOutFilePath << " in "
            << (eOutputFormat == native ? (dec.GetBitDepth() == 8 ? "nv12" : "p010") : (eOutputFormat == bgra ? "bgra" : "bgra64"))
            << " format" << std::endl;
    }
    catch (const std::exception &)
    {
        decExceptionPtr = std::current_exception();
        cuMemFree(dpRgbFrame);
    }
}

int main(int argc, char **argv)
{
    char szInFilePath[256] = "",
        szOutFilePath[256] = "";
    int nWidth = 0, nHeight = 0;
    NV_ENC_BUFFER_FORMAT eInputFormat = NV_ENC_BUFFER_FORMAT_IYUV;
    OutputFormat eOutputFormat = native;
    int iGpu = 0;
    bool bBgra64 = false;
    std::exception_ptr encExceptionPtr;
    std::exception_ptr decExceptionPtr;
    try
    {
        NvEncoderInitParam encodeCLIOptions;
        ParseCommandLine(argc, argv, szInFilePath, nWidth, nHeight, eInputFormat, eOutputFormat, szOutFilePath, encodeCLIOptions, iGpu);

        CheckInputFile(szInFilePath);
        ValidateResolution(nWidth, nHeight);

        if (eInputFormat == NV_ENC_BUFFER_FORMAT_UNDEFINED) {
            bBgra64 = true;
            eInputFormat = NV_ENC_BUFFER_FORMAT_YUV420_10BIT;
        }
        if (!*szOutFilePath) {
            sprintf(szOutFilePath, "out.%s", eOutputFormat != native ?
                vstrOutputFormatName[eOutputFormat].c_str() :
                (eInputFormat != NV_ENC_BUFFER_FORMAT_YUV420_10BIT ? "nv12" : "p010"));
        }

        ck(cuInit(0));
        int nGpu = 0;
        ck(cuDeviceGetCount(&nGpu));
        if (iGpu < 0 || iGpu >= nGpu) {
            std::cout << "GPU ordinal out of range. Should be within [" << 0 << ", " << nGpu - 1 << "]" << std::endl;
            return 1;
        }
        CUdevice cuDevice = 0;
        ck(cuDeviceGet(&cuDevice, iGpu));
        char szDeviceName[80];
        ck(cuDeviceGetName(szDeviceName, sizeof(szDeviceName), cuDevice));
        std::cout << "GPU in use: " << szDeviceName << std::endl;

        const char *szMediaUri = "tcp://127.0.0.1:8899";
        char szMediaUriDecode[1024];
        sprintf(szMediaUriDecode, "%s?listen", szMediaUri);
        NvThread thDecode(std::thread(DecodeProc, cuDevice, szMediaUriDecode, eOutputFormat, szOutFilePath, std::ref(decExceptionPtr)));
        NvThread thEncode(std::thread(EncodeProc, cuDevice, nWidth, nHeight, eInputFormat, &encodeCLIOptions, bBgra64, szInFilePath, szMediaUri, std::ref(encExceptionPtr)));
        thEncode.join();
        thDecode.join();
        if (encExceptionPtr)
        {
            std::rethrow_exception(encExceptionPtr);
        }
        if (decExceptionPtr)
        {
            std::rethrow_exception(decExceptionPtr);
        }
    }
    catch (const std::exception &ex)
    {
        std::cout << ex.what();
        exit(1);
    }
    return 0;
}
