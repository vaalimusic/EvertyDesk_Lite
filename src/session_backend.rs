// =============================================================================
// Session Backend Layer — Universal Hypervisor Access Fabric
//
// Each interactive console mode is backed by a SessionBackend implementation.
// The Session Broker picks the backend via Smart Connect and delegates to it.
//
// §27 plan: Session Backend Architecture.
// §27.2: SessionBackend trait with probe() + open() + close().
// §27.3: Console backends: RDP Relay, VNC, SPICE, WebMKS, Serial, BasicRescue.
//
// Isolation guarantee:
//   Core (Broker/Client) calls only SessionBackend — never provider-specific code.
//   Provider-specific setup happens in the provider's BackendBuilder implementation.
// =============================================================================

use crate::capability_engine::SessionMode;

// ── Session token ─────────────────────────────────────────────────────────────

/// Short-lived token issued by Broker for this session.
/// Relay and Client use this to authenticate the data plane stream.
#[derive(Debug, Clone)]
pub struct SessionToken {
    pub token: String,
    pub session_id: String,
    /// Token expiry as Unix seconds (UTC).
    pub expires_at: u64,
}

impl SessionToken {
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.expires_at < now
    }
}

// ── Transport config ──────────────────────────────────────────────────────────

/// How the data plane stream is routed to the client.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportMode {
    /// Direct TCP from client to connector (LAN / VPN).
    Direct,
    /// Reverse tunnel via Relay (connector initiates outbound connection).
    ReverseTunnel,
    /// Relay in the middle (client → Relay ← connector).
    Relay,
    /// Hybrid: try direct first, fall back to relay.
    Hybrid,
}

#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub mode: TransportMode,
    pub relay_url: Option<String>,
    pub relay_token: Option<String>,
    pub direct_host: Option<String>,
    pub direct_port: Option<u16>,
}

// ── Stream descriptor ─────────────────────────────────────────────────────────

/// §27.4 — Describes one logical data stream within a session.
#[derive(Debug, Clone)]
pub struct StreamDescriptor {
    pub stream_id: String,
    pub channel: StreamChannel,
    pub codec: Option<String>,
    pub direction: StreamDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamChannel {
    Preview,
    Input,
    Clipboard,
    Audio,
    FileTransfer,
    Recording,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamDirection {
    ServerToClient,
    ClientToServer,
    Bidirectional,
}

// ── Session backend trait ─────────────────────────────────────────────────────

/// §27.2 — Core trait every console backend must implement.
/// Sync + Send required so the broker can call from any thread.
pub trait SessionBackend: Send + Sync {
    fn mode(&self) -> SessionMode;

    /// Probe whether this backend is reachable and ready.
    /// Returns Ok(()) if ready, Err with reason code if not.
    fn probe(&self) -> Result<(), BackendError>;

    /// Open the backend session. Returns stream descriptors for the data plane.
    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError>;

    /// Close the session and release resources.
    fn close(&mut self);

    /// Human-readable name for logging / audit.
    fn name(&self) -> String {
        self.mode().label().to_owned()
    }
}

// ── Backend error ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BackendError {
    /// Backend probe failed — not reachable.
    NotReachable(String),
    /// Authentication failed.
    AuthFailed(String),
    /// Session open failed.
    OpenFailed(String),
    /// Provider-native error (pass-through).
    ProviderError(String),
    /// Session expired or revoked.
    SessionExpired,
    /// Configuration invalid.
    BadConfig(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReachable(s)  => write!(f, "Backend not reachable: {s}"),
            Self::AuthFailed(s)    => write!(f, "Auth failed: {s}"),
            Self::OpenFailed(s)    => write!(f, "Session open failed: {s}"),
            Self::ProviderError(s) => write!(f, "Provider error: {s}"),
            Self::SessionExpired   => write!(f, "Session expired or revoked"),
            Self::BadConfig(s)     => write!(f, "Bad config: {s}"),
        }
    }
}

// ── BasicRescue backend ───────────────────────────────────────────────────────

