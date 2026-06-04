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
//! \file AppDecUtils.h
//! \brief Header file containing definitions of miscellaneous functions used by Decode samples
//---------------------------------------------------------------------------

#pragma once
#include <sstream>
#include <iostream>

static void ShowDecoderCapability();
static void getOutputFormatNames(unsigned short nOutputFormatMask, char *OutputFormats);
static void createCudaContext(CUcontext* cuContext, int iGpu, unsigned int flags);

static void ShowBriefHelp(char *szOutputFileName, bool *pbVerbose, int *piD3d, bool *pbForce_zero_latency)
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Sample Application\n";
    oss << "====================================\n\n";
    
    oss << "Usage: AppDec -i <input_file> [options]\n\n";

    // Brief table of core arguments
    oss << "Common Arguments:\n";
    oss << std::left << std::setw(25) << "Argument" 
        << std::setw(12) << "Type"
        << "Default Value\n";
    oss << std::string(50, '-') << "\n";

    oss << std::left << std::setw(25) << "-i <path>" 
        << std::setw(12) << "Required"
        << "N/A\n";

    if (szOutputFileName) {
        oss << std::left << std::setw(25) << "-o <path>" 
            << std::setw(12) << "Optional"
            << "out.native/.planar\n";
    }

    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << "0\n";

    if (pbVerbose) {
        oss << std::left << std::setw(25) << "-v" 
            << std::setw(12) << "Optional"
            << "false\n";
    }

    if (piD3d) {
        oss << std::left << std::setw(25) << "-d3d <n>" 
            << std::setw(12) << "Optional"
            << "9\n";
    }

    if (pbForce_zero_latency) {
        oss << std::left << std::setw(25) << "-force_zero_latency" 
            << std::setw(12) << "Optional"
            << "false\n";
    }

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view decode capabilities, use -dc/--decode-caps\n";
    std::cout << oss.str();
    exit(0);
}

static void ShowDetailedHelp(char *szOutputFileName, bool *pbVerbose, int *piD3d, bool *pbForce_zero_latency)
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Sample Application - Detailed Help\n";
    oss << "================================================\n\n";
    
    oss << "Usage: AppDec -i <input_file> [options]\n\n";

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
        << "-i input.h264\n";

    // Optional arguments
    if (szOutputFileName) {
        oss << std::left << std::setw(25) << "-o <path>" 
            << std::setw(12) << "Optional"
            << std::setw(20) << "out.native/.planar"
            << "-o output.yuv\n";
    }

    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";

    if (pbVerbose) {
        oss << std::left << std::setw(25) << "-v" 
            << std::setw(12) << "Optional"
            << std::setw(20) << "false"
            << "-v\n";
    }

    if (piD3d) {
        oss << std::left << std::setw(25) << "-d3d <n>" 
            << std::setw(12) << "Optional"
            << std::setw(20) << "9"
            << "-d3d 11\n";
    }

    if (pbForce_zero_latency) {
        oss << std::left << std::setw(25) << "-force_zero_latency" 
            << std::setw(12) << "Optional"
            << std::setw(20) << "false"
            << "-force_zero_latency\n";
    }

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    if (szOutputFileName) {
        oss << std::left << std::setw(25) << "-o" << ": Output file path\n";
    }
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    if (pbVerbose) {
        oss << std::left << std::setw(25) << "-v" << ": Enable verbose message output\n";
    }
    if (piD3d) {
        oss << std::left << std::setw(25) << "-d3d" << ": DirectX version to use (9 or 11)\n";
    }
    if (pbForce_zero_latency) {
        oss << std::left << std::setw(25) << "-force_zero_latency" << ": Enable zero latency for All-Intra/IPPP streams\n";
    }
    oss << std::left << std::setw(25) << "-h/--help" << ": Print usage information for common commandline options\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print usage information for common and advanced commandline options\n";
    oss << std::left << std::setw(25) << "-dc/--decode-caps" << ": Print decode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    if (pbForce_zero_latency) {
        oss << "* Do not use -force_zero_latency if the stream contains B-frames\n";
    }
    if (piD3d) {
        oss << "* D3D version 9 is used by default\n";
    }
    oss << std::endl;
    oss << "To view decode capabilities, use -dc/--decode-caps\n";

    std::cout << oss.str();
    exit(0);
}

