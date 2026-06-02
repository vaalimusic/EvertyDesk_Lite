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
//! \file AppDecMultiFiles.cpp
//! \brief Source file for AppDecMultiFiles sample
//!
//!  This sample application illustrates the decoding of multiple files with/without using the decoder Reconfigure API.
//!  It also displays the time taken for decoder creation and destruction.
//!  Multiple files are specified using '-filelist' commandline option.
//!  The app will decode files in a sequential manner.
//---------------------------------------------------------------------------

#include <iostream>
#include <algorithm>
#include <thread>
#include <cuda.h>
#include <deque>
#include <fstream>
#include <sstream>
#include <string>
#include "NvDecoder/NvDecoder.h"
#include "../Utils/NvCodecUtils.h"
#include "../Utils/FFmpegDemuxer.h"
#include "../Common/AppDecUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();

typedef struct
{
    char inFile[256];
    char outFile[256];
    Dim resizeDim;
    Rect cropRect;
    bool outplanar;
} FILEINFO;

void ConvertToPlanar(uint8_t *pHostFrame, int nWidth, int nHeight, int nBitDepth, uint8_t nChromaFormat) {
    if (nBitDepth == 8) {
        YuvConverter<uint8_t> converter8(nWidth, nHeight, nChromaFormat);
        converter8.UVInterleavedToPlanar(pHostFrame);
    } else {
        YuvConverter<uint16_t> converter16(nWidth, nHeight, nChromaFormat);
        converter16.UVInterleavedToPlanar((uint16_t *)pHostFrame);
    }
}

void getMaxWidthandMaxHeight(int &maxWidth, int &maxHeight, cudaVideoCodec aeCodec, int anBitDepthMinus8)
{
    CUVIDDECODECAPS decodeCaps = {};
    decodeCaps.eCodecType = aeCodec;
    decodeCaps.eChromaFormat = cudaVideoChromaFormat_420;
    decodeCaps.nBitDepthMinus8 = anBitDepthMinus8;

    cuvidGetDecoderCaps(&decodeCaps);

    maxWidth = decodeCaps.nMaxWidth;
    maxHeight = decodeCaps.nMaxHeight;

}

