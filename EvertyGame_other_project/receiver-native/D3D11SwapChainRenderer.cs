using SharpGen.Runtime;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using Vortice;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using Vortice.Mathematics;

namespace ReceiverNative;

internal sealed class D3D11SwapChainRenderer : IDisposable
{
    private readonly Control _playbackHost;
    private readonly Panel _surface = new()
    {
        Dock = DockStyle.Fill,
        BackColor = System.Drawing.Color.Black,
        Margin = Padding.Empty,
    };
    private readonly object _sync = new();

    private ID3D11Device? _device;
    private ID3D11DeviceContext? _deviceContext;
    private ID3D11VideoDevice? _videoDevice;
    private ID3D11VideoContext? _videoContext;
    private IDXGIFactory2? _factory;
    private IDXGISwapChain1? _swapChain;
    private ID3D11Texture2D? _backBuffer;
    private ID3D11RenderTargetView? _renderTargetView;
    private ID3D11VideoProcessorEnumerator? _videoProcessorEnumerator;
    private ID3D11VideoProcessor? _videoProcessor;
    private ID3D11VideoProcessorOutputView? _outputView;
    private ID3D11Texture2D? _cpuUploadTexture;
    private ID3D11Texture2D? _overlayCpuUploadTexture;
    private Format _processorInputFormat = Format.Unknown;
    private int _processorInputWidth;
    private int _processorInputHeight;
    private int _lastSwapChainWidth;
    private int _lastSwapChainHeight;
    private bool _allowTearing;
    private bool _disposed;
    private volatile int _targetWidth;
    private volatile int _targetHeight;

    public event Action<long>? FramePresented;

    public D3D11SwapChainRenderer(Control playbackHost)
    {
        _playbackHost = playbackHost;
        RunOnUiThread(() =>
        {
            _playbackHost.Controls.Clear();
            _playbackHost.Controls.Add(_surface);
            _surface.HandleCreated += OnSurfaceHandleCreated;
            _surface.HandleDestroyed += OnSurfaceHandleDestroyed;
            _surface.Resize += OnSurfaceResize;
            _targetWidth = Math.Max(1, _surface.ClientSize.Width);
            _targetHeight = Math.Max(1, _surface.ClientSize.Height);
        });
    }

    public ID3D11Device Device
    {
        get
        {
            EnsureSurfaceReady();
            lock (_sync)
            {
                if (!EnsureInitialized() || _device is null)
                {
                    throw new InvalidOperationException("D3D11 device not initialized");
                }

                return _device;
            }
        }
    }

    public void Clear()
    {
        EnsureSurfaceReady();
        lock (_sync)
        {
            if (!EnsureInitialized())
            {
                return;
            }

            ResizeSwapChainIfNeeded();
            _deviceContext!.ClearRenderTargetView(_renderTargetView!, new Color4(0f, 0f, 0f, 1f));
            Present();
        }
    }

    public void RenderGpuTexture(ID3D11Texture2D texture, uint subresourceIndex, Format format, int sourceWidth, int sourceHeight)
    {
        EnsureSurfaceReady();
        lock (_sync)
        {
            if (!EnsureInitialized())
            {
                return;
            }

            ResizeSwapChainIfNeeded();
            EnsureVideoProcessor(format, sourceWidth, sourceHeight);

            var inputDesc = new VideoProcessorInputViewDescription
            {
                FourCC = 0,
                ViewDimension = VideoProcessorInputViewDimension.Texture2D,
                Texture2D = new Texture2DVideoProcessorInputView
                {
                    MipSlice = 0,
                    ArraySlice = subresourceIndex,
                },
            };
            using var inputView = _videoDevice!.CreateVideoProcessorInputView(texture, _videoProcessorEnumerator!, inputDesc);
            RenderInputView(inputView, sourceWidth, sourceHeight);
        }
    }

