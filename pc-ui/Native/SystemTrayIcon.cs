using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using WinRT.Interop;

namespace WifiCam.Desktop.Native;

/// <summary>
/// Pure Win32 Shell_NotifyIconW system-tray controller for WinUI 3 with native
/// right-click context menu and Explorer-restart recovery (zero WinForms deps).
/// </summary>
public sealed class SystemTrayIcon : IDisposable
{
    private const uint WM_APP = 0x8000;
    private const uint WM_TRAYICON = WM_APP + 42;
    private const uint WM_LBUTTONUP = 0x0202;
    private const uint WM_LBUTTONDBLCLK = 0x0203;
    private const uint WM_RBUTTONUP = 0x0205;
    private const uint WM_CONTEXTMENU = 0x007B;

    private const uint NIM_ADD = 0x00000000;
    private const uint NIM_MODIFY = 0x00000001;
    private const uint NIM_DELETE = 0x00000002;
    private const uint NIF_MESSAGE = 0x00000001;
    private const uint NIF_ICON = 0x00000002;
    private const uint NIF_TIP = 0x00000004;

    private const uint IMAGE_ICON = 1;
    private const uint LR_LOADFROMFILE = 0x00000010;
    private const uint LR_DEFAULTSIZE = 0x00000040;

    private const uint MF_STRING = 0x00000000;
    private const uint MF_SEPARATOR = 0x00000800;
    private const uint TPM_RIGHTALIGN = 0x0008;
    private const uint TPM_BOTTOMALIGN = 0x0020;
    private const uint TPM_RETURNCMD = 0x0100;
    private const uint TPM_RIGHTBUTTON = 0x0002;

    private const uint CMD_OPEN = 1001;
    private const uint CMD_MUTE = 1002;
    private const uint CMD_PAUSE = 1003;
    private const uint CMD_EXIT = 1004;

    private const nuint SubclassId = 4242;

    private readonly nint _hwnd;
    private readonly uint _taskbarCreatedMsg;
    private readonly uint _showExistingWindowMsg;
    private readonly SubclassProcDelegate _subclassProc;
    private nint _hIcon;
    private bool _added;
    private bool _disposed;

    public event Action? OpenRequested;
    public event Action? ToggleMuteRequested;
    public event Action? TogglePauseRequested;
    public event Action? ExitRequested;

    public SystemTrayIcon(Window window, string iconPath, string tooltip)
    {
        _hwnd = WindowNative.GetWindowHandle(window);
        _taskbarCreatedMsg = RegisterWindowMessageW("TaskbarCreated");
        _showExistingWindowMsg = RegisterWindowMessageW(App.ShowExistingWindowMessageName);

        if (File.Exists(iconPath))
        {
            _hIcon = LoadImageW(nint.Zero, iconPath, IMAGE_ICON, 32, 32, LR_LOADFROMFILE | LR_DEFAULTSIZE);
        }
        if (_hIcon == nint.Zero)
        {
            _hIcon = LoadIconW(nint.Zero, (nint)32512); // IDI_APPLICATION fallback
        }

        _subclassProc = SubclassWndProc;
        SetWindowSubclass(_hwnd, _subclassProc, SubclassId, 0);
        AddIcon(tooltip);
    }

    public void BringToForeground()
    {
        if (_disposed) return;
        ShowWindow(_hwnd, 9); // SW_RESTORE
        SetForegroundWindow(_hwnd);
    }

    public void UpdateTooltip(string tooltip)
    {
        if (_disposed || !_added) return;
        var nid = CreateNotifyData(tooltip);
        nid.uFlags = NIF_TIP;
        Shell_NotifyIconW(NIM_MODIFY, ref nid);
    }

    private void AddIcon(string tooltip)
    {
        var del = CreateNotifyData("");
        Shell_NotifyIconW(NIM_DELETE, ref del);

        var nid = CreateNotifyData(tooltip);
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        _added = Shell_NotifyIconW(NIM_ADD, ref nid);
    }

    private NOTIFYICONDATAW CreateNotifyData(string tooltip)
    {
        return new NOTIFYICONDATAW
        {
            cbSize = (uint)Marshal.SizeOf<NOTIFYICONDATAW>(),
            hWnd = _hwnd,
            uID = 1,
            uCallbackMessage = WM_TRAYICON,
            hIcon = _hIcon,
            szTip = tooltip.Length > 127 ? tooltip[..127] : tooltip,
        };
    }

