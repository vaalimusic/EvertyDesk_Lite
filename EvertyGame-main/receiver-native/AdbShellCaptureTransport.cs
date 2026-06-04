using System.Buffers;
using System.Diagnostics;

namespace ReceiverNative;

internal static class AdbShellCaptureTransport
{
    public static async Task RunAsync(
        string adbPath,
        AdbScreenrecordProfile profile,
        Action<SessionConfig> onSessionConfig,
        Action<byte[], bool, long> onAccessUnit,
        Action<Process>? onProcessStarted,
        CancellationToken cancellationToken)
    {
        onSessionConfig(profile.ToSessionConfig());

        using var process = new Process
        {
            StartInfo = AdbTunnelManager.BuildShellCaptureStartInfo(adbPath, profile),
        };
        if (!process.Start())
        {
            throw new InvalidOperationException("Failed to start adb exec-out screenrecord");
        }
        onProcessStarted?.Invoke(process);

        ReceiverTrace.Log($"ADB shell capture started: {profile.DisplayWidth}x{profile.DisplayHeight} -> {profile.CaptureWidth}x{profile.CaptureHeight} @ {profile.TargetFps} fps, {(profile.BitrateBps / 1_000_000.0):0.0} Mbps");

        using var cancellationRegistration = cancellationToken.Register(() => TryTerminateProcess(process));
        var stderrTask = process.StandardError.ReadToEndAsync();
        var reader = new AnnexBAccessUnitReader();
        var startedAtTicks = Stopwatch.GetTimestamp();
        long lastPresentationTimeUs = 0;
        var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var bytesRead = await process.StandardOutput.BaseStream.ReadAsync(buffer.AsMemory(0, buffer.Length), cancellationToken);
                if (bytesRead <= 0)
                {
                    break;
                }

                reader.Feed(buffer.AsSpan(0, bytesRead), (bytes, isKeyFrame) =>
                {
                    var presentationTimeUs = Math.Max(
                        lastPresentationTimeUs + 1,
                        (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                    lastPresentationTimeUs = presentationTimeUs;
                    onAccessUnit(bytes, isKeyFrame, presentationTimeUs);
                });
            }

            reader.Complete((bytes, isKeyFrame) =>
            {
                var presentationTimeUs = Math.Max(
                    lastPresentationTimeUs + 1,
                    (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                lastPresentationTimeUs = presentationTimeUs;
                onAccessUnit(bytes, isKeyFrame, presentationTimeUs);
            });

            if (cancellationToken.IsCancellationRequested)
            {
                return;
            }

            var stderr = await stderrTask;
            if (!process.WaitForExit(500))
            {
                throw new IOException("adb exec-out screenrecord stopped responding");
            }

            var errorMessage = string.IsNullOrWhiteSpace(stderr)
                ? $"adb exec-out screenrecord exited with code {process.ExitCode}"
                : stderr.Trim();
            throw new IOException(errorMessage);
        }
        finally
        {
            ArrayPool<byte>.Shared.Return(buffer);
            TryTerminateProcess(process);
        }
    }

    private static void TryTerminateProcess(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
        }
    }

    private sealed class AnnexBAccessUnitReader
    {
        private static readonly byte[] StartCode = { 0x00, 0x00, 0x00, 0x01 };

        private readonly List<byte> _buffer = new();
        private readonly List<byte[]> _currentAccessUnit = new();
        private byte[]? _latestSps;
        private byte[]? _latestPps;
        private bool _currentHasVcl;
        private bool _currentIsKeyFrame;
        private bool _currentHasSps;
        private bool _currentHasPps;

        public void Feed(ReadOnlySpan<byte> data, Action<byte[], bool> onAccessUnit)
        {
            for (var index = 0; index < data.Length; index++)
            {
                _buffer.Add(data[index]);
            }

            ParseAvailable(flushTail: false, onAccessUnit);
        }

        public void Complete(Action<byte[], bool> onAccessUnit)
        {
            ParseAvailable(flushTail: true, onAccessUnit);
            FlushCurrent(onAccessUnit);
            _buffer.Clear();
        }

        private void ParseAvailable(bool flushTail, Action<byte[], bool> onAccessUnit)
        {
            while (true)
            {
                var firstStartCodeIndex = FindStartCode(_buffer, 0);
                if (firstStartCodeIndex < 0)
                {
                    if (!flushTail && _buffer.Count > 4)
                    {
                        _buffer.RemoveRange(0, _buffer.Count - 4);
                    }
                    return;
                }

                if (firstStartCodeIndex > 0)
                {
                    _buffer.RemoveRange(0, firstStartCodeIndex);
                }

                var firstStartCodeLength = GetStartCodeLength(_buffer, 0);
                if (firstStartCodeLength == 0)
                {
                    return;
                }

                var nextStartCodeIndex = FindStartCode(_buffer, firstStartCodeLength);
                if (nextStartCodeIndex < 0)
                {
                    if (!flushTail)
                    {
                        return;
                    }

                    ProcessNal(_buffer.GetRange(firstStartCodeLength, _buffer.Count - firstStartCodeLength).ToArray(), onAccessUnit);
                    _buffer.Clear();
                    return;
                }

                ProcessNal(_buffer.GetRange(firstStartCodeLength, nextStartCodeIndex - firstStartCodeLength).ToArray(), onAccessUnit);
                _buffer.RemoveRange(0, nextStartCodeIndex);
            }
        }

        private void ProcessNal(byte[] nalUnit, Action<byte[], bool> onAccessUnit)
        {
            if (nalUnit.Length == 0)
            {
                return;
            }

            var nalType = nalUnit[0] & 0x1F;
            var isVcl = nalType is 1 or 5;
            var beginsNewPicture = isVcl && _currentHasVcl && IsFirstSliceOfPicture(nalUnit);

            if (nalType == 9 || beginsNewPicture || (!isVcl && _currentHasVcl))
            {
                FlushCurrent(onAccessUnit);
            }

            if (nalType == 9)
            {
                return;
            }

            _currentAccessUnit.Add(nalUnit);

            switch (nalType)
            {
                case 1:
                    _currentHasVcl = true;
                    break;
                case 5:
                    _currentHasVcl = true;
                    _currentIsKeyFrame = true;
                    break;
                case 7:
                    _latestSps = nalUnit.ToArray();
                    _currentHasSps = true;
                    break;
                case 8:
                    _latestPps = nalUnit.ToArray();
                    _currentHasPps = true;
                    break;
            }
        }

        private void FlushCurrent(Action<byte[], bool> onAccessUnit)
        {
            if (!_currentHasVcl || _currentAccessUnit.Count == 0)
            {
                ResetCurrent();
                return;
            }

            var nalUnits = new List<byte[]>();
            if (_currentIsKeyFrame)
            {
                if (!_currentHasSps && _latestSps is not null)
                {
                    nalUnits.Add(_latestSps);
                }
                if (!_currentHasPps && _latestPps is not null)
                {
                    nalUnits.Add(_latestPps);
                }
            }

            nalUnits.AddRange(_currentAccessUnit);

            var totalBytes = nalUnits.Sum(static nal => StartCode.Length + nal.Length);
            var combined = GC.AllocateUninitializedArray<byte>(totalBytes);
            var offset = 0;
            foreach (var nalUnit in nalUnits)
            {
                Buffer.BlockCopy(StartCode, 0, combined, offset, StartCode.Length);
                offset += StartCode.Length;
                Buffer.BlockCopy(nalUnit, 0, combined, offset, nalUnit.Length);
                offset += nalUnit.Length;
            }

            onAccessUnit(combined, _currentIsKeyFrame);
            ResetCurrent();
        }

        private void ResetCurrent()
        {
            _currentAccessUnit.Clear();
            _currentHasVcl = false;
            _currentIsKeyFrame = false;
            _currentHasSps = false;
            _currentHasPps = false;
        }

        private static int FindStartCode(List<byte> buffer, int startIndex)
        {
            for (var index = Math.Max(0, startIndex); index <= buffer.Count - 3; index++)
            {
                if (buffer[index] == 0x00 && buffer[index + 1] == 0x00)
                {
                    if (buffer[index + 2] == 0x01)
                    {
                        return index;
                    }

                    if (index + 3 < buffer.Count && buffer[index + 2] == 0x00 && buffer[index + 3] == 0x01)
                    {
                        return index;
                    }
                }
            }

            return -1;
        }

        private static int GetStartCodeLength(List<byte> buffer, int index)
        {
            if (index + 2 < buffer.Count &&
                buffer[index] == 0x00 &&
                buffer[index + 1] == 0x00 &&
                buffer[index + 2] == 0x01)
            {
                return 3;
            }

            if (index + 3 < buffer.Count &&
                buffer[index] == 0x00 &&
                buffer[index + 1] == 0x00 &&
                buffer[index + 2] == 0x00 &&
                buffer[index + 3] == 0x01)
            {
                return 4;
            }

            return 0;
        }

        private static bool IsFirstSliceOfPicture(byte[] nalUnit)
        {
            try
            {
                if (nalUnit.Length <= 1)
                {
                    return false;
                }

                var rbsp = RemoveEmulationPreventionBytes(nalUnit, 1);
                var bitReader = new H264BitReader(rbsp);
                return bitReader.ReadUnsignedExpGolomb() == 0;
            }
            catch
            {
                return false;
            }
        }

        private static byte[] RemoveEmulationPreventionBytes(byte[] bytes, int offset)
        {
            var result = new List<byte>(bytes.Length - offset);
            var zeroCount = 0;
            for (var index = offset; index < bytes.Length; index++)
            {
                var value = bytes[index];
                if (zeroCount == 2 && value == 0x03)
                {
                    zeroCount = 0;
                    continue;
                }

                result.Add(value);
                zeroCount = value == 0x00 ? zeroCount + 1 : 0;
            }

            return result.ToArray();
        }
    }

    private sealed class H264BitReader
    {
        private readonly byte[] _buffer;
        private int _bitOffset;

        public H264BitReader(byte[] buffer)
        {
            _buffer = buffer;
        }

        public int ReadUnsignedExpGolomb()
        {
            var leadingZeroBits = 0;
            while (ReadBit() == 0)
            {
                leadingZeroBits++;
            }

            var suffix = leadingZeroBits == 0 ? 0 : ReadBits(leadingZeroBits);
            return ((1 << leadingZeroBits) - 1) + suffix;
        }

        private int ReadBits(int count)
        {
            var value = 0;
            for (var index = 0; index < count; index++)
            {
                value = (value << 1) | ReadBit();
            }

            return value;
        }

        private int ReadBit()
        {
            if (_bitOffset >= _buffer.Length * 8)
            {
                throw new InvalidOperationException("Unexpected end of H.264 bitstream");
            }

            var byteIndex = _bitOffset / 8;
            var bitIndex = 7 - (_bitOffset % 8);
            _bitOffset++;
            return (_buffer[byteIndex] >> bitIndex) & 0x01;
        }
    }
}
