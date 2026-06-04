namespace ReceiverNative;

using System.IO;

internal sealed class PlaybackStreamStats
{
    public int QueuedAccessUnits { get; init; }
    public int QueuedBytes { get; init; }
    public long DroppedAccessUnits { get; init; }
    public bool WaitingForKeyFrame { get; init; }
}

internal sealed class LatestAccessUnitStream : Stream
{
    private sealed record QueuedUnit(byte[] Bytes, bool IsKeyFrame);

    private readonly object _sync = new();
    private readonly Queue<QueuedUnit> _queue = new();
    private readonly int _maxQueuedUnits;
    private readonly int _maxQueuedBytes;
    private readonly Action<PlaybackStreamStats>? _statsChanged;
    private readonly bool _hardResetOnKeyFrame;
    private readonly bool _dropCurrentOnWaitForKeyFrame;

    private bool _closed;
    private bool _waitingForKeyFrame;
    private long _droppedAccessUnits;
    private int _queuedBytes;
    private byte[]? _current;
    private int _currentOffset;

    public LatestAccessUnitStream(
        int maxQueuedUnits,
        int maxQueuedBytes,
        Action<PlaybackStreamStats>? statsChanged,
        bool hardResetOnKeyFrame = false,
        bool dropCurrentOnWaitForKeyFrame = false)
    {
        _maxQueuedUnits = Math.Max(1, maxQueuedUnits);
        _maxQueuedBytes = Math.Max(16 * 1024, maxQueuedBytes);
        _statsChanged = statsChanged;
        _hardResetOnKeyFrame = hardResetOnKeyFrame;
        _dropCurrentOnWaitForKeyFrame = dropCurrentOnWaitForKeyFrame;
    }

    public override bool CanRead => true;
    public override bool CanSeek => false;
    public override bool CanWrite => false;
    public override long Length => throw new NotSupportedException();

    public override long Position
    {
        get => throw new NotSupportedException();
        set => throw new NotSupportedException();
    }

    public void Enqueue(byte[] bytes, bool isKeyFrame)
    {
        PlaybackStreamStats snapshot;
        lock (_sync)
        {
            if (_closed)
            {
                return;
            }

            if (_waitingForKeyFrame && !isKeyFrame)
            {
                _droppedAccessUnits += 1;
                snapshot = SnapshotLocked();
                PublishStats(snapshot);
                return;
            }

            if (_waitingForKeyFrame && isKeyFrame)
            {
                ClearQueuedLocked();
                if (_dropCurrentOnWaitForKeyFrame)
                {
                    DropCurrentLocked();
                }
                _waitingForKeyFrame = false;
                EnqueueLocked(bytes, true);
                Monitor.PulseAll(_sync);
                snapshot = SnapshotLocked();
                PublishStats(snapshot);
                return;
            }

            if (_hardResetOnKeyFrame && isKeyFrame && (_current is not null || _queue.Count > 0))
            {
                ClearQueuedLocked();
                DropCurrentLocked();
                _waitingForKeyFrame = false;
                EnqueueLocked(bytes, true);
                Monitor.PulseAll(_sync);
                snapshot = SnapshotLocked();
                PublishStats(snapshot);
                return;
            }

            var wouldOverflowUnits = _queue.Count >= _maxQueuedUnits;
            var wouldOverflowBytes = _queuedBytes + bytes.Length > _maxQueuedBytes;
            if (wouldOverflowUnits || wouldOverflowBytes)
            {
                _droppedAccessUnits += _queue.Count;
                ClearQueuedLocked();
                if (_dropCurrentOnWaitForKeyFrame)
                {
                    DropCurrentLocked();
                }

                // In latest-frame mode the newest AU is still useful even if it is individually large.
                // Waiting for the next keyframe here creates a self-sustaining stall on high-detail frames.
                var canAcceptAsFreshest = _current is null && _queue.Count == 0;
                if (isKeyFrame || canAcceptAsFreshest)
                {
                    EnqueueLocked(bytes, isKeyFrame);
                    _waitingForKeyFrame = false;
                    Monitor.PulseAll(_sync);
                }
                else
                {
                    _droppedAccessUnits += 1;
                    _waitingForKeyFrame = true;
                }

                snapshot = SnapshotLocked();
                PublishStats(snapshot);
                return;
            }

            EnqueueLocked(bytes, isKeyFrame);
            Monitor.PulseAll(_sync);
            snapshot = SnapshotLocked();
        }

        PublishStats(snapshot);
    }

    public void WaitForKeyFrame()
    {
        PlaybackStreamStats snapshot;
        lock (_sync)
        {
            if (_closed)
            {
                return;
            }

            ClearQueuedLocked();
            if (_dropCurrentOnWaitForKeyFrame)
            {
                DropCurrentLocked();
            }
            _waitingForKeyFrame = true;
            snapshot = SnapshotLocked();
            Monitor.PulseAll(_sync);
        }

        PublishStats(snapshot);
    }

    public override int Read(byte[] buffer, int offset, int count)
    {
        if (count == 0)
        {
            return 0;
        }

        lock (_sync)
        {
            while (true)
            {
                EnsureCurrentLocked();
                if (_current is not null)
                {
                    var remaining = _current.Length - _currentOffset;
                    var toCopy = Math.Min(count, remaining);
                    Array.Copy(_current, _currentOffset, buffer, offset, toCopy);
                    _currentOffset += toCopy;
                    if (_currentOffset >= _current.Length)
                    {
                        _current = null;
                        _currentOffset = 0;
                    }
                    return toCopy;
                }

                if (_closed)
                {
                    return 0;
                }

                Monitor.Wait(_sync);
            }
        }
    }

    public override void Flush()
    {
    }

    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();
    public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        base.Dispose(disposing);
        PlaybackStreamStats snapshot;
        lock (_sync)
        {
            _closed = true;
            ClearQueuedLocked();
            _current = null;
            _currentOffset = 0;
            snapshot = SnapshotLocked();
            Monitor.PulseAll(_sync);
        }

        PublishStats(snapshot);
    }

    private void EnqueueLocked(byte[] bytes, bool isKeyFrame)
    {
        _queue.Enqueue(new QueuedUnit(bytes, isKeyFrame));
        _queuedBytes += bytes.Length;
    }

    private void EnsureCurrentLocked()
    {
        if (_current is not null || _queue.Count == 0)
        {
            return;
        }

        var next = _queue.Dequeue();
        _current = next.Bytes;
        _currentOffset = 0;
        _queuedBytes -= next.Bytes.Length;
    }

    private void ClearQueuedLocked()
    {
        _queue.Clear();
        _queuedBytes = 0;
    }

    private void DropCurrentLocked()
    {
        if (_current is null)
        {
            return;
        }

        _droppedAccessUnits += 1;
        _current = null;
        _currentOffset = 0;
    }

    private PlaybackStreamStats SnapshotLocked()
    {
        var currentBytes = _current is null ? 0 : _current.Length - _currentOffset;
        return new PlaybackStreamStats
        {
            QueuedAccessUnits = _queue.Count + (_current is null ? 0 : 1),
            QueuedBytes = _queuedBytes + currentBytes,
            DroppedAccessUnits = _droppedAccessUnits,
            WaitingForKeyFrame = _waitingForKeyFrame,
        };
    }

    private void PublishStats(PlaybackStreamStats snapshot)
    {
        _statsChanged?.Invoke(snapshot);
    }
}