    public void RenderCpuArgbFrame(IntPtr data, int width, int height, int stride)
    {
        EnsureSurfaceReady();
        lock (_sync)
        {
            if (!EnsureInitialized())
            {
                return;
            }

            ResizeSwapChainIfNeeded();
            EnsureCpuUploadTexture(ref _cpuUploadTexture, width, height);
            _deviceContext!.UpdateSubresource(_cpuUploadTexture!, 0, null, data, (uint)stride, 0);
            EnsureVideoProcessor(Format.B8G8R8A8_UNorm, width, height);

            var inputDesc = new VideoProcessorInputViewDescription
            {
                FourCC = 0,
                ViewDimension = VideoProcessorInputViewDimension.Texture2D,
                Texture2D = new Texture2DVideoProcessorInputView
                {
                    MipSlice = 0,
                    ArraySlice = 0,
                },
            };
            using var inputView = _videoDevice!.CreateVideoProcessorInputView(_cpuUploadTexture!, _videoProcessorEnumerator!, inputDesc);
            RenderInputView(inputView, width, height);
        }
    }

    public void RenderCpuCompositeFrame(
        byte[] baseBytes,
        int baseWidth,
        int baseHeight,
        int baseStride,
        byte[]? overlayBytes,
        int overlayWidth,
        int overlayHeight,
        int overlayStride,
        RawRect overlaySourceRect)
    {
        EnsureSurfaceReady();
        lock (_sync)
        {
            if (!EnsureInitialized())
            {
                return;
            }

            ResizeSwapChainIfNeeded();
            var baseHandle = GCHandle.Alloc(baseBytes, GCHandleType.Pinned);
            GCHandle overlayHandle = default;
            try
            {
                EnsureCpuUploadTexture(ref _cpuUploadTexture, baseWidth, baseHeight);
                _deviceContext!.UpdateSubresource(_cpuUploadTexture!, 0, null, baseHandle.AddrOfPinnedObject(), (uint)baseStride, 0);
                EnsureVideoProcessor(Format.B8G8R8A8_UNorm, baseWidth, baseHeight);
                using (var inputView = _videoDevice!.CreateVideoProcessorInputView(_cpuUploadTexture!, _videoProcessorEnumerator!, BuildInputViewDescription()))
                {
                    RenderInputView(inputView, baseWidth, baseHeight, CalculateTargetRect(baseWidth, baseHeight, _lastSwapChainWidth, _lastSwapChainHeight), clear: true, present: false);
                }

                if (overlayBytes is not null && overlayWidth > 0 && overlayHeight > 0 && !IsEmptyRect(overlaySourceRect))
                {
                    overlayHandle = GCHandle.Alloc(overlayBytes, GCHandleType.Pinned);
                    EnsureCpuUploadTexture(ref _overlayCpuUploadTexture, overlayWidth, overlayHeight);
                    _deviceContext.UpdateSubresource(_overlayCpuUploadTexture!, 0, null, overlayHandle.AddrOfPinnedObject(), (uint)overlayStride, 0);
                    EnsureVideoProcessor(Format.B8G8R8A8_UNorm, overlayWidth, overlayHeight);
                    using var overlayView = _videoDevice.CreateVideoProcessorInputView(_overlayCpuUploadTexture!, _videoProcessorEnumerator!, BuildInputViewDescription());
                    var targetRect = MapSourceRectToTargetRect(baseWidth, baseHeight, overlaySourceRect);
                    RenderInputView(overlayView, overlayWidth, overlayHeight, targetRect, clear: false, present: false);
                }

                Present();
            }
            finally
            {
                if (overlayHandle.IsAllocated)
                {
                    overlayHandle.Free();
                }
                baseHandle.Free();
            }
        }
    }

