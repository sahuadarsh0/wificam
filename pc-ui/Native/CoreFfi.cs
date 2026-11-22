using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace WifiCam.Desktop.Native;

public sealed class StudioTelemetry
{
    [JsonPropertyName("running")] public bool Running { get; set; }
    [JsonPropertyName("active_tier")] public string ActiveTier { get; set; } = "Standby";
    [JsonPropertyName("video_paused")] public bool VideoPaused { get; set; }
    [JsonPropertyName("session_locked")] public bool SessionLocked { get; set; }
    [JsonPropertyName("mic_muted")] public bool MicMuted { get; set; }
    [JsonPropertyName("aec_enabled")] public bool AecEnabled { get; set; }
    [JsonPropertyName("denoise_mode")] public byte DenoiseMode { get; set; } = 1;
    [JsonPropertyName("gain_db_x10")] public short GainDbX10 { get; set; }
    [JsonPropertyName("peak_dbfs_x10")] public short PeakDbfsX10 { get; set; } = -960;
    [JsonPropertyName("rms_dbfs_x10")] public short RmsDbfsX10 { get; set; } = -960;
    [JsonPropertyName("vu_segments_16")] public byte VuSegments16 { get; set; }
    [JsonPropertyName("vad_pct")] public byte VadPct { get; set; }
    [JsonPropertyName("skew_ppm_x10")] public short SkewPpmX10 { get; set; }
    [JsonPropertyName("video_width")] public uint VideoWidth { get; set; } = 1920;
    [JsonPropertyName("video_height")] public uint VideoHeight { get; set; } = 1080;
    [JsonPropertyName("video_frames_rx")] public ulong VideoFramesRx { get; set; }
    [JsonPropertyName("audio_packets_rx")] public ulong AudioPacketsRx { get; set; }
    [JsonPropertyName("mf_vcam_supported")] public bool MfVcamSupported { get; set; }
    [JsonPropertyName("vcam_active")] public bool VcamActive { get; set; }
    [JsonPropertyName("vcam_status")] public string VcamStatus { get; set; } = "";
    [JsonPropertyName("host_ip")] public string HostIp { get; set; } = "127.0.0.1";
    [JsonPropertyName("control_port")] public ushort ControlPort { get; set; } = 7890;
    [JsonPropertyName("media_udp_port")] public ushort MediaUdpPort { get; set; } = 7891;
    [JsonPropertyName("host_fingerprint")] public string HostFingerprint { get; set; } = "";
    [JsonPropertyName("pairing_uri")] public string PairingUri { get; set; } = "";
    [JsonPropertyName("virtual_mic_name")] public string VirtualMicName { get; set; } = "";
}

internal static partial class CoreNativeMethods
{
    private const string DllName = "pc_core_ffi.dll";

    [LibraryImport(DllName)]
    internal static partial nint wificam_engine_create();

    [LibraryImport(DllName)]
    internal static partial void wificam_engine_destroy(nint handle);

    [LibraryImport(DllName)]
    internal static partial int wificam_engine_start(nint handle);

    [LibraryImport(DllName)]
    internal static partial void wificam_engine_stop(nint handle);

    [LibraryImport(DllName)]
    internal static partial nint wificam_get_telemetry_json(nint handle);

    [LibraryImport(DllName)]
    internal static partial nint wificam_trigger_adb_reverse(nint handle);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_muted(nint handle, [MarshalAs(UnmanagedType.I1)] bool muted);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_video_paused(nint handle, [MarshalAs(UnmanagedType.I1)] bool paused);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_session_locked(nint handle, [MarshalAs(UnmanagedType.I1)] bool locked);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_aec_enabled(nint handle, [MarshalAs(UnmanagedType.I1)] bool enabled);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_denoise_mode(nint handle, byte mode);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_gain_db_x10(nint handle, short gainDbX10);

    [LibraryImport(DllName)]
    internal static partial void wificam_set_video_transform(
        nint handle,
        ushort rotationDeg,
        [MarshalAs(UnmanagedType.I1)] bool flipH,
        [MarshalAs(UnmanagedType.I1)] bool flipV,
        ushort cropZoomX100);

