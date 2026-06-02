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
*  This sample application measures transcoding performance in FPS.
*  This sample application takes a single input stream and spawns N pairs
*  of thread. In each pair, one thread is responsible for decoding the input
*  stream and making the decoded frames available to the other thread for
*  encoding.
*/

#include <stdio.h>
#include <iostream>
#include <thread>
#include <chrono>
#include <mutex>
#include <string.h>
#include <memory>
#include <functional>
#include <stdint.h>
#include <iomanip>
#include "NvEncoder/NvEncoderCuda.h"
#include "../Utils/NvEncoderCLIOptions.h"
#include "NvDecoder/NvDecoder.h"
#include "../Utils/NvCodecUtils.h"
#include "../Utils/FFmpegDemuxer.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

void EncProc(NvEncoderCuda *pEnc, uint8_t **apFrame, int nFrame, uint32_t inputFramePitch,
    volatile int *piEnc, volatile int *piDec, volatile bool *pbEnd, int *pnFrameTrans, std::exception_ptr& encException)
{
    try
    {
        StopWatch w;
        w.Start();
        while (*piEnc != *piDec || !*pbEnd)
        {
            if (*piEnc == *piDec)
            {
                std::this_thread::sleep_for(std::chrono::milliseconds(1));
                continue;
            }
            for (; *piEnc < *piDec || *pbEnd; (*piEnc)++)
            {
                std::vector<NvEncOutputFrame> vPacket;
                if (*piEnc < *piDec)
                {
                    const NvEncInputFrame* encoderInputFrame = pEnc->GetNextInputFrame();
                    NvEncoderCuda::CopyToDeviceFrame((CUcontext)pEnc->GetDevice(), (void*)apFrame[*piEnc % nFrame], inputFramePitch, (CUdeviceptr)encoderInputFrame->inputPtr,
                        encoderInputFrame->pitch, pEnc->GetEncodeWidth(), pEnc->GetEncodeHeight(), CU_MEMORYTYPE_DEVICE,
                        encoderInputFrame->bufferFormat,
                        encoderInputFrame->chromaOffsets,
                        encoderInputFrame->numChromaPlanes);

                    pEnc->EncodeFrame(vPacket);
                }
                else
                {
                    pEnc->EndEncode(vPacket);
                }
                *pnFrameTrans += (int)vPacket.size();
                if (*piEnc == *piDec && *pbEnd) break;
            }
        }
        std::cout << "Thread FPS=" << *pnFrameTrans / w.Stop() << std::endl;
    }
    catch (const std::exception&)
    {
        encException = std::current_exception();
    }
}

