// Programming IME G0 TSF pass-through prototype.
// WARNING: Windows-only and NOT built/tested in this Linux packaging environment.
// This registers 3 Windows language profiles but intentionally does not consume keys.
// It cannot yet show Rust candidates, contact the AI worker, or edit a document.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <msctf.h>
#include <objbase.h>
#include <atomic>
#include <string>
#include <cwchar>
#include <utility>
#include <new>
#include <initializer_list>

static HMODULE g_module = nullptr;
static std::atomic<long> g_objects{0};
static std::atomic<long> g_locks{0};

// Stable experimental UUIDs: NEVER publish under these identifiers without release governance.
// Change before production signing/releasing, then freeze for all future versions.
static const CLSID CLSID_Pime = {0x43f95ed1,0x95b4,0x421d,{0x8f,0x5a,0x83,0x71,0x28,0xc2,0xd8,0x54}};
static const GUID ProfileZh = {0x223fd919,0xf8cb,0x476c,{0xa8,0x52,0x35,0x1e,0x73,0x81,0xc8,0x01}};
static const GUID ProfileEn = {0x68ed92de,0x77cf,0x43b1,{0xa7,0xb2,0xd3,0xbe,0x26,0x86,0xe2,0xa0}};
static const GUID ProfileJa = {0x22e54b52,0x5082,0x42d3,{0x9e,0x41,0x1c,0x2e,0x0c,0xe7,0x4d,0xfb}};

class PimeTip final : public ITfTextInputProcessor, public ITfKeyEventSink {
 public:
  PimeTip() : refs_(1) { ++g_objects; }
  ~PimeTip() { Deactivate(); --g_objects; }
  STDMETHODIMP QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (IsEqualIID(iid, IID_IUnknown) || IsEqualIID(iid, IID_ITfTextInputProcessor))
      *out = static_cast<ITfTextInputProcessor*>(this);
    else if (IsEqualIID(iid, IID_ITfKeyEventSink))
      *out = static_cast<ITfKeyEventSink*>(this);
    else return E_NOINTERFACE;
    AddRef(); return S_OK;
  }
  STDMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(++refs_); }
  STDMETHODIMP_(ULONG) Release() override {
    auto n=--refs_;
    if (n==0) delete this;
    return static_cast<ULONG>(n);
  }
  STDMETHODIMP Activate(ITfThreadMgr* mgr, TfClientId clientId) override {
    if (!mgr) return E_INVALIDARG;
    Deactivate();
    mgr_=mgr; mgr_->AddRef(); clientId_=clientId;
    // Sink observes events; every key is explicitly passed through unchanged.
    HRESULT hr=mgr_->QueryInterface(IID_PPV_ARGS(&keys_));
    if (SUCCEEDED(hr)) {
      hr=keys_->AdviseKeyEventSink(clientId_,static_cast<ITfKeyEventSink*>(this),TRUE);
      advised_=SUCCEEDED(hr);
    }
    return S_OK; // Fail open to direct keyboard input if sink is unavailable.
  }
  STDMETHODIMP Deactivate() override {
    if (keys_) {
      if (advised_) keys_->UnadviseKeyEventSink(clientId_);
      keys_->Release(); keys_=nullptr; advised_=false;
    }
    if (mgr_) { mgr_->Release(); mgr_=nullptr; }
    return S_OK;
  }
  STDMETHODIMP OnSetFocus(BOOL) override { return S_OK; }
  STDMETHODIMP OnTestKeyDown(ITfContext*,WPARAM,LPARAM,BOOL* eat) override {
    if (!eat) return E_POINTER; *eat=FALSE; return S_OK;
  }
  STDMETHODIMP OnTestKeyUp(ITfContext*,WPARAM,LPARAM,BOOL* eat) override {
    if (!eat) return E_POINTER; *eat=FALSE; return S_OK;
  }
  STDMETHODIMP OnKeyDown(ITfContext*,WPARAM,LPARAM,BOOL* eat) override {
    if (!eat) return E_POINTER; *eat=FALSE; return S_OK;
  }
  STDMETHODIMP OnKeyUp(ITfContext*,WPARAM,LPARAM,BOOL* eat) override {
    if (!eat) return E_POINTER; *eat=FALSE; return S_OK;
  }
  STDMETHODIMP OnPreservedKey(ITfContext*,REFGUID,BOOL* eat) override {
    if (!eat) return E_POINTER; *eat=FALSE; return S_OK;
  }
 private:
  std::atomic<long> refs_;
  ITfThreadMgr* mgr_=nullptr;
  ITfKeystrokeMgr* keys_=nullptr;
  TfClientId clientId_=TF_CLIENTID_NULL;
  bool advised_=false;
};

class PimeFactory final : public IClassFactory {
 public:
  PimeFactory() : refs_(1) { ++g_objects; }
  ~PimeFactory() { --g_objects; }
  STDMETHODIMP QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER; *out=nullptr;
    if (IsEqualIID(iid,IID_IUnknown)||IsEqualIID(iid,IID_IClassFactory))
      *out=static_cast<IClassFactory*>(this);
    else return E_NOINTERFACE;
    AddRef(); return S_OK;
  }
  STDMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(++refs_); }
  STDMETHODIMP_(ULONG) Release() override {
    auto n=--refs_; if(!n)delete this; return static_cast<ULONG>(n);
  }
  STDMETHODIMP CreateInstance(IUnknown* outer,REFIID iid,void** out) override {
    if (outer) return CLASS_E_NOAGGREGATION;
    auto* tip = new (std::nothrow) PimeTip();
    if(!tip) return E_OUTOFMEMORY;
    const HRESULT hr=tip->QueryInterface(iid,out);
    tip->Release();return hr;
  }
  STDMETHODIMP LockServer(BOOL lock) override {
    if(lock)++g_locks;else --g_locks;return S_OK;
  }
 private:
  std::atomic<long> refs_;
};

