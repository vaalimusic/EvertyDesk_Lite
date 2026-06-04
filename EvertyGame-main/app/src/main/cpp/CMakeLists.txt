cmake_minimum_required(VERSION 3.22.1)

project(evertysender)

add_library(
    evertysender
    SHARED
    NativeUdpSender.cpp
    NativeVideoDecoderBridge.cpp
)

find_library(log-lib log)
find_library(android-lib android)
find_library(mediandk-lib mediandk)

target_link_libraries(
    evertysender
    ${log-lib}
    ${android-lib}
    ${mediandk-lib}
)