    public void RenderCpuArgbOverlayFrame(
        byte[] overlayBytes,
        int overlayWidth,
        int overlayHeight,
        int overlayStride,
        int baseWidth,
        int baseHeight,
        RawRect overlaySourceRect)
    {
        EnsureSurfaceReady();
        lock (_sync)
        {
            if (!EnsureInitialized() || overlayBytes.Length == 0 || overlayWidth <= 0 || overlayHeight <= 0 || IsEmptyRect(overlaySourceRect))
            {
                return;
            }

            ResizeSwapChainIfNeeded();
            var overlayHandle = GCHandle.Alloc(overlayBytes, GCHandleType.Pinned);
            try
            {
                EnsureCpuUploadTexture(ref _overlayCpuUploadTexture, overlayWidth, overlayHeight);
                _deviceContext!.UpdateSubresource(_overlayCpuUploadTexture!, 0, null, overlayHandle.AddrOfPinnedObject(), (uint)overlayStride, 0);
                EnsureVideoProcessor(Format.B8G8R8A8_UNorm, overlayWidth, overlayHeight);
                using var overlayView = _videoDevice!.CreateVideoProcessorInputView(_overlayCpuUploadTexture!, _videoProcessorEnumerator!, BuildInputViewDescription());
                var targetRect = MapSourceRectToTargetRect(baseWidth, baseHeight, overlaySourceRect);
                RenderInputView(overlayView, overlayWidth, overlayHeight, targetRect, clear: false, present: true);
            }
            finally
            {
                overlayHandle.Free();
            }
        }
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        lock (_sync)
        {
            DisposeResources();
        }

        RunOnUiThread(() =>
        {
            _surface.HandleCreated -= OnSurfaceHandleCreated;
            _surface.HandleDestroyed -= OnSurfaceHandleDestroyed;
            _surface.Resize -= OnSurfaceResize;
            if (_surface.Parent == _playbackHost)
            {
                _playbackHost.Controls.Remove(_surface);
            }
            _surface.Dispose();
        });
    }

    private void OnSurfaceHandleCreated(object? sender, EventArgs e)
    {
        _targetWidth = Math.Max(1, _surface.ClientSize.Width);
        _targetHeight = Math.Max(1, _surface.ClientSize.Height);
    }

    private void OnSurfaceHandleDestroyed(object? sender, EventArgs e)
    {
        lock (_sync)
        {
            ReleaseSwapChainResources();
        }
    }

    private void OnSurfaceResize(object? sender, EventArgs e)
    {
        _targetWidth = Math.Max(1, _surface.ClientSize.Width);
        _targetHeight = Math.Max(1, _surface.ClientSize.Height);
    }

    private bool EnsureInitialized()
    {
        if (_disposed || !_surface.IsHandleCreated)
        {
            return false;
        }

        if (_device is not null && _swapChain is not null)
        {
            return true;
        }

        var featureLevels = new[]
        {
            FeatureLevel.Level_11_1,
            FeatureLevel.Level_11_0,
            FeatureLevel.Level_10_1,
        };
        var creationFlags = DeviceCreationFlags.BgraSupport | DeviceCreationFlags.VideoSupport;
        _device = D3D11.D3D11CreateDevice(DriverType.Hardware, creationFlags, featureLevels);
        _deviceContext = _device.ImmediateContext;

        using var multithread = _deviceContext.QueryInterfaceOrNull<ID3D11Multithread>();
        multithread?.SetMultithreadProtected(true);

        _videoDevice = _device.QueryInterface<ID3D11VideoDevice>();
        _videoContext = _deviceContext.QueryInterface<ID3D11VideoContext>();
        _factory = DXGI.CreateDXGIFactory2<IDXGIFactory2>(false);
        using var factory5 = _factory.QueryInterfaceOrNull<IDXGIFactory5>();
        _allowTearing = factory5?.PresentAllowTearing ?? false;
        CreateSwapChain();
        return true;
    }

