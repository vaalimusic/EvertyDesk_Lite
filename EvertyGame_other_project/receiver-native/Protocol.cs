using System.Buffers.Binary;
using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace ReceiverNative;

internal static class TransportProtocol
{
    public const int Magic = 0x45565254;
    public const byte Version = 3;
    public const int MaxFramedPacketSize = 64 * 1024;

    public const byte TypeSessionConfig = 1;
    public const byte TypeCodecConfig = 2;
    public const byte TypeVideoFrame = 3;
    public const byte TypeControl = 4;
    public const byte TypeAudioConfig = 5;
    public const byte TypeAudioFrame = 6;
    public const byte TypeEnhancementConfig = 7;
    public const byte TypeEnhancementFrame = 8;
    public const byte TypeRoiMetadata = 9;

    public const int FlagKeyFrame = 0x0001;

    public const int HeaderSize = 24;
}

internal static class TcpPacketFraming
{
    public static async ValueTask<byte[]?> ReadPacketAsync(Stream stream, CancellationToken cancellationToken)
    {
        var prefix = new byte[4];
        var hasPrefix = await TryReadExactAsync(stream, prefix, cancellationToken);
        if (!hasPrefix)
        {
            return null;
        }

        var length = BinaryPrimitives.ReadInt32BigEndian(prefix);
        if (length < TransportProtocol.HeaderSize || length > TransportProtocol.MaxFramedPacketSize)
        {
            throw new IOException($"Invalid framed packet length: {length}");
        }

        var packet = GC.AllocateUninitializedArray<byte>(length);
        var hasPacket = await TryReadExactAsync(stream, packet, cancellationToken);
        return hasPacket ? packet : null;
    }

    public static async ValueTask WritePacketAsync(Stream stream, byte[] packet, CancellationToken cancellationToken)
    {
        if (packet.Length < TransportProtocol.HeaderSize || packet.Length > TransportProtocol.MaxFramedPacketSize)
        {
            throw new IOException($"Packet length {packet.Length} is outside framed transport bounds");
        }

        var prefix = new byte[4];
        BinaryPrimitives.WriteInt32BigEndian(prefix, packet.Length);
        await stream.WriteAsync(prefix, cancellationToken);
        await stream.WriteAsync(packet, cancellationToken);
    }

    private static async ValueTask<bool> TryReadExactAsync(Stream stream, byte[] buffer, CancellationToken cancellationToken)
    {
        var offset = 0;
        while (offset < buffer.Length)
        {
            var bytesRead = await stream.ReadAsync(buffer.AsMemory(offset, buffer.Length - offset), cancellationToken);
            if (bytesRead <= 0)
            {
                return false;
            }

            offset += bytesRead;
        }

        return true;
    }
}

internal sealed record UdpPacket(
    byte Type,
    int Flags,
    int FrameId,
    int PacketIndex,
    int PacketCount,
    long PresentationTimeUs,
    byte[] Payload)
{
    public bool IsKeyFrame => (Flags & TransportProtocol.FlagKeyFrame) != 0;
}

internal static class ProtocolParser
{
    public static bool TryParse(byte[] datagram, int length, out UdpPacket? packet)
    {
        packet = null;
        if (length < TransportProtocol.HeaderSize)
        {
            return false;
        }

        var span = datagram.AsSpan(0, length);
        var magic = BinaryPrimitives.ReadInt32BigEndian(span[..4]);
        if (magic != TransportProtocol.Magic)
        {
            return false;
        }

        var version = span[4];
        if (version != TransportProtocol.Version)
        {
            return false;
        }

        packet = new UdpPacket(
            Type: span[5],
            Flags: BinaryPrimitives.ReadUInt16BigEndian(span.Slice(6, 2)),
            FrameId: unchecked((int)BinaryPrimitives.ReadUInt32BigEndian(span.Slice(8, 4))),
            PacketIndex: BinaryPrimitives.ReadUInt16BigEndian(span.Slice(12, 2)),
            PacketCount: BinaryPrimitives.ReadUInt16BigEndian(span.Slice(14, 2)),
            PresentationTimeUs: unchecked((long)BinaryPrimitives.ReadUInt64BigEndian(span.Slice(16, 8))),
            Payload: span[TransportProtocol.HeaderSize..].ToArray());
        return true;
    }
}

