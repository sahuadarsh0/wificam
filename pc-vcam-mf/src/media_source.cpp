#include "media_source.h"

#include <algorithm>
#include <cstring>

using Microsoft::WRL::ComPtr;

namespace wificam::vcam_mf {

namespace {

struct VideoMode {
    uint32_t width;
    uint32_t height;
    uint32_t fps_num;
    uint32_t fps_den;
};

constexpr VideoMode kSupportedModes[] = {
    {1920, 1080, 60, 1},
    {1920, 1080, 30, 1},
    {1280, 720, 60, 1},
    {3840, 2160, 30, 1},
};

} // namespace

// ---------------------------------------------------------------------------
// ShmFrameReader
// ---------------------------------------------------------------------------

ShmFrameReader::~ShmFrameReader() {
    Close();
}

bool ShmFrameReader::EnsureConnected() {
    if (view_ != nullptr) {
        return true;
    }
    mapping_ = OpenFileMappingW(FILE_MAP_READ, FALSE, framebus::kGlobalShmName);
    if (!mapping_) {
        mapping_ = OpenFileMappingW(FILE_MAP_READ, FALSE, framebus::kLocalShmName);
    }
    if (!mapping_) {
        return false;
    }
    view_ = MapViewOfFile(mapping_, FILE_MAP_READ, 0, 0, framebus::kTotalShmBytes);
    if (!view_) {
        CloseHandle(mapping_);
        mapping_ = nullptr;
        return false;
    }
    return true;
}

void ShmFrameReader::Close() {
    if (view_) {
        UnmapViewOfFile(view_);
        view_ = nullptr;
    }
    if (mapping_) {
        CloseHandle(mapping_);
        mapping_ = nullptr;
    }
}

bool ShmFrameReader::ReadLatest(
    uint8_t* dst,
    size_t dst_cap,
    framebus::FrameSnapshotMeta& out_meta) {
    if (!EnsureConnected()) {
        return false;
    }
    return framebus::TryReadLatestFrame(view_, dst, dst_cap, out_meta);
}

// ---------------------------------------------------------------------------
// WifiCamMediaStream
// ---------------------------------------------------------------------------

WifiCamMediaStream::WifiCamMediaStream(
    WifiCamMediaSource* parent,
    IMFStreamDescriptor* descriptor)
    : parent_(parent), descriptor_(descriptor) {
    g_dll_objects.fetch_add(1, std::memory_order_relaxed);
}

WifiCamMediaStream::~WifiCamMediaStream() {
    shm_reader_.Close();
    g_dll_objects.fetch_sub(1, std::memory_order_relaxed);
}

HRESULT WifiCamMediaStream::CreateInstance(
    WifiCamMediaSource* parent,
    IMFStreamDescriptor* descriptor,
    WifiCamMediaStream** out_stream) {
    if (!parent || !descriptor || !out_stream) {
        return E_POINTER;
    }
    auto* stream = new (std::nothrow) WifiCamMediaStream(parent, descriptor);
    if (!stream) {
        return E_OUTOFMEMORY;
    }
    HRESULT hr = stream->Initialize();
    if (FAILED(hr)) {
        stream->Release();
        return hr;
    }
    *out_stream = stream;
    return S_OK;
}

HRESULT WifiCamMediaStream::Initialize() {
    return MFCreateEventQueue(&event_queue_);
}

STDMETHODIMP WifiCamMediaStream::QueryInterface(REFIID riid, void** ppv) {
    if (!ppv) {
        return E_POINTER;
    }
    if (riid == IID_IUnknown || riid == IID_IMFMediaEventGenerator || riid == IID_IMFMediaStream) {
        *ppv = static_cast<IMFMediaStream*>(this);
        AddRef();
        return S_OK;
    }
    *ppv = nullptr;
    return E_NOINTERFACE;
}

STDMETHODIMP_(ULONG) WifiCamMediaStream::AddRef() {
    return ref_count_.fetch_add(1, std::memory_order_relaxed) + 1;
}

STDMETHODIMP_(ULONG) WifiCamMediaStream::Release() {
    const ULONG count = ref_count_.fetch_sub(1, std::memory_order_acq_rel) - 1;
    if (count == 0) {
        delete this;
    }
    return count;
}

STDMETHODIMP WifiCamMediaStream::GetEvent(DWORD flags, IMFMediaEvent** out_event) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->GetEvent(flags, out_event);
}

STDMETHODIMP WifiCamMediaStream::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->BeginGetEvent(callback, state);
}

STDMETHODIMP WifiCamMediaStream::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** out_event) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->EndGetEvent(result, out_event);
}

STDMETHODIMP WifiCamMediaStream::QueueEvent(
    MediaEventType met,
    REFGUID extended_type,
    HRESULT status,
    const PROPVARIANT* value) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->QueueEventParamVar(met, extended_type, status, value);
}

