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
//! \file AppDec.cpp
//! \brief Source file for AppDec sample
//!
//! This sample application illustrates the demuxing and decoding of a media file followed by resize and crop of the output frames.
//! The application supports both planar (YUV420P and YUV420P16) and non-planar (NV12 and P016) output formats.
//---------------------------------------------------------------------------

#include <iostream>
#include <algorithm>
#include <thread>
#include <cuda.h>
#include "NvDecoder/NvDecoder.h"
#include "../Utils/NvCodecUtils.h"
#include "../Utils/FFmpegDemuxer.h"
#include "../Common/AppDecUtils.h"

simplelogger::Logger *logger = simplelogger::LoggerFactory::CreateConsoleLogger();


void ConvertSemiplanarToPlanar(uint8_t *pHostFrame, int nWidth, int nHeight, int nBitDepth, uint8_t nChromaFormat) {
    if (nBitDepth == 8) {
        YuvConverter<uint8_t> converter8(nWidth, nHeight, nChromaFormat);
        converter8.UVInterleavedToPlanar(pHostFrame);
    } else {
        YuvConverter<uint16_t> converter16(nWidth, nHeight, nChromaFormat);
        converter16.UVInterleavedToPlanar((uint16_t *)pHostFrame);
    }
}

/**
*   @brief  Function to decode media file and write raw frames into an output file.
*   @param  cuContext     - Handle to CUDA context
*   @param  szInFilePath  - Path to file to be decoded
*   @param  szOutFilePath - Path to output file into which raw frames are stored
*   @param  bOutPlanar    - Flag to indicate whether output needs to be converted to planar format
*   @param  cropRect      - Cropping rectangle coordinates
*   @param  resizeDim     - Resizing dimensions for output
*   @param  opPoint       - Select an operating point of an AV1 scalable bitstream
*   @param  bDispAllLayers - Output all decoded frames of an AV1 scalable bitstream
*   @param  bExtractUserSEIMessage - Output unregistered user SEI messages in display order
    @param  bExtStream    - Flag to indicate whether to create the cuda stream in the App.
*/
void DecodeMediaFile(CUcontext cuContext, const char *szInFilePath, const char *szOutFilePath, bool bOutPlanar,
    const Rect &cropRect, const Dim &resizeDim, const unsigned int opPoint, const bool bDispAllLayers, const bool bExtractUserSEIMessage,
    unsigned int decsurf,bool bExtStream)
{
    std::ofstream fpOut(szOutFilePath, std::ios::out | std::ios::binary);
    if (!fpOut)
    {
        std::ostringstream err;
        err << "Unable to open output file: " << szOutFilePath << std::endl;
        throw std::invalid_argument(err.str());
    }
    CUstream cuStream = NULL;
    
    if(bExtStream)
    {
        ck(cuCtxPushCurrent(cuContext));
        ck(cuStreamCreate(&cuStream, CU_STREAM_DEFAULT));
    }
    
    FFmpegDemuxer demuxer(szInFilePath);
    NvDecoder dec(cuContext, false, FFmpeg2NvCodecId(demuxer.GetVideoCodec()), false, false, &cropRect, &resizeDim, bExtractUserSEIMessage, 0, 0, 1000, false, decsurf, cuStream);

    /* Set operating point for AV1 SVC. It has no impact for other profiles or codecs
     * PFNVIDOPPOINTCALLBACK Callback from video parser will pick operating point set to NvDecoder  */
    dec.SetOperatingPoint(opPoint, bDispAllLayers);

    int nVideoBytes = 0, nFrameReturned = 0, nFrame = 0;
    uint8_t *pVideo = NULL, *pFrame;
    bool bDecodeOutSemiPlanar = false;
    do {
        demuxer.Demux(&pVideo, &nVideoBytes);
        nFrameReturned = dec.Decode(pVideo, nVideoBytes);
        if (!nFrame && nFrameReturned)
            LOG(INFO) << dec.GetVideoInfo();
        
        bDecodeOutSemiPlanar = (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV12) || (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P016)
                               || (dec.GetOutputFormat() == cudaVideoSurfaceFormat_NV16) || (dec.GetOutputFormat() == cudaVideoSurfaceFormat_P216);
        
        for (int i = 0; i < nFrameReturned; i++) {
            pFrame = dec.GetFrame();
            if(bExtStream)
            {   
                // If using external stream App needs to wait for memcpy to complete. 
                ck(cuStreamSynchronize(cuStream));          
            }
            if (bOutPlanar && bDecodeOutSemiPlanar) {
                ConvertSemiplanarToPlanar(pFrame, dec.GetWidth(), dec.GetHeight(), dec.GetBitDepth(), dec.GetOutputChromaFormat());
            }
            // dump YUV to disk
            if (dec.GetWidth() == dec.GetDecodeWidth())
            {
                fpOut.write(reinterpret_cast<char*>(pFrame), dec.GetFrameSize());
            }
            else
            {
                // 4:2:0/4:2:2 output width is 2 byte aligned. If decoded width is odd , luma has 1 pixel padding
                // Remove padding from luma while dumping it to disk
                // dump luma
                for (auto i = 0; i < dec.GetHeight(); i++)
                {
                    fpOut.write(reinterpret_cast<char*>(pFrame), dec.GetDecodeWidth()*dec.GetBPP());
                    pFrame += dec.GetWidth() * dec.GetBPP();
                }
                // dump Chroma
                fpOut.write(reinterpret_cast<char*>(pFrame), dec.GetChromaPlaneSize());
            }
        }
        nFrame += nFrameReturned;
    } while (nVideoBytes);
    
    if(cuStream)
    {
        ck(cuStreamDestroy(cuStream));
        cuStream = NULL;
        CUcontext tempCuContext;
        ck(cuCtxPopCurrent(&tempCuContext));
    }

    std::vector <std::string> aszDecodeOutFormat = { "NV12", "P016", "YUV444", "YUV444P16", "NV16", "P216" };
    if (bOutPlanar) {
        aszDecodeOutFormat[0] = "iyuv";     aszDecodeOutFormat[1] = "yuv420p16";
        aszDecodeOutFormat[4] = "yuv422p";  aszDecodeOutFormat[5] = "yuv422p16";
    }
    std::cout << "Total frame decoded: " << nFrame << std::endl
            << "Saved in file " << szOutFilePath << " in "
            << aszDecodeOutFormat[dec.GetOutputFormat()]
            << " format" << std::endl;
    fpOut.close();
}

