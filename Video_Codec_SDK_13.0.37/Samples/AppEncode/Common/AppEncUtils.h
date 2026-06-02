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

#pragma once
#include <iostream>
#include <iomanip>
#include <sstream>
#include "NvEncoder/NvEncoder.h"
#include "../Utils/NvEncoderCLIOptions.h"
#include "NvEncoder/NvEncoderCuda.h"
#include "../Utils/NvCodecUtils.h"

// Ensure NVCODEC_CUDA_CTX_CREATE is available if not already defined
#ifndef NVCODEC_CUDA_CTX_CREATE
#if CUDA_VERSION >= 13000
    #define NVCODEC_CUDA_CTX_CREATE(pctx, flags, dev) \
        cuCtxCreate_v4(pctx, 0, flags, dev)
#else
    #define NVCODEC_CUDA_CTX_CREATE(pctx, flags, dev) \
        cuCtxCreate_v2(pctx, flags, dev)
#endif
#endif

inline void ShowEncoderCapability()
{
    ck(cuInit(0));
    int nGpu = 0;
    ck(cuDeviceGetCount(&nGpu));
    std::cout << "Encoder Capability" << std::endl << std::endl;
    for (int iGpu = 0; iGpu < nGpu; iGpu++) {
        CUdevice cuDevice = 0;
        ck(cuDeviceGet(&cuDevice, iGpu));
        char szDeviceName[80];
        ck(cuDeviceGetName(szDeviceName, sizeof(szDeviceName), cuDevice));
        CUcontext cuContext = NULL;
        ck(NVCODEC_CUDA_CTX_CREATE(&cuContext, 0, cuDevice));
        NvEncoderCuda enc(cuContext, 1280, 720, NV_ENC_BUFFER_FORMAT_NV12);

        std::cout << "GPU " << iGpu << " - " << szDeviceName << std::endl << std::endl;
        std::cout << "\tH264:\t\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID,
            NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES) ? "yes" : "no") << std::endl <<
            "\tH264_Main10:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID,
            NV_ENC_CAPS_SUPPORT_10BIT_ENCODE) ? "yes" : "no") << std::endl <<
            "\tH264_422:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID,
            NV_ENC_CAPS_SUPPORT_YUV422_ENCODE) ? "yes" : "no") << std::endl <<
            "\tH264_444:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID,
            NV_ENC_CAPS_SUPPORT_YUV444_ENCODE) ? "yes" : "no") << std::endl <<
            "\tH264_ME:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID,
            NV_ENC_CAPS_SUPPORT_MEONLY_MODE) ? "yes" : "no") << std::endl <<
            "\tH264_WxH:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID,
            NV_ENC_CAPS_WIDTH_MAX)) << "*" <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_H264_GUID, NV_ENC_CAPS_HEIGHT_MAX)) << std::endl <<
            "\tHEVC:\t\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES) ? "yes" : "no") << std::endl <<
            "\tHEVC_Main10:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORT_10BIT_ENCODE) ? "yes" : "no") << std::endl <<
            "\tHEVC_Lossless:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORT_LOSSLESS_ENCODE) ? "yes" : "no") << std::endl <<
            "\tHEVC_SAO:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORT_SAO) ? "yes" : "no") << std::endl <<
            "\tHEVC_422:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORT_YUV422_ENCODE) ? "yes" : "no") << std::endl <<
            "\tHEVC_444:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORT_YUV444_ENCODE) ? "yes" : "no") << std::endl <<
            "\tHEVC_ME:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_SUPPORT_MEONLY_MODE) ? "yes" : "no") << std::endl <<
            "\tHEVC_WxH:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID,
            NV_ENC_CAPS_WIDTH_MAX)) << "*" <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_HEVC_GUID, NV_ENC_CAPS_HEIGHT_MAX)) << std::endl <<
            "\tAV1:\t\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_AV1_GUID,
            NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES) ? "yes" : "no") << std::endl <<
            "\tAV1_422:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_AV1_GUID,
            NV_ENC_CAPS_SUPPORT_YUV422_ENCODE) ? "yes" : "no") << std::endl <<
            "\tAV1_444:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_AV1_GUID,
            NV_ENC_CAPS_SUPPORT_YUV444_ENCODE) ? "yes" : "no") << std::endl <<
            "\tAV1_WxH:\t" << "  " <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_AV1_GUID,
            NV_ENC_CAPS_WIDTH_MAX)) << "*" <<
            (enc.GetCapabilityValue(NV_ENC_CODEC_AV1_GUID, NV_ENC_CAPS_HEIGHT_MAX)) << std::endl;

        std::cout << std::endl;

        enc.DestroyEncoder();
        ck(cuCtxDestroy(cuContext));
    }
    exit(0);
}

inline void ShowEncoderD3DBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Encoder AppEncD3D Sample Application\n";
    oss << "============================================\n\n";

    oss << "Usage: AppEncD3D -i <input_file> [options]\n\n";

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
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << "0\n";
    oss << std::left << std::setw(25) << "-nv12"
        << std::setw(12) << "Optional"
        << "false\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view encoder capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

