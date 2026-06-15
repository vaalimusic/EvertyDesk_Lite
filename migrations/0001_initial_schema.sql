-- =============================================================================
-- Universal Hypervisor Access Fabric — Initial PostgreSQL Schema
-- Migration: 0001_initial_schema
-- §26 plan: Database schema (full 33-table design)
--
-- ADR-001: Schema is provider-agnostic. Provider-specific data in JSONB.
-- ADR-005: provider_metadata JSONB holds hypervisor-specific fields.
-- =============================================================================

-- Extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ── Tenants ───────────────────────────────────────────────────────────────────

CREATE TABLE tenants (
    tenant_id     UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    name          TEXT        NOT NULL,
    slug          TEXT        NOT NULL UNIQUE,
    status        TEXT        NOT NULL DEFAULT 'active'
                              CHECK (status IN ('active', 'suspended', 'deleted')),
    plan          TEXT        NOT NULL DEFAULT 'community',
    settings      JSONB       NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Users ─────────────────────────────────────────────────────────────────────

CREATE TABLE users (
    user_id       UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    email         TEXT        NOT NULL,
    display_name  TEXT,
    role          TEXT        NOT NULL DEFAULT 'operator'
                              CHECK (role IN ('viewer', 'operator', 'power_operator',
                                              'snapshot_operator', 'admin', 'auditor',
                                              'break_glass')),
    mfa_enabled   BOOLEAN     NOT NULL DEFAULT FALSE,
    status        TEXT        NOT NULL DEFAULT 'active'
                              CHECK (status IN ('active', 'suspended', 'deleted')),
    last_login_at TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, email)
);

-- ── Groups ────────────────────────────────────────────────────────────────────

CREATE TABLE groups (
    group_id      UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    name          TEXT        NOT NULL,
    description   TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE group_members (
    group_id      UUID        NOT NULL REFERENCES groups(group_id) ON DELETE CASCADE,
    user_id       UUID        NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    added_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (group_id, user_id)
);

-- ── Providers ─────────────────────────────────────────────────────────────────
-- §24 Universal ProviderType

CREATE TABLE providers (
    provider_id     UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    provider_type   TEXT        NOT NULL
                                CHECK (provider_type IN (
                                    'hyperv', 'vmware', 'proxmox', 'libvirt',
                                    'qemu_standalone', 'xen', 'xcp_ng',
                                    'azure_local', 'custom'
                                )),
    name            TEXT        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'healthy', 'degraded',
                                                  'unavailable', 'disabled')),
    config          JSONB       NOT NULL DEFAULT '{}',
    -- Provider-specific metadata (vcenter_url, wmi_namespace, api_url, etc.)
    provider_metadata JSONB     NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- ── Connectors ────────────────────────────────────────────────────────────────
-- §25 Connector enrollment

CREATE TABLE connectors (
    connector_id    UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    provider_id     UUID        REFERENCES providers(provider_id) ON DELETE SET NULL,
    name            TEXT        NOT NULL,
    hostname        TEXT,
    status          TEXT        NOT NULL DEFAULT 'offline'
                                CHECK (status IN ('offline', 'online', 'degraded', 'disabled')),
    version         TEXT,
    features        TEXT[]      NOT NULL DEFAULT '{}',
    last_seen_at    TIMESTAMPTZ,
    enrollment_token_hash TEXT,
    cert_fingerprint TEXT,
    -- Connector-specific metadata
    connector_metadata JSONB    NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Hosts ─────────────────────────────────────────────────────────────────────

CREATE TABLE hosts (
    host_id         UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    provider_id     UUID        NOT NULL REFERENCES providers(provider_id) ON DELETE CASCADE,
    connector_id    UUID        REFERENCES connectors(connector_id) ON DELETE SET NULL,
    -- Stable ID assigned by provider (ESXi BIOS UUID, WMI ComputerSystemGUID, etc.)
    provider_native_id TEXT     NOT NULL,
    hostname        TEXT        NOT NULL,
    cluster         TEXT,
    os_version      TEXT,
    status          TEXT        NOT NULL DEFAULT 'unknown'
                                CHECK (status IN ('healthy', 'degraded', 'offline', 'unknown')),
    host_metadata   JSONB       NOT NULL DEFAULT '{}',
    last_seen_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_id, provider_native_id)
);

-- ── Clusters ──────────────────────────────────────────────────────────────────

CREATE TABLE clusters (
    cluster_id      UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    provider_id     UUID        NOT NULL REFERENCES providers(provider_id) ON DELETE CASCADE,
    name            TEXT        NOT NULL,
    cluster_type    TEXT,       -- "hyperv_failover", "vcenter_cluster", "proxmox_cluster"
    cluster_metadata JSONB      NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_id, name)
);