extern "C" BOOL WINAPI DllMain(HINSTANCE inst,DWORD reason,LPVOID) {
  if(reason==DLL_PROCESS_ATTACH) {g_module=inst; DisableThreadLibraryCalls(inst);}
  return TRUE;
}
extern "C" __declspec(dllexport) HRESULT __stdcall DllGetClassObject(REFCLSID cls,REFIID iid,void** out) {
  if(!IsEqualCLSID(cls,CLSID_Pime))return CLASS_E_CLASSNOTAVAILABLE;
  auto* factory = new (std::nothrow) PimeFactory();
  if(!factory)return E_OUTOFMEMORY;
  const HRESULT hr=factory->QueryInterface(iid,out);
  factory->Release();return hr;
}
extern "C" __declspec(dllexport) HRESULT __stdcall DllCanUnloadNow() {
  return (g_objects.load()==0&&g_locks.load()==0)?S_OK:S_FALSE;
}

static HRESULT SetRegistryString(HKEY root,const std::wstring& path,const wchar_t* valueName,const std::wstring& value){
  HKEY key=nullptr;
  LONG error=RegCreateKeyExW(root,path.c_str(),0,nullptr,REG_OPTION_NON_VOLATILE,KEY_SET_VALUE,nullptr,&key,nullptr);
  if(error!=ERROR_SUCCESS)return HRESULT_FROM_WIN32(error);
  error=RegSetValueExW(key,valueName,0,REG_SZ,reinterpret_cast<const BYTE*>(value.c_str()),
                       static_cast<DWORD>((value.size()+1)*sizeof(wchar_t)));
  RegCloseKey(key);
  return HRESULT_FROM_WIN32(error);
}
static std::wstring CLSIDPath(){
  wchar_t guid[64]{};
  StringFromGUID2(CLSID_Pime,guid,64);
  return std::wstring(L"Software\\Classes\\CLSID\\")+guid;
}
static HRESULT RegisterProfiles(){
  ITfInputProcessorProfiles* profiles=nullptr;
  HRESULT hr=CoCreateInstance(CLSID_TF_InputProcessorProfiles,nullptr,CLSCTX_INPROC_SERVER,
                              IID_PPV_ARGS(&profiles));
  if(FAILED(hr))return hr;
  hr=profiles->Register(CLSID_Pime);
  if(SUCCEEDED(hr)){
    constexpr wchar_t description[]=L"Programming IME (experimental pass-through)";
    for(const auto& [lang,guid] : {std::pair<LANGID,GUID>{0x0804,ProfileZh},
                                  {0x0409,ProfileEn},{0x0411,ProfileJa}}){
      hr=profiles->AddLanguageProfile(CLSID_Pime,lang,guid,description,
                                     static_cast<ULONG>(wcslen(description)),nullptr,0,0);
      if(FAILED(hr))break;
    }
  }
  if(FAILED(hr))profiles->Unregister(CLSID_Pime);
  profiles->Release();
  if(FAILED(hr))return hr;
  ITfCategoryMgr* mgr=nullptr;
  hr=CoCreateInstance(CLSID_TF_CategoryMgr,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&mgr));
  if(SUCCEEDED(hr)){
    hr=mgr->RegisterCategory(CLSID_Pime,GUID_TFCAT_TIP_KEYBOARD,CLSID_Pime);
    mgr->Release();
  }
  return hr;
}
extern "C" __declspec(dllexport) HRESULT __stdcall DllRegisterServer(){
  wchar_t module[MAX_PATH]{};
  if(!GetModuleFileNameW(g_module,module,MAX_PATH))return HRESULT_FROM_WIN32(GetLastError());
  const auto key=CLSIDPath();
  HRESULT hr=SetRegistryString(HKEY_CURRENT_USER,key,nullptr,L"Programming IME Prototype");
  if(SUCCEEDED(hr))hr=SetRegistryString(HKEY_CURRENT_USER,key+L"\\InprocServer32",nullptr,module);
  if(SUCCEEDED(hr))hr=SetRegistryString(HKEY_CURRENT_USER,key+L"\\InprocServer32",L"ThreadingModel",L"Apartment");
  if(FAILED(hr))return hr;
  hr=CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
  if(FAILED(hr))return hr;
  const HRESULT result=RegisterProfiles();
  CoUninitialize();
  return result;
}
extern "C" __declspec(dllexport) HRESULT __stdcall DllUnregisterServer(){
  const HRESULT init=CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
  if(SUCCEEDED(init)){
    ITfCategoryMgr* mgr=nullptr;
    if(SUCCEEDED(CoCreateInstance(CLSID_TF_CategoryMgr,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&mgr)))){
      mgr->UnregisterCategory(CLSID_Pime,GUID_TFCAT_TIP_KEYBOARD,CLSID_Pime);mgr->Release();
    }
    ITfInputProcessorProfiles* profiles=nullptr;
    if(SUCCEEDED(CoCreateInstance(CLSID_TF_InputProcessorProfiles,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&profiles)))){
      profiles->Unregister(CLSID_Pime);profiles->Release();
    }
    CoUninitialize();
  }
  const LONG error=RegDeleteTreeW(HKEY_CURRENT_USER,CLSIDPath().c_str());
  return(error==ERROR_SUCCESS||error==ERROR_FILE_NOT_FOUND)?S_OK:HRESULT_FROM_WIN32(error);
}
