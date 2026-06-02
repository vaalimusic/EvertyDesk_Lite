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
//! \file AppDecImageProvider.cpp
//! \brief Source file for AppDecImageProvider sample
//!
//! This sample application illustrates the decoding of a media file in a desired color format.
//! The application supports native (nv12 or p016), bgra, bgrp and bgra64 output formats.
//---------------------------------------------------------------------------

#include <cuda.h>
#include <iostream>
#include <algorithm>
#include <memory>
#include "NvDecoder/NvDecoder.h"
#include "../Utils/FFmpegDemuxer.h"
#include "../Utils/ColorSpace.h"
#include "../Common/AppDecUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

/**
*   @brief  Function to copy image data from CUDA device pointer to host buffer
*   @param  dpSrc   - CUDA device pointer which holds decoded raw frame
*   @param  pDst    - Pointer to host buffer which acts as the destination for the copy
*   @param  nWidth  - Width of decoded frame
*   @param  nHeight - Height of decoded frame
*/
void GetImage(CUdeviceptr dpSrc, uint8_t *pDst, int nWidth, int nHeight)
{
    CUDA_MEMCPY2D m = { 0 };
    m.WidthInBytes = nWidth;
    m.Height = nHeight;
    m.srcMemoryType = CU_MEMORYTYPE_DEVICE;
    m.srcDevice = (CUdeviceptr)dpSrc;
    m.srcPitch = m.WidthInBytes;
    m.dstMemoryType = CU_MEMORYTYPE_HOST;
    m.dstDevice = (CUdeviceptr)(m.dstHost = pDst);
    m.dstPitch = m.WidthInBytes;
    cuMemcpy2D(&m);
}

enum OutputFormat
{
    native = 0, bgrp, rgbp, bgra, rgba, bgra64, rgba64, rgb, bgr
};

std::vector<std::string> vstrOutputFormatName =
{
    "native", "bgrp", "rgbp", "bgra", "rgba", "bgra64", "rgba64", "rgb", "bgr"
};

std::string GetSupportedFormats()
{
    std::ostringstream oss;
    for (auto& v : vstrOutputFormatName)
    {
        oss << " " << v;
    }

    return oss.str();
}

static void ShowBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Image Provider Sample Application\n";
    oss << "================================================\n\n";
    
    oss << "Usage: AppDecImageProvider -i <input_file> [options]\n\n";

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
        << "out.<format>\n";

    oss << std::left << std::setw(25) << "-of <format>" 
        << std::setw(12) << "Optional"
        << "native\n";

    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << "0\n";

    oss << "\nSupported output formats:" << GetSupportedFormats() << "\n";
    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view decode capabilities, use -dc/--decode-caps\n";
    std::cout << oss.str();
    exit(0);
}

static void ShowDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Image Provider Sample Application - Detailed Help\n";
    oss << "=============================================================\n\n";
    
    oss << "Usage: AppDecImageProvider -i <input_file> [options]\n\n";

    // Full table of all arguments
    oss << "All Arguments:\n";
    oss << std::left << std::setw(25) << "Argument" 
        << std::setw(12) << "Type"
        << std::setw(20) << "Default Value"
        << "Description\n";
    oss << std::string(80, '-') << "\n";

    // Required arguments
    oss << std::left << std::setw(25) << "-i <path>" 
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "Input video file path\n";

    // Optional arguments
    oss << std::left << std::setw(25) << "-o <path>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "out.<format>"
        << "Output file path\n";

    oss << std::left << std::setw(25) << "-of <format>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "native"
        << "Output format\n";

    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "GPU device ordinal\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input video file path. Supported formats include H.264, HEVC, MPEG-2, VP8, VP9, AV1\n";
    oss << std::left << std::setw(25) << "-o" << ": Output file path. If not specified, defaults to 'out.<format>'\n";
    oss << std::left << std::setw(25) << "-of" << ": Output format for decoded frames. See supported formats below\n";
    oss << std::left << std::setw(25) << "-gpu" << ": GPU device ordinal for decoding. Must be within available GPU range\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print usage information for common commandline options\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print usage information for common and advanced commandline options\n";
    oss << std::left << std::setw(25) << "-dc/--decode-caps" << ": Print decode capabilities of GPU\n";

    // Output format details
    oss << "\nSupported Output Formats:\n";
    oss << "----------------------\n";
    oss << "native    : Original decoded format (NV12/P016)\n";
    oss << "bgrp     : Planar BGR\n";
    oss << "rgbp     : Planar RGB\n";
    oss << "bgra     : Interleaved BGRA-8bit\n";
    oss << "rgba     : Interleaved RGBA-8bit\n";
    oss << "bgra64   : Interleaved BGRA-16bit\n";
    oss << "rgba64   : Interleaved RGBA-16bit\n";
    oss << "rgb      : Interleaved RGB-8bit\n";
    oss << "bgr      : Interleaved BGR-8bit\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Native format is used by default if -of is not specified\n";
    oss << "* Output filename will be 'out.<format>' if -o is not specified\n";
    oss << "* GPU ordinal must be within available GPU range\n";
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

