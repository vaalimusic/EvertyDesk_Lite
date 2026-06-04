# Copyright 2025 NVIDIA Corporation.  All rights reserved.
#
# Please refer to the NVIDIA end user license agreement (EULA) associated
# with this source code for terms and conditions that govern your use of
# this software. Any use, reproduction, disclosure, or distribution of
# this software and related documentation outside the terms of the EULA
# is strictly prohibited.

# toolchain cmake file for arm64 cross-compilation on x86 ubuntu platform
# Set the target system

set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR aarch64)

# Specify the cross compiler
set(CMAKE_C_COMPILER aarch64-linux-gnu-gcc)
set(CMAKE_CXX_COMPILER aarch64-linux-gnu-g++)

# Set CUDA host compiler to use the cross-compiler
set(CMAKE_CUDA_HOST_COMPILER ${CMAKE_CXX_COMPILER})

# Require CMAKE_SYSROOT to be passed via -DCMAKE_SYSROOT=... command line
if(NOT DEFINED CMAKE_SYSROOT OR CMAKE_SYSROOT STREQUAL "")
    message(FATAL_ERROR 
        "CMAKE_SYSROOT is not set!\n"
        "Please provide it via command line:\n"
        "  cmake -DCMAKE_SYSROOT=/path/to/sysroot \n"
        "        -DCMAKE_TOOLCHAIN_FILE=../aarch64-cc-toolchain.cmake \n"
        "        -DCMAKE_BUILD_TYPE=Release .."
    )
endif()
message(STATUS "Cross-compile config with sysroot: ${CMAKE_SYSROOT}")

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)   # Programs should run with host binaries
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)    # Only look for libraries in the root path
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)    # Only look for headers in the root path

# CUDA target toolkit path for jetson
set(CUDA_TARGET_TOOLKIT "${CMAKE_SYSROOT}/usr/local/cuda-13.0/targets/sbsa-linux")

# Point CMake to target CUDA libraries
list(APPEND CMAKE_LIBRARY_PATH
    "${CUDA_TARGET_TOOLKIT}/lib"
    "${CUDA_TARGET_TOOLKIT}/lib/stubs"
)

# Add CUDA include and library paths for cross-compilation
set(CMAKE_CUDA_FLAGS "${CMAKE_CUDA_FLAGS} -I${CUDA_TARGET_TOOLKIT}/include")
set(CMAKE_CUDA_FLAGS "${CMAKE_CUDA_FLAGS} -Xlinker=-L${CUDA_TARGET_TOOLKIT}/lib")
set(CMAKE_CUDA_FLAGS "${CMAKE_CUDA_FLAGS} -Xlinker=-L${CUDA_TARGET_TOOLKIT}/lib/stubs")

