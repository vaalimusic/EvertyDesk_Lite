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
*  This sample application measures encoding performance in FPS.
*  The application creates multiple host threads and runs a different encoding session
*  on each thread. The number of threads can be controlled by the CLI option "-thread".
*  The application creates 2 host threads, each with a separate encode session, by
*  default. Note that on systems with GeForce GPUs, the number of simultaneous encode
*  sessions allowed on the system is restricted to 3 sessions.
*/

#include <stdio.h>
#include <iostream>
#include <thread>
#include <chrono>
#include <cuda.h>
#include <memory>
#include <iomanip>
#include "NvEncoder/NvEncoderCuda.h"
#include "../Utils/NvEncoderCLIOptions.h"
#include "../Utils/NvCodecUtils.h"
#include "../Common/AppEncUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

void EncProc(NvEncoder *pEnc, uint8_t *pBuf, uint64_t nBufSize, uint32_t nFrameTotal,
    std::exception_ptr &encException)
{
    try
    {
        std::vector<NvEncOutputFrame> vPacket;
        uint64_t nFrameSize = pEnc->GetFrameSize();
        uint32_t n = static_cast<uint32_t>(nBufSize / nFrameSize);
        ck(cuCtxSetCurrent((CUcontext)pEnc->GetDevice()));
        for (uint32_t i = 0; i < nFrameTotal; i++)
        {
            uint32_t iFrame = i / n % 2 ? (n - i % n - 1) : i % n;
            const NvEncInputFrame* encoderInputFrame = pEnc->GetNextInputFrame();
            NvEncoderCuda::CopyToDeviceFrame((CUcontext)pEnc->GetDevice(),
                pBuf + iFrame * nFrameSize,
                0,
                (CUdeviceptr)encoderInputFrame->inputPtr,
                encoderInputFrame->pitch,
                pEnc->GetEncodeWidth(),
                pEnc->GetEncodeHeight(),
                CU_MEMORYTYPE_DEVICE,
                encoderInputFrame->bufferFormat,
                encoderInputFrame->chromaOffsets,
                encoderInputFrame->numChromaPlanes, true);

            pEnc->EncodeFrame(vPacket);
        }
        pEnc->EndEncode(vPacket);
    }
    catch (const std::exception&)
    {
        encException = std::current_exception();
    }
}

void ShowEncoderBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Encoder Performance Sample Application\n";
    oss << "============================================\n\n";

    oss << "Usage: AppEncPerf -i <input_file> [options]\n\n";

    // Brief table of core arguments
    oss << "Common Arguments:\n";
    oss << std::left << std::setw(25) << "Argument"
        << std::setw(12) << "Type"
        << "Default Value\n";
    oss << std::string(50, '-') << "\n";

    oss << std::left << std::setw(25) << "-i <path>"
        << std::setw(12) << "Required"
        << "N/A\n";
    oss << std::left << std::setw(25) << "-s <WxH>"
        << std::setw(12) << "Required"
        << "N/A\n";
    oss << std::left << std::setw(25) << "-if <format>"
        << std::setw(12) << "Optional"
        << "iyuv\n";
    oss << std::left << std::setw(25) << "-frame <n>"
        << std::setw(12) << "Optional"
        << "1000\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << "0\n";
    oss << std::left << std::setw(25) << "-thread <n>"
        << std::setw(12) << "Optional"
        << "2\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view encoder capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

void ShowEncoderDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Encoder Performance Sample Application - Detailed Help\n";
    oss << "=======================================================\n\n";

    oss << "Usage: AppEncPerf -i <input_file> [options]\n\n";

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
    oss << std::left << std::setw(25) << "-if <format>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "iyuv"
        << "-if yuv444\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";
    oss << std::left << std::setw(25) << "-frame <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "2000"
        << "-frame 2000\n";
    oss << std::left << std::setw(25) << "-thread <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "2"
        << "-thread 4\n";
    oss << std::left << std::setw(25) << "-single"
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-single\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-s" << ": Input resolution in WxH format\n";
    oss << std::left << std::setw(25) << "-if" << ": Input format (iyuv/nv12/yv12/nv16/yuv444/p010/p210/yuv444p16/bgra/argb10/ayuv/abgr/abgr10)\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-frame" << ": Number of frames to encode per thread\n";
    oss << std::left << std::setw(25) << "-thread" << ": Number of encoding threads\n";
    oss << std::left << std::setw(25) << "-single" << ": Use single context (may result in suboptimal performance)\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print basic usage information\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print detailed usage information\n";
    oss << std::left << std::setw(25) << "-ec/--encode-caps" << ": Print encode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Width and height must be specified for encoding\n";
    oss << "* Multiple contexts are used by default for optimal performance\n";
    oss << "* Encode session limits: 8 concurrent sessions (GeForce) or unlimited (Quadro/Tesla)\n";
    oss << std::endl;

    oss << NvEncoderInitParam().GetHelpMessage(false, false, false, false, false, true, false, false) << std::endl;
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

