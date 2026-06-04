using System.Buffers.Binary;
using System.Text;

namespace ReceiverNative;

internal sealed class EvrtPacketizer
{
    public const int MaxPacketSize = 1200;
    private const int MaxPayloadSize = MaxPacketSize - TransportProtocol.HeaderSize;

    public byte[] BuildSessionConfigPacket(byte[] payload) =>
        BuildSinglePacket(TransportProtocol.TypeSessionConfig, payload);

    public byte[] BuildCodecConfigPacket(byte[] payload) =>
        BuildSinglePacket(TransportProtocol.TypeCodecConfig, payload);

    public byte[] BuildControlPacket(byte[] payload) =>
        BuildSinglePacket(TransportProtocol.TypeControl, payload);

    public byte[] BuildAudioConfigPacket(byte[] payload) =>
        BuildSinglePacket(TransportProtocol.TypeAudioConfig, payload);

    public IReadOnlyList<byte[]> PacketizeAudioFrame(int frameId, long presentationTimeUs, byte[] payload)
    {
        if (payload.Length == 0)
        {
            throw new ArgumentException("Audio payload must not be empty", nameof(payload));
        }

        var packetCount = Math.Max(1, (int)Math.Ceiling(payload.Length / (double)MaxPayloadSize));
        var packets = new List<byte[]>(packetCount);

        for (var packetIndex = 0; packetIndex < packetCount; packetIndex++)
        {
            var offset = packetIndex * MaxPayloadSize;
            var chunkLength = Math.Min(MaxPayloadSize, payload.Length - offset);
            var chunk = new byte[chunkLength];
            Buffer.BlockCopy(payload, offset, chunk, 0, chunkLength);
            packets.Add(BuildPacket(
                type: TransportProtocol.TypeAudioFrame,
                flags: 0,
                frameId: frameId,
                packetIndex: packetIndex,
                packetCount: packetCount,
                presentationTimeUs: presentationTimeUs,
                payload: chunk));
        }

        return packets;
    }

    public IReadOnlyList<byte[]> PacketizeVideoFrame(int frameId, long presentationTimeUs, bool isKeyFrame, byte[] payload)
    {
        if (payload.Length == 0)
        {
            throw new ArgumentException("Video payload must not be empty", nameof(payload));
        }

        var packetCount = Math.Max(1, (int)Math.Ceiling(payload.Length / (double)MaxPayloadSize));
        var flags = isKeyFrame ? TransportProtocol.FlagKeyFrame : 0;
        var packets = new List<byte[]>(packetCount);

        for (var packetIndex = 0; packetIndex < packetCount; packetIndex++)
        {
            var offset = packetIndex * MaxPayloadSize;
            var chunkLength = Math.Min(MaxPayloadSize, payload.Length - offset);
            var chunk = new byte[chunkLength];
            Buffer.BlockCopy(payload, offset, chunk, 0, chunkLength);
            packets.Add(BuildPacket(
                type: TransportProtocol.TypeVideoFrame,
                flags: flags,
                frameId: frameId,
                packetIndex: packetIndex,
                packetCount: packetCount,
                presentationTimeUs: presentationTimeUs,
                payload: chunk));
        }

        return packets;
    }

    private static byte[] BuildSinglePacket(byte type, byte[] payload)
    {
        if (payload.Length > MaxPayloadSize)
        {
            throw new ArgumentException($"Payload is too large for a single EVRT packet: {payload.Length}", nameof(payload));
        }

        return BuildPacket(
            type: type,
            flags: 0,
            frameId: 0,
            packetIndex: 0,
            packetCount: 1,
            presentationTimeUs: 0,
            payload: payload);
    }

    private static byte[] BuildPacket(
        byte type,
        int flags,
        int frameId,
        int packetIndex,
        int packetCount,
        long presentationTimeUs,
        byte[] payload)
    {
        var packet = new byte[TransportProtocol.HeaderSize + payload.Length];
        var span = packet.AsSpan();
        BinaryPrimitives.WriteInt32BigEndian(span[..4], TransportProtocol.Magic);
        span[4] = TransportProtocol.Version;
        span[5] = type;
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(6, 2), (ushort)flags);
        BinaryPrimitives.WriteUInt32BigEndian(span.Slice(8, 4), unchecked((uint)frameId));
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(12, 2), (ushort)packetIndex);
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(14, 2), (ushort)packetCount);
        BinaryPrimitives.WriteUInt64BigEndian(span.Slice(16, 8), unchecked((ulong)presentationTimeUs));
        payload.CopyTo(span[TransportProtocol.HeaderSize..]);
        return packet;
    }
}