/// §27.3.5 — Basic Rescue backend: WMI keyboard + thumbnail preview stream.
/// This is the most reliable mode — works when VM has no network at all.
/// Hyper-V specific implementation lives in hyperv.rs / vm_bridge.rs.
pub struct BasicRescueBackend {
    pub vm_id: String,
    pub provider_id: String,
    opened: bool,
}

impl BasicRescueBackend {
    pub fn new(vm_id: &str, provider_id: &str) -> Self {
        Self {
            vm_id: vm_id.to_owned(),
            provider_id: provider_id.to_owned(),
            opened: false,
        }
    }
}

impl SessionBackend for BasicRescueBackend {
    fn mode(&self) -> SessionMode {
        SessionMode::BasicRescue
    }

    fn probe(&self) -> Result<(), BackendError> {
        // For Hyper-V: check that vm_bridge has WMI access
        // For other providers: check provider-specific rescue availability
        // MVP: assume probe passes if vm_id is non-empty
        if self.vm_id.is_empty() {
            return Err(BackendError::BadConfig("vm_id is empty".to_owned()));
        }
        Ok(())
    }

    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError> {
        if token.is_expired() {
            return Err(BackendError::SessionExpired);
        }
        self.opened = true;
        Ok(vec![
            StreamDescriptor {
                stream_id: format!("{}-preview", token.session_id),
                channel: StreamChannel::Preview,
                codec: Some("webp".to_owned()),
                direction: StreamDirection::ServerToClient,
            },
            StreamDescriptor {
                stream_id: format!("{}-input", token.session_id),
                channel: StreamChannel::Input,
                codec: None,
                direction: StreamDirection::ClientToServer,
            },
        ])
    }

    fn close(&mut self) {
        self.opened = false;
    }

    fn name(&self) -> String {
        "Basic Rescue (WMI Keyboard + Preview)".to_owned()
    }
}

// ── PreviewOnly backend ───────────────────────────────────────────────────────

/// §27.3.1 — Preview-only backend: live thumbnail stream, no input.
pub struct PreviewOnlyBackend {
    pub vm_id: String,
    opened: bool,
}

impl PreviewOnlyBackend {
    pub fn new(vm_id: &str) -> Self {
        Self { vm_id: vm_id.to_owned(), opened: false }
    }
}

impl SessionBackend for PreviewOnlyBackend {
    fn mode(&self) -> SessionMode {
        SessionMode::PreviewOnly
    }

    fn probe(&self) -> Result<(), BackendError> {
        if self.vm_id.is_empty() {
            return Err(BackendError::BadConfig("vm_id is empty".to_owned()));
        }
        Ok(())
    }

    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError> {
        if token.is_expired() {
            return Err(BackendError::SessionExpired);
        }
        self.opened = true;
        Ok(vec![StreamDescriptor {
            stream_id: format!("{}-preview", token.session_id),
            channel: StreamChannel::Preview,
            codec: Some("webp".to_owned()),
            direction: StreamDirection::ServerToClient,
        }])
    }

    fn close(&mut self) {
        self.opened = false;
    }

    fn name(&self) -> String {
        "Preview Only (thumbnail stream)".to_owned()
    }
}

// ── RDP Relay backend stub ────────────────────────────────────────────────────

/// §27.3.2 — RDP Relay backend stub.
/// Full implementation: ironrdp / FreeRDP wrapper connecting to guest port 3389.
pub struct RdpRelayBackend {
    pub vm_id: String,
    pub guest_ip: String,
    pub guest_port: u16,
    opened: bool,
}

impl RdpRelayBackend {
    pub fn new(vm_id: &str, guest_ip: &str, guest_port: u16) -> Self {
        Self {
            vm_id: vm_id.to_owned(),
            guest_ip: guest_ip.to_owned(),
            guest_port,
            opened: false,
        }
    }
}

impl SessionBackend for RdpRelayBackend {
    fn mode(&self) -> SessionMode {
        SessionMode::RdpRelay
    }

