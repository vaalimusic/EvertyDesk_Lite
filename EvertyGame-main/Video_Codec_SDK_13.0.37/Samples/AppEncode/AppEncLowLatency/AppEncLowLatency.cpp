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
*  This sample application demonstrates low latency encoding features and other QOS features
*  like bitrate change and resolution change. The application uses the CUDA interface
*  to demonstrate the above features but can also be used with the D3D or OpenGL interfaces.
*  There are 2 cases of operation demonstrated in this application, controlled by the CLI
*  option "-case". In the first case the application demonstrates bitrate change at runtime
*  without the need to reset the encoder session. The application reduces the bitrate by half
*  and then restores it to the original value after 100 frames.
*  The second case demonstrates dynamic resolution change feature where the application can
*  reduce resolution depending upon bandwidth requirement. In the application, the encode
*  dimensions are reduced by half and restored to the original dimensions after 100 frames.
*/

#include <fstream>
#include <iostream>
#include <cuda.h>
#include <memory>
#include <iomanip>
#include "NvEncoder/NvEncoderCuda.h"
#include "../Utils/Logger.h"
#include "../Utils/NvEncoderCLIOptions.h"
#include "../Utils/NvCodecUtils.h"
#include "../Common/AppEncUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

void ShowEncoderBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Low Latency Encoder Sample Application\n";
    oss << "===========================================\n\n";

    oss << "Usage: AppEncLowLatency -i <input_file> [options]\n\n";

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
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << "0\n";
    oss << std::left << std::setw(25) << "-case <n>"
        << std::setw(12) << "Optional"
        << "1\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view encoder capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

void ShowEncoderDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Low Latency Encoder Sample Application - Detailed Help\n";
    oss << "======================================================\n\n";

    oss << "Usage: AppEncLowLatency -i <input_file> [options]\n\n";

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
        << "-if nv12\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";
    oss << std::left << std::setw(25) << "-case <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "1"
        << "-case 2\n";
    oss << std::left << std::setw(25) << "-frame <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "All frames"
        << "-frame 100\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-o" << ": Output file path\n";
    oss << std::left << std::setw(25) << "-s" << ": Input resolution in WxH format\n";
    oss << std::left << std::setw(25) << "-if" << ": Input format (iyuv/nv12/nv16/p210)\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-case" << ": Test case (1: Bitrate change, 2: Resolution change)\n";
    oss << std::left << std::setw(25) << "-frame" << ": Number of frames to encode\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print basic usage information\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print detailed usage information\n";
    oss << std::left << std::setw(25) << "-ec/--encode-caps" << ": Print encode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* This sample demonstrates low latency encoding features\n";
    oss << "* Case 1: Runtime bitrate change without encoder reset\n";
    oss << "* Case 2: Dynamic resolution change for bandwidth control\n";
    oss << "* Uses infinite GOP length for low latency streaming\n";
    oss << "* Supports CBR rate control with two-pass full resolution\n";
    oss << std::endl;

    // Create a low latency instance to get proper help
    NvEncoderInitParam lowLatencyParam("", nullptr, true);
    oss << lowLatencyParam.GetHelpMessage(false, false, true, true, false, true, false, false) << std::endl;
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