STDMETHODIMP WifiCamMediaStream::GetMediaSource(IMFMediaSource** out_source) {
    if (!out_source) {
        return E_POINTER;
    }
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return parent_.CopyTo(out_source);
}

STDMETHODIMP WifiCamMediaStream::GetStreamDescriptor(IMFStreamDescriptor** out_desc) {
    if (!out_desc) {
        return E_POINTER;
    }
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return descriptor_.CopyTo(out_desc);
}

HRESULT WifiCamMediaStream::Start(const PROPVARIANT* start_pos) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    ComPtr<IMFMediaTypeHandler> handler;
    ComPtr<IMFMediaType> current_type;
    if (SUCCEEDED(descriptor_->GetMediaTypeHandler(&handler)) &&
        SUCCEEDED(handler->GetCurrentMediaType(&current_type))) {
        UINT32 w = 1920, h = 1080, num = 60, den = 1;
        if (SUCCEEDED(MFGetAttributeSize(current_type.Get(), MF_MT_FRAME_SIZE, &w, &h))) {
            width_ = w;
            height_ = h;
        }
        if (SUCCEEDED(MFGetAttributeRatio(current_type.Get(), MF_MT_FRAME_RATE, &num, &den)) &&
            den > 0) {
            fps_num_ = num;
            fps_den_ = den;
        }
    }
    is_started_ = true;
    sample_index_ = 0;
    return event_queue_->QueueEventParamVar(MEStreamStarted, GUID_NULL, S_OK, start_pos);
}

HRESULT WifiCamMediaStream::Stop() {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    is_started_ = false;
    return event_queue_->QueueEventParamVar(MEStreamStopped, GUID_NULL, S_OK, nullptr);
}

HRESULT WifiCamMediaStream::Shutdown() {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return S_OK;
    }
    is_shutdown_ = true;
    is_started_ = false;
    shm_reader_.Close();
    if (event_queue_) {
        event_queue_->Shutdown();
        event_queue_.Reset();
    }
    parent_.Reset();
    descriptor_.Reset();
    return S_OK;
}

void WifiCamMediaStream::FillSlateNv12(uint8_t* dst, uint32_t width, uint32_t height) const {
    const size_t y_len = static_cast<size_t>(width) * height;
    const size_t uv_len = y_len / 2;
    std::memset(dst, 24, y_len);
    uint8_t* uv = dst + y_len;
    for (size_t i = 0; i + 1 < uv_len; i += 2) {
        uv[i] = 138;
        uv[i + 1] = 120;
    }
}

STDMETHODIMP WifiCamMediaStream::RequestSample(IUnknown* token) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    if (!is_started_) {
        return MF_E_INVALIDREQUEST;
    }

    const size_t target_nv12_bytes = static_cast<size_t>(width_) * height_ * 3 / 2;
    if (scratch_.size() < framebus::kMaxSlotBytes) {
        scratch_.resize(framebus::kMaxSlotBytes);
    }

    ComPtr<IMFMediaBuffer> buffer;
    HRESULT hr = MFCreateMemoryBuffer(static_cast<DWORD>(target_nv12_bytes), &buffer);
    if (FAILED(hr)) {
        return hr;
    }

    BYTE* dst_ptr = nullptr;
    hr = buffer->Lock(&dst_ptr, nullptr, nullptr);
    if (FAILED(hr)) {
        return hr;
    }

    framebus::FrameSnapshotMeta meta{};
    const bool got_frame = shm_reader_.ReadLatest(scratch_.data(), scratch_.size(), meta);
    if (got_frame && meta.pixel_format == framebus::PixelFormat::Nv12 &&
        meta.width == width_ && meta.height == height_ &&
        meta.payload_bytes >= target_nv12_bytes) {
        std::memcpy(dst_ptr, scratch_.data(), target_nv12_bytes);
    } else {
        FillSlateNv12(dst_ptr, width_, height_);
    }

    buffer->Unlock();
    buffer->SetCurrentLength(static_cast<DWORD>(target_nv12_bytes));

    ComPtr<IMFSample> sample;
    hr = MFCreateSample(&sample);
    if (FAILED(hr)) {
        return hr;
    }
    hr = sample->AddBuffer(buffer.Get());
    if (FAILED(hr)) {
        return hr;
    }

    const int64_t duration_hns =
        (10'000'000LL * static_cast<int64_t>(fps_den_)) / std::max<int64_t>(1, fps_num_);
    const int64_t timestamp_hns = sample_index_ * duration_hns;
    ++sample_index_;

    sample->SetSampleTime(timestamp_hns);
    sample->SetSampleDuration(duration_hns);

    if (token) {
        sample->SetUnknown(MFSampleExtension_Token, token);
    }

    return event_queue_->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.Get());
}

