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

//---------------------------------------------------------------------------
//! \file AppDecPerf.cpp
//! \brief Source file for AppDecPerf sample
//!
//!  This sample application measures decoding performance in FPS.
//!  The application creates multiple host threads and runs a different decoding session on each thread.
//!  The number of threads can be controlled by the CLI option "-thread".
//!  The application creates 2 host threads, each with a separate decode session, by default.
//!  The application supports measuring the decode performance only (keeping decoded
//!  frames in device memory as well as measuring the decode performance including transfer
//!  of frames to the host memory.
//---------------------------------------------------------------------------

#include <cuda.h>
#include <cudaProfiler.h>
#include <stdio.h>
#include <iostream>
#include <thread>
#include <chrono>
#include <string.h>
#include <memory>
#include "NvDecoder/NvDecoder.h"
#include "../Utils/NvCodecUtils.h"
#include "../Utils/FFmpegDemuxer.h"
#include "../Common/AppDecUtils.h"
#include <chrono>
#include <future>
#include <iomanip>

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

struct SessionStats
{
    int64_t initTime;   // session initialization time
    int64_t decodeTime; // time taken by actual decoding operation
    int frames;     // number of frames decoded
};

using NvDecodePromise = std::promise<SessionStats>;
using NvDecodeFuture = std::future<SessionStats>;


class NvDecoderPerf : public NvDecoder
{
public:
    NvDecoderPerf(CUcontext cuContext, bool bUseDeviceFrame, cudaVideoCodec eCodec);
    void SetSessionInitTime(int64_t duration) { m_sessionInitTime = duration; }
    int64_t GetSessionInitTime() { return m_sessionInitTime; }

    static void IncrementSessionInitCounter() { m_sessionInitCounter++; }
    static uint32_t GetSessionInitCounter() { return m_sessionInitCounter; }
    static void SetSessionCount(uint32_t count) { m_sessionCount = count; }
    static uint32_t GetSessionCount(void) { return m_sessionCount; }

protected:
    int HandleVideoSequence(CUVIDEOFORMAT *pVideoFormat);

    int64_t m_sessionInitTime;
    static std::mutex m_initMutex;
    static std::condition_variable m_cvInit;
    static uint32_t m_sessionInitCounter;
    static uint32_t m_sessionCount;
};

std::mutex NvDecoderPerf::m_initMutex;
std::condition_variable NvDecoderPerf::m_cvInit;
uint32_t NvDecoderPerf::m_sessionInitCounter = 0;
uint32_t NvDecoderPerf::m_sessionCount = 1;

NvDecoderPerf::NvDecoderPerf(CUcontext cuContext, bool bUseDeviceFrame, cudaVideoCodec eCodec)
: NvDecoder(cuContext, bUseDeviceFrame, eCodec)
{
}

int NvDecoderPerf::HandleVideoSequence(CUVIDEOFORMAT *pVideoFormat)
{
    auto sessionStart = std::chrono::high_resolution_clock::now();

    int nDecodeSurface = NvDecoder::HandleVideoSequence(pVideoFormat);

    std::unique_lock<std::mutex> lock(m_initMutex);

    IncrementSessionInitCounter();

    // Wait for all threads to finish initialization of the decoder session.
    // This ensures that all threads start decoding frames at the same
    // time and saturate the decoder engines. This also leads to more
    // accurate measurement of decoding performance.
    if (GetSessionInitCounter() == GetSessionCount())
    {
        m_cvInit.notify_all();
    }
    else
    {
        m_cvInit.wait(lock, [] { return NvDecoderPerf::GetSessionInitCounter() >= NvDecoderPerf::GetSessionCount(); });
    }

    auto sessionEnd = std::chrono::high_resolution_clock::now();
    int64_t elapsedTime = std::chrono::duration_cast<std::chrono::milliseconds>(sessionEnd - sessionStart).count();

    SetSessionInitTime(elapsedTime);
    return nDecodeSurface;
}