void EncodeLowLatency(CUcontext cuContext, char *szInFilePath, int nWidth, int nHeight, NV_ENC_BUFFER_FORMAT eFormat,
    char *szOutFilePath, NvEncoderInitParam *pEncodeCLIOptions)
{
    std::ifstream fpIn(szInFilePath, std::ifstream::in | std::ifstream::binary);
    if (!fpIn)
    {
        std::ostringstream err;
        err << "Unable to open input file: " << szInFilePath << std::endl;
        throw std::invalid_argument(err.str());
    }

    std::ofstream fpOut(szOutFilePath, std::ios::out | std::ios::binary);
    if (!fpOut)
    {
        std::ostringstream err;
        err << "Unable to open output file: " << szOutFilePath << std::endl;
        throw std::invalid_argument(err.str());
    }

    NvEncoderCuda enc(cuContext, nWidth, nHeight, eFormat, 0);

    NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
    NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
    initializeParams.encodeConfig = &encodeConfig;
    enc.CreateDefaultEncoderParams(&initializeParams, pEncodeCLIOptions->GetEncodeGUID(), pEncodeCLIOptions->GetPresetGUID(),
                pEncodeCLIOptions->GetTuningInfo() == NV_ENC_TUNING_INFO_LOW_LATENCY ? NV_ENC_TUNING_INFO_LOW_LATENCY : NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY);

    encodeConfig.gopLength = NVENC_INFINITE_GOPLENGTH;
    encodeConfig.frameIntervalP = 1;
    if (pEncodeCLIOptions->IsCodecH264())
    {
        encodeConfig.encodeCodecConfig.h264Config.idrPeriod = NVENC_INFINITE_GOPLENGTH;
    }
    else if (pEncodeCLIOptions->IsCodecHEVC())
    {
        encodeConfig.encodeCodecConfig.hevcConfig.idrPeriod = NVENC_INFINITE_GOPLENGTH;
    }
	else
	{
		encodeConfig.encodeCodecConfig.av1Config.idrPeriod = NVENC_INFINITE_GOPLENGTH;
	}

    encodeConfig.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
    encodeConfig.rcParams.multiPass = NV_ENC_TWO_PASS_FULL_RESOLUTION;
    encodeConfig.rcParams.averageBitRate = (static_cast<unsigned int>(5.0f * initializeParams.encodeWidth * initializeParams.encodeHeight) / (1280 * 720)) * 100000;
    encodeConfig.rcParams.vbvBufferSize = (encodeConfig.rcParams.averageBitRate * initializeParams.frameRateDen / initializeParams.frameRateNum) * 5;
    encodeConfig.rcParams.maxBitRate = encodeConfig.rcParams.averageBitRate;
    encodeConfig.rcParams.vbvInitialDelay = encodeConfig.rcParams.vbvBufferSize;

    pEncodeCLIOptions->SetInitParams(&initializeParams, eFormat);

    enc.CreateEncoder(&initializeParams);

    // Params for one frame
    NV_ENC_PIC_PARAMS picParams = {NV_ENC_PIC_PARAMS_VER};
    picParams.encodePicFlags = 0;

    std::streamsize  nRead = 0;
    int nFrameSize = enc.GetFrameSize();
    std::unique_ptr<uint8_t[]> pHostFrame(new uint8_t[nFrameSize]);


    int nFrame = 0, i = 0;
    do
    {
        std::vector<NvEncOutputFrame> vPacket;
        nRead = fpIn.read(reinterpret_cast<char*>(pHostFrame.get()), nFrameSize).gcount();
        if (nRead == nFrameSize)
        {
            const NvEncInputFrame* encoderInputFrame =  enc.GetNextInputFrame();
            NvEncoderCuda::CopyToDeviceFrame(cuContext,
                pHostFrame.get(),
                0,
                (CUdeviceptr)encoderInputFrame->inputPtr,
                (int)encoderInputFrame->pitch,
                enc.GetEncodeWidth(),
                enc.GetEncodeHeight(),
                CU_MEMORYTYPE_HOST,
                encoderInputFrame->bufferFormat,
                encoderInputFrame->chromaOffsets,
                encoderInputFrame->numChromaPlanes);

            if (i && i % 100 == 0)
            {
                // i == 100, 200, 300, 400
                NV_ENC_RECONFIGURE_PARAMS reconfigureParams = {NV_ENC_RECONFIGURE_PARAMS_VER};
                memcpy(&reconfigureParams.reInitEncodeParams, &initializeParams, sizeof(initializeParams));
                NV_ENC_CONFIG reInitCodecConfig = { NV_ENC_CONFIG_VER };
                memcpy(&reInitCodecConfig, initializeParams.encodeConfig, sizeof(reInitCodecConfig));
                reconfigureParams.reInitEncodeParams.encodeConfig = &reInitCodecConfig;
                if (i % 200 != 0)
                {
                    reconfigureParams.reInitEncodeParams.encodeConfig->rcParams.averageBitRate = encodeConfig.rcParams.averageBitRate / 2;
                    reconfigureParams.reInitEncodeParams.encodeConfig->rcParams.vbvBufferSize = reconfigureParams.reInitEncodeParams.encodeConfig->rcParams.averageBitRate *
                        reconfigureParams.reInitEncodeParams.frameRateDen / reconfigureParams.reInitEncodeParams.frameRateNum;
                    reconfigureParams.reInitEncodeParams.encodeConfig->rcParams.vbvInitialDelay = reconfigureParams.reInitEncodeParams.encodeConfig->rcParams.vbvBufferSize;
                }
                enc.Reconfigure(&reconfigureParams);
            }
            enc.EncodeFrame(vPacket, &picParams);
        } else
        {
            enc.EndEncode(vPacket);
        }
        nFrame += (int)vPacket.size();
        for (NvEncOutputFrame &packet : vPacket)
        {
            fpOut.write(reinterpret_cast<char*>(packet.frame.data()), packet.frame.size());
        }
        i++;
    } while (nRead == nFrameSize);

    enc.DestroyEncoder();
    fpOut.close();
    fpIn.close();

    std::cout << "Total frames encoded: " << nFrame << std::endl << "Saved in file " << szOutFilePath << std::endl;
}


