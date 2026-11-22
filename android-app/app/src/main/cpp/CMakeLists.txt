cmake_minimum_required(VERSION 3.22.1)
project(wificam_audio LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(oboe REQUIRED CONFIG)

add_library(wificam_audio SHARED
    oboe_audio_jni.cpp
)

target_link_libraries(wificam_audio
    oboe::oboe
    log
)
