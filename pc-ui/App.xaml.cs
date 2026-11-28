using System.Runtime.InteropServices;
using System.Threading;
using Microsoft.UI.Xaml;

namespace WifiCam.Desktop;

public partial class App : Application
{
    private const string SingleInstanceMutexName = "Local\\WifiCamStudio_SingleInstance_Mutex";
    internal const string ShowExistingWindowMessageName = "WifiCamStudio_ShowExistingWindow";
    private static readonly nint HwndBroadcast = (nint)0xFFFF;

    private static Mutex? _instanceMutex;
    private Window? _window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _instanceMutex = new Mutex(initiallyOwned: true, SingleInstanceMutexName, out bool createdNew);
        if (!createdNew)
        {
            // Another instance is already running (likely minimized in System Tray) — wake it and exit immediately.
            uint showMsg = RegisterWindowMessageW(ShowExistingWindowMessageName);
            if (showMsg != 0)
            {
                PostMessageW(HwndBroadcast, showMsg, 0, nint.Zero);
            }
            _instanceMutex.Dispose();
            _instanceMutex = null;
            Environment.Exit(0);
            return;
        }

        _window = new MainWindow();
        _window.Closed += (_, _) =>
        {
            try { _instanceMutex?.ReleaseMutex(); } catch { }
            _instanceMutex?.Dispose();
            _instanceMutex = null;
        };
        _window.Activate();
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint RegisterWindowMessageW(string lpString);

    [DllImport("user32.dll")]
    private static extern bool PostMessageW(nint hWnd, uint msg, nuint wParam, nint lParam);
}
