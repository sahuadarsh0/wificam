#pragma once

#include <atomic>
#include <cstdint>
#include <mutex>
#include <vector>

#include <windows.h>
#include <objbase.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <ks.h>
#include <ksmedia.h>
#include <ksproxy.h>
#include <wrl/client.h>

#include "framebus_shm.h"

namespace wificam::vcam_mf {

// Must match `pc-core/src/video/vcam_host.rs::VCAM_MF_CLSID_STR`.
// {B7E4F9A1-6C2D-4F18-9E3A-1D4C5B6E7F80}
inline constexpr GUID kClsidWifiCamMfSource = {
    0xb7e4f9a1, 0x6c2d, 0x4f18, {0x9e, 0x3a, 0x1d, 0x4c, 0x5b, 0x6e, 0x7f, 0x80}};

inline constexpr wchar_t kClsidString[] = L"{B7E4F9A1-6C2D-4F18-9E3A-1D4C5B6E7F80}";
inline constexpr wchar_t kFriendlyName[] = L"WifiCam Virtual Camera";

extern std::atomic<long> g_dll_objects;

class ShmFrameReader {
public:
    ShmFrameReader() = default;
    ~ShmFrameReader();

    ShmFrameReader(const ShmFrameReader&) = delete;
    ShmFrameReader& operator=(const ShmFrameReader&) = delete;

    bool EnsureConnected();
    void Close();
    bool ReadLatest(uint8_t* dst, size_t dst_cap, framebus::FrameSnapshotMeta& out_meta);

private:
    HANDLE mapping_ = nullptr;
    const void* view_ = nullptr;
};

class WifiCamMediaSource;

class WifiCamMediaStream final : public IMFMediaStream {
public:
    static HRESULT CreateInstance(
        WifiCamMediaSource* parent,
        IMFStreamDescriptor* descriptor,
        WifiCamMediaStream** out_stream);

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IMFMediaEventGenerator
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** out_event) override;
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override;
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** out_event) override;
    STDMETHODIMP QueueEvent(
        MediaEventType met,
        REFGUID extended_type,
        HRESULT status,
        const PROPVARIANT* value) override;

    // IMFMediaStream
    STDMETHODIMP GetMediaSource(IMFMediaSource** out_source) override;
    STDMETHODIMP GetStreamDescriptor(IMFStreamDescriptor** out_desc) override;
    STDMETHODIMP RequestSample(IUnknown* token) override;

    // Lifecycle controlled by parent WifiCamMediaSource
    HRESULT Start(const PROPVARIANT* start_pos);
    HRESULT Stop();
    HRESULT Shutdown();

private:
    WifiCamMediaStream(WifiCamMediaSource* parent, IMFStreamDescriptor* descriptor);
    ~WifiCamMediaStream();

    HRESULT Initialize();
    void FillSlateNv12(uint8_t* dst, uint32_t width, uint32_t height) const;

    std::atomic<ULONG> ref_count_{1};
    std::mutex lock_;
    bool is_shutdown_ = false;
    bool is_started_ = false;
    uint32_t width_ = 1920;
    uint32_t height_ = 1080;
    uint32_t fps_num_ = 60;
    uint32_t fps_den_ = 1;
    int64_t sample_index_ = 0;

    Microsoft::WRL::ComPtr<WifiCamMediaSource> parent_;
    Microsoft::WRL::ComPtr<IMFStreamDescriptor> descriptor_;
    Microsoft::WRL::ComPtr<IMFMediaEventQueue> event_queue_;
    ShmFrameReader shm_reader_;
    std::vector<uint8_t> scratch_;
};

class WifiCamMediaSource final : public IMFMediaSourceEx, public IKsControl {
public:
    static HRESULT CreateInstance(REFIID riid, void** ppv);

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IMFMediaEventGenerator
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** out_event) override;
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override;
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** out_event) override;
    STDMETHODIMP QueueEvent(
        MediaEventType met,
        REFGUID extended_type,
        HRESULT status,
        const PROPVARIANT* value) override;

    // IMFMediaSource
    STDMETHODIMP GetCharacteristics(DWORD* characteristics) override;
    STDMETHODIMP CreatePresentationDescriptor(IMFPresentationDescriptor** out_pd) override;
    STDMETHODIMP Start(
        IMFPresentationDescriptor* pd,
        const GUID* time_format,
        const PROPVARIANT* start_pos) override;
    STDMETHODIMP Stop() override;
    STDMETHODIMP Pause() override;
    STDMETHODIMP Shutdown() override;

    // IMFMediaSourceEx
    STDMETHODIMP GetSourceAttributes(IMFAttributes** out_attrs) override;
    STDMETHODIMP GetStreamAttributes(DWORD stream_id, IMFAttributes** out_attrs) override;
    STDMETHODIMP SetD3DManager(IUnknown* manager) override;

    // IKsControl (required by Windows 11 Frame Server for KS property queries)
    STDMETHODIMP KsProperty(
        PKSPROPERTY property,
        ULONG property_length,
        LPVOID property_data,
        ULONG data_length,
        ULONG* bytes_returned) override;
    STDMETHODIMP KsMethod(
        PKSMETHOD method,
        ULONG method_length,
        LPVOID method_data,
        ULONG data_length,
        ULONG* bytes_returned) override;
    STDMETHODIMP KsEvent(
        PKSEVENT event,
        ULONG event_length,
        LPVOID event_data,
        ULONG data_length,
        ULONG* bytes_returned) override;

private:
    WifiCamMediaSource();
    ~WifiCamMediaSource();

    HRESULT Initialize();
    static HRESULT CreateNv12MediaType(
        uint32_t width,
        uint32_t height,
        uint32_t fps_num,
        uint32_t fps_den,
        IMFMediaType** out_type);

    std::atomic<ULONG> ref_count_{1};
    std::mutex lock_;
    bool is_shutdown_ = false;

    Microsoft::WRL::ComPtr<IMFMediaEventQueue> event_queue_;
    Microsoft::WRL::ComPtr<IMFPresentationDescriptor> presentation_desc_;
    Microsoft::WRL::ComPtr<IMFAttributes> source_attrs_;
    Microsoft::WRL::ComPtr<IMFAttributes> stream_attrs_;
    Microsoft::WRL::ComPtr<WifiCamMediaStream> stream_;
};

} // namespace wificam::vcam_mf
