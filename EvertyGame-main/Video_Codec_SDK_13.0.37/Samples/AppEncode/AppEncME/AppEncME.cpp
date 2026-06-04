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
*  This sample application illustrates the use of NVENC hardware to calculate
*  motion vectors. The application uses the CUDA device type and associated
*  buffers when demonstrating the usage of the ME-only mode but can be used
*  with other device types like D3D and OpenGL.
*/

#include <iostream>
#include <fstream>
#include <cuda.h>
#include <memory>
#include <functional>
#include <stdint.h>
#include <iomanip>
#include "NvEncoder/NvEncoderCuda.h"
#include "../Utils/Logger.h"
#include "../Utils/NvEncoderCLIOptions.h"
#include "../Utils/NvCodecUtils.h"
#include "../Common/AppEncUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

void MotionEstimationWithBufferedFile(NvEncoderCuda *pEnc, int nWidth, int nHeight, NvEncoderInitParam *pInitParam,
    char *szInFilePath, char *szOutFilePath, uint32_t nFrame)
{
    std::ofstream fpOut(szOutFilePath, std::ios::out | std::ios::binary);
    if (!fpOut)
    {
        std::ostringstream err;
        err << "Unable to open output file: " << szOutFilePath << std::endl;
        throw std::invalid_argument(err.str());
    }

    uint8_t *pBuf = NULL;
    uint64_t nBufSize = 0;
    BufferedFileReader bufferedFileReader(szInFilePath);
    if (!bufferedFileReader.GetBuffer(&pBuf, &nBufSize)) {
        std::ostringstream err;
        err << "Failed to read file " << szInFilePath << std::endl;
        throw std::invalid_argument(err.str());
    }

    uint64_t nFrameSize = pEnc->GetFrameSize();
    uint32_t n = static_cast<uint32_t>(nBufSize / nFrameSize);

    if (nFrame == 0)
    {
        nFrame = n - 1;
    }
    else
    {
        nFrame = (std::min)(nFrame, n);
    }

    if (nFrame == 1)
    {
        std::ostringstream err;
        err << "At least 2 frames are needed for motion estimation." << std::endl;
        throw std::invalid_argument(err.str());
    }

    for (uint32_t i = 0; i < nFrame - 1; i++)
    {
        uint32_t iReferenceFrame = i, iFrame = i + 1;

        const NvEncInputFrame* inputFrame = pEnc->GetNextInputFrame();
        const NvEncInputFrame* referenceFrame = pEnc->GetNextReferenceFrame();

        // Copy the reference and input frames to the base class device memory allocations.

        NvEncoderCuda::CopyToDeviceFrame(reinterpret_cast<CUcontext>(pEnc->GetDevice()),
            (uint8_t *)pBuf + iFrame * nFrameSize,
            0,
            (CUdeviceptr)inputFrame->inputPtr,
            (uint32_t)inputFrame->pitch,
            pEnc->GetEncodeWidth(),
            pEnc->GetEncodeHeight(),
            CU_MEMORYTYPE_HOST,
            inputFrame->bufferFormat,
            inputFrame->chromaOffsets,
            inputFrame->numChromaPlanes);

        NvEncoderCuda::CopyToDeviceFrame(reinterpret_cast<CUcontext>(pEnc->GetDevice()),
            (uint8_t *)pBuf + iReferenceFrame * nFrameSize,
            0,
            (CUdeviceptr)referenceFrame->inputPtr,
            (uint32_t)referenceFrame->pitch,
            pEnc->GetEncodeWidth(),
            pEnc->GetEncodeHeight(),
            CU_MEMORYTYPE_HOST,
            referenceFrame->bufferFormat,
            referenceFrame->chromaOffsets,
            referenceFrame->numChromaPlanes);

        // Execute motion estimation for the current pair of frames.
        std::vector<uint8_t> mvData;
        pEnc->RunMotionEstimation(mvData);

        // Parse the output from the API to produce human-readable motion vector output.
        fpOut << "Motion Vectors for input frame = " << iFrame << ", reference frame = " << iReferenceFrame << std::endl;
        if (pInitParam->IsCodecH264())
        {
            int m = ((nWidth + 15) / 16) * ((nHeight + 15) / 16);
            fpOut << "block, mb_type, partitionType, "
                << "MV[0].x, MV[0].y, MV[1].x, MV[1].y, MV[2].x, MV[2].y, MV[3].x, MV[3].y, cost" << std::endl;

            NV_ENC_H264_MV_DATA *outputMV = (NV_ENC_H264_MV_DATA *)mvData.data();
            for (int l = 0; l < m; l++)
            {
                fpOut << l << ", " << static_cast<int>(outputMV[l].mbType) << ", " << static_cast<int>(outputMV[l].partitionType) << ", " <<
                    outputMV[l].mv[0].mvx << ", " << outputMV[l].mv[0].mvy << ", " << outputMV[l].mv[1].mvx << ", " << outputMV[l].mv[1].mvy << ", " <<
                    outputMV[l].mv[2].mvx << ", " << outputMV[l].mv[2].mvy << ", " << outputMV[l].mv[3].mvx << ", " << outputMV[l].mv[3].mvy << ", " << outputMV[l].mbCost;
                fpOut << std::endl;
            }
        } else {
            int m = ((nWidth + 31) / 32) * ((nHeight + 31) / 32);
            fpOut << "ctb, cuType, cuSize, partitionMode, " <<
                "MV[0].x, MV[0].y, MV[1].x, MV[1].y, MV[2].x, MV[2].y, MV[3].x, MV[3].y" << std::endl;
            NV_ENC_HEVC_MV_DATA *outputMV = (NV_ENC_HEVC_MV_DATA *)mvData.data();
            bool lastCUInCTB = false;
            for (int l = 0; l < m;)
            {
                do
                {
                    lastCUInCTB = outputMV->lastCUInCTB ? true : false;
                    fpOut << l << ", " << static_cast<int>(outputMV->cuType) << ", " << static_cast<int>(outputMV->cuSize) << ", " << static_cast<int>(outputMV->partitionMode) << ", " <<
                    outputMV->mv[0].mvx << ", " << outputMV->mv[0].mvy << ", " << outputMV->mv[1].mvx << ", " << outputMV->mv[1].mvy << ", " <<
                    outputMV->mv[2].mvx << ", " << outputMV->mv[2].mvy << ", " << outputMV->mv[3].mvx << ", " << outputMV->mv[3].mvy << std::endl;

                    outputMV += 1;
                    l++;
                } while (!lastCUInCTB);
            }
        }
    }
    fpOut.close();

    std::cout << "Motion vectors saved in file " << szOutFilePath << std::endl;
}

void ShowEncoderBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Motion Estimation Sample Application\n";
    oss << "===========================================\n\n";

    oss << "Usage: AppEncME -i <input_file> [options]\n\n";

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
        << "out.mv.txt\n";
    oss << std::left << std::setw(25) << "-s <WxH>"
        << std::setw(12) << "Required"
        << "N/A\n";
    oss << std::left << std::setw(25) << "-if <format>"
        << std::setw(12) << "Optional"
        << "iyuv\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << "0\n";
    oss << std::left << std::setw(25) << "-frame <n>"
        << std::setw(12) << "Optional"
        << "0\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view encoder capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

void ShowEncoderDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Motion Estimation Sample Application - Detailed Help\n";
    oss << "===========================================================\n\n";

    oss << "Usage: AppEncME -i <input_file> [options]\n\n";

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
        << std::setw(20) << "out.mv.txt"
        << "-o output.mv.txt\n";
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
        << std::setw(20) << "All frames"
        << "-frame 100\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-o" << ": Output motion vector file path\n";
    oss << std::left << std::setw(25) << "-s" << ": Input resolution in WxH format\n";
    oss << std::left << std::setw(25) << "-if" << ": Input format (iyuv/nv12/yv12/yuv444/p010/yuv444p16/bgra/argb10/ayuv/abgr/abgr10)\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-frame" << ": Number of frames to process\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print basic usage information\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print detailed usage information\n";
    oss << std::left << std::setw(25) << "-ec/--encode-caps" << ": Print encode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* This sample demonstrates hardware motion estimation\n";
    oss << "* Outputs motion vectors to a text file for analysis\n";
    oss << "* At least 2 frames are needed for motion estimation\n";
    oss << "* Supports both H.264 and HEVC motion estimation\n";
    oss << std::endl;

    oss << NvEncoderInitParam().GetHelpMessage(true, false, true, false, false, false, false, false) << std::endl;
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
    NV_ENC_BUFFER_FORMAT &eFormat, char *szOutputFileName, NvEncoderInitParam &initParam,
    int &iGpu, uint32_t &nFrame)
{
    std::ostringstream oss;
    int i;

    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:\n";
        ShowEncoderBriefHelp();
    }

    for (i = 1; i < argc; i++)
    {
        if (!_stricmp(argv[i], "-h") || !_stricmp(argv[i], "--help"))
        {
            ShowEncoderBriefHelp();
        }
        if (!_stricmp(argv[i], "-A") || !_stricmp(argv[i], "--advanced-options"))
        {
            ShowEncoderDetailedHelp();
        }
        if (!_stricmp(argv[i], "-ec") || !_stricmp(argv[i], "--encode-caps"))
        {
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
        if (!_stricmp(argv[i], "-o"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit("-o");
            }
            sprintf(szOutputFileName, "%s", argv[i]);
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
            "iyuv", "nv12", "yv12", "yuv444", "p010", "yuv444p16", "bgra", "argb10", "ayuv", "abgr", "abgr10"
        };
        NV_ENC_BUFFER_FORMAT aFormat[] =
        {
            NV_ENC_BUFFER_FORMAT_IYUV,
            NV_ENC_BUFFER_FORMAT_NV12,
            NV_ENC_BUFFER_FORMAT_YV12,
            NV_ENC_BUFFER_FORMAT_YUV444,
            NV_ENC_BUFFER_FORMAT_YUV420_10BIT,
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
            auto it = find(vszFileFormatName.begin(), vszFileFormatName.end(), argv[i]);
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
    char szInFilePath[256] = "",
        szOutFilePath[256] = "out.mv.txt";
    int nWidth = 0, nHeight = 0;
    NV_ENC_BUFFER_FORMAT eFormat = NV_ENC_BUFFER_FORMAT_IYUV;
    int iGpu = 0;
    uint32_t nFrame = 0;
    try
    {
        using NvEncCudaPtr = std::unique_ptr<NvEncoderCuda, std::function<void(NvEncoderCuda*)>>;
        auto EncodeDeleteFunc = [](NvEncoderCuda *pEnc)
        {
            if (pEnc)
            {
                pEnc->DestroyEncoder();
                delete pEnc;
            }
        };

        NvEncoderInitParam encodeCLIOptions;
        ParseCommandLine(argc, argv, szInFilePath, nWidth, nHeight, eFormat, szOutFilePath, encodeCLIOptions, iGpu, nFrame);

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
        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));

        NvEncCudaPtr pEnc(new NvEncoderCuda(cuContext, nWidth, nHeight, eFormat, 0, true), EncodeDeleteFunc);

        NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
        NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
        initializeParams.encodeConfig = &encodeConfig;
        pEnc->CreateDefaultEncoderParams(&initializeParams, encodeCLIOptions.GetEncodeGUID(), encodeCLIOptions.GetPresetGUID());
        encodeCLIOptions.SetInitParams(&initializeParams, eFormat);

        pEnc->CreateEncoder(&initializeParams);

        MotionEstimationWithBufferedFile(pEnc.get(), nWidth, nHeight, &encodeCLIOptions, szInFilePath, szOutFilePath, nFrame);
    }
    catch (const std::exception &ex)
    {
        std::cout << ex.what();
        exit(1);
    }
    return 0;
}