void ShowBriefHelp()
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
    oss << std::left << std::setw(25) << "-o <path>" 
        << std::setw(12) << "Optional"
        << "out.native/.planar\n";
    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << "0\n";

    oss << "\nFor detailed help, use -A/--advanced-options\n";
    oss << "To view decode capabilities, use -dc/--decode-caps\n";
    std::cout << oss.str();
    exit(0);
}

void ShowDetailedHelp()
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
    oss << std::left << std::setw(25) << "-o <path>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "out.native/.planar"
        << "-o output.yuv\n";
    oss << std::left << std::setw(25) << "-outplanar" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-outplanar\n";
    oss << std::left << std::setw(25) << "-gpu <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-gpu 1\n";
    oss << std::left << std::setw(25) << "-crop <l,t,r,b>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "No cropping"
        << "-crop 0,0,1920,1080\n";
    oss << std::left << std::setw(25) << "-resize <WxH>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "No resizing"
        << "-resize 1280x720\n";
    oss << std::left << std::setw(25) << "-oppoint <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-oppoint 2\n";
    oss << std::left << std::setw(25) << "-alllayers" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-alllayers\n";
    oss << std::left << std::setw(25) << "-extractUserSEIMessage" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-extractUserSEIMessage\n";
    oss << std::left << std::setw(25) << "-decsurf <n>" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "0"
        << "-decsurf 8\n";
    oss << std::left << std::setw(25) << "-extCudaStrm" 
        << std::setw(12) << "Optional"
        << std::setw(20) << "false"
        << "-extCudaStrm\n";

    // Detailed descriptions
    oss << "\nDetailed Descriptions:\n";
    oss << "-------------------\n";
    oss << std::left << std::setw(25) << "-i" << ": Input file path\n";
    oss << std::left << std::setw(25) << "-o" << ": Output file path (defaults based on -outplanar flag)\n";
    oss << std::left << std::setw(25) << "-outplanar" << ": Convert output to planar format\n";
    oss << std::left << std::setw(25) << "-gpu" << ": Ordinal of GPU to use\n";
    oss << std::left << std::setw(25) << "-crop" << ": Crop rectangle in left,top,right,bottom (ignored for case 0)\n";
    oss << std::left << std::setw(25) << "-resize" << ": Resize to dimension W times H (ignored for case 0)\n";
    oss << std::left << std::setw(25) << "-oppoint" << ": Select an operating point of an AV1 scalable bitstream\n";
    oss << std::left << std::setw(25) << "-alllayers" << ": Output all decoded frames of an AV1 scalable bitstream\n";
    oss << std::left << std::setw(25) << "-extractUserSEIMessage" << ": Output unregistered user SEI messages in display order\n";
    oss << std::left << std::setw(25) << "-decsurf" << ": Allocate n number of decode surfaces at initialization\n";
    oss << std::left << std::setw(25) << "-extCudaStrm" << ": Use external CUDA stream for memory operations\n";
    oss << std::left << std::setw(25) << "-h/--help" << ": Print usage information for common commandline options\n";
    oss << std::left << std::setw(25) << "-A/--advanced-options" << ": Print usage information for common and advanced commandline options\n";
    oss << std::left << std::setw(25) << "-dc/--decode-caps" << ": Print decode capabilities of GPU\n";

    // Important notes
    oss << "\nNotes:\n";
    oss << "------\n";
    oss << "* Width and height for crop and resize must be even numbers\n";
    oss << "* Output format will be out.native unless -outplanar is specified\n";
    oss << std::endl;

    oss << "To view decode capabilities, use -dc/--decode-caps\n";
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