/**
*   @brief  Function to decode media file and write raw frames into another file.
*   @param  cuContext      - Handle to CUDA context
*   @param  pDec           - Handle to NvDecoder
*   @param  fileData       - FILEINFO variable which holds various file details
*   @param  useReconfigure - Flag to enable/disable reconfigure api for decoding multiple files
*   @param  maxWidth       - The maximum width that the decoder session can be reconfigured to.
*                            The widths of all the input streams in the files specified by the -filelist option,
*                            must not exceed this value. This value defaults to 0.
*   @param  maxHeight      - The maximum height that the decoder session can be reconfigured to.
*                            The heights of all the input streams in the files specified by the -filelist option,
*                            must not exceed this value. This value defaults to 0.
*/
void DecodeMediaFile(CUcontext cuContext, NvDecoder **pDec, FILEINFO fileData, int useReconfigure, int maxWidth = 0, int maxHeight = 0)
{
    std::ofstream fpOut(fileData.outFile, std::ios::out | std::ios::binary);
    if (!fpOut)
    {
        std::ostringstream err;
        err << "Unable to open output file: " << fileData.outFile << std::endl;
        throw std::invalid_argument(err.str());
    }

    FFmpegDemuxer demuxer(fileData.inFile);
    NvDecoder *dec = *pDec;

    if (useReconfigure)
    {
        if (dec == NULL)
        {
            if ((!maxWidth) || (!maxHeight))
            {
                // Get MaxWidth/MaxHeight for particular codec and bitdepth if not set in commandline
                getMaxWidthandMaxHeight(maxWidth, maxHeight, FFmpeg2NvCodecId(demuxer.GetVideoCodec()), demuxer.GetBitDepth() - 8);
            }

            dec = new NvDecoder(cuContext, false, FFmpeg2NvCodecId(demuxer.GetVideoCodec()),
                false, false, &fileData.cropRect, &fileData.resizeDim, false, maxWidth, maxHeight);
            *pDec = dec;
        }
        else
        {
            dec->setReconfigParams(&fileData.cropRect, &fileData.resizeDim);
        }

    }
    else
    {
        dec = new NvDecoder(cuContext, false, FFmpeg2NvCodecId(demuxer.GetVideoCodec()),
            false, false, &fileData.cropRect, &fileData.resizeDim);
    }

    int nVideoBytes = 0, nFrameReturned = 0, nFrame = 0;
    uint8_t *pVideo = NULL, *pFrame;
    bool bDecodeOutSemiPlanar = false;
    do {
        demuxer.Demux(&pVideo, &nVideoBytes);
        nFrameReturned = dec->Decode(pVideo, nVideoBytes);
        if (!nFrame && nFrameReturned)
            LOG(INFO) << dec->GetVideoInfo();

        bDecodeOutSemiPlanar = (dec->GetOutputFormat() == cudaVideoSurfaceFormat_NV12) || (dec->GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                               || (dec->GetOutputFormat() == cudaVideoSurfaceFormat_NV16) || (dec->GetOutputFormat() == cudaVideoSurfaceFormat_P216);
        for (int i = 0; i < nFrameReturned; i++) {
            pFrame = dec->GetFrame();
            if (fileData.outplanar && bDecodeOutSemiPlanar) {
                ConvertToPlanar(pFrame, dec->GetWidth(), dec->GetHeight(), dec->GetBitDepth(), dec->GetOutputChromaFormat());
            }
            fpOut.write(reinterpret_cast<char*>(pFrame), dec->GetFrameSize());
        }
        nFrame += nFrameReturned;
    } while (nVideoBytes);

    std::vector<std::string> aszDecodeOutFormat = { "NV12", "P016", "YUV444", "YUV444P16", "NV16", "P216" };
    if (fileData.outplanar) {
        aszDecodeOutFormat[0] = "iyuv";      aszDecodeOutFormat[1] = "yuv420p16";
        aszDecodeOutFormat[4] = "yuv422p";   aszDecodeOutFormat[5] = "yuv422p16";
    }
    std::cout << "Total frame decoded: " << nFrame << std::endl
            << "Saved in file " << fileData.outFile << " in "
            << aszDecodeOutFormat[dec->GetOutputFormat()]
            << " format" << std::endl;
    if (!useReconfigure)
    {
        delete dec;
        dec = NULL;
    }
    fpOut.close();
}

static void ShowBriefHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Multi-File Sample Application\n";
    oss << "============================================\n\n";
    
    oss << "Usage: AppDecMultiFiles -filelist <list.txt> [options]\n\n";

    // Brief table of core arguments
    oss << "Common Arguments:\n";
    oss << std::left << std::setw(25) << "Argument" 
        << std::setw(12) << "Type"
        << "Default Value\n";
    oss << std::string(50, '-') << "\n";

    oss << std::left << std::setw(25) << "-filelist <path>" 
        << std::setw(12) << "Required"
        << "N/A\n";

    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << "0\n";

    oss << std::left << std::setw(25) << "-usereconfigure <n>" 
        << std::setw(12) << "Optional"
        << "1\n";

    oss << std::left << std::setw(25) << "-maxwidth <n>" 
        << std::setw(12) << "Optional"
        << "0\n";

    oss << std::left << std::setw(25) << "-maxheight <n>" 
        << std::setw(12) << "Optional"
        << "0\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view decode capabilities, use -dc/--decode-caps\n";
    std::cout << oss.str();
    exit(0);
}