    [LibraryImport(DllName)]
    internal static unsafe partial nuint wificam_copy_preview_bgra(
        nint handle,
        byte* dstBuf,
        nuint dstCap,
        ulong lastSeq,
        out uint outW,
        out uint outH,
        out ulong outSeq);

    [LibraryImport(DllName)]
    internal static partial void wificam_free_string(nint ptr);
}

public sealed class StudioEngineClient : IDisposable
{
    private nint _handle;

    public StudioEngineClient()
    {
        _handle = CoreNativeMethods.wificam_engine_create();
        if (_handle == 0)
        {
            throw new InvalidOperationException("Failed to initialize Rust pc_core_ffi engine.");
        }
    }

    public bool Start() => _handle != 0 && CoreNativeMethods.wificam_engine_start(_handle) == 0;

    public void Stop()
    {
        if (_handle != 0) CoreNativeMethods.wificam_engine_stop(_handle);
    }

    public StudioTelemetry GetTelemetry()
    {
        if (_handle == 0) return new StudioTelemetry();
        var ptr = CoreNativeMethods.wificam_get_telemetry_json(_handle);
        if (ptr == 0) return new StudioTelemetry();
        try
        {
            var json = Marshal.PtrToStringUTF8(ptr) ?? "{}";
            return JsonSerializer.Deserialize<StudioTelemetry>(json) ?? new StudioTelemetry();
        }
        finally
        {
            CoreNativeMethods.wificam_free_string(ptr);
        }
    }

    public string TriggerAdbReverse()
    {
        if (_handle == 0) return "ERR: Engine not initialized";
        var ptr = CoreNativeMethods.wificam_trigger_adb_reverse(_handle);
        if (ptr == 0) return "ERR: Empty response";
        try
        {
            return Marshal.PtrToStringUTF8(ptr) ?? "ERR: Unknown";
        }
        finally
        {
            CoreNativeMethods.wificam_free_string(ptr);
        }
    }

    public void SetMuted(bool muted)
    {
        if (_handle != 0) CoreNativeMethods.wificam_set_muted(_handle, muted);
    }

    public void SetVideoPaused(bool paused)
    {
        if (_handle != 0) CoreNativeMethods.wificam_set_video_paused(_handle, paused);
    }

    public void SetSessionLocked(bool locked)
    {
        if (_handle != 0) CoreNativeMethods.wificam_set_session_locked(_handle, locked);
    }

    public void SetAecEnabled(bool enabled)
    {
        if (_handle != 0) CoreNativeMethods.wificam_set_aec_enabled(_handle, enabled);
    }

    public void SetDenoiseMode(byte mode)
    {
        if (_handle != 0) CoreNativeMethods.wificam_set_denoise_mode(_handle, mode);
    }

    public void SetGainDbX10(short gainDbX10)
    {
        if (_handle != 0) CoreNativeMethods.wificam_set_gain_db_x10(_handle, gainDbX10);
    }

    public void SetVideoTransform(ushort rotationDeg, bool flipH, bool flipV, ushort cropZoomX100)
    {
        if (_handle != 0)
        {
            CoreNativeMethods.wificam_set_video_transform(_handle, rotationDeg, flipH, flipV, cropZoomX100);
        }
    }

    public unsafe bool TryCopyPreviewBgra(
        byte[] buffer,
        ulong lastSeq,
        out int width,
        out int height,
        out ulong seq,
        out int bytesCopied)
    {
        width = 0;
        height = 0;
        seq = lastSeq;
        bytesCopied = 0;
        if (_handle == 0 || buffer.Length == 0) return false;

        fixed (byte* ptr = buffer)
        {
            nuint copied = CoreNativeMethods.wificam_copy_preview_bgra(
                _handle,
                ptr,
                (nuint)buffer.Length,
                lastSeq,
                out uint w,
                out uint h,
                out ulong s);
            if (copied == 0) return false;
            width = (int)w;
            height = (int)h;
            seq = s;
            bytesCopied = (int)copied;
            return true;
        }
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            CoreNativeMethods.wificam_engine_destroy(_handle);
            _handle = 0;
        }
    }
}