void ParseCommandLine(int argc, char *argv[], char *szInputFileName, int &nWidth, int &nHeight,
    NV_ENC_BUFFER_FORMAT &eFormat, int &iGpu, uint32_t &nFrame, int &nThread,
    bool &bSingle, NvEncoderInitParam &initParam)
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
        if (!_stricmp(argv[i], "-i"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-i");
            }
            sprintf(szInputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-s"))
        {
            if (++i == argc || 2 != sscanf(argv[i], "%dx%d", &nWidth, &nHeight))
            {
                ShowHelpAndExit("-s");
            }
            continue;
        }
        std::vector<std::string> vszFileFormatName =
        {
            "iyuv", "nv12", "yv12", "nv16", "yuv444", "p010", "p210", "yuv444p16", "bgra", "argb10", "ayuv", "abgr", "abgr10"
        };
        NV_ENC_BUFFER_FORMAT aFormat[] =
        {
            NV_ENC_BUFFER_FORMAT_IYUV,
            NV_ENC_BUFFER_FORMAT_NV12,
            NV_ENC_BUFFER_FORMAT_YV12,
            NV_ENC_BUFFER_FORMAT_NV16,
            NV_ENC_BUFFER_FORMAT_YUV444,
            NV_ENC_BUFFER_FORMAT_YUV420_10BIT,
            NV_ENC_BUFFER_FORMAT_P210,
            NV_ENC_BUFFER_FORMAT_YUV444_10BIT,
            NV_ENC_BUFFER_FORMAT_ARGB,
            NV_ENC_BUFFER_FORMAT_ARGB10,
            NV_ENC_BUFFER_FORMAT_AYUV,
            NV_ENC_BUFFER_FORMAT_ABGR,
            NV_ENC_BUFFER_FORMAT_ABGR10,
        };
        if (!_stricmp(argv[i], "-if"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-if");
            }
            auto it = std::find(vszFileFormatName.begin(), vszFileFormatName.end(), argv[i]);
            if (it == vszFileFormatName.end())
            {
                ShowHelpAndExit("-if");
            }
            eFormat = aFormat[it - vszFileFormatName.begin()];
            continue;
        }
        if (!_stricmp(argv[i], "-gpu"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-frame"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-frame");
            }
            nFrame = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-thread"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-thread");
            }
            nThread = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-single"))
        {
            bSingle = true;
            continue;
        }
        // Regard as encoder parameter
        if (argv[i][0] != '-')
        {
            ShowHelpAndExit(argv[i]);
        }
        oss << argv[i] << " ";
        while (i + 1 < argc && argv[i + 1][0] != '-')
        {
            oss << argv[++i] << " ";
        }
    }
    initParam = NvEncoderInitParam(oss.str().c_str());
}