// ---------------------------------------------------------------------------
// WifiCamMediaSource
// ---------------------------------------------------------------------------

WifiCamMediaSource::WifiCamMediaSource() {
    g_dll_objects.fetch_add(1, std::memory_order_relaxed);
}

WifiCamMediaSource::~WifiCamMediaSource() {
    Shutdown();
    g_dll_objects.fetch_sub(1, std::memory_order_relaxed);
}

HRESULT WifiCamMediaSource::CreateInstance(REFIID riid, void** ppv) {
    if (!ppv) {
        return E_POINTER;
    }
    *ppv = nullptr;
    auto* src = new (std::nothrow) WifiCamMediaSource();
    if (!src) {
        return E_OUTOFMEMORY;
    }
    HRESULT hr = src->Initialize();
    if (SUCCEEDED(hr)) {
        hr = src->QueryInterface(riid, ppv);
    }
    src->Release();
    return hr;
}

HRESULT WifiCamMediaSource::CreateNv12MediaType(
    uint32_t width,
    uint32_t height,
    uint32_t fps_num,
    uint32_t fps_den,
    IMFMediaType** out_type) {
    ComPtr<IMFMediaType> mt;
    HRESULT hr = MFCreateMediaType(&mt);
    if (FAILED(hr)) {
        return hr;
    }
    mt->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
    mt->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12);
    mt->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive);
    mt->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE);
    MFSetAttributeSize(mt.Get(), MF_MT_FRAME_SIZE, width, height);
    MFSetAttributeRatio(mt.Get(), MF_MT_FRAME_RATE, fps_num, fps_den);
    MFSetAttributeRatio(mt.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1);
    mt->SetUINT32(MF_MT_DEFAULT_STRIDE, width);
    mt->SetUINT32(MF_MT_SAMPLE_SIZE, width * height * 3 / 2);
    *out_type = mt.Detach();
    return S_OK;
}

HRESULT WifiCamMediaSource::Initialize() {
    HRESULT hr = MFCreateEventQueue(&event_queue_);
    if (FAILED(hr)) {
        return hr;
    }
    hr = MFCreateAttributes(&source_attrs_, 4);
    if (FAILED(hr)) {
        return hr;
    }
    hr = MFCreateAttributes(&stream_attrs_, 4);
    if (FAILED(hr)) {
        return hr;
    }

    constexpr size_t mode_count = std::size(kSupportedModes);
    IMFMediaType* types[mode_count]{};
    for (size_t i = 0; i < mode_count; ++i) {
        hr = CreateNv12MediaType(
            kSupportedModes[i].width,
            kSupportedModes[i].height,
            kSupportedModes[i].fps_num,
            kSupportedModes[i].fps_den,
            &types[i]);
        if (FAILED(hr)) {
            for (size_t j = 0; j < i; ++j) {
                types[j]->Release();
            }
            return hr;
        }
    }

    ComPtr<IMFStreamDescriptor> stream_desc;
    hr = MFCreateStreamDescriptor(0, static_cast<DWORD>(mode_count), types, &stream_desc);
    for (auto* t : types) {
        if (t) {
            t->Release();
        }
    }
    if (FAILED(hr)) {
        return hr;
    }

    ComPtr<IMFMediaTypeHandler> handler;
    if (SUCCEEDED(stream_desc->GetMediaTypeHandler(&handler))) {
        ComPtr<IMFMediaType> default_type;
        if (SUCCEEDED(handler->GetMediaTypeByIndex(0, &default_type))) {
            handler->SetCurrentMediaType(default_type.Get());
        }
    }

    IMFStreamDescriptor* sd_raw = stream_desc.Get();
    hr = MFCreatePresentationDescriptor(1, &sd_raw, &presentation_desc_);
    if (FAILED(hr)) {
        return hr;
    }
    presentation_desc_->SelectStream(0);

    hr = WifiCamMediaStream::CreateInstance(this, stream_desc.Get(), &stream_);
    return hr;
}

STDMETHODIMP WifiCamMediaSource::QueryInterface(REFIID riid, void** ppv) {
    if (!ppv) {
        return E_POINTER;
    }
    if (riid == IID_IUnknown || riid == IID_IMFMediaEventGenerator ||
        riid == IID_IMFMediaSource || riid == IID_IMFMediaSourceEx) {
        *ppv = static_cast<IMFMediaSourceEx*>(this);
        AddRef();
        return S_OK;
    }
    if (riid == __uuidof(IKsControl)) {
        *ppv = static_cast<IKsControl*>(this);
        AddRef();
        return S_OK;
    }
    *ppv = nullptr;
    return E_NOINTERFACE;
}

STDMETHODIMP_(ULONG) WifiCamMediaSource::AddRef() {
    return ref_count_.fetch_add(1, std::memory_order_relaxed) + 1;
}

