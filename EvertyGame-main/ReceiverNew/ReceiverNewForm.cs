namespace ReceiverNative;

using System.Diagnostics;
using System.Globalization;

internal enum ReceiverNewHostState
{
    Ready,
    Starting,
    Streaming,
    Stopping,
}

internal sealed class ReceiverNewForm : Form
{
    private readonly ComboBox _targetBox = new()
    {
        Width = 320,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
    };

    private readonly Label _statusLabel = new()
    {
        AutoSize = true,
        Text = "Ready",
    };

    private readonly Label _hostCodeLabel = new()
    {
        AutoSize = true,
        Text = "Host code: -",
    };

    private readonly Label _sessionLabel = new()
    {
        AutoSize = true,
        Text = "Session: -",
    };

    private readonly Label _routeLabel = new()
    {
        AutoSize = true,
        Text = "Route: -",
    };

    private readonly Button _startButton = new()
    {
        Text = "Start",
        AutoSize = true,
    };

    private readonly Button _stopButton = new()
    {
        Text = "Stop",
        AutoSize = true,
        Enabled = false,
    };

    private readonly TextBox _baseUrlBox = new()
    {
        Width = 240,
        Text = Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_URL") ?? "http://46.45.217.19:5180",
    };

    private readonly TextBox _regionBox = new()
    {
        Width = 100,
        Text = "global",
    };

    private readonly TextBox _hudBox = new()
    {
        Multiline = true,
        ReadOnly = true,
        ScrollBars = ScrollBars.Vertical,
        Dock = DockStyle.Fill,
        Font = new Font("Consolas", 10f, FontStyle.Regular, GraphicsUnit.Point),
        BackColor = Color.FromArgb(24, 26, 31),
        ForeColor = Color.FromArgb(228, 233, 241),
    };

    private readonly System.Windows.Forms.Timer _uiTimer = new() { Interval = 500 };
    private readonly WindowsSenderSession _senderSession = new();
    private readonly ControlPlaneAgent _controlPlaneAgent = new();
    private readonly string _startedAtLabel = DateTime.Now.ToString("HH:mm:ss");
    private readonly string _buildLabel = File.GetLastWriteTime(typeof(ReceiverNewForm).Assembly.Location).ToString("yyyy-MM-dd HH:mm:ss");

    private ReceiverNewHostState _hostState = ReceiverNewHostState.Ready;
    private string? _activeSessionId;
    private string? _suppressedSessionId;
    private Task? _transitionTask;
    private string _lastHudText = string.Empty;

