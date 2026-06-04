using NAudio.CoreAudioApi;
using NAudio.Wave;
using System.Text.Json;

namespace ReceiverNative;

internal sealed record ReceiverAudioConfig(string Codec, int SampleRate, int Channels, int BytesPerSample)
{
    public static ReceiverAudioConfig? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<AudioConfigDto>(payload);
            if (dto is null || dto.sampleRate <= 0 || dto.channels <= 0 || dto.bytesPerSample <= 0)
            {
                return null;
            }

            return new ReceiverAudioConfig(
                dto.codec ?? "pcm_s16le",
                dto.sampleRate,
                dto.channels,
                dto.bytesPerSample);
        }
        catch
        {
            return null;
        }
    }

    private sealed class AudioConfigDto
    {
        public string? codec { get; set; }
        public int sampleRate { get; set; }
        public int channels { get; set; }
        public int bytesPerSample { get; set; }
    }
}

internal sealed class AudioFrameReassembler
{
    private readonly Action<byte[]> _onFrameReady;
    private readonly Dictionary<int, AudioFrameAssembly> _frames = new();
    private int _latestFrameIdSeen = -1;

    public AudioFrameReassembler(Action<byte[]> onFrameReady)
    {
        _onFrameReady = onFrameReady;
    }

    public void Reset()
    {
        _frames.Clear();
        _latestFrameIdSeen = -1;
    }

    public void OnPacket(UdpPacket packet)
    {
        if (packet.PacketCount <= 0 || packet.PacketIndex >= packet.PacketCount)
        {
            return;
        }

        if (packet.FrameId > _latestFrameIdSeen)
        {
            foreach (var staleKey in _frames.Keys.Where(key => key < packet.FrameId).ToArray())
            {
                _frames.Remove(staleKey);
            }
            _latestFrameIdSeen = packet.FrameId;
        }

        if (!_frames.TryGetValue(packet.FrameId, out var assembly))
        {
            assembly = new AudioFrameAssembly(packet.PacketCount);
            _frames[packet.FrameId] = assembly;
        }

        if (assembly.PacketCount != packet.PacketCount || assembly.IsSet(packet.PacketIndex))
        {
            return;
        }

        assembly.Set(packet.PacketIndex, packet.Payload);
        if (!assembly.IsComplete)
        {
            return;
        }

        _frames.Remove(packet.FrameId);
        _onFrameReady(assembly.Join());
    }

    private sealed class AudioFrameAssembly
    {
        private readonly byte[][] _parts;
        private int _received;

        public AudioFrameAssembly(int packetCount)
        {
            PacketCount = packetCount;
            _parts = new byte[packetCount][];
        }

        public int PacketCount { get; }
        public bool IsComplete => _received == PacketCount;
        public bool IsSet(int index) => _parts[index] is not null;

        public void Set(int index, byte[] payload)
        {
            _parts[index] = payload;
            _received += 1;
        }

        public byte[] Join()
        {
            using var stream = new MemoryStream();
            foreach (var part in _parts)
            {
                if (part is not null)
                {
                    stream.Write(part, 0, part.Length);
                }
            }

            return stream.ToArray();
        }
    }
}

internal sealed class AudioPlaybackSink : IDisposable
{
    private readonly object _sync = new();
    private IWavePlayer? _waveOut;
    private BufferedWaveProvider? _bufferedWaveProvider;
    private ReceiverAudioConfig? _config;
    private bool _cinemaSmoothMode;
    private int? _manualBufferDurationMs;

    public void UpdateCinemaSmoothMode(bool enabled)
    {
        lock (_sync)
        {
            if (_cinemaSmoothMode == enabled)
            {
                return;
            }

            _cinemaSmoothMode = enabled;
            if (_config is not null)
            {
                var config = _config;
                _config = null;
                DisposePlaybackLocked();
                ApplyConfig(config);
            }
        }
    }

    public void UpdateManualBufferDurationMs(int valueMs)
    {
        lock (_sync)
        {
            int? normalized = valueMs > 0 ? Math.Clamp(valueMs, 80, 1500) : null;
            if (_manualBufferDurationMs == normalized)
            {
                return;
            }

            _manualBufferDurationMs = normalized;
            if (_config is not null)
            {
                var config = _config;
                _config = null;
                DisposePlaybackLocked();
                ApplyConfig(config);
            }
        }
    }

    public void ApplyConfig(ReceiverAudioConfig config)
    {
        if (!string.Equals(config.Codec, "pcm_s16le", StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        lock (_sync)
        {
            if (_config == config && _waveOut is not null && _bufferedWaveProvider is not null)
            {
                return;
            }

            DisposePlaybackLocked();

            var bitsPerSample = Math.Max(8, config.BytesPerSample * 8);
            var waveFormat = new WaveFormat(config.SampleRate, bitsPerSample, config.Channels);
            var provider = new BufferedWaveProvider(waveFormat)
            {
                BufferDuration = TimeSpan.FromMilliseconds(GetEffectiveBufferDurationMs()),
                DiscardOnBufferOverflow = true,
                ReadFully = true,
            };
            var waveOut = CreateWavePlayer(_cinemaSmoothMode);
            waveOut.Init(provider);
            waveOut.Play();

            _config = config;
            _bufferedWaveProvider = provider;
            _waveOut = waveOut;
        }
    }

    public void EnqueuePcmFrame(byte[] payload)
    {
        if (payload.Length == 0)
        {
            return;
        }

        lock (_sync)
        {
            _bufferedWaveProvider?.AddSamples(payload, 0, payload.Length);
        }
    }

    public void Reset()
    {
        lock (_sync)
        {
            _bufferedWaveProvider?.ClearBuffer();
        }
    }

    public void Dispose()
    {
        lock (_sync)
        {
            DisposePlaybackLocked();
            _config = null;
        }
    }

    private void DisposePlaybackLocked()
    {
        try
        {
            _waveOut?.Stop();
        }
        catch
        {
        }

        _waveOut?.Dispose();
        _waveOut = null;
        _bufferedWaveProvider = null;
    }

    private IWavePlayer CreateWavePlayer(bool cinemaSmoothMode)
    {
        return CreateWavePlayerInternal(cinemaSmoothMode, _manualBufferDurationMs);
    }

    private static IWavePlayer CreateWavePlayerInternal(bool cinemaSmoothMode, int? manualBufferDurationMs)
    {
        var effectiveBufferMs = manualBufferDurationMs.HasValue
            ? Math.Clamp(manualBufferDurationMs.Value, 80, 1500)
            : (cinemaSmoothMode ? 560 : 320);
        var wasapiLatency = manualBufferDurationMs.HasValue
            ? Math.Clamp(effectiveBufferMs / 5, 35, 220)
            : (cinemaSmoothMode ? 110 : 45);
        var waveOutLatency = manualBufferDurationMs.HasValue
            ? Math.Clamp(effectiveBufferMs / 4, 50, 280)
            : (cinemaSmoothMode ? 140 : 60);
        try
        {
            using var enumerator = new MMDeviceEnumerator();
            var device = enumerator.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
            return new WasapiOut(device, AudioClientShareMode.Shared, useEventSync: false, latency: wasapiLatency);
        }
        catch
        {
            return new WaveOutEvent
            {
                DesiredLatency = waveOutLatency,
                NumberOfBuffers = 2,
            };
        }
    }

    private int GetEffectiveBufferDurationMs()
    {
        return _manualBufferDurationMs ?? (_cinemaSmoothMode ? 560 : 320);
    }
}