void ParseCommandLine(int argc, char *argv[], char *szInputFileName, char *szOutputFileName,
    bool &bOutPlanar, int &iGpu, Rect &cropRect, Dim &resizeDim, unsigned int &opPoint, bool &bDispAllLayers, bool &bExtractUserSEIMessage, unsigned int &decsurf, bool &bExtStream)
{
    std::ostringstream oss;
    int i;
    bDispAllLayers = false;
    opPoint = 0;
    if (argc == 1) {
        std::cout << "No Arguments provided! Please refer to the following for options:" << "\"\n";
        ShowBriefHelp();
    }

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
        if (!_stricmp(argv[i], "-outplanar")) {
            bOutPlanar = true;
            continue;
        }
        if (!_stricmp(argv[i], "-gpu")) {
            if (++i == argc) {
                ShowHelpAndExit("-gpu");
            }
            iGpu = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-crop")) {
            if (++i == argc || 4 != sscanf(
                    argv[i], "%d,%d,%d,%d",
                    &cropRect.l, &cropRect.t, &cropRect.r, &cropRect.b)) {
                ShowHelpAndExit("-crop");
            }
            if ((cropRect.r - cropRect.l) % 2 == 1 || (cropRect.b - cropRect.t) % 2 == 1) {
                std::cout << "Cropping rect must have width and height of even numbers" << std::endl;
                exit(1);
            }
            continue;
        }
        if (!_stricmp(argv[i], "-resize")) {
            if (++i == argc || 2 != sscanf(argv[i], "%dx%d", &resizeDim.w, &resizeDim.h)) {
                ShowHelpAndExit("-resize");
            }
            if (resizeDim.w % 2 == 1 || resizeDim.h % 2 == 1) {
                std::cout << "Resizing rect must have width and height of even numbers" << std::endl;
                exit(1);
            }
            continue;
        }
        if (!_stricmp(argv[i], "-oppoint")) {
            if (++i == argc ) {
                ShowHelpAndExit("-oppoint");
            }
            opPoint = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-alllayers")) {
            
            bDispAllLayers = true;
            continue;
        }
        if (!_stricmp(argv[i], "-extractUserSEIMessage")) {
            bExtractUserSEIMessage = true;
            continue;
        }
        if (!_stricmp(argv[i], "-decsurf")) {
            if (++i == argc) {
                ShowHelpAndExit("-decsurf");
            }
            decsurf = atoi(argv[i]);
            continue;
        }
        if (!_stricmp(argv[i], "-extCudaStrm")) {
            bExtStream = true;
            continue;
        }
        ShowHelpAndExit(argv[i]);
    }
}

int main(int argc, char **argv) 
{
    char szInFilePath[256] = "", szOutFilePath[256] = "";
    bool bOutPlanar = false;
    int iGpu = 0;
    Rect cropRect = {};
    Dim resizeDim = {};
    unsigned int opPoint = 0;
    bool bDispAllLayers = false;
    bool bExtractUserSEIMessage = false;
    unsigned int decsurf = 0;
    bool bExtStream = false;
    try
    {
        ParseCommandLine(argc, argv, szInFilePath, szOutFilePath, bOutPlanar, iGpu, cropRect, resizeDim, opPoint, bDispAllLayers, bExtractUserSEIMessage, decsurf, bExtStream);
        CheckInputFile(szInFilePath);

        if (!*szOutFilePath) {
            sprintf(szOutFilePath, bOutPlanar ? "out.planar" : "out.native");
        }

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
        DecodeMediaFile(cuContext, szInFilePath, szOutFilePath, bOutPlanar, cropRect, resizeDim, opPoint, bDispAllLayers, bExtractUserSEIMessage, decsurf, bExtStream);
    }
    catch (const std::exception& ex)
    {
        std::cout << ex.what();
        exit(1);
    }

    return 0;
}
