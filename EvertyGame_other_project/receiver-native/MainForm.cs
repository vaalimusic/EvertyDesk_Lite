namespace ReceiverNative;

internal sealed class MainForm : Form
{
    private static readonly Color WindowColor = Color.FromArgb(10, 10, 12);
    private static readonly Color SurfaceColor = Color.FromArgb(24, 26, 31);
    private static readonly Color SurfaceAltColor = Color.FromArgb(31, 35, 42);
    private static readonly Color AccentColor = Color.FromArgb(72, 143, 255);
    private static readonly Color ForegroundColor = Color.FromArgb(228, 233, 241);
    private static readonly Color MutedForegroundColor = Color.FromArgb(216, 223, 233);

    private readonly TextBox _portBox = new() { Text = "5001", Width = 72 };
    private readonly ComboBox _transportBox = new()
    {
        Width = 150,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<ReceiverTransportMode>(),
    };
    private readonly ComboBox _decoderBox = new()
    {
        Width = 180,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<HardwareDecodeMode>(),
    };
    private readonly ComboBox _backendBox = new()
    {
        Width = 260,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<PlaybackBackendKind>(),
    };
    private readonly CheckBox _aggressiveTailDropCheck = new()
    {
        Text = "Aggressive tail-drop",
        Checked = true,
        AutoSize = true,
    };
    private readonly CheckBox _ultraLowLatencyCheck = new()
    {
        Text = "Ultra low latency",
        Checked = true,
        AutoSize = true,
    };
    private readonly Button _prepareAdbButton = new() { Text = "Prepare ADB", AutoSize = true };
    private readonly Button _startButton = new() { Text = "Start", AutoSize = true };
    private readonly Button _stopButton = new() { Text = "Stop", AutoSize = true, Enabled = false };
    private readonly Button _fullscreenButton = new() { Text = "Fullscreen", AutoSize = true };
    private readonly Label _statusLabel = new()
    {
        AutoSize = false,
        Dock = DockStyle.Fill,
        Padding = new Padding(10, 6, 10, 6),
        Text = "Idle",
    };
    private readonly TextBox _hudBox = new()
    {
        Dock = DockStyle.Fill,
        Multiline = true,
        ReadOnly = true,
        BorderStyle = BorderStyle.FixedSingle,
        BackColor = Color.FromArgb(19, 20, 24),
        ForeColor = Color.FromArgb(228, 233, 241),
        ScrollBars = ScrollBars.Both,
        WordWrap = false,
        Font = new Font("Consolas", 10f, FontStyle.Regular, GraphicsUnit.Point),
    };
    private readonly Panel _playbackHost = new()
    {
        Dock = DockStyle.Fill,
        BackColor = Color.Black,
    };
    private readonly System.Windows.Forms.Timer _hudTimer = new() { Interval = 250 };
    private readonly NativeReceiverSession _session;
    private readonly string _startedAtLabel = DateTime.Now.ToString("HH:mm:ss");
    private readonly string _buildLabel = File.GetLastWriteTime(typeof(MainForm).Assembly.Location).ToString("yyyy-MM-dd HH:mm:ss");
    private bool _fullscreen;
    private Rectangle _restoreBounds = Rectangle.Empty;
    private FormBorderStyle _restoreBorderStyle = FormBorderStyle.Sizable;
    private FormWindowState _restoreWindowState = FormWindowState.Normal;
    private string _lastHudText = string.Empty;
    private long _lastHudRefreshAtMs;
    private Task? _sessionActionTask;
    private bool _closeAfterSessionAction;
    private bool _allowClose;
    private string _adbTunnelStatus = "-";
    private string _lastAutoFitResolution = "-";
    private bool? _lastAutoFitLandscape;

