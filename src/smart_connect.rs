// =============================================================================
// Smart Connect — Universal Hypervisor Access Fabric
//
// Selects the best available session backend automatically based on:
//   • VmCapabilityGraph from the provider
//   • SessionPolicy (RBAC / ACL / JIT)
//   • Client capabilities (decoder support)
//   • Requested mode (explicit or Auto)
//
// §38 plan: Smart Connect Algorithm.
// §52 plan: Explainability — reason codes + remediation.
// §39 plan: Policy Engine Details.
//
// Key rules:
//   ADR-003: Client reads CapabilityGraph, not provider-specific fields.
//   §55.3:   No provider leakage — no "if provider == HyperV" in this file.
// =============================================================================

use crate::{
    capability_engine::{Capability, CapabilityState, SessionMode, VmCapabilityGraph},
    provider_api::{BackendDecision, BackendScore, PolicyResult, RecordingMode},
};

// ── Requested mode ────────────────────────────────────────────────────────────

/// What the client requested when opening a session.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestedMode {
    /// Let Smart Connect choose the best backend automatically.
    Auto,
    /// Explicit mode requested by the user.
    Explicit(SessionMode),
}

impl RequestedMode {
    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::Explicit(_))
    }
}

// ── Client capabilities ───────────────────────────────────────────────────────

/// What the client (GUI) can decode and display.
/// Populated at startup from available decoders.
#[derive(Debug, Clone)]
pub struct ClientCapabilities {
    pub can_rdp: bool,
    pub can_vnc: bool,
    pub can_spice: bool,
    pub can_webmks: bool,
    pub can_serial: bool,
    pub can_preview: bool,
    pub can_rescue: bool,
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        // Desktop client supports all modes by default
        Self {
            can_rdp: true,
            can_vnc: true,
            can_spice: true,
            can_webmks: true,
            can_serial: true,
            can_preview: true,
            can_rescue: true,
        }
    }
}

impl ClientCapabilities {
    pub fn supports(&self, mode: &SessionMode) -> bool {
        match mode {
            SessionMode::Offline => false,
            SessionMode::PreviewOnly => self.can_preview,
            SessionMode::BasicRescue => self.can_rescue,
            SessionMode::RdpRelay => self.can_rdp,
            SessionMode::VncConsole => self.can_vnc,
            SessionMode::SpiceConsole => self.can_spice,
            SessionMode::WebConsole => true, // embedded webview
            SessionMode::WebMksConsole => self.can_webmks,
            SessionMode::SerialConsole => self.can_serial,
            SessionMode::EnhancedSession => self.can_rdp,
            SessionMode::ProviderNativeConsole => true,
            SessionMode::ExperimentalHvSocket => false, // never client-enabled by default
            SessionMode::Experimental(_) => false,
        }
    }
}

// ── Smart Connect ─────────────────────────────────────────────────────────────

