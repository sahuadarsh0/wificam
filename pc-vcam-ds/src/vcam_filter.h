#pragma once

#include <atomic>
#include <cstdint>
#include <mutex>
#include <thread>
#include <vector>

#include <windows.h>
#include <objbase.h>
#include <dshow.h>
#include <dvdmedia.h>
#include <ks.h>
#include <ksmedia.h>
#include <wrl/client.h>

#include "framebus_shm.h"

namespace wificam::vcam_ds {

// Must match `pc-core/src/video/vcam_host.rs::VCAM_DS_CLSID_STR`.
// {C8F5A0B2-7D3E-4A29-8F4B-2E5D6C7F8091}
inline constexpr GUID kClsidWifiCamDsFilter = {
    0xc8f5a0b2, 0x7d3e, 0x4a29, {0x8f, 0x4b, 0x2e, 0x5d, 0x6c, 0x7f, 0x80, 0x91}};

inline constexpr wchar_t kClsidString[] = L"{C8F5A0B2-7D3E-4A29-8F4B-2E5D6C7F8091}";
inline constexpr wchar_t kFriendlyName[] = L"WifiCam Virtual Camera (DirectShow)";

extern std::atomic<long> g_dll_objects;

struct DsVideoMode {
    uint32_t width;
    uint32_t height;
    uint32_t fps_num;
    uint32_t fps_den;
};

inline constexpr DsVideoMode kSupportedModes[] = {
    {1920, 1080, 60, 1},
    {1920, 1080, 30, 1},
    {1280, 720, 60, 1},
    {3840, 2160, 30, 1},
};

HRESULT BuildNv12AmMediaType(const DsVideoMode& mode, AM_MEDIA_TYPE* out_mt);
void FreeAmMediaType(AM_MEDIA_TYPE& mt);
HRESULT CopyAmMediaType(const AM_MEDIA_TYPE& src, AM_MEDIA_TYPE* dst);

class WifiCamDsFilter;

class WifiCamDsPin final : public IPin, public IAMStreamConfig, public IKsPropertySet {
public:
    explicit WifiCamDsPin(WifiCamDsFilter* filter);
    ~WifiCamDsPin();

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IPin
    STDMETHODIMP Connect(IPin* receive_pin, const AM_MEDIA_TYPE* mt) override;
    STDMETHODIMP ReceiveConnection(IPin* connector, const AM_MEDIA_TYPE* mt) override;
    STDMETHODIMP Disconnect() override;
    STDMETHODIMP ConnectedTo(IPin** out_pin) override;
    STDMETHODIMP ConnectionMediaType(AM_MEDIA_TYPE* out_mt) override;
    STDMETHODIMP QueryPinInfo(PIN_INFO* out_info) override;
    STDMETHODIMP QueryDirection(PIN_DIRECTION* out_dir) override;
    STDMETHODIMP QueryId(LPWSTR* out_id) override;
    STDMETHODIMP QueryAccept(const AM_MEDIA_TYPE* mt) override;
    STDMETHODIMP EnumMediaTypes(IEnumMediaTypes** out_enum) override;
    STDMETHODIMP QueryInternalConnections(IPin** pins, ULONG* count) override;
    STDMETHODIMP EndOfStream() override;
    STDMETHODIMP BeginFlush() override;
    STDMETHODIMP EndFlush() override;
    STDMETHODIMP NewSegment(REFERENCE_TIME start, REFERENCE_TIME stop, double rate) override;

    // IAMStreamConfig
    STDMETHODIMP SetFormat(AM_MEDIA_TYPE* mt) override;
    STDMETHODIMP GetFormat(AM_MEDIA_TYPE** out_mt) override;
    STDMETHODIMP GetNumberOfCapabilities(int* count, int* size) override;
    STDMETHODIMP GetStreamCaps(int index, AM_MEDIA_TYPE** out_mt, BYTE* scc) override;

    // IKsPropertySet
    STDMETHODIMP Set(
        REFGUID prop_set,
        DWORD prop_id,
        LPVOID instance_data,
        DWORD instance_len,
        LPVOID prop_data,
        DWORD data_len) override;
    STDMETHODIMP Get(
        REFGUID prop_set,
        DWORD prop_id,
        LPVOID instance_data,
        DWORD instance_len,
        LPVOID prop_data,
        DWORD data_len,
        DWORD* bytes_returned) override;
    STDMETHODIMP QuerySupported(REFGUID prop_set, DWORD prop_id, DWORD* support) override;

    HRESULT StartStreaming();
    HRESULT StopStreaming();

private:
    void StreamThreadLoop();
    void FillSlateNv12(uint8_t* dst, uint32_t width, uint32_t height) const;

    std::atomic<ULONG> ref_count_{1};
    std::mutex lock_;
    WifiCamDsFilter* filter_ = nullptr; // non-owning back-pointer; AddRef'd via QueryPinInfo
    DsVideoMode current_mode_{1920, 1080, 60, 1};
    AM_MEDIA_TYPE connected_mt_{};
    bool has_connected_mt_ = false;

    Microsoft::WRL::ComPtr<IPin> peer_;
    Microsoft::WRL::ComPtr<IMemInputPin> mem_input_;
    Microsoft::WRL::ComPtr<IMemAllocator> allocator_;

    std::atomic<bool> streaming_{false};
    std::thread worker_;
};

class WifiCamDsFilter final : public IBaseFilter {
public:
    static HRESULT CreateInstance(REFIID riid, void** ppv);

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IPersist
    STDMETHODIMP GetClassID(CLSID* out_clsid) override;

    // IMediaFilter
    STDMETHODIMP Stop() override;
    STDMETHODIMP Pause() override;
    STDMETHODIMP Run(REFERENCE_TIME start) override;
    STDMETHODIMP GetState(DWORD timeout_ms, FILTER_STATE* out_state) override;
    STDMETHODIMP SetSyncSource(IReferenceClock* clock) override;
    STDMETHODIMP GetSyncSource(IReferenceClock** out_clock) override;

    // IBaseFilter
    STDMETHODIMP EnumPins(IEnumPins** out_enum) override;
    STDMETHODIMP FindPin(LPCWSTR id, IPin** out_pin) override;
    STDMETHODIMP QueryFilterInfo(FILTER_INFO* out_info) override;
    STDMETHODIMP JoinFilterGraph(IFilterGraph* graph, LPCWSTR name) override;
    STDMETHODIMP QueryVendorInfo(LPWSTR* out_info) override;

    WifiCamDsPin* Pin() const { return pin_.Get(); }

private:
    WifiCamDsFilter();
    ~WifiCamDsFilter();

    std::atomic<ULONG> ref_count_{1};
    std::mutex lock_;
    FILTER_STATE state_ = State_Stopped;
    IFilterGraph* graph_ = nullptr;
    wchar_t name_[128]{};
    Microsoft::WRL::ComPtr<IReferenceClock> clock_;
    Microsoft::WRL::ComPtr<WifiCamDsPin> pin_;
};

} // namespace wificam::vcam_ds