    private void CreateSwapChain()
    {
        ReleaseSwapChainResources();

        var width = Math.Max(1, _targetWidth);
        var height = Math.Max(1, _targetHeight);
        var flags = _allowTearing ? SwapChainFlags.AllowTearing : SwapChainFlags.None;
        var swapChainDesc = new SwapChainDescription1(
            (uint)width,
            (uint)height,
            Format.B8G8R8A8_UNorm,
            false,
            Usage.RenderTargetOutput,
            2,
            Scaling.Stretch,
            SwapEffect.FlipDiscard,
            AlphaMode.Ignore,
            flags);

        _swapChain = _factory!.CreateSwapChainForHwnd(_device!, _surface.Handle, swapChainDesc, null, null);
        _backBuffer = _swapChain.GetBuffer<ID3D11Texture2D>(0);
        _renderTargetView = _device!.CreateRenderTargetView(_backBuffer);
        _lastSwapChainWidth = width;
        _lastSwapChainHeight = height;
        RecreateVideoProcessorForTarget();
        _deviceContext!.ClearRenderTargetView(_renderTargetView!, new Color4(0f, 0f, 0f, 1f));
        Present();
    }

    private void ResizeSwapChainIfNeeded()
    {
        if (_swapChain is null)
        {
            return;
        }

        var width = Math.Max(1, _targetWidth);
        var height = Math.Max(1, _targetHeight);
        if (width == _lastSwapChainWidth && height == _lastSwapChainHeight)
        {
            return;
        }

        _renderTargetView?.Dispose();
        _outputView?.Dispose();
        _backBuffer?.Dispose();
        _renderTargetView = null;
        _outputView = null;
        _backBuffer = null;

        var flags = _allowTearing ? SwapChainFlags.AllowTearing : SwapChainFlags.None;
        _swapChain.ResizeBuffers(2, (uint)width, (uint)height, Format.B8G8R8A8_UNorm, flags);
        _backBuffer = _swapChain.GetBuffer<ID3D11Texture2D>(0);
        _renderTargetView = _device!.CreateRenderTargetView(_backBuffer);
        _lastSwapChainWidth = width;
        _lastSwapChainHeight = height;
        RecreateVideoProcessorForTarget();
    }

    private void EnsureVideoProcessor(Format inputFormat, int sourceWidth, int sourceHeight)
    {
        if (_videoProcessorEnumerator is not null &&
            _videoProcessor is not null &&
            _outputView is not null &&
            _processorInputFormat == inputFormat &&
            _processorInputWidth == sourceWidth &&
            _processorInputHeight == sourceHeight)
        {
            return;
        }

        _videoProcessor?.Dispose();
        _videoProcessorEnumerator?.Dispose();
        _outputView?.Dispose();
        _videoProcessor = null;
        _videoProcessorEnumerator = null;
        _outputView = null;

        var desc = new VideoProcessorContentDescription
        {
            InputFrameFormat = VideoFrameFormat.Progressive,
            InputFrameRate = new Rational(60, 1),
            InputWidth = (uint)Math.Max(1, sourceWidth),
            InputHeight = (uint)Math.Max(1, sourceHeight),
            OutputFrameRate = new Rational(60, 1),
            OutputWidth = (uint)Math.Max(1, _lastSwapChainWidth),
            OutputHeight = (uint)Math.Max(1, _lastSwapChainHeight),
            Usage = VideoUsage.OptimalSpeed,
        };
        _videoProcessorEnumerator = _videoDevice!.CreateVideoProcessorEnumerator(desc);
        _videoProcessor = _videoDevice.CreateVideoProcessor(_videoProcessorEnumerator, 0);

        var outputDesc = new VideoProcessorOutputViewDescription
        {
            ViewDimension = VideoProcessorOutputViewDimension.Texture2D,
            Texture2D = new Texture2DVideoProcessorOutputView
            {
                MipSlice = 0,
            },
        };
        _outputView = _videoDevice.CreateVideoProcessorOutputView(_backBuffer!, _videoProcessorEnumerator, outputDesc);
        _videoContext!.VideoProcessorSetStreamFrameFormat(_videoProcessor, 0, VideoFrameFormat.Progressive);
        _processorInputFormat = inputFormat;
        _processorInputWidth = sourceWidth;
        _processorInputHeight = sourceHeight;
    }