    public MainForm()
    {
        Text = "Everty Native Receiver";
        MinimumSize = new Size(1440, 720);
        BackColor = WindowColor;
        KeyPreview = true;

        _session = new NativeReceiverSession(_playbackHost);

        ApplyTheme();
        BuildLayout();
        BindEvents();

        _hudTimer.Tick += (_, _) => RenderSnapshot(_session.GetSnapshot());
        _hudTimer.Start();

        _transportBox.SelectedItem = ReceiverTransportMode.Udp;
        _backendBox.SelectedItem = PlaybackBackendKind.MediaFoundationDirect3D11;
        _decoderBox.SelectedItem = HardwareDecodeMode.Auto;

        RenderSnapshot(_session.GetSnapshot());
    }

    protected override void OnFormClosing(FormClosingEventArgs e)
    {
        if (_allowClose)
        {
            base.OnFormClosing(e);
            return;
        }

        e.Cancel = true;
        BeginSessionAction(closeAfterCompletion: true);
    }

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        base.OnFormClosed(e);
    }

    protected override bool ProcessCmdKey(ref Message msg, Keys keyData)
    {
        if (keyData == Keys.F11 || keyData == (Keys.Alt | Keys.Enter))
        {
            ToggleFullscreen();
            return true;
        }

        if (keyData == Keys.Escape && _fullscreen)
        {
            ToggleFullscreen();
            return true;
        }

        return base.ProcessCmdKey(ref msg, keyData);
    }

    private void BuildLayout()
    {
        var root = new TableLayoutPanel
        {
            Dock = DockStyle.Fill,
            BackColor = WindowColor,
            ColumnCount = 1,
            RowCount = 2,
        };
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        root.RowStyles.Add(new RowStyle(SizeType.Percent, 100f));

        var controls = new FlowLayoutPanel
        {
            Dock = DockStyle.Fill,
            AutoSize = true,
            WrapContents = true,
            Padding = new Padding(10, 10, 10, 0),
            FlowDirection = FlowDirection.LeftToRight,
            BackColor = WindowColor,
        };
        controls.Controls.AddRange(
            new Control[]
            {
                LabelFor("Transport"),
                _transportBox,
                LabelFor("Port"),
                _portBox,
                LabelFor("Backend"),
                _backendBox,
                LabelFor("Decoder"),
                _decoderBox,
                _ultraLowLatencyCheck,
                _aggressiveTailDropCheck,
                _prepareAdbButton,
                _startButton,
                _stopButton,
                _fullscreenButton,
            });
        controls.AutoSizeMode = AutoSizeMode.GrowAndShrink;

        var statusHost = new Panel
        {
            Dock = DockStyle.Fill,
            Padding = new Padding(10, 0, 10, 0),
            BackColor = WindowColor,
        };
        statusHost.Controls.Add(_statusLabel);

        var header = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            BackColor = WindowColor,
            ColumnCount = 1,
            RowCount = 2,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            Margin = Padding.Empty,
            Padding = Padding.Empty,
        };
        header.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        header.RowStyles.Add(new RowStyle(SizeType.Absolute, 36f));
        header.Controls.Add(controls, 0, 0);
        header.Controls.Add(statusHost, 0, 1);

        var content = new TableLayoutPanel
        {
            Dock = DockStyle.Fill,
            BackColor = WindowColor,
            ColumnCount = 2,
            RowCount = 1,
            Margin = Padding.Empty,
            Padding = new Padding(10, 10, 10, 10),
        };
        content.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100f));
        content.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 520f));
        content.RowStyles.Add(new RowStyle(SizeType.Percent, 100f));
        content.Controls.Add(_playbackHost, 0, 0);
        content.Controls.Add(_hudBox, 1, 0);

        root.Controls.Add(header, 0, 0);
        root.Controls.Add(content, 0, 1);
        Controls.Add(root);
    }

    private void BindEvents()
    {
        _startButton.Click += async (_, _) => await StartSessionAsync();
        _stopButton.Click += (_, _) => StopSession();
        _prepareAdbButton.Click += async (_, _) => await PrepareAdbTunnelAsync(showDialogOnFailure: true);
        _fullscreenButton.Click += (_, _) => ToggleFullscreen();
        _transportBox.Format += (_, args) =>
        {
            if (args.ListItem is ReceiverTransportMode mode)
            {
                args.Value = mode.ToUiLabel();
            }
        };
        _transportBox.SelectedIndexChanged += (_, _) =>
        {
            if (_transportBox.SelectedItem is not ReceiverTransportMode transport)
            {
                return;
            }

            _adbTunnelStatus = transport == ReceiverTransportMode.AdbTunnelTcp
                ? "Not prepared"
                : "-";
            RenderSnapshot(_session.GetSnapshot());
        };
        _decoderBox.Format += (_, args) =>
        {
            if (args.ListItem is HardwareDecodeMode mode)
            {
                args.Value = mode.ToUiLabel();
            }
        };
        _backendBox.Format += (_, args) =>
        {
            if (args.ListItem is PlaybackBackendKind kind)
            {
                args.Value = kind.ToUiLabel();
            }
        };
        _backendBox.SelectedIndexChanged += (_, _) =>
        {
            if (_backendBox.SelectedItem is PlaybackBackendKind backend)
            {
                _session.UpdatePlaybackBackend(backend);
            }
        };
        _decoderBox.SelectedIndexChanged += (_, _) =>
        {
            if (_decoderBox.SelectedItem is HardwareDecodeMode mode)
            {
                _session.UpdateHardwareDecodeMode(mode);
            }
        };
        _aggressiveTailDropCheck.CheckedChanged += (_, _) =>
        {
            _session.UpdateAggressiveMode(_aggressiveTailDropCheck.Checked);
        };
        _ultraLowLatencyCheck.CheckedChanged += (_, _) =>
        {
            _session.UpdateUltraLowLatencyMode(_ultraLowLatencyCheck.Checked);
        };
    }

    private async Task StartSessionAsync()
    {
        if (!int.TryParse(_portBox.Text.Trim(), out var port) || port is < 1 or > 65535)
        {
            MessageBox.Show(this, "Enter a valid listener port", "Invalid port", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        var transport = _transportBox.SelectedItem is ReceiverTransportMode selectedTransport
            ? selectedTransport
            : ReceiverTransportMode.Udp;
        var backend = _backendBox.SelectedItem is PlaybackBackendKind selectedBackend
            ? selectedBackend
            : PlaybackBackendKind.MediaFoundationDirect3D11;
        var mode = _decoderBox.SelectedItem is HardwareDecodeMode selected ? selected : HardwareDecodeMode.Auto;
        if (transport == ReceiverTransportMode.AdbTunnelTcp)
        {
            var prepared = await PrepareAdbTunnelAsync(showDialogOnFailure: true);
            if (!prepared)
            {
                return;
            }
        }

        try
        {
            _session.UpdatePlaybackBackend(backend);
            _session.UpdateUltraLowLatencyMode(_ultraLowLatencyCheck.Checked);
            _session.Start(port, transport, mode, _aggressiveTailDropCheck.Checked);
            _startButton.Enabled = false;
            _stopButton.Enabled = true;
        }
        catch (Exception ex)
        {
            MessageBox.Show(this, ex.Message, "Failed to start receiver", MessageBoxButtons.OK, MessageBoxIcon.Error);
            RenderSnapshot(_session.GetSnapshot());
        }
    }

    private async Task<bool> PrepareAdbTunnelAsync(bool showDialogOnFailure)
    {
        if (!int.TryParse(_portBox.Text.Trim(), out var port) || port is < 1 or > 65535)
        {
            MessageBox.Show(this, "Enter a valid listener port", "Invalid port", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return false;
        }

        _prepareAdbButton.Enabled = false;
        _adbTunnelStatus = "Preparing...";
        RenderSnapshot(_session.GetSnapshot());

        AdbTunnelResult result;
        try
        {
            result = await Task.Run(() => AdbTunnelManager.PrepareReverse(port));
        }
        finally
        {
            _prepareAdbButton.Enabled = true;
        }

        _adbTunnelStatus = result.Message;
        RenderSnapshot(_session.GetSnapshot());

        if (!result.Success && showDialogOnFailure)
        {
            MessageBox.Show(
                this,
                $"Failed to prepare ADB reverse on port {port}.{Environment.NewLine}{Environment.NewLine}{result.Message}",
                "ADB tunnel failed",
                MessageBoxButtons.OK,
                MessageBoxIcon.Error);
        }

        return result.Success;
    }

    private void StopSession()
    {
        BeginSessionAction(closeAfterCompletion: false);
    }

    private void ToggleFullscreen()
    {
        if (_fullscreen)
        {
            FormBorderStyle = _restoreBorderStyle;
            WindowState = _restoreWindowState;
            Bounds = _restoreBounds;
            _fullscreen = false;
            _fullscreenButton.Text = "Fullscreen";
            return;
        }

        _restoreBounds = Bounds;
        _restoreBorderStyle = FormBorderStyle;
        _restoreWindowState = WindowState;
        _fullscreen = true;
        FormBorderStyle = FormBorderStyle.None;
        WindowState = FormWindowState.Normal;
        Bounds = Screen.FromControl(this).Bounds;
        _fullscreenButton.Text = "Windowed";
    }

    private void RenderSnapshot(ReceiverSessionSnapshot snapshot)
    {
        _statusLabel.Text = snapshot.Status;
        MaybeAutoFitWindowToStream(snapshot);
        var showPlaybackError =
            string.Equals(snapshot.PlaybackBackend, PlaybackBackendKind.MediaFoundationDirect3D11.ToUiLabel(), StringComparison.Ordinal) ||
            snapshot.PlaybackStatus.Contains("error", StringComparison.OrdinalIgnoreCase);
        var playbackErrorText = showPlaybackError ? snapshot.LastPlaybackError : "-";
        var adbTunnelText = snapshot.TransportMode.Contains("ADB", StringComparison.OrdinalIgnoreCase)
            ? _adbTunnelStatus
            : "-";
        var hudText = string.Join(
            Environment.NewLine,
            new[]
            {
                "Everty Native Receiver",
                "Native playback backend experiment for ultra-low-latency path",
                $"Started         : {_startedAtLabel}",
                $"Build           : {_buildLabel}",
                string.Empty,
                $"State           : {snapshot.Status}",
                $"Transport       : {snapshot.TransportMode}",
                $"Backend         : {snapshot.PlaybackBackend}",
                $"Playback        : {snapshot.PlaybackStatus}",
                $"Playback error  : {playbackErrorText}",
                $"Backend failure : {snapshot.LastBackendFailure}",
                $"ADB tunnel      : {adbTunnelText}",
                $"Decoder         : {snapshot.DecodeMode}",
                $"Codec           : {snapshot.Codec}",
                $"Preset          : {snapshot.Preset}",
                $"Stream mode     : {snapshot.StreamMode}",
                $"Resolution      : {snapshot.Resolution}",
                $"Target FPS      : {(snapshot.TargetFps > 0 ? snapshot.TargetFps : 0)}",
                $"Bitrate         : {(snapshot.BitrateMbps > 0 ? $"{snapshot.BitrateMbps:0.0} Mbps" : "-")}",
                $"Packets         : {snapshot.PacketsReceived}",
                $"Last packet     : {snapshot.LastPacketType}",
                $"Session packets : {snapshot.SessionConfigPackets}",
                $"Codec cfg pkt   : {snapshot.CodecConfigPackets}",
                $"Video packets   : {snapshot.VideoPackets}",
                $"Audio packets   : {snapshot.AudioPackets}",
                $"Control packets : {snapshot.ControlPackets}",
                $"Frames ready    : {snapshot.FramesAssembled}",
                $"Frames dropped  : {snapshot.TotalDroppedFrames}",
                $"Input FPS proxy : {snapshot.InputFpsProxy}",
                $"Enhancement FPS : {snapshot.EnhancementFps}",
                $"Enhancement drop: {snapshot.EnhancementDroppedFrames}",
                $"Assembly delay  : {snapshot.AssemblyDelayMs} ms",
                $"Arrival delta   : {FormatDelta(snapshot.ArrivalDeltaMs)}",
                $"Decode delta    : {FormatDelta(snapshot.DecodeDeltaMs)}",
                $"Present delta   : {FormatDelta(snapshot.PresentDeltaMs)}",
                $"Adaptive jitter : {snapshot.AdaptiveJitterMs} ms",
                $"Queue           : {snapshot.StreamQueuedAccessUnits} AU / {snapshot.StreamQueuedKilobytes} KB",
                $"Enh queue       : {snapshot.EnhancementQueuedAccessUnits} AU / {snapshot.EnhancementQueuedKilobytes} KB",
                $"Queue drops     : {snapshot.StreamDroppedAccessUnits}",
                $"Waiting keyframe: {snapshot.WaitingForKeyFrame}",
                $"ROI active      : {snapshot.RoiActive}",
                $"ROI rect        : {snapshot.RoiRect}",
                $"Ultra low mode  : {snapshot.UltraLowLatencyMode}",
                $"System hints    : {snapshot.SystemHintsEnabled}",
                $"Remote endpoint : {snapshot.RemoteEndpoint}",
            });

        var nowMs = Environment.TickCount64;
        var activePlayback = snapshot.PlaybackStatus is "Playing" or "Opening";
        var minRefreshMs = activePlayback
            ? (snapshot.UltraLowLatencyMode ? 1500L : 700L)
            : 0L;
        if (hudText == _lastHudText && activePlayback)
        {
            return;
        }
        if (activePlayback && nowMs - _lastHudRefreshAtMs < minRefreshMs)
        {
            return;
        }

        _hudBox.Text = hudText;
        _lastHudText = hudText;
        _lastHudRefreshAtMs = nowMs;
    }

    private void MaybeAutoFitWindowToStream(ReceiverSessionSnapshot snapshot)
    {
        if (_fullscreen || WindowState != FormWindowState.Normal)
        {
            return;
        }

        if (!TryParseResolution(snapshot.Resolution, out var width, out var height))
        {
            return;
        }

        var isLandscape = width >= height;
        var resolutionChanged = !string.Equals(_lastAutoFitResolution, snapshot.Resolution, StringComparison.Ordinal);
        var orientationChanged = _lastAutoFitLandscape != isLandscape;
        if (!resolutionChanged && !orientationChanged)
        {
            return;
        }

        _lastAutoFitResolution = snapshot.Resolution;
        _lastAutoFitLandscape = isLandscape;

        var targetPlaybackWidth = isLandscape ? 1040 : 760;
        var targetPlaybackHeight = isLandscape ? 760 : 1060;
        var targetWidth = Math.Max(MinimumSize.Width, targetPlaybackWidth + 520 + 48);
        var targetHeight = Math.Max(MinimumSize.Height, targetPlaybackHeight + 140);

        Size = new Size(targetWidth, targetHeight);
    }

    private static Label LabelFor(string text)
    {
        return new Label
        {
            Text = text,
            AutoSize = true,
            ForeColor = MutedForegroundColor,
            Margin = new Padding(6, 9, 6, 0),
        };
    }

    private void ApplyTheme()
    {
        ForeColor = ForegroundColor;
        _statusLabel.ForeColor = MutedForegroundColor;
        _statusLabel.BackColor = SurfaceColor;
        _statusLabel.Font = new Font("Segoe UI Semibold", 10f, FontStyle.Regular, GraphicsUnit.Point);

        _portBox.BackColor = SurfaceColor;
        _portBox.ForeColor = ForegroundColor;
        _portBox.BorderStyle = BorderStyle.FixedSingle;
        _portBox.TextAlign = HorizontalAlignment.Center;
        _portBox.Margin = new Padding(4, 5, 10, 0);

        _transportBox.BackColor = SurfaceColor;
        _transportBox.ForeColor = ForegroundColor;
        _transportBox.FlatStyle = FlatStyle.Popup;
        _transportBox.Margin = new Padding(4, 5, 10, 0);

        _backendBox.BackColor = SurfaceColor;
        _backendBox.ForeColor = ForegroundColor;
        _backendBox.FlatStyle = FlatStyle.Popup;
        _backendBox.Margin = new Padding(4, 5, 10, 0);

        _decoderBox.BackColor = SurfaceColor;
        _decoderBox.ForeColor = ForegroundColor;
        _decoderBox.FlatStyle = FlatStyle.Popup;
        _decoderBox.Margin = new Padding(4, 5, 10, 0);

        _aggressiveTailDropCheck.ForeColor = ForegroundColor;
        _aggressiveTailDropCheck.BackColor = WindowColor;
        _aggressiveTailDropCheck.Margin = new Padding(10, 7, 12, 0);

        _ultraLowLatencyCheck.ForeColor = ForegroundColor;
        _ultraLowLatencyCheck.BackColor = WindowColor;
        _ultraLowLatencyCheck.Margin = new Padding(10, 7, 12, 0);

        StyleButton(_startButton);
        StyleButton(_stopButton);
        StyleButton(_fullscreenButton);
        StyleButton(_prepareAdbButton);

        _hudBox.BackColor = Color.FromArgb(19, 20, 24);
        _hudBox.ForeColor = ForegroundColor;
    }

    private static void StyleButton(Button button)
    {
        button.AutoSize = false;
        button.Size = new Size(110, 30);
        button.FlatStyle = FlatStyle.Flat;
        button.FlatAppearance.BorderSize = 1;
        button.FlatAppearance.BorderColor = AccentColor;
        button.UseVisualStyleBackColor = false;
        button.BackColor = SurfaceAltColor;
        button.ForeColor = ForegroundColor;
        button.TextAlign = ContentAlignment.MiddleCenter;
        button.Margin = new Padding(6, 3, 0, 0);
    }

    private void BeginSessionAction(bool closeAfterCompletion)
    {
        _closeAfterSessionAction |= closeAfterCompletion;
        if (_sessionActionTask is { IsCompleted: false })
        {
            return;
        }

        _hudTimer.Stop();
        SetControlsEnabled(false);
        _statusLabel.Text = closeAfterCompletion ? "Closing..." : "Stopping...";

        _sessionActionTask = Task.Run(() =>
        {
            if (_closeAfterSessionAction)
            {
                _session.Dispose();
            }
            else
            {
                _session.Stop();
            }
        });

        _ = _sessionActionTask.ContinueWith(_ =>
        {
            if (IsDisposed)
            {
                return;
            }

            try
            {
                BeginInvoke(new Action(CompleteSessionAction));
            }
            catch (ObjectDisposedException)
            {
            }
            catch (InvalidOperationException)
            {
            }
        }, TaskScheduler.Default);
    }

    private void CompleteSessionAction()
    {
        if (IsDisposed)
        {
            return;
        }

        _sessionActionTask = null;

        if (_closeAfterSessionAction)
        {
            _allowClose = true;
            Close();
            return;
        }

        _closeAfterSessionAction = false;
        SetControlsEnabled(true);
        _startButton.Enabled = true;
        _stopButton.Enabled = false;
        _hudTimer.Start();
        RenderSnapshot(_session.GetSnapshot());
    }

    private void SetControlsEnabled(bool enabled)
    {
        _transportBox.Enabled = enabled;
        _portBox.Enabled = enabled;
        _backendBox.Enabled = enabled;
        _decoderBox.Enabled = enabled;
        _ultraLowLatencyCheck.Enabled = enabled;
        _aggressiveTailDropCheck.Enabled = enabled;
        _prepareAdbButton.Enabled = enabled;
        _fullscreenButton.Enabled = enabled;
        _startButton.Enabled = enabled;
        _stopButton.Enabled = enabled && !_startButton.Enabled;
    }

    private static string FormatDelta(int valueMs) => valueMs >= 0 ? $"{valueMs} ms" : "-";

    private static bool TryParseResolution(string resolution, out int width, out int height)
    {
        width = 0;
        height = 0;
        if (string.IsNullOrWhiteSpace(resolution))
        {
            return false;
        }

        var separatorIndex = resolution.IndexOf('x');
        if (separatorIndex <= 0 || separatorIndex >= resolution.Length - 1)
        {
            return false;
        }

        return int.TryParse(resolution[..separatorIndex], out width) &&
            int.TryParse(resolution[(separatorIndex + 1)..], out height) &&
            width > 0 &&
            height > 0;
    }
}