    fn probe(&self) -> Result<(), BackendError> {
        if self.guest_ip.is_empty() {
            return Err(BackendError::NotReachable("RDP_NO_GUEST_IP: guest IP unknown".to_owned()));
        }
        // TODO Phase 3: TCP probe to guest_ip:guest_port (port 3389)
        Ok(())
    }

    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError> {
        if token.is_expired() {
            return Err(BackendError::SessionExpired);
        }
        // TODO Phase 3: establish RDP connection through relay transport
        self.opened = true;
        Ok(vec![
            StreamDescriptor {
                stream_id: format!("{}-rdp-video", token.session_id),
                channel: StreamChannel::Preview,
                codec: Some("h264".to_owned()),
                direction: StreamDirection::ServerToClient,
            },
            StreamDescriptor {
                stream_id: format!("{}-rdp-input", token.session_id),
                channel: StreamChannel::Input,
                codec: None,
                direction: StreamDirection::ClientToServer,
            },
            StreamDescriptor {
                stream_id: format!("{}-rdp-clipboard", token.session_id),
                channel: StreamChannel::Clipboard,
                codec: None,
                direction: StreamDirection::Bidirectional,
            },
        ])
    }

    fn close(&mut self) {
        self.opened = false;
    }

    fn name(&self) -> String {
        "RDP Relay (TCP proxy to guest port 3389)".to_owned()
    }
}

// ── VNC Console backend stub ──────────────────────────────────────────────────

/// §27.3.3 — VNC/noVNC backend stub (Proxmox, libvirt/KVM, Xen).
pub struct VncConsoleBackend {
    pub vm_id: String,
    pub vnc_host: String,
    pub vnc_port: u16,
    /// VNC ticket for one-time auth (Proxmox, libvirt SASL)
    pub ticket: Option<String>,
    opened: bool,
}

impl VncConsoleBackend {
    pub fn new(vm_id: &str, vnc_host: &str, vnc_port: u16, ticket: Option<String>) -> Self {
        Self {
            vm_id: vm_id.to_owned(),
            vnc_host: vnc_host.to_owned(),
            vnc_port,
            ticket,
            opened: false,
        }
    }
}

impl SessionBackend for VncConsoleBackend {
    fn mode(&self) -> SessionMode {
        SessionMode::VncConsole
    }

    fn probe(&self) -> Result<(), BackendError> {
        if self.vnc_host.is_empty() {
            return Err(BackendError::NotReachable("VNC host not configured".to_owned()));
        }
        // TODO Phase 1.3/1.4: TCP probe to vnc_host:vnc_port
        Ok(())
    }

    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError> {
        if token.is_expired() {
            return Err(BackendError::SessionExpired);
        }
        // TODO: establish VNC connection, apply ticket auth
        self.opened = true;
        Ok(vec![
            StreamDescriptor {
                stream_id: format!("{}-vnc-frame", token.session_id),
                channel: StreamChannel::Preview,
                codec: Some("rfb".to_owned()),
                direction: StreamDirection::ServerToClient,
            },
            StreamDescriptor {
                stream_id: format!("{}-vnc-input", token.session_id),
                channel: StreamChannel::Input,
                codec: None,
                direction: StreamDirection::ClientToServer,
            },
        ])
    }

    fn close(&mut self) {
        self.opened = false;
    }

    fn name(&self) -> String {
        "VNC Console".to_owned()
    }
}

// ── WebMKS backend stub ───────────────────────────────────────────────────────

/// §27.3.4 — VMware WebMKS / MKS console backend stub.
pub struct WebMksBackend {
    pub vm_id: String,
    /// WebMKS ticket from vSphere API (AcquireTicket).
    pub ticket: String,
    pub vcenter_url: String,
    opened: bool,
}

impl WebMksBackend {
    pub fn new(vm_id: &str, ticket: &str, vcenter_url: &str) -> Self {
        Self {
            vm_id: vm_id.to_owned(),
            ticket: ticket.to_owned(),
            vcenter_url: vcenter_url.to_owned(),
            opened: false,
        }
    }
}