    private void RecreateVideoProcessorForTarget()
    {
        if (_processorInputWidth <= 0 || _processorInputHeight <= 0 || _processorInputFormat == Format.Unknown)
        {
            _videoProcessor?.Dispose();
            _videoProcessorEnumerator?.Dispose();
            _outputView?.Dispose();
            _videoProcessor = null;
            _videoProcessorEnumerator = null;
            _outputView = null;
            return;
        }

        EnsureVideoProcessor(_processorInputFormat, _processorInputWidth, _processorInputHeight);
    }

    private void EnsureCpuUploadTexture(ref ID3D11Texture2D? texture, int width, int height)
    {
        if (texture is not null)
        {
            var description = texture.Description;
            if (description.Width == width && description.Height == height)
            {
                return;
            }

            texture.Dispose();
            texture = null;
        }

        var textureDesc = new Texture2DDescription(
            Format.B8G8R8A8_UNorm,
            (uint)Math.Max(1, width),
            (uint)Math.Max(1, height),
            1,
            1,
            BindFlags.ShaderResource,
            ResourceUsage.Default,
            CpuAccessFlags.None,
            1,
            0,
            ResourceOptionFlags.None);
        texture = _device!.CreateTexture2D(textureDesc);
    }

    private void RenderInputView(ID3D11VideoProcessorInputView inputView, int sourceWidth, int sourceHeight)
    {
        var targetRect = CalculateTargetRect(sourceWidth, sourceHeight, _lastSwapChainWidth, _lastSwapChainHeight);
        RenderInputView(inputView, sourceWidth, sourceHeight, targetRect, clear: true, present: true);
    }

    private void RenderInputView(
        ID3D11VideoProcessorInputView inputView,
        int sourceWidth,
        int sourceHeight,
        RawRect targetRect,
        bool clear,
        bool present)
    {
        var sourceRect = new RawRect(0, 0, Math.Max(1, sourceWidth), Math.Max(1, sourceHeight));

        if (clear)
        {
            _deviceContext!.ClearRenderTargetView(_renderTargetView!, new Color4(0f, 0f, 0f, 1f));
        }
        _videoContext!.VideoProcessorSetOutputTargetRect(_videoProcessor!, true, targetRect);
        _videoContext.VideoProcessorSetStreamSourceRect(_videoProcessor, 0, true, sourceRect);
        _videoContext.VideoProcessorSetStreamDestRect(_videoProcessor, 0, true, targetRect);

        var stream = new VideoProcessorStream
        {
            Enable = true,
            OutputIndex = 0,
            InputFrameOrField = 0,
            PastFrames = 0,
            FutureFrames = 0,
            InputSurface = inputView,
        };
        _videoContext.VideoProcessorBlt(_videoProcessor!, _outputView!, 0, new[] { stream }).CheckError();
        if (present)
        {
            Present();
        }
    }

    public RawRect MapSourceRectToTargetRect(int sourceWidth, int sourceHeight, RawRect sourceRect)
    {
        var targetRect = CalculateTargetRect(sourceWidth, sourceHeight, _lastSwapChainWidth, _lastSwapChainHeight);
        if (sourceWidth <= 0 || sourceHeight <= 0)
        {
            return targetRect;
        }

        var scaleX = (targetRect.Right - targetRect.Left) / (float)sourceWidth;
        var scaleY = (targetRect.Bottom - targetRect.Top) / (float)sourceHeight;
        return new RawRect(
            targetRect.Left + (int)Math.Round(sourceRect.Left * scaleX),
            targetRect.Top + (int)Math.Round(sourceRect.Top * scaleY),
            targetRect.Left + (int)Math.Round(sourceRect.Right * scaleX),
            targetRect.Top + (int)Math.Round(sourceRect.Bottom * scaleY));
    }