/**
*   @brief  Function to decode media file using NvDecoder interface
*   @param  pDec    - Handle to NvDecoder
*   @param  demuxer - Pointer to an FFmpegDemuxer instance
*   @param  pnFrame - Variable to record the number of frames decoded
*   @param  ex      - Stores current exception in case of failure
*/
void DecProc(NvDecoderPerf *pDec, FFmpegDemuxer *demuxer, NvDecodePromise& promise, std::exception_ptr &ex)
{
    SessionStats stats = {0, 0, 0};
    auto sessionStart = std::chrono::high_resolution_clock::now();

    try
    {
        int nVideoBytes = 0, nFrameReturned = 0, nFrame = 0;
        uint8_t *pVideo = NULL, *pFrame = NULL;

        do {
            demuxer->Demux(&pVideo, &nVideoBytes);
            nFrameReturned = pDec->Decode(pVideo, nVideoBytes);
            if (!nFrame && nFrameReturned)
                LOG(INFO) << pDec->GetVideoInfo();

            nFrame += nFrameReturned;
        } while (nVideoBytes);
        stats.frames = nFrame;
        stats.initTime = pDec->GetSessionInitTime();
    }
    catch (std::exception&)
    {
        ex = std::current_exception();
    }

    auto sessionEnd = std::chrono::high_resolution_clock::now();
    stats.decodeTime = std::chrono::duration_cast<std::chrono::milliseconds>(sessionEnd - sessionStart).count();

    promise.set_value(stats);
}

static void ShowBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Performance Sample Application\n";
    oss << "============================================\n\n";
    
    oss << "Usage: AppDecPerf -i <input_file> [options]\n\n";

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

    oss << std::left << std::setw(25) << "-host" 
        << std::setw(12) << "Optional"
        << "false\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view decode capabilities, use -dc/--decode-caps\n";
    std::cout << oss.str();
    exit(0);
}

static void ShowDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Performance Sample Application - Detailed Help\n";
    oss << "========================================================\n\n";
    
    oss << "Usage: AppDecPerf -i <input_file> [options]\n\n";

    // Full table of all arguments
    oss << "All Arguments:\n";
    oss << std::left << std::setw(25) << "Argument" 
        << std::setw(12) << "Type"
        << std::setw(20) << "Default Value"
        << "Usage\n";
    oss << std::string(80, '-') << "\n";

    // Required arguments
    oss << std::left << std::setw(25) << "-i <path>" 
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "-i input.h264\n";

    // Optional arguments
    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";

    oss << std::left << std::setw(25) << "-thread <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "2"
        << "-thread 4\n";

    oss << std::left << std::setw(25) << "-single" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-single\n";

    oss << std::left << std::setw(25) << "-host" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-host\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-thread" << ": Number of decoding threads\n";
    oss << std::left << std::setw(25) << "-single" << ": Use single context\n";
    oss << std::left << std::setw(25) << "-host" << ": Copy frame to host memory\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print usage information for common commandline options\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print usage information for common and advanced commandline options\n";
    oss << std::left << std::setw(25) << "-dc/--decode-caps" << ": Print decode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Single context may result in suboptimal performance\n";
    oss << "* Host memory copy may result in suboptimal performance\n";
    oss << "* Multiple contexts are used by default\n";
    oss << "* Device memory is used by default\n";
    oss << std::endl;
    oss << "To view decode capabilities, use -dc/--decode-caps\n";

    std::cout << oss.str();
    exit(0);
}

static void ShowHelpAndExit(const char *szBadOption = NULL)
{
    if (szBadOption) 
    {
        std::ostringstream oss;
        oss << "Error parsing \"" << szBadOption << "\"\n";
        oss << "Use -h/--help for basic usage or -A/--advanced-options for detailed information\n";
        throw std::invalid_argument(oss.str());
    }
}

void ParseCommandLine(int argc, char *argv[], char *szInputFileName, int &iGpu, int &nThread, bool &bSingle, bool &bHost) 
{
    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:" << "\"\n";
        ShowBriefHelp();
    }
    for (int i = 1; i < argc; i++) {
        if (!_stricmp(argv[i], "-h") || !_stricmp(argv[i], "--help")) {
            ShowBriefHelp();
        }
        if (!_stricmp(argv[i], "-A") || !_stricmp(argv[i], "--advanced-options")) {
            ShowDetailedHelp();
        }
        if (!_stricmp(argv[i], "-dc") || !_stricmp(argv[i], "--decode-caps")) {
            ShowDecoderCapability();
        }
        if (!_stricmp(argv[i], "-i")) {
            if (++i == argc) {
                ShowHelpAndExit("-i");
            }
            sprintf(szInputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-thread")) {
            if (++i == argc) {
                ShowHelpAndExit("-thread");
            }
            nThread = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-single")) {
            bSingle = true;
            continue;
        }
        if (!_stricmp(argv[i], "-host")) {
            bHost = true;
            continue;
        }
        ShowHelpAndExit(argv[i]);
    }
}

