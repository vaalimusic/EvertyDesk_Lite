namespace ReceiverNative;

using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;

internal sealed class MainForm : Form
{
    private const string DefaultControlPlaneUrl = "http://46.45.217.19:5180";
    private const string DemoAdminEmail = "admin";
    private const string DemoAdminPassword = "admin";
    private const string DemoTestEmail = "test";
    private const string DemoTestPassword = "test";

    private static readonly Color WindowColor = Color.FromArgb(10, 10, 12);
    private static readonly Color SurfaceColor = Color.FromArgb(24, 26, 31);
    private static readonly Color SurfaceAltColor = Color.FromArgb(31, 35, 42);
    private static readonly Color AccentColor = Color.FromArgb(72, 143, 255);
    private static readonly Color ForegroundColor = Color.FromArgb(228, 233, 241);
    private static readonly Color MutedForegroundColor = Color.FromArgb(216, 223, 233);

    private readonly Label _roleLabel = LabelFor("Role");
    private readonly Label _transportLabel = LabelFor("Transport");
    private readonly Label _portLabel = LabelFor("Port");
    private readonly Label _backendLabel = LabelFor("Playback");
    private readonly Label _decoderLabel = LabelFor("Decoder");
    private readonly Label _sendTargetLabel = LabelFor("Monitor");
    private readonly Label _sendHostLabel = LabelFor("Receiver");
    private readonly Label _sendPortLabel = LabelFor("Port");
    private readonly Label _controlPlaneLabel = LabelFor("Control");
    private readonly Label _controlRegionLabel = LabelFor("Region");
    private readonly Label _controlAuthLabel = LabelFor("Auth");
    private readonly Label _managedHostsLabel = LabelFor("Hosts");
    private readonly Label _sendPresetLabel = LabelFor("Preset");
    private readonly Label _sendEncoderLabel = LabelFor("Encoder");
    private readonly Label _sendCodecLabel = LabelFor("Codec");
    private readonly Label _sendWidthLabel = LabelFor("Width");
    private readonly Label _sendHeightLabel = LabelFor("Height");
    private readonly Label _sendFpsLabel = LabelFor("FPS");
    private readonly Label _sendBitrateLabel = LabelFor("Bitrate");
    private readonly TableLayoutPanel _commonSection = new() { AutoSize = true, AutoSizeMode = AutoSizeMode.GrowAndShrink, Dock = DockStyle.Top };
    private readonly TableLayoutPanel _clientQuickSection = new() { AutoSize = true, AutoSizeMode = AutoSizeMode.GrowAndShrink, Dock = DockStyle.Top };
    private readonly TableLayoutPanel _clientSettingsSection = new() { AutoSize = true, AutoSizeMode = AutoSizeMode.GrowAndShrink, Dock = DockStyle.Top };
    private readonly TableLayoutPanel _hostSection = new() { AutoSize = true, AutoSizeMode = AutoSizeMode.GrowAndShrink, Dock = DockStyle.Top };
    private readonly FlowLayoutPanel _controlsPanel = new();
    private readonly TableLayoutPanel _headerPanel = new();
    private readonly Label _brandTitleLabel = new()
    {
        AutoSize = true,
        Text = "Everty Studio",
    };
    private readonly Label _brandSubtitleLabel = new()
    {
        AutoSize = true,
        Text = "Low-latency display streaming for Windows, Android and LAN game sessions",
    };
    private readonly CheckBox _sendAudioCheck = new()
    {
        Text = "Audio",
        Checked = true,
        AutoSize = true,
    };
    private readonly CheckBox _sendCursorCheck = new()
    {
        Text = "Cursor",
        Checked = false,
        AutoSize = true,
    };
    private readonly CheckBox _sendPulseFlashCheck = new()
    {
        Text = "Pulse flash",
        Checked = false,
        AutoSize = true,
    };
    private readonly CheckBox _sendAdaptiveCheck = new()
    {
        Text = "Adaptive",
        Checked = false,
        AutoSize = true,
    };
    private readonly CheckBox _leaseAutoRunCheck = new()
    {
        Text = "Lease auto-run",
        Checked = true,
        AutoSize = true,
    };
    private readonly ComboBox _roleBox = new()
    {
        Width = 100,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<AppRole>(),
    };
    private readonly TextBox _portBox = new() { Text = "5001", Width = 72 };
    private readonly ComboBox _managedHostBox = new()
    {
        Width = 240,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
    };
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
    private readonly CheckBox _captureInputCheck = new()
    {
        Text = "Capture Input",
        AutoSize = true,
    };
    private readonly CheckBox _relativeMouseCheck = new()
    {
        Text = "Relative Mouse",
        AutoSize = true,
    };
    private readonly Label _inputStateLabel = new()
    {
        AutoSize = true,
        Text = "Input: disarmed",
    };
    private readonly Button _prepareAdbButton = new() { Text = "Prepare ADB", AutoSize = true };
    private readonly Button _startButton = new() { Text = "Start", AutoSize = true };
    private readonly Button _stopButton = new() { Text = "Stop", AutoSize = true, Enabled = false };
    private readonly ComboBox _sendTargetBox = new()
    {
        Width = 280,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
    };
    private readonly TextBox _sendHostBox = new() { Text = "192.168.0.67", Width = 140 };
    private readonly Button _sendDiscoverButton = new() { Text = "Find", AutoSize = true };
    private readonly TextBox _sendPortBox = new() { Text = "5001", Width = 72 };
    private readonly TextBox _controlPlaneUrlBox = new() { Width = 200, PlaceholderText = DefaultControlPlaneUrl };
    private readonly TextBox _controlRegionBox = new() { Width = 92, Text = "global" };
    private readonly TextBox _controlUserEmailBox = new() { Width = 170, PlaceholderText = "user@everty" };
    private readonly TextBox _controlUserPasswordBox = new() { Width = 120, PlaceholderText = "password", UseSystemPasswordChar = true };
    private readonly Button _controlDemoAdminButton = new() { Text = "Войти как admin", AutoSize = true };
    private readonly Button _controlDemoTestButton = new() { Text = "Войти как test", AutoSize = true };
    private readonly Button _controlUserLoginButton = new() { Text = "Войти", AutoSize = true };
    private readonly Button _controlUserRegisterButton = new() { Text = "Регистрация", AutoSize = true };
    private readonly CheckBox _advancedModeCheck = new() { Text = "Расширенный режим", Checked = false, AutoSize = true };
    private readonly Label _simpleModeHintLabel = new()
    {
        AutoSize = true,
        Text = "Простой режим: войди, дождись регистрации ПК или загрузи список компьютеров и подключись.",
    };
    private readonly Button _managedRefreshHostsButton = new() { Text = "Загрузить ПК", AutoSize = true };
    private readonly Button _managedResumeSessionButton = new() { Text = "Вернуть сессию", AutoSize = true };
    private readonly TextBox _managedHostCodeBox = new() { Width = 132, PlaceholderText = "short code" };
    private readonly Button _managedStartByCodeButton = new() { Text = "По коду", AutoSize = true };
    private readonly CheckBox _managedPreferHevcCheck = new()
    {
        Text = "Prefer HEVC",
        Checked = true,
        AutoSize = true,
    };
    private readonly CheckBox _managedPreferRelayCheck = new()
    {
        Text = "Prefer relay",
        Checked = true,
        AutoSize = true,
    };
    private readonly CheckBox _managedRequestAudioCheck = new()
    {
        Text = "Request audio",
        Checked = true,
        AutoSize = true,
    };
    private readonly Button _managedStartSessionButton = new() { Text = "Подключиться", AutoSize = true };
    private readonly Button _managedStopSessionButton = new() { Text = "Остановить", AutoSize = true, Enabled = false };
    private readonly Button _managedCloseSessionButton = new() { Text = "\u0417\u0430\u043a\u0440\u044b\u0442\u044c \u0441\u0435\u0441\u0441\u0438\u044e", AutoSize = true, Enabled = false };
    private readonly ComboBox _sendPresetBox = new()
    {
        Width = 220,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<WindowsSenderPreset>(),
    };
    private readonly ComboBox _sendEncoderBox = new()
    {
        Width = 170,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<WindowsSenderEncoderBackend>(),
    };
    private readonly ComboBox _sendCodecBox = new()
    {
        Width = 160,
        DropDownStyle = ComboBoxStyle.DropDownList,
        FormattingEnabled = true,
        DataSource = Enum.GetValues<WindowsVideoCodec>(),
    };
    private readonly TextBox _sendWidthBox = new() { Width = 72 };
    private readonly TextBox _sendHeightBox = new() { Width = 72 };
    private readonly TextBox _sendFpsBox = new() { Width = 64 };
    private readonly TextBox _sendBitrateBox = new() { Width = 72 };
    private readonly Button _startSendingButton = new() { Text = "Start Sending", AutoSize = true };
    private readonly Button _stopSendingButton = new() { Text = "Stop Sending", AutoSize = true, Enabled = false };
    private readonly Button _fullscreenButton = new() { Text = "Fullscreen", AutoSize = true };
    private readonly Button _tuningButton = new() { Text = "Tuning", AutoSize = true };
    private readonly Button _copyHudButton = new() { Text = "Copy HUD", AutoSize = true };
    private readonly Button _heroPrimaryActionButton = new() { Text = "Start", AutoSize = true };
    private readonly Button _diagnosticsToggleButton = new() { Text = "Diagnostics", AutoSize = true };
    private readonly Label _statusLabel = new()
    {
        AutoSize = false,
        Dock = DockStyle.Fill,
        Padding = new Padding(10, 6, 10, 6),
        Text = "Idle",
    };
    private readonly Label _heroDetailLabel = new()
    {
        AutoSize = true,
        Text = "Ready to host",
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
    private readonly Panel _diagnosticsBodyPanel = new()
    {
        Dock = DockStyle.Fill,
        BackColor = SurfaceColor,
    };
    private readonly Label _senderOverlayLabel = new()
    {
        Dock = DockStyle.Fill,
        TextAlign = ContentAlignment.MiddleCenter,
        Text = "Sender Mode\nStreaming selected monitor over LAN",
        Visible = false,
    };
    private readonly System.Windows.Forms.Timer _hudTimer = new() { Interval = 250 };
    private readonly NativeReceiverSession _session;
    private readonly WindowsSenderSession _senderSession = new();
    private readonly ControlPlaneAgent _controlPlaneAgent = new();
    private readonly DesktopControlPlaneClient _desktopControlPlaneClient = new();
    private readonly string _startedAtLabel = DateTime.Now.ToString("HH:mm:ss");
    private readonly string _buildLabel = File.GetLastWriteTime(typeof(MainForm).Assembly.Location).ToString("yyyy-MM-dd HH:mm:ss");
    private AppRole _currentRole = AppRole.Send;
    private bool _fullscreen;
    private Rectangle _restoreBounds = Rectangle.Empty;
    private FormBorderStyle _restoreBorderStyle = FormBorderStyle.Sizable;
    private FormWindowState _restoreWindowState = FormWindowState.Normal;
    private bool _senderAutoMinimized;
    private FormWindowState _senderRestoreWindowState = FormWindowState.Normal;
    private bool _diagnosticsDrawerExpanded = false;
    private string _lastHudText = string.Empty;
    private string? _pendingHudText;
    private long _lastHudRefreshAtMs;
    private long _lastTuningRefreshAtMs;
    private Task? _sessionActionTask;
    private Task? _leaseAutomationTask;
    private CancellationTokenSource? _leaseAutomationCts;
    private Task? _managedSessionSyncTask;
    private bool _closeAfterSessionAction;
    private bool _allowClose;
    private string _adbTunnelStatus = "-";
    private string _lastAutoFitResolution = "-";
    private bool? _lastAutoFitLandscape;
    private TuningOverlayForm? _tuningOverlay;
    private readonly SemaphoreSlim _sessionMutationGate = new(1, 1);
    private bool _suppressControlEvents;
    private string? _leaseDrivenSessionId;
    private string? _leaseSuppressedSessionId;
    private CancellationTokenSource? _controlPlaneRestartCts;
    private int _manualJitterMs;
    private int _manualAudioBufferMs;
    private int _manualPacingMinTenthsMs;
    private int _manualPacingMaxTenthsMs;
    private int _manualCatchUpMs;
    private int _manualIdrCooldownMs;
    private int _manualPanicQueueAu;
    private int _manualFeedbackTickMs;
    private int _manualHighDeltaMs;
    private int _manualCriticalDeltaMs;
    private int _manualStartupGraceMs;
    private int _manualDropBurstStep;
    private bool _inputCaptureArmed;
    private long _inputSequence;
    private bool _relativeMouseWarpPending;
    private readonly HashSet<Keys> _remoteKeysDown = new();
    private string _managedSessionId = string.Empty;
    private string _managedSessionToken = string.Empty;
    private string _managedHostId = string.Empty;
    private string _managedSessionHostLabel = "-";
    private string _managedRouteKind = "-";
    private string _managedRouteState = "-";
    private int _managedRouteVersion;
    private string _managedSessionHealth = "-";
    private string _managedSessionHealthReason = "-";
    private string _managedRouteActionHint = "-";
    private string _managedRouteActionReason = "-";
    private int _managedRouteFallbackReadyDurationSeconds;
    private int _managedRouteRecoveryReadyDurationSeconds;
    private int _managedRecommendedSyncDelaySeconds = 10;
    private string _managedTransportLossLevel = "-";
    private string _managedTransportAnomalyKind = "-";
    private string _managedTransportAnomalyReason = "-";
    private string _managedTransportAnomalyConfidence = "-";
    private int _managedReceiverTelemetryAgeSeconds = -1;
    private int _managedSenderTelemetryAgeSeconds = -1;
    private string _managedLastRouteActionKind = "-";
    private string _managedLastRouteActionReason = "-";
    private string _managedLastRouteActionActor = "-";
    private string _managedLastRouteActionUtc = "-";
    private int _managedRouteRecoveryCount;
    private int _managedRouteRecoveryCooldownSeconds;
    private int _managedRouteFallbackCount;
    private int _managedRouteFallbackCooldownSeconds;
    private string _managedRelayEndpoint = "-";
    private string _managedNatStatus = "-";
    private int _managedHostNatProbeAgeSeconds = -1;
    private int _managedClientNatProbeAgeSeconds = -1;
    private bool _managedNatProbeFresh;
    private long _lastManagedSessionSyncAtMs;
    private int _managedSessionSyncFailureCount;
    private int _managedSessionHealthDegradedStreak;
    private bool _managedRouteFallbackArmed = true;
    private int _managedSessionSyncDelayMs = 10_000;
    private string _friendlyStatusText = string.Empty;
    private DateTimeOffset _friendlyStatusExpiresUtc = DateTimeOffset.MinValue;
    private readonly MainFormLaunchOptions _launchOptions;
    private readonly bool _lockRoleSelection;
    private string _lastLeaseAutomationDecision = string.Empty;

    public MainForm(MainFormLaunchOptions? launchOptions = null)
    {
        SuspendLayout();
        _launchOptions = launchOptions ?? new MainFormLaunchOptions(AppRole.Send);
        _lockRoleSelection = _launchOptions.LockRoleSelection;
        Text = "Everty Native Receiver";
        MinimumSize = new Size(1440, 720);
        BackColor = WindowColor;
        KeyPreview = true;
        DoubleBuffered = true;

        _session = new NativeReceiverSession(_playbackHost);

        ApplyTheme();
        BuildLayout();
        BindEvents();

        _hudTimer.Tick += (_, _) => RenderCurrentSnapshot();
        _hudTimer.Start();

        _transportBox.SelectedItem = ReceiverTransportMode.Udp;
        _backendBox.SelectedItem = PlaybackBackendKind.MediaFoundationDirect3D11;
        _decoderBox.SelectedItem = HardwareDecodeMode.Auto;
        ForceDefaultSenderSettings();
        var controlPlaneUrlFromEnvironment = Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_URL");
        _controlPlaneUrlBox.Text = !string.IsNullOrWhiteSpace(_launchOptions.ControlPlaneUrl)
            ? _launchOptions.ControlPlaneUrl!.Trim()
            : string.IsNullOrWhiteSpace(controlPlaneUrlFromEnvironment)
                ? DefaultControlPlaneUrl
                : controlPlaneUrlFromEnvironment.Trim();
        _controlUserEmailBox.Text = Environment.GetEnvironmentVariable("EVERTY_CONTROL_USER_EMAIL") ?? string.Empty;
        _controlUserPasswordBox.Text = Environment.GetEnvironmentVariable("EVERTY_CONTROL_USER_PASSWORD") ?? string.Empty;
        _sendTargetBox.DataSource = WindowsSenderSession.GetCaptureTargets().ToArray();
        if (_sendTargetBox.Items.Count > 0)
        {
            _sendTargetBox.SelectedIndex = 0;
        }

        ApplyLaunchOptions();
        RenderCurrentSnapshot();
        ResumeLayout(performLayout: true);
    }

    protected override void OnFormClosing(FormClosingEventArgs e)
    {
        base.OnFormClosing(e);
    }

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        ReceiverTrace.Log("Main form closed");
        _controlPlaneRestartCts?.Cancel();
        _controlPlaneRestartCts?.Dispose();
        _tuningOverlay?.Dispose();
        _desktopControlPlaneClient.Dispose();
        _controlPlaneAgent.Dispose();
        _senderSession.Dispose();
        base.OnFormClosed(e);
    }

    protected override bool ProcessCmdKey(ref Message msg, Keys keyData)
    {
        if (_currentRole == AppRole.Receive && _inputCaptureArmed)
        {
            var keyCode = keyData & Keys.KeyCode;
            if (keyCode == Keys.Escape)
            {
                SetRemoteInputCaptureArmed(false, sendReleaseAll: true);
                return true;
            }

            if (!_remoteKeysDown.Contains(keyCode))
            {
                _remoteKeysDown.Add(keyCode);
                _session.SendRemoteKey(NextInputSequence(), (int)keyCode, pressed: true);
            }

            return true;
        }

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
        _playbackHost.Controls.Add(_senderOverlayLabel);

        var root = new TableLayoutPanel
        {
            Dock = DockStyle.Fill,
            BackColor = WindowColor,
            ColumnCount = 1,
            RowCount = 2,
        };
        root.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        root.RowStyles.Add(new RowStyle(SizeType.Percent, 100f));

        var brandPanel = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            ColumnCount = 1,
            RowCount = 2,
            Padding = new Padding(14, 14, 14, 8),
            BackColor = SurfaceColor,
            Margin = new Padding(10, 10, 10, 0),
        };
        brandPanel.Controls.Add(_brandTitleLabel, 0, 0);
        brandPanel.Controls.Add(_brandSubtitleLabel, 0, 1);

