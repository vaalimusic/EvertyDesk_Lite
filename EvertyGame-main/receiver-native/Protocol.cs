using System.Buffers.Binary;
using System.Diagnostics;
using System.IO;
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
    string AdaptationMode,
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
    public bool IsCinemaSmooth => string.Equals(AdaptationMode, "CINEMA_SMOOTH", StringComparison.OrdinalIgnoreCase);

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
                AdaptationMode: dto.adaptationMode ?? "AUTO_BALANCED",
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
        public string? adaptationMode { get; set; }
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

internal sealed record LatencyPulseControl(
    long PulseId,
    string Source,
    long PresentationTimeUs,
    int TapToUiMs,
    int SenderPipelineMs,
    int ApproxSenderMs,
    long InputSeq)
{
    public static LatencyPulseControl? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<LatencyPulseControlDto>(payload);
            if (dto is null || !string.Equals(dto.kind, "latency_pulse", StringComparison.OrdinalIgnoreCase) || dto.presentationTimeUs <= 0)
            {
                return null;
            }

            return new LatencyPulseControl(
                PulseId: dto.pulseId,
                Source: dto.source ?? "UNKNOWN",
                PresentationTimeUs: dto.presentationTimeUs,
                TapToUiMs: Math.Max(0, dto.tapToUiMs),
                SenderPipelineMs: Math.Max(0, dto.senderPipelineMs),
                ApproxSenderMs: Math.Max(0, dto.approxSenderMs),
                InputSeq: Math.Max(0, dto.inputSeq));
        }
        catch
        {
            return null;
        }
    }

    private sealed class LatencyPulseControlDto
    {
        public string? kind { get; set; }
        public long pulseId { get; set; }
        public string? source { get; set; }
        public long presentationTimeUs { get; set; }
        public int tapToUiMs { get; set; }
        public int senderPipelineMs { get; set; }
        public int approxSenderMs { get; set; }
        public long inputSeq { get; set; }
    }
}

internal sealed record LatencyPulseRequestControl(
    long Seq,
    string Source)
{
    public static LatencyPulseRequestControl? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<LatencyPulseRequestControlDto>(payload);
            if (dto is null || !string.Equals(dto.kind, "latency_pulse_request", StringComparison.OrdinalIgnoreCase))
            {
                return null;
            }

            return new LatencyPulseRequestControl(
                Seq: Math.Max(0, dto.seq),
                Source: string.IsNullOrWhiteSpace(dto.source) ? "manual" : dto.source);
        }
        catch
        {
            return null;
        }
    }

    private sealed class LatencyPulseRequestControlDto
    {
        public string? kind { get; set; }
        public long seq { get; set; }
        public string? source { get; set; }
    }
}

internal sealed record RelayRegistrationControl(
    string SessionId,
    string SessionToken,
    string Role)
{
    public static RelayRegistrationControl? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<RelayRegistrationControlDto>(payload);
            if (dto is null ||
                !string.Equals(dto.kind, "relay_register", StringComparison.OrdinalIgnoreCase) ||
                string.IsNullOrWhiteSpace(dto.sessionId) ||
                string.IsNullOrWhiteSpace(dto.sessionToken) ||
                string.IsNullOrWhiteSpace(dto.role))
            {
                return null;
            }

            return new RelayRegistrationControl(
                SessionId: dto.sessionId.Trim(),
                SessionToken: dto.sessionToken.Trim(),
                Role: dto.role.Trim().ToLowerInvariant());
        }
        catch
        {
            return null;
        }
    }

    private sealed class RelayRegistrationControlDto
    {
        public string? kind { get; set; }
        public string? sessionId { get; set; }
        public string? sessionToken { get; set; }
        public string? role { get; set; }
    }
}