internal sealed record SessionConfig(
    string Codec,
    string Preset,
    string? Transport,
    int Width,
    int Height,
    int Fps,
    int Bitrate,
    string StreamMode,
    bool EnhancementEnabled,
    string? EnhancementCodec,
    int EnhancementMaxWidth,
    int EnhancementMaxHeight,
    string RoiMode)
{
    public string ResolutionLabel => $"{Width}x{Height}";
    public bool IsSplitStream => string.Equals(StreamMode, "split", StringComparison.OrdinalIgnoreCase) && EnhancementEnabled;

    public static SessionConfig? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<SessionConfigDto>(payload);
            if (dto is null || string.IsNullOrWhiteSpace(dto.codec) || dto.width <= 0 || dto.height <= 0)
            {
                return null;
            }

            return new SessionConfig(
                Codec: dto.codec,
                Preset: dto.preset ?? "-",
                Transport: dto.transport,
                Width: dto.baseWidth > 0 ? dto.baseWidth : dto.width,
                Height: dto.baseHeight > 0 ? dto.baseHeight : dto.height,
                Fps: dto.fps,
                Bitrate: dto.baseBitrate > 0 ? dto.baseBitrate : dto.bitrate,
                StreamMode: dto.streamMode ?? "single",
                EnhancementEnabled: dto.enhancementEnabled,
                EnhancementCodec: dto.enhancementCodec,
                EnhancementMaxWidth: dto.enhancementMaxWidth,
                EnhancementMaxHeight: dto.enhancementMaxHeight,
                RoiMode: dto.roiMode ?? "none");
        }
        catch
        {
            return null;
        }
    }

    private sealed class SessionConfigDto
    {
        public string? codec { get; set; }
        public string? preset { get; set; }
        public string? transport { get; set; }
        public int width { get; set; }
        public int height { get; set; }
        public int fps { get; set; }
        public int bitrate { get; set; }
        public string? streamMode { get; set; }
        public int baseWidth { get; set; }
        public int baseHeight { get; set; }
        public int baseBitrate { get; set; }
        public bool enhancementEnabled { get; set; }
        public string? enhancementCodec { get; set; }
        public int enhancementMaxWidth { get; set; }
        public int enhancementMaxHeight { get; set; }
        public string? roiMode { get; set; }
    }
}

internal sealed record RoiMetadata(
    int FrameId,
    int X,
    int Y,
    int Width,
    int Height,
    int ScreenWidth,
    int ScreenHeight,
    long PresentationTimeUs,
    string PulseKind)
{
    public string RectLabel => Width > 0 && Height > 0 ? $"{X},{Y} {Width}x{Height}" : "-";

    public static RoiMetadata? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<RoiMetadataDto>(payload);
            if (dto is null || dto.width <= 0 || dto.height <= 0 || dto.screenWidth <= 0 || dto.screenHeight <= 0)
            {
                return null;
            }

            return new RoiMetadata(
                FrameId: dto.frameId,
                X: dto.x,
                Y: dto.y,
                Width: dto.width,
                Height: dto.height,
                ScreenWidth: dto.screenWidth,
                ScreenHeight: dto.screenHeight,
                PresentationTimeUs: dto.presentationTimeUs,
                PulseKind: dto.pulseKind ?? "tap");
        }
        catch
        {
            return null;
        }
    }

    private sealed class RoiMetadataDto
    {
        public int frameId { get; set; }
        public int x { get; set; }
        public int y { get; set; }
        public int width { get; set; }
        public int height { get; set; }
        public int screenWidth { get; set; }
        public int screenHeight { get; set; }
        public long presentationTimeUs { get; set; }
        public string? pulseKind { get; set; }
    }
}

internal static class ControlPacketBuilder
{
    public static byte[] BuildRequestKeyFrame()
    {
        return BuildControl("""{"kind":"request_keyframe"}""");
    }

    public static byte[] BuildReceiverFeedback(
        string pressure,
        int backlogFrames,
        long queueDrops,
        int decodeFps,
        int assemblyDelayMs,
        int arrivalDeltaMs,
        int decodeDeltaMs,
        int presentDeltaMs)
    {
        var json = JsonSerializer.Serialize(
            new
            {
                kind = "receiver_feedback",
                pressure,
                backlogFrames,
                queueDrops,
                decodeFps,
                assemblyDelayMs,
                arrivalDeltaMs,
                decodeDeltaMs,
                presentDeltaMs,
            });
        return BuildControl(json);
    }

    private static byte[] BuildControl(string json)
    {
        var payload = Encoding.UTF8.GetBytes(json);
        var packet = new byte[TransportProtocol.HeaderSize + payload.Length];
        var span = packet.AsSpan();
        BinaryPrimitives.WriteInt32BigEndian(span[..4], TransportProtocol.Magic);
        span[4] = TransportProtocol.Version;
        span[5] = TransportProtocol.TypeControl;
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(6, 2), 0);
        BinaryPrimitives.WriteUInt32BigEndian(span.Slice(8, 4), 0);
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(12, 2), 0);
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(14, 2), 1);
        BinaryPrimitives.WriteUInt64BigEndian(span.Slice(16, 8), 0);
        payload.CopyTo(span[TransportProtocol.HeaderSize..]);
        return packet;
    }
}