STDMETHODIMP_(ULONG) WifiCamMediaSource::Release() {
    const ULONG count = ref_count_.fetch_sub(1, std::memory_order_acq_rel) - 1;
    if (count == 0) {
        delete this;
    }
    return count;
}

STDMETHODIMP WifiCamMediaSource::GetEvent(DWORD flags, IMFMediaEvent** out_event) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->GetEvent(flags, out_event);
}

STDMETHODIMP WifiCamMediaSource::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->BeginGetEvent(callback, state);
}

STDMETHODIMP WifiCamMediaSource::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** out_event) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->EndGetEvent(result, out_event);
}

STDMETHODIMP WifiCamMediaSource::QueueEvent(
    MediaEventType met,
    REFGUID extended_type,
    HRESULT status,
    const PROPVARIANT* value) {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return event_queue_->QueueEventParamVar(met, extended_type, status, value);
}

STDMETHODIMP WifiCamMediaSource::GetCharacteristics(DWORD* characteristics) {
    if (!characteristics) {
        return E_POINTER;
    }
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    *characteristics = MFMEDIASOURCE_IS_LIVE;
    return S_OK;
}

STDMETHODIMP WifiCamMediaSource::CreatePresentationDescriptor(IMFPresentationDescriptor** out_pd) {
    if (!out_pd) {
        return E_POINTER;
    }
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return presentation_desc_->Clone(out_pd);
}

STDMETHODIMP WifiCamMediaSource::Start(
    IMFPresentationDescriptor* pd,
    const GUID* time_format,
    const PROPVARIANT* start_pos) {
    if (!pd || !start_pos) {
        return E_INVALIDARG;
    }
    if (time_format && *time_format != GUID_NULL) {
        return MF_E_UNSUPPORTED_TIME_FORMAT;
    }

    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }

    HRESULT hr = event_queue_->QueueEventParamVar(MESourceStarted, GUID_NULL, S_OK, start_pos);
    if (FAILED(hr)) {
        return hr;
    }
    hr = event_queue_->QueueEventParamUnk(MENewStream, GUID_NULL, S_OK, stream_.Get());
    if (FAILED(hr)) {
        return hr;
    }
    return stream_->Start(start_pos);
}

STDMETHODIMP WifiCamMediaSource::Stop() {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    if (stream_) {
        stream_->Stop();
    }
    return event_queue_->QueueEventParamVar(MESourceStopped, GUID_NULL, S_OK, nullptr);
}

STDMETHODIMP WifiCamMediaSource::Pause() {
    return MF_E_INVALID_STATE_TRANSITION;
}

STDMETHODIMP WifiCamMediaSource::Shutdown() {
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return S_OK;
    }
    is_shutdown_ = true;
    if (stream_) {
        stream_->Shutdown();
        stream_.Reset();
    }
    if (event_queue_) {
        event_queue_->Shutdown();
        event_queue_.Reset();
    }
    presentation_desc_.Reset();
    source_attrs_.Reset();
    stream_attrs_.Reset();
    return S_OK;
}

STDMETHODIMP WifiCamMediaSource::GetSourceAttributes(IMFAttributes** out_attrs) {
    if (!out_attrs) {
        return E_POINTER;
    }
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return source_attrs_.CopyTo(out_attrs);
}

STDMETHODIMP WifiCamMediaSource::GetStreamAttributes(DWORD stream_id, IMFAttributes** out_attrs) {
    if (!out_attrs) {
        return E_POINTER;
    }
    if (stream_id != 0) {
        return MF_E_INVALIDSTREAMNUMBER;
    }
    std::lock_guard guard(lock_);
    if (is_shutdown_) {
        return MF_E_SHUTDOWN;
    }
    return stream_attrs_.CopyTo(out_attrs);
}

STDMETHODIMP WifiCamMediaSource::SetD3DManager(IUnknown* /*manager*/) {
    return S_OK;
}

STDMETHODIMP WifiCamMediaSource::KsProperty(
    PKSPROPERTY /*property*/,
    ULONG /*property_length*/,
    LPVOID /*property_data*/,
    ULONG /*data_length*/,
    ULONG* bytes_returned) {
    if (bytes_returned) {
        *bytes_returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP WifiCamMediaSource::KsMethod(
    PKSMETHOD /*method*/,
    ULONG /*method_length*/,
    LPVOID /*method_data*/,
    ULONG /*data_length*/,
    ULONG* bytes_returned) {
    if (bytes_returned) {
        *bytes_returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP WifiCamMediaSource::KsEvent(
    PKSEVENT /*event*/,
    ULONG /*event_length*/,
    LPVOID /*event_data*/,
    ULONG /*data_length*/,
    ULONG* bytes_returned) {
    if (bytes_returned) {
        *bytes_returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

} // namespace wificam::vcam_mf
