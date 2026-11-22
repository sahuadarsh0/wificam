cmake_minimum_required(VERSION 3.25)
project(pc_vcam_ds LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_library(pc-vcam-ds SHARED
    src/dllmain.cpp
    src/vcam_filter.cpp
    exports.def
)

target_include_directories(pc-vcam-ds PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/../pc-vcam-common/include
)

target_compile_definitions(pc-vcam-ds PRIVATE
    UNICODE
    _UNICODE
    WIN32_LEAN_AND_MEAN
    NOMINMAX
)

if(MSVC)
    target_compile_options(pc-vcam-ds PRIVATE /W4 /WX /EHsc)
endif()

target_link_libraries(pc-vcam-ds PRIVATE
    strmiids
    ole32
    oleaut32
    advapi32
    winmm
)
