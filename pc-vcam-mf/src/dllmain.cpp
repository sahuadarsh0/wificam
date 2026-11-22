#include "media_source.h"

#include <string>

namespace wificam::vcam_mf {

std::atomic<long> g_dll_objects{0};

namespace {

HMODULE g_module = nullptr;

class ClassFactory final : public IClassFactory {
public:
    ClassFactory() { g_dll_objects.fetch_add(1, std::memory_order_relaxed); }
    ~ClassFactory() { g_dll_objects.fetch_sub(1, std::memory_order_relaxed); }

    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (!ppv) {
            return E_POINTER;
        }
        if (riid == IID_IUnknown || riid == IID_IClassFactory) {
            *ppv = static_cast<IClassFactory*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    STDMETHODIMP_(ULONG) AddRef() override {
        return ref_count_.fetch_add(1, std::memory_order_relaxed) + 1;
    }

    STDMETHODIMP_(ULONG) Release() override {
        const ULONG count = ref_count_.fetch_sub(1, std::memory_order_acq_rel) - 1;
        if (count == 0) {
            delete this;
        }
        return count;
    }

    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** ppv) override {
        if (outer != nullptr) {
            return CLASS_E_NOAGGREGATION;
        }
        return WifiCamMediaSource::CreateInstance(riid, ppv);
    }

    STDMETHODIMP LockServer(BOOL lock) override {
        if (lock) {
            g_dll_objects.fetch_add(1, std::memory_order_relaxed);
        } else {
            g_dll_objects.fetch_sub(1, std::memory_order_relaxed);
        }
        return S_OK;
    }

private:
    std::atomic<ULONG> ref_count_{1};
};

HRESULT SetRegString(HKEY root, const std::wstring& subkey, const wchar_t* name, const wchar_t* val) {
    HKEY key = nullptr;
    const LONG rc = RegCreateKeyExW(
        root, subkey.c_str(), 0, nullptr, REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr, &key, nullptr);
    if (rc != ERROR_SUCCESS) {
        return HRESULT_FROM_WIN32(rc);
    }
    const DWORD bytes = static_cast<DWORD>((wcslen(val) + 1) * sizeof(wchar_t));
    const LONG set_rc = RegSetValueExW(key, name, 0, REG_SZ, reinterpret_cast<const BYTE*>(val), bytes);
    RegCloseKey(key);
    return HRESULT_FROM_WIN32(set_rc);
}

} // namespace

} // namespace wificam::vcam_mf

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID /*reserved*/) {
    if (reason == DLL_PROCESS_ATTACH) {
        wificam::vcam_mf::g_module = module;
        DisableThreadLibraryCalls(module);
    }
    return TRUE;
}

STDAPI DllCanUnloadNow() {
    return wificam::vcam_mf::g_dll_objects.load(std::memory_order_acquire) == 0 ? S_OK : S_FALSE;
}

STDAPI DllGetClassObject(REFCLSID rclsid, REFIID riid, LPVOID* ppv) {
    if (!ppv) {
        return E_POINTER;
    }
    *ppv = nullptr;
    if (rclsid != wificam::vcam_mf::kClsidWifiCamMfSource) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    auto* factory = new (std::nothrow) wificam::vcam_mf::ClassFactory();
    if (!factory) {
        return E_OUTOFMEMORY;
    }
    const HRESULT hr = factory->QueryInterface(riid, ppv);
    factory->Release();
    return hr;
}

STDAPI DllRegisterServer() {
    wchar_t dll_path[MAX_PATH]{};
    if (GetModuleFileNameW(wificam::vcam_mf::g_module, dll_path, MAX_PATH) == 0) {
        return HRESULT_FROM_WIN32(GetLastError());
    }

    const std::wstring base = std::wstring(L"Software\\Classes\\CLSID\\") + wificam::vcam_mf::kClsidString;
    const std::wstring inproc = base + L"\\InProcServer32";

    HRESULT hr = wificam::vcam_mf::SetRegString(
        HKEY_LOCAL_MACHINE, base, nullptr, wificam::vcam_mf::kFriendlyName);
    if (FAILED(hr)) {
        return hr;
    }
    hr = wificam::vcam_mf::SetRegString(HKEY_LOCAL_MACHINE, inproc, nullptr, dll_path);
    if (FAILED(hr)) {
        return hr;
    }
    return wificam::vcam_mf::SetRegString(HKEY_LOCAL_MACHINE, inproc, L"ThreadingModel", L"Both");
}

STDAPI DllUnregisterServer() {
    const std::wstring base = std::wstring(L"Software\\Classes\\CLSID\\") + wificam::vcam_mf::kClsidString;
    const LONG rc = RegDeleteTreeW(HKEY_LOCAL_MACHINE, base.c_str());
    if (rc == ERROR_FILE_NOT_FOUND) {
        return S_OK;
    }
    return HRESULT_FROM_WIN32(rc);
}