void EncodeLowLatencyDRC(CUcontext cuContext, char *szInFilePath, int nWidth, int nHeight, NV_ENC_BUFFER_FORMAT eFormat,
    char *szOutFilePath, NvEncoderInitParam *pEncodeCLIOptions)
{
    CUdeviceptr dpInputYPlane = 0;
    CUdeviceptr dpInputChromaPlane = 0;
    try
    {
        std::ifstream fpIn(szInFilePath, std::ifstream::in | std::ifstream::binary);
        if (!fpIn)
        {
            std::ostringstream err;
            err << "Unable to open input file: " << szInFilePath << std::endl;
            throw std::invalid_argument(err.str());
        }

        std::ofstream fpOut(szOutFilePath, std::ios::out | std::ios::binary);
        if (!fpOut)
        {
            std::ostringstream err;
            err << "Unable to open output file: " << szOutFilePath << std::endl;
            throw std::invalid_argument(err.str());
        }

        if ((eFormat != NV_ENC_BUFFER_FORMAT_NV12) && (eFormat != NV_ENC_BUFFER_FORMAT_IYUV))
        {
            std::cout << "Invalid yuv format : " << eFormat << std::endl;
            return;
        }


        NvEncoderCuda enc(cuContext, nWidth, nHeight, eFormat, 0);

        NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
        NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
        initializeParams.encodeConfig = &encodeConfig;
        enc.CreateDefaultEncoderParams(&initializeParams, pEncodeCLIOptions->GetEncodeGUID(), pEncodeCLIOptions->GetPresetGUID(),
                    pEncodeCLIOptions->GetTuningInfo() == NV_ENC_TUNING_INFO_LOW_LATENCY ? NV_ENC_TUNING_INFO_LOW_LATENCY : NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY);

        encodeConfig.gopLength = NVENC_INFINITE_GOPLENGTH;
        encodeConfig.frameIntervalP = 1;
        if (pEncodeCLIOptions->IsCodecH264())
        {
            encodeConfig.encodeCodecConfig.h264Config.idrPeriod = NVENC_INFINITE_GOPLENGTH;
        }
		else if (pEncodeCLIOptions->IsCodecHEVC())
		{
			encodeConfig.encodeCodecConfig.hevcConfig.idrPeriod = NVENC_INFINITE_GOPLENGTH;
		}
		else
		{
			encodeConfig.encodeCodecConfig.av1Config.idrPeriod = NVENC_INFINITE_GOPLENGTH;
		}

        encodeConfig.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        encodeConfig.rcParams.averageBitRate = (static_cast<unsigned int>(5.0f * initializeParams.encodeWidth * initializeParams.encodeHeight) / (1280 * 720)) * 1000000;
        encodeConfig.rcParams.vbvBufferSize = (encodeConfig.rcParams.averageBitRate * initializeParams.frameRateDen / initializeParams.frameRateNum) * 5;
        encodeConfig.rcParams.maxBitRate = encodeConfig.rcParams.averageBitRate;
        encodeConfig.rcParams.vbvInitialDelay = encodeConfig.rcParams.vbvBufferSize;

        pEncodeCLIOptions->SetInitParams(&initializeParams, eFormat);

        enc.CreateEncoder(&initializeParams);

        uint32_t curEncodeWidth = enc.GetEncodeWidth();
        uint32_t curEncodeHeight = enc.GetEncodeHeight();

        // Params for one frame
        NV_ENC_PIC_PARAMS picParams = { NV_ENC_PIC_PARAMS_VER };
        picParams.encodePicFlags = 0;

        std::streamsize  nRead = 0;
        int nFrameSize = enc.GetFrameSize();
        std::unique_ptr<uint8_t[]> pHostFrame(new uint8_t[nFrameSize]);


        size_t inputYPlanePitch = 0;
        size_t inputChromaPlanePitch = 0;

        ck(cuMemAllocPitch((CUdeviceptr *)&dpInputYPlane,
            &inputYPlanePitch,
            NvEncoder::GetWidthInBytes(eFormat, enc.GetEncodeWidth()),
            enc.GetEncodeHeight(), 16));

        bool bSemiplanar = NvEncoder::GetNumChromaPlanes(eFormat) == 1 ? true : false; // uv interleaved

        ck(cuMemAllocPitch((CUdeviceptr *)&dpInputChromaPlane,
            &inputChromaPlanePitch,
            bSemiplanar ? NvEncoder::GetWidthInBytes(eFormat, enc.GetEncodeWidth()) : NvEncoder::GetChromaWidthInBytes(eFormat, enc.GetEncodeWidth()),
            NvEncoder::GetNumChromaPlanes(eFormat) * NvEncoder::GetChromaHeight(eFormat, enc.GetEncodeHeight()), 16));

        std::vector<CUdeviceptr> chromaDevicePtrs;
        chromaDevicePtrs.push_back(dpInputChromaPlane);
        if (NvEncoder::GetNumChromaPlanes(eFormat) == 2)
        {
            chromaDevicePtrs.push_back(dpInputChromaPlane + (inputChromaPlanePitch * NvEncoder::GetChromaHeight(eFormat, enc.GetEncodeHeight())));
        }

        int nFrame = 0, i = 0;
        do
        {
            std::vector<NvEncOutputFrame> vPacket;
            nRead = fpIn.read(reinterpret_cast<char*>(pHostFrame.get()), nFrameSize).gcount();
            if (nRead == nFrameSize)
            {
                const NvEncInputFrame* encoderInputFrame = enc.GetNextInputFrame();
                if (i && i % 100 == 0)
                {
                    NV_ENC_RECONFIGURE_PARAMS reconfigureParams = { NV_ENC_RECONFIGURE_PARAMS_VER };
                    memcpy(&reconfigureParams.reInitEncodeParams, &initializeParams, sizeof(initializeParams));
                    NV_ENC_CONFIG reInitCodecConfig = { NV_ENC_CONFIG_VER };
                    memcpy(&reInitCodecConfig, initializeParams.encodeConfig, sizeof(reInitCodecConfig));
                    reconfigureParams.reInitEncodeParams.encodeConfig = &reInitCodecConfig;
                    if (i % 200 != 0)
                    {
                        // i == 100, 300, ...
                        // downsample the YUV
                        reconfigureParams.reInitEncodeParams.encodeWidth = (initializeParams.encodeWidth + 1) / 2;
                        reconfigureParams.reInitEncodeParams.encodeHeight = (initializeParams.encodeHeight + 1) / 2;
                    }
                    else
                    {
                        // i == 200, 400, ...
                        // restore the original encode dimensions
                        reconfigureParams.reInitEncodeParams.encodeWidth = initializeParams.encodeWidth;
                        reconfigureParams.reInitEncodeParams.encodeHeight = initializeParams.encodeHeight;
                    }
                    reconfigureParams.reInitEncodeParams.darWidth = reconfigureParams.reInitEncodeParams.encodeWidth;
                    reconfigureParams.reInitEncodeParams.darHeight = reconfigureParams.reInitEncodeParams.encodeHeight;
                    reconfigureParams.forceIDR = true;
                    curEncodeWidth = reconfigureParams.reInitEncodeParams.encodeWidth;
                    curEncodeHeight = reconfigureParams.reInitEncodeParams.encodeHeight;
                    enc.Reconfigure(&reconfigureParams);
                }

                if ((curEncodeWidth != initializeParams.encodeWidth) || (curEncodeHeight != initializeParams.encodeHeight))
                {
                    NvEncoderCuda::CopyToDeviceFrame(cuContext,
                        pHostFrame.get(),
                        0,
                        dpInputYPlane,
                        (uint32_t)inputYPlanePitch,
                        initializeParams.encodeWidth,
                        initializeParams.encodeHeight,
                        CU_MEMORYTYPE_HOST,
                        eFormat,
                        &chromaDevicePtrs[0],
                        (uint32_t)inputChromaPlanePitch,
                        (uint32_t)chromaDevicePtrs.size());


                    ScaleYUV420((unsigned char *)encoderInputFrame->inputPtr,
                        (unsigned char *)encoderInputFrame->inputPtr + encoderInputFrame->chromaOffsets[0],
                        (eFormat == NV_ENC_BUFFER_FORMAT_NV12) ? nullptr : (unsigned char *)encoderInputFrame->inputPtr + encoderInputFrame->chromaOffsets[1],
                        (int)encoderInputFrame->pitch,
                        (int)encoderInputFrame->chromaPitch,
                        enc.GetEncodeWidth(),
                        enc.GetEncodeHeight(),
                        (uint8_t*)dpInputYPlane,
                        (unsigned char *)chromaDevicePtrs[0],
                        (eFormat == NV_ENC_BUFFER_FORMAT_NV12) ? nullptr : (unsigned char *)chromaDevicePtrs[1],
                        (int)inputYPlanePitch,
                        (int)inputChromaPlanePitch,
                        initializeParams.encodeWidth,
                        initializeParams.encodeHeight,
                        (eFormat == NV_ENC_BUFFER_FORMAT_NV12) ? true : false);
                }
                else
                {
                    NvEncoderCuda::CopyToDeviceFrame(cuContext,
                        pHostFrame.get(),
                        0,
                        (CUdeviceptr)encoderInputFrame->inputPtr,
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
            nFrame += (int)vPacket.size();
            for (NvEncOutputFrame &packet : vPacket)
            {
                fpOut.write(reinterpret_cast<char*>(packet.frame.data()), packet.frame.size());
            }
            i++;
        } while (nRead == nFrameSize);

        cuMemFree(dpInputYPlane);
        dpInputYPlane = 0;
        cuMemFree(dpInputChromaPlane);
        dpInputChromaPlane = 0;
        enc.DestroyEncoder();
        fpOut.close();
        fpIn.close();

        std::cout << "Total frames encoded: " << nFrame << std::endl << "Saved in file " << szOutFilePath << std::endl;
    }
    catch (const std::exception&)
    {
        cuMemFree(dpInputYPlane);
        cuMemFree(dpInputChromaPlane);
        throw;
    }
}

void ParseCommandLine(int argc, char *argv[], char *szInputFileName, int &nWidth, int &nHeight,
    NV_ENC_BUFFER_FORMAT &eFormat, char *szOutputFileName, NvEncoderInitParam &initParam,
    int &iGpu, int &iCase, int &nFrame)
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

        std::vector<std::string> vszFileFormatName = { "iyuv", "nv12", "nv16", "p210" };

        NV_ENC_BUFFER_FORMAT aFormat[] =
        {
            NV_ENC_BUFFER_FORMAT_IYUV,
            NV_ENC_BUFFER_FORMAT_NV12,
            NV_ENC_BUFFER_FORMAT_NV16,
            NV_ENC_BUFFER_FORMAT_P210,
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
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-case")) {
            if (++i == argc) {
                ShowHelpAndExit("-case");
            }
            iCase = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-frame")) {
            if (++i == argc) {
                ShowHelpAndExit("-frame");
            }
            nFrame = atoi(argv[i]);
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
    initParam = NvEncoderInitParam(oss.str().c_str(), nullptr, true);
}

int main(int argc, char **argv)
{
    char szInFilePath[256] = "",
        szOutFilePath[256] = "";
    int nWidth = 0, nHeight = 0;
    NV_ENC_BUFFER_FORMAT eFormat = NV_ENC_BUFFER_FORMAT_IYUV;
    int iGpu = 0;
    int iCase = 0;
    int nFrame = 0;
    try
    {
        NvEncoderInitParam encodeCLIOptions;
        ParseCommandLine(argc, argv, szInFilePath, nWidth, nHeight, eFormat, szOutFilePath, encodeCLIOptions, iGpu, iCase, nFrame);

        CheckInputFile(szInFilePath);
        ValidateResolution(nWidth, nHeight);

        if (!*szOutFilePath) {
            sprintf(szOutFilePath, encodeCLIOptions.IsCodecH264() ? "out.h264" : encodeCLIOptions.IsCodecHEVC() ? "out.hevc" : "out.av1");
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
        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));

        switch (iCase)
        {
        default:
        case 0:
            std::cout << "low latency encode with bit rate change" << std::endl;
            EncodeLowLatency(cuContext, szInFilePath, nWidth, nHeight, eFormat, szOutFilePath, &encodeCLIOptions);
            break;
        case 1:
            std::cout << "low latency encode with dynamic resolution change" << std::endl;
            EncodeLowLatencyDRC(cuContext, szInFilePath, nWidth, nHeight, eFormat, szOutFilePath, &encodeCLIOptions);
            break;
        }
    }
    catch (const std::exception &e)
    {
        std::cout << e.what();
        exit(1);
    }
    return 0;
}