void TransProc(CUcontext cuContext, NvDecoder *pDec, FFmpegDemuxer *pDemuxer, int *pnFrameTrans,
    NvEncoderInitParam *pEncodeCLIOptions, std::exception_ptr& decException, std::exception_ptr& encException)
{
    try
    {
        // next frame to be decoded. apFrame[iDec] is unoccupied when iDec - iEnc < nFrame
        volatile int iDec = 0;
        // next frame to be encoded. apFrame[iEnc] is eligible for encoding when iEnc < iDec
        volatile int iEnc = 0;
        uint8_t *apFrameBuffer[16] = {};
        const int nFrameBuffer = sizeof(apFrameBuffer) / sizeof(apFrameBuffer[0]);
        volatile bool bEnd = false;

        int nVideoBytes = 0, nFrameReturned = 0, nFrame = 0;
        uint8_t *pVideo = NULL, *pFrame = NULL;
        using NvEncCudaPtr = std::unique_ptr<NvEncoderCuda, std::function<void(NvEncoderCuda*)>>;
        auto EncodeDeleteFunc = [](NvEncoderCuda *pEnc)
        {
            if (pEnc)
            {
                pEnc->DestroyEncoder();
                delete pEnc;
            }
        };
        NvEncCudaPtr pEnc(nullptr, EncodeDeleteFunc);
        NvThread thread;
        do
        {
            pDemuxer->Demux(&pVideo, &nVideoBytes);
            nFrameReturned = pDec->Decode(pVideo, nVideoBytes);
            if (!pEnc && nFrameReturned)
            {
                pEnc.reset(new NvEncoderCuda(cuContext, pDec->GetWidth(), pDec->GetHeight(),
                    pDec->GetBitDepth() == 8 ? NV_ENC_BUFFER_FORMAT_NV12 : NV_ENC_BUFFER_FORMAT_YUV420_10BIT));

                NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
                NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
                initializeParams.encodeConfig = &encodeConfig;
                pEnc->CreateDefaultEncoderParams(&initializeParams, pEncodeCLIOptions->GetEncodeGUID(), pEncodeCLIOptions->GetPresetGUID(), pEncodeCLIOptions->GetTuningInfo());

                pEncodeCLIOptions->SetInitParams(&initializeParams, pDec->GetBitDepth() == 8 ? NV_ENC_BUFFER_FORMAT_NV12 : NV_ENC_BUFFER_FORMAT_YUV420_10BIT);

                pEnc->CreateEncoder(&initializeParams);

                thread = NvThread(std::thread(EncProc, pEnc.get(), apFrameBuffer, nFrameBuffer, pDec->GetDeviceFramePitch(), &iEnc, &iDec, &bEnd, pnFrameTrans, std::ref(encException)));
            }
            for (int i = 0; i < nFrameReturned; i++)
            {
                /*
                 * If the condition below is true, the encoder thread is still
                 * consuming, or is about to consume, the oldest decoded frame
                 * and there is no space in apFrameBuffer[] to hold new decoded
                 * frames, so we must wait.
                 */
                pFrame = pDec->GetLockedFrame();
                while (iDec - iEnc == nFrameBuffer)
                {
                    std::this_thread::sleep_for(std::chrono::milliseconds(1));
                }

                if (apFrameBuffer[iDec % nFrameBuffer])
                {
                    pDec->UnlockFrame(&apFrameBuffer[iDec % nFrameBuffer]);
                }
                apFrameBuffer[iDec % nFrameBuffer] = pFrame;
                iDec++;
            }
        } while (nVideoBytes);

        bEnd = true;

        thread.join();

        for (int i = 0; i < nFrameBuffer; i++)
        {
            if (apFrameBuffer[i])
            {
                pDec->UnlockFrame(&apFrameBuffer[i]);
            }
        }
    }
    catch (const std::exception&)
    {
        decException = std::current_exception();
    }

}

static void ShowBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Transcoder Performance Sample Application\n";
    oss << "==============================================\n\n";

    oss << "Usage: AppTransPerf -i <input_file> [options]\n\n";

    // Brief table of core arguments
    oss << "Common Arguments:\n";
    oss << std::left << std::setw(25) << "Argument"
        << std::setw(12) << "Type"
        << "Default Value\n";
    oss << std::string(50, '-') << "\n";

    oss << std::left << std::setw(25) << "-i <path>"
        << std::setw(12) << "Required"
        << "N/A\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << "0\n";
    oss << std::left << std::setw(25) << "-thread <n>"
        << std::setw(12) << "Optional"
        << "2\n";
    oss << std::left << std::setw(25) << "-single"
        << std::setw(12) << "Optional"
        << "false\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    std::cout << oss.str();
    exit(0);
}

static void ShowHelpAdvanced()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Transcoder Performance Sample Application\n";
    oss << "==============================================\n\n";

    oss << "Usage: AppTransPerf -i <input_file> [options]\n\n";

    // Detailed descriptions
    oss << "Basic Options:\n";
    oss << std::left << std::setw(25) << "-i <path>" << ": Input video file to transcode\n";
    oss << std::left << std::setw(25) << "-gpu <n>" << ": GPU device ordinal\n";
    oss << std::left << std::setw(25) << "-thread <n>" << ": Number of encoding threads\n";
    oss << std::left << std::setw(25) << "-single" << ": Use single context\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print basic usage information\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print detailed usage information\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Multiple contexts are used by default for optimal performance\n";
    oss << "* Single context may result in suboptimal performance\n";
    oss << "* Each thread performs both decode and encode operations\n";
    oss << "* YUV444 format is not supported\n";
    oss << std::endl;

    // Encoder Parameters
    oss << "Encoder Parameters:\n";
    oss << NvEncoderInitParam().GetHelpMessage(false, false, true);

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

