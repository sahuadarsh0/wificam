; WifiCam Studio — Windows 11 / 10 Production Installer (Inno Setup 6)
; Installs the WinUI 3 Studio Console, Rust Core FFI DLL, and Virtual Camera COM DLLs
; into %ProgramFiles%\WifiCam (required for Win11 FrameServer access).

#define AppName "WifiCam Studio"
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppPublisher "WifiCam Open Source Contributors"
#define AppURL "https://github.com/sahuadarsh0/wificam"
#define AppExeName "WifiCam.Desktop.exe"

[Setup]
AppId={{B7E4F9A1-6C2D-4F18-9E3A-1D4C5B6E7F80}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
DefaultDirName={autopf}\WifiCam
DefaultGroupName={#AppName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
OutputBaseFilename=WifiCam-Studio-Setup-{#AppVersion}-x64
SetupIconFile=..\pc-ui\Assets\wificam.ico
UninstallDisplayIcon={app}\{#AppExeName}
LicenseFile=..\LICENSE

[Files]
; WinUI 3 Desktop Studio Console + Rust pc_core_ffi.dll
Source: "..\pc-ui\bin\x64\Release\net10.0-windows10.0.26100.0\win-x64\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; Windows 11 Media Foundation Virtual Camera COM DLL
Source: "..\pc-vcam-mf\build\Release\pc-vcam-mf.dll"; DestDir: "{app}\vcam"; Flags: ignoreversion skipifsourcedoesntexist
; Windows 10 DirectShow Virtual Camera Fallback DLL
Source: "..\pc-vcam-ds\build\Release\pc-vcam-ds.dll"; DestDir: "{app}\vcam"; Flags: ignoreversion skipifsourcedoesntexist
; Optional 1-click VB-Audio Virtual Cable helper script
Source: "install-virtual-mic.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"
Name: "firewall"; Description: "Allow WifiCam LAN streaming (TCP 7890, UDP 7891) in Windows Firewall"; GroupDescription: "Network:"
Name: "virtualmic"; Description: "Install free VB-Audio Virtual Cable (enables Windows Microphone input in Meet / Teams / Zoom)"; GroupDescription: "Audio:"; Flags: unchecked

[Run]
; Register Win11 Media Foundation Virtual Camera COM source
Filename: "{sys}\regsvr32.exe"; Parameters: "/s ""{app}\vcam\pc-vcam-mf.dll"""; StatusMsg: "Registering Windows 11 Media Foundation Virtual Camera..."; Flags: runhidden; Check: FileExists(ExpandConstant('{app}\vcam\pc-vcam-mf.dll'))
; Register Win10 DirectShow fallback filter
Filename: "{sys}\regsvr32.exe"; Parameters: "/s ""{app}\vcam\pc-vcam-ds.dll"""; StatusMsg: "Registering DirectShow Virtual Camera fallback..."; Flags: runhidden; Check: FileExists(ExpandConstant('{app}\vcam\pc-vcam-ds.dll'))
; Add scoped Windows Defender Firewall rules (Private networks)
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""WifiCam Control (TCP 7890)"" dir=in action=allow protocol=TCP localport=7890 profile=private"; Flags: runhidden; Tasks: firewall
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""WifiCam Media (UDP 7891)"" dir=in action=allow protocol=UDP localport=7891 profile=private"; Flags: runhidden; Tasks: firewall
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\install-virtual-mic.ps1"""; StatusMsg: "Setting up Virtual Microphone driver (VB-Audio Virtual Cable)..."; Tasks: virtualmic
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\regsvr32.exe"; Parameters: "/s /u ""{app}\vcam\pc-vcam-mf.dll"""; Flags: runhidden; RunOnceId: "UnregMF"; Check: FileExists(ExpandConstant('{app}\vcam\pc-vcam-mf.dll'))
Filename: "{sys}\regsvr32.exe"; Parameters: "/s /u ""{app}\vcam\pc-vcam-ds.dll"""; Flags: runhidden; RunOnceId: "UnregDS"; Check: FileExists(ExpandConstant('{app}\vcam\pc-vcam-ds.dll'))
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""WifiCam Control (TCP 7890)"""; Flags: runhidden; RunOnceId: "DelFwTcp"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""WifiCam Media (UDP 7891)"""; Flags: runhidden; RunOnceId: "DelFwUdp"