        var heroPanel = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            ColumnCount = 2,
            RowCount = 1,
            Padding = new Padding(14, 12, 14, 12),
            BackColor = SurfaceColor,
            Margin = new Padding(10, 10, 10, 0),
        };
        heroPanel.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100f));
        heroPanel.ColumnStyles.Add(new ColumnStyle(SizeType.AutoSize));

        var heroText = new FlowLayoutPanel
        {
            Dock = DockStyle.Fill,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            FlowDirection = FlowDirection.TopDown,
            WrapContents = false,
            Margin = Padding.Empty,
            Padding = Padding.Empty,
            BackColor = Color.Transparent,
        };
        _statusLabel.Font = new Font("Segoe UI Semibold", 18f, FontStyle.Regular, GraphicsUnit.Point);
        _statusLabel.Margin = new Padding(0, 0, 0, 4);
        _heroDetailLabel.ForeColor = MutedForegroundColor;
        _heroDetailLabel.Font = new Font("Segoe UI", 10.5f, FontStyle.Regular, GraphicsUnit.Point);
        _heroDetailLabel.Margin = new Padding(2, 0, 0, 0);
        heroText.Controls.Add(_statusLabel);
        heroText.Controls.Add(_heroDetailLabel);

        var heroActions = new FlowLayoutPanel
        {
            Dock = DockStyle.Fill,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            FlowDirection = FlowDirection.LeftToRight,
            WrapContents = false,
            Margin = Padding.Empty,
            Padding = Padding.Empty,
            BackColor = Color.Transparent,
        };
        heroActions.Controls.Add(_heroPrimaryActionButton);
        heroActions.Controls.Add(_diagnosticsToggleButton);

        heroPanel.Controls.Add(heroText, 0, 0);
        heroPanel.Controls.Add(heroActions, 1, 0);

        ConfigureSection(
            _commonSection,
            "Connection",
            _roleLabel,
            _roleBox,
            _controlPlaneLabel,
            _controlPlaneUrlBox,
            _simpleModeHintLabel,
            _controlAuthLabel,
            _controlDemoAdminButton,
            _controlDemoTestButton,
            _controlUserEmailBox,
            _controlUserPasswordBox,
            _controlUserLoginButton,
            _controlUserRegisterButton,
            _controlRegionLabel,
            _controlRegionBox,
            _advancedModeCheck);

        ConfigureSection(
            _clientQuickSection,
            "Connect",
            _managedHostsLabel,
            _managedHostCodeBox,
            _managedStartByCodeButton,
            _managedHostBox,
            _managedRefreshHostsButton,
            _managedResumeSessionButton,
            _managedStartSessionButton,
            _managedStopSessionButton,
            _managedCloseSessionButton);

        ConfigureSection(
            _clientSettingsSection,
            "Client Settings",
            _managedPreferHevcCheck,
            _managedPreferRelayCheck,
            _managedRequestAudioCheck,
            _transportLabel,
            _transportBox,
            _portLabel,
            _portBox,
            _backendLabel,
            _backendBox,
            _decoderLabel,
            _decoderBox,
            _ultraLowLatencyCheck,
            _aggressiveTailDropCheck,
            _captureInputCheck,
            _relativeMouseCheck,
            _inputStateLabel,
            _prepareAdbButton,
            _startButton,
            _stopButton,
            _tuningButton,
            _fullscreenButton,
            _copyHudButton);

        ConfigureSection(
            _hostSection,
            "Host",
            _sendTargetLabel,
            _sendTargetBox,
            _sendHostLabel,
            _sendHostBox,
            _sendDiscoverButton,
            _sendPortLabel,
            _sendPortBox,
            _sendPresetLabel,
            _sendPresetBox,
            _sendEncoderLabel,
            _sendEncoderBox,
            _sendCodecLabel,
            _sendCodecBox,
            _sendWidthLabel,
            _sendWidthBox,
            _sendHeightLabel,
            _sendHeightBox,
            _sendFpsLabel,
            _sendFpsBox,
            _sendBitrateLabel,
            _sendBitrateBox,
            _sendAudioCheck,
            _sendCursorCheck,
            _sendPulseFlashCheck,
            _sendAdaptiveCheck,
            _leaseAutoRunCheck,
            _startSendingButton,
            _stopSendingButton,
            _copyHudButton,
            _fullscreenButton);

        var diagnosticsDrawer = new TableLayoutPanel
        {
            Dock = DockStyle.Fill,
            AutoSize = false,
            ColumnCount = 1,
            RowCount = 2,
            Padding = new Padding(0, 0, 10, 0),
            Margin = Padding.Empty,
            BackColor = WindowColor,
        };
        diagnosticsDrawer.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        diagnosticsDrawer.RowStyles.Add(new RowStyle(SizeType.Percent, 100f));

        var diagnosticsHeader = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            FlowDirection = FlowDirection.LeftToRight,
            WrapContents = false,
            BackColor = SurfaceColor,
            Margin = new Padding(0, 0, 0, 8),
            Padding = new Padding(12, 10, 12, 10),
        };
        diagnosticsHeader.Controls.Add(new Label
        {
            AutoSize = true,
            Text = "Diagnostics",
            ForeColor = ForegroundColor,
            Font = new Font("Segoe UI Semibold", 10.5f, FontStyle.Regular, GraphicsUnit.Point),
            Margin = new Padding(0, 4, 12, 0),
        });
        _diagnosticsToggleButton.Size = new Size(106, 30);
        diagnosticsHeader.Controls.Add(_diagnosticsToggleButton);

        _diagnosticsBodyPanel.Padding = new Padding(10);
        _diagnosticsBodyPanel.Margin = Padding.Empty;
        _diagnosticsBodyPanel.Controls.Clear();
        _diagnosticsBodyPanel.Visible = false;
        _hudBox.Visible = false;
        _diagnosticsBodyPanel.Controls.Add(_hudBox);

        diagnosticsDrawer.Controls.Add(diagnosticsHeader, 0, 0);
        diagnosticsDrawer.Controls.Add(_diagnosticsBodyPanel, 0, 1);

        var controls = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = false,
            AutoScroll = true,
            Height = 168,
            WrapContents = false,
            Padding = new Padding(10, 10, 10, 0),
            FlowDirection = FlowDirection.TopDown,
            BackColor = WindowColor,
        };
        controls.Controls.AddRange(
            new Control[]
            {
                _commonSection,
                _clientQuickSection,
                _clientSettingsSection,
                _hostSection,
            });
        WireScrollHostHandlers(controls, controls);

        var header = new TableLayoutPanel
        {
            Dock = DockStyle.Top,
            BackColor = WindowColor,
            ColumnCount = 1,
            RowCount = 3,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            Margin = Padding.Empty,
            Padding = Padding.Empty,
        };
        header.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        header.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        header.RowStyles.Add(new RowStyle(SizeType.Absolute, 168f));
        header.Controls.Add(brandPanel, 0, 0);
        header.Controls.Add(heroPanel, 0, 1);
        header.Controls.Add(controls, 0, 2);

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
        content.Controls.Add(diagnosticsDrawer, 1, 0);

        root.Controls.Add(header, 0, 0);
        root.Controls.Add(content, 0, 1);
        Controls.Add(root);
    }

    private void BindEvents()
    {
        _controlPlaneAgent.SnapshotChanged += HandleControlPlaneAgentSnapshotChanged;
        _startButton.Click += async (_, _) => await StartSessionAsync();
        _stopButton.Click += (_, _) => StopSession();
        _startSendingButton.Click += async (_, _) => await StartSendingAsync();
        _stopSendingButton.Click += async (_, _) => await StopSendingAsync();
        _prepareAdbButton.Click += async (_, _) =>
        {
            var transport = _transportBox.SelectedItem is ReceiverTransportMode selectedTransport
                ? selectedTransport
                : ReceiverTransportMode.Udp;
            if (transport == ReceiverTransportMode.AdbShellH264)
            {
                await PrepareAdbShellCaptureAsync(showDialogOnFailure: true);
                return;
            }

            await PrepareAdbTunnelAsync(showDialogOnFailure: true);
        };
        _sendDiscoverButton.Click += async (_, _) => await DiscoverReceiversAsync();
        _managedRefreshHostsButton.Click += async (_, _) => await LoadManagedHostsAsync();
        _managedResumeSessionButton.Click += async (_, _) => await ResumeManagedReceiverSessionAsync();
        _managedStartByCodeButton.Click += async (_, _) => await StartManagedReceiverSessionByCodeAsync();
        _controlDemoAdminButton.Click += async (_, _) => await LoginWithDemoUserAsync(DemoAdminEmail, DemoAdminPassword);
        _controlDemoTestButton.Click += async (_, _) => await LoginWithDemoUserAsync(DemoTestEmail, DemoTestPassword);
        _controlUserLoginButton.Click += async (_, _) => await AuthenticateControlPlaneUserAsync(register: false);
        _controlUserRegisterButton.Click += async (_, _) => await AuthenticateControlPlaneUserAsync(register: true);
        _managedStartSessionButton.Click += async (_, _) => await StartManagedReceiverSessionAsync();
        _managedStopSessionButton.Click += async (_, _) => await StopManagedReceiverSessionAsync("desktop_receiver_stop", stopLocalReceiver: true);
        _managedCloseSessionButton.Click += async (_, _) => await StopManagedReceiverSessionAsync("desktop_receiver_close", stopLocalReceiver: false);
        _controlPlaneUrlBox.TextChanged += (_, _) => RefreshControlPlaneAgentConfiguration();
        _controlRegionBox.TextChanged += (_, _) => RefreshControlPlaneAgentConfiguration();
        _advancedModeCheck.CheckedChanged += (_, _) => UpdateRoleUi();
        _fullscreenButton.Click += (_, _) => ToggleFullscreen();
        _tuningButton.Click += (_, _) => ToggleTuningOverlay();
        _copyHudButton.Click += (_, _) => CopyHudToClipboard();
        _heroPrimaryActionButton.Click += async (_, _) => await RunHeroPrimaryActionAsync();
        _diagnosticsToggleButton.Click += (_, _) => ToggleDiagnosticsDrawer();
        _roleBox.Format += (_, args) =>
        {
            if (args.ListItem is AppRole role)
            {
                args.Value = role.ToUiLabel();
            }
        };
        _roleBox.SelectedIndexChanged += async (_, _) =>
        {
            if (_suppressControlEvents || _roleBox.SelectedItem is not AppRole role)
            {
                return;
            }

            await SwitchRoleAsync(role);
        };
        _hudBox.Enter += (_, _) => ReceiverTrace.Log("HUD focus entered; live HUD updates deferred");
        _hudBox.Leave += (_, _) =>
        {
            ReceiverTrace.Log("HUD focus left; applying deferred HUD update if present");
            FlushDeferredHudText();
        };
        _hudBox.KeyDown += (_, args) =>
        {
            if (args.Control && args.KeyCode == Keys.A)
            {
                _hudBox.SelectAll();
                args.SuppressKeyPress = true;
            }
        };
        _transportBox.Format += (_, args) =>
        {
            if (args.ListItem is ReceiverTransportMode mode)
            {
                args.Value = mode.ToUiLabel();
            }
        };
        _sendPresetBox.Format += (_, args) =>
        {
            if (args.ListItem is WindowsSenderPreset preset)
            {
                args.Value = preset.ToUiLabel();
            }
        };
        _sendEncoderBox.Format += (_, args) =>
        {
            if (args.ListItem is WindowsSenderEncoderBackend backend)
            {
                args.Value = backend.ToUiLabel();
            }
        };
        _sendCodecBox.Format += (_, args) =>
        {
            if (args.ListItem is WindowsVideoCodec codec)
            {
                args.Value = codec.ToUiLabel();
            }
        };
        _sendPresetBox.SelectedIndexChanged += (_, _) =>
        {
            if (_suppressControlEvents || _sendPresetBox.SelectedItem is not WindowsSenderPreset preset)
            {
                return;
            }

            ApplySenderPresetTemplate(preset);
        };
        _sendTargetBox.Format += (_, args) =>
        {
            if (args.ListItem is WindowsCaptureTargetInfo target)
            {
                args.Value = target.UiLabel;
            }
        };
        _managedHostBox.Format += (_, args) =>
        {
            if (args.ListItem is DesktopControlPlaneHostSummary host)
            {
                args.Value = BuildManagedHostUiLabel(host);
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
                : transport == ReceiverTransportMode.AdbShellH264
                    ? "Run Prepare ADB or Start to validate adb exec-out"
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
            if (_suppressControlEvents)
            {
                return;
            }
            if (_backendBox.SelectedItem is PlaybackBackendKind backend)
            {
                _ = RunSessionMutationAsync("Switching backend...", () => _session.UpdatePlaybackBackend(backend));
            }
        };
        _decoderBox.SelectedIndexChanged += (_, _) =>
        {
            if (_suppressControlEvents)
            {
                return;
            }
            if (_decoderBox.SelectedItem is HardwareDecodeMode mode)
            {
                _ = RunSessionMutationAsync("Switching decoder...", () => _session.UpdateHardwareDecodeMode(mode));
            }
        };
        _aggressiveTailDropCheck.CheckedChanged += (_, _) =>
        {
            if (_suppressControlEvents)
            {
                return;
            }
            _ = RunLightweightSessionMutationAsync("Updating tail-drop", () => _session.UpdateAggressiveMode(_aggressiveTailDropCheck.Checked));
        };
        _ultraLowLatencyCheck.CheckedChanged += (_, _) =>
        {
            if (_suppressControlEvents)
            {
                return;
            }
            _ = RunLightweightSessionMutationAsync("Updating latency mode", () => _session.UpdateUltraLowLatencyMode(_ultraLowLatencyCheck.Checked));
        };
        _captureInputCheck.CheckedChanged += (_, _) =>
        {
            if (_suppressControlEvents)
            {
                return;
            }

            SetRemoteInputCaptureArmed(_captureInputCheck.Checked, sendReleaseAll: true);
        };
        _relativeMouseCheck.CheckedChanged += (_, _) =>
        {
            if (_suppressControlEvents || !_inputCaptureArmed)
            {
                UpdateInputCaptureStateLabel();
                return;
            }

            if (_relativeMouseCheck.Checked)
            {
                Cursor.Hide();
                CenterRelativeMouseCursor();
            }
            else
            {
                Cursor.Show();
            }

            UpdateInputCaptureStateLabel();
        };

        Deactivate += (_, _) => SetRemoteInputCaptureArmed(false, sendReleaseAll: true);
        KeyUp += (_, args) => HandleRemoteKeyUp(args.KeyCode);

        WirePlaybackInputHandlers(_playbackHost);
        _playbackHost.ControlAdded += (_, args) =>
        {
            if (args.Control is not null)
            {
                WirePlaybackInputHandlers(args.Control);
            }
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
        else if (transport == ReceiverTransportMode.AdbShellH264)
        {
            var prepared = await PrepareAdbShellCaptureAsync(showDialogOnFailure: true);
            if (!prepared)
            {
                return;
            }
        }

        if (_currentRole == AppRole.Receive &&
            transport == ReceiverTransportMode.Udp &&
            !_session.GetSnapshot().Listening &&
            !IsUdpPortAvailable(port))
        {
            var resolvedPort = FindAvailableUdpPort(port + 1, 32);
            if (resolvedPort > 0)
            {
                port = resolvedPort;
                _portBox.Text = port.ToString(CultureInfo.InvariantCulture);
                ShowFriendlyStatus($"Порт занят. Переключаю listener на {port}.", sticky: true);
            }
        }

        try
        {
            ReceiverTrace.Log($"Start requested: transport={transport}, port={port}, backend={backend}, decoder={mode}, ultraLow={_ultraLowLatencyCheck.Checked}, tailDrop={_aggressiveTailDropCheck.Checked}");
            await RunSessionMutationAsync("Starting receiver...", () =>
            {
                ReceiverTrace.Log("Start sequence: Session.Start begin");
                _session.ConfigureRelayRoute(null);
                _session.ConfigureRelayRegistrationRoute(null);
                _session.Start(port, transport, mode, _aggressiveTailDropCheck.Checked);
                ReceiverTrace.Log("Start sequence: Session.Start end");
            });
        }
        catch (Exception ex)
        {
            MessageBox.Show(this, ex.Message, "Failed to start receiver", MessageBoxButtons.OK, MessageBoxIcon.Error);
            RenderSnapshot(_session.GetSnapshot());
        }
    }

    private async Task StartSendingAsync()
    {
        if (string.IsNullOrWhiteSpace(_sendHostBox.Text))
        {
            MessageBox.Show(this, "Enter a valid receiver host/IP", "Invalid receiver", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        if (!int.TryParse(_sendPortBox.Text.Trim(), out var port) || port is < 1 or > 65535)
        {
            MessageBox.Show(this, "Enter a valid receiver port", "Invalid port", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        var target = _sendTargetBox.SelectedItem as WindowsCaptureTargetInfo;
        if (target is null)
        {
            MessageBox.Show(this, "Select a monitor to capture", "No monitor selected", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        var preset = _sendPresetBox.SelectedItem is WindowsSenderPreset selectedPreset
            ? selectedPreset
            : WindowsSenderPreset.Game;
        var encoderBackend = _sendEncoderBox.SelectedItem is WindowsSenderEncoderBackend selectedBackend
            ? selectedBackend
            : WindowsSenderEncoderBackend.Auto;
        var codec = _sendCodecBox.SelectedItem is WindowsVideoCodec selectedCodec
            ? selectedCodec
            : WindowsVideoCodec.H265Hevc;
        var senderSpec = BuildSenderSpecFromUi(preset);

        try
        {
            await StartSenderCoreAsync(
                host: _sendHostBox.Text.Trim(),
                port: port,
                encoderBackend: encoderBackend,
                codec: codec,
                senderSpec: senderSpec,
                audioEnabled: _sendAudioCheck.Checked,
                captureCursorInStream: _sendCursorCheck.Checked,
                adaptiveEnabled: _sendAdaptiveCheck.Checked,
                relayRoute: null,
                busyStatus: "Starting sender...");
            AutoHideSenderWindow();
        }
        catch (Exception ex)
        {
            MessageBox.Show(this, ex.Message, "Failed to start sender", MessageBoxButtons.OK, MessageBoxIcon.Error);
            RenderCurrentSnapshot();
        }
    }

    private async Task StopSendingAsync()
    {
        try
        {
            var stoppedLeaseSession = _leaseDrivenSessionId;
            _leaseSuppressedSessionId = stoppedLeaseSession;
            _leaseDrivenSessionId = null;
            await RunSessionMutationAsync("Stopping sender...", () => _senderSession.Stop());
            RestoreSenderWindowIfNeeded();
            if (!string.IsNullOrWhiteSpace(stoppedLeaseSession))
            {
                TraceLeaseAutomationDecision($"stop: sender manually stopped; suppressing lease session={stoppedLeaseSession} until a new session appears");
            }
        }
        catch (Exception ex)
        {
            MessageBox.Show(this, ex.Message, "Failed to stop sender", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private async Task StartSenderCoreAsync(
        string host,
        int port,
        WindowsSenderEncoderBackend encoderBackend,
        WindowsVideoCodec codec,
        WindowsSenderPresetSpec senderSpec,
        bool audioEnabled,
        bool captureCursorInStream,
        bool adaptiveEnabled,
        RelayTransportRoute? relayRoute,
        string busyStatus)
    {
        var target = _sendTargetBox.SelectedItem as WindowsCaptureTargetInfo;
        if (target is null)
        {
            throw new InvalidOperationException("Select a monitor to capture.");
        }

        await RunSessionMutationAsync(
            busyStatus,
            () => _senderSession.Start(
                host,
                port,
                target.DeviceName,
                encoderBackend,
                codec,
                senderSpec,
                audioEnabled,
                captureCursorInStream,
                _sendPulseFlashCheck.Checked,
                adaptiveEnabled,
                relayRoute));
    }

    private async Task DiscoverReceiversAsync()
    {
        if (!int.TryParse(_sendPortBox.Text.Trim(), out var port) || port is < 1 or > 65535)
        {
            MessageBox.Show(this, "Enter a valid receiver port before discovery", "Invalid port", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        _statusLabel.Text = "Discovering Everty receivers on LAN...";
        _sendDiscoverButton.Enabled = false;

        try
        {
            var found = await Task.Run(() => ProbeLanForReceiverAsync(port));
            if (found is null)
            {
                _statusLabel.Text = $"No receiver found on UDP {port}";
                return;
            }

            var receiver = found.Value;
            _sendHostBox.Text = receiver.Address.ToString();
            _sendPortBox.Text = receiver.Port.ToString(CultureInfo.InvariantCulture);
            _statusLabel.Text = $"Receiver found: {receiver.DeviceName} at {receiver.Address}:{receiver.Port}";
        }
        catch (Exception ex)
        {
            _statusLabel.Text = $"Discovery failed: {ex.Message}";
        }
        finally
        {
            _sendDiscoverButton.Enabled = true;
        }
    }

    private async Task LoadManagedHostsAsync()
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl))
        {
            ShowFriendlyStatus("Укажи адрес control-plane, чтобы загрузить список игровых ПК.", sticky: true);
            return;
        }

        var previousHostId = (_managedHostBox.SelectedItem as DesktopControlPlaneHostSummary)?.HostId;
        _managedRefreshHostsButton.Enabled = false;
        try
        {
            var hosts = (await _desktopControlPlaneClient.ListHostsAsync(baseUrl))
                .Where(host => host.Online)
                .OrderBy(host => !string.IsNullOrWhiteSpace(host.ActiveSessionId))
                .ThenBy(host => host.DisplayName, StringComparer.OrdinalIgnoreCase)
                .ToList();
            _managedHostBox.DataSource = null;
            _managedHostBox.DataSource = hosts;

            if (!string.IsNullOrWhiteSpace(previousHostId))
            {
                var previousIndex = hosts.FindIndex(host => string.Equals(host.HostId, previousHostId, StringComparison.Ordinal));
                if (previousIndex >= 0)
                {
                    _managedHostBox.SelectedIndex = previousIndex;
                }
            }

            if (_managedHostBox.SelectedIndex < 0 && hosts.Count > 0)
            {
                _managedHostBox.SelectedIndex = 0;
            }

            ShowFriendlyStatus(
                hosts.Count == 0
                    ? "Логин выполнен, но онлайн-хостов пока нет."
                    : $"Хосты загружены: {hosts.Count}. Выбери ПК и нажми «Подключиться».",
                sticky: true);
        }
        catch (Exception ex)
        {
            ShowFriendlyStatus($"Не удалось загрузить хосты: {ex.Message}", sticky: true);
        }
        finally
        {
            RefreshManagedClientUiState();
        }
    }

    private async Task AuthenticateControlPlaneUserAsync(bool register)
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl))
        {
            MessageBox.Show(this, "Enter a control plane URL first.", "Control plane required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            ShowFriendlyStatus("Сначала укажи адрес control-plane.", sticky: true);
            return;
        }

        var email = _controlUserEmailBox.Text.Trim();
        var password = _controlUserPasswordBox.Text;
        if (string.IsNullOrWhiteSpace(email) || string.IsNullOrWhiteSpace(password))
        {
            MessageBox.Show(this, "Enter user email and password first.", "Credentials required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            ShowFriendlyStatus("Сначала введи email и пароль.", sticky: true);
            return;
        }

        try
        {
            var authState = register
                ? await _desktopControlPlaneClient.RegisterUserAsync(baseUrl, email, password)
                : await _desktopControlPlaneClient.LoginUserAsync(baseUrl, email, password);
            if (IsDisposed || Disposing)
            {
                return;
            }
            ShowFriendlyStatus(
                authState.UserAuthenticated
                    ? $"Вход выполнен: {authState.Label}. Сейчас загружу список хостов."
                    : "Авторизация control-plane обновлена.",
                sticky: true);
            if (_currentRole == AppRole.Receive)
            {
                await LoadManagedHostsAsync();
            }
        }
        catch (Exception ex)
        {
            if (IsDisposed || Disposing)
            {
                return;
            }
            MessageBox.Show(this, ex.Message, register ? "Register failed" : "Login failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
            ShowFriendlyStatus($"Ошибка входа: {ex.Message}", sticky: true);
        }
    }

    private async Task LoginWithDemoUserAsync(string email, string password)
    {
        _controlUserEmailBox.Text = email;
        _controlUserPasswordBox.Text = password;
        await AuthenticateControlPlaneUserAsync(register: false);
    }

    private async Task StartManagedReceiverSessionAsync()
    {
        if (_managedHostBox.SelectedItem is not DesktopControlPlaneHostSummary selectedHost)
        {
            MessageBox.Show(this, "Load and select a host first.", "Host required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        await StartManagedReceiverSessionAsync(selectedHost, allowActorSessionReplacement: true, allowLocalRetry: true);
    }

    private async Task StartManagedReceiverSessionByCodeAsync()
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl))
        {
            MessageBox.Show(this, "Enter a control plane URL first.", "Control plane required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        var requestedCode = NormalizeHostCode(_managedHostCodeBox.Text);
        if (string.IsNullOrWhiteSpace(requestedCode))
        {
            MessageBox.Show(this, "Enter a short host code first.", "Host code required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        var hosts = (await _desktopControlPlaneClient.ListHostsAsync(baseUrl))
            .Where(host => host.Online)
            .OrderBy(host => !string.IsNullOrWhiteSpace(host.ActiveSessionId))
            .ThenBy(host => host.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ToList();
        _managedHostBox.DataSource = null;
        _managedHostBox.DataSource = hosts;

        var selectedHost = hosts.FirstOrDefault(host =>
            string.Equals(NormalizeHostCode(host.HostCode), requestedCode, StringComparison.OrdinalIgnoreCase) ||
            string.Equals(GetHostCode(host.HostId), requestedCode, StringComparison.OrdinalIgnoreCase));
        if (selectedHost is null)
        {
            ShowFriendlyStatus($"Хост с кодом {requestedCode} не найден.", sticky: true);
            return;
        }

        var selectedIndex = hosts.FindIndex(host => string.Equals(host.HostId, selectedHost.HostId, StringComparison.Ordinal));
        if (selectedIndex >= 0)
        {
            _managedHostBox.SelectedIndex = selectedIndex;
        }

        await StartManagedReceiverSessionAsync(selectedHost, allowActorSessionReplacement: true, allowLocalRetry: true);
    }

    private async Task StartManagedReceiverSessionAsync(bool allowActorSessionReplacement)
    {
        if (_managedHostBox.SelectedItem is not DesktopControlPlaneHostSummary selectedHost)
        {
            MessageBox.Show(this, "Load and select a host first.", "Host required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        await StartManagedReceiverSessionAsync(selectedHost, allowActorSessionReplacement, allowLocalRetry: true);
    }

    private async Task StartManagedReceiverSessionAsync(
        DesktopControlPlaneHostSummary selectedHost,
        bool allowActorSessionReplacement,
        bool allowLocalRetry)
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl))
        {
            MessageBox.Show(this, "Enter a control plane URL first.", "Control plane required", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        if (!int.TryParse(_portBox.Text.Trim(), out var port) || port is < 1 or > 65535)
        {
            MessageBox.Show(this, "Enter a valid listener port first.", "Invalid port", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            return;
        }

        DesktopControlPlaneSessionLease? lease = null;
        try
        {
            if (!_session.GetSnapshot().Listening)
            {
                await StartSessionAsync();
            }

            if (!_session.GetSnapshot().Listening)
            {
                return;
            }

            if (!TryResolveManagedReceiverEndpoint(port, out var receiverHost, out var receiverPort))
            {
                MessageBox.Show(this, "Desktop receiver LAN IP is unavailable.", "Receiver endpoint unavailable", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            var effectivePreferRelay = _managedPreferRelayCheck.Checked || ShouldForceRelayForManagedSession(baseUrl, receiverHost);
            if (effectivePreferRelay && !_managedPreferRelayCheck.Checked)
            {
                ShowFriendlyStatus("Внешний сервер обнаружен. Для стабильности включаю relay автоматически.", sticky: true);
            }
            var preferredCodecs = BuildManagedPreferredCodecs();

            lease = await _desktopControlPlaneClient.CreateSessionAsync(
                baseUrl: baseUrl,
                hostId: selectedHost.HostId,
                clientLabel: $"{Environment.MachineName.ToLowerInvariant()}-desktop-receiver",
                clientRegion: string.IsNullOrWhiteSpace(_controlRegionBox.Text) ? "global" : _controlRegionBox.Text.Trim(),
                codecPreference: preferredCodecs.FirstOrDefault(),
                preferRelay: effectivePreferRelay,
                audioRequested: _managedRequestAudioCheck.Checked,
                controllerCount: 1,
                leaseMinutes: 30,
                receiverAddress: receiverHost,
                receiverPort: receiverPort,
                desiredStream: new DesktopControlPlaneDesiredStreamRequest(
                    Width: null,
                    Height: null,
                    Fps: null,
                    BitrateBps: null,
                    CaptureCursor: false,
                    AdaptiveMode: false,
                    PreferredCodecs: preferredCodecs,
                    PresetId: "desktop_low_latency"),
                clientCapabilities: new DesktopControlPlaneClientCapabilities(
                    SupportedDecodeCodecs: GetDesktopSupportedDecodeCodecs(),
                    LanAddresses: GetLanAddresses()));

            try
            {
                await _desktopControlPlaneClient.PublishNatProbeAsync(
                    baseUrl,
                    lease.SessionId,
                    lease.SessionToken,
                    lease.ProbeToken,
                    lease.ProbeAddress ?? string.Empty,
                    lease.ProbePort ?? 0,
                    "client");
            }
            catch
            {
            }

            await _desktopControlPlaneClient.ActivateSessionAsync(baseUrl, lease.SessionId, lease.SessionToken);
            var connectInstructions = await _desktopControlPlaneClient.GetConnectInstructionsAsync(baseUrl, lease.SessionId, lease.SessionToken);
            var relayRegistrationRoute = TryBuildRelayRoute(
                sessionId: lease.SessionId,
                sessionToken: lease.SessionToken,
                relayHost: connectInstructions.RelayHost,
                relayPort: connectInstructions.RelayPort);
            var relayRoute = BuildManagedRelayRoute(
                connectInstructions.RouteKind,
                sessionId: lease.SessionId,
                sessionToken: lease.SessionToken,
                relayHost: connectInstructions.RelayHost,
                relayPort: connectInstructions.RelayPort);
            _session.ConfigureRelayRegistrationRoute(relayRegistrationRoute);
            _session.ConfigureRelayRoute(relayRoute);

            _managedSessionId = lease.SessionId;
            _managedSessionToken = lease.SessionToken;
            _managedHostId = lease.HostId;
            _managedSessionHostLabel = lease.HostDisplayName;
            _managedRouteKind = connectInstructions.RouteKind;
            _managedRouteState = connectInstructions.RouteState;
            _managedRouteVersion = connectInstructions.RouteVersion;
            _managedSessionHealth = connectInstructions.SessionHealth;
            _managedSessionHealthReason = connectInstructions.SessionHealthReason;
            _managedRouteActionHint = connectInstructions.RouteActionHint;
            _managedRouteActionReason = connectInstructions.RouteActionReason;
            _managedRouteFallbackReadyDurationSeconds = connectInstructions.RouteFallbackReadyDurationSeconds;
            _managedRouteRecoveryReadyDurationSeconds = connectInstructions.RouteRecoveryReadyDurationSeconds;
            _managedRecommendedSyncDelaySeconds = Math.Clamp(connectInstructions.RecommendedSyncDelaySeconds, 5, 60);
            _managedTransportLossLevel = connectInstructions.TransportLossLevel;
            _managedTransportAnomalyKind = connectInstructions.TransportAnomalyKind;
            _managedTransportAnomalyReason = connectInstructions.TransportAnomalyReason;
            _managedTransportAnomalyConfidence = connectInstructions.TransportAnomalyConfidence;
            _managedReceiverTelemetryAgeSeconds = connectInstructions.ReceiverTelemetryAgeSeconds;
            _managedSenderTelemetryAgeSeconds = connectInstructions.SenderTelemetryAgeSeconds;
            _managedLastRouteActionKind = connectInstructions.LastRouteActionKind ?? "-";
            _managedLastRouteActionReason = connectInstructions.LastRouteActionReason ?? "-";
            _managedLastRouteActionActor = connectInstructions.LastRouteActionActor ?? "-";
            _managedLastRouteActionUtc = connectInstructions.LastRouteActionUtc?.ToString("u") ?? "-";
            _managedRouteRecoveryCount = connectInstructions.RouteRecoveryCount;
            _managedRouteRecoveryCooldownSeconds = connectInstructions.RouteRecoveryCooldownSeconds;
            _managedRouteFallbackCount = connectInstructions.RouteFallbackCount;
            _managedRouteFallbackCooldownSeconds = connectInstructions.RouteFallbackCooldownSeconds;
            _managedRelayEndpoint = connectInstructions.RelayHost is not null && connectInstructions.RelayPort is not null
                ? $"{connectInstructions.RelayHost}:{connectInstructions.RelayPort} ({connectInstructions.RelayRegion ?? "relay"})"
                : "-";
            _managedNatStatus = connectInstructions.NatStatus;
            _managedHostNatProbeAgeSeconds = connectInstructions.HostNatProbeAgeSeconds;
            _managedClientNatProbeAgeSeconds = connectInstructions.ClientNatProbeAgeSeconds;
            _managedNatProbeFresh = connectInstructions.NatProbeFresh;
            _managedHostNatProbeAgeSeconds = connectInstructions.HostNatProbeAgeSeconds;
            _managedClientNatProbeAgeSeconds = connectInstructions.ClientNatProbeAgeSeconds;
            _managedNatProbeFresh = connectInstructions.NatProbeFresh;
            _managedSessionSyncFailureCount = 0;
            _managedSessionHealthDegradedStreak = 0;
            _managedRouteFallbackArmed = true;
            _managedSessionSyncDelayMs = _managedRecommendedSyncDelaySeconds * 1000;
            _desktopControlPlaneClient.SaveManagedSessionState(new DesktopControlPlaneManagedSessionState(
                BaseUrl: baseUrl,
                SessionId: lease.SessionId,
                SessionToken: lease.SessionToken,
                HostId: lease.HostId,
                HostDisplayName: lease.HostDisplayName,
                RouteKind: connectInstructions.RouteKind,
                RouteState: connectInstructions.RouteState,
                RouteVersion: connectInstructions.RouteVersion,
                SessionHealth: connectInstructions.SessionHealth,
                SessionHealthReason: connectInstructions.SessionHealthReason,
                RouteActionHint: connectInstructions.RouteActionHint,
                RouteActionReason: connectInstructions.RouteActionReason,
                RouteFallbackReadyDurationSeconds: connectInstructions.RouteFallbackReadyDurationSeconds,
                RouteRecoveryReadyDurationSeconds: connectInstructions.RouteRecoveryReadyDurationSeconds,
                RecommendedSyncDelaySeconds: connectInstructions.RecommendedSyncDelaySeconds,
                TransportLossLevel: connectInstructions.TransportLossLevel,
                ReceiverTelemetryAgeSeconds: connectInstructions.ReceiverTelemetryAgeSeconds,
                SenderTelemetryAgeSeconds: connectInstructions.SenderTelemetryAgeSeconds,
                RouteRecoveryCount: connectInstructions.RouteRecoveryCount,
                RouteRecoveryCooldownSeconds: connectInstructions.RouteRecoveryCooldownSeconds,
                RouteFallbackCount: connectInstructions.RouteFallbackCount,
                RouteFallbackCooldownSeconds: connectInstructions.RouteFallbackCooldownSeconds,
                NatStatus: connectInstructions.NatStatus,
                HostNatProbeAgeSeconds: connectInstructions.HostNatProbeAgeSeconds,
                ClientNatProbeAgeSeconds: connectInstructions.ClientNatProbeAgeSeconds,
                NatProbeFresh: connectInstructions.NatProbeFresh,
                RelayAddress: connectInstructions.RelayHost,
                RelayPort: connectInstructions.RelayPort,
                ReceiverAddress: connectInstructions.StreamHost,
                ReceiverPort: connectInstructions.StreamPort,
                ProbeAddress: lease.ProbeAddress,
                ProbePort: lease.ProbePort,
                ProbeToken: lease.ProbeToken,
                TransportAnomalyKind: connectInstructions.TransportAnomalyKind,
                TransportAnomalyReason: connectInstructions.TransportAnomalyReason,
                TransportAnomalyConfidence: connectInstructions.TransportAnomalyConfidence));
            _statusLabel.Text = $"Managed session active for {lease.HostDisplayName}. Route: {connectInstructions.RouteKind} ({connectInstructions.RouteState}), Health: {connectInstructions.SessionHealth} ({connectInstructions.SessionHealthReason}), Action: {connectInstructions.RouteActionHint} ({connectInstructions.RouteActionReason}), Sync: {connectInstructions.RecommendedSyncDelaySeconds}s, Loss: {connectInstructions.TransportLossLevel}, Telemetry age: {connectInstructions.ReceiverTelemetryAgeSeconds}s/{connectInstructions.SenderTelemetryAgeSeconds}s, Fallbacks: {connectInstructions.RouteFallbackCount}, Cooldown: {connectInstructions.RouteFallbackCooldownSeconds}s. NAT: {connectInstructions.NatStatus}, probe age: {connectInstructions.HostNatProbeAgeSeconds}s/{connectInstructions.ClientNatProbeAgeSeconds}s, fresh: {connectInstructions.NatProbeFresh}.";
        }
        catch (ControlPlaneApiException ex) when (string.Equals(ex.Code, "actor_session_exists", StringComparison.Ordinal))
        {
            var savedManagedSession = _desktopControlPlaneClient.GetManagedSessionState(baseUrl);
            if (savedManagedSession is not null)
            {
                _statusLabel.Text = "Для этого аккаунта уже есть активная сессия. Пробую восстановить текущее подключение.";
                await ResumeManagedReceiverSessionAsync();
                return;
            }

            if (allowActorSessionReplacement &&
                TryExtractExistingActorSessionId(ex.Message, out var existingActorSessionId))
            {
                try
                {
                    _statusLabel.Text = "Для этого аккаунта уже есть активная сессия. Останавливаю старую и запускаю новую.";
                    await _desktopControlPlaneClient.StopSessionForActorAsync(
                        baseUrl,
                        existingActorSessionId,
                        "desktop_replace_existing_actor_session");
                    await Task.Delay(300);
                    await StartManagedReceiverSessionAsync(selectedHost, allowActorSessionReplacement: false, allowLocalRetry);
                    return;
                }
                catch (Exception replacementEx)
                {
                    var replacementMessage = $"Не удалось заменить старую сессию: {replacementEx.Message}";
                    MessageBox.Show(this, replacementMessage, "Сессия уже запущена", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                    _statusLabel.Text = replacementMessage;
                    return;
                }
            }

            var friendlyMessage = "Для этого аккаунта уже есть активная сессия на другом устройстве. Останови ее там или войди другим аккаунтом.";
            MessageBox.Show(this, friendlyMessage, "Сессия уже запущена", MessageBoxButtons.OK, MessageBoxIcon.Information);
            _statusLabel.Text = friendlyMessage;
        }
        catch (Exception ex)
        {
            if (lease is not null)
            {
                try
                {
                    await _desktopControlPlaneClient.StopSessionAsync(baseUrl, lease.SessionId, lease.SessionToken, "desktop_receiver_setup_failed");
                }
                catch
                {
                }
            }

            if (allowLocalRetry && ShouldRetryManagedReceiverSetup(ex.Message))
            {
                ShowFriendlyStatus("Повторяю подключение. Перезапускаю локальный listener.", sticky: true);
                try
                {
                    _session.Stop();
                }
                catch
                {
                }

                await Task.Delay(250);
                await StartManagedReceiverSessionAsync(selectedHost, allowActorSessionReplacement, allowLocalRetry: false);
                return;
            }

            MessageBox.Show(this, ex.Message, "Failed to start managed session", MessageBoxButtons.OK, MessageBoxIcon.Error);
            _statusLabel.Text = $"Managed session failed: {ex.Message}";
        }
        finally
        {
            RefreshManagedClientUiState();
            RenderSnapshot(_session.GetSnapshot());
        }
    }

    private static bool TryExtractExistingActorSessionId(string message, out string sessionId)
    {
        sessionId = string.Empty;
        if (string.IsNullOrWhiteSpace(message))
        {
            return false;
        }

        var marker = "session_";
        var startIndex = message.IndexOf(marker, StringComparison.Ordinal);
        if (startIndex < 0)
        {
            return false;
        }

        var endIndex = startIndex;
        while (endIndex < message.Length)
        {
            var ch = message[endIndex];
            if (!(char.IsLetterOrDigit(ch) || ch == '_'))
            {
                break;
            }

            endIndex++;
        }

        sessionId = message[startIndex..endIndex];
        return !string.IsNullOrWhiteSpace(sessionId);
    }

    private static bool ShouldForceRelayForManagedSession(string baseUrl, string receiverHost)
    {
        if (!Uri.TryCreate(baseUrl, UriKind.Absolute, out var uri))
        {
            return false;
        }

        if (!IPAddress.TryParse(uri.Host, out var controlPlaneIp) || IsPrivateOrLocalIp(controlPlaneIp))
        {
            return false;
        }

        return IPAddress.TryParse(receiverHost, out var receiverIp) && IsPrivateOrLocalIp(receiverIp);
    }

    private static bool IsPrivateOrLocalIp(IPAddress address)
    {
        if (IPAddress.IsLoopback(address))
        {
            return true;
        }

        if (address.AddressFamily == AddressFamily.InterNetworkV6)
        {
            return address.IsIPv6LinkLocal || address.IsIPv6SiteLocal;
        }

        var bytes = address.GetAddressBytes();
        return bytes[0] switch
        {
            10 => true,
            127 => true,
            169 when bytes[1] == 254 => true,
            172 when bytes[1] >= 16 && bytes[1] <= 31 => true,
            192 when bytes[1] == 168 => true,
            _ => false,
        };
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

    private static string NormalizeHostCode(string value)
    {
        return string.IsNullOrWhiteSpace(value)
            ? string.Empty
            : value.Trim().Replace(" ", string.Empty, StringComparison.Ordinal).ToLowerInvariant();
    }

    private static string BuildManagedHostUiLabel(DesktopControlPlaneHostSummary host)
    {
        var availability = !host.Online
            ? "Оффлайн"
            : !string.IsNullOrWhiteSpace(host.ActiveSessionId)
                ? "Занят"
                : "Можно подключиться";

        return host.PricePerHour is > 0 && !string.IsNullOrWhiteSpace(host.Currency)
            ? $"{host.DisplayName} [{host.HostCode}] [{host.Region}] {host.PricePerHour:0.##} {host.Currency}/ч {availability}"
            : $"{host.DisplayName} [{host.HostCode}] [{host.Region}] {availability}";
    }

    private static bool IsUdpPortAvailable(int port)
    {
        try
        {
            using var udp = new UdpClient(port);
            return true;
        }
        catch (SocketException)
        {
            return false;
        }
    }

    private static int FindAvailableUdpPort(int startPort, int maxProbeCount)
    {
        for (var port = startPort; port < startPort + Math.Max(1, maxProbeCount); port++)
        {
            if (port is < 1 or > 65535)
            {
                continue;
            }

            if (IsUdpPortAvailable(port))
            {
                return port;
            }
        }

        return -1;
    }

    private static bool ShouldRetryManagedReceiverSetup(string message)
    {
        if (string.IsNullOrWhiteSpace(message))
        {
            return false;
        }

        return message.Contains("did not start streaming", StringComparison.OrdinalIgnoreCase) ||
               message.Contains("registered in relay", StringComparison.OrdinalIgnoreCase) ||
               message.Contains("offline", StringComparison.OrdinalIgnoreCase) ||
               message.Contains("timed out", StringComparison.OrdinalIgnoreCase);
    }

    private async Task ResumeManagedReceiverSessionAsync()
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl))
        {
            return;
        }

        var managed = _desktopControlPlaneClient.GetManagedSessionState(baseUrl);
        if (managed is null)
        {
            return;
        }

        _managedRouteVersion = managed.RouteVersion;
        _managedRouteActionHint = managed.RouteActionHint;
        _managedRouteActionReason = managed.RouteActionReason;
        _managedRouteFallbackReadyDurationSeconds = managed.RouteFallbackReadyDurationSeconds;
        _managedRouteRecoveryReadyDurationSeconds = managed.RouteRecoveryReadyDurationSeconds;
        _managedRecommendedSyncDelaySeconds = Math.Clamp(managed.RecommendedSyncDelaySeconds, 5, 60);
        _managedTransportLossLevel = managed.TransportLossLevel;
        _managedTransportAnomalyKind = managed.TransportAnomalyKind;
        _managedTransportAnomalyReason = managed.TransportAnomalyReason;
        _managedTransportAnomalyConfidence = managed.TransportAnomalyConfidence;
        _managedReceiverTelemetryAgeSeconds = managed.ReceiverTelemetryAgeSeconds;
        _managedSenderTelemetryAgeSeconds = managed.SenderTelemetryAgeSeconds;
        _managedLastRouteActionKind = managed.LastRouteActionKind ?? "-";
        _managedLastRouteActionReason = managed.LastRouteActionReason ?? "-";
        _managedLastRouteActionActor = managed.LastRouteActionActor ?? "-";
        _managedLastRouteActionUtc = managed.LastRouteActionUtc?.ToString("u") ?? "-";
        _managedHostNatProbeAgeSeconds = managed.HostNatProbeAgeSeconds;
        _managedClientNatProbeAgeSeconds = managed.ClientNatProbeAgeSeconds;
        _managedNatProbeFresh = managed.NatProbeFresh;

        if (HasManagedClientSession && string.Equals(_managedSessionId, managed.SessionId, StringComparison.Ordinal))
        {
            return;
        }

        try
        {
            if (!_session.GetSnapshot().Listening)
            {
                await StartSessionAsync();
            }

            if (!_session.GetSnapshot().Listening)
            {
                return;
            }

            var connectInstructions = await _desktopControlPlaneClient.ResumeManagedSessionAsync(baseUrl, managed.SessionId, managed.SessionToken);
            var relayRegistrationRoute = TryBuildRelayRoute(
                sessionId: managed.SessionId,
                sessionToken: managed.SessionToken,
                relayHost: connectInstructions.RelayHost,
                relayPort: connectInstructions.RelayPort);
            var relayRoute = BuildManagedRelayRoute(
                connectInstructions.RouteKind,
                sessionId: managed.SessionId,
                sessionToken: managed.SessionToken,
                relayHost: connectInstructions.RelayHost,
                relayPort: connectInstructions.RelayPort);
            _session.ConfigureRelayRegistrationRoute(relayRegistrationRoute);
            _session.ConfigureRelayRoute(relayRoute);

            _managedSessionId = managed.SessionId;
            _managedSessionToken = managed.SessionToken;
            _managedHostId = managed.HostId;
            _managedSessionHostLabel = managed.HostDisplayName;
            _managedRouteKind = connectInstructions.RouteKind;
            _managedRouteState = connectInstructions.RouteState;
            _managedRouteVersion = connectInstructions.RouteVersion;
            _managedSessionHealth = connectInstructions.SessionHealth;
            _managedSessionHealthReason = connectInstructions.SessionHealthReason;
            _managedRouteActionHint = connectInstructions.RouteActionHint;
            _managedRouteActionReason = connectInstructions.RouteActionReason;
            _managedRouteFallbackReadyDurationSeconds = connectInstructions.RouteFallbackReadyDurationSeconds;
            _managedRouteRecoveryReadyDurationSeconds = connectInstructions.RouteRecoveryReadyDurationSeconds;
            _managedRecommendedSyncDelaySeconds = Math.Clamp(connectInstructions.RecommendedSyncDelaySeconds, 5, 60);
            _managedTransportLossLevel = connectInstructions.TransportLossLevel;
            _managedTransportAnomalyKind = connectInstructions.TransportAnomalyKind;
            _managedTransportAnomalyReason = connectInstructions.TransportAnomalyReason;
            _managedTransportAnomalyConfidence = connectInstructions.TransportAnomalyConfidence;
            _managedReceiverTelemetryAgeSeconds = connectInstructions.ReceiverTelemetryAgeSeconds;
            _managedSenderTelemetryAgeSeconds = connectInstructions.SenderTelemetryAgeSeconds;
            _managedLastRouteActionKind = connectInstructions.LastRouteActionKind ?? "-";
            _managedLastRouteActionReason = connectInstructions.LastRouteActionReason ?? "-";
            _managedLastRouteActionActor = connectInstructions.LastRouteActionActor ?? "-";
            _managedLastRouteActionUtc = connectInstructions.LastRouteActionUtc?.ToString("u") ?? "-";
            _managedRouteRecoveryCount = connectInstructions.RouteRecoveryCount;
            _managedRouteRecoveryCooldownSeconds = connectInstructions.RouteRecoveryCooldownSeconds;
            _managedRouteFallbackCount = connectInstructions.RouteFallbackCount;
            _managedRouteFallbackCooldownSeconds = connectInstructions.RouteFallbackCooldownSeconds;
            _managedRelayEndpoint = connectInstructions.RelayHost is not null && connectInstructions.RelayPort is not null
                ? $"{connectInstructions.RelayHost}:{connectInstructions.RelayPort} ({connectInstructions.RelayRegion ?? "relay"})"
                : "-";
            _managedNatStatus = connectInstructions.NatStatus;
            _managedHostNatProbeAgeSeconds = connectInstructions.HostNatProbeAgeSeconds;
            _managedClientNatProbeAgeSeconds = connectInstructions.ClientNatProbeAgeSeconds;
            _managedNatProbeFresh = connectInstructions.NatProbeFresh;
            _managedSessionSyncFailureCount = 0;
            _managedSessionHealthDegradedStreak = 0;
            _managedRouteFallbackArmed = true;
            _managedSessionSyncDelayMs = _managedRecommendedSyncDelaySeconds * 1000;
            _statusLabel.Text = $"Resumed managed session for {managed.HostDisplayName}. Route: {connectInstructions.RouteKind} ({connectInstructions.RouteState}), Health: {connectInstructions.SessionHealth} ({connectInstructions.SessionHealthReason}), Action: {connectInstructions.RouteActionHint} ({connectInstructions.RouteActionReason}), Sync: {connectInstructions.RecommendedSyncDelaySeconds}s, Loss: {connectInstructions.TransportLossLevel}, Telemetry age: {connectInstructions.ReceiverTelemetryAgeSeconds}s/{connectInstructions.SenderTelemetryAgeSeconds}s, Fallbacks: {connectInstructions.RouteFallbackCount}, Cooldown: {connectInstructions.RouteFallbackCooldownSeconds}s. NAT: {connectInstructions.NatStatus}, probe age: {connectInstructions.HostNatProbeAgeSeconds}s/{connectInstructions.ClientNatProbeAgeSeconds}s, fresh: {connectInstructions.NatProbeFresh}.";
        }
        catch (Exception ex)
        {
            _desktopControlPlaneClient.ClearManagedSessionState(baseUrl);
            _session.ConfigureRelayRoute(null);
            _session.ConfigureRelayRegistrationRoute(null);
            _statusLabel.Text = $"Managed session resume failed: {ex.Message}";
        }
        finally
        {
            RefreshManagedClientUiState();
            RenderSnapshot(_session.GetSnapshot());
        }
    }

    private async Task StopManagedReceiverSessionAsync(string reason, bool stopLocalReceiver)
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        var sessionId = _managedSessionId;
        var sessionToken = _managedSessionToken;
        var hadManagedSession = HasManagedClientSession;

        _managedSessionId = string.Empty;
        _managedSessionToken = string.Empty;
        _managedHostId = string.Empty;
        _managedSessionHostLabel = "-";
        _managedRouteKind = "-";
        _managedRouteState = "-";
        _managedRouteVersion = 0;
        _managedSessionHealth = "-";
        _managedSessionHealthReason = "-";
        _managedRouteActionHint = "-";
        _managedRouteActionReason = "-";
        _managedRouteFallbackReadyDurationSeconds = 0;
        _managedRouteRecoveryReadyDurationSeconds = 0;
        _managedRecommendedSyncDelaySeconds = 10;
        _managedTransportLossLevel = "-";
        _managedTransportAnomalyKind = "-";
        _managedTransportAnomalyReason = "-";
        _managedTransportAnomalyConfidence = "-";
        _managedReceiverTelemetryAgeSeconds = -1;
        _managedSenderTelemetryAgeSeconds = -1;
        _managedLastRouteActionKind = "-";
        _managedLastRouteActionReason = "-";
        _managedLastRouteActionActor = "-";
        _managedLastRouteActionUtc = "-";
        _managedRouteRecoveryCount = 0;
        _managedRouteRecoveryCooldownSeconds = 0;
        _managedRouteFallbackCount = 0;
        _managedRouteFallbackCooldownSeconds = 0;
        _managedRelayEndpoint = "-";
        _managedNatStatus = "-";
        _managedHostNatProbeAgeSeconds = -1;
        _managedClientNatProbeAgeSeconds = -1;
        _managedNatProbeFresh = false;
        _managedSessionSyncFailureCount = 0;
        _managedSessionHealthDegradedStreak = 0;
        _managedRouteFallbackArmed = true;
        _managedSessionSyncDelayMs = 10_000;
        _desktopControlPlaneClient.ClearManagedSessionState(baseUrl);
        _session.ConfigureRelayRoute(null);
        _session.ConfigureRelayRegistrationRoute(null);
        RefreshManagedClientUiState();

        if (hadManagedSession && !string.IsNullOrWhiteSpace(baseUrl))
        {
            try
            {
                await _desktopControlPlaneClient.StopSessionAsync(baseUrl, sessionId, sessionToken, reason);
                _statusLabel.Text = "Managed session stopped.";
            }
            catch (Exception ex)
            {
                _statusLabel.Text = $"Managed session stop failed: {ex.Message}";
            }
        }

        if (stopLocalReceiver)
        {
            BeginSessionAction(closeAfterCompletion: false);
        }
        else
        {
            RenderSnapshot(_session.GetSnapshot());
        }
    }

    private bool HasManagedClientSession =>
        !string.IsNullOrWhiteSpace(_managedSessionId) &&
        !string.IsNullOrWhiteSpace(_managedSessionToken);

    private void RefreshManagedClientUiState()
    {
        var controlsEnabled = _currentRole == AppRole.Receive;
        var hasSavedManagedSession = !string.IsNullOrWhiteSpace(_controlPlaneUrlBox.Text) &&
            _desktopControlPlaneClient.GetManagedSessionState(_controlPlaneUrlBox.Text) is not null;
        _managedHostBox.Enabled = controlsEnabled;
        _managedRefreshHostsButton.Enabled = controlsEnabled;
        _managedResumeSessionButton.Enabled = controlsEnabled && hasSavedManagedSession;
        _managedPreferHevcCheck.Enabled = controlsEnabled;
        _managedPreferRelayCheck.Enabled = controlsEnabled;
        _managedRequestAudioCheck.Enabled = controlsEnabled;
        _managedStartSessionButton.Enabled = controlsEnabled;
        _managedStopSessionButton.Enabled = controlsEnabled && HasManagedClientSession;
    }

    private static bool TryResolveManagedReceiverEndpoint(int port, out string host, out int resolvedPort)
    {
        host = ResolvePreferredIpv4Address();
        resolvedPort = port is > 0 and <= 65535 ? port : 5001;
        return !string.IsNullOrWhiteSpace(host);
    }

    private static (IPAddress Address, int Port, string DeviceName)? ProbeLanForReceiverAsync(int port)
    {
        using var udpClient = new UdpClient(AddressFamily.InterNetwork);
        udpClient.EnableBroadcast = true;
        udpClient.Client.ReceiveTimeout = 250;
        udpClient.Client.SendTimeout = 250;
        udpClient.Client.Bind(new IPEndPoint(IPAddress.Any, 0));

        var probe = ControlPacketBuilder.BuildDiscoveryProbe(Environment.MachineName);
        var targets = GetDiscoveryBroadcastEndpoints(port).ToArray();

        foreach (var target in targets)
        {
            try
            {
                udpClient.Send(probe, probe.Length, target);
            }
            catch
            {
                // Ignore dead interfaces and keep probing the rest.
            }
        }

        var deadline = Stopwatch.GetTimestamp() + Stopwatch.Frequency;
        while (Stopwatch.GetTimestamp() < deadline)
        {
            try
            {
                var remote = new IPEndPoint(IPAddress.Any, 0);
                var response = udpClient.Receive(ref remote);
                if (!ProtocolParser.TryParse(response, response.Length, out var packet) || packet is null || packet.Type != TransportProtocol.TypeControl)
                {
                    continue;
                }

                var discovery = ControlMessageParser.TryParseDiscoveryResponse(packet.Payload);
                if (discovery is null)
                {
                    continue;
                }

                return (remote.Address, discovery.Port, discovery.DeviceName);
            }
            catch (SocketException)
            {
                // Timeout; loop until deadline.
            }
        }

        return null;
    }

    private static IEnumerable<IPEndPoint> GetDiscoveryBroadcastEndpoints(int port)
    {
        yield return new IPEndPoint(IPAddress.Broadcast, port);

        foreach (var networkInterface in NetworkInterface.GetAllNetworkInterfaces())
        {
            if (networkInterface.OperationalStatus != OperationalStatus.Up)
            {
                continue;
            }

            IPInterfaceProperties? properties;
            try
            {
                properties = networkInterface.GetIPProperties();
            }
            catch
            {
                continue;
            }

            foreach (var unicastAddress in properties.UnicastAddresses)
            {
                if (unicastAddress.Address.AddressFamily != AddressFamily.InterNetwork ||
                    unicastAddress.IPv4Mask is null)
                {
                    continue;
                }

                var addressBytes = unicastAddress.Address.GetAddressBytes();
                var maskBytes = unicastAddress.IPv4Mask.GetAddressBytes();
                var broadcastBytes = new byte[4];
                for (var index = 0; index < 4; index++)
                {
                    broadcastBytes[index] = (byte)(addressBytes[index] | ~maskBytes[index]);
                }

                yield return new IPEndPoint(new IPAddress(broadcastBytes), port);
            }
        }
    }

    private static string ResolvePreferredIpv4Address()
    {
        foreach (var networkInterface in NetworkInterface.GetAllNetworkInterfaces())
        {
            if (networkInterface.OperationalStatus != OperationalStatus.Up ||
                networkInterface.NetworkInterfaceType == NetworkInterfaceType.Loopback)
            {
                continue;
            }

            IPInterfaceProperties? properties;
            try
            {
                properties = networkInterface.GetIPProperties();
            }
            catch
            {
                continue;
            }

            foreach (var unicast in properties.UnicastAddresses)
            {
                if (unicast.Address.AddressFamily == AddressFamily.InterNetwork && !IPAddress.IsLoopback(unicast.Address))
                {
                    return unicast.Address.ToString();
                }
            }
        }

        return string.Empty;
    }

    private async Task SwitchRoleAsync(AppRole role)
    {
        if (_currentRole == role || IsDisposed)
        {
            return;
        }

        SetRemoteInputCaptureArmed(false, sendReleaseAll: true);
        var shouldStopManagedSession = _currentRole == AppRole.Receive && HasManagedClientSession;

        await RunSessionMutationAsync(
            $"Switching to {role.ToUiLabel()}...",
            () =>
            {
                if (_currentRole == AppRole.Receive)
                {
                    _session.Stop();
                }
                else
                {
                    _senderSession.Stop();
                    RestoreSenderWindowIfNeeded();
                }
            });

        if (shouldStopManagedSession)
        {
            await StopManagedReceiverSessionAsync("role_switch", stopLocalReceiver: false);
        }

        _currentRole = role;
        if (_currentRole != AppRole.Receive && _tuningOverlay is not null && !_tuningOverlay.IsDisposed)
        {
            _tuningOverlay.Close();
            _tuningOverlay = null;
        }

        UpdateRoleUi();
        RenderCurrentSnapshot();

        if (_currentRole == AppRole.Receive && !HasManagedClientSession)
        {
            await ResumeManagedReceiverSessionAsync();
        }
    }

    private void UpdateRoleUi()
    {
        var receiveMode = _currentRole == AppRole.Receive;
        var sendMode = !receiveMode;
        var advancedMode = _advancedModeCheck.Checked;

        _commonSection.Visible = true;
        _clientQuickSection.Visible = receiveMode;
        _clientSettingsSection.Visible = receiveMode && advancedMode;
        _hostSection.Visible = sendMode;

        SetControlVisibility(receiveMode,
            _managedHostsLabel,
            _managedHostCodeBox,
            _managedStartByCodeButton,
            _managedHostBox,
            _managedRefreshHostsButton,
            _managedResumeSessionButton,
            _managedStartSessionButton,
            _managedStopSessionButton,
            _managedCloseSessionButton);

        SetControlVisibility(sendMode,
            _sendTargetLabel,
            _sendTargetBox,
            _sendHostLabel,
            _sendHostBox,
            _sendPortLabel,
            _sendPortBox,
            _startSendingButton,
            _stopSendingButton,
            _sendPresetLabel,
            _sendPresetBox,
            _sendEncoderLabel,
            _sendEncoderBox,
            _sendCodecLabel,
            _sendCodecBox,
            _sendWidthLabel,
            _sendWidthBox,
            _sendHeightLabel,
            _sendHeightBox,
            _sendFpsLabel,
            _sendFpsBox,
            _sendBitrateLabel,
            _sendBitrateBox,
            _sendAudioCheck,
            _sendCursorCheck,
            _sendPulseFlashCheck,
            _sendAdaptiveCheck,
            _leaseAutoRunCheck);

        SetControlVisibility(true,
            _controlPlaneLabel,
            _controlPlaneUrlBox,
            _simpleModeHintLabel,
            _controlAuthLabel,
            _controlDemoAdminButton,
            _controlDemoTestButton,
            _advancedModeCheck);
        SetControlVisibility(advancedMode,
            _controlRegionLabel,
            _controlRegionBox,
            _controlUserEmailBox,
            _controlUserPasswordBox,
            _controlUserLoginButton,
            _controlUserRegisterButton);

        SetControlVisibility(receiveMode && advancedMode,
            _managedPreferHevcCheck,
            _managedPreferRelayCheck,
            _managedRequestAudioCheck,
            _transportLabel,
            _transportBox,
            _portLabel,
            _portBox,
            _backendLabel,
            _backendBox,
            _decoderLabel,
            _decoderBox,
            _ultraLowLatencyCheck,
            _aggressiveTailDropCheck,
            _captureInputCheck,
            _relativeMouseCheck,
            _inputStateLabel,
            _prepareAdbButton,
            _startButton,
            _stopButton,
            _tuningButton,
            _fullscreenButton);

        SetControlVisibility(sendMode && advancedMode,
            _sendTargetLabel,
            _sendTargetBox,
            _sendHostLabel,
            _sendHostBox,
            _sendPortLabel,
            _sendPortBox,
            _sendPresetLabel,
            _sendPresetBox,
            _sendEncoderLabel,
            _sendEncoderBox,
            _sendCodecLabel,
            _sendCodecBox,
            _sendWidthLabel,
            _sendWidthBox,
            _sendHeightLabel,
            _sendHeightBox,
            _sendFpsLabel,
            _sendFpsBox,
            _sendBitrateLabel,
            _sendBitrateBox,
            _sendAudioCheck,
            _sendCursorCheck,
            _sendPulseFlashCheck,
            _sendAdaptiveCheck,
            _leaseAutoRunCheck,
            _startSendingButton,
            _stopSendingButton);

        _senderOverlayLabel.Visible = sendMode;
        _senderOverlayLabel.BringToFront();
        _playbackHost.BackColor = sendMode ? Color.FromArgb(18, 18, 20) : Color.Black;

        if (!receiveMode)
        {
            SetRemoteInputCaptureArmed(false, sendReleaseAll: true);
        }

        UpdateInputCaptureStateLabel();
        RefreshManagedClientUiState();
    }

    private void ApplyLaunchOptions()
    {
        _suppressControlEvents = true;
        _currentRole = _launchOptions.InitialRole;
        _roleBox.SelectedItem = _launchOptions.InitialRole;
        _advancedModeCheck.Checked = _launchOptions.AdvancedMode;
        if (_launchOptions.InitialRole == AppRole.Send)
        {
            _leaseAutoRunCheck.Checked = true;
        }
        _roleLabel.Visible = !_lockRoleSelection;
        _roleBox.Visible = !_lockRoleSelection;
        _suppressControlEvents = false;

        UpdateRoleUi();
        ForceDefaultSenderSettings();
        _roleBox.Enabled = !_lockRoleSelection;
        Text = _currentRole == AppRole.Send ? "Everty Studio Host" : "Everty Studio Client";

        if (_currentRole == AppRole.Send)
        {
            ShowFriendlyStatus("Этот ПК готовится к публикации. Оставь окно открытым и подключайся с телефона.", sticky: true);
        }
        else
        {
            ShowFriendlyStatus("Введи short code или выбери компьютер в списке.", sticky: true);
        }
    }

    private void ForceDefaultSenderRole()
    {
        _suppressControlEvents = true;
        _currentRole = AppRole.Send;
        _roleBox.SelectedItem = AppRole.Send;
        _suppressControlEvents = false;
        UpdateRoleUi();
    }

    private void ForceDefaultSenderSettings()
    {
        _suppressControlEvents = true;
        _sendPresetBox.SelectedItem = WindowsSenderPreset.Game;
        _sendEncoderBox.SelectedItem = WindowsSenderEncoderBackend.Auto;
        _sendCodecBox.SelectedItem = WindowsVideoCodec.H265Hevc;
        ApplySenderPresetTemplate(WindowsSenderPreset.Game);
        _suppressControlEvents = false;
    }

    private static void SetControlVisibility(bool visible, params Control[] controls)
    {
        foreach (var control in controls)
        {
            control.Visible = visible;
        }
    }

    private void SetRemoteInputCaptureArmed(bool armed, bool sendReleaseAll)
    {
        var canArm =
            armed &&
            _currentRole == AppRole.Receive &&
            _session.GetSnapshot() is var snapshot &&
            snapshot.Listening &&
            !string.Equals(snapshot.RemoteEndpoint, "-", StringComparison.Ordinal);

        if (!canArm)
        {
            armed = false;
        }

        if (_inputCaptureArmed == armed)
        {
            if (!armed)
            {
                if (sendReleaseAll)
                {
                    _session.SendRemoteReleaseAll(NextInputSequence());
                }

                _remoteKeysDown.Clear();
                _relativeMouseWarpPending = false;
                if (_relativeMouseCheck.Checked)
                {
                    Cursor.Show();
                }
            }

            UpdateInputCaptureStateLabel();
            return;
        }

        _inputCaptureArmed = armed;
        _remoteKeysDown.Clear();
        _relativeMouseWarpPending = false;

        _suppressControlEvents = true;
        try
        {
            _captureInputCheck.Checked = armed;
        }
        finally
        {
            _suppressControlEvents = false;
        }

        if (_inputCaptureArmed)
        {
            ActiveControl = _playbackHost;
            _playbackHost.Focus();
            if (_relativeMouseCheck.Checked)
            {
                Cursor.Hide();
                CenterRelativeMouseCursor();
            }
        }
        else
        {
            if (_relativeMouseCheck.Checked)
            {
                Cursor.Show();
            }

            if (sendReleaseAll)
            {
                _session.SendRemoteReleaseAll(NextInputSequence());
            }
        }

        UpdateInputCaptureStateLabel();
    }

    private void UpdateInputCaptureStateLabel()
    {
        if (_currentRole != AppRole.Receive)
        {
            _inputStateLabel.Text = "Input: sender role";
            return;
        }

        var snapshot = _session.GetSnapshot();
        if (!snapshot.Listening || string.Equals(snapshot.RemoteEndpoint, "-", StringComparison.Ordinal))
        {
            _inputStateLabel.Text = "Input: unavailable";
            return;
        }

        if (!_inputCaptureArmed)
        {
            _inputStateLabel.Text = "Input: disarmed";
            return;
        }

        _inputStateLabel.Text = _relativeMouseCheck.Checked
            ? "Input: armed (relative)"
            : "Input: armed (absolute)";
    }

    private long NextInputSequence() => Interlocked.Increment(ref _inputSequence);

    private void HandleRemoteKeyUp(Keys keyCode)
    {
        if (!_inputCaptureArmed || _currentRole != AppRole.Receive)
        {
            return;
        }

        keyCode &= Keys.KeyCode;
        if (_remoteKeysDown.Remove(keyCode))
        {
            _session.SendRemoteKey(NextInputSequence(), (int)keyCode, pressed: false);
        }
    }

    private void WirePlaybackInputHandlers(Control control)
    {
        control.MouseMove -= HandlePlaybackMouseMove;
        control.MouseMove += HandlePlaybackMouseMove;
        control.MouseDown -= HandlePlaybackMouseDown;
        control.MouseDown += HandlePlaybackMouseDown;
        control.MouseUp -= HandlePlaybackMouseUp;
        control.MouseUp += HandlePlaybackMouseUp;
        control.MouseWheel -= HandlePlaybackMouseWheel;
        control.MouseWheel += HandlePlaybackMouseWheel;

        foreach (Control child in control.Controls)
        {
            WirePlaybackInputHandlers(child);
        }
    }

    private static void WireScrollHostHandlers(Control control, ScrollableControl scrollHost)
    {
        control.MouseEnter -= HandleScrollHostMouseEnter;
        control.MouseEnter += HandleScrollHostMouseEnter;
        control.MouseWheel -= HandleScrollHostMouseWheel;
        control.MouseWheel += HandleScrollHostMouseWheel;

        foreach (Control child in control.Controls)
        {
            WireScrollHostHandlers(child, scrollHost);
        }

        return;

        void HandleScrollHostMouseEnter(object? sender, EventArgs args)
        {
            scrollHost.Focus();
        }

        void HandleScrollHostMouseWheel(object? sender, MouseEventArgs args)
        {
            if (scrollHost is not FlowLayoutPanel panel || !panel.AutoScroll)
            {
                return;
            }

            var nextValue = Math.Clamp(
                Math.Max(0, panel.VerticalScroll.Value - args.Delta),
                panel.VerticalScroll.Minimum,
                Math.Max(panel.VerticalScroll.Minimum, panel.VerticalScroll.Maximum - panel.ClientSize.Height));
            panel.AutoScrollPosition = new Point(0, nextValue);
        }
    }

    private void HandlePlaybackMouseMove(object? sender, MouseEventArgs args)
    {
        if (!_inputCaptureArmed || _currentRole != AppRole.Receive || sender is not Control sourceControl)
        {
            return;
        }

        if (_relativeMouseCheck.Checked)
        {
            if (_relativeMouseWarpPending)
            {
                _relativeMouseWarpPending = false;
                return;
            }

            var hostPoint = _playbackHost.PointToClient(sourceControl.PointToScreen(args.Location));
            var center = new Point(_playbackHost.ClientRectangle.Width / 2, _playbackHost.ClientRectangle.Height / 2);
            var dx = hostPoint.X - center.X;
            var dy = hostPoint.Y - center.Y;
            if (dx == 0 && dy == 0)
            {
                return;
            }

            _session.SendRemoteMouseMoveRelative(NextInputSequence(), dx, dy);
            CenterRelativeMouseCursor();
            return;
        }

        if (!TryMapPlaybackPointToNormalized(sourceControl, args.Location, out var normalizedX, out var normalizedY))
        {
            return;
        }

        _session.SendRemoteMouseMoveAbsolute(NextInputSequence(), normalizedX, normalizedY);
    }

    private void HandlePlaybackMouseDown(object? sender, MouseEventArgs args)
    {
        if (!_inputCaptureArmed || _currentRole != AppRole.Receive)
        {
            return;
        }

        ActiveControl = _playbackHost;
        _playbackHost.Focus();

        if (TryMapMouseButton(args.Button, out var button))
        {
            _session.SendRemoteMouseButton(NextInputSequence(), button, pressed: true);
        }
    }

    private void HandlePlaybackMouseUp(object? sender, MouseEventArgs args)
    {
        if (!_inputCaptureArmed || _currentRole != AppRole.Receive)
        {
            return;
        }

        if (TryMapMouseButton(args.Button, out var button))
        {
            _session.SendRemoteMouseButton(NextInputSequence(), button, pressed: false);
        }
    }

    private void HandlePlaybackMouseWheel(object? sender, MouseEventArgs args)
    {
        if (!_inputCaptureArmed || _currentRole != AppRole.Receive || args.Delta == 0)
        {
            return;
        }

        _session.SendRemoteMouseWheel(NextInputSequence(), args.Delta);
    }

    private bool TryMapPlaybackPointToNormalized(Control sourceControl, Point controlPoint, out double normalizedX, out double normalizedY)
    {
        normalizedX = 0.0;
        normalizedY = 0.0;

        var snapshot = _session.GetSnapshot();
        if (!TryParseResolution(snapshot.Resolution, out var videoWidth, out var videoHeight))
        {
            return false;
        }

        var hostClient = _playbackHost.ClientRectangle;
        if (hostClient.Width <= 0 || hostClient.Height <= 0)
        {
            return false;
        }

        var screenPoint = sourceControl.PointToScreen(controlPoint);
        var hostPoint = _playbackHost.PointToClient(screenPoint);

        var scale = Math.Min(hostClient.Width / (double)videoWidth, hostClient.Height / (double)videoHeight);
        if (scale <= 0.0)
        {
            return false;
        }

        var contentWidth = Math.Max(1, (int)Math.Round(videoWidth * scale));
        var contentHeight = Math.Max(1, (int)Math.Round(videoHeight * scale));
        var contentLeft = (hostClient.Width - contentWidth) / 2;
        var contentTop = (hostClient.Height - contentHeight) / 2;

        var clampedX = Math.Clamp(hostPoint.X, contentLeft, contentLeft + contentWidth - 1);
        var clampedY = Math.Clamp(hostPoint.Y, contentTop, contentTop + contentHeight - 1);

        normalizedX = (clampedX - contentLeft) / (double)Math.Max(1, contentWidth - 1);
        normalizedY = (clampedY - contentTop) / (double)Math.Max(1, contentHeight - 1);
        return true;
    }

    private void CenterRelativeMouseCursor()
    {
        if (!_inputCaptureArmed || !_relativeMouseCheck.Checked || !_playbackHost.IsHandleCreated)
        {
            return;
        }

        var clientRect = _playbackHost.ClientRectangle;
        if (clientRect.Width <= 0 || clientRect.Height <= 0)
        {
            return;
        }

        var center = new Point(clientRect.Width / 2, clientRect.Height / 2);
        _relativeMouseWarpPending = true;
        Cursor.Position = _playbackHost.PointToScreen(center);
    }

    private static bool TryMapMouseButton(MouseButtons button, out RemoteMouseButtonKind mappedButton)
    {
        mappedButton = button switch
        {
            MouseButtons.Right => RemoteMouseButtonKind.Right,
            MouseButtons.Middle => RemoteMouseButtonKind.Middle,
            MouseButtons.XButton1 => RemoteMouseButtonKind.X1,
            MouseButtons.XButton2 => RemoteMouseButtonKind.X2,
            _ => RemoteMouseButtonKind.Left,
        };

        return button is MouseButtons.Left or MouseButtons.Right or MouseButtons.Middle or MouseButtons.XButton1 or MouseButtons.XButton2;
    }

    private void RenderCurrentSnapshot()
    {
        RefreshControlPlaneAgentConfiguration();
        EnforceLeaseSenderConsistency();
        MaybeRunLeaseAutomation();
        MaybeRunManagedSessionSync();

        if (_currentRole == AppRole.Receive)
        {
            RenderSnapshot(_session.GetSnapshot());
            return;
        }

        RenderSenderSnapshot(_senderSession.GetSnapshot());
    }

    private void HandleControlPlaneAgentSnapshotChanged(ControlPlaneAgentSnapshot snapshot)
    {
        if (IsDisposed)
        {
            return;
        }

        if (InvokeRequired)
        {
            try
            {
                BeginInvoke((MethodInvoker)(() => HandleControlPlaneAgentSnapshotChanged(snapshot)));
            }
            catch
            {
            }

            return;
        }

        if (_currentRole == AppRole.Send)
        {
            if (!string.IsNullOrWhiteSpace(snapshot.LeaseSessionId) &&
                !string.Equals(snapshot.LeaseSessionId, "-", StringComparison.Ordinal))
            {
                ReceiverTrace.Log(
                    $"Lease snapshot delivered to host UI: role={_currentRole}; " +
                    $"auto={_leaseAutoRunCheck.Checked}; session={snapshot.LeaseSessionId}; " +
                    $"receiver={snapshot.LeaseReceiverEndpoint}; status={snapshot.LeaseStatus}; " +
                    $"registered={snapshot.LeaseReceiverRegistered}; ready={snapshot.LeaseHostReady}.");
            }
            MaybeRunLeaseAutomation();
            RenderSenderSnapshot(_senderSession.GetSnapshot());
            return;
        }

        RenderSnapshot(_session.GetSnapshot());
    }

    private void MaybeRunLeaseAutomation()
    {
        if (_currentRole != AppRole.Send)
        {
            TraceLeaseAutomationDecision($"skip: role={_currentRole}");
            return;
        }

        if (!_leaseAutoRunCheck.Checked)
        {
            TraceLeaseAutomationDecision("skip: auto-run disabled");
            return;
        }

        if (_leaseAutomationTask is { IsCompleted: false })
        {
            TraceLeaseAutomationDecision("skip: previous automation task still running");
            return;
        }

        var snapshot = _controlPlaneAgent.GetSnapshot();
        var leaseReady =
            string.Equals(snapshot.LeaseStatus, "Active", StringComparison.OrdinalIgnoreCase) &&
            !string.IsNullOrWhiteSpace(snapshot.LeaseSessionId) &&
            !string.Equals(snapshot.LeaseSessionId, "-", StringComparison.Ordinal) &&
            !string.IsNullOrWhiteSpace(snapshot.LeaseReceiverEndpoint) &&
            !string.Equals(snapshot.LeaseReceiverEndpoint, "-", StringComparison.Ordinal) &&
            snapshot.LeaseReceiverRegistered &&
            snapshot.LeaseHostReady;

        var senderSnapshot = _senderSession.GetSnapshot();
        if (!leaseReady)
        {
            if (senderSnapshot.Sending)
            {
                _leaseSuppressedSessionId = snapshot.LeaseSessionId;
                TraceLeaseAutomationDecision(
                    $"stop: lease cleared; sender={senderSnapshot.RemoteEndpoint}; session={_leaseDrivenSessionId ?? "-"}");
                _leaseDrivenSessionId = null;
                _leaseAutomationTask = HandleLeaseStopAsync();
            }
            else
            {
                TraceLeaseAutomationDecision(
                    $"skip: lease not ready; session={snapshot.LeaseSessionId}; receiver={snapshot.LeaseReceiverEndpoint}; status={snapshot.LeaseStatus}; registered={snapshot.LeaseReceiverRegistered}; ready={snapshot.LeaseHostReady}");
            }

            return;
        }

        if (senderSnapshot.Sending)
        {
            if (string.Equals(_leaseDrivenSessionId, snapshot.LeaseSessionId, StringComparison.Ordinal))
            {
                TraceLeaseAutomationDecision($"hold: lease-driven session={_leaseDrivenSessionId} still current");
                return;
            }

            TraceLeaseAutomationDecision(
                $"stop: sender running on {_leaseDrivenSessionId ?? "-"}; next={snapshot.LeaseSessionId}; ready={leaseReady}");
            _leaseAutomationTask = HandleLeaseChangeAsync(snapshot);
            return;
        }

        if (string.Equals(senderSnapshot.LastControlKind, "receiver_stop", StringComparison.Ordinal) &&
            string.Equals(_leaseDrivenSessionId, snapshot.LeaseSessionId, StringComparison.Ordinal))
        {
            _leaseSuppressedSessionId = snapshot.LeaseSessionId;
            _leaseDrivenSessionId = null;
            ScheduleControlPlaneAgentRestart();
            TraceLeaseAutomationDecision(
                $"skip: receiver_stop suppressed stale lease session={snapshot.LeaseSessionId} until control plane changes");
            ShowFriendlyStatus("Waiting for connection", ttlSeconds: 2);
            RenderCurrentSnapshot();
            return;
        }

        if (string.Equals(_leaseSuppressedSessionId, snapshot.LeaseSessionId, StringComparison.Ordinal))
        {
            TraceLeaseAutomationDecision($"skip: lease session={_leaseSuppressedSessionId} suppressed until a new session appears");
            return;
        }

        TraceLeaseAutomationDecision($"arm: session={snapshot.LeaseSessionId}; receiver={snapshot.LeaseReceiverEndpoint}");
        ReceiverTrace.Log(
            $"Lease auto-run armed for {snapshot.LeaseSessionId}; receiver={snapshot.LeaseReceiverEndpoint}; status={snapshot.LeaseStatus}; unattended={snapshot.LeaseUnattendedAuthorized}.");
        _leaseAutomationTask = HandleLeaseStartAsync(snapshot);
    }

    private async Task HandleLeaseStopAsync()
    {
        try
        {
            if (_senderSession.GetSnapshot().Sending)
            {
                await RunSessionMutationAsync("Stopping lease sender...", () => _senderSession.Stop());
                RestoreSenderWindowIfNeeded();
            }
            _leaseDrivenSessionId = null;
            _leaseSuppressedSessionId = null;
            ShowFriendlyStatus("Waiting for connection", ttlSeconds: 2);
            RenderCurrentSnapshot();
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Lease automation stop failed");
            _statusLabel.Text = $"Lease auto-stop failed: {ex.Message}";
        }
    }

    private void ScheduleControlPlaneAgentRestart()
    {
        _controlPlaneRestartCts?.Cancel();
        _controlPlaneRestartCts?.Dispose();
        var cts = new CancellationTokenSource();
        _controlPlaneRestartCts = cts;
        _ = Task.Run(async () =>
        {
            try
            {
                await Task.Delay(750, cts.Token);
                if (cts.IsCancellationRequested)
                {
                    return;
                }

                _controlPlaneAgent.RestartLoop();
            }
            catch (OperationCanceledException)
            {
            }
        });
    }

    private void TraceLeaseAutomationDecision(string decision)
    {
        if (string.Equals(_lastLeaseAutomationDecision, decision, StringComparison.Ordinal))
        {
            return;
        }

        _lastLeaseAutomationDecision = decision;
        ReceiverTrace.Log($"Lease automation {decision}.");
    }

    private void EnforceLeaseSenderConsistency()
    {
        if (_currentRole != AppRole.Send)
        {
            return;
        }

        var senderSnapshot = _senderSession.GetSnapshot();
        if (!senderSnapshot.Sending)
        {
            return;
        }

        var snapshot = _controlPlaneAgent.GetSnapshot();
        var leaseReady =
            string.Equals(snapshot.LeaseStatus, "Active", StringComparison.OrdinalIgnoreCase) &&
            !string.IsNullOrWhiteSpace(snapshot.LeaseSessionId) &&
            !string.Equals(snapshot.LeaseSessionId, "-", StringComparison.Ordinal) &&
            !string.IsNullOrWhiteSpace(snapshot.LeaseReceiverEndpoint) &&
            !string.Equals(snapshot.LeaseReceiverEndpoint, "-", StringComparison.Ordinal) &&
            snapshot.LeaseReceiverRegistered &&
            snapshot.LeaseHostReady;

        var leaseMatchesSender =
            leaseReady &&
            !string.IsNullOrWhiteSpace(_leaseDrivenSessionId) &&
            string.Equals(_leaseDrivenSessionId, snapshot.LeaseSessionId, StringComparison.Ordinal);

        if (leaseMatchesSender || _leaseAutomationTask is { IsCompleted: false })
        {
            return;
        }

        _leaseSuppressedSessionId = snapshot.LeaseSessionId;
        ReceiverTrace.Log(
            $"Lease watchdog stopping sender; active={_leaseDrivenSessionId ?? "-"}; " +
            $"snapshot={snapshot.LeaseSessionId}; status={snapshot.LeaseStatus}; " +
            $"registered={snapshot.LeaseReceiverRegistered}; ready={snapshot.LeaseHostReady}.");
        _leaseDrivenSessionId = null;
        _leaseAutomationTask = HandleLeaseStopAsync();
    }

    private void MaybeRunManagedSessionSync()
    {
        if (_currentRole != AppRole.Receive || !HasManagedClientSession)
        {
            return;
        }

        if (_managedSessionSyncTask is { IsCompleted: false })
        {
            return;
        }

        var nowMs = Environment.TickCount64;
        if (nowMs - _lastManagedSessionSyncAtMs < _managedSessionSyncDelayMs)
        {
            return;
        }

        _lastManagedSessionSyncAtMs = nowMs;
        _managedSessionSyncTask = RunManagedSessionSyncAsync();
    }

    private async Task RunManagedSessionSyncAsync()
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl) || !HasManagedClientSession)
        {
            return;
        }

        try
        {
            await _desktopControlPlaneClient.KeepAliveSessionAsync(baseUrl, _managedSessionId, _managedSessionToken);
            var connectInstructions = await _desktopControlPlaneClient.GetConnectInstructionsAsync(baseUrl, _managedSessionId, _managedSessionToken);
            if (connectInstructions.Status is "Stopped" or "Expired")
            {
                await StopManagedReceiverSessionAsync($"managed_session_{connectInstructions.Status.ToLowerInvariant()}", stopLocalReceiver: true);
                return;
            }
            if (connectInstructions.RouteVersion < _managedRouteVersion)
            {
                _statusLabel.Text = $"Managed sync ignored stale route version {connectInstructions.RouteVersion} < {_managedRouteVersion}.";
                return;
            }

            _managedRouteKind = connectInstructions.RouteKind;
            _managedRouteState = connectInstructions.RouteState;
            _managedRouteVersion = connectInstructions.RouteVersion;
            _managedSessionHealth = connectInstructions.SessionHealth;
            _managedSessionHealthReason = connectInstructions.SessionHealthReason;
            _managedRouteActionHint = connectInstructions.RouteActionHint;
            _managedRouteActionReason = connectInstructions.RouteActionReason;
            _managedRouteFallbackReadyDurationSeconds = connectInstructions.RouteFallbackReadyDurationSeconds;
            _managedRouteRecoveryReadyDurationSeconds = connectInstructions.RouteRecoveryReadyDurationSeconds;
            _managedRecommendedSyncDelaySeconds = Math.Clamp(connectInstructions.RecommendedSyncDelaySeconds, 5, 60);
            _managedTransportLossLevel = connectInstructions.TransportLossLevel;
            _managedTransportAnomalyKind = connectInstructions.TransportAnomalyKind;
            _managedTransportAnomalyReason = connectInstructions.TransportAnomalyReason;
            _managedTransportAnomalyConfidence = connectInstructions.TransportAnomalyConfidence;
            _managedReceiverTelemetryAgeSeconds = connectInstructions.ReceiverTelemetryAgeSeconds;
            _managedSenderTelemetryAgeSeconds = connectInstructions.SenderTelemetryAgeSeconds;
            _managedLastRouteActionKind = connectInstructions.LastRouteActionKind ?? "-";
            _managedLastRouteActionReason = connectInstructions.LastRouteActionReason ?? "-";
            _managedLastRouteActionActor = connectInstructions.LastRouteActionActor ?? "-";
            _managedLastRouteActionUtc = connectInstructions.LastRouteActionUtc?.ToString("u") ?? "-";
            _managedRouteRecoveryCount = connectInstructions.RouteRecoveryCount;
            _managedRouteRecoveryCooldownSeconds = connectInstructions.RouteRecoveryCooldownSeconds;
            _managedRouteFallbackCount = connectInstructions.RouteFallbackCount;
            _managedRouteFallbackCooldownSeconds = connectInstructions.RouteFallbackCooldownSeconds;
            _managedNatStatus = connectInstructions.NatStatus;
            _managedRelayEndpoint = connectInstructions.RelayHost is not null && connectInstructions.RelayPort is not null
                ? $"{connectInstructions.RelayHost}:{connectInstructions.RelayPort} ({connectInstructions.RelayRegion ?? "relay"})"
                : "-";
            _managedSessionSyncFailureCount = 0;
            _managedSessionHealthDegradedStreak = 0;
            _managedRouteFallbackArmed = true;
            _managedSessionSyncDelayMs = _managedRecommendedSyncDelaySeconds * 1000;

            var relayRegistrationRoute = TryBuildRelayRoute(
                sessionId: _managedSessionId,
                sessionToken: _managedSessionToken,
                relayHost: connectInstructions.RelayHost,
                relayPort: connectInstructions.RelayPort);
            var relayRoute = BuildManagedRelayRoute(
                connectInstructions.RouteKind,
                sessionId: _managedSessionId,
                sessionToken: _managedSessionToken,
                relayHost: connectInstructions.RelayHost,
                relayPort: connectInstructions.RelayPort);
            _session.ConfigureRelayRegistrationRoute(relayRegistrationRoute);
            _session.ConfigureRelayRoute(relayRoute);
            _desktopControlPlaneClient.SaveManagedSessionState(new DesktopControlPlaneManagedSessionState(
                BaseUrl: baseUrl,
                SessionId: _managedSessionId,
                SessionToken: _managedSessionToken,
                HostId: _managedHostId,
                HostDisplayName: _managedSessionHostLabel,
                RouteKind: connectInstructions.RouteKind,
                RouteState: connectInstructions.RouteState,
                RouteVersion: connectInstructions.RouteVersion,
                SessionHealth: connectInstructions.SessionHealth,
                SessionHealthReason: connectInstructions.SessionHealthReason,
                RouteActionHint: connectInstructions.RouteActionHint,
                RouteActionReason: connectInstructions.RouteActionReason,
                RouteFallbackReadyDurationSeconds: connectInstructions.RouteFallbackReadyDurationSeconds,
                RouteRecoveryReadyDurationSeconds: connectInstructions.RouteRecoveryReadyDurationSeconds,
                RecommendedSyncDelaySeconds: connectInstructions.RecommendedSyncDelaySeconds,
                TransportLossLevel: connectInstructions.TransportLossLevel,
                ReceiverTelemetryAgeSeconds: connectInstructions.ReceiverTelemetryAgeSeconds,
                SenderTelemetryAgeSeconds: connectInstructions.SenderTelemetryAgeSeconds,
                RouteRecoveryCount: connectInstructions.RouteRecoveryCount,
                RouteRecoveryCooldownSeconds: connectInstructions.RouteRecoveryCooldownSeconds,
                NatStatus: connectInstructions.NatStatus,
                HostNatProbeAgeSeconds: connectInstructions.HostNatProbeAgeSeconds,
                ClientNatProbeAgeSeconds: connectInstructions.ClientNatProbeAgeSeconds,
                NatProbeFresh: connectInstructions.NatProbeFresh,
                RouteFallbackCount: connectInstructions.RouteFallbackCount,
                RouteFallbackCooldownSeconds: connectInstructions.RouteFallbackCooldownSeconds,
                RelayAddress: connectInstructions.RelayHost,
                RelayPort: connectInstructions.RelayPort,
                ReceiverAddress: connectInstructions.StreamHost,
                ReceiverPort: connectInstructions.StreamPort,
                ProbeAddress: connectInstructions.ProbeHost,
                ProbePort: connectInstructions.ProbePort,
                ProbeToken: connectInstructions.ProbeToken,
                TransportAnomalyKind: connectInstructions.TransportAnomalyKind,
                TransportAnomalyReason: connectInstructions.TransportAnomalyReason,
                TransportAnomalyConfidence: connectInstructions.TransportAnomalyConfidence));
            _managedSessionHealthDegradedStreak = string.Equals(connectInstructions.SessionHealth, "degraded", StringComparison.OrdinalIgnoreCase)
                ? _managedSessionHealthDegradedStreak + 1
                : 0;
            _statusLabel.Text = $"Managed session synced for {_managedSessionHostLabel}. Route: {connectInstructions.RouteKind} ({connectInstructions.RouteState}), Health: {connectInstructions.SessionHealth} ({connectInstructions.SessionHealthReason}), Action: {connectInstructions.RouteActionHint} ({connectInstructions.RouteActionReason}), Sync: {connectInstructions.RecommendedSyncDelaySeconds}s, Loss: {connectInstructions.TransportLossLevel}, Telemetry age: {connectInstructions.ReceiverTelemetryAgeSeconds}s/{connectInstructions.SenderTelemetryAgeSeconds}s, Route ver: {connectInstructions.RouteVersion}, Fallbacks: {connectInstructions.RouteFallbackCount}, Cooldown: {connectInstructions.RouteFallbackCooldownSeconds}s, Degraded streak: {_managedSessionHealthDegradedStreak}. NAT: {connectInstructions.NatStatus}, probe age: {connectInstructions.HostNatProbeAgeSeconds}s/{connectInstructions.ClientNatProbeAgeSeconds}s, fresh: {connectInstructions.NatProbeFresh}.";
            RenderSnapshot(_session.GetSnapshot());
            var backendRecoveryRecommended = string.Equals(connectInstructions.RouteActionHint, "direct_recovery_recommended", StringComparison.OrdinalIgnoreCase);
            if (backendRecoveryRecommended && _managedRouteRecoveryCooldownSeconds <= 0)
            {
                try
                {
                    var recoveryInstructions = await _desktopControlPlaneClient.RecoverManagedSessionRouteAsync(baseUrl, _managedSessionId, _managedSessionToken, "health_recovered");
                    if (recoveryInstructions.RouteVersion >= _managedRouteVersion)
                    {
                        _managedRouteKind = recoveryInstructions.RouteKind;
                        _managedRouteState = recoveryInstructions.RouteState;
                        _managedRouteVersion = recoveryInstructions.RouteVersion;
                        _managedSessionHealth = recoveryInstructions.SessionHealth;
                        _managedSessionHealthReason = recoveryInstructions.SessionHealthReason;
                        _managedRouteActionHint = recoveryInstructions.RouteActionHint;
                        _managedRouteActionReason = recoveryInstructions.RouteActionReason;
                        _managedRouteFallbackReadyDurationSeconds = recoveryInstructions.RouteFallbackReadyDurationSeconds;
                        _managedRouteRecoveryReadyDurationSeconds = recoveryInstructions.RouteRecoveryReadyDurationSeconds;
                        _managedRecommendedSyncDelaySeconds = Math.Clamp(recoveryInstructions.RecommendedSyncDelaySeconds, 5, 60);
                        _managedTransportLossLevel = recoveryInstructions.TransportLossLevel;
                        _managedTransportAnomalyKind = recoveryInstructions.TransportAnomalyKind;
                        _managedTransportAnomalyReason = recoveryInstructions.TransportAnomalyReason;
                        _managedTransportAnomalyConfidence = recoveryInstructions.TransportAnomalyConfidence;
                        _managedReceiverTelemetryAgeSeconds = recoveryInstructions.ReceiverTelemetryAgeSeconds;
                        _managedSenderTelemetryAgeSeconds = recoveryInstructions.SenderTelemetryAgeSeconds;
                        _managedLastRouteActionKind = recoveryInstructions.LastRouteActionKind ?? "-";
                        _managedLastRouteActionReason = recoveryInstructions.LastRouteActionReason ?? "-";
                        _managedLastRouteActionActor = recoveryInstructions.LastRouteActionActor ?? "-";
                        _managedLastRouteActionUtc = recoveryInstructions.LastRouteActionUtc?.ToString("u") ?? "-";
                        _managedRouteRecoveryCount = recoveryInstructions.RouteRecoveryCount;
                        _managedRouteRecoveryCooldownSeconds = recoveryInstructions.RouteRecoveryCooldownSeconds;
                        _managedRouteFallbackCount = recoveryInstructions.RouteFallbackCount;
                        _managedRouteFallbackCooldownSeconds = recoveryInstructions.RouteFallbackCooldownSeconds;
                        _managedNatStatus = recoveryInstructions.NatStatus;
                        _managedHostNatProbeAgeSeconds = recoveryInstructions.HostNatProbeAgeSeconds;
                        _managedClientNatProbeAgeSeconds = recoveryInstructions.ClientNatProbeAgeSeconds;
                        _managedNatProbeFresh = recoveryInstructions.NatProbeFresh;
                        _managedRelayEndpoint = recoveryInstructions.RelayHost is not null && recoveryInstructions.RelayPort is not null
                            ? $"{recoveryInstructions.RelayHost}:{recoveryInstructions.RelayPort} ({recoveryInstructions.RelayRegion ?? "relay"})"
                            : "-";
                        _managedSessionSyncDelayMs = _managedRecommendedSyncDelaySeconds * 1000;
                        var recoveryRelayRegistrationRoute = TryBuildRelayRoute(
                            _managedSessionId,
                            _managedSessionToken,
                            recoveryInstructions.RelayHost,
                            recoveryInstructions.RelayPort);
                        var recoveryRelayRoute = BuildManagedRelayRoute(
                            recoveryInstructions.RouteKind,
                            _managedSessionId,
                            _managedSessionToken,
                            recoveryInstructions.RelayHost,
                            recoveryInstructions.RelayPort);
                        _session.ConfigureRelayRegistrationRoute(recoveryRelayRegistrationRoute);
                        _session.ConfigureRelayRoute(recoveryRelayRoute);
                        _desktopControlPlaneClient.SaveManagedSessionState(new DesktopControlPlaneManagedSessionState(
                            BaseUrl: baseUrl,
                            SessionId: _managedSessionId,
                            SessionToken: _managedSessionToken,
                            HostId: _managedHostId,
                            HostDisplayName: _managedSessionHostLabel,
                            RouteKind: recoveryInstructions.RouteKind,
                            RouteState: recoveryInstructions.RouteState,
                            RouteVersion: recoveryInstructions.RouteVersion,
                            SessionHealth: recoveryInstructions.SessionHealth,
                            SessionHealthReason: recoveryInstructions.SessionHealthReason,
                            RouteActionHint: recoveryInstructions.RouteActionHint,
                            RouteActionReason: recoveryInstructions.RouteActionReason,
                            RouteFallbackReadyDurationSeconds: recoveryInstructions.RouteFallbackReadyDurationSeconds,
                            RouteRecoveryReadyDurationSeconds: recoveryInstructions.RouteRecoveryReadyDurationSeconds,
                            RecommendedSyncDelaySeconds: recoveryInstructions.RecommendedSyncDelaySeconds,
                            TransportLossLevel: recoveryInstructions.TransportLossLevel,
                            ReceiverTelemetryAgeSeconds: recoveryInstructions.ReceiverTelemetryAgeSeconds,
                            SenderTelemetryAgeSeconds: recoveryInstructions.SenderTelemetryAgeSeconds,
                            RouteRecoveryCount: recoveryInstructions.RouteRecoveryCount,
                            RouteRecoveryCooldownSeconds: recoveryInstructions.RouteRecoveryCooldownSeconds,
                            RouteFallbackCount: recoveryInstructions.RouteFallbackCount,
                            RouteFallbackCooldownSeconds: recoveryInstructions.RouteFallbackCooldownSeconds,
                            NatStatus: recoveryInstructions.NatStatus,
                            HostNatProbeAgeSeconds: recoveryInstructions.HostNatProbeAgeSeconds,
                            ClientNatProbeAgeSeconds: recoveryInstructions.ClientNatProbeAgeSeconds,
                            NatProbeFresh: recoveryInstructions.NatProbeFresh,
                            RelayAddress: recoveryInstructions.RelayHost,
                            RelayPort: recoveryInstructions.RelayPort,
                            ReceiverAddress: recoveryInstructions.StreamHost,
                            ReceiverPort: recoveryInstructions.StreamPort,
                            ProbeAddress: recoveryInstructions.ProbeHost,
                            ProbePort: recoveryInstructions.ProbePort,
                            ProbeToken: recoveryInstructions.ProbeToken,
                            TransportAnomalyKind: recoveryInstructions.TransportAnomalyKind,
                            TransportAnomalyReason: recoveryInstructions.TransportAnomalyReason,
                            TransportAnomalyConfidence: recoveryInstructions.TransportAnomalyConfidence));
                        _statusLabel.Text = $"Managed session route recovery for {_managedSessionHostLabel}. Route: {recoveryInstructions.RouteKind} ({recoveryInstructions.RouteState}), Health: {recoveryInstructions.SessionHealth} ({recoveryInstructions.SessionHealthReason}), Action: {recoveryInstructions.RouteActionHint} ({recoveryInstructions.RouteActionReason}), Sync: {recoveryInstructions.RecommendedSyncDelaySeconds}s, Loss: {recoveryInstructions.TransportLossLevel}, Telemetry age: {recoveryInstructions.ReceiverTelemetryAgeSeconds}s/{recoveryInstructions.SenderTelemetryAgeSeconds}s, Recoveries: {recoveryInstructions.RouteRecoveryCount}, Recovery cooldown: {recoveryInstructions.RouteRecoveryCooldownSeconds}s.";
                    }
                }
                catch (Exception recoveryEx)
                {
                    _statusLabel.Text = $"Managed route recovery failed: {recoveryEx.Message}";
                }
            }
            var backendFallbackRecommended = string.Equals(connectInstructions.RouteActionHint, "fallback_recommended", StringComparison.OrdinalIgnoreCase);
            if ((backendFallbackRecommended || _managedSessionHealthDegradedStreak >= 2) && _managedRouteFallbackArmed && _managedRouteFallbackCooldownSeconds <= 0)
            {
                _managedRouteFallbackArmed = false;
                _managedSessionSyncFailureCount = 0;
                _managedSessionSyncDelayMs = 10_000;
                _ = Task.Run(async () =>
                {
                    try
                    {
                        var fallbackInstructions = await _desktopControlPlaneClient.FallbackManagedSessionRouteAsync(baseUrl, _managedSessionId, _managedSessionToken, "health_degraded");
                        if (fallbackInstructions.Status is "Stopped" or "Expired")
                        {
                            await StopManagedReceiverSessionAsync($"managed_session_{fallbackInstructions.Status.ToLowerInvariant()}", stopLocalReceiver: true);
                            return;
                        }
                        if (fallbackInstructions.RouteVersion < _managedRouteVersion)
                        {
                            _statusLabel.Text = $"Managed health fallback ignored stale route version {fallbackInstructions.RouteVersion} < {_managedRouteVersion}.";
                            return;
                        }

                        _managedRouteKind = fallbackInstructions.RouteKind;
                        _managedRouteState = fallbackInstructions.RouteState;
                        _managedRouteVersion = fallbackInstructions.RouteVersion;
                        _managedSessionHealth = fallbackInstructions.SessionHealth;
                        _managedSessionHealthReason = fallbackInstructions.SessionHealthReason;
                        _managedRouteActionHint = fallbackInstructions.RouteActionHint;
                        _managedRouteActionReason = fallbackInstructions.RouteActionReason;
                        _managedRouteFallbackReadyDurationSeconds = fallbackInstructions.RouteFallbackReadyDurationSeconds;
                        _managedRouteRecoveryReadyDurationSeconds = fallbackInstructions.RouteRecoveryReadyDurationSeconds;
                        _managedRecommendedSyncDelaySeconds = Math.Clamp(fallbackInstructions.RecommendedSyncDelaySeconds, 5, 60);
                        _managedTransportLossLevel = fallbackInstructions.TransportLossLevel;
                        _managedTransportAnomalyKind = fallbackInstructions.TransportAnomalyKind;
                        _managedTransportAnomalyReason = fallbackInstructions.TransportAnomalyReason;
                        _managedTransportAnomalyConfidence = fallbackInstructions.TransportAnomalyConfidence;
                        _managedReceiverTelemetryAgeSeconds = fallbackInstructions.ReceiverTelemetryAgeSeconds;
                        _managedSenderTelemetryAgeSeconds = fallbackInstructions.SenderTelemetryAgeSeconds;
                        _managedLastRouteActionKind = fallbackInstructions.LastRouteActionKind ?? "-";
                        _managedLastRouteActionReason = fallbackInstructions.LastRouteActionReason ?? "-";
                        _managedLastRouteActionActor = fallbackInstructions.LastRouteActionActor ?? "-";
                        _managedLastRouteActionUtc = fallbackInstructions.LastRouteActionUtc?.ToString("u") ?? "-";
                        _managedRouteRecoveryCount = fallbackInstructions.RouteRecoveryCount;
                        _managedRouteRecoveryCooldownSeconds = fallbackInstructions.RouteRecoveryCooldownSeconds;
                        _managedRouteFallbackCount = fallbackInstructions.RouteFallbackCount;
                        _managedRouteFallbackCooldownSeconds = fallbackInstructions.RouteFallbackCooldownSeconds;
                        _managedNatStatus = fallbackInstructions.NatStatus;
                        _managedHostNatProbeAgeSeconds = fallbackInstructions.HostNatProbeAgeSeconds;
                        _managedClientNatProbeAgeSeconds = fallbackInstructions.ClientNatProbeAgeSeconds;
                        _managedNatProbeFresh = fallbackInstructions.NatProbeFresh;
                        _managedRelayEndpoint = fallbackInstructions.RelayHost is not null && fallbackInstructions.RelayPort is not null
                            ? $"{fallbackInstructions.RelayHost}:{fallbackInstructions.RelayPort} ({fallbackInstructions.RelayRegion ?? "relay"})"
                            : "-";
                        var relayRegistrationRoute = TryBuildRelayRoute(
                            sessionId: _managedSessionId,
                            sessionToken: _managedSessionToken,
                            relayHost: fallbackInstructions.RelayHost,
                            relayPort: fallbackInstructions.RelayPort);
                        var relayRoute = BuildManagedRelayRoute(
                            fallbackInstructions.RouteKind,
                            sessionId: _managedSessionId,
                            sessionToken: _managedSessionToken,
                            relayHost: fallbackInstructions.RelayHost,
                            relayPort: fallbackInstructions.RelayPort);
                        _session.ConfigureRelayRegistrationRoute(relayRegistrationRoute);
                        _session.ConfigureRelayRoute(relayRoute);
                        _desktopControlPlaneClient.SaveManagedSessionState(new DesktopControlPlaneManagedSessionState(
                            BaseUrl: baseUrl,
                            SessionId: _managedSessionId,
                            SessionToken: _managedSessionToken,
                            HostId: _managedHostId,
                            HostDisplayName: _managedSessionHostLabel,
                            RouteKind: fallbackInstructions.RouteKind,
                            RouteState: fallbackInstructions.RouteState,
                            RouteVersion: fallbackInstructions.RouteVersion,
                            SessionHealth: fallbackInstructions.SessionHealth,
                            SessionHealthReason: fallbackInstructions.SessionHealthReason,
                            RouteActionHint: fallbackInstructions.RouteActionHint,
                            RouteActionReason: fallbackInstructions.RouteActionReason,
                            RouteFallbackReadyDurationSeconds: fallbackInstructions.RouteFallbackReadyDurationSeconds,
                            RouteRecoveryReadyDurationSeconds: fallbackInstructions.RouteRecoveryReadyDurationSeconds,
                            RecommendedSyncDelaySeconds: fallbackInstructions.RecommendedSyncDelaySeconds,
                            TransportLossLevel: fallbackInstructions.TransportLossLevel,
                            ReceiverTelemetryAgeSeconds: fallbackInstructions.ReceiverTelemetryAgeSeconds,
                            SenderTelemetryAgeSeconds: fallbackInstructions.SenderTelemetryAgeSeconds,
                            RouteRecoveryCount: fallbackInstructions.RouteRecoveryCount,
                            RouteRecoveryCooldownSeconds: fallbackInstructions.RouteRecoveryCooldownSeconds,
                            RouteFallbackCount: fallbackInstructions.RouteFallbackCount,
                            RouteFallbackCooldownSeconds: fallbackInstructions.RouteFallbackCooldownSeconds,
                            NatStatus: fallbackInstructions.NatStatus,
                            HostNatProbeAgeSeconds: fallbackInstructions.HostNatProbeAgeSeconds,
                            ClientNatProbeAgeSeconds: fallbackInstructions.ClientNatProbeAgeSeconds,
                            NatProbeFresh: fallbackInstructions.NatProbeFresh,
                            RelayAddress: fallbackInstructions.RelayHost,
                            RelayPort: fallbackInstructions.RelayPort,
                            ReceiverAddress: fallbackInstructions.StreamHost,
                            ReceiverPort: fallbackInstructions.StreamPort,
                            ProbeAddress: fallbackInstructions.ProbeHost,
                            ProbePort: fallbackInstructions.ProbePort,
                            ProbeToken: fallbackInstructions.ProbeToken,
                            TransportAnomalyKind: fallbackInstructions.TransportAnomalyKind,
                            TransportAnomalyReason: fallbackInstructions.TransportAnomalyReason,
                            TransportAnomalyConfidence: fallbackInstructions.TransportAnomalyConfidence));
                        _managedSessionSyncDelayMs = _managedRecommendedSyncDelaySeconds * 1000;
                        _statusLabel.Text = $"Managed session health fallback for {_managedSessionHostLabel}. Route: {fallbackInstructions.RouteKind} ({fallbackInstructions.RouteState}), Health: {fallbackInstructions.SessionHealth} ({fallbackInstructions.SessionHealthReason}), Action: {fallbackInstructions.RouteActionHint} ({fallbackInstructions.RouteActionReason}), Sync: {fallbackInstructions.RecommendedSyncDelaySeconds}s, Loss: {fallbackInstructions.TransportLossLevel}, Telemetry age: {fallbackInstructions.ReceiverTelemetryAgeSeconds}s/{fallbackInstructions.SenderTelemetryAgeSeconds}s, Route ver: {fallbackInstructions.RouteVersion}, Fallbacks: {fallbackInstructions.RouteFallbackCount}, Cooldown: {fallbackInstructions.RouteFallbackCooldownSeconds}s.";
                    }
                    catch (Exception fallbackHealthEx)
                    {
                        _statusLabel.Text = $"Managed health fallback failed: {fallbackHealthEx.Message}";
                    }
                    finally
                    {
                        _managedRouteFallbackArmed = true;
                    }
                });
            }
        }
        catch (Exception ex)
        {
            _managedSessionSyncFailureCount++;
            _managedSessionHealthDegradedStreak = 0;
            _managedSessionSyncDelayMs = ComputeManagedSessionSyncDelayMs(_managedSessionSyncFailureCount);
            if (_managedSessionSyncFailureCount >= 3 && _managedRouteFallbackArmed && _managedRouteFallbackCooldownSeconds <= 0)
            {
                _managedRouteFallbackArmed = false;
                try
                {
                    var connectInstructions = await _desktopControlPlaneClient.FallbackManagedSessionRouteAsync(baseUrl, _managedSessionId, _managedSessionToken);
                    if (connectInstructions.Status is "Stopped" or "Expired")
                    {
                        await StopManagedReceiverSessionAsync($"managed_session_{connectInstructions.Status.ToLowerInvariant()}", stopLocalReceiver: true);
                        return;
                    }
                    if (connectInstructions.RouteVersion < _managedRouteVersion)
                    {
                        _statusLabel.Text = $"Managed sync ignored stale route version {connectInstructions.RouteVersion} < {_managedRouteVersion}.";
                        return;
                    }

                    _managedRouteKind = connectInstructions.RouteKind;
                    _managedRouteState = connectInstructions.RouteState;
                    _managedRouteVersion = connectInstructions.RouteVersion;
                    _managedSessionHealth = connectInstructions.SessionHealth;
                    _managedSessionHealthReason = connectInstructions.SessionHealthReason;
                    _managedRouteActionHint = connectInstructions.RouteActionHint;
                    _managedRouteActionReason = connectInstructions.RouteActionReason;
                    _managedRouteFallbackReadyDurationSeconds = connectInstructions.RouteFallbackReadyDurationSeconds;
                    _managedRouteRecoveryReadyDurationSeconds = connectInstructions.RouteRecoveryReadyDurationSeconds;
                    _managedRecommendedSyncDelaySeconds = Math.Clamp(connectInstructions.RecommendedSyncDelaySeconds, 5, 60);
                    _managedTransportLossLevel = connectInstructions.TransportLossLevel;
                    _managedTransportAnomalyKind = connectInstructions.TransportAnomalyKind;
                    _managedTransportAnomalyReason = connectInstructions.TransportAnomalyReason;
                    _managedTransportAnomalyConfidence = connectInstructions.TransportAnomalyConfidence;
                    _managedReceiverTelemetryAgeSeconds = connectInstructions.ReceiverTelemetryAgeSeconds;
                    _managedSenderTelemetryAgeSeconds = connectInstructions.SenderTelemetryAgeSeconds;
                    _managedLastRouteActionKind = connectInstructions.LastRouteActionKind ?? "-";
                    _managedLastRouteActionReason = connectInstructions.LastRouteActionReason ?? "-";
                    _managedLastRouteActionActor = connectInstructions.LastRouteActionActor ?? "-";
                    _managedLastRouteActionUtc = connectInstructions.LastRouteActionUtc?.ToString("u") ?? "-";
                    _managedRouteRecoveryCount = connectInstructions.RouteRecoveryCount;
                    _managedRouteRecoveryCooldownSeconds = connectInstructions.RouteRecoveryCooldownSeconds;
                    _managedRouteFallbackCount = connectInstructions.RouteFallbackCount;
                    _managedRouteFallbackCooldownSeconds = connectInstructions.RouteFallbackCooldownSeconds;
                    _managedNatStatus = connectInstructions.NatStatus;
                    _managedHostNatProbeAgeSeconds = connectInstructions.HostNatProbeAgeSeconds;
                    _managedClientNatProbeAgeSeconds = connectInstructions.ClientNatProbeAgeSeconds;
                    _managedNatProbeFresh = connectInstructions.NatProbeFresh;
                    _managedRelayEndpoint = connectInstructions.RelayHost is not null && connectInstructions.RelayPort is not null
                        ? $"{connectInstructions.RelayHost}:{connectInstructions.RelayPort} ({connectInstructions.RelayRegion ?? "relay"})"
                        : "-";
                    _managedSessionSyncFailureCount = 0;
                    _managedRouteFallbackArmed = true;
                    _managedSessionSyncDelayMs = _managedRecommendedSyncDelaySeconds * 1000;

                    var relayRegistrationRoute = TryBuildRelayRoute(
                        sessionId: _managedSessionId,
                        sessionToken: _managedSessionToken,
                        relayHost: connectInstructions.RelayHost,
                        relayPort: connectInstructions.RelayPort);
                    var relayRoute = BuildManagedRelayRoute(
                        connectInstructions.RouteKind,
                        sessionId: _managedSessionId,
                        sessionToken: _managedSessionToken,
                        relayHost: connectInstructions.RelayHost,
                        relayPort: connectInstructions.RelayPort);
                    _session.ConfigureRelayRegistrationRoute(relayRegistrationRoute);
                    _session.ConfigureRelayRoute(relayRoute);
                    _desktopControlPlaneClient.SaveManagedSessionState(new DesktopControlPlaneManagedSessionState(
                        BaseUrl: baseUrl,
                        SessionId: _managedSessionId,
                        SessionToken: _managedSessionToken,
                        HostId: _managedHostId,
                        HostDisplayName: _managedSessionHostLabel,
                        RouteKind: connectInstructions.RouteKind,
                        RouteState: connectInstructions.RouteState,
                        RouteVersion: connectInstructions.RouteVersion,
                        SessionHealth: connectInstructions.SessionHealth,
                        SessionHealthReason: connectInstructions.SessionHealthReason,
                        RouteActionHint: connectInstructions.RouteActionHint,
                        RouteActionReason: connectInstructions.RouteActionReason,
                        RouteFallbackReadyDurationSeconds: connectInstructions.RouteFallbackReadyDurationSeconds,
                        RouteRecoveryReadyDurationSeconds: connectInstructions.RouteRecoveryReadyDurationSeconds,
                        RecommendedSyncDelaySeconds: connectInstructions.RecommendedSyncDelaySeconds,
                        TransportLossLevel: connectInstructions.TransportLossLevel,
                        ReceiverTelemetryAgeSeconds: connectInstructions.ReceiverTelemetryAgeSeconds,
                        SenderTelemetryAgeSeconds: connectInstructions.SenderTelemetryAgeSeconds,
                        RouteRecoveryCount: connectInstructions.RouteRecoveryCount,
                        RouteRecoveryCooldownSeconds: connectInstructions.RouteRecoveryCooldownSeconds,
                        RouteFallbackCount: connectInstructions.RouteFallbackCount,
                        RouteFallbackCooldownSeconds: connectInstructions.RouteFallbackCooldownSeconds,
                        NatStatus: connectInstructions.NatStatus,
                        HostNatProbeAgeSeconds: connectInstructions.HostNatProbeAgeSeconds,
                        ClientNatProbeAgeSeconds: connectInstructions.ClientNatProbeAgeSeconds,
                        NatProbeFresh: connectInstructions.NatProbeFresh,
                        RelayAddress: connectInstructions.RelayHost,
                        RelayPort: connectInstructions.RelayPort,
                        ReceiverAddress: connectInstructions.StreamHost,
                        ReceiverPort: connectInstructions.StreamPort,
                        ProbeAddress: connectInstructions.ProbeHost,
                        ProbePort: connectInstructions.ProbePort,
                        ProbeToken: connectInstructions.ProbeToken,
                        TransportAnomalyKind: connectInstructions.TransportAnomalyKind,
                        TransportAnomalyReason: connectInstructions.TransportAnomalyReason,
                        TransportAnomalyConfidence: connectInstructions.TransportAnomalyConfidence));
                    _statusLabel.Text = $"Managed session route fallback for {_managedSessionHostLabel}. Route: {connectInstructions.RouteKind} ({connectInstructions.RouteState}), Health: {connectInstructions.SessionHealth} ({connectInstructions.SessionHealthReason}), Action: {connectInstructions.RouteActionHint} ({connectInstructions.RouteActionReason}), Sync: {connectInstructions.RecommendedSyncDelaySeconds}s, Loss: {connectInstructions.TransportLossLevel}, Telemetry age: {connectInstructions.ReceiverTelemetryAgeSeconds}s/{connectInstructions.SenderTelemetryAgeSeconds}s, Route ver: {connectInstructions.RouteVersion}, Fallbacks: {connectInstructions.RouteFallbackCount}, Cooldown: {connectInstructions.RouteFallbackCooldownSeconds}s. NAT: {connectInstructions.NatStatus}, probe age: {connectInstructions.HostNatProbeAgeSeconds}s/{connectInstructions.ClientNatProbeAgeSeconds}s, fresh: {connectInstructions.NatProbeFresh}.";
                    RenderSnapshot(_session.GetSnapshot());
                    return;
                }
                catch (Exception fallbackEx)
                {
                    _statusLabel.Text = $"Managed sync retry failed: {ex.Message}; fallback failed: {fallbackEx.Message}";
                }
            }
            else
            {
                _statusLabel.Text = $"Managed sync failed ({_managedSessionSyncFailureCount}/3, retry in {_managedSessionSyncDelayMs / 1000}s): {ex.Message}";
            }
        }
    }

    private static int ComputeManagedSessionSyncDelayMs(int failureCount)
    {
        return failureCount switch
        {
            <= 0 => 10_000,
            1 => 10_000,
            2 => 20_000,
            3 => 40_000,
            _ => 60_000,
        };
    }

    private void RefreshControlPlaneAgentConfiguration()
    {
        var baseUrl = _controlPlaneUrlBox.Text.Trim();
        var enabled = _currentRole == AppRole.Send && !string.IsNullOrWhiteSpace(baseUrl);
        var directPort = int.TryParse(_sendPortBox.Text.Trim(), out var parsedPort) && parsedPort is > 0 and <= 65535
            ? parsedPort
            : 5001;
        var senderSnapshot = _senderSession.GetSnapshot();
        var senderCapabilities = WindowsSenderSession.GetSenderCapabilityProbe();

        _controlPlaneAgent.ApplyConfiguration(
            new ControlPlaneAgentConfiguration(
                Enabled: enabled,
                BaseUrl: baseUrl,
                DisplayName: Environment.MachineName,
                Region: string.IsNullOrWhiteSpace(_controlRegionBox.Text) ? "global" : _controlRegionBox.Text.Trim(),
                DirectPort: directPort,
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
                SupportsHevc: senderCapabilities.SupportsAdvertisedEncodeCodec(WindowsVideoCodec.H265Hevc),
                SupportsAudio: true,
                SupportsGamepad: true,
                EncoderBackends: senderCapabilities.SupportedBackends,
                Capabilities: BuildControlPlaneCapabilities()));
    }

    private static ControlPlaneHostCapabilities BuildControlPlaneCapabilities()
    {
        var senderCapabilities = WindowsSenderSession.GetSenderCapabilityProbe();
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
            MaxFps: 120,
            SupportedEncodeCodecs: senderCapabilities.SupportedEncodeCodecs,
            SupportedDecodeCodecs: GetDesktopSupportedDecodeCodecs(),
            SupportedEncoderBackends: senderCapabilities.SupportedBackends,
            LanAddresses: GetLanAddresses());
    }

    private static string[] GetDesktopSupportedDecodeCodecs()
    {
        var codecs = new List<string> { WindowsVideoCodec.H264Avc.ToMimeType(), WindowsVideoCodec.H265Hevc.ToMimeType() };
        if (WindowsSenderSession.GetSenderCapabilityProbe().SupportsCodec(WindowsVideoCodec.Av1))
        {
            codecs.Insert(0, WindowsVideoCodec.Av1.ToMimeType());
        }

        return codecs.Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
    }

    private string[] BuildManagedPreferredCodecs()
    {
        var supported = GetDesktopSupportedDecodeCodecs();
        var preferred = new[] { WindowsVideoCodec.H264Avc.ToMimeType(), WindowsVideoCodec.H265Hevc.ToMimeType() };

        return preferred
            .Where(codec => supported.Contains(codec, StringComparer.OrdinalIgnoreCase))
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .ToArray();
    }

    private static string[] GetLanAddresses()
    {
        try
        {
            return NetworkInterface.GetAllNetworkInterfaces()
                .Where(static nic => nic.OperationalStatus == OperationalStatus.Up && nic.NetworkInterfaceType != NetworkInterfaceType.Loopback)
                .SelectMany(static nic => nic.GetIPProperties().UnicastAddresses)
                .Select(static address => address.Address)
                .Where(static address => address.AddressFamily == AddressFamily.InterNetwork)
                .Select(static address => address.ToString())
                .Where(static address =>
                    address.StartsWith("10.", StringComparison.Ordinal) ||
                    address.StartsWith("192.168.", StringComparison.Ordinal) ||
                    address.StartsWith("172.16.", StringComparison.Ordinal) ||
                    address.StartsWith("172.17.", StringComparison.Ordinal) ||
                    address.StartsWith("172.18.", StringComparison.Ordinal) ||
                    address.StartsWith("172.19.", StringComparison.Ordinal) ||
                    address.StartsWith("172.20.", StringComparison.Ordinal) ||
                    address.StartsWith("172.21.", StringComparison.Ordinal) ||
                    address.StartsWith("172.22.", StringComparison.Ordinal) ||
                    address.StartsWith("172.23.", StringComparison.Ordinal) ||
                    address.StartsWith("172.24.", StringComparison.Ordinal) ||
                    address.StartsWith("172.25.", StringComparison.Ordinal) ||
                    address.StartsWith("172.26.", StringComparison.Ordinal) ||
                    address.StartsWith("172.27.", StringComparison.Ordinal) ||
                    address.StartsWith("172.28.", StringComparison.Ordinal) ||
                    address.StartsWith("172.29.", StringComparison.Ordinal) ||
                    address.StartsWith("172.30.", StringComparison.Ordinal) ||
                    address.StartsWith("172.31.", StringComparison.Ordinal))
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .ToArray();
        }
        catch
        {
            return Array.Empty<string>();
        }
    }

    private async Task HandleLeaseStartAsync(ControlPlaneAgentSnapshot agentSnapshot)
    {
        try
        {
            if (!TryParseLeaseReceiverEndpoint(agentSnapshot.LeaseReceiverEndpoint, out var receiverHost, out var receiverPort))
            {
                ReceiverTrace.Log($"Lease auto-run skipped: failed to parse receiver endpoint '{agentSnapshot.LeaseReceiverEndpoint}'.");
                return;
            }

            ReceiverTrace.Log(
                $"Lease auto-run starting sender for {agentSnapshot.LeaseSessionId}; " +
                $"receiver={receiverHost}:{receiverPort}; route={agentSnapshot.LeaseRouteKind}; " +
                $"status={agentSnapshot.LeaseStatus}; unattended={agentSnapshot.LeaseUnattendedAuthorized}.");

            _sendHostBox.Text = receiverHost;
            _sendPortBox.Text = receiverPort.ToString(CultureInfo.InvariantCulture);

            var encoderBackend = _sendEncoderBox.SelectedItem is WindowsSenderEncoderBackend selectedBackend
                ? selectedBackend
                : WindowsSenderEncoderBackend.NvidiaNvenc;
            var codec = TryResolveLeaseCodec(agentSnapshot.LeaseCodecPreference);
            var senderSpec = BuildSenderSpecFromLease(agentSnapshot);
            var audioEnabled = _sendAudioCheck.Checked;
            var captureCursor = agentSnapshot.LeaseCaptureCursor ?? _sendCursorCheck.Checked;
            var adaptiveEnabled = agentSnapshot.LeaseAdaptiveMode ?? _sendAdaptiveCheck.Checked;
            var hasRelayEndpoint = TryParseLeaseRelayEndpoint(agentSnapshot.LeaseRelayEndpoint, out var relayHost, out var relayPort);
            var relayRoute = ShouldUseRelayRoute(agentSnapshot.LeaseRouteKind)
                ? TryBuildRelayRoute(
                    sessionId: agentSnapshot.LeaseSessionId,
                    sessionToken: agentSnapshot.LeaseSessionToken,
                    relayHost: hasRelayEndpoint ? relayHost : null,
                    relayPort: hasRelayEndpoint ? relayPort : null)
                : null;

            await StartSenderCoreAsync(
                host: receiverHost,
                port: receiverPort,
                encoderBackend: encoderBackend,
                codec: codec,
                senderSpec: senderSpec,
                audioEnabled: audioEnabled,
                captureCursorInStream: captureCursor,
                adaptiveEnabled: adaptiveEnabled,
                relayRoute: relayRoute,
                busyStatus: $"Starting sender for lease {agentSnapshot.LeaseSessionId}...");

            _leaseDrivenSessionId = agentSnapshot.LeaseSessionId;
            AutoHideSenderWindow();
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Lease automation start failed");
            _statusLabel.Text = $"Lease auto-start failed: {ex.Message}";
        }
    }

    private async Task HandleLeaseChangeAsync(ControlPlaneAgentSnapshot agentSnapshot)
    {
        using var leaseAutomationCts = new CancellationTokenSource();
        _leaseAutomationCts = leaseAutomationCts;
        var oldSession = _leaseDrivenSessionId ?? "-";
        var newSession = agentSnapshot.LeaseSessionId ?? "-";
        ReceiverTrace.Log($"Lease session changed: stopping sender {oldSession} -> {newSession}.");

        try
        {
            try
            {
                if (_senderSession.GetSnapshot().Sending)
                {
                    await RunSessionMutationAsync("Stopping lease sender...", () => _senderSession.Stop());
                    RestoreSenderWindowIfNeeded();
                }
            }
            catch (Exception ex)
            {
                ReceiverTrace.Log(ex, "Lease automation stop failed");
                _statusLabel.Text = $"Lease auto-stop failed: {ex.Message}";
            }
            finally
            {
                _leaseDrivenSessionId = null;
            }

            var currentSnapshot = _controlPlaneAgent.GetSnapshot();
            if (leaseAutomationCts.IsCancellationRequested)
            {
                ReceiverTrace.Log("Lease automation change cancelled before restart.");
                return;
            }
            var leaseReady =
                !string.IsNullOrWhiteSpace(currentSnapshot.LeaseSessionId) &&
                !string.Equals(currentSnapshot.LeaseSessionId, "-", StringComparison.Ordinal) &&
                !string.IsNullOrWhiteSpace(currentSnapshot.LeaseReceiverEndpoint) &&
                !string.Equals(currentSnapshot.LeaseReceiverEndpoint, "-", StringComparison.Ordinal) &&
                string.Equals(currentSnapshot.LeaseStatus, "Active", StringComparison.OrdinalIgnoreCase) &&
                currentSnapshot.LeaseReceiverRegistered &&
                currentSnapshot.LeaseHostReady;

            ReceiverTrace.Log(
                $"[HOST] session={currentSnapshot.LeaseSessionId ?? "-"}; " +
                $"sender={(_senderSession.GetSnapshot().Sending ? "running" : "stopped")}; " +
                $"relay={(leaseReady ? "registered" : "not")}; " +
                $"receiverRegistered={currentSnapshot.LeaseReceiverRegistered}; hostReady={currentSnapshot.LeaseHostReady}.");

            if (!leaseReady || _leaseAutomationTask is { IsCompleted: false })
            {
                if (!leaseReady)
                {
                    ReceiverTrace.Log("Lease not ready after stop; new sender will not start.");
                }
                return;
            }

            _leaseSuppressedSessionId = null;
            await HandleLeaseStartAsync(currentSnapshot);
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Lease automation change failed; sender left stopped.");
            _statusLabel.Text = $"Lease automation change failed: {ex.Message}";
        }
        finally
        {
            if (ReferenceEquals(_leaseAutomationCts, leaseAutomationCts))
            {
                _leaseAutomationCts = null;
            }
        }
    }

    private WindowsSenderPresetSpec BuildSenderSpecFromLease(ControlPlaneAgentSnapshot agentSnapshot)
    {
        var preset = _sendPresetBox.SelectedItem is WindowsSenderPreset selectedPreset
            ? selectedPreset
            : WindowsSenderPreset.Game;
        var baseSpec = BuildSenderSpecFromUi(preset);
        var width = agentSnapshot.LeaseRequestedWidth > 0 ? agentSnapshot.LeaseRequestedWidth : baseSpec.TargetWidth;
        var height = agentSnapshot.LeaseRequestedHeight > 0 ? agentSnapshot.LeaseRequestedHeight : baseSpec.TargetHeight;
        var fps = agentSnapshot.LeaseRequestedFps > 0 ? agentSnapshot.LeaseRequestedFps : baseSpec.TargetFps;
        var bitrateBps = agentSnapshot.LeaseRequestedBitrateBps > 0 ? agentSnapshot.LeaseRequestedBitrateBps : baseSpec.TargetBitrateBps;

        if (width == baseSpec.TargetWidth &&
            height == baseSpec.TargetHeight &&
            fps == baseSpec.TargetFps &&
            bitrateBps == baseSpec.TargetBitrateBps)
        {
            return baseSpec;
        }

        return new WindowsSenderPresetSpec(
            UiLabel: $"Lease {width}x{height} @ {fps} / {bitrateBps / 1_000_000.0:0.0} Mbps",
            ProtocolPreset: baseSpec.ProtocolPreset,
            TargetWidth: width,
            TargetHeight: height,
            TargetFps: fps,
            TargetBitrateBps: bitrateBps,
            KeyFrameIntervalSeconds: Math.Max(1, baseSpec.KeyFrameIntervalSeconds));
    }

    private static WindowsVideoCodec TryResolveLeaseCodec(string codecPreference)
    {
        if (codecPreference.Contains("av1", StringComparison.OrdinalIgnoreCase))
        {
            return WindowsVideoCodec.Av1;
        }

        if (codecPreference.Contains("hevc", StringComparison.OrdinalIgnoreCase) ||
            codecPreference.Contains("h265", StringComparison.OrdinalIgnoreCase))
        {
            return WindowsVideoCodec.H265Hevc;
        }

        if (codecPreference.Contains("avc", StringComparison.OrdinalIgnoreCase) ||
            codecPreference.Contains("h264", StringComparison.OrdinalIgnoreCase))
        {
            return WindowsVideoCodec.H264Avc;
        }

        return WindowsVideoCodec.H265Hevc;
    }

    private static bool TryParseLeaseReceiverEndpoint(string displayText, out string host, out int port)
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

    private static bool TryParseLeaseRelayEndpoint(string displayText, out string host, out int port) =>
        TryParseLeaseReceiverEndpoint(displayText, out host, out port);

    private static bool ShouldUseRelayRoute(string? routeKind) =>
        !string.IsNullOrWhiteSpace(routeKind) &&
        routeKind.Contains("relay", StringComparison.OrdinalIgnoreCase);

    private static RelayTransportRoute? BuildManagedRelayRoute(string? routeKind, string sessionId, string sessionToken, string? relayHost, int? relayPort) =>
        ShouldUseRelayRoute(routeKind)
            ? TryBuildRelayRoute(sessionId, sessionToken, relayHost, relayPort)
            : null;

    private static string DescribeRouteKind(string? routeKind, string remoteEndpoint)
    {
        if (string.IsNullOrWhiteSpace(routeKind) || routeKind is "-")
        {
            return remoteEndpoint.Contains("relay", StringComparison.OrdinalIgnoreCase) ? "Relay" : "Direct";
        }

        return routeKind switch
        {
            "direct_host_push" => "Direct LAN",
            "direct_punched" => "Direct P2P",
            "direct_fallback" => "Direct fallback",
            "relay_assigned" => "Relay",
            _ => routeKind,
        };
    }

    private static RelayTransportRoute? TryBuildRelayRoute(string sessionId, string sessionToken, string? relayHost, int? relayPort)
    {
        if (string.IsNullOrWhiteSpace(sessionId) ||
            string.IsNullOrWhiteSpace(sessionToken) ||
            string.IsNullOrWhiteSpace(relayHost) ||
            relayPort is null or <= 0 or > 65535)
        {
            return null;
        }

        return new RelayTransportRoute(
            SessionId: sessionId.Trim(),
            SessionToken: sessionToken.Trim(),
            RelayHost: relayHost.Trim(),
            RelayPort: relayPort.Value);
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

    private async Task<bool> PrepareAdbShellCaptureAsync(bool showDialogOnFailure)
    {
        _prepareAdbButton.Enabled = false;
        _adbTunnelStatus = "Preparing ADB shell capture...";
        RenderSnapshot(_session.GetSnapshot());

        AdbTunnelResult result;
        try
        {
            result = await Task.Run(AdbTunnelManager.PrepareShellCapture);
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
                $"Failed to prepare ADB shell capture.{Environment.NewLine}{Environment.NewLine}{result.Message}",
                "ADB shell capture failed",
                MessageBoxButtons.OK,
                MessageBoxIcon.Error);
        }

        return result.Success;
    }

    private void StopSession()
    {
        ReceiverTrace.Log("Stop requested");
        if (_currentRole == AppRole.Receive && HasManagedClientSession)
        {
            _ = StopManagedReceiverSessionAsync("desktop_receiver_stop", stopLocalReceiver: true);
            return;
        }

        if (_currentRole == AppRole.Send)
        {
            _ = ResetHostToReadyAsync("desktop_host_stop");
            return;
        }

        BeginSessionAction(closeAfterCompletion: false);
    }

    private async Task ResetHostToReadyAsync(string reason)
    {
        try
        {
            ReceiverTrace.Log($"Reset host to Ready requested: {reason}");
            var stoppedLeaseSession = _leaseDrivenSessionId;
            _leaseSuppressedSessionId = stoppedLeaseSession;
            _leaseDrivenSessionId = null;
            await RunSessionMutationAsync("Returning host to Ready...", () => _senderSession.Stop());
            RestoreSenderWindowIfNeeded();
            if (!string.IsNullOrWhiteSpace(stoppedLeaseSession))
            {
                TraceLeaseAutomationDecision($"stop: host reset; suppressing lease session={stoppedLeaseSession} until a new session appears");
            }
            ShowFriendlyStatus("Waiting for connection", ttlSeconds: 2);
            RenderCurrentSnapshot();
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Reset host to Ready failed");
            _statusLabel.Text = $"Host reset failed: {ex.Message}";
        }
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

    private void AutoHideSenderWindow()
    {
        if (_senderAutoMinimized || WindowState == FormWindowState.Minimized)
        {
            return;
        }

        _senderRestoreWindowState = WindowState;
        _senderAutoMinimized = true;
        WindowState = FormWindowState.Minimized;
    }

    private void RestoreSenderWindowIfNeeded()
    {
        if (!_senderAutoMinimized)
        {
            return;
        }

        _senderAutoMinimized = false;
        WindowState = _senderRestoreWindowState == FormWindowState.Minimized
            ? FormWindowState.Normal
            : _senderRestoreWindowState;
    }

    private void RenderSnapshot(ReceiverSessionSnapshot snapshot)
    {
        if (_inputCaptureArmed && (!snapshot.Listening || string.Equals(snapshot.RemoteEndpoint, "-", StringComparison.Ordinal)))
        {
            SetRemoteInputCaptureArmed(false, sendReleaseAll: true);
        }

        _statusLabel.Text = ResolveFriendlyStatus(snapshot.Status);
        _heroDetailLabel.Text = string.Join(" · ", new[]
        {
            $"Transport: {snapshot.TransportMode}",
            $"Backend: {snapshot.PlaybackBackend}",
            $"Route: {DescribeRouteKind(_managedRouteKind, snapshot.RemoteEndpoint)}",
        });
        _heroPrimaryActionButton.Text = HasManagedClientSession ? "Disconnect" : "Connect";
        var nowMs = Environment.TickCount64;
        if (_tuningOverlay is not null && !_tuningOverlay.IsDisposed)
        {
            var tuningRefreshMs = snapshot.PlaybackStatus is "Playing" or "Opening" ? 900L : 250L;
            if (nowMs - _lastTuningRefreshAtMs >= tuningRefreshMs)
            {
                _tuningOverlay.UpdateSnapshot(snapshot);
                _lastTuningRefreshAtMs = nowMs;
            }
        }
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
                $"Control plane   : {(string.IsNullOrWhiteSpace(_controlPlaneUrlBox.Text) ? "-" : _controlPlaneUrlBox.Text.Trim())}",
                $"Control region  : {(string.IsNullOrWhiteSpace(_controlRegionBox.Text) ? "global" : _controlRegionBox.Text.Trim())}",
                $"Control auth    : {_desktopControlPlaneClient.GetAuthState().Label}",
                $"Managed host    : {_managedSessionHostLabel}",
                $"Host code       : {GetHostCode(_managedHostId)}",
                $"Managed session : {(HasManagedClientSession ? _managedSessionId : "-")}",
                $"Connect route   : {DescribeRouteKind(_managedRouteKind, snapshot.RemoteEndpoint)} ({_managedRouteState})",
                $"Session health  : {_managedSessionHealth} ({_managedSessionHealthReason})",
                $"Route action    : {_managedRouteActionHint} ({_managedRouteActionReason})",
                $"Action ready    : f={_managedRouteFallbackReadyDurationSeconds}s r={_managedRouteRecoveryReadyDurationSeconds}s",
                $"Sync cadence    : {_managedRecommendedSyncDelaySeconds}s",
                $"Loss level      : {_managedTransportLossLevel}",
                $"Transport anom. : {_managedTransportAnomalyKind} ({_managedTransportAnomalyConfidence})",
                $"Anomaly reason  : {_managedTransportAnomalyReason}",
                $"Telemetry age   : {_managedReceiverTelemetryAgeSeconds}s / {_managedSenderTelemetryAgeSeconds}s",
                $"Last route act. : {_managedLastRouteActionKind} by {_managedLastRouteActionActor}",
                $"Route act. info : {_managedLastRouteActionReason} @ {_managedLastRouteActionUtc}",
                $"Route recovery  : {_managedRouteRecoveryCount} / {_managedRouteRecoveryCooldownSeconds}s",
                $"Route fallback  : {_managedRouteFallbackCount} / {_managedRouteFallbackCooldownSeconds}s",
                $"Relay endpoint  : {_managedRelayEndpoint}",
                $"NAT status      : {_managedNatStatus}",
                $"NAT probe age   : {_managedHostNatProbeAgeSeconds}s / {_managedClientNatProbeAgeSeconds}s",
                $"NAT probe fresh : {_managedNatProbeFresh}",
                $"Managed sync    : {_managedSessionSyncFailureCount} fails / {_managedSessionSyncDelayMs / 1000}s",
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
                $"Pulse -> PC est : {(snapshot.PulseToPcEstimateMs >= 0 ? $"{snapshot.PulseToPcEstimateMs} ms" : "-")}",
                $"Tap -> PC est   : {(snapshot.TapToPcEstimateMs >= 0 ? $"{snapshot.TapToPcEstimateMs} ms" : "-")}",
                $"Adaptive jitter : {snapshot.AdaptiveJitterMs} ms",
                $"Catch-up ms     : {FormatManualValue(_manualCatchUpMs)}",
                $"IDR cooldown    : {FormatManualValue(_manualIdrCooldownMs)}",
                $"Panic queue AU  : {FormatManualValue(_manualPanicQueueAu)}",
                $"Feedback tick   : {FormatManualValue(_manualFeedbackTickMs)}",
                $"High delta ms   : {FormatManualValue(_manualHighDeltaMs)}",
                $"Critical delta  : {FormatManualValue(_manualCriticalDeltaMs)}",
                $"Startup grace   : {FormatManualValue(_manualStartupGraceMs)}",
                $"Drop burst step : {FormatManualValue(_manualDropBurstStep)}",
                $"Queue           : {snapshot.StreamQueuedAccessUnits} AU / {snapshot.StreamQueuedKilobytes} KB",
                $"Enh queue       : {snapshot.EnhancementQueuedAccessUnits} AU / {snapshot.EnhancementQueuedKilobytes} KB",
                $"Queue drops     : {snapshot.StreamDroppedAccessUnits}",
                $"Waiting keyframe: {snapshot.WaitingForKeyFrame}",
                $"ROI active      : {snapshot.RoiActive}",
                $"ROI rect        : {snapshot.RoiRect}",
                $"Ultra low mode  : {snapshot.UltraLowLatencyMode}",
                $"System hints    : {snapshot.SystemHintsEnabled}",
                $"Remote endpoint : {snapshot.RemoteEndpoint}",
                $"Log file        : {ReceiverTrace.LogFilePath}",
            });

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

        if (_hudBox.Focused)
        {
            _pendingHudText = hudText;
            return;
        }

        _hudBox.Text = hudText;
        _lastHudText = hudText;
        _pendingHudText = null;
        _lastHudRefreshAtMs = nowMs;
    }

    private void RenderSenderSnapshot(WindowsSenderSessionSnapshot snapshot)
    {
        var agentSnapshot = _controlPlaneAgent.GetSnapshot();
        _statusLabel.Text = ResolveFriendlyStatus(snapshot.Status);
        _heroDetailLabel.Text = string.Join(" · ", new[]
        {
            $"Code: {GetHostCode(agentSnapshot.HostId)}",
            $"Encoder: {snapshot.AutoEncoderSelected}",
            $"Codec: {snapshot.Codec}",
            $"Route: {DescribeRouteKind(agentSnapshot.LeaseRouteKind, snapshot.RemoteEndpoint)}",
        });
        _heroPrimaryActionButton.Text = snapshot.Sending ? "Stop Hosting" : "Start Hosting";
        var leaseActive = string.Equals(agentSnapshot.LeaseStatus, "Active", StringComparison.OrdinalIgnoreCase) &&
                          !string.IsNullOrWhiteSpace(agentSnapshot.LeaseSessionId) &&
                          !string.Equals(agentSnapshot.LeaseSessionId, "-", StringComparison.Ordinal) &&
                          !string.IsNullOrWhiteSpace(agentSnapshot.LeaseReceiverEndpoint) &&
                          !string.Equals(agentSnapshot.LeaseReceiverEndpoint, "-", StringComparison.Ordinal) &&
                          agentSnapshot.LeaseReceiverRegistered &&
                          agentSnapshot.LeaseHostReady;
        if (!leaseActive && snapshot.Sending)
        {
            snapshot = snapshot with
            {
                Status = "Idle",
                Preset = "-",
                EncoderPath = "-",
                CaptureTarget = "-",
                Codec = "-",
                Resolution = "-",
                TargetFps = 0,
                BitrateMbps = 0,
                RemoteEndpoint = "-",
                AudioStatus = "-",
                GamepadStatus = "-",
                GamepadInput = "-",
                ReceiverPressure = "-",
                ReceiverDecodeFps = 0,
                ReceiverQueueDrops = 0,
                ReceiverDecodeDeltaMs = -1,
                ReceiverPresentDeltaMs = -1,
                PulseToAndroidEstimateMs = -1,
                InputToAndroidEstimateMs = -1,
                AdaptiveEnabled = false,
                AdaptiveStep = 0,
                LastEncoderError = "-",
            };
        }
        var hudText = string.Join(
            Environment.NewLine,
            new[]
            {
                "Everty Native Sender",
                "Windows PC-to-PC low-latency experiment",
                $"Started         : {_startedAtLabel}",
                $"Build           : {_buildLabel}",
                string.Empty,
                $"Role            : Send",
                $"State           : {snapshot.Status}",
                $"Pulse -> Android: {FormatLatencyMetric(snapshot.PulseToAndroidEstimateMs, snapshot.ReceiverFeedbackAgeMs)}",
                $"Input -> Android: {FormatLatencyMetric(snapshot.InputToAndroidEstimateMs, snapshot.ReceiverFeedbackAgeMs)}",
                $"Preset          : {snapshot.Preset}",
                $"Latency mode    : {(snapshot.Preset.Contains("Game", StringComparison.OrdinalIgnoreCase) ? "Low latency" : "Balanced")}",
                $"Auto encoder    : {snapshot.AutoEncoderSelected}",
                $"Selected codec  : {snapshot.Codec}",
                $"Selected route  : {DescribeRouteKind(agentSnapshot.LeaseRouteKind, snapshot.RemoteEndpoint)}",
                $"Encoder path    : {snapshot.EncoderPath}",
                $"Capture target  : {snapshot.CaptureTarget}",
                $"Remote endpoint : {snapshot.RemoteEndpoint}",
                $"Control plane   : {agentSnapshot.Status}",
                $"Host registration: {agentSnapshot.HostId}",
                $"Host code       : {GetHostCode(agentSnapshot.HostId)}",
                $"Lease status    : {agentSnapshot.LeaseStatus}",
                $"Lease session   : {agentSnapshot.LeaseSessionId}",
                $"Lease client    : {agentSnapshot.LeaseClientLabel}",
                $"Lease receiver  : {agentSnapshot.LeaseReceiverEndpoint}",
                $"Lease route     : {agentSnapshot.LeaseRouteKind}",
                $"Lease relay     : {agentSnapshot.LeaseRelayEndpoint}",
                $"Lease probe     : {agentSnapshot.LeaseProbeEndpoint}",
                $"Lease NAT       : {agentSnapshot.LeaseNatStatus}",
                $"Lease receiver ok: {(agentSnapshot.LeaseReceiverRegistered ? "yes" : "no")}",
                $"Lease host ready : {(agentSnapshot.LeaseHostReady ? "yes" : "no")}",
                $"Lease codec     : {agentSnapshot.LeaseCodecPreference}",
                $"Lease profile   : {(agentSnapshot.LeaseRequestedWidth > 0 && agentSnapshot.LeaseRequestedHeight > 0 ? $"{agentSnapshot.LeaseRequestedWidth}x{agentSnapshot.LeaseRequestedHeight}" : "-")} / {(agentSnapshot.LeaseRequestedFps > 0 ? $"{agentSnapshot.LeaseRequestedFps} fps" : "-")} / {(agentSnapshot.LeaseRequestedBitrateBps > 0 ? $"{agentSnapshot.LeaseRequestedBitrateBps / 1_000_000.0:0.0} Mbps" : "-")}",
                $"Lease unattended: {(agentSnapshot.LeaseUnattendedAuthorized ? "authorized" : "blocked")}",
                $"Lease expires   : {(agentSnapshot.LeaseExpiresUtc is null ? "-" : agentSnapshot.LeaseExpiresUtc.Value.ToLocalTime().ToString("yyyy-MM-dd HH:mm:ss"))}",
                $"Codec           : {snapshot.Codec}",
                $"Resolution      : {snapshot.Resolution}",
                $"Target FPS      : {snapshot.TargetFps}",
                $"Bitrate         : {(snapshot.BitrateMbps > 0 ? $"{snapshot.BitrateMbps:0.0} Mbps" : "-")}",
                $"Packets sent    : {snapshot.PacketsSent}",
                $"Session packets : {snapshot.SessionConfigPackets}",
                $"Codec cfg pkt   : {snapshot.CodecConfigPackets}",
                $"Video packets   : {snapshot.VideoPackets}",
                $"Audio packets   : {snapshot.AudioPackets}",
                $"Control tx      : {snapshot.ControlPacketsSent}",
                $"Control rx      : {snapshot.ControlPacketsReceived}",
                $"Frames captured : {snapshot.FramesCaptured}",
                $"Frames encoded  : {snapshot.FramesEncoded}",
                $"Frames dropped  : {snapshot.FramesDropped}",
                $"Capture FPS     : {snapshot.CaptureFps}",
                $"Submit FPS      : {snapshot.SubmitFps}",
                $"Encode FPS      : {snapshot.EncodeFps}",
                $"Native stages   : {snapshot.NativeStageStats}",
                $"DXGI timeouts   : {snapshot.NativeDxgiTimeouts}",
                $"Paced skips     : {snapshot.NativePacedSkips}",
                $"Audio status    : {snapshot.AudioStatus}",
                $"Gamepad status  : {snapshot.GamepadStatus}",
                $"Gamepad input   : {snapshot.GamepadInput}",
                $"Adaptive        : {(snapshot.AdaptiveEnabled ? $"on / step {snapshot.AdaptiveStep}" : "off")}",
                $"Receiver decode : {snapshot.ReceiverDecodeFps}",
                $"Receiver press. : {snapshot.ReceiverPressure}",
                $"Receiver drops  : {snapshot.ReceiverQueueDrops}",
                $"Receiver d/p ms : {(snapshot.ReceiverDecodeDeltaMs >= 0 || snapshot.ReceiverPresentDeltaMs >= 0 ? $"{snapshot.ReceiverDecodeDeltaMs} / {snapshot.ReceiverPresentDeltaMs}" : "-")}",
                $"Feedback age ms : {(snapshot.ReceiverFeedbackAgeMs >= 0 ? snapshot.ReceiverFeedbackAgeMs.ToString(CultureInfo.InvariantCulture) : "-")}",
                $"Last control    : {snapshot.LastControlKind}",
                $"Encoder error   : {snapshot.LastEncoderError}",
                $"Log file        : {ReceiverTrace.LogFilePath}",
            });

        if (_hudBox.Focused)
        {
            _pendingHudText = hudText;
            return;
        }

        _hudBox.Text = hudText;
        _lastHudText = hudText;
        _pendingHudText = null;
        _lastHudRefreshAtMs = Environment.TickCount64;
    }

    private void FlushDeferredHudText()
    {
        if (string.IsNullOrWhiteSpace(_pendingHudText))
        {
            return;
        }

        _hudBox.Text = _pendingHudText;
        _lastHudText = _pendingHudText;
        _pendingHudText = null;
        _lastHudRefreshAtMs = Environment.TickCount64;
    }

    private void CopyHudToClipboard()
    {
        var hudText = !string.IsNullOrWhiteSpace(_pendingHudText) ? _pendingHudText! : _lastHudText;
        if (string.IsNullOrWhiteSpace(hudText))
        {
            return;
        }

        try
        {
            Clipboard.SetText(hudText);
            _statusLabel.Text = "HUD copied";
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Copy HUD failed");
            _statusLabel.Text = "HUD copy failed. See log.";
        }
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

    private static void ConfigureSection(TableLayoutPanel section, string title, params Control[] controls)
    {
        section.SuspendLayout();
        section.Controls.Clear();
        section.ColumnStyles.Clear();
        section.RowStyles.Clear();
        section.ColumnCount = 1;
        section.RowCount = 2;
        section.Margin = new Padding(10, 0, 10, 10);
        section.Padding = new Padding(12, 10, 12, 12);

        var titleLabel = new Label
        {
            AutoSize = true,
            Text = title,
            ForeColor = ForegroundColor,
            Font = new Font("Segoe UI Semibold", 10.5f, FontStyle.Regular, GraphicsUnit.Point),
            Margin = new Padding(0, 0, 0, 8),
        };
        var body = new FlowLayoutPanel
        {
            Dock = DockStyle.Top,
            AutoSize = true,
            AutoSizeMode = AutoSizeMode.GrowAndShrink,
            WrapContents = true,
            FlowDirection = FlowDirection.LeftToRight,
            BackColor = Color.Transparent,
            Margin = Padding.Empty,
            Padding = Padding.Empty,
        };
        body.Controls.AddRange(controls);

        section.Controls.Add(titleLabel, 0, 0);
        section.Controls.Add(body, 0, 1);
        section.ResumeLayout(performLayout: true);
    }

    private void ApplyTheme()
    {
        ForeColor = ForegroundColor;
        _statusLabel.ForeColor = MutedForegroundColor;
        _statusLabel.BackColor = SurfaceColor;
        _statusLabel.Font = new Font("Segoe UI Semibold", 10f, FontStyle.Regular, GraphicsUnit.Point);
        _senderOverlayLabel.BackColor = Color.FromArgb(18, 18, 20);
        _senderOverlayLabel.ForeColor = MutedForegroundColor;
        _senderOverlayLabel.Font = new Font("Segoe UI Semibold", 18f, FontStyle.Regular, GraphicsUnit.Point);

        _roleBox.BackColor = SurfaceColor;
        _roleBox.ForeColor = ForegroundColor;
        _roleBox.FlatStyle = FlatStyle.Popup;
        _roleBox.Margin = new Padding(4, 5, 10, 0);

        _portBox.BackColor = SurfaceColor;
        _portBox.ForeColor = ForegroundColor;
        _portBox.BorderStyle = BorderStyle.FixedSingle;
        _portBox.TextAlign = HorizontalAlignment.Center;
        _portBox.Margin = new Padding(4, 5, 10, 0);

        _controlRegionBox.BackColor = SurfaceColor;
        _controlRegionBox.ForeColor = ForegroundColor;
        _controlRegionBox.BorderStyle = BorderStyle.FixedSingle;
        _controlRegionBox.TextAlign = HorizontalAlignment.Center;
        _controlRegionBox.Margin = new Padding(4, 5, 10, 0);

        _controlUserEmailBox.BackColor = SurfaceColor;
        _controlUserEmailBox.ForeColor = ForegroundColor;
        _controlUserEmailBox.BorderStyle = BorderStyle.FixedSingle;
        _controlUserEmailBox.Margin = new Padding(4, 5, 10, 0);

        _controlUserPasswordBox.BackColor = SurfaceColor;
        _controlUserPasswordBox.ForeColor = ForegroundColor;
        _controlUserPasswordBox.BorderStyle = BorderStyle.FixedSingle;
        _controlUserPasswordBox.Margin = new Padding(4, 5, 10, 0);

        _managedHostBox.BackColor = SurfaceColor;
        _managedHostBox.ForeColor = ForegroundColor;
        _managedHostBox.FlatStyle = FlatStyle.Popup;
        _managedHostBox.Width = 340;
        _managedHostBox.Margin = new Padding(4, 5, 10, 0);

        _managedHostCodeBox.BackColor = SurfaceColor;
        _managedHostCodeBox.ForeColor = ForegroundColor;
        _managedHostCodeBox.BorderStyle = BorderStyle.FixedSingle;
        _managedHostCodeBox.TextAlign = HorizontalAlignment.Center;
        _managedHostCodeBox.Font = new Font("Segoe UI Semibold", 12f, FontStyle.Regular, GraphicsUnit.Point);
        _managedHostCodeBox.Margin = new Padding(4, 5, 10, 0);

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

        _sendTargetBox.BackColor = SurfaceColor;
        _sendTargetBox.ForeColor = ForegroundColor;
        _sendTargetBox.FlatStyle = FlatStyle.Popup;
        _sendTargetBox.Margin = new Padding(4, 5, 10, 0);

        _sendPresetBox.BackColor = SurfaceColor;
        _sendPresetBox.ForeColor = ForegroundColor;
        _sendPresetBox.FlatStyle = FlatStyle.Popup;
        _sendPresetBox.Margin = new Padding(4, 5, 10, 0);

        _sendEncoderBox.BackColor = SurfaceColor;
        _sendEncoderBox.ForeColor = ForegroundColor;
        _sendEncoderBox.FlatStyle = FlatStyle.Popup;
        _sendEncoderBox.Margin = new Padding(4, 5, 10, 0);

        _sendCodecBox.BackColor = SurfaceColor;
        _sendCodecBox.ForeColor = ForegroundColor;
        _sendCodecBox.FlatStyle = FlatStyle.Popup;
        _sendCodecBox.Margin = new Padding(4, 5, 10, 0);

        _sendWidthBox.BackColor = SurfaceColor;
        _sendWidthBox.ForeColor = ForegroundColor;
        _sendWidthBox.BorderStyle = BorderStyle.FixedSingle;
        _sendWidthBox.TextAlign = HorizontalAlignment.Center;
        _sendWidthBox.Margin = new Padding(4, 5, 10, 0);

        _sendHeightBox.BackColor = SurfaceColor;
        _sendHeightBox.ForeColor = ForegroundColor;
        _sendHeightBox.BorderStyle = BorderStyle.FixedSingle;
        _sendHeightBox.TextAlign = HorizontalAlignment.Center;
        _sendHeightBox.Margin = new Padding(4, 5, 10, 0);

        _sendFpsBox.BackColor = SurfaceColor;
        _sendFpsBox.ForeColor = ForegroundColor;
        _sendFpsBox.BorderStyle = BorderStyle.FixedSingle;
        _sendFpsBox.TextAlign = HorizontalAlignment.Center;
        _sendFpsBox.Margin = new Padding(4, 5, 10, 0);

        _sendBitrateBox.BackColor = SurfaceColor;
        _sendBitrateBox.ForeColor = ForegroundColor;
        _sendBitrateBox.BorderStyle = BorderStyle.FixedSingle;
        _sendBitrateBox.TextAlign = HorizontalAlignment.Center;
        _sendBitrateBox.Margin = new Padding(4, 5, 10, 0);

        _brandTitleLabel.ForeColor = ForegroundColor;
        _brandTitleLabel.Font = new Font("Segoe UI Semibold", 20f, FontStyle.Regular, GraphicsUnit.Point);
        _brandSubtitleLabel.ForeColor = MutedForegroundColor;
        _brandSubtitleLabel.Font = new Font("Segoe UI", 10.5f, FontStyle.Regular, GraphicsUnit.Point);

        foreach (var section in new[] { _commonSection, _clientQuickSection, _clientSettingsSection, _hostSection })
        {
            section.BackColor = SurfaceColor;
        }

        _sendAudioCheck.ForeColor = ForegroundColor;
        _sendAudioCheck.BackColor = WindowColor;
        _sendAudioCheck.Margin = new Padding(6, 7, 12, 0);

        _managedPreferHevcCheck.ForeColor = ForegroundColor;
        _managedPreferHevcCheck.BackColor = WindowColor;
        _managedPreferHevcCheck.Margin = new Padding(6, 7, 12, 0);

        _managedPreferRelayCheck.ForeColor = ForegroundColor;
        _managedPreferRelayCheck.BackColor = WindowColor;
        _managedPreferRelayCheck.Margin = new Padding(6, 7, 12, 0);

        _managedRequestAudioCheck.ForeColor = ForegroundColor;
        _managedRequestAudioCheck.BackColor = WindowColor;
        _managedRequestAudioCheck.Margin = new Padding(6, 7, 12, 0);

        StyleButton(_sendDiscoverButton);
        _sendDiscoverButton.Margin = new Padding(0, 4, 12, 0);
        StyleButton(_controlDemoAdminButton);
        StyleButton(_controlDemoTestButton);
        StyleButton(_controlUserLoginButton);
        StyleButton(_controlUserRegisterButton);
        StyleButton(_managedRefreshHostsButton);
        StyleButton(_managedStartByCodeButton);
        StyleButton(_managedStartSessionButton);
        StyleButton(_managedStopSessionButton);
        StyleButton(_managedCloseSessionButton);
        StyleHeroButton(_managedStartByCodeButton, AccentColor, Color.White);
        StyleHeroButton(_managedStartSessionButton, Color.FromArgb(53, 134, 255), Color.White);
        StyleHeroButton(_managedStopSessionButton, Color.FromArgb(122, 50, 50), Color.White);

        _sendCursorCheck.ForeColor = ForegroundColor;
        _sendCursorCheck.BackColor = WindowColor;
        _sendCursorCheck.Margin = new Padding(6, 7, 12, 0);

        _sendPulseFlashCheck.ForeColor = ForegroundColor;
        _sendPulseFlashCheck.BackColor = WindowColor;
        _sendPulseFlashCheck.Margin = new Padding(6, 7, 12, 0);

        _sendAdaptiveCheck.ForeColor = ForegroundColor;
        _sendAdaptiveCheck.BackColor = WindowColor;
        _sendAdaptiveCheck.Margin = new Padding(6, 7, 12, 0);

        _leaseAutoRunCheck.ForeColor = ForegroundColor;
        _leaseAutoRunCheck.BackColor = WindowColor;
        _leaseAutoRunCheck.Margin = new Padding(6, 7, 12, 0);

        _advancedModeCheck.ForeColor = ForegroundColor;
        _advancedModeCheck.BackColor = WindowColor;
        _advancedModeCheck.Margin = new Padding(6, 7, 12, 0);

        _simpleModeHintLabel.ForeColor = MutedForegroundColor;
        _simpleModeHintLabel.BackColor = WindowColor;
        _simpleModeHintLabel.Margin = new Padding(6, 9, 16, 0);

        _sendHostBox.BackColor = SurfaceColor;
        _sendHostBox.ForeColor = ForegroundColor;
        _sendHostBox.BorderStyle = BorderStyle.FixedSingle;
        _sendHostBox.Margin = new Padding(4, 5, 10, 0);

        _sendPortBox.BackColor = SurfaceColor;
        _sendPortBox.ForeColor = ForegroundColor;
        _sendPortBox.BorderStyle = BorderStyle.FixedSingle;
        _sendPortBox.TextAlign = HorizontalAlignment.Center;
        _sendPortBox.Margin = new Padding(4, 5, 10, 0);

        _controlPlaneUrlBox.BackColor = SurfaceColor;
        _controlPlaneUrlBox.ForeColor = ForegroundColor;
        _controlPlaneUrlBox.BorderStyle = BorderStyle.FixedSingle;
        _controlPlaneUrlBox.Margin = new Padding(4, 5, 10, 0);

        _aggressiveTailDropCheck.ForeColor = ForegroundColor;
        _aggressiveTailDropCheck.BackColor = WindowColor;
        _aggressiveTailDropCheck.Margin = new Padding(10, 7, 12, 0);

        _ultraLowLatencyCheck.ForeColor = ForegroundColor;
        _ultraLowLatencyCheck.BackColor = WindowColor;
        _ultraLowLatencyCheck.Margin = new Padding(10, 7, 12, 0);

        _captureInputCheck.ForeColor = ForegroundColor;
        _captureInputCheck.BackColor = WindowColor;
        _captureInputCheck.Margin = new Padding(10, 7, 12, 0);

        _relativeMouseCheck.ForeColor = ForegroundColor;
        _relativeMouseCheck.BackColor = WindowColor;
        _relativeMouseCheck.Margin = new Padding(6, 7, 12, 0);

        _inputStateLabel.ForeColor = MutedForegroundColor;
        _inputStateLabel.BackColor = WindowColor;
        _inputStateLabel.Margin = new Padding(6, 9, 12, 0);

        StyleButton(_startButton);
        StyleButton(_stopButton);
        StyleButton(_startSendingButton);
        StyleButton(_stopSendingButton);
        StyleButton(_fullscreenButton);
        StyleButton(_prepareAdbButton);
        StyleButton(_tuningButton);
        StyleButton(_copyHudButton);
        StyleHeroButton(_heroPrimaryActionButton, AccentColor, Color.White);
        StyleButton(_diagnosticsToggleButton);
        _diagnosticsToggleButton.Size = new Size(106, 30);

        _hudBox.BackColor = Color.FromArgb(19, 20, 24);
        _hudBox.ForeColor = ForegroundColor;
        _heroDetailLabel.ForeColor = MutedForegroundColor;
        _heroDetailLabel.Font = new Font("Segoe UI", 10.5f, FontStyle.Regular, GraphicsUnit.Point);
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

    private static void StyleHeroButton(Button button, Color backColor, Color foreColor)
    {
        button.Size = new Size(146, 38);
        button.Font = new Font("Segoe UI Semibold", 10.5f, FontStyle.Regular, GraphicsUnit.Point);
        button.BackColor = backColor;
        button.ForeColor = foreColor;
        button.FlatAppearance.BorderColor = ControlPaint.Light(backColor);
        button.Margin = new Padding(6, 2, 0, 0);
    }

    private void BeginSessionAction(bool closeAfterCompletion)
    {
        ReceiverTrace.Log(closeAfterCompletion ? "Begin close session action" : "Begin stop session action");
        _closeAfterSessionAction = closeAfterCompletion;
        if (_sessionActionTask is { IsCompleted: false })
        {
            ReceiverTrace.Log("Session action already running; request coalesced");
            return;
        }

        _hudTimer.Stop();
        SetControlsEnabled(false);
        _statusLabel.Text = closeAfterCompletion ? "Closing..." : "Stopping...";

        _sessionActionTask = Task.Run(() =>
        {
            _sessionMutationGate.Wait();
            try
            {
                ReceiverTrace.Log(_closeAfterSessionAction ? "Disposing receiver session" : "Stopping receiver session");
                if (_closeAfterSessionAction)
                {
                    _senderSession.Dispose();
                    _session.Dispose();
                }
                else if (_currentRole == AppRole.Send)
                {
                    _leaseDrivenSessionId = null;
                    _senderSession.Stop();
                }
                else
                {
                    _session.Stop();
                }
            }
            finally
            {
                _sessionMutationGate.Release();
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

        ReceiverTrace.Log(_closeAfterSessionAction ? "Session action completed: close" : "Session action completed: stop");
        _sessionActionTask = null;

        if (_closeAfterSessionAction)
        {
            _tuningOverlay?.Close();
            _tuningOverlay = null;
            _allowClose = true;
            _closeAfterSessionAction = false;
            Close();
            return;
        }

        _closeAfterSessionAction = false;
        SetControlsEnabled(true);
        _startButton.Enabled = true;
        _stopButton.Enabled = false;
        _hudTimer.Start();
        if (_currentRole == AppRole.Send)
        {
            RestoreSenderWindowIfNeeded();
            ShowFriendlyStatus("Waiting for connection", ttlSeconds: 2);
            RenderCurrentSnapshot();
        }
        else
        {
            RenderSnapshot(_session.GetSnapshot());
        }
        SyncUiControls();
    }

    private void SetControlsEnabled(bool enabled)
    {
        _roleBox.Enabled = enabled && !_lockRoleSelection;
        _transportBox.Enabled = enabled;
        _portBox.Enabled = enabled;
        _backendBox.Enabled = enabled;
        _decoderBox.Enabled = enabled;
        _ultraLowLatencyCheck.Enabled = enabled;
        _aggressiveTailDropCheck.Enabled = enabled;
        _captureInputCheck.Enabled = enabled;
        _relativeMouseCheck.Enabled = enabled && _captureInputCheck.Checked;
        _prepareAdbButton.Enabled = enabled;
        _sendTargetBox.Enabled = enabled;
        _sendHostBox.Enabled = enabled;
        _sendPortBox.Enabled = enabled;
        _controlPlaneUrlBox.Enabled = enabled;
        _controlRegionBox.Enabled = enabled;
        _controlUserEmailBox.Enabled = enabled;
        _controlUserPasswordBox.Enabled = enabled;
        _controlUserLoginButton.Enabled = enabled;
        _controlUserRegisterButton.Enabled = enabled;
        _managedHostBox.Enabled = enabled;
        _managedHostCodeBox.Enabled = enabled;
        _sendPresetBox.Enabled = enabled;
        _sendEncoderBox.Enabled = enabled;
        _sendCodecBox.Enabled = enabled;
        _sendWidthBox.Enabled = enabled;
        _sendHeightBox.Enabled = enabled;
        _sendFpsBox.Enabled = enabled;
        _sendBitrateBox.Enabled = enabled;
        _sendAudioCheck.Enabled = enabled;
        _managedRefreshHostsButton.Enabled = enabled;
        _managedStartByCodeButton.Enabled = enabled;
        _managedResumeSessionButton.Enabled = enabled && !string.IsNullOrWhiteSpace(_controlPlaneUrlBox.Text) &&
            _desktopControlPlaneClient.GetManagedSessionState(_controlPlaneUrlBox.Text) is not null;
        _managedPreferHevcCheck.Enabled = enabled;
        _managedPreferRelayCheck.Enabled = enabled;
        _managedRequestAudioCheck.Enabled = enabled;
        _managedStartSessionButton.Enabled = enabled;
        _managedStopSessionButton.Enabled = enabled && HasManagedClientSession;
        _managedCloseSessionButton.Enabled = enabled && HasManagedClientSession;
        _sendDiscoverButton.Enabled = enabled;
        _sendCursorCheck.Enabled = enabled;
        _sendPulseFlashCheck.Enabled = enabled;
        _sendAdaptiveCheck.Enabled = enabled;
        _leaseAutoRunCheck.Enabled = enabled;
        _startSendingButton.Enabled = enabled;
        _stopSendingButton.Enabled = enabled;
        _fullscreenButton.Enabled = enabled;
        _tuningButton.Enabled = enabled;
        _copyHudButton.Enabled = true;
        _startButton.Enabled = enabled;
        _stopButton.Enabled = enabled && !_startButton.Enabled;
        UpdateRoleUi();
        RefreshManagedClientUiState();
    }

    private void ApplySenderPresetTemplate(WindowsSenderPreset preset)
    {
        ApplySenderSpecToFields(preset.ToSpec());
    }

    private void ApplySenderSpecToFields(WindowsSenderPresetSpec spec)
    {
        _sendWidthBox.Text = spec.TargetWidth.ToString(CultureInfo.InvariantCulture);
        _sendHeightBox.Text = spec.TargetHeight.ToString(CultureInfo.InvariantCulture);
        _sendFpsBox.Text = spec.TargetFps.ToString(CultureInfo.InvariantCulture);
        _sendBitrateBox.Text = (spec.TargetBitrateBps / 1_000_000.0).ToString("0.0", CultureInfo.CurrentCulture);
    }

    private WindowsSenderPresetSpec BuildSenderSpecFromUi(WindowsSenderPreset preset)
    {
        var presetSpec = preset.ToSpec();
        var width = ParseSenderIntSetting(_sendWidthBox.Text, "Width", minValue: 64, maxValue: 7680, evenRequired: true);
        var height = ParseSenderIntSetting(_sendHeightBox.Text, "Height", minValue: 64, maxValue: 4320, evenRequired: true);
        var fps = ParseSenderIntSetting(_sendFpsBox.Text, "FPS", minValue: 1, maxValue: 240, evenRequired: false);
        var bitrateMbps = ParseSenderBitrateMbps(_sendBitrateBox.Text);
        var bitrateBps = (int)Math.Round(bitrateMbps * 1_000_000.0);

        var custom =
            width != presetSpec.TargetWidth ||
            height != presetSpec.TargetHeight ||
            fps != presetSpec.TargetFps ||
            bitrateBps != presetSpec.TargetBitrateBps;

        if (!custom)
        {
            return presetSpec;
        }

        var label = string.Format(
            CultureInfo.CurrentCulture,
            "Custom {0}x{1} @ {2} / {3:0.0} Mbps",
            width,
            height,
            fps,
            bitrateMbps);

        return new WindowsSenderPresetSpec(
            UiLabel: label,
            ProtocolPreset: presetSpec.ProtocolPreset,
            TargetWidth: width,
            TargetHeight: height,
            TargetFps: fps,
            TargetBitrateBps: bitrateBps,
            KeyFrameIntervalSeconds: Math.Max(1, presetSpec.KeyFrameIntervalSeconds));
    }

    private static int ParseSenderIntSetting(string text, string fieldName, int minValue, int maxValue, bool evenRequired)
    {
        if (!int.TryParse(text.Trim(), NumberStyles.Integer, CultureInfo.InvariantCulture, out var value) &&
            !int.TryParse(text.Trim(), NumberStyles.Integer, CultureInfo.CurrentCulture, out value))
        {
            throw new InvalidOperationException($"{fieldName} must be a whole number.");
        }

        if (value < minValue || value > maxValue)
        {
            throw new InvalidOperationException($"{fieldName} must be between {minValue} and {maxValue}.");
        }

        if (evenRequired && (value & 1) != 0)
        {
            throw new InvalidOperationException($"{fieldName} must be an even number for H.264 encoding.");
        }

        return value;
    }

    private static double ParseSenderBitrateMbps(string text)
    {
        var valueText = text.Trim();
        if (!double.TryParse(valueText, NumberStyles.Float, CultureInfo.CurrentCulture, out var bitrateMbps) &&
            !double.TryParse(valueText, NumberStyles.Float, CultureInfo.InvariantCulture, out bitrateMbps))
        {
            throw new InvalidOperationException("Bitrate must be a number in Mbps, for example 12 or 16.5.");
        }

        if (bitrateMbps is < 0.2 or > 200)
        {
            throw new InvalidOperationException("Bitrate must be between 0.2 and 200 Mbps.");
        }

        return bitrateMbps;
    }

    private async Task RunSessionMutationAsync(string busyStatus, Action action)
    {
        if (IsDisposed)
        {
            return;
        }

        ReceiverTrace.Log($"Heavy mutation begin: {busyStatus}");
        SetControlsEnabled(false);
        _statusLabel.Text = busyStatus;

        Exception? error = null;
        await _sessionMutationGate.WaitAsync();
        try
        {
            await Task.Run(action);
        }
        catch (Exception ex)
        {
            error = ex;
        }
        finally
        {
            _sessionMutationGate.Release();
        }

        if (IsDisposed)
        {
            return;
        }

        var receiverSnapshot = _session.GetSnapshot();
        var senderSnapshot = _senderSession.GetSnapshot();
        if (error is not null)
        {
            ReceiverTrace.Log(error, $"Heavy mutation failed: {busyStatus}");
            MessageBox.Show(this, error.Message, "Receiver operation failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
        else
        {
            ReceiverTrace.Log($"Heavy mutation applied: {busyStatus}");
        }

        RenderCurrentSnapshot();
        SyncUiControls();
        SetControlsEnabled(true);
        _startButton.Enabled = !receiverSnapshot.Listening;
        _stopButton.Enabled = receiverSnapshot.Listening;
        _startSendingButton.Enabled = !senderSnapshot.Sending;
        _stopSendingButton.Enabled = senderSnapshot.Sending;
        UpdateRoleUi();
    }

    private async Task RunLightweightSessionMutationAsync(string operationName, Action action)
    {
        if (IsDisposed)
        {
            return;
        }

        ReceiverTrace.Log($"Light mutation begin: {operationName}");
        Exception? error = null;
        var startedAt = Stopwatch.GetTimestamp();
        try
        {
            await Task.Run(action);
        }
        catch (Exception ex)
        {
            error = ex;
        }

        if (IsDisposed)
        {
            return;
        }

        var elapsedMs = (Stopwatch.GetElapsedTime(startedAt).TotalMilliseconds);

        if (error is not null)
        {
            ReceiverTrace.Log(error, $"Light mutation failed: {operationName}");
            _statusLabel.Text = $"{operationName} failed. See log.";
        }
        else
        {
            ReceiverTrace.Log($"Light mutation applied: {operationName} ({elapsedMs:0.0} ms)");
        }

        RenderSnapshot(_session.GetSnapshot());
        SyncUiControls();
    }

    private void SyncUiControls()
    {
        _suppressControlEvents = true;
        try
        {
            _tuningOverlay?.SyncState(
                _backendBox.SelectedItem is PlaybackBackendKind backend ? backend : PlaybackBackendKind.MediaFoundationDirect3D11,
                _decoderBox.SelectedItem is HardwareDecodeMode decoder ? decoder : HardwareDecodeMode.Auto,
                _ultraLowLatencyCheck.Checked,
                _aggressiveTailDropCheck.Checked,
                _manualJitterMs,
                _manualAudioBufferMs,
                _manualPacingMinTenthsMs,
                _manualPacingMaxTenthsMs,
                _manualCatchUpMs,
                _manualIdrCooldownMs,
                _manualPanicQueueAu,
                _manualFeedbackTickMs,
                _manualHighDeltaMs,
                _manualCriticalDeltaMs,
                _manualStartupGraceMs,
                _manualDropBurstStep);
        }
        finally
        {
            _suppressControlEvents = false;
        }
    }

    private void ToggleTuningOverlay()
    {
        if (_tuningOverlay is not null && !_tuningOverlay.IsDisposed)
        {
            ReceiverTrace.Log("Closing tuning overlay");
            _tuningOverlay.Close();
            _tuningOverlay = null;
            return;
        }

        ReceiverTrace.Log("Opening tuning overlay");
        var overlay = new TuningOverlayForm();
        overlay.Location = new Point(Right - overlay.Width - 24, Top + 96);
        overlay.SyncState(
            _backendBox.SelectedItem is PlaybackBackendKind backend ? backend : PlaybackBackendKind.MediaFoundationDirect3D11,
            _decoderBox.SelectedItem is HardwareDecodeMode decoder ? decoder : HardwareDecodeMode.Auto,
            _ultraLowLatencyCheck.Checked,
            _aggressiveTailDropCheck.Checked,
            _manualJitterMs,
            _manualAudioBufferMs,
            _manualPacingMinTenthsMs,
            _manualPacingMaxTenthsMs,
            _manualCatchUpMs,
            _manualIdrCooldownMs,
            _manualPanicQueueAu,
            _manualFeedbackTickMs,
            _manualHighDeltaMs,
            _manualCriticalDeltaMs,
            _manualStartupGraceMs,
            _manualDropBurstStep);
        overlay.UpdateSnapshot(_session.GetSnapshot());
        overlay.BackendChanged += backend =>
        {
            if (_backendBox.SelectedItem is PlaybackBackendKind current && current == backend)
            {
                return;
            }
            _backendBox.SelectedItem = backend;
        };
        overlay.DecoderChanged += decoder =>
        {
            if (_decoderBox.SelectedItem is HardwareDecodeMode current && current == decoder)
            {
                return;
            }
            _decoderBox.SelectedItem = decoder;
        };
        overlay.UltraLowLatencyChanged += enabled => _ultraLowLatencyCheck.Checked = enabled;
        overlay.AggressiveTailDropChanged += enabled => _aggressiveTailDropCheck.Checked = enabled;
        overlay.JitterChanged += value =>
        {
            _manualJitterMs = value;
            _ = RunLightweightSessionMutationAsync("Updating jitter", () => _session.UpdateAdaptiveJitterOverride(value));
        };
        overlay.AudioBufferChanged += value =>
        {
            _manualAudioBufferMs = value;
            _ = RunLightweightSessionMutationAsync("Updating audio buffer", () => _session.UpdateAudioBufferMs(value));
        };
        overlay.PacingChanged += (minTenths, maxTenths) =>
        {
            _manualPacingMinTenthsMs = minTenths;
            _manualPacingMaxTenthsMs = maxTenths;
            _ = RunLightweightSessionMutationAsync("Updating pacing", () => _session.UpdatePacingWindowMs(
                (int)Math.Round(minTenths / 10.0),
                (int)Math.Round(maxTenths / 10.0)));
        };
        overlay.CatchUpChanged += value =>
        {
            _manualCatchUpMs = value;
            _ = RunLightweightSessionMutationAsync("Updating catch-up threshold", () => _session.UpdateCatchUpThresholdMs(value));
        };
        overlay.IdrCooldownChanged += value =>
        {
            _manualIdrCooldownMs = value;
            _ = RunLightweightSessionMutationAsync("Updating IDR cooldown", () => _session.UpdateKeyFrameCooldownMs(value));
        };
        overlay.PanicQueueChanged += value =>
        {
            _manualPanicQueueAu = value;
            _ = RunLightweightSessionMutationAsync("Updating panic queue", () => _session.UpdatePanicQueueThresholdAu(value));
        };
        overlay.FeedbackTickChanged += value =>
        {
            _manualFeedbackTickMs = value;
            _ = RunLightweightSessionMutationAsync("Updating feedback tick", () => _session.UpdateFeedbackTickMs(value));
        };
        overlay.HighDeltaChanged += value =>
        {
            _manualHighDeltaMs = value;
            _ = RunLightweightSessionMutationAsync("Updating high delta", () => _session.UpdateHighDeltaThresholdMs(value));
        };
        overlay.CriticalDeltaChanged += value =>
        {
            _manualCriticalDeltaMs = value;
            _ = RunLightweightSessionMutationAsync("Updating critical delta", () => _session.UpdateCriticalDeltaThresholdMs(value));
        };
        overlay.StartupGraceChanged += value =>
        {
            _manualStartupGraceMs = value;
            _ = RunLightweightSessionMutationAsync("Updating startup grace", () => _session.UpdateStartupGraceMs(value));
        };
        overlay.DropBurstChanged += value =>
        {
            _manualDropBurstStep = value;
            _ = RunLightweightSessionMutationAsync("Updating drop burst", () => _session.UpdateDropBurstStep(value));
        };
        overlay.GamePresetRequested += () => ApplyReceiverPreset(ultraLowLatency: true, aggressiveTailDrop: true);
        overlay.BalancedPresetRequested += () => ApplyReceiverPreset(ultraLowLatency: true, aggressiveTailDrop: false);
        overlay.CinemaPresetRequested += () => ApplyReceiverPreset(ultraLowLatency: false, aggressiveTailDrop: false);
        overlay.DefaultsRequested += () =>
        {
            _manualJitterMs = 0;
            _manualAudioBufferMs = 0;
            _manualPacingMinTenthsMs = 0;
            _manualPacingMaxTenthsMs = 0;
            _manualCatchUpMs = 0;
            _manualIdrCooldownMs = 0;
            _manualPanicQueueAu = 0;
            _manualFeedbackTickMs = 0;
            _manualHighDeltaMs = 0;
            _manualCriticalDeltaMs = 0;
            _manualStartupGraceMs = 0;
            _manualDropBurstStep = 0;
            SyncUiControls();
            _ = RunLightweightSessionMutationAsync("Resetting tuning defaults", () => _session.ResetManualTuningOverrides());
        };
        overlay.FormClosed += (_, _) =>
        {
            if (ReferenceEquals(_tuningOverlay, overlay))
            {
                _tuningOverlay = null;
            }
        };

        _tuningOverlay = overlay;
        overlay.Show();
        overlay.BringToFront();
    }

    private void ToggleDiagnosticsDrawer()
    {
        _diagnosticsDrawerExpanded = !_diagnosticsDrawerExpanded;
        _diagnosticsBodyPanel.Visible = _diagnosticsDrawerExpanded;
        _hudBox.Visible = _diagnosticsDrawerExpanded;
        _diagnosticsToggleButton.Text = _diagnosticsDrawerExpanded ? "Hide logs" : "Diagnostics";
    }

    private async Task RunHeroPrimaryActionAsync()
    {
        if (_currentRole == AppRole.Send)
        {
            if (_senderSession.GetSnapshot().Sending)
            {
                await ResetHostToReadyAsync("hero_stop");
                return;
            }

            await StartSendingAsync();
            return;
        }

        if (HasManagedClientSession)
        {
            await StopManagedReceiverSessionAsync("hero_stop", stopLocalReceiver: true);
            return;
        }

        await StartManagedReceiverSessionAsync();
    }

    private void ApplyReceiverPreset(bool ultraLowLatency, bool aggressiveTailDrop)
    {
        _ultraLowLatencyCheck.Checked = ultraLowLatency;
        _aggressiveTailDropCheck.Checked = aggressiveTailDrop;
    }

    private static string FormatDelta(int valueMs) => valueMs >= 0 ? $"{valueMs} ms" : "-";

    private static string FormatLatencyMetric(int valueMs, int ageMs)
    {
        if (valueMs < 0)
        {
            return "-";
        }

        return ageMs >= 0
            ? $"{valueMs} ms ({ageMs} ms ago)"
            : $"{valueMs} ms";
    }

    private static string FormatManualValue(int value) => value > 0 ? value.ToString() : "auto";

    private void ShowFriendlyStatus(string text, bool sticky = false, int ttlSeconds = 18)
    {
        _friendlyStatusText = text.Trim();
        _friendlyStatusExpiresUtc = sticky
            ? DateTimeOffset.MaxValue
            : DateTimeOffset.UtcNow.AddSeconds(ttlSeconds);
        _statusLabel.Text = _friendlyStatusText;
    }

    private string ResolveFriendlyStatus(string fallback)
    {
        if (!string.IsNullOrWhiteSpace(_friendlyStatusText) && _friendlyStatusExpiresUtc > DateTimeOffset.UtcNow)
        {
            return _friendlyStatusText;
        }

        _friendlyStatusText = string.Empty;
        _friendlyStatusExpiresUtc = DateTimeOffset.MinValue;

        if (_currentRole == AppRole.Send)
        {
            var agentSnapshot = _controlPlaneAgent.GetSnapshot();
            var senderSnapshot = _senderSession.GetSnapshot();
            if (senderSnapshot.Sending)
            {
                return $"Идет стрим на {senderSnapshot.RemoteEndpoint}.";
            }

            if (!string.IsNullOrWhiteSpace(agentSnapshot.LeaseSessionId) &&
                !string.Equals(agentSnapshot.LeaseSessionId, "-", StringComparison.Ordinal))
            {
                return "Клиент подключается. Подготавливаю sender для managed session.";
            }

            if (string.Equals(agentSnapshot.Status, "Online", StringComparison.OrdinalIgnoreCase) &&
                !string.IsNullOrWhiteSpace(agentSnapshot.HostId))
            {
                return "Этот ПК виден другим устройствам. Жди подключения с телефона или другого клиента.";
            }

            if (!string.IsNullOrWhiteSpace(_controlPlaneUrlBox.Text))
            {
                return $"Подключаю этот ПК к серверу {_controlPlaneUrlBox.Text.Trim()}...";
            }
        }

        if (_currentRole == AppRole.Receive && HasManagedClientSession)
        {
            if (string.Equals(_managedSessionHealth, "syncing", StringComparison.OrdinalIgnoreCase) &&
                _managedSenderTelemetryAgeSeconds < 0)
            {
                return $"Подключено к {_managedSessionHostLabel}. Жду запуск sender на Windows.";
            }

            if (string.Equals(_managedSessionHealth, "healthy", StringComparison.OrdinalIgnoreCase))
            {
                return $"Идет managed session с {_managedSessionHostLabel}.";
            }
        }

        if (_currentRole == AppRole.Receive && !HasManagedClientSession)
        {
            var baseUrl = _controlPlaneUrlBox.Text.Trim();
            var authState = _desktopControlPlaneClient.GetAuthState();
            if (!string.IsNullOrWhiteSpace(baseUrl) && authState.UserAuthenticated)
            {
                return $"Вы вошли как {authState.Label}. Загрузите список ПК и нажмите «Подключиться».";
            }

            if (!string.IsNullOrWhiteSpace(baseUrl) && !string.Equals(authState.Mode, "anonymous", StringComparison.OrdinalIgnoreCase))
            {
                return $"Control-plane настроен ({baseUrl}). Можно загрузить хосты и начать managed session.";
            }
        }

        return fallback;
    }

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