void ParseCommandLine(int argc, char *argv[], char *szInputFileName,
    int &iGpu, int &nThread, bool &bSingle, NvEncoderInitParam &initParam)
{
    std::ostringstream oss;

    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:\n";
        ShowBriefHelp();
    }

    for (int i = 1; i < argc; i++)
    {
        if (!_stricmp(argv[i], "-h") || !_stricmp(argv[i], "--help"))
        {
            ShowBriefHelp();
        }
        if (!_stricmp(argv[i], "-A") || !_stricmp(argv[i], "--advanced-options"))
        {
            ShowHelpAdvanced();
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
        if (!_stricmp(argv[i], "-gpu"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
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
    int iGpu = 0;
    int nThread = 2;
    bool bSingle = false;
    std::vector<std::exception_ptr> vDecExceptionPtrs;
    std::vector<std::exception_ptr> vEncExceptionPtrs;
    try
    {
        NvEncoderInitParam encodeCLIOptions;
        ParseCommandLine(argc, argv, szInFilePath, iGpu, nThread, bSingle, encodeCLIOptions);

        CheckInputFile(szInFilePath);

        uint8_t *pBuf = NULL;
        uint64_t nBufSize = 0;
        BufferedFileReader bufferedFileReader(szInFilePath);
        if (!bufferedFileReader.GetBuffer(&pBuf, &nBufSize)) {
            std::cout << "Failed to read file" << std::endl;
        }

        ck(cuInit(0));
        int nGpu = 0;
        ck(cuDeviceGetCount(&nGpu));
        if (iGpu < 0 || iGpu >= nGpu)
        {
            std::cout << "GPU ordinal out of range. Should be within [" << 0 << ", " << nGpu - 1 << "]" << std::endl;
            return 1;
        }
        CUdevice cuDevice = 0;
        ck(cuDeviceGet(&cuDevice, iGpu));
        char szDeviceName[80];
        ck(cuDeviceGetName(szDeviceName, sizeof(szDeviceName), cuDevice));
        std::cout << "GPU in use: " << szDeviceName << std::endl;

        std::vector<std::unique_ptr<FFmpegDemuxer>> vDemuxer;
        std::vector<std::unique_ptr<NvDecoder>> vpDec;
        std::vector<NvThread> vpThread;
        std::vector<int> vnFrameTrans(nThread);
        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));
        auto t0 = std::chrono::high_resolution_clock::now();
        vDecExceptionPtrs.resize(nThread);
        vEncExceptionPtrs.resize(nThread);

        for (int i = 0; i < nThread; i++)
        {
            if (!bSingle)
            {
                ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));
            }
            std::unique_ptr<FFmpegDemuxer> demuxer(new FFmpegDemuxer(szInFilePath));

            if (demuxer->GetChromaFormat() == AV_PIX_FMT_YUV444P || demuxer->GetChromaFormat() == AV_PIX_FMT_YUV444P10LE || demuxer->GetChromaFormat() == AV_PIX_FMT_YUV444P12LE)
            {
                std::cout << "Error: Sample app doesn't support YUV444" << std::endl;
                return 1;
            }

            vDemuxer.push_back(std::move(demuxer));

            std::unique_ptr<NvDecoder> dec(new NvDecoder(cuContext,
                true,
                FFmpeg2NvCodecId(vDemuxer[i]->GetVideoCodec()),
                false,
                true));

            vpDec.push_back(std::move(dec));

            vpThread.push_back(NvThread(std::thread(TransProc, cuContext, vpDec[i].get(), vDemuxer[i].get(), &vnFrameTrans[i], &encodeCLIOptions, std::ref(vDecExceptionPtrs[i]), std::ref(vEncExceptionPtrs[i]))));
        }
        for (int i = 0; i < nThread; i++)
        {
            vpThread[i].join();
        }
        for (int i = 0; i < nThread; i++)
        {
            if (vDecExceptionPtrs[i])
            {
                std::rethrow_exception(vDecExceptionPtrs[i]);
            }
            if (vEncExceptionPtrs[i])
            {
                std::rethrow_exception(vEncExceptionPtrs[i]);
            }
        }

        auto t1 = std::chrono::high_resolution_clock::now();
        auto msec = std::chrono::duration_cast<std::chrono::milliseconds>(t1.time_since_epoch() - t0.time_since_epoch()).count();

        int nFrameTransTotal = 0;
        for (int i = 0; i < nThread; i++)
        {
            nFrameTransTotal += vnFrameTrans[i];
            vpDec[i].reset(nullptr);
        }
        std::cout << "nFrameTransTotal=" << nFrameTransTotal << ", time=" << msec << " millisec, FPS=" << (nFrameTransTotal * 1000 / msec) << std::endl;
    }
    catch (const std::exception& ex)
    {
        std::cout << ex.what();
        exit(1);
    }
    return 0;
}