/// Full Smart Connect decision — selects best backend with fallbacks.
///
/// Does NOT apply policy — caller must pre-filter capability graph
/// using PolicyResult::allowed_modes before calling this.
pub fn choose_backend(
    graph: &VmCapabilityGraph,
    policy: &PolicyResult,
    client: &ClientCapabilities,
    requested: RequestedMode,
) -> BackendDecision {
    // Explicit mode: return it if client+policy allow, otherwise error
    if let RequestedMode::Explicit(mode) = &requested {
        let cap = mode_capability(graph, mode);
        if !policy.allowed_modes.contains(mode) {
            return BackendDecision {
                selected_mode: SessionMode::Offline,
                fallback_modes: vec![],
                reason: format!(
                    "Mode '{}' is blocked by policy: {}",
                    mode.label(),
                    policy.reason.as_deref().unwrap_or("policy denied")
                ),
                warnings: vec![],
            };
        }
        if !client.supports(mode) {
            return BackendDecision {
                selected_mode: SessionMode::Offline,
                fallback_modes: vec![],
                reason: format!("Client does not support decoder for '{}'", mode.label()),
                warnings: vec![],
            };
        }
        if matches!(cap.state, CapabilityState::Unsupported | CapabilityState::BlockedByPolicy) {
            return BackendDecision {
                selected_mode: SessionMode::Offline,
                fallback_modes: vec![],
                reason: format!(
                    "Mode '{}' is not available: {}",
                    mode.label(),
                    cap.human_reason
                ),
                warnings: vec![],
            };
        }
        return BackendDecision {
            selected_mode: mode.clone(),
            fallback_modes: vec![],
            reason: cap.human_reason.clone(),
            warnings: if cap.state == CapabilityState::Degraded {
                vec![format!("Mode is degraded: {}", cap.reason_code)]
            } else {
                vec![]
            },
        };
    }

    // Auto mode: score all candidates, apply policy + client filter, pick best
    let all_candidates: Vec<(SessionMode, &Capability, i32)> = vec![
        (SessionMode::ProviderNativeConsole, &graph.experimental, 100),
        (SessionMode::EnhancedSession,       &graph.enhanced_session, 95),
        (SessionMode::WebMksConsole,         &graph.webmks_console, 95),
        (SessionMode::SpiceConsole,          &graph.spice_console, 90),
        (SessionMode::VncConsole,            &graph.vnc_console, 85),
        (SessionMode::RdpRelay,              &graph.rdp_relay, 80),
        (SessionMode::WebConsole,            &graph.web_console, 75),
        (SessionMode::SerialConsole,         &graph.serial_console, 55),
        (SessionMode::BasicRescue,           &graph.keyboard_rescue, 50),
        (SessionMode::PreviewOnly,           &graph.preview, 20),
    ];

    let mut scored: Vec<BackendScore> = all_candidates
        .into_iter()
        .filter(|(mode, cap, _)| {
            // Skip blocked/unsupported
            !matches!(cap.state, CapabilityState::Unsupported | CapabilityState::BlockedByPolicy | CapabilityState::Experimental)
            // Skip if policy doesn't allow
            && policy.allowed_modes.contains(mode)
            // Skip if client can't decode
            && client.supports(mode)
        })
        .map(|(mode, cap, base)| {
            let mut score = base;
            // §38.4 Modifiers
            if cap.state == CapabilityState::Degraded { score -= 20; }
            if cap.state == CapabilityState::Unknown   { score -= 10; }
            // Bonus: no guest network needed (rescue/keyboard modes)
            if matches!(mode, SessionMode::BasicRescue | SessionMode::SerialConsole) {
                score += 20;
            }
            // Clipboard bonus
            if graph.clipboard.state.is_available()
                && matches!(mode, SessionMode::EnhancedSession | SessionMode::RdpRelay
                    | SessionMode::WebMksConsole | SessionMode::SpiceConsole
                    | SessionMode::VncConsole)
            {
                score += 10;
            }
            // Policy: recording penalty if recording required but not available
            if matches!(policy.recording_mode, RecordingMode::VideoRequired)
                && !graph.recording.state.is_available()
            {
                score -= 30;
            }
            BackendScore {
                mode,
                available: cap.state.is_available(),
                score,
                reason: cap.human_reason.clone(),
            }
        })
        .collect();

    if scored.is_empty() {
        return BackendDecision {
            selected_mode: SessionMode::Offline,
            fallback_modes: vec![],
            reason: "No accessible backend available — VM may be offline or access blocked".to_owned(),
            warnings: vec![],
        };
    }

    scored.sort_by(|a, b| b.score.cmp(&a.score));

    let best = scored.remove(0);
    let fallbacks: Vec<SessionMode> = scored.into_iter().map(|s| s.mode).collect();

    let mut warnings = vec![];
    if !graph.any_interactive() {
        warnings.push("No interactive backend available — preview/rescue only".to_owned());
    }

    BackendDecision {
        selected_mode: best.mode,
        fallback_modes: fallbacks,
        reason: best.reason,
        warnings,
    }
}

/// Get the relevant Capability for a given SessionMode from the graph.
pub fn mode_capability<'a>(graph: &'a VmCapabilityGraph, mode: &SessionMode) -> &'a Capability {
    match mode {
        SessionMode::PreviewOnly        => &graph.preview,
        SessionMode::BasicRescue        => &graph.keyboard_rescue,
        SessionMode::RdpRelay           => &graph.rdp_relay,
        SessionMode::VncConsole         => &graph.vnc_console,
        SessionMode::SpiceConsole       => &graph.spice_console,
        SessionMode::WebConsole         => &graph.web_console,
        SessionMode::WebMksConsole      => &graph.webmks_console,
        SessionMode::SerialConsole      => &graph.serial_console,
        SessionMode::EnhancedSession    => &graph.enhanced_session,
        SessionMode::ProviderNativeConsole => &graph.experimental,
        SessionMode::ExperimentalHvSocket |
        SessionMode::Experimental(_)    => &graph.hv_socket,
        SessionMode::Offline            => &graph.preview, // offline has no specific cap
    }
}

// ── Explain API ───────────────────────────────────────────────────────────────

/// §52 Explainability — why a connection mode is unavailable.