-- ── VMs ───────────────────────────────────────────────────────────────────────
-- §24 Universal VmInfo — no provider-specific columns in core fields

CREATE TABLE vms (
    vm_id           UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    provider_id     UUID        NOT NULL REFERENCES providers(provider_id) ON DELETE CASCADE,
    host_id         UUID        REFERENCES hosts(host_id) ON DELETE SET NULL,
    cluster_id      UUID        REFERENCES clusters(cluster_id) ON DELETE SET NULL,
    -- Provider-native stable ID (GUID, MOID, VMID number, domain UUID)
    provider_native_id TEXT     NOT NULL,
    name            TEXT        NOT NULL,
    -- Universal power state
    power_state     TEXT        NOT NULL DEFAULT 'unknown'
                                CHECK (power_state IN (
                                    'running', 'stopped', 'paused', 'suspended',
                                    'saved', 'starting', 'stopping', 'resetting', 'unknown'
                                )),
    guest_ips       TEXT[]      NOT NULL DEFAULT '{}',
    tools_status    TEXT        NOT NULL DEFAULT 'unknown'
                                CHECK (tools_status IN (
                                    'running', 'not_running', 'not_installed',
                                    'outdated', 'unknown', 'not_applicable'
                                )),
    tags            TEXT[]      NOT NULL DEFAULT '{}',
    -- All provider-specific fields go here (§55.2 ADR-005)
    provider_metadata JSONB     NOT NULL DEFAULT '{}',
    last_seen_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_id, provider_native_id)
);

CREATE INDEX idx_vms_tenant     ON vms(tenant_id);
CREATE INDEX idx_vms_provider   ON vms(provider_id);
CREATE INDEX idx_vms_host       ON vms(host_id);
CREATE INDEX idx_vms_power      ON vms(power_state);
CREATE INDEX idx_vms_tags       ON vms USING GIN(tags);

-- ── VM capabilities ───────────────────────────────────────────────────────────
-- Cached CapabilityGraph per VM (refreshed on inventory sync)

CREATE TABLE vm_capabilities (
    cap_id          UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    vm_id           UUID        NOT NULL REFERENCES vms(vm_id) ON DELETE CASCADE,
    -- Full CapabilityGraph serialised as JSONB
    capability_graph JSONB      NOT NULL DEFAULT '{}',
    recommended_mode TEXT,
    evaluated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (vm_id)
);

-- ── Snapshots ─────────────────────────────────────────────────────────────────
-- Universal SnapshotInfo — maps Hyper-V checkpoints, VMware/Proxmox snapshots