internal sealed record ReceiverFeedbackControl(
    string Pressure,
    int BacklogFrames,
    long QueueDrops,
    int DecodeFps,
    int AssemblyDelayMs,
    int ArrivalDeltaMs,
    int DecodeDeltaMs,
    int PresentDeltaMs,
    int PulseEstimateMs,
    int InputEstimateMs)
{
    public static ReceiverFeedbackControl? Parse(byte[] payload)
    {
        try
        {
            var dto = JsonSerializer.Deserialize<ReceiverFeedbackControlDto>(payload);
            if (dto is null || !string.Equals(dto.kind, "receiver_feedback", StringComparison.OrdinalIgnoreCase))
            {
                return null;
            }

            return new ReceiverFeedbackControl(
                Pressure: dto.pressure ?? "normal",
                BacklogFrames: Math.Max(0, dto.backlogFrames),
                QueueDrops: Math.Max(0, dto.queueDrops),
                DecodeFps: Math.Max(0, dto.decodeFps),
                AssemblyDelayMs: Math.Max(0, dto.assemblyDelayMs),
                ArrivalDeltaMs: dto.arrivalDeltaMs,
                DecodeDeltaMs: dto.decodeDeltaMs,
                PresentDeltaMs: dto.presentDeltaMs,
                PulseEstimateMs: dto.pulseEstimateMs,
                InputEstimateMs: dto.inputEstimateMs);
        }
        catch
        {
            return null;
        }
    }

    private sealed class ReceiverFeedbackControlDto
    {
        public string? kind { get; set; }
        public string? pressure { get; set; }
        public int backlogFrames { get; set; }
        public long queueDrops { get; set; }
        public int decodeFps { get; set; }
        public int assemblyDelayMs { get; set; }
        public int arrivalDeltaMs { get; set; }
        public int decodeDeltaMs { get; set; }
        public int presentDeltaMs { get; set; }
        public int pulseEstimateMs { get; set; } = -1;
        public int inputEstimateMs { get; set; } = -1;
    }
}

internal enum RemoteMouseButtonKind
{
    Left,
    Right,
    Middle,
    X1,
    X2,
}

internal abstract record RemoteInputControlMessage(long Seq)
{
    public static RemoteInputControlMessage? Parse(byte[] payload)
    {
        try
        {
            using var document = JsonDocument.Parse(payload);
            if (!document.RootElement.TryGetProperty("kind", out var kindProperty))
            {
                return null;
            }

            var kind = kindProperty.GetString();
            var seq = document.RootElement.TryGetProperty("seq", out var seqProperty)
                ? seqProperty.GetInt64()
                : 0L;

            return kind switch
            {
                "remote_mouse_move_abs" => new RemoteMouseMoveAbsolute(
                    seq,
                    document.RootElement.TryGetProperty("x", out var xProperty) ? xProperty.GetDouble() : 0.0,
                    document.RootElement.TryGetProperty("y", out var yProperty) ? yProperty.GetDouble() : 0.0),
                "remote_mouse_move_rel" => new RemoteMouseMoveRelative(
                    seq,
                    document.RootElement.TryGetProperty("dx", out var dxProperty) ? dxProperty.GetInt32() : 0,
                    document.RootElement.TryGetProperty("dy", out var dyProperty) ? dyProperty.GetInt32() : 0),
                "remote_mouse_button" => new RemoteMouseButtonMessage(
                    seq,
                    ParseMouseButton(document.RootElement.TryGetProperty("button", out var buttonProperty) ? buttonProperty.GetString() : null),
                    document.RootElement.TryGetProperty("pressed", out var pressedProperty) && pressedProperty.GetBoolean()),
                "remote_mouse_wheel" => new RemoteMouseWheelMessage(
                    seq,
                    document.RootElement.TryGetProperty("delta", out var deltaProperty) ? deltaProperty.GetInt32() : 0),
                "remote_key" => new RemoteKeyMessage(
                    seq,
                    document.RootElement.TryGetProperty("vkey", out var vkeyProperty) ? vkeyProperty.GetInt32() : 0,
                    document.RootElement.TryGetProperty("pressed", out var keyPressedProperty) && keyPressedProperty.GetBoolean()),
                "remote_gamepad_state" => new RemoteGamepadStateMessage(
                    seq,
                    document.RootElement.TryGetProperty("controllerId", out var controllerIdProperty) ? Math.Max(0, controllerIdProperty.GetInt32()) : 0,
                    document.RootElement.TryGetProperty("buttons", out var buttonsProperty) ? buttonsProperty.GetUInt16() : (ushort)0,
                    document.RootElement.TryGetProperty("leftTrigger", out var leftTriggerProperty) ? (byte)Math.Clamp(leftTriggerProperty.GetInt32(), 0, 255) : (byte)0,
                    document.RootElement.TryGetProperty("rightTrigger", out var rightTriggerProperty) ? (byte)Math.Clamp(rightTriggerProperty.GetInt32(), 0, 255) : (byte)0,
                    document.RootElement.TryGetProperty("leftThumbX", out var leftThumbXProperty) ? (short)Math.Clamp(leftThumbXProperty.GetInt32(), short.MinValue, short.MaxValue) : (short)0,
                    document.RootElement.TryGetProperty("leftThumbY", out var leftThumbYProperty) ? (short)Math.Clamp(leftThumbYProperty.GetInt32(), short.MinValue, short.MaxValue) : (short)0,
                    document.RootElement.TryGetProperty("rightThumbX", out var rightThumbXProperty) ? (short)Math.Clamp(rightThumbXProperty.GetInt32(), short.MinValue, short.MaxValue) : (short)0,
                    document.RootElement.TryGetProperty("rightThumbY", out var rightThumbYProperty) ? (short)Math.Clamp(rightThumbYProperty.GetInt32(), short.MinValue, short.MaxValue) : (short)0),
                "remote_release_all" => new RemoteReleaseAllMessage(seq),
                _ => null,
            };
        }
        catch
        {
            return null;
        }
    }