    private static VideoProcessorInputViewDescription BuildInputViewDescription()
    {
        return new VideoProcessorInputViewDescription
        {
            FourCC = 0,
            ViewDimension = VideoProcessorInputViewDimension.Texture2D,
            Texture2D = new Texture2DVideoProcessorInputView
            {
                MipSlice = 0,
                ArraySlice = 0,
            },
        };
    }

    private void Present()
    {
        if (_swapChain is null)
        {
            return;
        }

        var flags = _allowTearing ? PresentFlags.AllowTearing : PresentFlags.None;
        _swapChain.Present(0, flags).CheckError();
        FramePresented?.Invoke(Stopwatch.GetTimestamp());
    }

    private static RawRect CalculateTargetRect(int sourceWidth, int sourceHeight, int targetWidth, int targetHeight)
    {
        if (sourceWidth <= 0 || sourceHeight <= 0 || targetWidth <= 0 || targetHeight <= 0)
        {
            return new RawRect(0, 0, Math.Max(1, targetWidth), Math.Max(1, targetHeight));
        }

        var scale = Math.Min(targetWidth / (float)sourceWidth, targetHeight / (float)sourceHeight);
        var drawWidth = Math.Max(1, (int)Math.Round(sourceWidth * scale));
        var drawHeight = Math.Max(1, (int)Math.Round(sourceHeight * scale));
        var left = (targetWidth - drawWidth) / 2;
        var top = (targetHeight - drawHeight) / 2;
        return new RawRect(left, top, left + drawWidth, top + drawHeight);
    }

    private static bool IsEmptyRect(RawRect rect) => rect.Right <= rect.Left || rect.Bottom <= rect.Top;

    private void ReleaseSwapChainResources()
    {
        _outputView?.Dispose();
        _outputView = null;
        _renderTargetView?.Dispose();
        _renderTargetView = null;
        _backBuffer?.Dispose();
        _backBuffer = null;
        _swapChain?.Dispose();
        _swapChain = null;
    }

    private void DisposeResources()
    {
        ReleaseSwapChainResources();
        _videoProcessor?.Dispose();
        _videoProcessor = null;
        _videoProcessorEnumerator?.Dispose();
        _videoProcessorEnumerator = null;
        _cpuUploadTexture?.Dispose();
        _cpuUploadTexture = null;
        _overlayCpuUploadTexture?.Dispose();
        _overlayCpuUploadTexture = null;
        _videoContext?.Dispose();
        _videoContext = null;
        _videoDevice?.Dispose();
        _videoDevice = null;
        _deviceContext?.Dispose();
        _deviceContext = null;
        _device?.Dispose();
        _device = null;
        _factory?.Dispose();
        _factory = null;
    }

    private void RunOnUiThread(Action action)
    {
        if (_playbackHost.IsDisposed)
        {
            return;
        }

        if (_playbackHost.IsHandleCreated && _playbackHost.InvokeRequired)
        {
            try
            {
                _playbackHost.Invoke(action);
            }
            catch (ObjectDisposedException)
            {
            }
            catch (InvalidOperationException)
            {
            }
            return;
        }

        action();
    }

    private void EnsureSurfaceReady()
    {
        RunOnUiThread(() =>
        {
            if (_playbackHost.IsDisposed || _surface.IsDisposed)
            {
                return;
            }

            if (_surface.Parent != _playbackHost)
            {
                _playbackHost.Controls.Clear();
                _playbackHost.Controls.Add(_surface);
            }

            if (!_surface.IsHandleCreated)
            {
                _surface.CreateControl();
            }

            _targetWidth = Math.Max(1, _surface.ClientSize.Width);
            _targetHeight = Math.Max(1, _surface.ClientSize.Height);
        });
    }
}
