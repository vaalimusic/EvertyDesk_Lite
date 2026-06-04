namespace ReceiverNative;

internal sealed class TuningOverlayForm : Form
{
    private static readonly Color WindowColor = Color.FromArgb(18, 20, 24);
    private static readonly Color SurfaceColor = Color.FromArgb(28, 31, 37);
    private static readonly Color AccentColor = Color.FromArgb(72, 143, 255);
    private static readonly Color ForegroundColor = Color.FromArgb(228, 233, 241);
    private static readonly Color MutedForegroundColor = Color.FromArgb(180, 188, 201);

    private readonly ComboBox _backendBox = new()
    {
        Width = 220,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<PlaybackBackendKind>(),
    };

    private readonly ComboBox _decoderBox = new()
    {
        Width = 180,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<HardwareDecodeMode>(),
    };

    private readonly CheckBox _ultraLowLatencyCheck = new()
    {
        Text = "Ultra low latency",
        AutoSize = true,
    };

    private readonly CheckBox _aggressiveTailDropCheck = new()
    {
        Text = "Aggressive tail-drop",
        AutoSize = true,
    };

    private readonly CheckBox _topMostCheck = new()
    {
        Text = "Pin on top",
        Checked = false,
        AutoSize = true,
    };

    private readonly Button _gamePresetButton = new() { Text = "Game", AutoSize = true };
    private readonly Button _balancedPresetButton = new() { Text = "Balanced", AutoSize = true };
    private readonly Button _cinemaPresetButton = new() { Text = "Cinema", AutoSize = true };
    private readonly Button _defaultsButton = new() { Text = "Defaults", AutoSize = true };
    private readonly NumericUpDown _jitterBox = new()
    {
        Minimum = 0,
        Maximum = 80,
        Width = 90,
    };
    private readonly NumericUpDown _audioBufferBox = new()
    {
        Minimum = 0,
        Maximum = 1500,
        Increment = 20,
        Width = 90,
    };
    private readonly NumericUpDown _pacingMinBox = new()
    {
        Minimum = 0,
        Maximum = 50,
        DecimalPlaces = 1,
        Increment = 1,
        Width = 90,
    };
    private readonly NumericUpDown _pacingMaxBox = new()
    {
        Minimum = 0,
        Maximum = 50,
        DecimalPlaces = 1,
        Increment = 1,
        Width = 90,
    };
    private readonly NumericUpDown _catchUpBox = new()
    {
        Minimum = 0,
        Maximum = 120,
        Width = 90,
    };
    private readonly NumericUpDown _idrCooldownBox = new()
    {
        Minimum = 0,
        Maximum = 2000,
        Increment = 20,
        Width = 90,
    };
    private readonly NumericUpDown _panicQueueBox = new()
    {
        Minimum = 0,
        Maximum = 12,
        Width = 90,
    };
    private readonly NumericUpDown _feedbackTickBox = new()
    {
        Minimum = 0,
        Maximum = 500,
        Increment = 10,
        Width = 90,
    };
    private readonly NumericUpDown _highDeltaBox = new()
    {
        Minimum = 0,
        Maximum = 120,
        Width = 90,
    };
    private readonly NumericUpDown _criticalDeltaBox = new()
    {
        Minimum = 0,
        Maximum = 180,
        Width = 90,
    };
    private readonly NumericUpDown _startupGraceBox = new()
    {
        Minimum = 0,
        Maximum = 4000,
        Increment = 50,
        Width = 90,
    };
    private readonly NumericUpDown _dropBurstBox = new()
    {
        Minimum = 0,
        Maximum = 20,
        Width = 90,
    };
    private readonly TrackBar _jitterTrack = TrackBarFor(0, 80, 1);
    private readonly TrackBar _audioBufferTrack = TrackBarFor(0, 1500, 20);
    private readonly TrackBar _pacingMinTrack = TrackBarFor(0, 500, 1);
    private readonly TrackBar _pacingMaxTrack = TrackBarFor(0, 500, 1);
    private readonly TrackBar _catchUpTrack = TrackBarFor(0, 120, 1);
    private readonly TrackBar _idrCooldownTrack = TrackBarFor(0, 2000, 20);
    private readonly TrackBar _panicQueueTrack = TrackBarFor(0, 12, 1);
    private readonly TrackBar _feedbackTickTrack = TrackBarFor(0, 500, 10);
    private readonly TrackBar _highDeltaTrack = TrackBarFor(0, 120, 1);
    private readonly TrackBar _criticalDeltaTrack = TrackBarFor(0, 180, 1);
    private readonly TrackBar _startupGraceTrack = TrackBarFor(0, 4000, 50);
    private readonly TrackBar _dropBurstTrack = TrackBarFor(0, 20, 1);
    private readonly System.Windows.Forms.Timer _jitterDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _audioBufferDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _pacingDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _catchUpDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _idrCooldownDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _panicQueueDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _feedbackTickDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _highDeltaDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _criticalDeltaDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _startupGraceDebounceTimer = new() { Interval = 180 };
    private readonly System.Windows.Forms.Timer _dropBurstDebounceTimer = new() { Interval = 180 };