int main(int argc, char **argv)
{
    char szInFilePath[256] = "";
    int nWidth = 0, nHeight = 0;
    NV_ENC_BUFFER_FORMAT eFormat = NV_ENC_BUFFER_FORMAT_IYUV;
    int iGpu = 0;
    uint32_t nFrame = 1000;
    int nThread = 2;
    bool bSingle = false;
    std::vector<std::exception_ptr> vExceptionPtrs;
    std::vector<CUdeviceptr> vdpBuf;
    using NvEncPtr = std::unique_ptr<NvEncoder, std::function<void(NvEncoder*)>>;
    auto EncodeDeleteFunc = [](NvEncoder *pEnc)
    {
        if (pEnc)
        {
            pEnc->DestroyEncoder();
            delete pEnc;
        }
    };
    try
    {
        NvEncoderInitParam encodeCLIOptions;
        ParseCommandLine(argc, argv, szInFilePath, nWidth, nHeight, eFormat,
            iGpu, nFrame, nThread, bSingle, encodeCLIOptions);

        CheckInputFile(szInFilePath);
        ValidateResolution(nWidth, nHeight);

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

        uint8_t *pBuf = NULL;
        uint64_t nBufSize = 0;
        BufferedFileReader bufferedFileReader(szInFilePath, true);
        if (!bufferedFileReader.GetBuffer(&pBuf, &nBufSize)) {
            std::cout << "Failed to read file " << szInFilePath << std::endl;
            return 1;
        }

        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, CU_CTX_SCHED_BLOCKING_SYNC, cuDevice));

        std::vector<CUdeviceptr> vdpBuf;


        std::vector<NvEncPtr> vEnc;
        CUdeviceptr dpBuf;
        ck(cuMemAlloc(&dpBuf, (size_t)nBufSize));
        vdpBuf.push_back(dpBuf);
        ck(cuMemcpyHtoD(dpBuf, pBuf, (size_t)nBufSize));
        NvEncPtr pEnc(new NvEncoderCuda(cuContext, nWidth, nHeight, eFormat), EncodeDeleteFunc);

        NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
        NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
        initializeParams.encodeConfig = &encodeConfig;
        pEnc->CreateDefaultEncoderParams(&initializeParams, encodeCLIOptions.GetEncodeGUID(), encodeCLIOptions.GetPresetGUID(), encodeCLIOptions.GetTuningInfo());

        encodeCLIOptions.SetInitParams(&initializeParams, eFormat);

        pEnc->CreateEncoder(&initializeParams);
        vEnc.push_back(std::move(pEnc));


        for (int i = 1; i < nThread; i++)
        {
            if (!bSingle) {
                ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, CU_CTX_SCHED_BLOCKING_SYNC, cuDevice));
                CUdeviceptr dpBuf;
                ck(cuMemAlloc(&dpBuf, (size_t)nBufSize));
                vdpBuf.push_back(dpBuf);
                ck(cuMemcpyHtoD(vdpBuf[i], pBuf, (size_t)nBufSize));
            }
            NvEncPtr pEncoder(new NvEncoderCuda(cuContext, nWidth, nHeight, eFormat), EncodeDeleteFunc);
            // all the encoder instances share the same config params , so just use the parameters from first encoder instance
            pEncoder->CreateEncoder(&initializeParams);

            vEnc.push_back(std::move(pEncoder));
        }

        std::vector<NvThread> vThread;
        vExceptionPtrs.resize(nThread);
        StopWatch w;
        w.Start();
        for (int i = 0; i < nThread; i++)
        {
            vThread.push_back(NvThread(std::thread(EncProc,
                vEnc[i].get(),
                (uint8_t *)(bSingle ? dpBuf : vdpBuf[i]),
                nBufSize, nFrame,
                std::ref(vExceptionPtrs[i]))));
        }

        for (auto& t : vThread)
            t.join();

        double t = w.Stop();

        for (int i = 0; i < nThread; i++)
        {
            ck(cuCtxSetCurrent((CUcontext)vEnc[i]->GetDevice()));
            if (!bSingle && i > 0)
            {
                ck(cuMemFree(vdpBuf[i]));
                vdpBuf[i] = 0;
            }
            if (vEnc[i])
            {
                vEnc[i]->DestroyEncoder();
            }
        }
        vdpBuf.clear();

        for (int i = 0; i < nThread; i++)
        {
            if (vExceptionPtrs[i])
                std::rethrow_exception(vExceptionPtrs[i]);
        }

        if (t)
        {
            int nTotal = nFrame * nThread;
            std::cout << "nTotal=" << nTotal << ", time=" << t << " seconds, FPS=" << nTotal / t << std::endl;
        }
    }
    catch (const std::exception &ex)
    {
        for (CUdeviceptr dpBuf : vdpBuf)
            cuMemFree(dpBuf);

        std::cout << ex.what();
        exit(1);
    }

    return 0;
}