    public ReceiverNewForm()
    {
        Text = "ReceiverNew";
        MinimumSize = new Size(980, 640);
        BackColor = Color.FromArgb(10, 10, 12);

        _targetBox.DataSource = WindowsSenderSession.GetCaptureTargets().ToArray();
        _targetBox.Format += (_, args) =>
        {
            if (args.ListItem is WindowsCaptureTargetInfo target)
            {
                args.Value = target.UiLabel;
            }
        };

        BuildLayout();
        BindEvents();
        RefreshAgentConfiguration();
        Render();
        _uiTimer.Start();
    }

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        _uiTimer.Stop();
        _controlPlaneAgent.Dispose();
        _senderSession.Dispose();
        base.OnFormClosed(e);
    }

    private void BuildLayout()
    {
        var controls = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            WrapContents = true,
            Padding = new Padding(12),
            BackColor = Color.FromArgb(24, 26, 31),
        };

        controls.Controls.Add(new Label { AutoSize = true, Text = "Control plane", ForeColor = Color.White, Margin = new Padding(0, 8, 8, 0) });
        controls.Controls.Add(_baseUrlBox);
        controls.Controls.Add(new Label { AutoSize = true, Text = "Region", ForeColor = Color.White, Margin = new Padding(12, 8, 8, 0) });
        controls.Controls.Add(_regionBox);
        controls.Controls.Add(new Label { AutoSize = true, Text = "Monitor", ForeColor = Color.White, Margin = new Padding(12, 8, 8, 0) });
        controls.Controls.Add(_targetBox);
        controls.Controls.Add(_startButton);
        controls.Controls.Add(_stopButton);

        var statusPanel = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            WrapContents = true,
            Padding = new Padding(12, 8, 12, 8),
            BackColor = Color.FromArgb(18, 19, 23),
        };
        statusPanel.Controls.Add(_statusLabel);
        statusPanel.Controls.Add(new Label { AutoSize = true, Width = 18 });
        statusPanel.Controls.Add(_hostCodeLabel);
        statusPanel.Controls.Add(new Label { AutoSize = true, Width = 18 });
        statusPanel.Controls.Add(_sessionLabel);
        statusPanel.Controls.Add(new Label { AutoSize = true, Width = 18 });
        statusPanel.Controls.Add(_routeLabel);

        Controls.Add(_hudBox);
        Controls.Add(statusPanel);
        Controls.Add(controls);
    }

    private void BindEvents()
    {
        _uiTimer.Tick += (_, _) =>
        {
            RefreshAgentConfiguration();
            EvaluateRuntimeState();
            Render();
        };

        _startButton.Click += async (_, _) => await EnsureStreamingForCurrentLeaseAsync();
        _stopButton.Click += async (_, _) => await StopAndSuppressCurrentSessionAsync("manual_stop");
        _baseUrlBox.TextChanged += (_, _) => RefreshAgentConfiguration();
        _regionBox.TextChanged += (_, _) => RefreshAgentConfiguration();
        _controlPlaneAgent.SnapshotChanged += HandleAgentSnapshotChanged;
    }

    private void HandleAgentSnapshotChanged(ControlPlaneAgentSnapshot snapshot)
    {
        if (IsDisposed)
        {
            return;
        }

        if (InvokeRequired)
        {
            try
            {
                BeginInvoke((MethodInvoker)(() => HandleAgentSnapshotChanged(snapshot)));
            }
            catch
            {
            }
            return;
        }

        _hostCodeLabel.Text = $"Host code: {GetHostCode(snapshot.HostId)}";
        _sessionLabel.Text = $"Session: {snapshot.LeaseSessionId}";
        _routeLabel.Text = $"Route: {snapshot.LeaseRouteKind}";

        if (_transitionTask is { IsCompleted: false })
        {
            Render();
            return;
        }

        if (!IsLeaseReady(snapshot))
        {
            if (_senderSession.GetSnapshot().Sending)
            {
                ReceiverTrace.Log($"ReceiverNew lease not ready; stopping sender. active={_activeSessionId ?? "-"} lease={snapshot.LeaseSessionId} status={snapshot.LeaseStatus}.");
                _transitionTask = StopAndSuppressCurrentSessionAsync("lease_not_ready");
            }
            else
            {
                if (!string.IsNullOrWhiteSpace(_activeSessionId))
                {
                    ReceiverTrace.Log($"ReceiverNew lease cleared while idle; active={_activeSessionId} lease={snapshot.LeaseSessionId} status={snapshot.LeaseStatus}.");
                }
                _hostState = ReceiverNewHostState.Ready;
            }

            Render();
            return;
        }

        if (string.Equals(_suppressedSessionId, snapshot.LeaseSessionId, StringComparison.Ordinal))
        {
            ReceiverTrace.Log($"ReceiverNew lease suppressed; session={snapshot.LeaseSessionId}.");
            Render();
            return;
        }

        if (_senderSession.GetSnapshot().Sending)
        {
            if (string.Equals(_activeSessionId, snapshot.LeaseSessionId, StringComparison.Ordinal))
            {
                _hostState = ReceiverNewHostState.Streaming;
            }
            else
            {
                ReceiverTrace.Log($"ReceiverNew lease changed; restarting sender old={_activeSessionId ?? "-"} new={snapshot.LeaseSessionId}.");
                _transitionTask = RestartForSnapshotAsync(snapshot);
            }

            Render();
            return;
        }

        ReceiverTrace.Log($"ReceiverNew lease ready; starting sender for session={snapshot.LeaseSessionId} route={snapshot.LeaseRouteKind}.");
        _transitionTask = EnsureStreamingForSnapshotAsync(snapshot);
        Render();
    }

    private async Task EnsureStreamingForCurrentLeaseAsync()
    {
        var snapshot = _controlPlaneAgent.GetSnapshot();
        if (!IsLeaseReady(snapshot))
        {
            _statusLabel.Text = "Waiting for lease";
            Render();
            return;
        }

        await EnsureStreamingForSnapshotAsync(snapshot);
    }

    private async Task EnsureStreamingForSnapshotAsync(ControlPlaneAgentSnapshot snapshot)
    {
        if (!IsLeaseReady(snapshot))
        {
            _hostState = ReceiverNewHostState.Ready;
            Render();
            return;
        }

        try
        {
            _hostState = ReceiverNewHostState.Starting;
            ReceiverTrace.Log($"ReceiverNew starting session={snapshot.LeaseSessionId}.");
            Render();

            if (!TryParseEndpoint(snapshot.LeaseReceiverEndpoint, out var receiverHost, out var receiverPort))
            {
                throw new InvalidOperationException($"Invalid receiver endpoint: {snapshot.LeaseReceiverEndpoint}");
            }

            var relayRoute = TryParseEndpoint(snapshot.LeaseRelayEndpoint, out var relayHost, out var relayPort)
                ? new RelayTransportRoute(snapshot.LeaseSessionId, snapshot.LeaseSessionToken, relayHost, relayPort)
                : null;

            var target = _targetBox.SelectedItem as WindowsCaptureTargetInfo
                ?? throw new InvalidOperationException("No capture target selected.");

            var preset = BuildSenderSpec(snapshot);
            var codec = ResolveCodec(snapshot.LeaseCodecPreference);

            await Task.Run(() =>
                _senderSession.Start(
                    receiverHost,
                    receiverPort,
                    target.DeviceName,
                    WindowsSenderEncoderBackend.Auto,
                    codec,
                    preset,
                    audioEnabled: true,
                    captureCursorInStream: snapshot.LeaseCaptureCursor ?? false,
                    latencyPulseFlashEnabled: false,
                    adaptiveEnabled: snapshot.LeaseAdaptiveMode ?? false,
                    relayRoute: relayRoute));

            _activeSessionId = snapshot.LeaseSessionId;
            _suppressedSessionId = null;
            _hostState = ReceiverNewHostState.Streaming;
            ReceiverTrace.Log($"ReceiverNew sender started for {snapshot.LeaseSessionId}.");
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "ReceiverNew start failed");
            _hostState = ReceiverNewHostState.Ready;
            _statusLabel.Text = $"Start failed: {ex.Message}";
        }
        finally
        {
            _transitionTask = null;
            Render();
        }
    }

    private async Task RestartForSnapshotAsync(ControlPlaneAgentSnapshot snapshot)
    {
        try
        {
            ReceiverTrace.Log($"ReceiverNew restarting for session={snapshot.LeaseSessionId}.");
            await StopAndSuppressCurrentSessionAsync("session_replaced", clearTransitionTask: false);
            _suppressedSessionId = null;
            await EnsureStreamingForSnapshotAsync(snapshot);
        }
        finally
        {
            _transitionTask = null;
            Render();
        }
    }

    private async Task StopAndSuppressCurrentSessionAsync(string reason, bool clearTransitionTask = true)
    {
        try
        {
            _hostState = ReceiverNewHostState.Stopping;
            var sessionToSuppress = _activeSessionId ?? _controlPlaneAgent.GetSnapshot().LeaseSessionId;
            if (!string.IsNullOrWhiteSpace(sessionToSuppress) && sessionToSuppress != "-")
            {
                _suppressedSessionId = sessionToSuppress;
            }
            ReceiverTrace.Log($"ReceiverNew stopping sender; reason={reason}; active={_activeSessionId ?? "-"}; suppress={_suppressedSessionId ?? "-"}.");

            await Task.Run(() => _senderSession.Stop());
            _activeSessionId = null;
            _hostState = ReceiverNewHostState.Ready;
            ReceiverTrace.Log($"ReceiverNew sender stopped; reason={reason}; suppressed={_suppressedSessionId ?? "-"}.");
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "ReceiverNew stop failed");
            _hostState = ReceiverNewHostState.Ready;
        }
        finally
        {
            if (clearTransitionTask)
            {
                _transitionTask = null;
            }
            Render();
        }
    }

    private void RefreshAgentConfiguration()
    {
        var senderSnapshot = _senderSession.GetSnapshot();
        _controlPlaneAgent.ApplyConfiguration(
            new ControlPlaneAgentConfiguration(
                Enabled: !string.IsNullOrWhiteSpace(_baseUrlBox.Text),
                BaseUrl: _baseUrlBox.Text.Trim(),
                DisplayName: Environment.MachineName,
                Region: string.IsNullOrWhiteSpace(_regionBox.Text) ? "global" : _regionBox.Text.Trim(),
                DirectPort: 5001,
                SenderBusy: senderSnapshot.Sending,
                EncoderPath: senderSnapshot.EncoderPath,
                Codec: senderSnapshot.Codec,
                Resolution: senderSnapshot.Resolution,
                CaptureFps: senderSnapshot.CaptureFps,
                EncodeFps: senderSnapshot.EncodeFps,
                ReceiverDecodeFps: senderSnapshot.ReceiverDecodeFps,
                PulseEstimateMs: senderSnapshot.PulseToAndroidEstimateMs,
                InputEstimateMs: senderSnapshot.InputToAndroidEstimateMs,
                FramesDropped: senderSnapshot.FramesDropped,
                PacketsSent: senderSnapshot.PacketsSent,
                SupportsHevc: true,
                SupportsAudio: true,
                SupportsGamepad: true,
                EncoderBackends: Enum.GetValues<WindowsSenderEncoderBackend>()
                    .Where(static backend => backend != WindowsSenderEncoderBackend.Auto)
                    .Select(static backend => backend.ToString())
                    .ToArray(),
                Capabilities: BuildCapabilities()));
    }

    private void EvaluateRuntimeState()
    {
        if (_transitionTask is { IsCompleted: false })
        {
            return;
        }

        var senderSnapshot = _senderSession.GetSnapshot();
        if (!string.IsNullOrWhiteSpace(_activeSessionId) &&
            !senderSnapshot.Sending &&
            (senderSnapshot.Status.StartsWith("Sender error:", StringComparison.OrdinalIgnoreCase) ||
             _hostState is ReceiverNewHostState.Streaming or ReceiverNewHostState.Starting))
        {
            ReceiverTrace.Log($"ReceiverNew watchdog forcing reset; session={_activeSessionId}; status={senderSnapshot.Status}.");
            _transitionTask = StopAndSuppressCurrentSessionAsync("sender_dead");
        }
    }

    private void Render()
    {
        var agentSnapshot = _controlPlaneAgent.GetSnapshot();
        var senderSnapshot = _senderSession.GetSnapshot();

        _startButton.Enabled = _transitionTask is null or { IsCompleted: true };
        _stopButton.Enabled = senderSnapshot.Sending || _hostState is ReceiverNewHostState.Starting or ReceiverNewHostState.Stopping;

        _statusLabel.Text = _hostState switch
        {
            ReceiverNewHostState.Ready => "Ready",
            ReceiverNewHostState.Starting => "Starting session",
            ReceiverNewHostState.Streaming => "Streaming",
            ReceiverNewHostState.Stopping => "Stopping session",
            _ => "Ready",
        };

        var hud = string.Join(
            Environment.NewLine,
            new[]
            {
                "ReceiverNew",
                "Minimal sender test host",
                $"Started         : {_startedAtLabel}",
                $"Build           : {_buildLabel}",
                string.Empty,
                $"State           : {_hostState}",
                $"Host status     : {agentSnapshot.Status}",
                $"Host code       : {GetHostCode(agentSnapshot.HostId)}",
                $"Lease status    : {agentSnapshot.LeaseStatus}",
                $"Lease session   : {agentSnapshot.LeaseSessionId}",
                $"Lease receiver  : {agentSnapshot.LeaseReceiverEndpoint}",
                $"Lease relay     : {agentSnapshot.LeaseRelayEndpoint}",
                $"Receiver ready  : {(agentSnapshot.LeaseReceiverRegistered ? "yes" : "no")}",
                $"Host ready      : {(agentSnapshot.LeaseHostReady ? "yes" : "no")}",
                $"Suppressed      : {_suppressedSessionId ?? "-"}",
                string.Empty,
                $"Sender status   : {senderSnapshot.Status}",
                $"Remote endpoint : {senderSnapshot.RemoteEndpoint}",
                $"Codec           : {senderSnapshot.Codec}",
                $"Resolution      : {senderSnapshot.Resolution}",
                $"Packets sent    : {senderSnapshot.PacketsSent}",
                $"Video packets   : {senderSnapshot.VideoPackets}",
                $"Audio packets   : {senderSnapshot.AudioPackets}",
                $"Control tx/rx   : {senderSnapshot.ControlPacketsSent}/{senderSnapshot.ControlPacketsReceived}",
                $"Frames encoded  : {senderSnapshot.FramesEncoded}",
                $"Frames dropped  : {senderSnapshot.FramesDropped}",
                $"Receiver decode : {senderSnapshot.ReceiverDecodeFps}",
                $"Last control    : {senderSnapshot.LastControlKind}",
                $"Log file        : {ReceiverTrace.LogFilePath}",
            });

        if (!string.Equals(hud, _lastHudText, StringComparison.Ordinal))
        {
            _hudBox.Text = hud;
            _lastHudText = hud;
        }
    }

    private static bool IsLeaseReady(ControlPlaneAgentSnapshot snapshot) =>
        string.Equals(snapshot.LeaseStatus, "Active", StringComparison.OrdinalIgnoreCase) &&
        !string.IsNullOrWhiteSpace(snapshot.LeaseSessionId) &&
        snapshot.LeaseSessionId != "-" &&
        !string.IsNullOrWhiteSpace(snapshot.LeaseReceiverEndpoint) &&
        snapshot.LeaseReceiverEndpoint != "-" &&
        snapshot.LeaseReceiverRegistered &&
        snapshot.LeaseHostReady;

    private static bool TryParseEndpoint(string displayText, out string host, out int port)
    {
        host = string.Empty;
        port = 0;

        var endpointText = displayText.Trim();
        if (string.IsNullOrWhiteSpace(endpointText) || endpointText == "-")
        {
            return false;
        }

        var transportSeparator = endpointText.IndexOf(" (", StringComparison.Ordinal);
        if (transportSeparator >= 0)
        {
            endpointText = endpointText[..transportSeparator];
        }

        var colonIndex = endpointText.LastIndexOf(':');
        if (colonIndex <= 0 || colonIndex >= endpointText.Length - 1)
        {
            return false;
        }

        host = endpointText[..colonIndex];
        return int.TryParse(endpointText[(colonIndex + 1)..], NumberStyles.Integer, CultureInfo.InvariantCulture, out port) &&
            port is > 0 and <= 65535;
    }

    private static WindowsVideoCodec ResolveCodec(string codecPreference)
    {
        return WindowsVideoCodec.H264Avc;
    }

    private static WindowsSenderPresetSpec BuildSenderSpec(ControlPlaneAgentSnapshot snapshot)
    {
        var baseSpec = WindowsSenderPreset.Game.ToSpec();
        var width = snapshot.LeaseRequestedWidth > 0 ? snapshot.LeaseRequestedWidth : baseSpec.TargetWidth;
        var height = snapshot.LeaseRequestedHeight > 0 ? snapshot.LeaseRequestedHeight : baseSpec.TargetHeight;
        var fps = snapshot.LeaseRequestedFps > 0 ? snapshot.LeaseRequestedFps : baseSpec.TargetFps;
        var bitrate = snapshot.LeaseRequestedBitrateBps > 0 ? snapshot.LeaseRequestedBitrateBps : baseSpec.TargetBitrateBps;

        return new WindowsSenderPresetSpec(
            UiLabel: $"Lease {width}x{height} @ {fps} / {bitrate / 1_000_000.0:0.0} Mbps",
            ProtocolPreset: baseSpec.ProtocolPreset,
            TargetWidth: width,
            TargetHeight: height,
            TargetFps: fps,
            TargetBitrateBps: bitrate,
            KeyFrameIntervalSeconds: baseSpec.KeyFrameIntervalSeconds);
    }

    private static ControlPlaneHostCapabilities BuildCapabilities()
    {
        var maxWidth = 0;
        var maxHeight = 0;
        foreach (var screen in Screen.AllScreens)
        {
            maxWidth = Math.Max(maxWidth, screen.Bounds.Width);
            maxHeight = Math.Max(maxHeight, screen.Bounds.Height);
        }

        return new ControlPlaneHostCapabilities(
            CpuModel: Environment.GetEnvironmentVariable("PROCESSOR_IDENTIFIER"),
            GpuModel: null,
            RamGb: 0,
            MaxWidth: maxWidth,
            MaxHeight: maxHeight,
            MaxFps: 120);
    }

    private static string GetHostCode(string hostId)
    {
        const string prefix = "host_";
        if (string.IsNullOrWhiteSpace(hostId))
        {
            return "-";
        }

        var trimmed = hostId.Trim();
        if (trimmed.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
        {
            var body = trimmed[prefix.Length..];
            return body.Length <= 4 ? body : body[..4];
        }

        return trimmed.Length <= 4 ? trimmed : trimmed[..4];
    }
}