    private static RemoteMouseButtonKind ParseMouseButton(string? rawButton)
    {
        return rawButton?.ToLowerInvariant() switch
        {
            "right" => RemoteMouseButtonKind.Right,
            "middle" => RemoteMouseButtonKind.Middle,
            "x1" => RemoteMouseButtonKind.X1,
            "x2" => RemoteMouseButtonKind.X2,
            _ => RemoteMouseButtonKind.Left,
        };
    }
}

internal sealed record RemoteMouseMoveAbsolute(long Seq, double X, double Y) : RemoteInputControlMessage(Seq);
internal sealed record RemoteMouseMoveRelative(long Seq, int Dx, int Dy) : RemoteInputControlMessage(Seq);
internal sealed record RemoteMouseButtonMessage(long Seq, RemoteMouseButtonKind Button, bool Pressed) : RemoteInputControlMessage(Seq);
internal sealed record RemoteMouseWheelMessage(long Seq, int Delta) : RemoteInputControlMessage(Seq);
internal sealed record RemoteKeyMessage(long Seq, int VirtualKey, bool Pressed) : RemoteInputControlMessage(Seq);
internal sealed record RemoteGamepadStateMessage(
    long Seq,
    int ControllerId,
    ushort Buttons,
    byte LeftTrigger,
    byte RightTrigger,
    short LeftThumbX,
    short LeftThumbY,
    short RightThumbX,
    short RightThumbY) : RemoteInputControlMessage(Seq);
internal sealed record RemoteReleaseAllMessage(long Seq) : RemoteInputControlMessage(Seq);

internal static class ControlMessageParser
{
    public static bool IsRequestKeyFrame(byte[] payload)
    {
        try
        {
            using var document = JsonDocument.Parse(payload);
            return document.RootElement.TryGetProperty("kind", out var kindProperty) &&
                string.Equals(kindProperty.GetString(), "request_keyframe", StringComparison.OrdinalIgnoreCase);
        }
        catch
        {
            return false;
        }
    }

    public static bool IsReceiverStop(byte[] payload)
    {
        try
        {
            using var document = JsonDocument.Parse(payload);
            if (!document.RootElement.TryGetProperty("kind", out var kindElement))
            {
                return false;
            }

            return string.Equals(kindElement.GetString(), "receiver_stop", StringComparison.OrdinalIgnoreCase);
        }
        catch
        {
            return false;
        }
    }

