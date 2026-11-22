#include "vcam_filter.h"

#include <algorithm>
#include <chrono>
#include <cstring>

using Microsoft::WRL::ComPtr;

namespace wificam::vcam_ds {

namespace {

class EnumPinsImpl final : public IEnumPins {
public:
    explicit EnumPinsImpl(IPin* pin, ULONG pos = 0) : pin_(pin), pos_(pos) {
        g_dll_objects.fetch_add(1, std::memory_order_relaxed);
    }
    ~EnumPinsImpl() { g_dll_objects.fetch_sub(1, std::memory_order_relaxed); }

    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (!ppv) return E_POINTER;
        if (riid == IID_IUnknown || riid == IID_IEnumPins) {
            *ppv = static_cast<IEnumPins*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() override { return ref_count_.fetch_add(1) + 1; }
    STDMETHODIMP_(ULONG) Release() override {
        ULONG c = ref_count_.fetch_sub(1) - 1;
        if (c == 0) delete this;
        return c;
    }
    STDMETHODIMP Next(ULONG count, IPin** pins, ULONG* fetched) override {
        if (!pins) return E_POINTER;
        ULONG got = 0;
        if (count > 0 && pos_ == 0 && pin_) {
            pin_.CopyTo(&pins[0]);
            pos_ = 1;
            got = 1;
        }
        if (fetched) *fetched = got;
        return got == count ? S_OK : S_FALSE;
    }
    STDMETHODIMP Skip(ULONG count) override {
        pos_ = std::min<ULONG>(1, pos_ + count);
        return pos_ < 1 ? S_OK : S_FALSE;
    }
    STDMETHODIMP Reset() override {
        pos_ = 0;
        return S_OK;
    }
    STDMETHODIMP Clone(IEnumPins** out_enum) override {
        if (!out_enum) return E_POINTER;
        *out_enum = new (std::nothrow) EnumPinsImpl(pin_.Get(), pos_);
        return *out_enum ? S_OK : E_OUTOFMEMORY;
    }

private:
    std::atomic<ULONG> ref_count_{1};
    ComPtr<IPin> pin_;
    ULONG pos_ = 0;
};

class EnumMediaTypesImpl final : public IEnumMediaTypes {
public:
    explicit EnumMediaTypesImpl(ULONG pos = 0) : pos_(pos) {
        g_dll_objects.fetch_add(1, std::memory_order_relaxed);
    }
    ~EnumMediaTypesImpl() { g_dll_objects.fetch_sub(1, std::memory_order_relaxed); }

    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (!ppv) return E_POINTER;
        if (riid == IID_IUnknown || riid == IID_IEnumMediaTypes) {
            *ppv = static_cast<IEnumMediaTypes*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() override { return ref_count_.fetch_add(1) + 1; }
    STDMETHODIMP_(ULONG) Release() override {
        ULONG c = ref_count_.fetch_sub(1) - 1;
        if (c == 0) delete this;
        return c;
    }
    STDMETHODIMP Next(ULONG count, AM_MEDIA_TYPE** types, ULONG* fetched) override {
        if (!types) return E_POINTER;
        constexpr ULONG total = static_cast<ULONG>(std::size(kSupportedModes));
        ULONG got = 0;
        while (got < count && pos_ < total) {
            auto* mt = static_cast<AM_MEDIA_TYPE*>(CoTaskMemAlloc(sizeof(AM_MEDIA_TYPE)));
            if (!mt) return E_OUTOFMEMORY;
            std::memset(mt, 0, sizeof(AM_MEDIA_TYPE));
            HRESULT hr = BuildNv12AmMediaType(kSupportedModes[pos_], mt);
            if (FAILED(hr)) {
                CoTaskMemFree(mt);
                return hr;
            }
            types[got++] = mt;
            ++pos_;
        }
        if (fetched) *fetched = got;
        return got == count ? S_OK : S_FALSE;
    }
    STDMETHODIMP Skip(ULONG count) override {
        constexpr ULONG total = static_cast<ULONG>(std::size(kSupportedModes));
        pos_ = std::min(total, pos_ + count);
        return pos_ < total ? S_OK : S_FALSE;
    }
    STDMETHODIMP Reset() override {
        pos_ = 0;
        return S_OK;
    }
    STDMETHODIMP Clone(IEnumMediaTypes** out_enum) override {
        if (!out_enum) return E_POINTER;
        *out_enum = new (std::nothrow) EnumMediaTypesImpl(pos_);
        return *out_enum ? S_OK : E_OUTOFMEMORY;
    }

private:
    std::atomic<ULONG> ref_count_{1};
    ULONG pos_ = 0;
};

} // namespace

HRESULT BuildNv12AmMediaType(const DsVideoMode& mode, AM_MEDIA_TYPE* out_mt) {
    if (!out_mt) return E_POINTER;
    std::memset(out_mt, 0, sizeof(AM_MEDIA_TYPE));

    auto* vih = static_cast<VIDEOINFOHEADER*>(CoTaskMemAlloc(sizeof(VIDEOINFOHEADER)));
    if (!vih) return E_OUTOFMEMORY;
    std::memset(vih, 0, sizeof(VIDEOINFOHEADER));

    const DWORD sample_bytes = mode.width * mode.height * 3 / 2;
    vih->AvgTimePerFrame =
        (10'000'000LL * static_cast<LONGLONG>(mode.fps_den)) / std::max<LONGLONG>(1, mode.fps_num);
    vih->dwBitRate = sample_bytes * 8 * mode.fps_num / std::max(1u, mode.fps_den);
    vih->rcSource = {0, 0, static_cast<LONG>(mode.width), static_cast<LONG>(mode.height)};
    vih->rcTarget = vih->rcSource;
    vih->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    vih->bmiHeader.biWidth = static_cast<LONG>(mode.width);
    vih->bmiHeader.biHeight = static_cast<LONG>(mode.height);
    vih->bmiHeader.biPlanes = 1;
    vih->bmiHeader.biBitCount = 12;
    vih->bmiHeader.biCompression = MAKEFOURCC('N', 'V', '1', '2');
    vih->bmiHeader.biSizeImage = sample_bytes;

    out_mt->majortype = MEDIATYPE_Video;
    out_mt->subtype = MEDIASUBTYPE_NV12;
    out_mt->bFixedSizeSamples = TRUE;
    out_mt->bTemporalCompression = FALSE;
    out_mt->lSampleSize = sample_bytes;
    out_mt->formattype = FORMAT_VideoInfo;
    out_mt->cbFormat = sizeof(VIDEOINFOHEADER);
    out_mt->pbFormat = reinterpret_cast<BYTE*>(vih);
    return S_OK;
}

void FreeAmMediaType(AM_MEDIA_TYPE& mt) {
    if (mt.cbFormat != 0 && mt.pbFormat != nullptr) {
        CoTaskMemFree(mt.pbFormat);
        mt.pbFormat = nullptr;
        mt.cbFormat = 0;
    }
    if (mt.pUnk != nullptr) {
        mt.pUnk->Release();
        mt.pUnk = nullptr;
    }
}

HRESULT CopyAmMediaType(const AM_MEDIA_TYPE& src, AM_MEDIA_TYPE* dst) {
    if (!dst) return E_POINTER;
    *dst = src;
    dst->pbFormat = nullptr;
    dst->cbFormat = 0;
    dst->pUnk = nullptr;
    if (src.cbFormat > 0 && src.pbFormat != nullptr) {
        dst->pbFormat = static_cast<BYTE*>(CoTaskMemAlloc(src.cbFormat));
        if (!dst->pbFormat) return E_OUTOFMEMORY;
        std::memcpy(dst->pbFormat, src.pbFormat, src.cbFormat);
        dst->cbFormat = src.cbFormat;
    }
    if (src.pUnk != nullptr) {
        src.pUnk->AddRef();
        dst->pUnk = src.pUnk;
    }
    return S_OK;
}

// ---------------------------------------------------------------------------
// WifiCamDsPin
// ---------------------------------------------------------------------------

WifiCamDsPin::WifiCamDsPin(WifiCamDsFilter* filter) : filter_(filter) {
    g_dll_objects.fetch_add(1, std::memory_order_relaxed);
}

WifiCamDsPin::~WifiCamDsPin() {
    StopStreaming();
    if (has_connected_mt_) {
        FreeAmMediaType(connected_mt_);
    }
    g_dll_objects.fetch_sub(1, std::memory_order_relaxed);
}

STDMETHODIMP WifiCamDsPin::QueryInterface(REFIID riid, void** ppv) {
    if (!ppv) return E_POINTER;
    if (riid == IID_IUnknown || riid == IID_IPin) {
        *ppv = static_cast<IPin*>(this);
        AddRef();
        return S_OK;
    }
    if (riid == IID_IAMStreamConfig) {
        *ppv = static_cast<IAMStreamConfig*>(this);
        AddRef();
        return S_OK;
    }
    if (riid == IID_IKsPropertySet) {
        *ppv = static_cast<IKsPropertySet*>(this);
        AddRef();
        return S_OK;
    }
    *ppv = nullptr;
    return E_NOINTERFACE;
}

STDMETHODIMP_(ULONG) WifiCamDsPin::AddRef() { return ref_count_.fetch_add(1) + 1; }

STDMETHODIMP_(ULONG) WifiCamDsPin::Release() {
    ULONG c = ref_count_.fetch_sub(1) - 1;
    if (c == 0) delete this;
    return c;
}

STDMETHODIMP WifiCamDsPin::Connect(IPin* receive_pin, const AM_MEDIA_TYPE* mt) {
    if (!receive_pin) return E_POINTER;
    std::lock_guard guard(lock_);
    if (peer_) return VFW_E_ALREADY_CONNECTED;

    AM_MEDIA_TYPE proposed{};
    HRESULT hr = mt ? CopyAmMediaType(*mt, &proposed) : BuildNv12AmMediaType(current_mode_, &proposed);
    if (FAILED(hr)) return hr;

    if (FAILED(QueryAccept(&proposed))) {
        FreeAmMediaType(proposed);
        return VFW_E_TYPE_NOT_ACCEPTED;
    }

    ComPtr<IMemInputPin> mem_in;
    hr = receive_pin->QueryInterface(IID_IMemInputPin, reinterpret_cast<void**>(mem_in.GetAddressOf()));
    if (FAILED(hr)) {
        FreeAmMediaType(proposed);
        return hr;
    }

    hr = receive_pin->ReceiveConnection(this, &proposed);
    if (FAILED(hr)) {
        FreeAmMediaType(proposed);
        return hr;
    }

    ComPtr<IMemAllocator> alloc;
    hr = mem_in->GetAllocator(&alloc);
    if (FAILED(hr) || !alloc) {
        hr = CoCreateInstance(
            CLSID_MemoryAllocator,
            nullptr,
            CLSCTX_INPROC_SERVER,
            IID_IMemAllocator,
            reinterpret_cast<void**>(alloc.GetAddressOf()));
        if (FAILED(hr)) {
            receive_pin->Disconnect();
            FreeAmMediaType(proposed);
            return hr;
        }
    }

    ALLOCATOR_PROPERTIES req{};
    ALLOCATOR_PROPERTIES actual{};
    req.cBuffers = 2;
    req.cbBuffer = static_cast<long>(proposed.lSampleSize);
    req.cbAlign = 16;
    req.cbPrefix = 0;
    alloc->SetProperties(&req, &actual);
    mem_in->NotifyAllocator(alloc.Get(), FALSE);

    if (has_connected_mt_) {
        FreeAmMediaType(connected_mt_);
    }
    connected_mt_ = proposed;
    has_connected_mt_ = true;
    peer_ = receive_pin;
    mem_input_ = mem_in;
    allocator_ = alloc;
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::ReceiveConnection(IPin* /*connector*/, const AM_MEDIA_TYPE* /*mt*/) {
    return E_UNEXPECTED; // Output pin only
}

STDMETHODIMP WifiCamDsPin::Disconnect() {
    StopStreaming();
    std::lock_guard guard(lock_);
    if (!peer_) return S_FALSE;
    peer_.Reset();
    mem_input_.Reset();
    allocator_.Reset();
    if (has_connected_mt_) {
        FreeAmMediaType(connected_mt_);
        has_connected_mt_ = false;
    }
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::ConnectedTo(IPin** out_pin) {
    if (!out_pin) return E_POINTER;
    std::lock_guard guard(lock_);
    if (!peer_) {
        *out_pin = nullptr;
        return VFW_E_NOT_CONNECTED;
    }
    return peer_.CopyTo(out_pin);
}

STDMETHODIMP WifiCamDsPin::ConnectionMediaType(AM_MEDIA_TYPE* out_mt) {
    if (!out_mt) return E_POINTER;
    std::lock_guard guard(lock_);
    if (!has_connected_mt_) return VFW_E_NOT_CONNECTED;
    return CopyAmMediaType(connected_mt_, out_mt);
}

STDMETHODIMP WifiCamDsPin::QueryPinInfo(PIN_INFO* out_info) {
    if (!out_info) return E_POINTER;
    out_info->pFilter = filter_;
    if (filter_) filter_->AddRef();
    out_info->dir = PINDIR_OUTPUT;
    wcsncpy_s(out_info->achName, L"Capture", _TRUNCATE);
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::QueryDirection(PIN_DIRECTION* out_dir) {
    if (!out_dir) return E_POINTER;
    *out_dir = PINDIR_OUTPUT;
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::QueryId(LPWSTR* out_id) {
    if (!out_id) return E_POINTER;
    const wchar_t id[] = L"Capture";
    *out_id = static_cast<LPWSTR>(CoTaskMemAlloc(sizeof(id)));
    if (!*out_id) return E_OUTOFMEMORY;
    std::memcpy(*out_id, id, sizeof(id));
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::QueryAccept(const AM_MEDIA_TYPE* mt) {
    if (!mt) return E_POINTER;
    if (mt->majortype != MEDIATYPE_Video || mt->subtype != MEDIASUBTYPE_NV12) {
        return S_FALSE;
    }
    if (mt->formattype != FORMAT_VideoInfo || mt->cbFormat < sizeof(VIDEOINFOHEADER) || !mt->pbFormat) {
        return S_FALSE;
    }
    const auto* vih = reinterpret_cast<const VIDEOINFOHEADER*>(mt->pbFormat);
    const uint32_t w = static_cast<uint32_t>(std::abs(vih->bmiHeader.biWidth));
    const uint32_t h = static_cast<uint32_t>(std::abs(vih->bmiHeader.biHeight));
    for (const auto& m : kSupportedModes) {
        if (m.width == w && m.height == h) {
            return S_OK;
        }
    }
    return S_FALSE;
}

STDMETHODIMP WifiCamDsPin::EnumMediaTypes(IEnumMediaTypes** out_enum) {
    if (!out_enum) return E_POINTER;
    *out_enum = new (std::nothrow) EnumMediaTypesImpl(0);
    return *out_enum ? S_OK : E_OUTOFMEMORY;
}

STDMETHODIMP WifiCamDsPin::QueryInternalConnections(IPin** /*pins*/, ULONG* /*count*/) {
    return E_NOTIMPL;
}

STDMETHODIMP WifiCamDsPin::EndOfStream() { return S_OK; }
STDMETHODIMP WifiCamDsPin::BeginFlush() { return S_OK; }
STDMETHODIMP WifiCamDsPin::EndFlush() { return S_OK; }
STDMETHODIMP WifiCamDsPin::NewSegment(REFERENCE_TIME, REFERENCE_TIME, double) { return S_OK; }

STDMETHODIMP WifiCamDsPin::SetFormat(AM_MEDIA_TYPE* mt) {
    if (!mt) return E_POINTER;
    if (QueryAccept(mt) != S_OK) return VFW_E_INVALIDMEDIATYPE;
    std::lock_guard guard(lock_);
    const auto* vih = reinterpret_cast<const VIDEOINFOHEADER*>(mt->pbFormat);
    current_mode_.width = static_cast<uint32_t>(std::abs(vih->bmiHeader.biWidth));
    current_mode_.height = static_cast<uint32_t>(std::abs(vih->bmiHeader.biHeight));
    if (vih->AvgTimePerFrame > 0) {
        current_mode_.fps_num = static_cast<uint32_t>(10'000'000LL / vih->AvgTimePerFrame);
        current_mode_.fps_den = 1;
    }
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::GetFormat(AM_MEDIA_TYPE** out_mt) {
    if (!out_mt) return E_POINTER;
    std::lock_guard guard(lock_);
    *out_mt = static_cast<AM_MEDIA_TYPE*>(CoTaskMemAlloc(sizeof(AM_MEDIA_TYPE)));
    if (!*out_mt) return E_OUTOFMEMORY;
    return BuildNv12AmMediaType(current_mode_, *out_mt);
}

STDMETHODIMP WifiCamDsPin::GetNumberOfCapabilities(int* count, int* size) {
    if (!count || !size) return E_POINTER;
    *count = static_cast<int>(std::size(kSupportedModes));
    *size = sizeof(VIDEO_STREAM_CONFIG_CAPS);
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::GetStreamCaps(int index, AM_MEDIA_TYPE** out_mt, BYTE* scc) {
    if (!out_mt || !scc) return E_POINTER;
    if (index < 0 || index >= static_cast<int>(std::size(kSupportedModes))) {
        return S_FALSE;
    }
    const auto& m = kSupportedModes[index];
    *out_mt = static_cast<AM_MEDIA_TYPE*>(CoTaskMemAlloc(sizeof(AM_MEDIA_TYPE)));
    if (!*out_mt) return E_OUTOFMEMORY;
    HRESULT hr = BuildNv12AmMediaType(m, *out_mt);
    if (FAILED(hr)) {
        CoTaskMemFree(*out_mt);
        *out_mt = nullptr;
        return hr;
    }

    auto* caps = reinterpret_cast<VIDEO_STREAM_CONFIG_CAPS*>(scc);
    std::memset(caps, 0, sizeof(VIDEO_STREAM_CONFIG_CAPS));
    caps->guid = FORMAT_VideoInfo;
    caps->VideoStandard = AnalogVideo_None;
    caps->InputSize = {static_cast<LONG>(m.width), static_cast<LONG>(m.height)};
    caps->MinCroppingSize = caps->InputSize;
    caps->MaxCroppingSize = caps->InputSize;
    caps->MinOutputSize = caps->InputSize;
    caps->MaxOutputSize = caps->InputSize;
    caps->MinFrameInterval = 166'667;
    caps->MaxFrameInterval = 333'333;
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::Set(REFGUID, DWORD, LPVOID, DWORD, LPVOID, DWORD) { return E_NOTIMPL; }

STDMETHODIMP WifiCamDsPin::Get(
    REFGUID prop_set,
    DWORD prop_id,
    LPVOID,
    DWORD,
    LPVOID prop_data,
    DWORD data_len,
    DWORD* bytes_returned) {
    if (prop_set != AMPROPSETID_Pin || prop_id != AMPROPERTY_PIN_CATEGORY) {
        return E_PROP_SET_UNSUPPORTED;
    }
    if (!prop_data && bytes_returned) {
        *bytes_returned = sizeof(GUID);
        return S_OK;
    }
    if (data_len < sizeof(GUID) || !prop_data) {
        return E_UNEXPECTED;
    }
    *reinterpret_cast<GUID*>(prop_data) = PIN_CATEGORY_CAPTURE;
    if (bytes_returned) {
        *bytes_returned = sizeof(GUID);
    }
    return S_OK;
}

STDMETHODIMP WifiCamDsPin::QuerySupported(REFGUID prop_set, DWORD prop_id, DWORD* support) {
    if (prop_set != AMPROPSETID_Pin || prop_id != AMPROPERTY_PIN_CATEGORY) {
        return E_PROP_SET_UNSUPPORTED;
    }
    if (support) *support = KSPROPERTY_SUPPORT_GET;
    return S_OK;
}

HRESULT WifiCamDsPin::StartStreaming() {
    if (streaming_.exchange(true)) {
        return S_OK;
    }
    {
        std::lock_guard guard(lock_);
        if (allocator_) {
            allocator_->Commit();
        }
    }
    worker_ = std::thread([this]() { StreamThreadLoop(); });
    return S_OK;
}

HRESULT WifiCamDsPin::StopStreaming() {
    if (!streaming_.exchange(false)) {
        return S_OK;
    }
    {
        std::lock_guard guard(lock_);
        if (allocator_) {
            allocator_->Decommit();
        }
    }
    if (worker_.joinable()) {
        worker_.join();
    }
    return S_OK;
}

void WifiCamDsPin::FillSlateNv12(uint8_t* dst, uint32_t width, uint32_t height) const {
    const size_t y_len = static_cast<size_t>(width) * height;
    const size_t uv_len = y_len / 2;
    std::memset(dst, 24, y_len);
    uint8_t* uv = dst + y_len;
    for (size_t i = 0; i + 1 < uv_len; i += 2) {
        uv[i] = 138;
        uv[i + 1] = 120;
    }
}

void WifiCamDsPin::StreamThreadLoop() {
    HANDLE mapping = OpenFileMappingW(FILE_MAP_READ, FALSE, framebus::kGlobalShmName);
    if (!mapping) {
        mapping = OpenFileMappingW(FILE_MAP_READ, FALSE, framebus::kLocalShmName);
    }
    const void* view =
        mapping ? MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, framebus::kTotalShmBytes) : nullptr;

    std::vector<uint8_t> scratch(framebus::kMaxSlotBytes);
    int64_t frame_idx = 0;

    while (streaming_.load(std::memory_order_acquire)) {
        ComPtr<IMemAllocator> alloc;
        ComPtr<IMemInputPin> mem_in;
        DsVideoMode mode{};
        {
            std::lock_guard guard(lock_);
            alloc = allocator_;
            mem_in = mem_input_;
            mode = current_mode_;
        }
        if (!alloc || !mem_in) {
            std::this_thread::sleep_for(std::chrono::milliseconds(16));
            continue;
        }

        const int64_t frame_dur_hns =
            (10'000'000LL * static_cast<int64_t>(mode.fps_den)) / std::max<int64_t>(1, mode.fps_num);
        const size_t nv12_bytes = static_cast<size_t>(mode.width) * mode.height * 3 / 2;

        ComPtr<IMediaSample> sample;
        if (FAILED(alloc->GetBuffer(&sample, nullptr, nullptr, 0)) || !sample) {
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
            continue;
        }

        BYTE* dst = nullptr;
        if (SUCCEEDED(sample->GetPointer(&dst)) && dst &&
            static_cast<size_t>(sample->GetSize()) >= nv12_bytes) {
            framebus::FrameSnapshotMeta meta{};
            if (view && framebus::TryReadLatestFrame(view, scratch.data(), scratch.size(), meta) &&
                meta.pixel_format == framebus::PixelFormat::Nv12 && meta.width == mode.width &&
                meta.height == mode.height && meta.payload_bytes >= nv12_bytes) {
                std::memcpy(dst, scratch.data(), nv12_bytes);
            } else {
                FillSlateNv12(dst, mode.width, mode.height);
            }
            sample->SetActualDataLength(static_cast<long>(nv12_bytes));
            REFERENCE_TIME t_start = frame_idx * frame_dur_hns;
            REFERENCE_TIME t_stop = t_start + frame_dur_hns;
            sample->SetTime(&t_start, &t_stop);
            sample->SetSyncPoint(TRUE);
            ++frame_idx;
            mem_in->Receive(sample.Get());
        }

        const auto sleep_us = std::max<int64_t>(1000, frame_dur_hns / 10);
        std::this_thread::sleep_for(std::chrono::microseconds(sleep_us));
    }

    if (view) UnmapViewOfFile(view);
    if (mapping) CloseHandle(mapping);
}

// ---------------------------------------------------------------------------
// WifiCamDsFilter
// ---------------------------------------------------------------------------

WifiCamDsFilter::WifiCamDsFilter() {
    g_dll_objects.fetch_add(1, std::memory_order_relaxed);
    pin_.Attach(new WifiCamDsPin(this));
}

WifiCamDsFilter::~WifiCamDsFilter() {
    Stop();
    pin_.Reset();
    g_dll_objects.fetch_sub(1, std::memory_order_relaxed);
}

HRESULT WifiCamDsFilter::CreateInstance(REFIID riid, void** ppv) {
    if (!ppv) return E_POINTER;
    *ppv = nullptr;
    auto* f = new (std::nothrow) WifiCamDsFilter();
    if (!f) return E_OUTOFMEMORY;
    HRESULT hr = f->QueryInterface(riid, ppv);
    f->Release();
    return hr;
}

STDMETHODIMP WifiCamDsFilter::QueryInterface(REFIID riid, void** ppv) {
    if (!ppv) return E_POINTER;
    if (riid == IID_IUnknown || riid == IID_IPersist || riid == IID_IMediaFilter ||
        riid == IID_IBaseFilter) {
        *ppv = static_cast<IBaseFilter*>(this);
        AddRef();
        return S_OK;
    }
    *ppv = nullptr;
    return E_NOINTERFACE;
}

STDMETHODIMP_(ULONG) WifiCamDsFilter::AddRef() { return ref_count_.fetch_add(1) + 1; }

STDMETHODIMP_(ULONG) WifiCamDsFilter::Release() {
    ULONG c = ref_count_.fetch_sub(1) - 1;
    if (c == 0) delete this;
    return c;
}

STDMETHODIMP WifiCamDsFilter::GetClassID(CLSID* out_clsid) {
    if (!out_clsid) return E_POINTER;
    *out_clsid = kClsidWifiCamDsFilter;
    return S_OK;
}

STDMETHODIMP WifiCamDsFilter::Stop() {
    std::lock_guard guard(lock_);
    state_ = State_Stopped;
    return pin_ ? pin_->StopStreaming() : S_OK;
}

STDMETHODIMP WifiCamDsFilter::Pause() {
    std::lock_guard guard(lock_);
    state_ = State_Paused;
    return pin_ ? pin_->StartStreaming() : S_OK;
}

STDMETHODIMP WifiCamDsFilter::Run(REFERENCE_TIME /*start*/) {
    std::lock_guard guard(lock_);
    state_ = State_Running;
    return pin_ ? pin_->StartStreaming() : S_OK;
}

STDMETHODIMP WifiCamDsFilter::GetState(DWORD /*timeout_ms*/, FILTER_STATE* out_state) {
    if (!out_state) return E_POINTER;
    std::lock_guard guard(lock_);
    *out_state = state_;
    return S_OK;
}

STDMETHODIMP WifiCamDsFilter::SetSyncSource(IReferenceClock* clock) {
    std::lock_guard guard(lock_);
    clock_ = clock;
    return S_OK;
}

STDMETHODIMP WifiCamDsFilter::GetSyncSource(IReferenceClock** out_clock) {
    if (!out_clock) return E_POINTER;
    std::lock_guard guard(lock_);
    return clock_.CopyTo(out_clock);
}

STDMETHODIMP WifiCamDsFilter::EnumPins(IEnumPins** out_enum) {
    if (!out_enum) return E_POINTER;
    *out_enum = new (std::nothrow) EnumPinsImpl(pin_.Get(), 0);
    return *out_enum ? S_OK : E_OUTOFMEMORY;
}

STDMETHODIMP WifiCamDsFilter::FindPin(LPCWSTR id, IPin** out_pin) {
    if (!id || !out_pin) return E_POINTER;
    if (wcscmp(id, L"Capture") == 0 && pin_) {
        return pin_.CopyTo(out_pin);
    }
    *out_pin = nullptr;
    return VFW_E_NOT_FOUND;
}

STDMETHODIMP WifiCamDsFilter::QueryFilterInfo(FILTER_INFO* out_info) {
    if (!out_info) return E_POINTER;
    std::lock_guard guard(lock_);
    wcsncpy_s(out_info->achName, name_, _TRUNCATE);
    out_info->pGraph = graph_;
    if (graph_) graph_->AddRef();
    return S_OK;
}

STDMETHODIMP WifiCamDsFilter::JoinFilterGraph(IFilterGraph* graph, LPCWSTR name) {
    std::lock_guard guard(lock_);
    graph_ = graph;
    if (name) {
        wcsncpy_s(name_, name, _TRUNCATE);
    } else {
        name_[0] = L'\0';
    }
    return S_OK;
}

STDMETHODIMP WifiCamDsFilter::QueryVendorInfo(LPWSTR* out_info) {
    if (!out_info) return E_POINTER;
    const wchar_t vendor[] = L"WifiCam Open Source";
    *out_info = static_cast<LPWSTR>(CoTaskMemAlloc(sizeof(vendor)));
    if (!*out_info) return E_OUTOFMEMORY;
    std::memcpy(*out_info, vendor, sizeof(vendor));
    return S_OK;
}

} // namespace wificam::vcam_ds
