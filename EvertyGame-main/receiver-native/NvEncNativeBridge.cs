using System.Runtime.InteropServices;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;

namespace ReceiverNative;

internal sealed class NvEncNativeBridge : IDisposable
{
    private const string BridgeFileName = "everty_nvenc_bridge.dll";

    private readonly IntPtr _libraryHandle;
    private readonly IntPtr _sessionHandle;
    private readonly DrainPacketCallback _drainPacketCallback;
    private readonly CreateSessionDelegate _createSession;
    private readonly EncodeFrameDelegate _encodeFrame;
    private readonly DrainPacketsDelegate _drainPackets;
    private readonly ReconfigureDelegate _reconfigure;
    private readonly DestroySessionDelegate _destroySession;
    private readonly GetLastErrorDelegate _getLastError;
    private bool _disposed;

    public NvEncNativeBridge(
        IntPtr d3d11Device,
        int width,
        int height,
        WindowsVideoCodec codec,
        int bitrateBps,
        int fps,
        int gopLength,
        bool gamePreset)
    {
        if (codec.IsAv1())
        {
            throw new NotSupportedException("Native NVENC bridge does not support AV1.");
        }

        if (!NativeLibrary.TryLoad(GetBridgePath(), out _libraryHandle))
        {
            throw new InvalidOperationException($"Failed to load {BridgeFileName}.");
        }

        _createSession = GetExport<CreateSessionDelegate>("create_session");
        _encodeFrame = GetExport<EncodeFrameDelegate>("encode_frame");
        _drainPackets = GetExport<DrainPacketsDelegate>("drain_packets");
        _reconfigure = GetExport<ReconfigureDelegate>("reconfigure");
        _destroySession = GetExport<DestroySessionDelegate>("destroy_session");
        _getLastError = GetExport<GetLastErrorDelegate>("get_last_error");
        _drainPacketCallback = OnDrainPacket;

        _sessionHandle = _createSession(
            d3d11Device,
            width,
            height,
            codec == WindowsVideoCodec.H264Avc ? 0 : 1,
            bitrateBps,
            fps,
            gopLength,
            gamePreset ? 1 : 0);
        if (_sessionHandle == IntPtr.Zero)
        {
            var error = GetLastErrorMessage();
            Dispose();
            throw new InvalidOperationException(string.IsNullOrWhiteSpace(error)
                ? "Native NVENC session creation failed."
                : error);
        }
    }

    public static string GetBridgePath() => Path.Combine(AppContext.BaseDirectory, BridgeFileName);

    public static bool IsBridgePresent() => File.Exists(GetBridgePath());

    public static string? TryProbe(WindowsVideoCodec codec, IDXGIAdapter1? preferredAdapter = null)
    {
        if (codec.IsAv1())
        {
            return "AV1 is not supported by the native NVENC bridge.";
        }

        if (!IsBridgePresent())
        {
            return $"{BridgeFileName} was not found.";
        }

        IDXGIAdapter1? adapter = null;
        ID3D11Device? device = null;
        ID3D11DeviceContext? context = null;

        try
        {
            adapter = preferredAdapter ?? TryGetFirstNvidiaAdapter();
            if (adapter is null)
            {
                return "NVIDIA adapter not found.";
            }

            D3D11.D3D11CreateDevice(
                adapter,
                DriverType.Unknown,
                DeviceCreationFlags.BgraSupport,
                WindowsSenderSession.PreferredFeatureLevels,
                out device,
                out _,
                out context).CheckError();

            using var bridge = new NvEncNativeBridge(
                device.NativePointer,
                width: 1280,
                height: 720,
                codec,
                bitrateBps: 4_000_000,
                fps: 60,
                gopLength: 60,
                gamePreset: true);
            return null;
        }
        catch (Exception ex)
        {
            return ex.Message;
        }
        finally
        {
            context?.Dispose();
            device?.Dispose();
            if (preferredAdapter is null)
            {
                adapter?.Dispose();
            }
        }
    }

    public void EncodeFrame(IntPtr texture, long timestampHns, bool forceIdr)
    {
        ThrowIfDisposed();
        var status = _encodeFrame(_sessionHandle, texture, timestampHns, forceIdr ? 1 : 0);
        if (status != 0)
        {
            ThrowLastError("Native NVENC encode failed.");
        }
    }