    private nint SubclassWndProc(nint hWnd, uint uMsg, nuint wParam, nint lParam, nuint uIdSubclass, nuint dwRefData)
    {
        if (uMsg == WM_TRAYICON)
        {
            uint mouseEvent = (uint)(lParam.ToInt64() & 0xFFFF);
            if (mouseEvent == WM_LBUTTONUP || mouseEvent == WM_LBUTTONDBLCLK)
            {
                OpenRequested?.Invoke();
                return nint.Zero;
            }
            if (mouseEvent == WM_RBUTTONUP || mouseEvent == WM_CONTEXTMENU)
            {
                ShowContextMenu();
                return nint.Zero;
            }
        }
        else if (_showExistingWindowMsg != 0 && uMsg == _showExistingWindowMsg)
        {
            OpenRequested?.Invoke();
            return nint.Zero;
        }
        else if (_taskbarCreatedMsg != 0 && uMsg == _taskbarCreatedMsg)
        {
            AddIcon("WifiCam Studio — Running in background");
        }

        return DefSubclassProc(hWnd, uMsg, wParam, lParam);
    }

    private void ShowContextMenu()
    {
        nint menu = CreatePopupMenu();
        if (menu == nint.Zero) return;
        try
        {
            AppendMenuW(menu, MF_STRING, CMD_OPEN, "Open WifiCam Studio");
            AppendMenuW(menu, MF_STRING, CMD_MUTE, "Mute / Unmute Mic");
            AppendMenuW(menu, MF_STRING, CMD_PAUSE, "Pause / Resume Video");
            AppendMenuW(menu, MF_SEPARATOR, 0, null);
            AppendMenuW(menu, MF_STRING, CMD_EXIT, "Exit WifiCam Studio");

            GetCursorPos(out var pt);
            SetForegroundWindow(_hwnd);
            uint cmd = TrackPopupMenu(
                menu,
                TPM_RIGHTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.X,
                pt.Y,
                0,
                _hwnd,
                nint.Zero);
            PostMessageW(_hwnd, 0, 0, nint.Zero);

            switch (cmd)
            {
                case CMD_OPEN:
                    OpenRequested?.Invoke();
                    break;
                case CMD_MUTE:
                    ToggleMuteRequested?.Invoke();
                    break;
                case CMD_PAUSE:
                    TogglePauseRequested?.Invoke();
                    break;
                case CMD_EXIT:
                    ExitRequested?.Invoke();
                    break;
            }
        }
        finally
        {
            DestroyMenu(menu);
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        if (_added)
        {
            var nid = new NOTIFYICONDATAW
            {
                cbSize = (uint)Marshal.SizeOf<NOTIFYICONDATAW>(),
                hWnd = _hwnd,
                uID = 1,
            };
            Shell_NotifyIconW(NIM_DELETE, ref nid);
            _added = false;
        }

        RemoveWindowSubclass(_hwnd, _subclassProc, SubclassId);
        if (_hIcon != nint.Zero)
        {
            DestroyIcon(_hIcon);
            _hIcon = nint.Zero;
        }
    }

    private delegate nint SubclassProcDelegate(
        nint hWnd,
        uint uMsg,
        nuint wParam,
        nint lParam,
        nuint uIdSubclass,
        nuint dwRefData);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NOTIFYICONDATAW
    {
        public uint cbSize;
        public nint hWnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public nint hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
        public string szTip;
        public uint dwState;
        public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
        public string szInfo;
        public uint uTimeoutOrVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
        public string szInfoTitle;
        public uint dwInfoFlags;
        public Guid guidItem;
        public nint hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT
    {
        public int X;
        public int Y;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern bool Shell_NotifyIconW(uint dwMessage, ref NOTIFYICONDATAW lpData);

    [DllImport("comctl32.dll")]
    private static extern bool SetWindowSubclass(nint hWnd, SubclassProcDelegate pfnSubclass, nuint uIdSubclass, nuint dwRefData);

    [DllImport("comctl32.dll")]
    private static extern bool RemoveWindowSubclass(nint hWnd, SubclassProcDelegate pfnSubclass, nuint uIdSubclass);

    [DllImport("comctl32.dll")]
    private static extern nint DefSubclassProc(nint hWnd, uint uMsg, nuint wParam, nint lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern nint LoadImageW(nint hInst, string name, uint type, int cx, int cy, uint fuLoad);

    [DllImport("user32.dll")]
    private static extern nint LoadIconW(nint hInstance, nint lpIconName);

    [DllImport("user32.dll")]
    private static extern bool DestroyIcon(nint hIcon);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint RegisterWindowMessageW(string lpString);

    [DllImport("user32.dll")]
    private static extern nint CreatePopupMenu();

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool AppendMenuW(nint hMenu, uint uFlags, nuint uIDNewItem, string? lpNewItem);

    [DllImport("user32.dll")]
    private static extern uint TrackPopupMenu(nint hMenu, uint uFlags, int x, int y, int nReserved, nint hWnd, nint prcRect);

    [DllImport("user32.dll")]
    private static extern bool DestroyMenu(nint hMenu);

    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT lpPoint);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(nint hWnd);

    [DllImport("user32.dll")]
    private static extern bool ShowWindow(nint hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    private static extern bool PostMessageW(nint hWnd, uint msg, nuint wParam, nint lParam);
}