    private readonly Label _modeValue = ValueLabel();
    private readonly Label _fpsValue = ValueLabel();
    private readonly Label _arrivalValue = ValueLabel();
    private readonly Label _decodeValue = ValueLabel();
    private readonly Label _presentValue = ValueLabel();
    private readonly Label _dropsValue = ValueLabel();
    private readonly Label _queueValue = ValueLabel();
    private readonly Label _audioValue = ValueLabel();

    private bool _suppressEvents;

    public TuningOverlayForm()
    {
        Text = "Everty Tuning";
        FormBorderStyle = FormBorderStyle.SizableToolWindow;
        ShowInTaskbar = false;
        StartPosition = FormStartPosition.Manual;
        TopMost = false;
        MinimumSize = new Size(520, 520);
        Size = new Size(620, 620);
        BackColor = WindowColor;
        AutoScroll = false;

        ApplyTheme();
        BuildLayout();
        BindEvents();
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            _jitterDebounceTimer.Dispose();
            _audioBufferDebounceTimer.Dispose();
            _pacingDebounceTimer.Dispose();
            _catchUpDebounceTimer.Dispose();
            _idrCooldownDebounceTimer.Dispose();
            _panicQueueDebounceTimer.Dispose();
            _feedbackTickDebounceTimer.Dispose();
            _highDeltaDebounceTimer.Dispose();
            _criticalDeltaDebounceTimer.Dispose();
            _startupGraceDebounceTimer.Dispose();
            _dropBurstDebounceTimer.Dispose();
        }