static void ShowDetailedHelp()
{
    std::ostringstream oss;
    oss << "NVIDIA Video Decoder Multi-File Sample Application - Detailed Help\n";
    oss << "========================================================\n\n";
    
    oss << "Usage: AppDecMultiFiles -filelist <list.txt> [options]\n\n";

    // Full table of all arguments
    oss << "All Arguments:\n";
    oss << std::left << std::setw(25) << "Argument" 
        << std::setw(12) << "Type"
        << std::setw(20) << "Default Value"
        << "Usage\n";
    oss << std::string(80, '-') << "\n";

    // Required arguments
    oss << std::left << std::setw(25) << "-filelist <path>" 
        << std::setw(12) << "Required"
        << std::setw(20) << "N/A"
        << "-filelist list.txt\n";

    // Optional arguments
    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";

    oss << std::left << std::setw(25) << "-usereconfigure <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "1"
        << "-usereconfigure 0\n";

    oss << std::left << std::setw(25) << "-maxwidth <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-maxwidth 1920\n";

    oss << std::left << std::setw(25) << "-maxheight <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-maxheight 1080\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-filelist" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-usereconfigure" << ": Use decoder reconfigure API\n";
    oss << std::left << std::setw(25) << "-maxwidth" << ": Maximum width for reconfigure\n";
    oss << std::left << std::setw(25) << "-maxheight" << ": Maximum height for reconfigure\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print usage information for common commandline options\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print usage information for common and advanced commandline options\n";
    oss << std::left << std::setw(25) << "-dc/--decode-caps" << ": Print decode capabilities of GPU\n";

    // File list format
    oss << "\nFile List Format:\n";
    oss << "-----------------\n";
    oss << "infile  input1.h264    (Input file path)\n";
    oss << "outfile out1.yuv       (Output file path)\n";
    oss << "outplanar 1            (Convert output to planar format)\n";
    oss << "resize WxH             (Resize to dimension Width x Height)\n";
    oss << "crop l,t,r,b           (Crop rectangle in left,top,right,bottom)\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Reconfigure API is enabled by default for decoding multiple files\n";
    oss << "* Width and height in resize must be even numbers\n";
    oss << "* Crop rectangle dimensions must be even numbers\n";
    oss << "* When using reconfigure, all input files must be same codec\n";
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

void ParseCommandLine(std::deque<FILEINFO> *multiFileData, int &maxWidth, int &maxHeight, int &iGpu, bool &useReconfigure, int argc, char *argv[])
{
    FILEINFO fileData;
    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:" << "\"\n";
        ShowBriefHelp();
    }

    char filelistPath[256];

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
        if (!_stricmp(argv[i], "-filelist")) {
            if (++i == argc) {
                ShowHelpAndExit("-filelist");
            }
            sprintf(filelistPath, "%s", argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-usereconfigure")) {
            if (++i == argc) {
                ShowHelpAndExit("-usereconfigure");
            }
            useReconfigure = atoi(argv[i]) ? true : false;
            continue;
        }
        if (!_stricmp(argv[i], "-maxwidth")) {
            if (++i == argc) {
                ShowHelpAndExit("-maxwidth");
            }
            maxWidth = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-maxheight")) {
            if (++i == argc) {
                ShowHelpAndExit("-maxheight");
            }
            maxHeight = atoi(argv[i]);
            continue;
        }
        ShowHelpAndExit(argv[i]);
    }

    // Parse the input filelist
    std::ifstream filestream(filelistPath);
    std::string line;
    char* str;
    char param[256];
    char value[256];
    int fileIdx = 0;
    while (std::getline(filestream, line))
    {
        str = (char *)line.c_str();
        sscanf(str,"%s %s", param, value);
        if (0 == _stricmp(param, "infile"))
        {
            if (fileIdx > 0)
            {
                multiFileData->push_back(fileData);
            }
            sprintf(fileData.inFile, "%s", value);
            fileIdx++;
            fileData.resizeDim = { 0, 0 };
            fileData.cropRect = { 0, 0, 0, 0 };
            fileData.outplanar = 0;
        }
        else if (0 == _stricmp(param, "outfile"))
        {
            sprintf(fileData.outFile, "%s", value);
        }
        else if (0 == _stricmp(param, "outplanar"))
        {
            fileData.outplanar = atoi(value) ? true : false;
        }
        else if (0 == _stricmp(param, "resize"))
        {
            sscanf(value, "%dx%d", &fileData.resizeDim.w, &fileData.resizeDim.h);
            if (fileData.resizeDim.w % 2 == 1 || fileData.resizeDim.h % 2 == 1) {
                std::cout << "Resizing rect must have width and height of even numbers" << std::endl;
                exit(1);
            }
        }
        else if (0 == _stricmp(param, "crop"))
        {
            sscanf(value, "%d,%d,%d,%d", &fileData.cropRect.l, &fileData.cropRect.t, &fileData.cropRect.r, &fileData.cropRect.b);
            if ((fileData.cropRect.r - fileData.cropRect.l) % 2 == 1 || (fileData.cropRect.b - fileData.cropRect.t) % 2 == 1) {
                std::cout << "Cropping rect must have width and height of even numbers" << std::endl;
                exit(1);
            }
        }
    }
    if (fileIdx > 0)
    {
        multiFileData->push_back(fileData);
    }
}

int main(int argc, char **argv) 
{
    std::deque<FILEINFO> multiFileData;

    int iGpu = 0;
    int maxWidth = 0, maxHeight = 0;
    bool useReconfigure = true;
    FILEINFO fileData;
    try
    {
        ParseCommandLine(&multiFileData, maxWidth, maxHeight, iGpu, useReconfigure, argc, argv);

        ck(cuInit(0));
        int nGpu = 0;
        ck(cuDeviceGetCount(&nGpu));
        if (iGpu < 0 || iGpu >= nGpu) {
            std::cout << "GPU ordinal out of range. Should be within [" << 0 << ", " << nGpu - 1 << "]" << std::endl;
            return 1;
        }

        CUcontext cuContext = NULL;
        createCudaContext(&cuContext, iGpu, 0);

        std::cout << "Decode with demuxing." << std::endl;
        NvDecoder* dec = NULL;

        while (!multiFileData.empty())
        {
            fileData = multiFileData.front();
            multiFileData.pop_front();

            CheckInputFile(fileData.inFile);

            DecodeMediaFile(cuContext, &dec, fileData, useReconfigure, maxWidth, maxHeight);

        }

        if (dec != NULL)
        {
            delete dec;
            dec = NULL;
        }
    }
    catch (const std::exception& ex)
    {
        std::cout << ex.what();
        exit(1);
    }
   
    return 0;
}