static void ShowHelpAndExit(const char *szBadOption, char *szOutputFileName, bool *pbVerbose, int *piD3d, bool *pbForce_zero_latency)
{
    if (szBadOption) 
    {
        std::ostringstream oss;
        oss << "Error parsing \"" << szBadOption << "\"\n";
        oss << "Use -h/--help for basic usage or -A/--advanced-options for detailed information\n";
        throw std::invalid_argument(oss.str());
    }
}

static void ParseCommandLine(int argc, char *argv[], char *szInputFileName,
    char *szOutputFileName, int &iGpu, bool *pbVerbose = NULL, int *piD3d = NULL,
    bool *pbForce_zero_latency = NULL)
{
    std::ostringstream oss;
    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:" << "\"\n";
        ShowBriefHelp(szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
    }
    int i;
    for (i = 1; i < argc; i++) {
        if (!_stricmp(argv[i], "-h") || !_stricmp(argv[i], "--help")) {
            ShowBriefHelp(szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
        }
        if (!_stricmp(argv[i], "-A") || !_stricmp(argv[i], "--advanced-options")) {
            ShowDetailedHelp(szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
        }
        if (!_stricmp(argv[i], "-dc") || !_stricmp(argv[i], "--decode-caps")) {
            ShowDecoderCapability();
        }
        if (!_stricmp(argv[i], "-i")) {
            if (++i == argc) {
                ShowHelpAndExit("-i", szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
            }
            sprintf(szInputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-o")) {
            if (++i == argc || !szOutputFileName) {
                ShowHelpAndExit("-o", szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
            }
            sprintf(szOutputFileName, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit("-gpu", szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-v")) {
            if (!pbVerbose) {
                ShowHelpAndExit("-v", szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
            }
            *pbVerbose = true;
            continue;
        }
        if (!_stricmp(argv[i], "-d3d")) {
            if (++i == argc || !piD3d) {
                ShowHelpAndExit("-d3d", szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
            }
            *piD3d = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-force_zero_latency")) {
            if (!pbForce_zero_latency) {
                ShowHelpAndExit("-force_zero_latency", szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
            }
            *pbForce_zero_latency = true;
            continue;
        }
        ShowHelpAndExit(argv[i], szOutputFileName, pbVerbose, piD3d, pbForce_zero_latency);
    }
}

/**
*   @brief  Function to generate space-separated list of supported video surface formats
*   @param  nOutputFormatMask - Bit mask to represent supported cudaVideoSurfaceFormat in decoder
*   @param  OutputFormats     - Variable into which output string is written
*/
static void getOutputFormatNames(unsigned short nOutputFormatMask, char *OutputFormats)
{
    if (nOutputFormatMask == 0) {
        strcpy(OutputFormats, "N/A");
        return;
    }

    if (nOutputFormatMask & (1U << cudaVideoSurfaceFormat_NV12)) {
        strcat(OutputFormats, "NV12 ");
    }

    if (nOutputFormatMask & (1U << cudaVideoSurfaceFormat_P016)) {
        strcat(OutputFormats, "P016 ");
    }

    if (nOutputFormatMask & (1U << cudaVideoSurfaceFormat_YUV444)) {
        strcat(OutputFormats, "YUV444 ");
    }

    if (nOutputFormatMask & (1U << cudaVideoSurfaceFormat_YUV444_16Bit)) {
        strcat(OutputFormats, "YUV444P16 ");
    }

    if (nOutputFormatMask & (1U << cudaVideoSurfaceFormat_NV16)) {
        strcat(OutputFormats, "NV16 ");
    }

    if (nOutputFormatMask & (1U << cudaVideoSurfaceFormat_P216)) {
        strcat(OutputFormats, "P216 ");
    }
    return;
}

/**
*   @brief  Utility function to create CUDA context
*   @param  cuContext - Pointer to CUcontext. Updated by this function.
*   @param  iGpu      - Device number to get handle for
*/
static void createCudaContext(CUcontext* cuContext, int iGpu, unsigned int flags)
{
    CUdevice cuDevice = 0;
    ck(cuDeviceGet(&cuDevice, iGpu));
    char szDeviceName[80];
    ck(cuDeviceGetName(szDeviceName, sizeof(szDeviceName), cuDevice));
    std::cout << "GPU in use: " << szDeviceName << std::endl;
    ck(NVCODEC_CUDA_CTX_CREATE(cuContext, flags, cuDevice));
}

/**
*   @brief  Print decoder capabilities on std::cout
*/
static void ShowDecoderCapability()
{
    ck(cuInit(0));
    int nGpu = 0;
    ck(cuDeviceGetCount(&nGpu));
    std::cout << "Decoder Capability" << std::endl << std::endl;
    
    struct caps {
        const char *aszCodecName;
        cudaVideoCodec aeCodec;
        cudaVideoChromaFormat aeChromaFormat;
        int anBitDepthMinus8;
    };
    
    caps queryList[] = {
        {"JPEG",     cudaVideoCodec_JPEG,     cudaVideoChromaFormat_420,          0},
        {"MPEG1",    cudaVideoCodec_MPEG1,    cudaVideoChromaFormat_420,          0},
        {"MPEG2",    cudaVideoCodec_MPEG2,    cudaVideoChromaFormat_420,          0},
        {"MPEG4",    cudaVideoCodec_MPEG4,    cudaVideoChromaFormat_420,          0},
        {"H264",     cudaVideoCodec_H264,     cudaVideoChromaFormat_420,          0},
        {"H264",     cudaVideoCodec_H264,     cudaVideoChromaFormat_420,          2},
        {"H264",     cudaVideoCodec_H264,     cudaVideoChromaFormat_422,          0},
        {"H264",     cudaVideoCodec_H264,     cudaVideoChromaFormat_422,          2},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_420,          0},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_420,          2},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_420,          4},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_422,          0},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_422,          2},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_422,          4},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_444,          0},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_444,          2},
        {"HEVC",     cudaVideoCodec_HEVC,     cudaVideoChromaFormat_444,          4},
        {"VC1",      cudaVideoCodec_VC1,      cudaVideoChromaFormat_420,          0},
        {"VP8",      cudaVideoCodec_VP8,      cudaVideoChromaFormat_420,          0},
        {"VP9",      cudaVideoCodec_VP9,      cudaVideoChromaFormat_420,          0},
        {"VP9",      cudaVideoCodec_VP9,      cudaVideoChromaFormat_420,          2},
        {"VP9",      cudaVideoCodec_VP9,      cudaVideoChromaFormat_420,          4},
        {"AV1",      cudaVideoCodec_AV1,      cudaVideoChromaFormat_420,          0},
        {"AV1",      cudaVideoCodec_AV1,      cudaVideoChromaFormat_420,          2},
        {"AV1",      cudaVideoCodec_AV1,      cudaVideoChromaFormat_Monochrome,   0},
        {"AV1",      cudaVideoCodec_AV1,      cudaVideoChromaFormat_Monochrome,   2},
    };

    const char *aszChromaFormat[] = { "4:0:0", "4:2:0", "4:2:2", "4:4:4" };
    char strOutputFormats[64];

    for (int iGpu = 0; iGpu < nGpu; iGpu++) {

        CUcontext cuContext = NULL;
        createCudaContext(&cuContext, iGpu, 0);

        for (int i = 0; i < sizeof(queryList) / sizeof(queryList[0]); i++) {

            CUVIDDECODECAPS decodeCaps = {};
            decodeCaps.eCodecType = queryList[i].aeCodec;
            decodeCaps.eChromaFormat = queryList[i].aeChromaFormat;
            decodeCaps.nBitDepthMinus8 = queryList[i].anBitDepthMinus8;

            cuvidGetDecoderCaps(&decodeCaps);

            strOutputFormats[0] = '\0';
            getOutputFormatNames(decodeCaps.nOutputFormatMask, strOutputFormats);

            // setw() width = maximum_width_of_string + 2 spaces
            std::cout << "Codec  " << std::left << std::setw(7) << queryList[i].aszCodecName <<
                "BitDepth  " << std::setw(4) << decodeCaps.nBitDepthMinus8 + 8 <<
                "ChromaFormat  " << std::setw(7) << aszChromaFormat[decodeCaps.eChromaFormat] <<
                "Supported  " << std::setw(3) << (int)decodeCaps.bIsSupported <<
                "MaxWidth  " << std::setw(7) << decodeCaps.nMaxWidth <<
                "MaxHeight  " << std::setw(7) << decodeCaps.nMaxHeight <<
                "MaxMBCount  " << std::setw(10) << decodeCaps.nMaxMBCount <<
                "MinWidth  " << std::setw(5) << decodeCaps.nMinWidth <<
                "MinHeight  " << std::setw(5) << decodeCaps.nMinHeight <<
                "SurfaceFormat  " << std::setw(11) << strOutputFormats << std::endl;
        }

        std::cout << std::endl;

        ck(cuCtxDestroy(cuContext));
    }
    exit(0);
}