        base.Dispose(disposing);
    }

    public event Action<PlaybackBackendKind>? BackendChanged;
    public event Action<HardwareDecodeMode>? DecoderChanged;
    public event Action<bool>? UltraLowLatencyChanged;
    public event Action<bool>? AggressiveTailDropChanged;
    public event Action? GamePresetRequested;
    public event Action? BalancedPresetRequested;
    public event Action? CinemaPresetRequested;
    public event Action? DefaultsRequested;
    public event Action<int>? JitterChanged;
    public event Action<int>? AudioBufferChanged;
    public event Action<int, int>? PacingChanged;
    public event Action<int>? CatchUpChanged;
    public event Action<int>? IdrCooldownChanged;
    public event Action<int>? PanicQueueChanged;
    public event Action<int>? FeedbackTickChanged;
    public event Action<int>? HighDeltaChanged;
    public event Action<int>? CriticalDeltaChanged;
    public event Action<int>? StartupGraceChanged;
    public event Action<int>? DropBurstChanged;

    public void SyncState(
        PlaybackBackendKind backend,
        HardwareDecodeMode decoder,
        bool ultraLowLatency,
        bool aggressiveTailDrop,
        int jitterMs = 0,
        int audioBufferMs = 0,
        int pacingMinTenthsMs = 0,
        int pacingMaxTenthsMs = 0,
        int catchUpMs = 0,
        int idrCooldownMs = 0,
        int panicQueueAu = 0,
        int feedbackTickMs = 0,
        int highDeltaMs = 0,
        int criticalDeltaMs = 0,
        int startupGraceMs = 0,
        int dropBurst = 0)
    {
        _suppressEvents = true;
        try
        {
            _jitterDebounceTimer.Stop();
            _audioBufferDebounceTimer.Stop();
            _pacingDebounceTimer.Stop();
            _catchUpDebounceTimer.Stop();
            _idrCooldownDebounceTimer.Stop();
            _panicQueueDebounceTimer.Stop();
            _feedbackTickDebounceTimer.Stop();
            _highDeltaDebounceTimer.Stop();
            _criticalDeltaDebounceTimer.Stop();
            _startupGraceDebounceTimer.Stop();
            _dropBurstDebounceTimer.Stop();
            _backendBox.SelectedItem = backend;
            _decoderBox.SelectedItem = decoder;
            _ultraLowLatencyCheck.Checked = ultraLowLatency;
            _aggressiveTailDropCheck.Checked = aggressiveTailDrop;
            _jitterBox.Value = Math.Clamp(jitterMs, (int)_jitterBox.Minimum, (int)_jitterBox.Maximum);
            _audioBufferBox.Value = Math.Clamp(audioBufferMs, (int)_audioBufferBox.Minimum, (int)_audioBufferBox.Maximum);
            _pacingMinBox.Value = Math.Clamp(pacingMinTenthsMs / 10m, _pacingMinBox.Minimum, _pacingMinBox.Maximum);
            _pacingMaxBox.Value = Math.Clamp(pacingMaxTenthsMs / 10m, _pacingMaxBox.Minimum, _pacingMaxBox.Maximum);
            _catchUpBox.Value = Math.Clamp(catchUpMs, (int)_catchUpBox.Minimum, (int)_catchUpBox.Maximum);
            _idrCooldownBox.Value = Math.Clamp(idrCooldownMs, (int)_idrCooldownBox.Minimum, (int)_idrCooldownBox.Maximum);
            _panicQueueBox.Value = Math.Clamp(panicQueueAu, (int)_panicQueueBox.Minimum, (int)_panicQueueBox.Maximum);
            _feedbackTickBox.Value = Math.Clamp(feedbackTickMs, (int)_feedbackTickBox.Minimum, (int)_feedbackTickBox.Maximum);
            _highDeltaBox.Value = Math.Clamp(highDeltaMs, (int)_highDeltaBox.Minimum, (int)_highDeltaBox.Maximum);
            _criticalDeltaBox.Value = Math.Clamp(criticalDeltaMs, (int)_criticalDeltaBox.Minimum, (int)_criticalDeltaBox.Maximum);
            _startupGraceBox.Value = Math.Clamp(startupGraceMs, (int)_startupGraceBox.Minimum, (int)_startupGraceBox.Maximum);
            _dropBurstBox.Value = Math.Clamp(dropBurst, (int)_dropBurstBox.Minimum, (int)_dropBurstBox.Maximum);
            SetTrackValueClamped(_jitterTrack, (int)_jitterBox.Value);
            SetTrackValueClamped(_audioBufferTrack, (int)_audioBufferBox.Value);
            SetTrackValueClamped(_pacingMinTrack, (int)Math.Round(_pacingMinBox.Value * 10m));
            SetTrackValueClamped(_pacingMaxTrack, (int)Math.Round(_pacingMaxBox.Value * 10m));
            SetTrackValueClamped(_catchUpTrack, (int)_catchUpBox.Value);
            SetTrackValueClamped(_idrCooldownTrack, (int)_idrCooldownBox.Value);
            SetTrackValueClamped(_panicQueueTrack, (int)_panicQueueBox.Value);
            SetTrackValueClamped(_feedbackTickTrack, (int)_feedbackTickBox.Value);
            SetTrackValueClamped(_highDeltaTrack, (int)_highDeltaBox.Value);
            SetTrackValueClamped(_criticalDeltaTrack, (int)_criticalDeltaBox.Value);
            SetTrackValueClamped(_startupGraceTrack, (int)_startupGraceBox.Value);
            SetTrackValueClamped(_dropBurstTrack, (int)_dropBurstBox.Value);
        }
        finally
        {
            _suppressEvents = false;
        }
    }

    public void UpdateSnapshot(ReceiverSessionSnapshot snapshot)
    {
        SuspendLayout();
        try
        {
            SetLabelText(_modeValue, $"{snapshot.TransportMode} | {snapshot.PlaybackBackend}");
            SetLabelText(_fpsValue, $"{snapshot.InputFpsProxy} / {snapshot.TargetFps}");
            SetLabelText(_arrivalValue, FormatDelta(snapshot.ArrivalDeltaMs));
            SetLabelText(_decodeValue, FormatDelta(snapshot.DecodeDeltaMs));
            SetLabelText(_presentValue, FormatDelta(snapshot.PresentDeltaMs));
            SetLabelText(_dropsValue, $"{snapshot.TotalDroppedFrames} total / {snapshot.StreamDroppedAccessUnits} queue");
            SetLabelText(_queueValue, $"{snapshot.StreamQueuedAccessUnits} AU / {snapshot.StreamQueuedKilobytes} KB");
            SetLabelText(_audioValue, $"{snapshot.AudioPackets} pkt | {snapshot.PlaybackStatus}");
        }
        finally
        {
            ResumeLayout(false);
        }
    }

    private void BuildLayout()
    {
        var root = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            ColumnCount = 1,
            RowCount = 5,
            BackColor = WindowColor,
            Padding = Padding.Empty,
            Margin = Padding.Empty,
            GrowStyle = TableLayoutPanelGrowStyle.AddRows,
        };
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));

        var controls = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            ColumnCount = 2,
            BackColor = WindowColor,
        };
        controls.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
        controls.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100f));
        controls.Controls.Add(LabelFor("Backend"), 0, 0);
        controls.Controls.Add(_backendBox, 1, 0);
        controls.Controls.Add(LabelFor("Decoder"), 0, 1);
        controls.Controls.Add(_decoderBox, 1, 1);

        var toggles = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            FlowDirection = FlowDirection.TopDown,
            WrapContents = false,
            BackColor = WindowColor,
            Margin = new Padding(0, 10, 0, 0),
        };
        toggles.Controls.Add(_ultraLowLatencyCheck);
        toggles.Controls.Add(_aggressiveTailDropCheck);
        toggles.Controls.Add(_topMostCheck);

        var presets = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            FlowDirection = FlowDirection.LeftToRight,
            WrapContents = false,
            BackColor = WindowColor,
            Margin = new Padding(0, 10, 0, 0),
        };
        presets.Controls.AddRange(new Control[] { _gamePresetButton, _balancedPresetButton, _cinemaPresetButton, _defaultsButton });

        var tuning = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            ColumnCount = 3,
            BackColor = WindowColor,
            Margin = new Padding(0, 10, 0, 0),
        };
        tuning.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
        tuning.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
        tuning.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100f));
        tuning.Controls.Add(LabelFor("Jitter ms"), 0, 0);
        tuning.Controls.Add(_jitterBox, 1, 0);
        tuning.Controls.Add(_jitterTrack, 2, 0);
        tuning.Controls.Add(LabelFor("Audio buf"), 0, 1);
        tuning.Controls.Add(_audioBufferBox, 1, 1);
        tuning.Controls.Add(_audioBufferTrack, 2, 1);
        tuning.Controls.Add(LabelFor("Pacing min"), 0, 2);
        tuning.Controls.Add(_pacingMinBox, 1, 2);
        tuning.Controls.Add(_pacingMinTrack, 2, 2);
        tuning.Controls.Add(LabelFor("Pacing max"), 0, 3);
        tuning.Controls.Add(_pacingMaxBox, 1, 3);
        tuning.Controls.Add(_pacingMaxTrack, 2, 3);
        tuning.Controls.Add(LabelFor("Catch-up ms"), 0, 4);
        tuning.Controls.Add(_catchUpBox, 1, 4);
        tuning.Controls.Add(_catchUpTrack, 2, 4);
        tuning.Controls.Add(LabelFor("IDR cooldown"), 0, 5);
        tuning.Controls.Add(_idrCooldownBox, 1, 5);
        tuning.Controls.Add(_idrCooldownTrack, 2, 5);
        tuning.Controls.Add(LabelFor("Panic queue"), 0, 6);
        tuning.Controls.Add(_panicQueueBox, 1, 6);
        tuning.Controls.Add(_panicQueueTrack, 2, 6);
        tuning.Controls.Add(LabelFor("Feedback tick"), 0, 7);
        tuning.Controls.Add(_feedbackTickBox, 1, 7);
        tuning.Controls.Add(_feedbackTickTrack, 2, 7);
        tuning.Controls.Add(LabelFor("High delta"), 0, 8);
        tuning.Controls.Add(_highDeltaBox, 1, 8);
        tuning.Controls.Add(_highDeltaTrack, 2, 8);
        tuning.Controls.Add(LabelFor("Critical delta"), 0, 9);
        tuning.Controls.Add(_criticalDeltaBox, 1, 9);
        tuning.Controls.Add(_criticalDeltaTrack, 2, 9);
        tuning.Controls.Add(LabelFor("Startup grace"), 0, 10);
        tuning.Controls.Add(_startupGraceBox, 1, 10);
        tuning.Controls.Add(_startupGraceTrack, 2, 10);
        tuning.Controls.Add(LabelFor("Drop burst"), 0, 11);
        tuning.Controls.Add(_dropBurstBox, 1, 11);
        tuning.Controls.Add(_dropBurstTrack, 2, 11);

        var metrics = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            ColumnCount = 2,
            BackColor = SurfaceColor,
            Padding = new Padding(10),
            Margin = new Padding(0, 12, 0, 0),
        };
        metrics.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));
        metrics.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100f));
        AddMetric(metrics, 0, "Mode", _modeValue);
        AddMetric(metrics, 1, "Input FPS", _fpsValue);
        AddMetric(metrics, 2, "Arrival", _arrivalValue);
        AddMetric(metrics, 3, "Decode", _decodeValue);
        AddMetric(metrics, 4, "Present", _presentValue);
        AddMetric(metrics, 5, "Drops", _dropsValue);
        AddMetric(metrics, 6, "Queue", _queueValue);
        AddMetric(metrics, 7, "Audio", _audioValue);

        root.Controls.Add(controls, 0, 0);
        root.Controls.Add(toggles, 0, 1);
        root.Controls.Add(presets, 0, 2);
        root.Controls.Add(tuning, 0, 3);
        root.Controls.Add(metrics, 0, 4);

        var scrollHost = new Panel
        {
            Dock = DockStyle.Fill,
            AutoScroll = true,
            BackColor = WindowColor,
            Padding = new Padding(10),
        };
        scrollHost.Resize += (_, _) =>
        {
            root.Width = Math.Max(540, scrollHost.ClientSize.Width - SystemInformation.VerticalScrollBarWidth - 8);
        };
        scrollHost.Controls.Add(root);
        Controls.Add(scrollHost);
    }

    private void BindEvents()
    {
        _backendBox.Format += (_, args) =>
        {
            if (args.ListItem is PlaybackBackendKind kind)
            {
                args.Value = kind.ToUiLabel();
            }
        };
        _decoderBox.Format += (_, args) =>
        {
            if (args.ListItem is HardwareDecodeMode mode)
            {
                args.Value = mode.ToUiLabel();
            }
        };
        _backendBox.SelectedIndexChanged += (_, _) =>
        {
            if (_suppressEvents)
            {
                return;
            }

            if (_backendBox.SelectedItem is PlaybackBackendKind backend)
            {
                BackendChanged?.Invoke(backend);
            }
        };
        _decoderBox.SelectedIndexChanged += (_, _) =>
        {
            if (_suppressEvents)
            {
                return;
            }

            if (_decoderBox.SelectedItem is HardwareDecodeMode decoder)
            {
                DecoderChanged?.Invoke(decoder);
            }
        };
        _ultraLowLatencyCheck.CheckedChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                UltraLowLatencyChanged?.Invoke(_ultraLowLatencyCheck.Checked);
            }
        };
        _aggressiveTailDropCheck.CheckedChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                AggressiveTailDropChanged?.Invoke(_aggressiveTailDropCheck.Checked);
            }
        };
        _topMostCheck.CheckedChanged += (_, _) => TopMost = _topMostCheck.Checked;
        _gamePresetButton.Click += (_, _) => GamePresetRequested?.Invoke();
        _balancedPresetButton.Click += (_, _) => BalancedPresetRequested?.Invoke();
        _cinemaPresetButton.Click += (_, _) => CinemaPresetRequested?.Invoke();
        _defaultsButton.Click += (_, _) => DefaultsRequested?.Invoke();
        _jitterDebounceTimer.Tick += (_, _) =>
        {
            _jitterDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                JitterChanged?.Invoke((int)_jitterBox.Value);
            }
        };
        _audioBufferDebounceTimer.Tick += (_, _) =>
        {
            _audioBufferDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                AudioBufferChanged?.Invoke((int)_audioBufferBox.Value);
            }
        };
        _catchUpDebounceTimer.Tick += (_, _) =>
        {
            _catchUpDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                CatchUpChanged?.Invoke((int)_catchUpBox.Value);
            }
        };
        _idrCooldownDebounceTimer.Tick += (_, _) =>
        {
            _idrCooldownDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                IdrCooldownChanged?.Invoke((int)_idrCooldownBox.Value);
            }
        };
        _panicQueueDebounceTimer.Tick += (_, _) =>
        {
            _panicQueueDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                PanicQueueChanged?.Invoke((int)_panicQueueBox.Value);
            }
        };
        _feedbackTickDebounceTimer.Tick += (_, _) =>
        {
            _feedbackTickDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                FeedbackTickChanged?.Invoke((int)_feedbackTickBox.Value);
            }
        };
        _highDeltaDebounceTimer.Tick += (_, _) =>
        {
            _highDeltaDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                HighDeltaChanged?.Invoke((int)_highDeltaBox.Value);
            }
        };
        _criticalDeltaDebounceTimer.Tick += (_, _) =>
        {
            _criticalDeltaDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                CriticalDeltaChanged?.Invoke((int)_criticalDeltaBox.Value);
            }
        };
        _startupGraceDebounceTimer.Tick += (_, _) =>
        {
            _startupGraceDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                StartupGraceChanged?.Invoke((int)_startupGraceBox.Value);
            }
        };
        _dropBurstDebounceTimer.Tick += (_, _) =>
        {
            _dropBurstDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                DropBurstChanged?.Invoke((int)_dropBurstBox.Value);
            }
        };
        _pacingDebounceTimer.Tick += (_, _) =>
        {
            _pacingDebounceTimer.Stop();
            if (!_suppressEvents)
            {
                PacingChanged?.Invoke((int)Math.Round(_pacingMinBox.Value * 10m), (int)Math.Round(_pacingMaxBox.Value * 10m));
            }
        };
        _jitterBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_jitterTrack, (int)_jitterBox.Value);
                _jitterDebounceTimer.Stop();
                _jitterDebounceTimer.Start();
            }
        };
        _audioBufferBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_audioBufferTrack, (int)_audioBufferBox.Value);
                _audioBufferDebounceTimer.Stop();
                _audioBufferDebounceTimer.Start();
            }
        };
        _catchUpBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_catchUpTrack, (int)_catchUpBox.Value);
                _catchUpDebounceTimer.Stop();
                _catchUpDebounceTimer.Start();
            }
        };
        _idrCooldownBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_idrCooldownTrack, (int)_idrCooldownBox.Value);
                _idrCooldownDebounceTimer.Stop();
                _idrCooldownDebounceTimer.Start();
            }
        };
        _panicQueueBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_panicQueueTrack, (int)_panicQueueBox.Value);
                _panicQueueDebounceTimer.Stop();
                _panicQueueDebounceTimer.Start();
            }
        };
        _feedbackTickBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_feedbackTickTrack, (int)_feedbackTickBox.Value);
                _feedbackTickDebounceTimer.Stop();
                _feedbackTickDebounceTimer.Start();
            }
        };
        _highDeltaBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_highDeltaTrack, (int)_highDeltaBox.Value);
                _highDeltaDebounceTimer.Stop();
                _highDeltaDebounceTimer.Start();
            }
        };
        _criticalDeltaBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_criticalDeltaTrack, (int)_criticalDeltaBox.Value);
                _criticalDeltaDebounceTimer.Stop();
                _criticalDeltaDebounceTimer.Start();
            }
        };
        _startupGraceBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_startupGraceTrack, (int)_startupGraceBox.Value);
                _startupGraceDebounceTimer.Stop();
                _startupGraceDebounceTimer.Start();
            }
        };
        _dropBurstBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_dropBurstTrack, (int)_dropBurstBox.Value);
                _dropBurstDebounceTimer.Stop();
                _dropBurstDebounceTimer.Start();
            }
        };
        _pacingMinBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_pacingMinTrack, (int)Math.Round(_pacingMinBox.Value * 10m));
                _pacingDebounceTimer.Stop();
                _pacingDebounceTimer.Start();
            }
        };
        _pacingMaxBox.ValueChanged += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncTrackBar(_pacingMaxTrack, (int)Math.Round(_pacingMaxBox.Value * 10m));
                _pacingDebounceTimer.Stop();
                _pacingDebounceTimer.Start();
            }
        };
        _jitterTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_jitterBox, _jitterTrack.Value);
                _jitterDebounceTimer.Stop();
                _jitterDebounceTimer.Start();
            }
        };
        _audioBufferTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_audioBufferBox, _audioBufferTrack.Value);
                _audioBufferDebounceTimer.Stop();
                _audioBufferDebounceTimer.Start();
            }
        };
        _catchUpTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_catchUpBox, _catchUpTrack.Value);
                _catchUpDebounceTimer.Stop();
                _catchUpDebounceTimer.Start();
            }
        };
        _idrCooldownTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_idrCooldownBox, _idrCooldownTrack.Value);
                _idrCooldownDebounceTimer.Stop();
                _idrCooldownDebounceTimer.Start();
            }
        };
        _panicQueueTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_panicQueueBox, _panicQueueTrack.Value);
                _panicQueueDebounceTimer.Stop();
                _panicQueueDebounceTimer.Start();
            }
        };
        _feedbackTickTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_feedbackTickBox, _feedbackTickTrack.Value);
                _feedbackTickDebounceTimer.Stop();
                _feedbackTickDebounceTimer.Start();
            }
        };
        _highDeltaTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_highDeltaBox, _highDeltaTrack.Value);
                _highDeltaDebounceTimer.Stop();
                _highDeltaDebounceTimer.Start();
            }
        };
        _criticalDeltaTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_criticalDeltaBox, _criticalDeltaTrack.Value);
                _criticalDeltaDebounceTimer.Stop();
                _criticalDeltaDebounceTimer.Start();
            }
        };
        _startupGraceTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_startupGraceBox, _startupGraceTrack.Value);
                _startupGraceDebounceTimer.Stop();
                _startupGraceDebounceTimer.Start();
            }
        };
        _dropBurstTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncNumeric(_dropBurstBox, _dropBurstTrack.Value);
                _dropBurstDebounceTimer.Stop();
                _dropBurstDebounceTimer.Start();
            }
        };
        _pacingMinTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncDecimalNumeric(_pacingMinBox, _pacingMinTrack.Value);
                _pacingDebounceTimer.Stop();
                _pacingDebounceTimer.Start();
            }
        };
        _pacingMaxTrack.Scroll += (_, _) =>
        {
            if (!_suppressEvents)
            {
                SyncDecimalNumeric(_pacingMaxBox, _pacingMaxTrack.Value);
                _pacingDebounceTimer.Stop();
                _pacingDebounceTimer.Start();
            }
        };
    }

    private void ApplyTheme()
    {
        ForeColor = ForegroundColor;
        _backendBox.BackColor = SurfaceColor;
        _backendBox.ForeColor = ForegroundColor;
        _backendBox.FlatStyle = FlatStyle.Popup;
        _decoderBox.BackColor = SurfaceColor;
        _decoderBox.ForeColor = ForegroundColor;
        _decoderBox.FlatStyle = FlatStyle.Popup;
        _jitterBox.BackColor = SurfaceColor;
        _jitterBox.ForeColor = ForegroundColor;
        _audioBufferBox.BackColor = SurfaceColor;
        _audioBufferBox.ForeColor = ForegroundColor;
        _catchUpBox.BackColor = SurfaceColor;
        _catchUpBox.ForeColor = ForegroundColor;
        _idrCooldownBox.BackColor = SurfaceColor;
        _idrCooldownBox.ForeColor = ForegroundColor;
        _panicQueueBox.BackColor = SurfaceColor;
        _panicQueueBox.ForeColor = ForegroundColor;
        _feedbackTickBox.BackColor = SurfaceColor;
        _feedbackTickBox.ForeColor = ForegroundColor;
        _highDeltaBox.BackColor = SurfaceColor;
        _highDeltaBox.ForeColor = ForegroundColor;
        _criticalDeltaBox.BackColor = SurfaceColor;
        _criticalDeltaBox.ForeColor = ForegroundColor;
        _startupGraceBox.BackColor = SurfaceColor;
        _startupGraceBox.ForeColor = ForegroundColor;
        _dropBurstBox.BackColor = SurfaceColor;
        _dropBurstBox.ForeColor = ForegroundColor;
        _pacingMinBox.BackColor = SurfaceColor;
        _pacingMinBox.ForeColor = ForegroundColor;
        _pacingMaxBox.BackColor = SurfaceColor;
        _pacingMaxBox.ForeColor = ForegroundColor;
        StyleTrackBar(_jitterTrack);
        StyleTrackBar(_audioBufferTrack);
        StyleTrackBar(_pacingMinTrack);
        StyleTrackBar(_pacingMaxTrack);
        StyleTrackBar(_catchUpTrack);
        StyleTrackBar(_idrCooldownTrack);
        StyleTrackBar(_panicQueueTrack);
        StyleTrackBar(_feedbackTickTrack);
        StyleTrackBar(_highDeltaTrack);
        StyleTrackBar(_criticalDeltaTrack);
        StyleTrackBar(_startupGraceTrack);
        StyleTrackBar(_dropBurstTrack);
        _ultraLowLatencyCheck.ForeColor = ForegroundColor;
        _ultraLowLatencyCheck.BackColor = WindowColor;
        _aggressiveTailDropCheck.ForeColor = ForegroundColor;
        _aggressiveTailDropCheck.BackColor = WindowColor;
        _topMostCheck.ForeColor = ForegroundColor;
        _topMostCheck.BackColor = WindowColor;
        StyleButton(_gamePresetButton);
        StyleButton(_balancedPresetButton);
        StyleButton(_cinemaPresetButton);
        StyleButton(_defaultsButton);
    }

    private static void AddMetric(TableLayoutPanel table, int row, string name, Control value)
    {
        table.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        table.Controls.Add(new Label
        {
            Text = name,
            AutoSize = true,
            ForeColor = MutedForegroundColor,
            Margin = new Padding(0, 0, 10, 6),
        }, 0, row);
        table.Controls.Add(value, 1, row);
    }

    private static Label LabelFor(string text)
    {
        return new Label
        {
            Text = text,
            AutoSize = true,
            ForeColor = MutedForegroundColor,
            Margin = new Padding(0, 7, 8, 0),
        };
    }

    private static Label ValueLabel()
    {
        return new Label
        {
            AutoSize = false,
            Width = 260,
            ForeColor = ForegroundColor,
            Margin = new Padding(0, 0, 0, 6),
        };
    }

    private static void SetLabelText(Label label, string value)
    {
        if (!string.Equals(label.Text, value, StringComparison.Ordinal))
        {
            label.Text = value;
        }
    }

    private static void StyleButton(Button button)
    {
        button.AutoSize = false;
        button.Size = new Size(96, 28);
        button.FlatStyle = FlatStyle.Flat;
        button.FlatAppearance.BorderSize = 1;
        button.FlatAppearance.BorderColor = AccentColor;
        button.BackColor = SurfaceColor;
        button.ForeColor = ForegroundColor;
        button.Margin = new Padding(0, 0, 8, 0);
    }

    private static TrackBar TrackBarFor(int min, int max, int smallChange)
    {
        return new TrackBar
        {
            Minimum = min,
            Maximum = max,
            SmallChange = Math.Max(1, smallChange),
            LargeChange = Math.Max(1, smallChange * 5),
            TickStyle = TickStyle.None,
            AutoSize = false,
            Height = 28,
            Width = 260,
            Margin = new Padding(8, 2, 0, 2),
        };
    }

    private static void StyleTrackBar(TrackBar trackBar)
    {
        trackBar.BackColor = WindowColor;
    }

    private void SyncTrackBar(TrackBar trackBar, int value)
    {
        var previous = _suppressEvents;
        _suppressEvents = true;
        try
        {
            SetTrackValueClamped(trackBar, value);
        }
        finally
        {
            _suppressEvents = previous;
        }
    }

    private void SyncNumeric(NumericUpDown box, int value)
    {
        var previous = _suppressEvents;
        _suppressEvents = true;
        try
        {
            box.Value = Math.Clamp(value, (int)box.Minimum, (int)box.Maximum);
        }
        finally
        {
            _suppressEvents = previous;
        }
    }

    private void SyncDecimalNumeric(NumericUpDown box, int tenths)
    {
        var previous = _suppressEvents;
        _suppressEvents = true;
        try
        {
            box.Value = Math.Clamp(tenths / 10m, box.Minimum, box.Maximum);
        }
        finally
        {
            _suppressEvents = previous;
        }
    }

    private static void SetTrackValueClamped(TrackBar trackBar, int value)
    {
        trackBar.Value = Math.Clamp(value, trackBar.Minimum, trackBar.Maximum);
    }

    private static string FormatDelta(int valueMs) => valueMs >= 0 ? $"{valueMs} ms" : "-";
}
