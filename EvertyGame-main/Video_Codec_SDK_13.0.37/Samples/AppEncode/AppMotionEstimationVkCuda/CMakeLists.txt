# Copyright 2020-2024 NVIDIA Corporation.  All rights reserved.
#
# Please refer to the NVIDIA end user license agreement (EULA) associated
# with this source code for terms and conditions that govern your use of
# this software. Any use, reproduction, disclosure, or distribution of
# this software and related documentation outside the terms of the EULA
# is strictly prohibited.

# 3.7 is required for FindVulkan module support in CMake.
cmake_minimum_required(VERSION 3.7)

project(AppMotionEstimationVkCuda)

message("VULKAN_PATH: ${VULKAN_PATH}")
message("VULKAN_SDK: $ENV{VULKAN_SDK}")
message("VULKAN_LIB_DIR: ${VULKAN_LIB_DIR}")
message("VULKAN_HDR_DIR: ${VULKAN_HDR_DIR}")

set(APP_SOURCES
 ${CMAKE_CURRENT_SOURCE_DIR}/AppMotionEstimationVkCuda.cpp
 ${CMAKE_CURRENT_SOURCE_DIR}/NvEnc.cpp
 ${CMAKE_CURRENT_SOURCE_DIR}/utility.cpp
)

set(APP_HDRS
 ${CMAKE_CURRENT_SOURCE_DIR}/NvEnc.h
 ${CMAKE_CURRENT_SOURCE_DIR}/utility.h
)

set(NV_ENC_SOURCES
 ${NV_ENC_DIR}/NvEncoder.cpp
 ${NV_ENC_DIR}/NvEncoderCuda.cpp
)

set(NV_ENC_HDRS
 ${NV_ENC_DIR}/NvEncoder.h
 ${NV_ENC_DIR}/NvEncoderCuda.h
 ${NVCODEC_PUBLIC_INTERFACE_DIR}/nvEncodeAPI.h
)

source_group( "headers" FILES ${NV_ENC_HDRS} ${APP_HDRS} )
source_group( "sources" FILES ${APP_SOURCES} ${NV_ENC_SOURCES} )

# Try modern CUDAToolkit first for CMake 3.17+, fallback to deprecated CUDA module
if(CMAKE_VERSION VERSION_GREATER_EQUAL "3.17")
    find_package(CUDAToolkit QUIET)
    if(CUDAToolkit_FOUND)
        # Map CUDAToolkit variables to legacy CUDA variable names
        set(CUDA_CUDA_LIBRARY CUDA::cuda_driver)
        set(CUDA_TOOLKIT_ROOT_DIR ${CUDAToolkit_TARGET_DIR})
        set(CUDA_INCLUDE_DIRS ${CUDAToolkit_INCLUDE_DIRS})
        set(CUDA_VERSION_MAJOR ${CUDAToolkit_VERSION_MAJOR})
        set(CUDA_VERSION_MINOR ${CUDAToolkit_VERSION_MINOR})
    endif()
endif()
if(NOT CUDAToolkit_FOUND)
    find_package(CUDA)
endif()

set(CUDA_HOST_COMPILER ${CMAKE_CXX_COMPILER})

list(APPEND CUDA_NVCC_FLAGS -I$(VULKAN_SDK)/include)

if(NVCODEC_CUDA_VERSION_13_OR_GREATER)
    set(CUDA_NVCC_FLAGS ${CUDA_NVCC_FLAGS};-gencode arch=compute_75,code=\"sm_75,compute_75\")
else()
    set(CUDA_NVCC_FLAGS ${CUDA_NVCC_FLAGS};-gencode arch=compute_50,code=\"sm_50,compute_50\")
endif()
if ( CMAKE_COMPILER_IS_GNUCC )
    if(NOT "${CUDA_NVCC_FLAGS}" MATCHES "-std=c\\+\\+11" )
        list(APPEND CUDA_NVCC_FLAGS -std=c++11)
    endif()
endif()

add_executable(${PROJECT_NAME} ${APP_SOURCES} ${NV_ENC_SOURCES} ${APP_HDRS} ${NV_ENC_HDRS})

set_target_properties(${PROJECT_NAME} PROPERTIES CUDA_SEPARABLE_COMPILATION ON)

target_include_directories(${PROJECT_NAME} PUBLIC ${CUDA_INCLUDE_DIRS}
 ${NVCODEC_PUBLIC_INTERFACE_DIR}
 ${NVCODEC_UTILS_DIR}
 ${NV_CODEC_DIR}
 ${VULKAN_HDR_DIR}
)

# Add any required preprocessor definitions here

if(${CMAKE_SYSTEM_NAME} MATCHES "Windows")
    # Include Vulkan header files from Vulkan SDK
    set(VULKAN_LIBS ${VULKAN_LIB_DIR}/vulkan-1.lib)
    add_definitions(-DVK_USE_PLATFORM_WIN32_KHR)
    include_directories(AFTER ${VULKAN_PATH}/Include)
    # Link directory for vulkan-1
    link_directories(${VULKAN_PATH}/Bin;${VULKAN_PATH}/Lib;)
elseif(${CMAKE_SYSTEM_NAME} MATCHES "Linux")
    add_definitions(-DVK_USE_PLATFORM_XCB_KHR)
    link_libraries(${XCB_LIBRARIES})
endif()

# Only enable validation layers in debug builds of the application
target_compile_options(${PROJECT_NAME} PUBLIC "$<$<CONFIG:DEBUG>:-DUSE_VALIDATION_LAYERS>")

target_link_libraries(${PROJECT_NAME} ${CUDA_CUDA_LIBRARY} ${CMAKE_DL_LIBS} ${NVENCODEAPI_LIB} ${CUVID_LIB} ${VULKAN_LIBS} )

install(TARGETS ${PROJECT_NAME} RUNTIME DESTINATION ${NVCODEC_SAMPLES_INSTALL_DIR})
if (MSVC)
    add_definitions(-D_CRT_SECURE_NO_WARNINGS)
    set_target_properties( ${PROJECT_NAME} PROPERTIES RUNTIME_OUTPUT_DIRECTORY ${NVCODEC_SAMPLES_INSTALL_DIR}/$<CONFIG>/ )
    install(FILES $<TARGET_PDB_FILE:${PROJECT_NAME}> DESTINATION ${NVCODEC_SAMPLES_INSTALL_DIR} CONFIGURATIONS Debug)
endif()