internal sealed record EnhancementAccessUnit(
    byte[] Bytes,
    bool IsKeyFrame,
    int AssemblyDelayMs,
    long PresentationTimeUs,
    RoiMetadata? Metadata);

internal sealed class FrameReassembler
{
    private readonly Action<SessionConfig> _onSessionConfig;
    private readonly Action<byte[], bool, int, long> _onBaseAccessUnitReady;
    private readonly Action<EnhancementAccessUnit> _onEnhancementAccessUnitReady;
    private readonly Action<long, long> _onDroppedFramesChanged;
    private readonly AccessUnitChannelReassembler _baseChannel;
    private readonly AccessUnitChannelReassembler _enhancementChannel;
    private readonly Dictionary<int, RoiMetadata> _roiMetadataByFrameId = new();

    public FrameReassembler(
        Action<SessionConfig> onSessionConfig,
        Action<byte[], bool, int, long> onBaseAccessUnitReady,
        Action<EnhancementAccessUnit> onEnhancementAccessUnitReady,
        Action<long, long> onDroppedFramesChanged)
    {
        _onSessionConfig = onSessionConfig;
        _onBaseAccessUnitReady = onBaseAccessUnitReady;
        _onEnhancementAccessUnitReady = onEnhancementAccessUnitReady;
        _onDroppedFramesChanged = onDroppedFramesChanged;
        _baseChannel = new AccessUnitChannelReassembler(
            onAccessUnitReady: (bytes, isKeyFrame, assemblyDelayMs, presentationTimeUs, _) =>
                _onBaseAccessUnitReady(bytes, isKeyFrame, assemblyDelayMs, presentationTimeUs),
            onDroppedChanged: PublishDrops);
        _enhancementChannel = new AccessUnitChannelReassembler(
            onAccessUnitReady: (bytes, isKeyFrame, assemblyDelayMs, presentationTimeUs, frameId) =>
            {
                _roiMetadataByFrameId.TryGetValue(frameId, out var metadata);
                _roiMetadataByFrameId.Remove(frameId);
                _onEnhancementAccessUnitReady(new EnhancementAccessUnit(bytes, isKeyFrame, assemblyDelayMs, presentationTimeUs, metadata));
            },
            onDroppedChanged: PublishDrops);
    }

    public long BaseDroppedFrames => _baseChannel.DroppedFrames;
    public long EnhancementDroppedFrames => _enhancementChannel.DroppedFrames;

    public void OnPacket(UdpPacket packet)
    {
        switch (packet.Type)
        {
            case TransportProtocol.TypeSessionConfig:
                var config = SessionConfig.Parse(packet.Payload);
                if (config is not null)
                {
                    ResetRealtimeState();
                    _onSessionConfig(config);
                }
                break;

            case TransportProtocol.TypeCodecConfig:
                _baseChannel.SetCodecConfig(packet.Payload);
                break;

            case TransportProtocol.TypeVideoFrame:
                _baseChannel.OnFramePacket(packet);
                break;

            case TransportProtocol.TypeEnhancementConfig:
                _enhancementChannel.SetCodecConfig(packet.Payload);
                break;

            case TransportProtocol.TypeEnhancementFrame:
                _enhancementChannel.OnFramePacket(packet);
                break;

            case TransportProtocol.TypeRoiMetadata:
                var metadata = RoiMetadata.Parse(packet.Payload);
                if (metadata is not null)
                {
                    _roiMetadataByFrameId[metadata.FrameId] = metadata;
                    TrimRoiMetadata(metadata.FrameId);
                }
                break;
        }
    }

    private void PublishDrops()
    {
        _onDroppedFramesChanged(_baseChannel.DroppedFrames, _enhancementChannel.DroppedFrames);
    }

    private void ResetRealtimeState()
    {
        _baseChannel.ResetRealtimeState();
        _enhancementChannel.ResetRealtimeState();
        _roiMetadataByFrameId.Clear();
    }

    private void TrimRoiMetadata(int newestFrameId)
    {
        foreach (var staleKey in _roiMetadataByFrameId.Keys.Where(key => key < newestFrameId - 8).ToArray())
        {
            _roiMetadataByFrameId.Remove(staleKey);
        }
    }

    private sealed class AccessUnitChannelReassembler
    {
        private readonly Action<byte[], bool, int, long, int> _onAccessUnitReady;
        private readonly Action _onDroppedChanged;
        private readonly Dictionary<int, FrameAssembly> _frames = new();
        private byte[]? _latestCodecConfig;
        private int _latestFrameIdSeen = -1;
        private int _latestCompletedFrameId = -1;
        private bool _waitingForKeyFrameAfterLoss;
        private long _droppedFrames;