#[derive(Debug, Clone)]
pub struct ExplainReason {
    pub code: String,
    pub message: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExplainResult {
    pub summary: String,
    pub reasons: Vec<ExplainReason>,
    pub recommended_actions: Vec<ExplainAction>,
}

#[derive(Debug, Clone)]
pub struct ExplainAction {
    pub label: String,
    pub mode: Option<SessionMode>,
}

/// Explain why interactive access to a VM is unavailable.
/// Returns a structured explanation with reason codes and remediation steps.
pub fn explain_unavailability(graph: &VmCapabilityGraph) -> ExplainResult {
    let mut reasons = Vec::new();
    let mut actions = Vec::new();

    // Check each interactive mode
    let interactive_modes: &[(SessionMode, &Capability)] = &[
        (SessionMode::EnhancedSession,  &graph.enhanced_session),
        (SessionMode::RdpRelay,         &graph.rdp_relay),
        (SessionMode::VncConsole,       &graph.vnc_console),
        (SessionMode::SpiceConsole,     &graph.spice_console),
        (SessionMode::WebMksConsole,    &graph.webmks_console),
        (SessionMode::SerialConsole,    &graph.serial_console),
        (SessionMode::BasicRescue,      &graph.keyboard_rescue),
    ];

    let mut any_available = false;
    for (mode, cap) in interactive_modes {
        if cap.state.is_available() {
            any_available = true;
        } else if !matches!(cap.state, CapabilityState::Available) {
            reasons.push(ExplainReason {
                code: cap.reason_code.clone(),
                message: format!("{} — {}", mode.label(), cap.human_reason),
                remediation: cap.remediation.clone(),
            });
        }
    }

    if any_available {
        return ExplainResult {
            summary: "At least one interactive backend is available.".to_owned(),
            reasons: vec![],
            recommended_actions: vec![ExplainAction {
                label: format!("Open {}", graph.recommended_mode.label()),
                mode: Some(graph.recommended_mode.clone()),
            }],
        };
    }

    // Add rescue action if available
    if graph.keyboard_rescue.state.is_available() {
        actions.push(ExplainAction {
            label: "Open Basic Rescue".to_owned(),
            mode: Some(SessionMode::BasicRescue),
        });
    }
    // Preview action
    if graph.preview.state.is_available() {
        actions.push(ExplainAction {
            label: "Open Preview Only".to_owned(),
            mode: Some(SessionMode::PreviewOnly),
        });
    }

    ExplainResult {
        summary: "Interactive console is unavailable for this VM.".to_owned(),
        reasons,
        recommended_actions: actions,
    }
}

/// Serialize ExplainResult to JSON for the Explain API (§60.5, §52.1).
pub fn explain_to_json(vm_id: &str, result: &ExplainResult) -> String {
    let reasons: Vec<serde_json::Value> = result.reasons.iter().map(|r| {
        serde_json::json!({
            "code": r.code,
            "message": r.message,
            "remediation": r.remediation,
        })
    }).collect();
    let actions: Vec<serde_json::Value> = result.recommended_actions.iter().map(|a| {
        serde_json::json!({
            "label": a.label,
            "mode": a.mode.as_ref().map(|m| m.label()),
        })
    }).collect();
    serde_json::json!({
        "vm_id": vm_id,
        "summary": result.summary,
        "reasons": reasons,
        "recommended_actions": actions,
    }).to_string()
}

// ── Policy Engine skeleton ────────────────────────────────────────────────────

/// §39 Policy Engine — evaluates session policy for a given VM + user context.
/// Full implementation: §64.7 Epic Policy and Security.
pub struct PolicyEngine;

impl PolicyEngine {
    /// Evaluate policy for a session request.
    /// In MVP: returns permissive policy. Full: evaluates 8-stage pipeline (§39.1).
    pub fn evaluate(
        _vm_tags: &[&str],
        _user_role: &str,
        _requested_mode: &SessionMode,
    ) -> PolicyResult {
        // MVP: allow all. Phase 2: evaluate tier rules (§39.1–§39.4).
        PolicyResult::allow_all()
    }

