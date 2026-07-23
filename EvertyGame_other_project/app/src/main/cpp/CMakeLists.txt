cmake_minimum_required(VERSION 3.22.1)

project(evertysender)

add_library(
    evertysender
    SHARED
    NativeUdpSender.cpp
)

find_library(log-lib log)

target_link_libraries(
    evertysender
    ${log-lib}
)