        public AccessUnitChannelReassembler(
            Action<byte[], bool, int, long, int> onAccessUnitReady,
            Action onDroppedChanged)
        {
            _onAccessUnitReady = onAccessUnitReady;
            _onDroppedChanged = onDroppedChanged;
        }

        public long DroppedFrames => _droppedFrames;

        public void SetCodecConfig(byte[] payload)
        {
            _latestCodecConfig = payload.ToArray();
        }

        public void OnFramePacket(UdpPacket packet)
        {
            if (packet.PacketCount <= 0 || packet.PacketIndex >= packet.PacketCount)
            {
                DropFrame();
                return;
            }

            if (packet.FrameId <= _latestCompletedFrameId || packet.FrameId < _latestFrameIdSeen)
            {
                DropFrame();
                return;
            }

            if (_waitingForKeyFrameAfterLoss && !packet.IsKeyFrame)
            {
                DropFrame();
                return;
            }

            if (packet.FrameId > _latestFrameIdSeen)
            {
                var droppedIncomplete = DropOlderFramesThan(packet.FrameId);
                _latestFrameIdSeen = packet.FrameId;
                if (droppedIncomplete && !packet.IsKeyFrame)
                {
                    _waitingForKeyFrameAfterLoss = true;
                    DropFrame();
                    return;
                }
            }

            if (packet.IsKeyFrame)
            {
                _waitingForKeyFrameAfterLoss = false;
                DropOlderFramesThan(packet.FrameId);
            }

            if (!_frames.TryGetValue(packet.FrameId, out var assembly))
            {
                assembly = new FrameAssembly(packet.FrameId, packet.PacketCount, packet.IsKeyFrame, packet.PresentationTimeUs);
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
            _latestCompletedFrameId = assembly.FrameId;
            var frameBytes = assembly.Join();
            if (assembly.IsKeyFrame)
            {
                _waitingForKeyFrameAfterLoss = false;
                if (_latestCodecConfig is null)
                {
                    DropFrame();
                    _waitingForKeyFrameAfterLoss = true;
                    return;
                }

                var combined = new byte[_latestCodecConfig.Length + frameBytes.Length];
                Buffer.BlockCopy(_latestCodecConfig, 0, combined, 0, _latestCodecConfig.Length);
                Buffer.BlockCopy(frameBytes, 0, combined, _latestCodecConfig.Length, frameBytes.Length);
                _onAccessUnitReady(combined, true, assembly.AssemblyDelayMs, assembly.PresentationTimeUs, assembly.FrameId);
                return;
            }

            if (_waitingForKeyFrameAfterLoss)
            {
                DropFrame();
                return;
            }

            _onAccessUnitReady(frameBytes, false, assembly.AssemblyDelayMs, assembly.PresentationTimeUs, assembly.FrameId);
        }

        public void ResetRealtimeState()
        {
            _frames.Clear();
            _latestFrameIdSeen = -1;
            _latestCompletedFrameId = -1;
            _waitingForKeyFrameAfterLoss = false;
        }

        private bool DropOlderFramesThan(int frameId)
        {
            var droppedIncomplete = false;
            if (_frames.Count == 0)
            {
                return false;
            }

            foreach (var key in _frames.Keys.Where(key => key < frameId).ToArray())
            {
                _frames.Remove(key);
                DropFrame();
                droppedIncomplete = true;
            }

            return droppedIncomplete;
        }

        private void DropFrame()
        {
            _droppedFrames += 1;
            _onDroppedChanged();
        }
    }

    private sealed class FrameAssembly
    {
        private readonly byte[][] _parts;
        private int _received;
        private readonly long _startedAtTicks = Stopwatch.GetTimestamp();

        public FrameAssembly(int frameId, int packetCount, bool isKeyFrame, long presentationTimeUs)
        {
            FrameId = frameId;
            PacketCount = packetCount;
            IsKeyFrame = isKeyFrame;
            PresentationTimeUs = presentationTimeUs;
            _parts = new byte[packetCount][];
        }

        public int FrameId { get; }
        public int PacketCount { get; }
        public bool IsKeyFrame { get; }
        public long PresentationTimeUs { get; }
        public bool IsComplete => _received == PacketCount;
        public int AssemblyDelayMs
        {
            get
            {
                var seconds = (Stopwatch.GetTimestamp() - _startedAtTicks) / (double)Stopwatch.Frequency;
                return Math.Max(0, (int)Math.Round(seconds * 1_000.0));
            }
        }

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