impl SessionBackend for WebMksBackend {
    fn mode(&self) -> SessionMode {
        SessionMode::WebMksConsole
    }

    fn probe(&self) -> Result<(), BackendError> {
        if self.ticket.is_empty() {
            return Err(BackendError::NotReachable(
                "WEBMKS_TICKET_FAILED: no ticket acquired from vSphere".to_owned(),
            ));
        }
        Ok(())
    }

    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError> {
        if token.is_expired() {
            return Err(BackendError::SessionExpired);
        }
        // TODO Phase 1.2: embed WebMKS via webview or native MKS protocol
        self.opened = true;
        Ok(vec![StreamDescriptor {
            stream_id: format!("{}-webmks", token.session_id),
            channel: StreamChannel::Preview,
            codec: Some("webmks".to_owned()),
            direction: StreamDirection::Bidirectional,
        }])
    }

    fn close(&mut self) {
        self.opened = false;
    }

    fn name(&self) -> String {
        "VMware WebMKS Console".to_owned()
    }
}

// ── Serial Console backend stub ───────────────────────────────────────────────

/// §27.3.6 — Serial console backend stub (libvirt, Proxmox).
pub struct SerialConsoleBackend {
    pub vm_id: String,
    /// WebSocket endpoint for serial console stream.
    pub ws_url: String,
    opened: bool,
}

impl SerialConsoleBackend {
    pub fn new(vm_id: &str, ws_url: &str) -> Self {
        Self {
            vm_id: vm_id.to_owned(),
            ws_url: ws_url.to_owned(),
            opened: false,
        }
    }
}

impl SessionBackend for SerialConsoleBackend {
    fn mode(&self) -> SessionMode {
        SessionMode::SerialConsole
    }

    fn probe(&self) -> Result<(), BackendError> {
        if self.ws_url.is_empty() {
            return Err(BackendError::NotReachable(
                "SERIAL_NO_ENDPOINT: serial console WebSocket URL not configured".to_owned(),
            ));
        }
        Ok(())
    }

    fn open(&mut self, token: &SessionToken) -> Result<Vec<StreamDescriptor>, BackendError> {
        if token.is_expired() {
            return Err(BackendError::SessionExpired);
        }
        // TODO Phase 1.4: establish WebSocket to ws_url with session auth
        self.opened = true;
        Ok(vec![StreamDescriptor {
            stream_id: format!("{}-serial", token.session_id),
            channel: StreamChannel::Input,
            codec: Some("text".to_owned()),
            direction: StreamDirection::Bidirectional,
        }])
    }

    fn close(&mut self) {
        self.opened = false;
    }

    fn name(&self) -> String {
        "Serial Console (WebSocket)".to_owned()
    }
}

// ── Session state ─────────────────────────────────────────────────────────────

/// §30 Session Broker — session lifecycle state.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Pending,
    Opening,
    Active,
    Degraded,
    Reconnecting,
    Closing,
    Closed,
    Failed(String),
}

/// §30.2 — Open session record.
pub struct Session {
    pub session_id: String,
    pub vm_id: String,
    pub provider_id: String,
    pub mode: SessionMode,
    pub state: SessionState,
    pub streams: Vec<StreamDescriptor>,
    pub token: Option<SessionToken>,
    pub started_at: u64,
    pub actor: String,
}

impl Session {
    pub fn new(
        session_id: &str,
        vm_id: &str,
        provider_id: &str,
        mode: SessionMode,
        actor: &str,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            session_id: session_id.to_owned(),
            vm_id: vm_id.to_owned(),
            provider_id: provider_id.to_owned(),
            mode,
            state: SessionState::Pending,
            streams: vec![],
            token: None,
            started_at: now,
            actor: actor.to_owned(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, SessionState::Active | SessionState::Degraded)
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "session_id": self.session_id,
            "vm_id": self.vm_id,
            "provider_id": self.provider_id,
            "mode": self.mode.label(),
            "state": format!("{:?}", self.state),
            "started_at": self.started_at,
            "actor": self.actor,
            "stream_count": self.streams.len(),
        })
        .to_string()
    }
}
