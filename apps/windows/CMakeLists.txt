# God's Camera on Windows: the virtual camera and the tools that test it.
#
# Built with the Visual Studio Build Tools and Ninja:
#
#   powershell -File tools\build-windows.ps1
#
# which finds the compiler and puts everything into apps/windows/build/bin.

cmake_minimum_required(VERSION 3.24)
project(GodsCameraWindows LANGUAGES CXX)

if(NOT MSVC)
    message(FATAL_ERROR "This part of God's Camera is built with the Microsoft compiler.")
endif()

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# The C++ runtime is linked into each binary. The media source is loaded into
# Windows' camera service, where nothing but Windows itself may be needed.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")

set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin")
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin")
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")

# Every warning is an error. /guard:cf and /sdl are cheap protections that a
# DLL living inside a system service should have.
add_compile_options(/W4 /WX /permissive- /utf-8 /sdl /guard:cf)
add_link_options(/WX /guard:cf)
add_compile_definitions(UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)

set(GC_SHARED "${CMAKE_CURRENT_SOURCE_DIR}/shared")

# The app's side of the camera's pipe.
add_library(gc_vcam_client STATIC vcam-client/client.cpp)
target_include_directories(gc_vcam_client PUBLIC vcam-client/include "${GC_SHARED}")

# The media source Windows loads into its camera service.
add_library(GodsCameraSource SHARED
    vcam/activate.cpp
    vcam/dllmain.cpp
    vcam/frame_feed.cpp
    vcam/source.cpp
    vcam/stream.cpp
    vcam/vcam.def)
target_include_directories(GodsCameraSource PRIVATE vcam "${GC_SHARED}")
target_precompile_headers(GodsCameraSource PRIVATE vcam/pch.h)
target_link_libraries(GodsCameraSource PRIVATE mfplat mfsensorgroup ole32 advapi32)

# Stands in for the app: makes the pipe and the camera, sends a test picture.
add_executable(gc-vcam-host vcam-host/main.cpp)
target_include_directories(gc-vcam-host PRIVATE "${GC_SHARED}")
target_link_libraries(gc-vcam-host PRIVATE gc_vcam_client mfplat mfsensorgroup ole32 advapi32)

# Reads the camera as a meeting app would and reports what it got.
add_executable(gc-vcam-probe vcam-probe/main.cpp vcam-probe/drive.cpp)
target_include_directories(gc-vcam-probe PRIVATE "${GC_SHARED}")
target_link_libraries(gc-vcam-probe PRIVATE mfplat mf mfreadwrite mfuuid ole32)