inline void ShowEncoderD3DDetailedHelp(bool bOutputInVidMem = false, bool bIsD3D12Encode = false)
{
    std::ostringstream oss;
    oss << "NVIDIA Video Encoder AppEncD3D Sample Application - Detailed Help\n";
    oss << "=======================================================\n\n";

    oss << "Usage: AppEncD3D -i <input_file> [options]\n\n";

    // Full table of all arguments
    oss << "All Arguments:\n";
    oss << std::left << std::setw(25) << "Argument"
        << std::setw(12) << "Type"
        << std::setw(20) << "Default Value"
        << "Example\n";
    oss << std::string(80, '-') << "\n";

    oss << std::left << std::setw(25) << "-i <path>"
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "-i input.bgra\n";
    oss << std::left << std::setw(25) << "-o <path>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "codec-based"
        << "-o output.h264\n";
    oss << std::left << std::setw(25) << "-s <WxH>"
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "-s 1920x1080\n";
    oss << std::left << std::setw(25) << "-gpu <n>"
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";
    oss << std::left << std::setw(25) << "-nv12"
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-nv12\n";

    if (bOutputInVidMem)
    {
        oss << std::left << std::setw(25) << "-outputInVidMem"
            << std::setw(12) << "Optional"
            << std::setw(20) << "0"
            << "-outputInVidMem 1\n";
    }

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path (must be in BGRA format)\n";
    oss << std::left << std::setw(25) << "-o" << ": Output file path\n";
    oss << std::left << std::setw(25) << "-s" << ": Input resolution in WxH format\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    if (!bIsD3D12Encode)
        oss << std::left << std::setw(25) << "-nv12" << ": Convert to NV12 before encoding\n";
    if (bOutputInVidMem)
        oss << std::left << std::setw(25) << "-outputInVidMem" << ": Enable output in Video Memory (0/1)\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print basic usage information\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print detailed usage information\n";
    oss << std::left << std::setw(25) << "-ec/--encode-caps" << ": Print encode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Input file must be in BGRA format\n";
    oss << "* Width and height must be specified for encoding\n";
    if (bOutputInVidMem)
    {
        oss << "* When outputInVidMem is enabled, CRC of encoded frames will be computed\n";
        oss << "  and dumped to file with suffix '_crc.txt' added to output file\n";
    }
    oss << std::endl;

    oss << NvEncoderInitParam().GetHelpMessage(false, false, true, false, bOutputInVidMem, !bIsD3D12Encode, bIsD3D12Encode, false) << std::endl;
    oss << "\nTo view encode capabilities, use -ec/--encode-caps\n";
    std::cout << oss.str();
    exit(0);
}

inline void ShowHelpAndExit_AppEncD3D(const char *szBadOption = NULL, bool bOutputInVidMem = false, bool bIsD3D12Encode = false)
{
    if (szBadOption)
    {
        std::ostringstream oss;
        oss << "Error parsing \"" << szBadOption << "\"\n";
        oss << "Use -h/--help for basic usage or -A/--advanced-options for detailed information\n";
        throw std::invalid_argument(oss.str());
    }
}

inline void ParseCommandLine_AppEncD3D(int argc, char *argv[], char *szInputFileName, int &nWidth, int &nHeight,
    char *szOutputFileName, NvEncoderInitParam &initParam, int &iGpu, bool &bForceNv12, int *outputInVidMem = NULL, bool bEnableOutputInVidMem = false, bool bIsD3D12Encode = false)
{
    std::ostringstream oss;
    int i;

    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:\n";
        ShowEncoderD3DBriefHelp();
    }

    for (i = 1; i < argc; i++)
    {
        if (!_stricmp(argv[i], "-h") || !_stricmp(argv[i], "--help")) {
            ShowEncoderD3DBriefHelp();
        }
        if (!_stricmp(argv[i], "-A") || !_stricmp(argv[i], "--advanced-options")) {
            ShowEncoderD3DDetailedHelp(bEnableOutputInVidMem, bIsD3D12Encode);
        }
        if (!_stricmp(argv[i], "-ec") || !_stricmp(argv[i], "--encode-caps")) {
            ShowEncoderCapability();
        }
        if (!_stricmp(argv[i], "-i"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit_AppEncD3D("-i", bEnableOutputInVidMem, bIsD3D12Encode);
            }
            sprintf(szInputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-o")) {
            if (++i == argc) {
                ShowHelpAndExit_AppEncD3D("-o", bEnableOutputInVidMem, bIsD3D12Encode);
            }
            sprintf(szOutputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-s")) {
            if (++i == argc || 2 != sscanf(argv[i], "%dx%d", &nWidth, &nHeight)) {
                ShowHelpAndExit_AppEncD3D("-s", bEnableOutputInVidMem, bIsD3D12Encode);
            }
            continue;
        }
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit_AppEncD3D("-gpu", bEnableOutputInVidMem, bIsD3D12Encode);
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-nv12")) {
            bForceNv12 = true;
            continue;
        }
        if (!_stricmp(argv[i], "-outputInVidMem"))
        {
            if (++i == argc)
            {
                ShowHelpAndExit_AppEncD3D("-outputInVidMem", bEnableOutputInVidMem, bIsD3D12Encode);
            }
            if (outputInVidMem != NULL)
            {
                *outputInVidMem = (atoi(argv[i]) != 0) ? 1 : 0;
            }
            continue;
        }

        // Regard as encoder parameter
        if (argv[i][0] != '-') {
            ShowHelpAndExit_AppEncD3D(argv[i], bEnableOutputInVidMem, bIsD3D12Encode);
        }
        oss << argv[i] << " ";
        while (i + 1 < argc && argv[i + 1][0] != '-') {
            oss << argv[++i] << " ";
        }
    }
    initParam = NvEncoderInitParam(oss.str().c_str());
}
