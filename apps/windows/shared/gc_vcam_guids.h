// The identifiers of gc_vcam_protocol.h as GUID values, for C++.
#pragma once

#include <guiddef.h>

// {1E1EB430-08A7-42F4-9F7D-C53A00B36024}: the class Windows creates in the camera service.
inline constexpr GUID CLSID_GodsCameraSource = {
    0x1E1EB430, 0x08A7, 0x42F4, {0x9F, 0x7D, 0xC5, 0x3A, 0x00, 0xB3, 0x60, 0x24}};

// {94A0A41C-B68F-4E28-98AA-B471A72D5025}: string, the pipe to fetch pictures from.
inline constexpr GUID GC_VCAM_ATTRIBUTE_PIPE_NAME = {
    0x94A0A41C, 0xB68F, 0x4E28, {0x98, 0xAA, 0xB4, 0x71, 0xA7, 0x2D, 0x50, 0x25}};

// {86C72A67-9AC2-4973-ACE5-EA51761B53D6}: UINT64 on a sample, the frame's sequence number.
inline constexpr GUID GC_VCAM_SAMPLE_SEQUENCE = {
    0x86C72A67, 0x9AC2, 0x4973, {0xAC, 0xE5, 0xEA, 0x51, 0x76, 0x1B, 0x53, 0xD6}};

// {57360040-62F0-4AB0-BC1C-B33E44317EF5}: UINT32 on a sample, one of GC_VCAM_SAMPLE_*.
inline constexpr GUID GC_VCAM_SAMPLE_KIND = {
    0x57360040, 0x62F0, 0x4AB0, {0xBC, 0x1C, 0xB3, 0x3E, 0x44, 0x31, 0x7E, 0xF5}};
