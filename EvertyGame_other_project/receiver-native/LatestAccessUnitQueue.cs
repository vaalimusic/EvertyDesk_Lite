using System.Diagnostics;

namespace ReceiverNative;

internal sealed class LatestAccessUnitQueue : IDisposable
{
    private sealed record QueuedUnit(byte[] Bytes, bool IsKeyFrame, long PresentationTimeUs, long EnqueuedAtTicks);

    private readonly object _sync = new();
    private readonly Queue<QueuedUnit> _queue = new();
    private readonly int _maxQueuedUnits;
    private readonly int _maxQueuedBytes;
    private readonly Action<PlaybackStreamStats>? _statsChanged;
    private readonly bool _hardResetOnKeyFrame;
    private readonly bool _dropCurrentOnWaitForKeyFrame;
    private readonly bool _preferLatestQueuedUnit;
    private TimeSpan _jitterBufferDelay;

    private bool _closed;
    private bool _waitingForKeyFrame;
    private long _droppedAccessUnits;
    private int _queuedBytes;
    private QueuedUnit? _current;

    public LatestAccessUnitQueue(
        int maxQueuedUnits,
        int maxQueuedBytes,
        Action<PlaybackStreamStats>? statsChanged,
        bool hardResetOnKeyFrame,
        bool dropCurrentOnWaitForKeyFrame,
        bool preferLatestQueuedUnit,
        TimeSpan jitterBufferDelay)
    {
        _maxQueuedUnits = Math.Max(1, maxQueuedUnits);
        _maxQueuedBytes = Math.Max(16 * 1024, maxQueuedBytes);
        _statsChanged = statsChanged;
        _hardResetOnKeyFrame = hardResetOnKeyFrame;
        _dropCurrentOnWaitForKeyFrame = dropCurrentOnWaitForKeyFrame;
        _preferLatestQueuedUnit = preferLatestQueuedUnit;
        _jitterBufferDelay = jitterBufferDelay > TimeSpan.Zero ? jitterBufferDelay : TimeSpan.Zero;
    }

    public void UpdateJitterBufferDelay(TimeSpan delay)
    {
        lock (_sync)
        {
            _jitterBufferDelay = delay > TimeSpan.Zero ? delay : TimeSpan.Zero;
            Monitor.PulseAll(_sync);
        }
    }

    public void Enqueue(byte[] bytes, bool isKeyFrame, long presentationTimeUs)
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
                EnqueueLocked(bytes, isKeyFrame, presentationTimeUs);
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
                EnqueueLocked(bytes, isKeyFrame, presentationTimeUs);
                Monitor.PulseAll(_sync);
                snapshot = SnapshotLocked();
                PublishStats(snapshot);
                return;
            }

            if (_preferLatestQueuedUnit && _queue.Count > 0)
            {
                _droppedAccessUnits += _queue.Count;
                ClearQueuedLocked();
                EnqueueLocked(bytes, isKeyFrame, presentationTimeUs);
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

                var canAcceptAsFreshest = _current is null && _queue.Count == 0;
                if (isKeyFrame || canAcceptAsFreshest)
                {
                    EnqueueLocked(bytes, isKeyFrame, presentationTimeUs);
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

            EnqueueLocked(bytes, isKeyFrame, presentationTimeUs);
            Monitor.PulseAll(_sync);
            snapshot = SnapshotLocked();
        }

        PublishStats(snapshot);
    }

    public bool TryDequeue(CancellationToken token, out byte[]? bytes, out bool isKeyFrame, out long presentationTimeUs)
    {
        bytes = null;
        isKeyFrame = false;
        presentationTimeUs = 0;

        lock (_sync)
        {
            while (true)
            {
                if (_current is null && _queue.Count > 0)
                {
                    _current = _queue.Dequeue();
                    _queuedBytes -= _current.Bytes.Length;
                }

                if (_current is not null)
                {
                    if (_jitterBufferDelay > TimeSpan.Zero && _queue.Count == 0)
                    {
                        var age = Stopwatch.GetElapsedTime(_current.EnqueuedAtTicks);
                        if (age < _jitterBufferDelay)
                        {
                            var waitMs = Math.Max(1, (int)Math.Ceiling((_jitterBufferDelay - age).TotalMilliseconds));
                            Monitor.Wait(_sync, waitMs);
                            continue;
                        }
                    }

                    bytes = _current.Bytes;
                    isKeyFrame = _current.IsKeyFrame;
                    presentationTimeUs = _current.PresentationTimeUs;
                    _current = null;
                    PublishStats(SnapshotLocked());
                    return true;
                }

                if (_closed || token.IsCancellationRequested)
                {
                    return false;
                }

                Monitor.Wait(_sync, 50);
            }
        }
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

    public void Flush(bool waitForKeyFrame)
    {
        PlaybackStreamStats snapshot;
        lock (_sync)
        {
            ClearQueuedLocked();
            DropCurrentLocked();
            _waitingForKeyFrame = waitForKeyFrame;
            snapshot = SnapshotLocked();
            Monitor.PulseAll(_sync);
        }

        PublishStats(snapshot);
    }

    public void Dispose()
    {
        PlaybackStreamStats snapshot;
        lock (_sync)
        {
            _closed = true;
            ClearQueuedLocked();
            DropCurrentLocked();
            snapshot = SnapshotLocked();
            Monitor.PulseAll(_sync);
        }

        PublishStats(snapshot);
    }

    private void EnqueueLocked(byte[] bytes, bool isKeyFrame, long presentationTimeUs)
    {
        _queue.Enqueue(new QueuedUnit(bytes, isKeyFrame, presentationTimeUs, Stopwatch.GetTimestamp()));
        _queuedBytes += bytes.Length;
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
    }

    private PlaybackStreamStats SnapshotLocked()
    {
        var currentBytes = _current?.Bytes.Length ?? 0;
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