CREATE TABLE snapshots (
    snapshot_id     UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    vm_id           UUID        NOT NULL REFERENCES vms(vm_id) ON DELETE CASCADE,
    provider_native_id TEXT     NOT NULL,
    name            TEXT        NOT NULL,
    description     TEXT,
    snapshot_type   TEXT,
    is_current      BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    snapshot_metadata JSONB     NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_snapshots_vm ON snapshots(vm_id);

-- ── Sessions ──────────────────────────────────────────────────────────────────
-- §30 Session records

CREATE TABLE sessions (
    session_id      UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    user_id         UUID        REFERENCES users(user_id) ON DELETE SET NULL,
    vm_id           UUID        REFERENCES vms(vm_id) ON DELETE SET NULL,
    provider_id     UUID        REFERENCES providers(provider_id) ON DELETE SET NULL,
    -- Session mode used (rdp_relay, vnc_console, enhanced_session, etc.)
    session_mode    TEXT        NOT NULL,
    state           TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (state IN (
                                    'pending', 'opening', 'active', 'degraded',
                                    'reconnecting', 'closing', 'closed', 'failed'
                                )),
    error           TEXT,
    -- Policy snapshot at session open time
    policy_snapshot JSONB       NOT NULL DEFAULT '{}',
    -- Relay / transport info
    transport_mode  TEXT,
    relay_id        TEXT,
    -- Timing
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at        TIMESTAMPTZ,
    max_duration_s  INTEGER     NOT NULL DEFAULT 86400,
    -- JIT grant reference (if session required approval)
    approval_id     UUID,
    session_metadata JSONB      NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_sessions_tenant  ON sessions(tenant_id);
CREATE INDEX idx_sessions_user    ON sessions(user_id);
CREATE INDEX idx_sessions_vm      ON sessions(vm_id);
CREATE INDEX idx_sessions_state   ON sessions(state);
CREATE INDEX idx_sessions_started ON sessions(started_at DESC);

-- ── Policies ──────────────────────────────────────────────────────────────────
-- §39 Policy Engine

CREATE TABLE policies (
    policy_id       UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    name            TEXT        NOT NULL,
    priority        INTEGER     NOT NULL DEFAULT 100,
    -- Selector: which VMs / users / groups this policy applies to
    selector        JSONB       NOT NULL DEFAULT '{}',
    -- Rules: what is allowed/denied
    rules           JSONB       NOT NULL DEFAULT '{}',
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    created_by      UUID        REFERENCES users(user_id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- ── Approvals ─────────────────────────────────────────────────────────────────
-- §33 JIT Access + Approval Workflow

CREATE TABLE approvals (
    approval_id     UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    requester_id    UUID        NOT NULL REFERENCES users(user_id),
    vm_id           UUID        REFERENCES vms(vm_id),
    requested_mode  TEXT,
    reason          TEXT,
    status          TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'cancelled')),
    reviewed_by     UUID        REFERENCES users(user_id),
    review_note     TEXT,
    -- Time-bound grant (TTL in seconds from approval)
    grant_ttl_s     INTEGER     NOT NULL DEFAULT 1800,
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_approvals_tenant  ON approvals(tenant_id);
CREATE INDEX idx_approvals_status  ON approvals(status);
CREATE INDEX idx_approvals_expires ON approvals(expires_at);

-- ── Audit events ──────────────────────────────────────────────────────────────
-- §32 Audit System — immutable audit log

CREATE TABLE audit_events (
    event_id        UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    session_id      UUID        REFERENCES sessions(session_id) ON DELETE SET NULL,
    vm_id           UUID        REFERENCES vms(vm_id) ON DELETE SET NULL,
    user_id         UUID        REFERENCES users(user_id) ON DELETE SET NULL,
    event_type      TEXT        NOT NULL,
    severity        TEXT        NOT NULL DEFAULT 'info'
                                CHECK (severity IN ('debug', 'info', 'warning', 'error', 'critical')),
    metadata        JSONB       NOT NULL DEFAULT '{}',
    -- Explicit timestamp (never rely on DB default alone — set by application)
    event_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Audit log is append-only — no UPDATE/DELETE in production
-- REVOKE UPDATE, DELETE ON audit_events FROM application_role;

CREATE INDEX idx_audit_tenant   ON audit_events(tenant_id);
CREATE INDEX idx_audit_session  ON audit_events(session_id);
CREATE INDEX idx_audit_vm       ON audit_events(vm_id);
CREATE INDEX idx_audit_user     ON audit_events(user_id);
CREATE INDEX idx_audit_type     ON audit_events(event_type);
CREATE INDEX idx_audit_time     ON audit_events(event_at DESC);
CREATE INDEX idx_audit_severity ON audit_events(severity);

-- ── Recordings ────────────────────────────────────────────────────────────────
-- §35 Session Recording metadata

CREATE TABLE recordings (
    recording_id    UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id      UUID        NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    tenant_id       UUID        NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    recording_type  TEXT        NOT NULL DEFAULT 'event_timeline'
                                CHECK (recording_type IN ('event_timeline', 'video', 'hybrid')),
    storage_path    TEXT,
    size_bytes      BIGINT,
    duration_s      INTEGER,
    status          TEXT        NOT NULL DEFAULT 'recording'
                                CHECK (status IN ('recording', 'completed', 'failed', 'deleted')),
    -- Retention policy applied at time of creation
    retain_until    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ,
    recording_metadata JSONB    NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_recordings_session ON recordings(session_id);
CREATE INDEX idx_recordings_tenant  ON recordings(tenant_id);

-- ── Feature flags ─────────────────────────────────────────────────────────────
-- §40 per-tenant feature flags (experimental features, AF_HYPERV, etc.)

CREATE TABLE feature_flags (
    flag_id         UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id       UUID        REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    -- NULL tenant_id = global flag
    flag_name       TEXT        NOT NULL,
    enabled         BOOLEAN     NOT NULL DEFAULT FALSE,
    config          JSONB       NOT NULL DEFAULT '{}',
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, flag_name)
);

-- Default flags: AF_HYPERV experimental disabled
INSERT INTO feature_flags (tenant_id, flag_name, enabled)
VALUES (NULL, 'experimental_hvsocket_backend', FALSE)
ON CONFLICT DO NOTHING;

-- ── Schema version ────────────────────────────────────────────────────────────

CREATE TABLE schema_versions (
    version         INTEGER     PRIMARY KEY,
    description     TEXT        NOT NULL,
    applied_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO schema_versions (version, description)
VALUES (1, '0001_initial_schema: Universal Hypervisor Access Fabric core tables');