struct NvDecPerfData
{
    uint8_t *pBuf;
    std::vector<uint8_t *> *pvpPacketData; 
    std::vector<int> *pvpPacketDataSize;
};

int CUDAAPI HandleVideoData(void *pUserData, CUVIDSOURCEDATAPACKET *pPacket) {
    NvDecPerfData *p = (NvDecPerfData *)pUserData;
    memcpy(p->pBuf, pPacket->payload, pPacket->payload_size);
    p->pvpPacketData->push_back(p->pBuf);
    p->pvpPacketDataSize->push_back(pPacket->payload_size);
    p->pBuf += pPacket->payload_size;
    return 1;
}

int main(int argc, char **argv)
{
    char szInFilePath[256] = "";
    int iGpu = 0;
    int nThread = 2; 
    bool bSingle = false;
    bool bHost = false;
    std::vector<std::exception_ptr> vExceptionPtrs;
    std::vector<NvDecodePromise> vPromise;
    std::vector<NvDecodeFuture> vFuture;

    try {
        ParseCommandLine(argc, argv, szInFilePath, iGpu, nThread, bSingle, bHost);
        CheckInputFile(szInFilePath);

        struct stat st;
        if (stat(szInFilePath, &st) != 0) {
            return 1;
        }
        int nBufSize = st.st_size;

        uint8_t *pBuf = NULL;
        try {
            pBuf = new uint8_t[nBufSize];
        }
        catch (std::bad_alloc) {
            std::cout << "Failed to allocate memory in BufferedReader" << std::endl;
            return 1;
        }
        std::vector<uint8_t *> vpPacketData;
        std::vector<int> vnPacketData;

        NvDecPerfData userData = { pBuf, &vpPacketData, &vnPacketData };

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

        std::vector<std::unique_ptr<FFmpegDemuxer>> vDemuxer;
        std::vector<std::unique_ptr<NvDecoderPerf>> vDec;
        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));
        vExceptionPtrs.resize(nThread);
        vPromise.resize(nThread);

        for (int i = 0; i < nThread; i++)
        {
            if (!bSingle)
            {
                ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));
            }
            std::unique_ptr<FFmpegDemuxer> demuxer(new FFmpegDemuxer(szInFilePath));

            NvDecoderPerf* sessionObject = new NvDecoderPerf(cuContext, !bHost, FFmpeg2NvCodecId(demuxer->GetVideoCodec()));
            std::unique_ptr<NvDecoderPerf> dec(sessionObject);
            vDemuxer.push_back(std::move(demuxer));
            vDec.push_back(std::move(dec));
        }

        NvDecoderPerf::SetSessionCount(nThread);

        float totalFPS = 0;
        std::vector<NvThread> vThread;

        for (int i = 0; i < nThread; i++)
        {
            vThread.push_back(NvThread(std::thread(DecProc, vDec[i].get(), vDemuxer[i].get(), std::ref(vPromise[i]), std::ref(vExceptionPtrs[i]))));
            vFuture.push_back(vPromise[i].get_future());
        }

        int nTotal = 0;
        for (int i = 0; i < nThread; i++)
        {
            SessionStats stats = vFuture[i].get();
            nTotal += stats.frames;

            totalFPS += (stats.frames / ((stats.decodeTime - stats.initTime) / 1000.0f));

            vThread[i].join();
            vDec[i].reset(nullptr);
        }

        std::cout << "Total Frames Decoded=" << nTotal << " FPS = " << totalFPS << std::endl;

        ck(cuProfilerStop());

        for (int i = 0; i < nThread; i++)
        {
            if (vExceptionPtrs[i])
            {
                std::rethrow_exception(vExceptionPtrs[i]);
            }
        }
    }
    catch (const std::exception& ex)
    {
        std::cout << ex.what();
        exit(1);
    }
    return 0;
}