void ParseCommandLine(int argc, char *argv[], char *szInputFileName,
    OutputFormat &eOutputFormat, char *szOutputFileName, int &iGpu)
{
    std::ostringstream oss;
    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:" << "\"\n";
        ShowBriefHelp();
    }

    int i;
    for (i = 1; i < argc; i++) {
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
        if (!_stricmp(argv[i], "-o")) {
            if (++i == argc) {
                ShowHelpAndExit("-o");
            }
            sprintf(szOutputFileName, "%s", argv[i]);
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
        ShowHelpAndExit(argv[i]);
    }
}

int main(int argc, char **argv)
{
    char szInFilePath[256] = "", szOutFilePath[256] = "";
    OutputFormat eOutputFormat = native;
    int iGpu = 0;
    bool bReturn = 1;
    CUdeviceptr pTmpImage = 0;

    try
    {
        ParseCommandLine(argc, argv, szInFilePath, eOutputFormat, szOutFilePath, iGpu);
        CheckInputFile(szInFilePath);

        if (!*szOutFilePath)
        {
            sprintf(szOutFilePath, "out.%s", vstrOutputFormatName[eOutputFormat].c_str());
        }

        std::ofstream fpOut(szOutFilePath, std::ios::out | std::ios::binary);
        if (!fpOut)
        {
            std::ostringstream err;
            err << "Unable to open output file: " << szOutFilePath << std::endl;
            throw std::invalid_argument(err.str());
        }

        ck(cuInit(0));
        int nGpu = 0;
        ck(cuDeviceGetCount(&nGpu));
        if (iGpu < 0 || iGpu >= nGpu)
        {
            std::ostringstream err;
            err << "GPU ordinal out of range. Should be within [" << 0 << ", " << nGpu - 1 << "]";
            throw std::invalid_argument(err.str());
        }

        CUcontext cuContext = NULL;
        createCudaContext(&cuContext, iGpu, 0);

        FFmpegDemuxer demuxer(szInFilePath);
        NvDecoder dec(cuContext, true, FFmpeg2NvCodecId(demuxer.GetVideoCodec()));
        int nWidth = 0, nHeight = 0, nFrameSize = 0;
        int anSize[] = { 0, 3, 3, 4, 4, 8, 8, 3, 3 };
        std::unique_ptr<uint8_t[]> pImage;

        int nVideoBytes = 0, nFrameReturned = 0, nFrame = 0, iMatrix = 0;
        uint8_t *pVideo = nullptr;
        uint8_t *pFrame;

        do {
            demuxer.Demux(&pVideo, &nVideoBytes);
            nFrameReturned = dec.Decode(pVideo, nVideoBytes);
            if (!nFrame && nFrameReturned)
            {
                LOG(INFO) << dec.GetVideoInfo();
                // Get output frame size from decoder
                nWidth = dec.GetWidth(); nHeight = dec.GetHeight();
                nFrameSize = eOutputFormat == native ? dec.GetFrameSize() : nWidth * nHeight * anSize[eOutputFormat];
                std::unique_ptr<uint8_t[]> pTemp(new uint8_t[nFrameSize]);
                pImage = std::move(pTemp);
                cuMemAlloc(&pTmpImage, nWidth * nHeight * anSize[eOutputFormat]);
            }

            for (int i = 0; i < nFrameReturned; i++)
            {
                iMatrix = dec.GetVideoFormatInfo().video_signal_description.matrix_coefficients;
                pFrame = dec.GetFrame();
                if (dec.GetBitDepth() == 8) {
                    switch (eOutputFormat) {
                    case native:
                        GetImage((CUdeviceptr)pFrame, reinterpret_cast<uint8_t*>(pImage.get()), dec.GetWidth(), dec.GetHeight() + (dec.GetChromaHeight() * dec.GetNumChromaPlanes()));
                        break;
                    case bgrp:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColorPlanar<BGRA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColorPlanar<BGRA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColorPlanar<BGRA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), dec.GetWidth(), 3 * dec.GetHeight());
                        break;
                    case rgbp:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColorPlanar<RGBA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColorPlanar<RGBA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColorPlanar<RGBA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), dec.GetWidth(), 3 * dec.GetHeight());
                        break;
                    case bgra:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColor32<BGRA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12) 
                            Nv12ToColor32<BGRA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColor32<BGRA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 4 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case rgba:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColor32<RGBA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColor32<RGBA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColor32<RGBA32>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 4 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case rgb:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColor24<RGB24>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColor24<RGB24>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColor24<RGB24>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 3 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case bgr:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColor24<BGR24>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColor24<BGR24>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColor24<BGR24>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 3 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case bgra64:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColor64<BGRA64>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColor64<BGRA64>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColor64<BGRA64>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 8 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case rgba64:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444)
                            YUV444ToColor64<RGBA64>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12)
                            Nv12ToColor64<RGBA64>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            Nv16ToColor64<RGBA64>(pFrame, dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);                           
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 8 * dec.GetWidth(), dec.GetHeight());
                        break;
                    }
                }
                else
                {
                    switch (eOutputFormat) {
                    case native:
                        GetImage((CUdeviceptr)pFrame, reinterpret_cast<uint8_t*>(pImage.get()), 2 * dec.GetWidth(), dec.GetHeight() + (dec.GetChromaHeight() * dec.GetNumChromaPlanes()));
                        break;
                    case bgrp:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColorPlanar<BGRA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColorPlanar<BGRA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColorPlanar<BGRA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), dec.GetWidth(), 3 * dec.GetHeight());
                        break;
                    case rgbp:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColorPlanar<RGBA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColorPlanar<RGBA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColorPlanar<RGBA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), dec.GetWidth(), 3 * dec.GetHeight());
                        break;
                    case bgra:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColor32<BGRA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColor32<BGRA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColor32<BGRA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 4 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case rgba:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColor32<RGBA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColor32<RGBA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColor32<RGBA32>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 4 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 4 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case rgb:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColor24<RGB24>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColor24<RGB24>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColor24<RGB24>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 3 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case bgr:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColor24<BGR24>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColor24<BGR24>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColor24<BGR24>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 3 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 3 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case bgra64:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColor64<BGRA64>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColor64<BGRA64>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColor64<BGRA64>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 8 * dec.GetWidth(), dec.GetHeight());
                        break;
                    case rgba64:
                        if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_YUV444_16Bit)
                            YUV444P16ToColor64<RGBA64>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else if (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                            P016ToColor64<RGBA64>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        else
                            P216ToColor64<RGBA64>(pFrame, 2 * dec.GetWidth(), (uint8_t*)pTmpImage, 8 * dec.GetWidth(), dec.GetWidth(), dec.GetHeight(), iMatrix);
                        GetImage(pTmpImage, reinterpret_cast<uint8_t*>(pImage.get()), 8 * dec.GetWidth(), dec.GetHeight());
                        break;
                    }
                }

                fpOut.write(reinterpret_cast<char*>(pImage.get()), nFrameSize);
            }
            nFrame += nFrameReturned;
        } while (nVideoBytes);

        if (pTmpImage) {
            cuMemFree(pTmpImage);
        }

        std::cout << "Total frame decoded: " << nFrame << std::endl << "Saved in file " << szOutFilePath << std::endl;
        fpOut.close();
    }
    catch (const std::exception& ex)
    {
        std::cout << ex.what();
        exit(1);
    }
    return 0;
}
