using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.Win32;
using QRCoder;
using System.Runtime.InteropServices.WindowsRuntime;
using WifiCam.Desktop.Native;
using Windows.Graphics;
using Windows.Storage.Streams;
using Windows.UI;

namespace WifiCam.Desktop;

public sealed partial class MainWindow : Window
{
    private readonly StudioEngineClient _engine;
    private readonly DispatcherTimer _telemetryTimer;
    private readonly Border[] _vuSegments = new Border[16];
    private readonly byte[] _previewBgraBuf = new byte[640 * 480 * 4];
    private WriteableBitmap? _previewBitmap;
    private ulong _lastPreviewSeq;
    private bool _engineRunning;
    private string _renderedQrUri = "";

    public MainWindow()
    {
        InitializeComponent();

        // Native Windows 11 Mica backdrop + extended title bar + branded icon
        SystemBackdrop = new MicaBackdrop();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);

        var icoPath = System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", "wificam.ico");
        if (System.IO.File.Exists(icoPath))
        {
            AppWindow.SetIcon(icoPath);
        }

        AppWindow.Resize(new SizeInt32(1080, 860));

        BuildVuMeterSegments();

        _engine = new StudioEngineClient();
        SystemEvents.SessionSwitch += OnWindowsSessionSwitch;

        _telemetryTimer = new DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(33), // 30 Hz live video preview + crisp VU refresh
        };
        _telemetryTimer.Tick += (_, _) => RefreshTelemetry();
        _telemetryTimer.Start();

        // Auto-start studio engine on launch so QR & mDNS are immediately ready.
        if (_engine.Start())
        {
            _engineRunning = true;
            UpdateStartStopButton();
        }
        RefreshTelemetry();

        Closed += (_, _) =>
        {
            _telemetryTimer.Stop();
            SystemEvents.SessionSwitch -= OnWindowsSessionSwitch;
            _engine.Dispose();
        };
    }

    private void BuildVuMeterSegments()
    {
        VuMeterGrid.ColumnDefinitions.Clear();
        VuMeterGrid.Children.Clear();
        for (int i = 0; i < 16; i++)
        {
            VuMeterGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var seg = new Border
            {
                CornerRadius = new CornerRadius(3),
                Background = new SolidColorBrush(Color.FromArgb(255, 29, 34, 43)),
            };
            Grid.SetColumn(seg, i);
            VuMeterGrid.Children.Add(seg);
            _vuSegments[i] = seg;
        }
    }

    private void RefreshTelemetry()
    {
        var snap = _engine.GetTelemetry();
        _engineRunning = snap.Running;

        TierBadgeText.Text = snap.ActiveTier.ToUpperInvariant();
        LiveStatusDot.Fill = new SolidColorBrush(
            snap.Running
                ? Color.FromArgb(255, 34, 197, 94)
                : Color.FromArgb(255, 156, 168, 186));

        VideoModeText.Text = $"{snap.VideoWidth}x{snap.VideoHeight} • NV12";
        FramesCounterText.Text = $"VIDEO {snap.VideoFramesRx} f";
        AudioCounterText.Text = $"AUDIO {snap.AudioPacketsRx} pkt";

        ViewportHeadline.Text = snap.SessionLocked
            ? "Privacy Locked (Win+L)"
            : snap.VideoPaused
                ? "Camera Paused"
                : snap.VideoFramesRx > 0
                    ? "Streaming Live to Virtual Camera"
                    : "Waiting for Android Phone";

        // Update live hardware-decoded video preview bitmap
        bool showLivePreview = snap.Running && !snap.VideoPaused && !snap.SessionLocked && snap.VideoFramesRx > 0;
        if (showLivePreview && _engine.TryCopyPreviewBgra(
                _previewBgraBuf,
                _lastPreviewSeq,
                out int pw,
                out int ph,
                out ulong nextSeq,
                out int bytesCopied))
        {
            _lastPreviewSeq = nextSeq;
            if (_previewBitmap == null || _previewBitmap.PixelWidth != pw || _previewBitmap.PixelHeight != ph)
            {
                _previewBitmap = new WriteableBitmap(pw, ph);
                LivePreviewImage.Source = _previewBitmap;
            }
            using (var pixelStream = _previewBitmap.PixelBuffer.AsStream())
            {
                pixelStream.Write(_previewBgraBuf, 0, bytesCopied);
            }
            _previewBitmap.Invalidate();
        }

        bool hasPreviewPixels = showLivePreview && _previewBitmap != null && _lastPreviewSeq > 0;
        LivePreviewImage.Visibility = hasPreviewPixels ? Visibility.Visible : Visibility.Collapsed;
        ViewportSlateOverlay.Visibility = hasPreviewPixels ? Visibility.Collapsed : Visibility.Visible;

        LanAddressText.Text = $"LAN: {snap.HostIp} • TCP {snap.ControlPort} / UDP {snap.MediaUdpPort}";
        FingerprintText.Text = $"TOFU FP: {snap.HostFingerprint}";
        VirtualMicStatusText.Text = $"Virtual Mic Sink: {snap.VirtualMicName}";

        if (string.IsNullOrEmpty(UsbStatusText.Text) ||
            UsbStatusText.Text.StartsWith("Virtual Camera") ||
            UsbStatusText.Text.StartsWith("Active in Apps:"))
        {
            if (!string.IsNullOrEmpty(snap.VcamStatus))
            {
                UsbStatusText.Text = snap.VcamStatus;
                UsbStatusText.Foreground = new SolidColorBrush(
                    snap.VcamActive
                        ? Color.FromArgb(255, 34, 197, 94)
                        : Color.FromArgb(255, 245, 158, 11));
            }
            else
            {
                UsbStatusText.Text = snap.MfVcamSupported
                    ? "Virtual Camera: Win11 Media Foundation (MFCreateVirtualCamera Ready)"
                    : "Virtual Camera: Win10 DirectShow Fallback Ready";
            }
        }

        // Update 16-segment Broadcast LED VU meter
        var lit = snap.MicMuted ? 0 : snap.VuSegments16;
        for (int i = 0; i < 16; i++)
        {
            Color color = (i + 1) > lit
                ? Color.FromArgb(255, 29, 34, 43)
                : i < 10
                    ? Color.FromArgb(255, 34, 197, 94)
                    : i < 14
                        ? Color.FromArgb(255, 245, 158, 11)
                        : Color.FromArgb(255, 239, 68, 68);
            _vuSegments[i].Background = new SolidColorBrush(color);
        }

        VuDbText.Text = snap.MicMuted || snap.RmsDbfsX10 <= -900
            ? (snap.MicMuted ? "MUTED" : "-∞ dBFS")
            : $"{snap.RmsDbfsX10 / 10.0:0.0} dBFS";
        VadText.Text = $"{snap.VadPct} %";
        SkewText.Text = $"{snap.SkewPpmX10 / 10.0:+0.0;-0.0;0.0} ppm";

        if (!string.IsNullOrEmpty(snap.PairingUri) && snap.PairingUri != _renderedQrUri)
        {
            _renderedQrUri = snap.PairingUri;
            _ = RenderQrCodeAsync(snap.PairingUri);
        }
    }

    private async Task RenderQrCodeAsync(string uri)
    {
        using var generator = new QRCodeGenerator();
        using var qrData = generator.CreateQrCode(uri, QRCodeGenerator.ECCLevel.M);
        var pngQr = new PngByteQRCode(qrData);
        var pngBytes = pngQr.GetGraphic(8);

        using var stream = new InMemoryRandomAccessStream();
        await stream.WriteAsync(pngBytes.AsBuffer());
        stream.Seek(0);

        var bmp = new BitmapImage();
        await bmp.SetSourceAsync(stream);
        PairingQrImage.Source = bmp;
    }

    private void UpdateStartStopButton()
    {
        if (_engineRunning)
        {
            StartStopButton.Content = "Stop Studio Engine";
            StartStopButton.Background = new SolidColorBrush(Color.FromArgb(255, 244, 63, 94));
            StartStopButton.Foreground = new SolidColorBrush(Color.FromArgb(255, 248, 250, 252));
        }
        else
        {
            StartStopButton.Content = "Start Studio Engine";
            StartStopButton.Background = new SolidColorBrush(Color.FromArgb(255, 56, 189, 248));
            StartStopButton.Foreground = new SolidColorBrush(Color.FromArgb(255, 6, 16, 24));
        }
    }

    private void OnStartStopClick(object sender, RoutedEventArgs e)
    {
        if (_engineRunning)
        {
            _engine.Stop();
            _engineRunning = false;
        }
        else
        {
            _engineRunning = _engine.Start();
        }
        UpdateStartStopButton();
        RefreshTelemetry();
    }

    private void OnPauseVideoClick(object sender, RoutedEventArgs e)
    {
        _engine.SetVideoPaused(PauseVideoToggle.IsChecked == true);
    }

    private void OnMuteMicClick(object sender, RoutedEventArgs e)
    {
        _engine.SetMuted(MuteMicToggle.IsChecked == true);
    }

    private void OnUsbAdbClick(object sender, RoutedEventArgs e)
    {
        UsbStatusText.Text = _engine.TriggerAdbReverse();
    }

    private void OnDenoiseSelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_engine == null) return;
        byte mode = DenoiseCombo.SelectedIndex switch
        {
            0 => 0, // Off
            2 => 2, // DeepFilterNet3
            _ => 1, // RNNoise
        };
        _engine.SetDenoiseMode(mode);
    }

    private void OnAecToggled(object sender, RoutedEventArgs e)
    {
        _engine?.SetAecEnabled(AecToggle.IsOn);
    }

    private void OnGainChanged(object sender, Microsoft.UI.Xaml.Controls.Primitives.RangeBaseValueChangedEventArgs e)
    {
        if (GainValueText != null)
        {
            GainValueText.Text = $"{e.NewValue:+0.0;-0.0;0.0} dB";
        }
        _engine?.SetGainDbX10((short)Math.Round(e.NewValue * 10.0));
    }

    private void OnWindowsSessionSwitch(object sender, SessionSwitchEventArgs e)
    {
        if (LockPrivacyToggle?.IsOn != true) return;
        if (e.Reason == SessionSwitchReason.SessionLock)
        {
            _engine.SetSessionLocked(true);
        }
        else if (e.Reason == SessionSwitchReason.SessionUnlock)
        {
            _engine.SetSessionLocked(false);
        }
    }
}