    public IReadOnlyList<EncodedPacket> DrainPackets()
    {
        ThrowIfDisposed();
        var packets = new List<EncodedPacket>();
        var state = new DrainState(packets);
        var handle = GCHandle.Alloc(state);
        try
        {
            var status = _drainPackets(_sessionHandle, _drainPacketCallback, GCHandle.ToIntPtr(handle));
            if (status != 0)
            {
                ThrowLastError("Native NVENC drain failed.");
            }

            return packets;
        }
        finally
        {
            handle.Free();
        }
    }

    public void Reconfigure(int bitrateBps, int fps, int gopLength)
    {
        ThrowIfDisposed();
        var status = _reconfigure(_sessionHandle, bitrateBps, fps, gopLength);
        if (status != 0)
        {
            ThrowLastError("Native NVENC reconfigure failed.");
        }
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        if (_sessionHandle != IntPtr.Zero)
        {
            try
            {
                _destroySession(_sessionHandle);
            }
            catch
            {
            }
        }

        if (_libraryHandle != IntPtr.Zero)
        {
            NativeLibrary.Free(_libraryHandle);
        }

        _disposed = true;
        GC.SuppressFinalize(this);
    }

    private void OnDrainPacket(IntPtr payload, int size, long timestampHns, int keyFrame, IntPtr userData)
    {
        if (payload == IntPtr.Zero || size <= 0)
        {
            return;
        }

        var state = (DrainState?)GCHandle.FromIntPtr(userData).Target;
        if (state is null)
        {
            return;
        }

        var managed = new byte[size];
        Marshal.Copy(payload, managed, 0, size);
        state.Packets.Add(new EncodedPacket(managed, timestampHns, keyFrame != 0));
    }

    private string GetLastErrorMessage()
    {
        try
        {
            var pointer = _getLastError();
            return pointer == IntPtr.Zero ? string.Empty : Marshal.PtrToStringAnsi(pointer) ?? string.Empty;
        }
        catch
        {
            return string.Empty;
        }
    }

    private void ThrowLastError(string fallbackMessage)
    {
        var error = GetLastErrorMessage();
        throw new InvalidOperationException(string.IsNullOrWhiteSpace(error) ? fallbackMessage : error);
    }

    private T GetExport<T>(string name) where T : Delegate
    {
        var export = NativeLibrary.GetExport(_libraryHandle, name);
        return Marshal.GetDelegateForFunctionPointer<T>(export);
    }

    private void ThrowIfDisposed()
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
    }

    private static IDXGIAdapter1? TryGetFirstNvidiaAdapter()
    {
        using var factory = DXGI.CreateDXGIFactory1<IDXGIFactory1>();
        for (uint adapterIndex = 0; ; adapterIndex++)
        {
            var adapterResult = factory.EnumAdapters1(adapterIndex, out var adapter);
            if (adapterResult.Failure || adapter is null)
            {
                break;
            }

            var desc = adapter.Description1;
            if (desc.VendorId == 0x10DE)
            {
                return adapter;
            }

            adapter.Dispose();
        }

        return null;
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate IntPtr CreateSessionDelegate(
        IntPtr d3d11Device,
        int width,
        int height,
        int codec,
        int bitrateBps,
        int fps,
        int gopLength,
        int gamePreset);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int EncodeFrameDelegate(IntPtr session, IntPtr texture, long timestampHns, int forceIdr);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int DrainPacketsDelegate(IntPtr session, DrainPacketCallback callback, IntPtr userData);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int ReconfigureDelegate(IntPtr session, int bitrateBps, int fps, int gopLength);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void DestroySessionDelegate(IntPtr session);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate IntPtr GetLastErrorDelegate();

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void DrainPacketCallback(IntPtr payload, int size, long timestampHns, int keyFrame, IntPtr userData);

    private sealed class DrainState(List<EncodedPacket> packets)
    {
        public List<EncodedPacket> Packets { get; } = packets;
    }

    internal readonly record struct EncodedPacket(byte[] Payload, long TimestampHns, bool IsKeyFrame);
}