    public static bool TryParseDiscoveryProbe(byte[] payload, out string senderName)
    {
        senderName = "Everty Sender";
        try
        {
            using var document = JsonDocument.Parse(payload);
            if (!document.RootElement.TryGetProperty("kind", out var kindProperty) ||
                !string.Equals(kindProperty.GetString(), "discovery_probe", StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }

            if (document.RootElement.TryGetProperty("senderName", out var senderNameProperty))
            {
                senderName = senderNameProperty.GetString() ?? senderName;
            }
            return true;
        }
        catch
        {
            return false;
        }
    }

    public static DiscoveryResponseControl? TryParseDiscoveryResponse(byte[] payload)
    {
        try
        {
            using var document = JsonDocument.Parse(payload);
            if (!document.RootElement.TryGetProperty("kind", out var kindProperty) ||
                !string.Equals(kindProperty.GetString(), "discovery_response", StringComparison.OrdinalIgnoreCase))
            {
                return null;
            }

            return new DiscoveryResponseControl(
                DeviceName: document.RootElement.TryGetProperty("deviceName", out var deviceNameProperty)
                    ? deviceNameProperty.GetString() ?? "Everty Receiver"
                    : "Everty Receiver",
                Role: document.RootElement.TryGetProperty("role", out var roleProperty)
                    ? roleProperty.GetString() ?? "receiver"
                    : "receiver",
                Port: document.RootElement.TryGetProperty("port", out var portProperty)
                    ? portProperty.GetInt32()
                    : 5001);
        }
        catch
        {
            return null;
        }
    }
}

internal sealed record DiscoveryResponseControl(
    string DeviceName,
    string Role,
    int Port);

internal static class ControlPacketBuilder
{
    public static byte[] BuildRequestKeyFrame()
    {
        return BuildControl("""{"kind":"request_keyframe"}""");
    }

    public static byte[] BuildLatencyPulseRequest(long seq, string source)
    {
        var json = JsonSerializer.Serialize(
            new
            {
                kind = "latency_pulse_request",
                seq,
                source,
            });
        return BuildControl(json);
    }

    public static byte[] BuildDiscoveryProbe(string senderName)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "discovery_probe",
            senderName,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRelayRegistration(string sessionId, string sessionToken, string role)
    {
        var json = JsonSerializer.Serialize(
            new
            {
                kind = "relay_register",
                sessionId,
                sessionToken,
                role,
            });
        return BuildControl(json);
    }

    public static byte[] BuildDiscoveryResponse(string deviceName, string role, int port)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "discovery_response",
            deviceName,
            role,
            port,
        });
        return BuildControl(json);
    }

    public static byte[] BuildLatencyPulse(
        long pulseId,
        string source,
        long presentationTimeUs,
        int tapToUiMs,
        int senderPipelineMs,
        int approxSenderMs,
        long inputSeq)
    {
        var json = JsonSerializer.Serialize(
            new
            {
                kind = "latency_pulse",
                pulseId,
                source,
                presentationTimeUs,
                tapToUiMs,
                senderPipelineMs,
                approxSenderMs,
                inputSeq,
            });
        return BuildControl(json);
    }

    public static byte[] BuildReceiverFeedback(
        string pressure,
        int backlogFrames,
        long queueDrops,
        int decodeFps,
        int assemblyDelayMs,
        int arrivalDeltaMs,
        int decodeDeltaMs,
        int presentDeltaMs,
        int pulseEstimateMs,
        int inputEstimateMs)
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
                pulseEstimateMs,
                inputEstimateMs,
            });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteMouseMoveAbsolute(long seq, double x, double y)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_mouse_move_abs",
            seq,
            x,
            y,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteMouseMoveRelative(long seq, int dx, int dy)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_mouse_move_rel",
            seq,
            dx,
            dy,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteMouseButton(long seq, RemoteMouseButtonKind button, bool pressed)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_mouse_button",
            seq,
            button = button.ToString().ToLowerInvariant(),
            pressed,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteMouseWheel(long seq, int delta)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_mouse_wheel",
            seq,
            delta,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteKey(long seq, int virtualKey, bool pressed)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_key",
            seq,
            vkey = virtualKey,
            pressed,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteGamepadState(
        long seq,
        int controllerId,
        ushort buttons,
        byte leftTrigger,
        byte rightTrigger,
        short leftThumbX,
        short leftThumbY,
        short rightThumbX,
        short rightThumbY)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_gamepad_state",
            seq,
            controllerId,
            buttons,
            leftTrigger,
            rightTrigger,
            leftThumbX,
            leftThumbY,
            rightThumbX,
            rightThumbY,
        });
        return BuildControl(json);
    }

    public static byte[] BuildRemoteReleaseAll(long seq)
    {
        var json = JsonSerializer.Serialize(new
        {
            kind = "remote_release_all",
            seq,
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