    /// Check if a specific action (power / snapshot / clipboard) is allowed.
    pub fn can_perform(action: &str, _user_role: &str, _vm_tags: &[&str]) -> bool {
        match action {
            // All actions allowed in MVP
            "power_start" | "power_stop" | "power_restart" | "power_pause" => true,
            "snapshot_create" | "snapshot_apply" | "snapshot_delete" => true,
            "clipboard_read" | "clipboard_write" => true,
            "rescue_input" => true,
            _ => false,
        }
    }
}

// ── Audit event skeleton ──────────────────────────────────────────────────────

/// §32 Audit system — event types for privileged actions.
#[derive(Debug, Clone)]
pub enum AuditEventType {
    SessionStarted,
    SessionEnded,
    SessionRevoked,
    PowerActionRequested,
    SnapshotCreated,
    SnapshotApplied,
    SnapshotDeleted,
    RescueInputSent,
    ClipboardEvent,
    PolicyDenied,
    ApprovalRequested,
    ApprovalGranted,
    ApprovalDenied,
}

impl AuditEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStarted    => "session.started",
            Self::SessionEnded      => "session.ended",
            Self::SessionRevoked    => "session.revoked",
            Self::PowerActionRequested => "power.action_requested",
            Self::SnapshotCreated   => "snapshot.created",
            Self::SnapshotApplied   => "snapshot.applied",
            Self::SnapshotDeleted   => "snapshot.deleted",
            Self::RescueInputSent   => "rescue.input_sent",
            Self::ClipboardEvent    => "clipboard.event",
            Self::PolicyDenied      => "policy.denied",
            Self::ApprovalRequested => "approval.requested",
            Self::ApprovalGranted   => "approval.granted",
            Self::ApprovalDenied    => "approval.denied",
        }
    }
    pub fn severity(&self) -> &'static str {
        match self {
            Self::SessionStarted | Self::SessionEnded => "info",
            Self::PowerActionRequested | Self::SnapshotApplied
            | Self::SnapshotDeleted | Self::RescueInputSent => "warning",
            Self::SessionRevoked | Self::PolicyDenied
            | Self::ApprovalDenied => "error",
            _ => "info",
        }
    }
}

/// §32 Audit event — serialisable audit record.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub event_id: String,
    pub session_id: Option<String>,
    pub vm_id: Option<String>,
    pub event_type: AuditEventType,
    pub actor: String,
    pub metadata: serde_json::Value,
}

impl AuditEvent {
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "event_id": self.event_id,
            "session_id": self.session_id,
            "vm_id": self.vm_id,
            "event_type": self.event_type.as_str(),
            "severity": self.event_type.severity(),
            "actor": self.actor,
            "metadata": self.metadata,
        })
        .to_string()
    }

    /// Create a simple session.started audit event.
    pub fn session_started(session_id: &str, vm_id: &str, actor: &str, mode: &SessionMode) -> Self {
        Self {
            event_id: uuid_v4_simple(),
            session_id: Some(session_id.to_owned()),
            vm_id: Some(vm_id.to_owned()),
            event_type: AuditEventType::SessionStarted,
            actor: actor.to_owned(),
            metadata: serde_json::json!({ "selected_mode": mode.label() }),
        }
    }
}

fn uuid_v4_simple() -> String {
    // Simple UUID v4 without external crate (random from system time + counter)
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (t >> 32) as u32,
        (t >> 16) as u16 & 0xFFFF,
        t as u16 & 0x0FFF,
        ((t >> 48) as u16 & 0x3FFF) | 0x8000,
        c ^ (t as u64 & 0xFFFF_FFFF_FFFF),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_engine::VmCapabilityGraph;

    #[test]
    fn test_choose_backend_prefers_enhanced_over_rdp() {
        let graph = VmCapabilityGraph {
            vm_id: "test-vm".to_owned(),
            provider_type: None,
            preview: Capability::available("P", "ok"),
            keyboard_rescue: Capability::available("R", "ok"),
            rdp_relay: Capability::available("RDP", "ok"),
            vnc_console: Capability::unsupported("VNC", "not available", None),
            spice_console: Capability::unsupported("SPICE", "not available", None),
            web_console: Capability::unsupported("WEB", "not available", None),
            webmks_console: Capability::unsupported("WEBMKS", "not available", None),
            serial_console: Capability::unsupported("SERIAL", "not available", None),
            enhanced_session: Capability::available("ENH", "enhanced ok"),
            hv_socket: Capability::experimental("EXP"),
            clipboard: Capability::available("CB", "ok"),
            file_transfer: Capability::unsupported("FT", "not available", None),
            recording: Capability::degraded("REC", "metadata only", None),
            experimental: Capability::experimental("EXP2"),
            recommended_mode: crate::capability_engine::SessionMode::EnhancedSession,
            constraints: vec![],
        };
        let policy = PolicyResult::allow_all();
        let client = ClientCapabilities::default();
        let result = choose_backend(&graph, &policy, &client, RequestedMode::Auto);
        assert_eq!(result.selected_mode, SessionMode::EnhancedSession);
    }

    #[test]
    fn test_explain_offline_vm() {
        let graph = VmCapabilityGraph::unsupported_stub("offline-vm", "VM_OFFLINE");
        let result = explain_unavailability(&graph);
        assert!(!result.summary.is_empty());
    }

    #[test]
    fn test_audit_event_json() {
        let ev = AuditEvent::session_started(
            "session-001",
            "vm-001",
            "admin@example.com",
            &SessionMode::RdpRelay,
        );
        let json = ev.to_json();
        assert!(json.contains("session.started"));
        assert!(json.contains("RDP Relay"));
    }
}
