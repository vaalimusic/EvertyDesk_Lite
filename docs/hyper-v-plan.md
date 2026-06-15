# ТЕХНИЧЕСКОЕ ЗАДАНИЕ

## Universal Hypervisor Access Fabric

### Универсальная безагентная платформа доступа, rescue, live-preview и управления ВМ на разных гипервизорах

---

## 0. Краткое описание

Необходимо реализовать универсальную платформу удаленного доступа и управления виртуальными машинами разных гипервизоров без установки сторонних агентов внутрь гостевых ОС.

Платформа должна поддерживать несколько virtualization providers:

* Hyper-V;
* VMware vSphere / ESXi / vCenter;
* Proxmox VE;
* KVM / QEMU / libvirt;
* Xen / XCP-ng в будущем;
* другие provider’ы через plugin-модель.

Ключевой принцип архитектуры:

```text
Core платформы не должен знать деталей конкретного гипервизора.
Каждый гипервизор подключается через отдельный Provider Connector.
```

Hyper-V является первым реализуемым provider’ом, но все основные сущности продукта должны называться универсально:

```text
Provider
Host
Vm
Session
Capability
Snapshot
Console
Preview
NodeAgent
ProviderConnector
```

а не:

```text
HyperVHost
HyperVVm
HyperVSession
```

---

# 1. Цели проекта

## 1.1. Главная цель

Создать универсальную платформу доступа к виртуальным машинам, которая позволяет администраторам:

* видеть все ВМ на разных гипервизорах в одном dashboard;
* получать live-preview ВМ;
* открывать консоль ВМ через лучший доступный backend;
* выполнять rescue-действия, если гостевая сеть сломана;
* подключаться к ВМ без установки стороннего агента внутрь гостевой ОС;
* управлять power state;
* управлять snapshot/checkpoint;
* контролировать доступ через RBAC, ACL, JIT и audit;
* работать через NAT/firewall с помощью relay;
* добавлять новые гипервизоры без переписывания core.

## 1.2. Продуктовая цель

Сделать не просто “удаленный рабочий стол”, а:

```text
Zero-Trust Universal VM Access & Rescue Platform
```

То есть платформу, которая решает не только задачу “подключиться к ВМ”, но и задачи:

* диагностики;
* восстановления;
* контроля доступа;
* аудита;
* работы с несколькими гипервизорами;
* безопасного enterprise-доступа.

## 1.3. Техническая цель

Реализовать архитектуру, где:

* core не зависит от Hyper-V, VMware, Proxmox или libvirt;
* каждый provider реализуется через единый trait/interface;
* GUI работает с универсальным Capability Graph;
* Broker выбирает backend автоматически;
* Agent/Connector инкапсулирует provider-specific API;
* experimental-функции не ломают production-core;
* новые provider’ы добавляются как модули.

---

# 2. Основные принципы

## 2.1. No Guest Agent

В гостевые ОС не устанавливается сторонний агент продукта.

Допускается использование встроенных компонентов гостевой ОС:

* RDP;
* xRDP;
* VMware Tools;
* QEMU Guest Agent;
* Hyper-V Integration Services;
* SPICE Guest Tools;
* стандартные системные службы ОС.

Но продукт не должен требовать установки собственного агента внутрь ВМ.

## 2.2. Provider-first архитектура

Все гипервизоры подключаются через abstraction layer.

Правильно:

```text
core -> provider_api -> provider_hyperv
core -> provider_api -> provider_vmware
core -> provider_api -> provider_proxmox
core -> provider_api -> provider_libvirt
```

Неправильно:

```text
core -> hyperv
client -> hyperv
broker -> hyperv
```

## 2.3. Smart Capability Model

Система не должна иметь один режим подключения. Она должна строить capability graph для каждой ВМ и выбирать лучший backend.

Пример:

```text
VM: APP-01
Provider: Hyper-V

Preview: Available
Basic Rescue: Available
RDP Relay: Unavailable — no guest IP
Enhanced Session: Degraded — guest RDS disabled
AF_HYPERV: Experimental disabled
Recommended: BasicRescue
```

## 2.4. Supported Core + Experimental Backends

Production-core должен строиться на поддерживаемых механизмах provider’ов:

* Hyper-V WMI/CIM;
* VMware vSphere API / WebMKS / MKS;
* Proxmox API / VNC/noVNC/SPICE;
* libvirt API / VNC/SPICE/serial;
* RDP relay where guest network exists.

Experimental backend’ы должны быть:

* отключены по умолчанию;
* изолированы;
* включаемы через feature flag;
* не обязательны для MVP;
* не являются единственной опорой продукта.

## 2.5. Единая UX-модель

Для пользователя не должно быть важно, где находится ВМ:

```text
Hyper-V
VMware
Proxmox
KVM/libvirt
```

Пользователь должен видеть:

```text
VM
Power state
Preview
Available session modes
Recommended action
Connect
Rescue
Snapshots
Audit
```

Provider-specific детали показываются отдельно.

---

# 3. Высокоуровневая архитектура

```text
[Admin GUI Client]
  ├── Universal Dashboard
  ├── Universal Session UX
  ├── RDP Decoder
  ├── VNC Decoder
  ├── SPICE Adapter / Decoder
  ├── WebMKS / MKS Adapter
  ├── Serial Console UI
  ├── Clipboard Manager
  ├── Input Manager
  ├── Local Cache
  └── Local Policy Enforcement

        ⇅ encrypted data plane / relay / NAT traversal

[Relay / Edge Network]
  ├── Direct path negotiation
  ├── Relay fallback
  ├── NAT traversal
  ├── Regional routing
  ├── Bandwidth shaping
  └── Reconnect support

        ⇅ secure session transport

[Control Plane / Broker]
  ├── Identity / SSO / MFA
  ├── Tenant Management
  ├── Provider Registry
  ├── Host Registry
  ├── VM Registry
  ├── RBAC / ACL
  ├── Policy Engine
  ├── Capability Aggregator
  ├── Session Broker
  ├── Approval Workflow
  ├── Audit Log
  ├── Recording Metadata
  ├── Agent / Connector Update Service
  └── Fleet Health

        ⇅ secure connector channel

[Node Agents / Provider Connectors]
  ├── Hyper-V Connector
  ├── VMware Connector
  ├── Proxmox Connector
  ├── Libvirt Connector
  ├── Xen Connector
  └── Future Provider Connectors

        ⇅ native provider APIs

[Virtualization Platforms]
  ├── Hyper-V
  ├── VMware vSphere / ESXi / vCenter
  ├── Proxmox VE
  ├── KVM / QEMU / libvirt
  ├── Xen / XCP-ng
  └── Future Hypervisors
```

---

# 4. Компоненты системы

## 4.1. Admin GUI Client

Клиент администратора.

Функции:

* авторизация пользователя;
* отображение universal dashboard;
* отображение карточек ВМ;
* live-preview;
* smart connect;
* rescue mode;
* открытие RDP/VNC/SPICE/WebMKS/serial сессий;
* обработка keyboard/mouse input;
* локальный clipboard;
* отображение audit/session policy;
* reconnect;
* diagnostics UI.

## 4.2. Control Plane / Broker

Центральный сервис управления.

Функции:

* tenant management;
* identity;
* SSO/MFA;
* RBAC;
* ACL;
* provider registry;
* host registry;
* VM registry;
* session broker;
* policy engine;
* approval workflow;
* audit;
* fleet health;
* update orchestration;
* relay coordination.

## 4.3. Relay / Edge Network

Сетевой слой для соединения Client и Provider Connector через NAT/firewall.

Функции:

* direct connection where possible;
* relay fallback;
* reverse tunnel;
* regional routing;
* session multiplexing;
* bandwidth limits;
* reconnect;
* session token validation.

## 4.4. Node Agent

Локальный агент, который устанавливается рядом с virtualization platform.

Варианты:

```text
Hyper-V:
  Windows Service на Hyper-V host.

VMware:
  Connector рядом с vCenter/ESXi или cloud/self-hosted connector, имеющий доступ к vSphere API.

Proxmox:
  Connector рядом с Proxmox cluster или на отдельной машине с доступом к Proxmox API.

libvirt/KVM:
  Linux service на KVM host или management node с доступом к libvirt.
```

Node Agent не должен быть guest agent. Он устанавливается на host/management-layer, а не внутрь ВМ.

## 4.5. Provider Connector

Provider-specific модуль, который реализует единый provider API.

Примеры:

```text
provider_hyperv
provider_vmware
provider_proxmox
provider_libvirt
provider_xen
```

---

# 5. Универсальная модель данных

## 5.1. ProviderType

```rust
pub enum ProviderType {
    HyperV,
    VMware,
    Proxmox,
    Libvirt,
    QemuStandalone,
    Xen,
    XcpNg,
    AzureLocal,
    Custom(String),
}
```

## 5.2. HostInfo

```rust
pub struct HostInfo {
    pub host_id: String,
    pub provider_type: ProviderType,
    pub provider_native_id: String,
    pub display_name: String,
    pub hostname: Option<String>,
    pub cluster_id: Option<String>,
    pub management_endpoint: Option<String>,
    pub os_version: Option<String>,
    pub hypervisor_version: Option<String>,
    pub connector_version: Option<String>,
    pub health: HostHealth,
    pub last_seen_at: DateTimeUtc,
    pub provider_metadata: serde_json::Value,
}
```

## 5.3. VmInfo

```rust
pub struct VmInfo {
    pub vm_id: String,
    pub provider_type: ProviderType,
    pub provider_native_id: String,
    pub host_id: String,
    pub cluster_id: Option<String>,
    pub owner_node: Option<String>,
    pub name: String,
    pub power_state: PowerState,
    pub guest_os: Option<String>,
    pub guest_ips: Vec<String>,
    pub cpu_count: Option<u32>,
    pub memory_mb: Option<u64>,
    pub uptime_seconds: Option<u64>,
    pub tools_status: Option<ToolsStatus>,
    pub checkpoint_count: Option<u32>,
    pub tags: Vec<String>,
    pub recommended_mode: SessionMode,
    pub capabilities: VmCapabilityGraph,
    pub provider_metadata: serde_json::Value,
}
```

## 5.4. PowerState

```rust
pub enum PowerState {
    Running,
    Stopped,
    Paused,
    Suspended,
    Saved,
    Starting,
    Stopping,
    Resetting,
    Unknown,
}
```

## 5.5. ToolsStatus

```rust
pub enum ToolsStatus {
    Running,
    NotRunning,
    NotInstalled,
    Outdated,
    Unknown,
    NotApplicable,
}
```

## 5.6. SessionMode

```rust
pub enum SessionMode {
    Offline,
    PreviewOnly,
    BasicRescue,
    RdpRelay,
    VncConsole,
    SpiceConsole,
    WebConsole,
    WebMksConsole,
    SerialConsole,
    EnhancedSession,
    ProviderNativeConsole,
    Experimental(String),
}
```

## 5.7. Capability

```rust
pub enum CapabilityState {
    Available,
    Degraded,
    BlockedByPolicy,
    Unsupported,
    Unknown,
    Experimental,
}

pub struct Capability {
    pub state: CapabilityState,
    pub confidence: u8,
    pub reason_code: String,
    pub human_reason: String,
    pub remediation: Option<String>,
}
```

## 5.8. VmCapabilityGraph

```rust
pub struct VmCapabilityGraph {
    pub vm_id: String,
    pub provider_type: ProviderType,

    pub preview: Capability,
    pub console: Capability,
    pub rescue_input: Capability,

    pub rdp_relay: Capability,
    pub vnc_console: Capability,
    pub spice_console: Capability,
    pub web_console: Capability,
    pub webmks_console: Capability,
    pub serial_console: Capability,
    pub enhanced_session: Capability,

    pub clipboard: Capability,
    pub file_transfer: Capability,
    pub snapshots: Capability,
    pub power_control: Capability,
    pub recording: Capability,

    pub experimental: Vec<ExperimentalCapability>,
    pub constraints: Vec<Constraint>,
    pub recommended_mode: SessionMode,
}
```

## 5.9. ExperimentalCapability

```rust
pub struct ExperimentalCapability {
    pub name: String,
    pub state: CapabilityState,
    pub feature_flag: String,
    pub reason_code: String,
    pub confidence: u8,
}
```

---

# 6. Provider API

## 6.1. Назначение

Provider API — главный контракт между core и provider-specific реализациями.

Core вызывает только этот API и не импортирует provider-specific модули напрямую.

## 6.2. Rust trait

```rust
#[async_trait::async_trait]
pub trait VirtualizationProvider: Send + Sync {
    fn provider_type(&self) -> ProviderType;

    async fn list_hosts(&self) -> Result<Vec<HostInfo>>;
    async fn get_host(&self, host_id: &str) -> Result<HostInfo>;

    async fn list_vms(&self, host_id: &str) -> Result<Vec<VmInfo>>;
    async fn get_vm(&self, vm_id: &str) -> Result<VmInfo>;

    async fn get_capabilities(&self, vm_id: &str) -> Result<VmCapabilityGraph>;

    async fn open_preview_stream(&self, req: PreviewRequest) -> Result<PreviewStream>;
    async fn open_console_session(&self, req: ConsoleRequest) -> Result<ConsoleSession>;

    async fn start_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn shutdown_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn poweroff_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn reset_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn suspend_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn resume_vm(&self, vm_id: &str) -> Result<TaskId>;

    async fn list_snapshots(&self, vm_id: &str) -> Result<Vec<SnapshotInfo>>;
    async fn create_snapshot(&self, vm_id: &str, req: SnapshotRequest) -> Result<TaskId>;
    async fn revert_snapshot(&self, vm_id: &str, snapshot_id: &str) -> Result<TaskId>;
    async fn delete_snapshot(&self, vm_id: &str, snapshot_id: &str) -> Result<TaskId>;

    async fn get_metrics(&self, req: MetricsRequest) -> Result<ProviderMetrics>;
}
```

## 6.3. Provider Registry

```rust
pub struct ProviderRegistry {
    providers: HashMap<ProviderType, Arc<dyn VirtualizationProvider>>,
}

impl ProviderRegistry {
    pub fn register(&mut self, provider: Arc<dyn VirtualizationProvider>) {
        self.providers.insert(provider.provider_type(), provider);
    }

    pub fn get(&self, provider_type: ProviderType) -> Option<Arc<dyn VirtualizationProvider>> {
        self.providers.get(&provider_type).cloned()
    }
}
```

## 6.4. Запрещено

В `agent-core`, `broker`, `client-ui` запрещено импортировать:

```text
hyperv::*
vmware::*
proxmox::*
libvirt::*
```

Разрешено импортировать только:

```text
provider_api::*
core_types::*
protocol::*
```

---

# 7. Provider-specific реализации

---

## 7.1. Hyper-V Provider

### 7.1.1. Назначение

Первый provider платформы. Реализует управление ВМ Hyper-V через Windows host-level API.

### 7.1.2. Deployment

Hyper-V Connector устанавливается как Windows Service на Hyper-V host.

### 7.1.3. Основные backend’ы

```text
- WMI/CIM Inventory
- WMI Thumbnail
- WMI Keyboard Rescue
- RDP Relay
- Enhanced Session
- AF_HYPERV Experimental Backend
```

### 7.1.4. Inventory

Использовать namespace:

```text
root\virtualization\v2
```

Основные классы:

```text
Msvm_ComputerSystem
Msvm_SummaryInformation
Msvm_VirtualSystemSettingData
Msvm_VirtualSystemManagementService
```

### 7.1.5. Thumbnail

Использовать:

```text
GetVirtualSystemThumbnailImage
```

Pipeline:

```text
WMI Thumbnail
  -> raw RGB565
  -> decode RGB/RGBA
  -> resize
  -> duplicate frame detection
  -> encode WebP/JPEG/PNG
  -> stream to client
```

Target:

```text
Dashboard:
  1–2 FPS

Rescue:
  1–2 FPS

15+ VM:
  host CPU overhead target <= 5%
```

### 7.1.6. Enhanced Session

Enhanced Session нельзя определять только по heartbeat.

Корректная логика:

```text
Enhanced Session candidate =
  VM running
  AND heartbeat OK
  AND EnhancedSessionModeState confirms availability
  AND host policy allows enhanced session
  AND guest requirements are met
  AND product policy allows backend
```

### 7.1.7. Basic Rescue

Basic Rescue должен поддерживать:

```text
- Ctrl+Alt+Del
- TypeText
- TypeKey
- PressKey / ReleaseKey where supported
- paste-as-keystrokes
```

Назначение:

```text
- восстановить сеть;
- включить RDP;
- исправить boot/login состояние;
- выполнить минимальные действия без guest network.
```

### 7.1.8. RDP Relay

Если у guest есть IP и RDP/xRDP доступен с host:

```text
Admin Client ⇄ Broker/Relay ⇄ Hyper-V Connector ⇄ VM:3389
```

### 7.1.9. AF_HYPERV Experimental

AF_HYPERV является experimental.

До POC запрещено обещать:

```text
- guaranteed display stream;
- guaranteed RDP over VMBus;
- 30–60 FPS;
- clipboard;
- input;
- универсальность для всех guest OS.
```

POC checklist:

```text
1. Найден documented/legal-safe endpoint.
2. Подтвержден connect.
3. Подтвержден disconnect.
4. Подтвержден reconnect.
5. Подтвержден duplex stream.
6. Описан protocol framing.
7. Подтвержден display path.
8. Подтвержден input path.
9. Clipboard либо подтвержден, либо disabled.
10. Работает после guest reboot.
11. Работает после pause/resume.
12. Работает после save/restore.
13. Live migration behavior описан.
14. Worker crash не роняет Agent Core.
15. Backend отключается через policy.
```

---

## 7.2. VMware Provider

### 7.2.1. Назначение

Provider для VMware vSphere / ESXi / vCenter.

### 7.2.2. Deployment

Варианты:

```text
- Connector рядом с vCenter;
- Connector внутри management network;
- cloud/self-hosted connector с доступом к vSphere API;
- direct ESXi mode для малых инсталляций.
```

### 7.2.3. Основные backend’ы

```text
- vSphere API inventory
- power operations
- snapshots
- screenshot/preview where available
- MKS / WebMKS console
- VMRC launch where allowed
- RDP Relay optional
```

### 7.2.4. Console

VMware Provider должен поддерживать:

```text
WebMKSConsole
ProviderNativeConsole
RdpRelay
PreviewOnly
```

### 7.2.5. Capability mapping

```text
VMware Tools running:
  improves guest metadata and operations.

WebMKS ticket available:
  WebMksConsole = Available.

No console permission:
  WebMksConsole = BlockedByPolicy.

Guest IP + RDP reachable:
  RdpRelay = Available.
```

### 7.2.6. Snapshots

Поддерживать:

```text
- list snapshots;
- create snapshot;
- revert snapshot;
- delete snapshot;
- snapshot tree metadata.
```

### 7.2.7. Provider metadata

VMware-specific данные хранить в `provider_metadata`:

```json
{
  "vcenter": "vc01.example.local",
  "moid": "vm-123",
  "datacenter": "DC1",
  "cluster": "Cluster-A",
  "datastore": "datastore1",
  "tools_status": "running"
}
```

---

## 7.3. Proxmox Provider

### 7.3.1. Назначение

Provider для Proxmox VE.

### 7.3.2. Deployment

Варианты:

```text
- Connector на Proxmox node;
- Connector рядом с Proxmox cluster;
- external connector с доступом к Proxmox API.
```

### 7.3.3. Основные backend’ы

```text
- Proxmox API inventory
- power operations
- snapshots
- VNC/noVNC console
- SPICE console where available
- serial console where available
- RDP Relay optional
```

### 7.3.4. Console

Proxmox Provider должен поддерживать:

```text
VncConsole
WebConsole
SpiceConsole
SerialConsole
RdpRelay
PreviewOnly
```

### 7.3.5. Capability mapping

```text
noVNC available:
  VncConsole = Available.

SPICE configured:
  SpiceConsole = Available.

VM has guest IP + RDP reachable:
  RdpRelay = Available.

No reachable API:
  all console modes = Unknown/Unavailable.
```

### 7.3.6. Provider metadata

```json
{
  "node": "pve01",
  "vmid": 101,
  "type": "qemu",
  "storage": "local-lvm",
  "ha_state": "started"
}
```

---

## 7.4. libvirt / KVM / QEMU Provider

### 7.4.1. Назначение

Provider для KVM/QEMU через libvirt.

### 7.4.2. Deployment

Варианты:

```text
- Linux service на KVM host;
- Connector на management node с доступом к libvirt;
- remote libvirt connection.
```

### 7.4.3. Основные backend’ы

```text
- libvirt inventory
- domain power operations
- snapshots where supported
- VNC console
- SPICE console where supported
- serial console
- RDP Relay optional
```

### 7.4.4. Console

Provider должен читать domain XML и определять доступные graphics devices:

```text
graphics type='vnc'
graphics type='spice'
serial console
```

### 7.4.5. Capability mapping

```text
VNC graphics configured:
  VncConsole = Available.

SPICE graphics configured:
  SpiceConsole = Available or Degraded depending on environment.

Serial configured:
  SerialConsole = Available.

Guest IP + RDP reachable:
  RdpRelay = Available.
```

### 7.4.6. Provider metadata

```json
{
  "domain_name": "vm01",
  "domain_uuid": "uuid",
  "uri": "qemu:///system",
  "graphics": ["vnc"],
  "emulator": "qemu-system-x86_64"
}
```

---

## 7.5. Future Providers

Платформа должна позволять добавить:

```text
- Xen
- XCP-ng
- Azure Local
- Nutanix AHV
- OpenStack
- oVirt
- Custom provider
```

без изменения core-сущностей.

---

# 8. Universal Capability Engine

## 8.1. Назначение

Capability Engine строит унифицированный граф возможностей для каждой VM вне зависимости от provider’а.

## 8.2. Источники данных

Capability Engine использует:

```text
- provider API;
- VM metadata;
- host metadata;
- guest tools state;
- network probes;
- console probes;
- policy engine;
- user permissions;
- tenant settings;
- feature flags.
```

## 8.3. Recommended mode

Система должна выбрать recommended mode по приоритету:

```text
1. ProviderNativeConsole / WebMKS / Enhanced / VNC / SPICE,
   если backend supported и policy разрешает.

2. RdpRelay,
   если guest network доступна и RDP/xRDP включен.

3. BasicRescue,
   если provider поддерживает preview + rescue input.

4. PreviewOnly.

5. Offline.
```

Provider может переопределять порядок через provider capability score.

## 8.4. Explainability

Каждая недоступная функция должна иметь reason.

Пример:

```text
VNC Console unavailable:
VM has no VNC graphics device configured.

RDP Relay unavailable:
Guest IP not detected or port 3389 closed.

Enhanced Session unavailable:
Provider is not Hyper-V.

WebMKS unavailable:
User lacks console permission.

Preview degraded:
Provider supports screenshot only, not live stream.
```

---

# 9. Session Broker

## 9.1. Назначение

Session Broker отвечает за создание и управление сессиями.

## 9.2. OpenSession flow

```text
1. Client requests session.
2. Broker checks identity.
3. Broker checks RBAC/ACL.
4. Broker requests capability graph.
5. Policy Engine evaluates allowed modes.
6. Broker selects backend.
7. Broker creates short-lived session token.
8. Relay path is negotiated.
9. Connector opens provider-specific session.
10. Client receives stream metadata.
```

## 9.3. Session states

```rust
pub enum SessionState {
    Requested,
    WaitingApproval,
    Opening,
    Active,
    Degraded,
    Reconnecting,
    Closing,
    Closed,
    Failed,
    Revoked,
}
```

## 9.4. Session object

```rust
pub struct Session {
    pub session_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub provider_type: ProviderType,
    pub host_id: String,
    pub vm_id: String,
    pub selected_mode: SessionMode,
    pub state: SessionState,
    pub policy: SessionPolicy,
    pub created_at: DateTimeUtc,
    pub expires_at: DateTimeUtc,
}
```

## 9.5. Session continuity

Система должна поддерживать:

```text
- client reconnect;
- relay reconnect;
- connector reconnect;
- session timeout;
- backend fallback;
- VM migration handling where provider supports it.
```

---

# 10. Preview Engine

## 10.1. Назначение

Единая подсистема preview для разных provider’ов.

## 10.2. Preview modes

```text
LiveThumbnail:
  low-FPS stream, e.g. Hyper-V WMI thumbnail.

ScreenshotPolling:
  periodic screenshots from provider API.

ConsoleFrameSampling:
  sampling frames from VNC/SPICE/MKS where available.

Unavailable:
  provider не поддерживает preview.
```

## 10.3. PreviewRequest

```rust
pub struct PreviewRequest {
    pub vm_id: String,
    pub width: u32,
    pub height: u32,
    pub target_fps: f32,
    pub quality: PreviewQuality,
}
```

## 10.4. Performance targets

```text
Dashboard:
  1–2 FPS for active cards.

Inactive dashboard:
  0.2–0.5 FPS.

Selected VM preview:
  1–2 FPS.

Duplicate frames:
  skip identical frames.

Slow client:
  drop stale frames.
```

## 10.5. Encoding

Поддержать:

```text
- WebP
- JPEG
- PNG
- raw RGBA for local/debug mode
```

---

# 11. Console Backends

## 11.1. RDP Relay

Универсальный backend, если guest имеет IP и RDP/xRDP доступен.

```text
Client ⇄ Broker/Relay ⇄ Connector ⇄ Guest VM RDP endpoint
```

Поддерживает:

```text
- keyboard;
- mouse;
- clipboard via RDP clipboard channel;
- reconnect;
- resize;
- recording where policy allows.
```

## 11.2. VNC Console

Используется для:

```text
- Proxmox;
- libvirt/KVM;
- QEMU;
- other providers with VNC display.
```

Client должен иметь:

```text
- VNC decoder;
- input mapping;
- resize handling where supported;
- optional clipboard if backend supports it.
```

## 11.3. SPICE Console

Используется для:

```text
- Proxmox where configured;
- libvirt/KVM where configured;
- QEMU where configured.
```

Требования:

```text
- SPICE adapter;
- keyboard/mouse;
- display resize;
- clipboard only where supported and policy allows.
```

SPICE поддержка должна быть optional, так как в некоторых современных окружениях она может быть ограничена или заменена VNC.

## 11.4. WebMKS / MKS Console

Используется для VMware.

Требования:

```text
- request console ticket via VMware provider;
- open WebMKS session;
- enforce user permission;
- support reconnect where provider allows;
- hide ticket from user logs;
- expire token safely.
```

## 11.5. Serial Console

Используется для:

```text
- Linux rescue;
- Proxmox;
- libvirt/KVM;
- cloud-like environments;
- minimal console access.
```

Требования:

```text
- terminal UI;
- input/output stream;
- audit metadata;
- optional recording as text log.
```

## 11.6. Basic Rescue

Generic fallback mode.

Возможен только если provider поддерживает:

```text
preview + limited input
```

Примеры:

```text
Hyper-V:
  WMI thumbnail + WMI keyboard.

Other providers:
  screenshot/preview + send key API if available.
```

---

# 12. Clipboard

## 12.1. Общая модель

Clipboard всегда управляется политикой.

## 12.2. Client Clipboard

Локальный clipboard читается только на стороне Admin GUI Client.

Для Rust GUI:

```text
arboard используется только в client-ui.
```

Запрещено:

```text
использовать clipboard API в Node Agent / Windows Service / Linux daemon.
```

## 12.3. Clipboard modes

```rust
pub enum ClipboardMode {
    Disabled,
    TextOnly,
    TextWithConfirmation,
    FullProviderClipboard,
    PasteAsKeystrokesOnly,
}
```

## 12.4. Backend mapping

```text
RDP:
  RDP clipboard virtual channel.

VNC:
  clipboard only if VNC extension/backend supports it.

SPICE:
  clipboard if SPICE channel supports it and policy allows.

WebMKS:
  provider-native clipboard if supported.

BasicRescue:
  paste-as-keystrokes only.

PreviewOnly:
  clipboard unavailable.
```

## 12.5. Security

Требования:

```text
- max clipboard size;
- text-only default;
- file clipboard disabled by default;
- confirmation for sensitive VMs;
- metadata audit;
- no plaintext clipboard in audit by default.
```

---

# 13. Input

## 13.1. Универсальные input events

```rust
pub enum InputEvent {
    KeyDown { code: KeyCode },
    KeyUp { code: KeyCode },
    MouseMove { x: i32, y: i32 },
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    MouseWheel { delta: i32 },
    TypeText { text: String, redact_audit: bool },
    SpecialSequence { sequence: SpecialKeySequence },
}
```

## 13.2. Special sequences

```text
Ctrl+Alt+Del
Ctrl+Alt+End
Enter
Escape
Tab
Function keys
Boot menu keys
Custom sequence
```

## 13.3. Provider mapping

Каждый provider должен преобразовывать универсальные input events в свой backend-specific формат.

---

# 14. Power Management

## 14.1. Универсальные операции

```text
Start
GracefulShutdown
PowerOff
Reset
Suspend
Resume
Pause
Unpause
Save
```

## 14.2. Policy

Опасные операции должны требовать:

```text
- authorization;
- confirmation;
- audit;
- reason;
- optional MFA;
- optional approval.
```

Опасные операции:

```text
PowerOff
Reset
RevertSnapshot
DeleteSnapshot
```

---

# 15. Snapshots / Checkpoints

## 15.1. Универсальная модель

```rust
pub struct SnapshotInfo {
    pub snapshot_id: String,
    pub provider_native_id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<DateTimeUtc>,
    pub parent_id: Option<String>,
    pub is_current: Option<bool>,
    pub provider_metadata: serde_json::Value,
}
```

## 15.2. Операции

```text
List
Create
Revert
Delete
Rename
```

## 15.3. Provider mapping

```text
Hyper-V:
  checkpoints.

VMware:
  snapshots.

Proxmox:
  snapshots.

libvirt:
  snapshots where supported.
```

---

# 16. Control Plane / Broker

## 16.1. Tenant model

Поддержать multi-tenant модель.

```rust
pub struct Tenant {
    pub tenant_id: String,
    pub name: String,
    pub plan: Option<String>,
    pub settings: TenantSettings,
}
```

## 16.2. Identity

Поддержать:

```text
- local accounts;
- SSO;
- MFA;
- service accounts;
- API tokens;
- device trust.
```

## 16.3. RBAC roles

```text
Viewer:
  dashboard and preview only.

Operator:
  connect and rescue where allowed.

PowerOperator:
  power operations.

SnapshotOperator:
  snapshot/checkpoint operations.

Admin:
  manage users, policies, providers.

Auditor:
  view audit and recordings.

BreakGlass:
  emergency access with strict audit.
```

## 16.4. ACL

ACL должны назначаться на:

```text
tenant
provider
cluster
host
VM
tag
user
group
role
```

## 16.5. Policy Engine

Policy dimensions:

```text
- provider type;
- VM tags;
- user role;
- time window;
- network location;
- device trust;
- session mode;
- clipboard;
- recording;
- destructive actions;
- experimental backend usage.
```

## 16.6. JIT Access

Временный доступ:

```text
- request reason;
- allowed VM;
- allowed backend;
- max duration;
- approver;
- expiration;
- audit id.
```

## 16.7. Approval Workflow

```text
1. User requests privileged access.
2. Broker creates approval request.
3. Approver approves/denies.
4. Broker issues short-lived session token.
5. Session starts.
6. Audit trail is generated.
```

---

# 17. Audit

## 17.1. Audit events

Логировать:

```text
- login;
- logout;
- provider added/removed;
- host connected/disconnected;
- VM discovered;
- session requested;
- session approved/denied;
- session started;
- session ended;
- backend selected;
- fallback occurred;
- clipboard used/blocked;
- power action;
- snapshot action;
- policy changed;
- experimental backend enabled.
```

## 17.2. AuditEvent

```rust
pub struct AuditEvent {
    pub event_id: String,
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub provider_type: Option<ProviderType>,
    pub host_id: Option<String>,
    pub vm_id: Option<String>,
    pub session_id: Option<String>,
    pub event_type: String,
    pub timestamp: DateTimeUtc,
    pub severity: AuditSeverity,
    pub metadata: serde_json::Value,
}
```

## 17.3. Sensitive data

По умолчанию не хранить:

```text
- plaintext passwords;
- clipboard content;
- full typed text;
- VM screen video unless recording policy enabled.
```

---

# 18. Recording и Forensics

## 18.1. Recording levels

```text
Level 0:
  disabled.

Level 1:
  metadata only.

Level 2:
  event timeline.

Level 3:
  video/session recording.

Level 4:
  full forensic mode with strict policy.
```

## 18.2. Forensic timeline

Пример:

```text
14:03 Session requested
14:04 Approved by admin
14:05 Connected via BasicRescue
14:06 Ctrl+Alt+Del sent
14:08 Clipboard blocked by policy
14:10 Network restored
14:11 Switched to RDP Relay
14:30 Session ended
```

---

# 19. Security

## 19.1. Transport security

Требования:

```text
- TLS 1.3;
- mTLS for connector/broker;
- short-lived session tokens;
- token binding to user/device/session;
- replay protection;
- per-session keys;
- optional E2E encryption for data plane.
```

## 19.2. Connector security

Connector должен:

```text
- использовать signed binary;
- проверять signed updates;
- хранить ключи через OS secure storage;
- не принимать arbitrary shell commands;
- иметь allowlist операций;
- иметь rate limits;
- логировать privileged actions;
- работать с minimum required privileges where possible.
```

## 19.3. Experimental security

Experimental backend’ы:

```text
- disabled by default;
- allowed only per-tenant/per-host/per-VM;
- require explicit warning;
- require audit;
- can be killed independently;
- cannot bypass policy engine.
```

## 19.4. Secret handling

Запрещено:

```text
- хранить provider passwords в plaintext;
- писать credentials в logs;
- показывать console tickets в UI/logs;
- хранить clipboard content без explicit policy.
```

---

# 20. Relay / Edge Network

## 20.1. Modes

```text
Direct:
  Client connects directly to Connector.

Reverse tunnel:
  Connector keeps outbound connection.

Relay:
  Client and Connector both connect to Relay.

Hybrid:
  start with relay, upgrade to direct if possible.
```

## 20.2. Requirements

```text
- session token validation;
- tenant isolation;
- connection multiplexing;
- bandwidth accounting;
- idle timeout;
- reconnect;
- regional routing;
- backpressure.
```

## 20.3. Data plane

Data plane channels:

```text
control
preview
console
input
clipboard
metrics
audit
recording
```

---

# 21. Сетевой протокол приложения

## 21.1. Требования

Протокол должен поддерживать:

```text
- multiplexed streams;
- request/response;
- server push;
- binary frames;
- backpressure;
- versioning;
- compression;
- heartbeat;
- reconnect;
- session resume.
```

## 21.2. Frame header

```rust
pub struct FrameHeader {
    pub protocol_version: u16,
    pub channel_id: u32,
    pub stream_id: u64,
    pub message_type: u16,
    pub flags: u16,
    pub sequence: u64,
    pub payload_len: u32,
}
```

## 21.3. Protocol versioning

Требования:

```text
- backward compatibility N-1;
- feature negotiation;
- provider capability negotiation;
- graceful unsupported version error.
```

---

# 22. Admin GUI Client

## 22.1. Dashboard

Dashboard должен показывать:

```text
- providers;
- clusters;
- hosts;
- VMs;
- live previews;
- power states;
- recommended modes;
- capability badges;
- risk badges;
- last session;
- audit status;
- search/filter/group.
```

## 22.2. VM Card

```text
VM Name
Provider
Host
Cluster
Power State
Preview
Recommended Mode
Connect
Rescue
Snapshots
Explain
More Actions
```

## 22.3. Provider details tab

Provider-specific данные показываются отдельно.

Hyper-V:

```text
Heartbeat
EnhancedSessionModeState
Integration Services
Generation
```

VMware:

```text
vCenter
MOID
VMware Tools status
Datastore
Cluster
```

Proxmox:

```text
Node
VMID
QEMU/LXC
HA state
```

libvirt:

```text
Domain UUID
URI
Graphics type
Serial console
```

## 22.4. Smart Connect

Алгоритм:

```text
1. User clicks Connect.
2. Client requests capability graph.
3. Broker applies policy.
4. Recommended backend is selected.
5. UI shows warning if needed.
6. Session starts.
7. Client opens matching decoder/adapter.
```

## 22.5. Explain UX

UI должен объяснять недоступность функций.

Пример:

```text
RDP Relay unavailable:
Guest IP is not detected.

Recommended:
Open VNC Console or Provider Console.
```

---

# 23. Node Agent / Connector

## 23.1. Общие требования

Connector должен:

```text
- работать как service/daemon;
- не зависеть от GUI;
- держать outbound channel к Broker/Relay;
- иметь local policy cache;
- иметь audit buffer;
- иметь metrics;
- поддерживать signed update;
- поддерживать plugin/provider loading.
```

## 23.2. Worker isolation

```text
Connector Core
  ├── Provider Worker
  ├── Preview Worker
  ├── Console Worker
  ├── Relay Worker
  ├── Metrics Worker
  └── Experimental Worker
```

Падение worker не должно ронять Connector Core.

## 23.3. Local cache

Connector должен кэшировать:

```text
- last policy;
- provider config;
- host identity;
- pending audit events;
- health data.
```

## 23.4. Offline behavior

Если Broker недоступен:

```text
- новые privileged sessions запрещены или ограничены policy;
- уже активные сессии живут до TTL;
- audit сохраняется локально;
- после восстановления связи audit отправляется.
```

---

# 24. Observability

## 24.1. Metrics

Собирать:

```text
Connector:
  CPU/RAM
  provider API latency
  preview FPS
  frames dropped
  console throughput
  reconnect count
  worker crashes

Client:
  decode FPS
  render FPS
  input latency
  network RTT
  clipboard failures

Broker:
  session count
  auth failures
  policy latency
  relay usage
  audit backlog
```

## 24.2. Logs

Structured logs:

```text
timestamp
level
tenant_id
provider_type
host_id
vm_id
session_id
component
event
fields
```

## 24.3. Health checks

```text
Connector health
Provider API health
Broker health
Relay health
Database health
Queue health
```

---

# 25. API-контракты

## 25.1. RegisterProviderConnector

Request:

```json
{
  "tenant_id": "tenant-1",
  "provider_type": "hyperv",
  "connector_name": "HV-01 Connector",
  "enrollment_token": "one-time-token"
}
```

Response:

```json
{
  "connector_id": "connector-001",
  "host_id": "host-001",
  "status": "registered"
}
```

## 25.2. GetProviders

Response:

```json
{
  "providers": [
    {
      "provider_type": "hyperv",
      "display_name": "Hyper-V",
      "status": "available"
    },
    {
      "provider_type": "vmware",
      "display_name": "VMware vSphere",
      "status": "planned"
    }
  ]
}
```

## 25.3. GetVMs

Request:

```json
{
  "provider_type": "hyperv",
  "host_id": "host-001"
}
```

Response:

```json
{
  "vms": [
    {
      "vm_id": "vm-001",
      "provider_type": "hyperv",
      "name": "DC-01",
      "power_state": "Running",
      "recommended_mode": "BasicRescue"
    }
  ]
}
```

## 25.4. GetCapabilityGraph

Request:

```json
{
  "vm_id": "vm-001"
}
```

Response:

```json
{
  "vm_id": "vm-001",
  "provider_type": "hyperv",
  "recommended_mode": "BasicRescue",
  "capabilities": {
    "preview": {
      "state": "Available",
      "confidence": 100,
      "reason_code": "PREVIEW_AVAILABLE"
    },
    "rdp_relay": {
      "state": "Unsupported",
      "confidence": 90,
      "reason_code": "NO_GUEST_IP"
    },
    "vnc_console": {
      "state": "Unsupported",
      "confidence": 100,
      "reason_code": "PROVIDER_NOT_SUPPORTED"
    }
  }
}
```

## 25.5. OpenSession

Request:

```json
{
  "vm_id": "vm-001",
  "requested_mode": "Auto",
  "reason": "Network recovery",
  "client_capabilities": {
    "rdp": true,
    "vnc": true,
    "spice": false,
    "webmks": true,
    "clipboard_text": true
  }
}
```

Response:

```json
{
  "session_id": "session-001",
  "selected_mode": "BasicRescue",
  "expires_at": "2026-06-15T11:00:00Z",
  "policy": {
    "clipboard": "PasteAsKeystrokesOnly",
    "recording": "EventLogOnly",
    "max_duration_seconds": 3600
  },
  "transport": {
    "relay_required": true,
    "token": "short-lived-token"
  }
}
```

---

# 26. Производительность

## 26.1. Dashboard

Target:

```text
15+ VM live previews:
  host/connector overhead <= 5% where provider supports efficient preview.

50+ VMs:
  adaptive polling.

100+ VMs:
  preview only for visible cards.
```

## 26.2. Console

Target:

```text
RDP/VNC/SPICE/WebMKS:
  >= 30 FPS target where backend supports it.

60 FPS:
  best effort.

Input lag:
  < 50 ms target in good network conditions.

PreviewOnly:
  1–2 FPS, no interactive FPS guarantee.
```

## 26.3. Scaling

Broker should support:

```text
Phase 1:
  10 connectors
  500 VMs
  50 concurrent sessions

Phase 2:
  100 connectors
  10,000 VMs
  1,000 concurrent sessions

Phase 3:
  multi-region scale
```

---

# 27. Acceptance Criteria

## 27.1. Universal Core

Core code не содержит прямой зависимости от Hyper-V, VMware, Proxmox или libvirt.

Проверка:

```text
agent-core, broker, client-ui импортируют только provider_api/core_types/protocol.
```

## 27.2. Provider Plugin

Hyper-V реализован как provider module:

```text
providers/hyperv
```

и может быть отключен/заменен без изменения core.

## 27.3. No Guest Agent

В гостевые ОС не устанавливается сторонний агент продукта.

## 27.4. Dashboard

Dashboard показывает ВМ минимум одного provider’а, но модель UI универсальна и готова к нескольким provider’ам.

## 27.5. Capability Graph

Для каждой VM система возвращает capability graph с:

```text
preview
console modes
rescue
rdp relay
snapshots
power control
clipboard
recording
recommended mode
reason codes
```

## 27.6. Smart Connect

Кнопка Connect выбирает backend автоматически через Broker и Capability Engine.

## 27.7. Hyper-V MVP

Hyper-V provider должен поддерживать:

```text
- inventory;
- thumbnails;
- power operations;
- checkpoints;
- BasicRescue where supported;
- RDP Relay;
- Enhanced candidate detection;
- AF_HYPERV as experimental disabled by default.
```

## 27.8. Future Provider Readiness

Должно быть возможно добавить VMware provider без изменения:

```text
- VmInfo;
- HostInfo;
- SessionMode;
- CapabilityGraph;
- Broker OpenSession flow;
- Client VM Card.
```

## 27.9. Security

Должны быть реализованы:

```text
- auth;
- RBAC;
- session tokens;
- audit;
- policy checks;
- clipboard policy;
- destructive action confirmation.
```

## 27.10. Stability

Connector должен:

```text
- работать как service/daemon;
- переживать worker crash;
- иметь reconnect;
- иметь audit buffer;
- не зависеть от GUI.
```

---

# 28. Этапы реализации

## Phase 0: Architecture Refactor Guardrails

Цель: заложить универсальный фундамент до углубления в Hyper-V.

Deliverables:

```text
- core-types crate
- provider-api crate
- provider-registry
- universal VmInfo
- universal CapabilityGraph
- universal SessionMode
- protocol with provider_type
- Hyper-V moved into providers/hyperv
```

## Phase 1: Hyper-V MVP Provider

Функции:

```text
- Hyper-V Connector Windows Service
- WMI inventory
- WMI thumbnail
- power operations
- checkpoints
- BasicRescue
- RDP Relay
- basic GUI dashboard
- Broker minimal
- Relay minimal
- audit metadata
```

## Phase 2: Universal Broker + Client

Функции:

```text
- Provider Registry in Broker
- universal dashboard
- universal session broker
- policy engine
- RBAC/ACL
- JIT access
- clipboard policy
- session audit
```

## Phase 3: VMware Provider

Функции:

```text
- vSphere inventory
- power operations
- snapshots
- WebMKS/MKS console
- RDP Relay
- provider metadata
- capability mapping
```

## Phase 4: Proxmox Provider

Функции:

```text
- Proxmox API inventory
- power operations
- snapshots
- VNC/noVNC console
- SPICE where available
- serial console
- RDP Relay
```

## Phase 5: libvirt/KVM Provider

Функции:

```text
- libvirt inventory
- power operations
- snapshots where supported
- VNC console
- SPICE console
- serial console
- RDP Relay
```

## Phase 6: Enterprise Features

Функции:

```text
- SSO/MFA
- advanced policy engine
- approval workflow
- session recording
- forensic timeline
- multi-region relay
- cluster awareness
- signed updates
- self-hosted deployment
```

## Phase 7: Experimental Backends

Функции:

```text
- Hyper-V AF_HYPERV backend
- custom provider-native transports
- no-network interactive experiments
```

---

# 29. Риски

## 29.1. Provider Differences

Разные гипервизоры имеют разные возможности.

Митигация:

```text
CapabilityGraph вместо hardcoded feature assumptions.
```

## 29.2. Console Protocol Complexity

RDP, VNC, SPICE, WebMKS отличаются.

Митигация:

```text
Console adapter layer.
Provider-specific session backend.
Unified input abstraction.
```

## 29.3. Hyper-V AF_HYPERV Risk

AF_HYPERV может не дать production-ready display path.

Митигация:

```text
Experimental only.
Supported fallback через WMI/RDP/Enhanced.
```

## 29.4. Proxmox Console Integration Risk

noVNC/VNC proxy flow может отличаться между версиями.

Митигация:

```text
Provider version detection.
Capability probe.
Fallback to raw VNC/SPICE where possible.
```

## 29.5. SPICE Availability Risk

SPICE может быть недоступен или deprecated в части окружений.

Митигация:

```text
SPICE optional.
VNC primary fallback.
```

---

# 30. Требования к структуре репозитория

```text
crates/
  core-types/
  provider-api/
  protocol/
  broker/
  relay/
  policy-engine/
  audit/
  agent-core/
  client-ui/
  client-rdp/
  client-vnc/
  client-spice/
  client-webmks/

providers/
  hyperv/
    inventory/
    thumbnails/
    rescue/
    rdp-relay/
    enhanced/
    hvsocket-experimental/

  vmware/
    inventory/
    webmks/
    snapshots/
    rdp-relay/

  proxmox/
    inventory/
    vnc/
    spice/
    snapshots/

  libvirt/
    inventory/
    vnc/
    spice/
    serial/
    snapshots/
```

---

# 31. Запрещенные архитектурные решения

Запрещено:

```text
- зашивать Hyper-V в core;
- называть универсальные сущности HyperV*;
- хранить provider-specific поля в core struct без metadata;
- делать GUI завязанным на Hyper-V;
- делать Broker завязанным на Hyper-V;
- использовать AF_HYPERV как обязательный production backend;
- устанавливать guest agent;
- выполнять arbitrary shell commands через connector;
- хранить secrets в plaintext;
- писать console tickets в logs;
- использовать desktop clipboard в service/daemon.
```

---

# 32. Итоговое позиционирование продукта

Продукт должен позиционироваться как:

```text
Universal zero-trust VM access, rescue and control platform for multiple hypervisors.
```

На русском:

```text
Универсальная zero-trust платформа доступа, восстановления и управления ВМ на разных гипервизорах без установки сторонних агентов внутрь гостевых ОС.
```

Первый provider:

```text
Hyper-V
```

Следующие provider’ы:

```text
VMware vSphere
Proxmox VE
KVM/libvirt
```

Главный архитектурный принцип:

```text
Новые гипервизоры добавляются как Provider Connector.
Core, Broker и Client не переписываются.
```
# 33. Схема данных Broker / Control Plane

## 33.1. Общие требования

Control Plane должен хранить:

* tenants;
* users;
* groups;
* roles;
* providers;
* connectors;
* hosts;
* VMs;
* capabilities;
* sessions;
* policies;
* audit events;
* approvals;
* recordings metadata;
* update state;
* health metrics.

База данных должна быть спроектирована так, чтобы:

```text id="0h1d1o"
- один tenant мог иметь несколько provider’ов;
- один provider мог иметь несколько connector’ов;
- один connector мог обслуживать несколько hosts;
- одна VM могла мигрировать между hosts;
- VM могла иметь provider_native_id, отличающийся от global vm_id;
- provider-specific поля не ломали универсальную схему.
```

Рекомендуемый основной storage:

```text id="3eug3o"
PostgreSQL
```

Для stream/telemetry/event ingestion допускается:

```text id="08q1mv"
NATS / Kafka / Redis Streams / ClickHouse / OpenTelemetry backend
```

---

## 33.2. Таблица tenants

```sql id="zgwai0"
CREATE TABLE tenants (
    tenant_id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT UNIQUE NOT NULL,
    status TEXT NOT NULL,
    settings JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

---

## 33.3. Таблица users

```sql id="sxwcjr"
CREATE TABLE users (
    user_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    email TEXT NOT NULL,
    display_name TEXT,
    status TEXT NOT NULL,
    auth_provider TEXT NOT NULL,
    external_id TEXT,
    mfa_enabled BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, email)
);
```

---

## 33.4. Таблица groups

```sql id="adsgpg"
CREATE TABLE groups (
    group_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    name TEXT NOT NULL,
    external_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, name)
);
```

---

## 33.5. Таблица user_groups

```sql id="mki83b"
CREATE TABLE user_groups (
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    user_id UUID NOT NULL REFERENCES users(user_id),
    group_id UUID NOT NULL REFERENCES groups(group_id),
    PRIMARY KEY (tenant_id, user_id, group_id)
);
```

---

## 33.6. Таблица providers

```sql id="5v7qin"
CREATE TABLE providers (
    provider_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    provider_type TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL,
    config_ref TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Примеры `provider_type`:

```text id="putiqx"
hyperv
vmware
proxmox
libvirt
xen
custom
```

---

## 33.7. Таблица connectors

```sql id="x0ljog"
CREATE TABLE connectors (
    connector_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    provider_id UUID NOT NULL REFERENCES providers(provider_id),
    connector_name TEXT NOT NULL,
    connector_type TEXT NOT NULL,
    version TEXT NOT NULL,
    status TEXT NOT NULL,
    last_seen_at TIMESTAMPTZ,
    public_key TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

`connector_type`:

```text id="kgdb6p"
node_agent
management_connector
cloud_connector
relay_only
```

---

## 33.8. Таблица hosts

```sql id="gfsdgo"
CREATE TABLE hosts (
    host_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    provider_id UUID NOT NULL REFERENCES providers(provider_id),
    connector_id UUID REFERENCES connectors(connector_id),
    provider_type TEXT NOT NULL,
    provider_native_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    hostname TEXT,
    cluster_id UUID,
    health TEXT NOT NULL,
    os_version TEXT,
    hypervisor_version TEXT,
    last_seen_at TIMESTAMPTZ,
    provider_metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, provider_id, provider_native_id)
);
```

---

## 33.9. Таблица clusters

```sql id="r6dw0p"
CREATE TABLE clusters (
    cluster_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    provider_id UUID NOT NULL REFERENCES providers(provider_id),
    provider_type TEXT NOT NULL,
    provider_native_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    health TEXT NOT NULL,
    provider_metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, provider_id, provider_native_id)
);
```

---

## 33.10. Таблица vms

```sql id="q7f5iv"
CREATE TABLE vms (
    vm_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    provider_id UUID NOT NULL REFERENCES providers(provider_id),
    provider_type TEXT NOT NULL,
    provider_native_id TEXT NOT NULL,
    host_id UUID REFERENCES hosts(host_id),
    cluster_id UUID REFERENCES clusters(cluster_id),
    owner_node_id UUID REFERENCES hosts(host_id),
    name TEXT NOT NULL,
    power_state TEXT NOT NULL,
    guest_os TEXT,
    guest_ips JSONB NOT NULL DEFAULT '[]',
    tools_status TEXT,
    recommended_mode TEXT,
    tags JSONB NOT NULL DEFAULT '[]',
    provider_metadata JSONB NOT NULL DEFAULT '{}',
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, provider_id, provider_native_id)
);
```

---

## 33.11. Таблица vm_capabilities

```sql id="12lt2z"
CREATE TABLE vm_capabilities (
    vm_id UUID PRIMARY KEY REFERENCES vms(vm_id),
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    provider_type TEXT NOT NULL,
    capability_graph JSONB NOT NULL,
    recommended_mode TEXT NOT NULL,
    evaluated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

---

## 33.12. Таблица sessions

```sql id="f05v3q"
CREATE TABLE sessions (
    session_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    user_id UUID NOT NULL REFERENCES users(user_id),
    provider_id UUID NOT NULL REFERENCES providers(provider_id),
    provider_type TEXT NOT NULL,
    connector_id UUID REFERENCES connectors(connector_id),
    host_id UUID REFERENCES hosts(host_id),
    vm_id UUID NOT NULL REFERENCES vms(vm_id),
    requested_mode TEXT NOT NULL,
    selected_mode TEXT NOT NULL,
    state TEXT NOT NULL,
    policy_snapshot JSONB NOT NULL,
    reason TEXT,
    started_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

---

## 33.13. Таблица policies

```sql id="v2l7oc"
CREATE TABLE policies (
    policy_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    name TEXT NOT NULL,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 100,
    enabled BOOLEAN NOT NULL DEFAULT true,
    selector JSONB NOT NULL,
    rules JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Пример `selector`:

```json id="qzdbte"
{
  "provider_type": ["hyperv", "vmware"],
  "vm_tags": ["production"],
  "groups": ["noc-admins"]
}
```

Пример `rules`:

```json id="mxx90n"
{
  "session": {
    "max_duration_seconds": 3600,
    "allowed_modes": ["RdpRelay", "BasicRescue"],
    "require_mfa": true,
    "require_approval": true
  },
  "clipboard": {
    "mode": "TextWithConfirmation",
    "max_size_bytes": 65536
  },
  "recording": {
    "mode": "EventTimeline"
  },
  "power": {
    "reset_requires_approval": true,
    "poweroff_requires_approval": true
  }
}
```

---

## 33.14. Таблица audit_events

```sql id="g1hfz4"
CREATE TABLE audit_events (
    event_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    user_id UUID REFERENCES users(user_id),
    provider_id UUID REFERENCES providers(provider_id),
    provider_type TEXT,
    connector_id UUID REFERENCES connectors(connector_id),
    host_id UUID REFERENCES hosts(host_id),
    vm_id UUID REFERENCES vms(vm_id),
    session_id UUID REFERENCES sessions(session_id),
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Индексы:

```sql id="l4j9ok"
CREATE INDEX idx_audit_tenant_time ON audit_events (tenant_id, created_at DESC);
CREATE INDEX idx_audit_vm_time ON audit_events (vm_id, created_at DESC);
CREATE INDEX idx_audit_session ON audit_events (session_id);
CREATE INDEX idx_audit_user_time ON audit_events (user_id, created_at DESC);
```

---

## 33.15. Таблица approvals

```sql id="9fz3oy"
CREATE TABLE approvals (
    approval_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    requester_user_id UUID NOT NULL REFERENCES users(user_id),
    approver_user_id UUID REFERENCES users(user_id),
    vm_id UUID REFERENCES vms(vm_id),
    requested_action TEXT NOT NULL,
    requested_mode TEXT,
    reason TEXT,
    status TEXT NOT NULL,
    expires_at TIMESTAMPTZ,
    decided_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

---

## 33.16. Таблица recordings

```sql id="mb4xj2"
CREATE TABLE recordings (
    recording_id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(tenant_id),
    session_id UUID NOT NULL REFERENCES sessions(session_id),
    vm_id UUID NOT NULL REFERENCES vms(vm_id),
    mode TEXT NOT NULL,
    storage_ref TEXT,
    encryption_key_ref TEXT,
    duration_seconds INTEGER,
    size_bytes BIGINT,
    status TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

---

# 34. Provider SDK

## 34.1. Назначение

Provider SDK нужен, чтобы новые гипервизоры подключались как модули без переписывания core.

SDK должен включать:

```text id="qdsgao"
- provider-api crate;
- core-types crate;
- тестовый harness;
- fake broker;
- fake connector runtime;
- examples;
- compatibility tests;
- capability validation tools.
```

---

## 34.2. Минимальный provider

Минимальный provider должен реализовать:

```rust id="8vw0uq"
#[async_trait::async_trait]
pub trait MinimalProvider: Send + Sync {
    fn provider_type(&self) -> ProviderType;

    async fn list_vms(&self, host_id: &str) -> Result<Vec<VmInfo>>;
    async fn get_vm(&self, vm_id: &str) -> Result<VmInfo>;
    async fn get_capabilities(&self, vm_id: &str) -> Result<VmCapabilityGraph>;
}
```

Provider с таким минимумом может отображать inventory, но не обязан поддерживать console.

---

## 34.3. Management provider

Management provider должен дополнительно реализовать:

```rust id="w5x5c1"
#[async_trait::async_trait]
pub trait ManagementProvider: MinimalProvider {
    async fn start_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn shutdown_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn poweroff_vm(&self, vm_id: &str) -> Result<TaskId>;
    async fn reset_vm(&self, vm_id: &str) -> Result<TaskId>;
}
```

---

## 34.4. Snapshot provider

```rust id="pxg4zz"
#[async_trait::async_trait]
pub trait SnapshotProvider: MinimalProvider {
    async fn list_snapshots(&self, vm_id: &str) -> Result<Vec<SnapshotInfo>>;
    async fn create_snapshot(&self, vm_id: &str, req: SnapshotRequest) -> Result<TaskId>;
    async fn revert_snapshot(&self, vm_id: &str, snapshot_id: &str) -> Result<TaskId>;
    async fn delete_snapshot(&self, vm_id: &str, snapshot_id: &str) -> Result<TaskId>;
}
```

---

## 34.5. Console provider

```rust id="d1k8it"
#[async_trait::async_trait]
pub trait ConsoleProvider: MinimalProvider {
    async fn open_preview_stream(&self, req: PreviewRequest) -> Result<PreviewStream>;
    async fn open_console_session(&self, req: ConsoleRequest) -> Result<ConsoleSession>;
}
```

---

## 34.6. Full provider

```rust id="jnfvx6"
pub trait FullProvider:
    MinimalProvider
    + ManagementProvider
    + SnapshotProvider
    + ConsoleProvider
{}
```

---

## 34.7. Provider manifest

Каждый provider должен иметь manifest:

```json id="dywe7m"
{
  "provider_type": "hyperv",
  "display_name": "Hyper-V",
  "version": "1.0.0",
  "supported_platforms": ["windows"],
  "capabilities": [
    "inventory",
    "preview",
    "rescue_input",
    "rdp_relay",
    "snapshots",
    "power_control"
  ],
  "experimental": [
    "af_hyperv"
  ]
}
```

---

## 34.8. Provider compatibility tests

Каждый provider должен проходить тесты:

```text id="cqa5zh"
- list_vms_returns_stable_ids;
- vm_info_contains_provider_type;
- capabilities_have_reason_codes;
- unsupported_features_report_unsupported;
- provider_metadata_is_valid_json;
- errors_are_typed;
- no_panic_on_provider_api_failure;
```

---

# 35. Provider Capability Normalization

## 35.1. Задача

Разные гипервизоры называют одинаковые сущности по-разному:

```text id="rvhpyf"
Hyper-V:
  checkpoint

VMware:
  snapshot

Proxmox:
  snapshot

libvirt:
  snapshot

Все это должно быть SnapshotInfo в core.
```

---

## 35.2. Mapping table

```text id="4dd1wa"
Core concept       Hyper-V          VMware          Proxmox       libvirt
---------------------------------------------------------------------------
VM                 VM               VirtualMachine  qemu/lxc      domain
Host               Hyper-V host     ESXi host       node          host
Cluster            FailoverCluster  vSphereCluster  PVE cluster   custom
Snapshot           Checkpoint       Snapshot        Snapshot      Snapshot
Console            Enhanced/RDP     WebMKS/MKS      noVNC/SPICE   VNC/SPICE
Preview            WMI thumbnail    Screenshot      VNC frame     VNC frame
Tools              IntegrationSvc   VMware Tools    QEMU Agent    QEMU Agent
Native ID          VM GUID          MOID            VMID          Domain UUID
```

---

## 35.3. Ошибки provider’ов

Provider-specific ошибки должны нормализоваться в `ProviderError`.

```rust id="zv5pf6"
pub enum ProviderError {
    AuthFailed,
    AccessDenied,
    NotFound,
    NotSupported,
    BlockedByPolicy,
    Timeout,
    ApiUnavailable,
    InvalidState,
    RateLimited,
    ProviderBug,
    ExperimentalDisabled,
    Unknown,
}
```

Provider-specific details хранить отдельно:

```rust id="6pje5g"
pub struct ErrorDetails {
    pub provider_type: ProviderType,
    pub provider_code: Option<String>,
    pub provider_message: Option<String>,
    pub correlation_id: Option<String>,
}
```

---

# 36. Universal Session Backend Layer

## 36.1. Назначение

Console protocol не должен быть размазан по UI, Broker и provider’ам. Нужен слой session backend adapter.

```text id="qzsa3d"
SessionBackend
  ├── RdpBackend
  ├── VncBackend
  ├── SpiceBackend
  ├── WebMksBackend
  ├── SerialBackend
  ├── BasicRescueBackend
  └── ExperimentalBackend
```

---

## 36.2. SessionBackend trait

```rust id="liqukg"
#[async_trait::async_trait]
pub trait SessionBackend: Send + Sync {
    fn mode(&self) -> SessionMode;

    async fn probe(&self, vm: &VmInfo) -> Result<Capability>;

    async fn open(
        &self,
        vm: &VmInfo,
        req: ConsoleRequest,
        policy: SessionPolicy,
    ) -> Result<ConsoleSession>;

    async fn close(&self, session_id: &str) -> Result<()>;
}
```

---

## 36.3. ConsoleSession

```rust id="192ksc"
pub struct ConsoleSession {
    pub session_id: String,
    pub vm_id: String,
    pub provider_type: ProviderType,
    pub mode: SessionMode,
    pub streams: Vec<StreamDescriptor>,
    pub capabilities: SessionCapabilities,
    pub expires_at: DateTimeUtc,
    pub metadata: serde_json::Value,
}
```

---

## 36.4. StreamDescriptor

```rust id="16p74i"
pub struct StreamDescriptor {
    pub stream_id: String,
    pub channel: StreamChannel,
    pub codec: Option<String>,
    pub direction: StreamDirection,
    pub transport: TransportDescriptor,
}
```

---

## 36.5. StreamChannel

```rust id="mk6tfs"
pub enum StreamChannel {
    Control,
    Video,
    Audio,
    Input,
    Clipboard,
    Serial,
    Preview,
    Metrics,
    Audit,
}
```

---

# 37. Client Decoder Architecture

## 37.1. Задача

Admin GUI Client должен уметь открывать разные типы console stream без переписывания UI.

```text id="p0apg5"
RDP -> RDP decoder
VNC -> VNC decoder
SPICE -> SPICE adapter
WebMKS -> WebMKS adapter
Serial -> terminal renderer
PreviewOnly -> image renderer
BasicRescue -> image renderer + virtual input panel
```

---

## 37.2. Decoder trait

```rust id="7njg3d"
pub trait ConsoleDecoder {
    fn mode(&self) -> SessionMode;

    fn attach_stream(&mut self, stream: StreamHandle) -> Result<()>;
    fn handle_input(&mut self, event: InputEvent) -> Result<()>;
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;
    fn clipboard_event(&mut self, event: ClipboardEvent) -> Result<()>;
    fn stats(&self) -> DecoderStats;
}
```

---

## 37.3. DecoderStats

```rust id="lv8xll"
pub struct DecoderStats {
    pub fps: f32,
    pub render_latency_ms: u32,
    pub input_latency_ms: Option<u32>,
    pub bytes_in_per_sec: u64,
    pub dropped_frames: u64,
    pub reconnects: u32,
}
```

---

## 37.4. Client UI states

```rust id="xufz74"
pub enum VmViewState {
    Loading,
    Preview,
    Connecting,
    Interactive,
    Rescue,
    Degraded,
    Reconnecting,
    Failed,
    Closed,
}
```

---

# 38. Smart Connect Algorithm

## 38.1. Назначение

Smart Connect выбирает лучший backend автоматически.

---

## 38.2. Inputs

```text id="3e0ibu"
- vm_id;
- user_id;
- client capabilities;
- provider capabilities;
- policy;
- current health;
- requested mode;
- network conditions;
- feature flags;
```

---

## 38.3. Scoring

Каждый backend получает score.

```rust id="7omsqx"
pub struct BackendScore {
    pub mode: SessionMode,
    pub available: bool,
    pub score: i32,
    pub reason: String,
}
```

Пример базовых весов:

```text id="bgmdlj"
ProviderNativeConsole: 100
EnhancedSession:       95
WebMksConsole:         95
SpiceConsole:          90
VncConsole:            85
RdpRelay:              80
BasicRescue:           50
PreviewOnly:           20
Offline:                0
Experimental:          disabled unless explicitly allowed
```

---

## 38.4. Modifiers

```text id="x8vqtx"
+20 если backend не требует guest network
+10 если backend поддерживает clipboard
+10 если backend поддерживает resize
+10 если backend поддерживает recording
-20 если backend degraded
-50 если backend experimental
-1000 если blocked by policy
-1000 если client не поддерживает decoder
```

---

## 38.5. Pseudocode

```rust id="qn3y8u"
fn choose_backend(
    graph: VmCapabilityGraph,
    policy: SessionPolicy,
    client: ClientCapabilities,
    requested: RequestedMode,
) -> BackendDecision {
    let mut candidates = build_candidates(graph);

    candidates.retain(|c| policy.allows(&c.mode));
    candidates.retain(|c| client.supports(&c.mode));
    candidates.retain(|c| c.capability.state != CapabilityState::Unsupported);
    candidates.retain(|c| c.capability.state != CapabilityState::BlockedByPolicy);

    if requested.is_explicit() {
        return choose_explicit_or_error(candidates, requested);
    }

    score_candidates(&mut candidates);
    candidates.sort_by(|a, b| b.score.cmp(&a.score));

    candidates.first()
        .cloned()
        .unwrap_or_else(|| BackendDecision::preview_or_error())
}
```

---

## 38.6. Decision output

```rust id="r0hh24"
pub struct BackendDecision {
    pub selected_mode: SessionMode,
    pub fallback_modes: Vec<SessionMode>,
    pub reason: String,
    pub warnings: Vec<String>,
    pub policy_snapshot: SessionPolicy,
}
```

---

# 39. Policy Engine Details

## 39.1. Policy evaluation stages

```text id="cooxff"
1. Tenant defaults
2. Provider policy
3. Host/cluster policy
4. VM tag policy
5. User/group policy
6. JIT grant policy
7. Emergency/break-glass policy
8. Deny overrides
```

---

## 39.2. Deny wins

Если любая политика явно запрещает действие, оно запрещено.

```text id="9kf2pg"
Explicit deny > allow
```

---

## 39.3. PolicyResult

```rust id="adkbbr"
pub struct PolicyResult {
    pub allowed: bool,
    pub require_mfa: bool,
    pub require_approval: bool,
    pub allowed_modes: Vec<SessionMode>,
    pub clipboard_mode: ClipboardMode,
    pub recording_mode: RecordingMode,
    pub max_duration_seconds: u32,
    pub restrictions: Vec<String>,
    pub reason: Option<String>,
}
```

---

## 39.4. Example policies

### Production VM policy

```json id="tiu0fu"
{
  "selector": {
    "vm_tags": ["production"]
  },
  "rules": {
    "session": {
      "require_mfa": true,
      "require_approval": true,
      "max_duration_seconds": 1800
    },
    "clipboard": {
      "mode": "TextWithConfirmation"
    },
    "recording": {
      "mode": "EventTimeline"
    },
    "power": {
      "reset_requires_approval": true,
      "poweroff_requires_approval": true
    }
  }
}
```

### Sensitive VM policy

```json id="euefuf"
{
  "selector": {
    "vm_tags": ["sensitive"]
  },
  "rules": {
    "session": {
      "require_mfa": true,
      "require_approval": true,
      "allowed_modes": ["PreviewOnly", "BasicRescue", "RdpRelay"]
    },
    "clipboard": {
      "mode": "Disabled"
    },
    "recording": {
      "mode": "VideoRequired"
    }
  }
}
```

---

# 40. Security Hardening Checklist

## 40.1. Broker

```text id="5n4h2q"
- TLS 1.3 only
- secure cookies / tokens
- short-lived access tokens
- refresh token rotation
- MFA support
- SSO support
- RBAC enforced server-side
- audit for all privileged actions
- rate limiting
- tenant isolation tests
- secret encryption at rest
- DB migrations reviewed
```

---

## 40.2. Connector

```text id="0a9c6s"
- signed binary
- signed updates
- mTLS with Broker
- no arbitrary shell command API
- allowlisted provider actions only
- local config protected
- local keys protected by OS key store
- outbound connection preferred
- no unauthenticated local API
- worker isolation
- crash restart
- audit buffer
```

---

## 40.3. Client

```text id="3ib2un"
- token storage protected
- clipboard controlled by policy
- no secrets in logs
- session warning banners
- certificate pinning optional
- crash dumps scrubbed
- recording indicator visible
```

---

## 40.4. Relay

```text id="ay499r"
- validates session tokens
- tenant isolation
- no session initiation by relay
- no cross-tenant routing
- idle timeout
- rate limits
- optional opaque E2E data plane
```

---

# 41. Packaging

## 41.1. Admin GUI Client

Platforms:

```text id="stt859"
Phase 1:
  Windows

Phase 2:
  macOS
  Linux

Phase 3:
  Web client for selected backends
```

Installer:

```text id="3evjwi"
- signed installer
- auto-update
- crash reporting opt-in
- local logs
- diagnostics export
```

---

## 41.2. Hyper-V Connector

Packaging:

```text id="to67ek"
- MSI
- Windows Service
- signed binary
- silent install
- enrollment token
- proxy config
- repair/uninstall
- rollback
```

---

## 41.3. Linux Connector

For Proxmox/libvirt:

```text id="vrnrtu"
- systemd service
- .deb package
- .rpm package
- container mode optional
- signed packages
- config file in /etc
- logs via journald
```

---

## 41.4. VMware Connector

Deployment options:

```text id="j9p4sp"
- Docker container
- Linux service
- Windows service
- Kubernetes deployment
- appliance image in future
```

---

# 42. Configuration

## 42.1. Connector config

```toml id="s0xgyd"
[connector]
id = "connector-001"
name = "hv-01-connector"
tenant_id = "tenant-001"

[broker]
url = "https://broker.example.com"
enrollment_token = "one-time-token"

[relay]
mode = "auto"

[provider]
type = "hyperv"

[logging]
level = "info"

[features]
experimental_hvsocket = false
```

---

## 42.2. Provider config examples

### Hyper-V

```toml id="t8xkbd"
[provider.hyperv]
wmi_namespace = "root\\virtualization\\v2"
thumbnail_enabled = true
rescue_enabled = true
rdp_relay_enabled = true
enhanced_session_enabled = true
hvsocket_experimental_enabled = false
```

### VMware

```toml id="rtz7yr"
[provider.vmware]
vcenter_url = "https://vcenter.example.local"
auth_mode = "service_account"
webmks_enabled = true
rdp_relay_enabled = true
```

### Proxmox

```toml id="zaw9bz"
[provider.proxmox]
api_url = "https://pve.example.local:8006"
auth_mode = "api_token"
vnc_enabled = true
spice_enabled = true
rdp_relay_enabled = true
```

### libvirt

```toml id="3wdw9h"
[provider.libvirt]
uri = "qemu:///system"
vnc_enabled = true
spice_enabled = true
serial_enabled = true
```

---

# 43. Secrets Management

## 43.1. Broker secrets

Secrets должны храниться в:

```text id="baol2z"
- cloud secret manager
или
- HashiCorp Vault
или
- encrypted database fields
или
- OS key store for self-hosted small deployments
```

---

## 43.2. Connector secrets

Connector должен хранить:

```text id="535j84"
- connector private key
- provider credentials
- local policy cache signature keys
```

Windows:

```text id="b4m050"
DPAPI / CNG / Windows Credential Manager
```

Linux:

```text id="wsvuiq"
root-owned encrypted file
systemd credentials
kernel keyring
Vault integration
```

---

## 43.3. Secret rotation

Требования:

```text id="vjme08"
- connector certificate rotation
- provider credential rotation
- relay token rotation
- session token expiration
- emergency revoke
```

---

# 44. Update System

## 44.1. Требования

Update system должен поддерживать:

```text id="gb2bix"
- signed updates
- staged rollout
- canary
- rollback
- version pinning
- forced security update
- compatibility check
```

---

## 44.2. Agent update flow

```text id="3by187"
1. Broker publishes update metadata.
2. Connector checks compatibility.
3. Connector downloads signed package.
4. Connector verifies signature.
5. Connector installs update.
6. Connector restarts workers.
7. Connector reports new version.
8. Broker records update event.
```

---

## 44.3. Rollback

Если update failed:

```text id="xlvmu5"
- return to previous version;
- report failure;
- keep audit/logs;
- do not lose enrollment identity.
```

---

# 45. Testing Strategy

## 45.1. Test pyramid

```text id="rya4v7"
Unit tests
  core logic, policy, capability, protocol parser

Integration tests
  provider APIs, broker flows, relay flows

E2E tests
  full client -> broker -> connector -> VM

Chaos tests
  failure scenarios

Security tests
  auth, RBAC, tenant isolation, token handling

Performance tests
  scaling, preview, console latency
```

---

## 45.2. Fake Provider

Для тестирования core нужен fake provider.

```rust id="xb6mnv"
pub struct FakeProvider {
    pub hosts: Vec<HostInfo>,
    pub vms: Vec<VmInfo>,
    pub capabilities: HashMap<String, VmCapabilityGraph>,
}
```

Fake provider должен уметь симулировать:

```text id="tdikhb"
- running VM;
- stopped VM;
- missing console;
- preview available;
- RDP unavailable;
- policy blocked;
- provider API timeout;
- VM migration;
- worker crash.
```

---

## 45.3. Provider contract tests

Каждый provider должен проходить единый набор тестов:

```text id="6nngjf"
- returns stable provider_type
- returns stable provider_native_id
- normalizes power_state
- fills provider_metadata
- does not panic on API failure
- unsupported operations return NotSupported
- capability graph has reason codes
- session open respects policy
```

---

## 45.4. Hyper-V test lab

Минимальный стенд:

```text id="q45g6o"
1 Hyper-V host
3 Windows VMs
2 Linux VMs
1 VM with disconnected vNIC
1 VM with RDP enabled
1 VM with RDP disabled
1 VM booting/paused/saved test case
```

Проверки:

```text id="p9mcgn"
- WMI inventory
- thumbnail retrieval
- BasicRescue
- RDP Relay
- checkpoint operations
- power operations
- Enhanced detection
- AF_HYPERV disabled by default
```

---

## 45.5. VMware test lab

Минимальный стенд:

```text id="os61sn"
1 vCenter or ESXi
3 VMs
1 VM with VMware Tools
1 VM without VMware Tools
1 VM with RDP enabled
1 snapshot tree
```

Проверки:

```text id="l25kv4"
- inventory
- power operations
- snapshots
- WebMKS/MKS ticket flow
- permission denied handling
- RDP Relay
```

---

## 45.6. Proxmox test lab

Минимальный стенд:

```text id="2ja5rx"
1 Proxmox node
3 QEMU VMs
1 Linux VM with serial console
1 VM with noVNC
1 VM with SPICE if available
```

Проверки:

```text id="ffyd4m"
- inventory
- power operations
- snapshots
- VNC/noVNC console
- serial console
- RDP Relay where applicable
```

---

## 45.7. libvirt test lab

Минимальный стенд:

```text id="2u1u6s"
1 Linux KVM host
3 domains
1 VNC domain
1 SPICE domain
1 serial console domain
```

Проверки:

```text id="fmelkc"
- domain inventory
- power operations
- snapshots where supported
- VNC console
- SPICE console
- serial console
```

---

# 46. Performance Test Scenarios

## 46.1. Dashboard scale

```text id="u75z3w"
Scenario A:
  15 VMs, live preview active

Scenario B:
  50 VMs, only visible cards update

Scenario C:
  100 VMs, metadata only + selected preview
```

Targets:

```text id="28o1c9"
- no unbounded queue growth
- no stale frames accumulation
- UI remains responsive
- connector CPU budget respected
```

---

## 46.2. Session latency

Measure:

```text id="46mh92"
- input latency
- frame latency
- network RTT
- decode FPS
- dropped frames
- reconnect time
```

Targets:

```text id="tijs8b"
Interactive backends:
  target input lag < 50 ms in good network conditions
  target >= 30 FPS where backend supports it

Preview/rescue:
  1–2 FPS
```

---

## 46.3. Relay performance

Scenarios:

```text id="64hf88"
- direct connection
- relay connection
- high latency
- packet loss
- low bandwidth
- reconnect
```

---

# 47. Migration Plan From Hyper-V-first Codebase

## 47.1. Problem

Если уже начата реализация Hyper-V-first, нужно избежать будущей переделки.

---

## 47.2. Step 1: Rename core concepts

Переименовать:

```text id="7cl3hq"
HyperVVm       -> VmInfo
HyperVHost     -> HostInfo
HyperVSession  -> Session
HyperVMode     -> SessionMode
HyperVStatus   -> Capability
HyperVAgent    -> NodeAgent / Connector
```

---

## 47.3. Step 2: Move Hyper-V specifics

Перенести:

```text id="hwvtw3"
hyperv.rs
wmi.rs
thumbnail.rs
enhanced.rs
hvsocket.rs
```

в:

```text id="5aq0dy"
providers/hyperv/
```

---

## 47.4. Step 3: Introduce provider_api

Создать:

```text id="x4335r"
crates/provider-api
```

Сначала trait может быть минимальным:

```rust id="r58cib"
#[async_trait::async_trait]
pub trait VirtualizationProvider {
    fn provider_type(&self) -> ProviderType;
    async fn list_vms(&self, host_id: &str) -> Result<Vec<VmInfo>>;
    async fn get_capabilities(&self, vm_id: &str) -> Result<VmCapabilityGraph>;
}
```

---

## 47.5. Step 4: Wrap Hyper-V implementation

Существующий Hyper-V код не переписывать полностью, а обернуть:

```rust id="kxj1u7"
pub struct HyperVProvider {
    inner: ExistingHyperVService,
}
```

---

## 47.6. Step 5: Update protocol

Все команды должны включать:

```json id="ae1vn7"
{
  "provider_type": "hyperv",
  "provider_id": "provider-001",
  "vm_id": "vm-001"
}
```

---

## 47.7. Step 6: Update UI

UI должен читать:

```text id="dwdqpa"
provider_type
recommended_mode
capability_graph
provider_metadata
```

а не Hyper-V-specific fields напрямую.

---

## 47.8. Step 7: Add FakeProvider

Добавить fake provider, чтобы проверить, что UI/Broker не завязаны на Hyper-V.

---

## 47.9. Step 8: Add second provider skeleton

До полной реализации VMware/Proxmox создать skeleton provider:

```text id="0lk5xs"
providers/mock_vmware
или
providers/mock_proxmox
```

Если skeleton отображается в UI без изменений core — архитектура правильная.

---

# 48. Definition of Done

## 48.1. Core Platform DoD

Core считается готовым, если:

```text id="16heeb"
- есть provider_api;
- есть provider_registry;
- Hyper-V подключен как provider;
- FakeProvider работает;
- Broker не импортирует Hyper-V modules;
- Client UI не импортирует Hyper-V modules;
- CapabilityGraph универсальный;
- SessionMode универсальный;
- protocol содержит provider_type;
- policy engine работает с provider-agnostic моделями.
```

---

## 48.2. Hyper-V Provider DoD

Hyper-V provider готов, если:

```text id="tjkwcy"
- inventory работает через WMI/CIM;
- thumbnails работают;
- power operations работают;
- checkpoints работают;
- BasicRescue работает where supported;
- RDP Relay работает;
- Enhanced detection не основан только на heartbeat;
- AF_HYPERV disabled by default;
- все Hyper-V поля лежат в provider_metadata;
- provider проходит contract tests.
```

---

## 48.3. Broker DoD

Broker готов, если:

```text id="vz40f9"
- user auth работает;
- RBAC работает;
- policy evaluation работает;
- provider registry работает;
- OpenSession flow работает;
- audit пишется;
- approval workflow basic работает;
- session tokens short-lived;
- relay coordination работает.
```

---

## 48.4. Client DoD

Client готов, если:

```text id="mupbz4"
- universal dashboard работает;
- VM cards provider-agnostic;
- Smart Connect работает;
- PreviewOnly работает;
- BasicRescue работает;
- RDP decoder работает;
- VNC decoder или adapter готов к подключению;
- clipboard через arboard только в client;
- provider details tab из provider_metadata.
```

---

## 48.5. Security DoD

Security готов, если:

```text id="mxuptk"
- mTLS connector-broker;
- session tokens expire;
- RBAC enforced server-side;
- tenant isolation covered by tests;
- clipboard policy enforced;
- destructive actions require confirmation;
- secrets not logged;
- signed update design approved;
- audit cannot be bypassed through public API.
```

---

# 49. MVP Scope

## 49.1. MVP должен включать

```text id="xw0gux"
Universal core:
  - core-types
  - provider-api
  - provider-registry
  - universal CapabilityGraph
  - universal SessionMode

Hyper-V provider:
  - inventory
  - thumbnails
  - power operations
  - checkpoints
  - BasicRescue
  - RDP Relay
  - Enhanced detection
  - AF_HYPERV disabled experimental

Broker:
  - auth
  - provider registry
  - session broker
  - basic RBAC
  - basic policy
  - audit

Client:
  - dashboard
  - VM cards
  - preview
  - smart connect
  - RDP session
  - BasicRescue UI
  - clipboard text policy

Relay:
  - reverse connector channel
  - basic relay
```

---

## 49.2. MVP не должен включать

```text id="fye7ta"
- VMware full provider
- Proxmox full provider
- libvirt full provider
- production AF_HYPERV
- video recording
- advanced approval workflow
- multi-region relay
- full HA broker
- full web client
```

Но архитектура должна быть готова к их добавлению.

---

# 50. Post-MVP Roadmap

## 50.1. Release 1.1

```text id="z2wla7"
- better policy engine
- approval workflow
- improved audit UI
- connector auto-update
- more Hyper-V diagnostics
- Hyper-V cluster awareness
```

---

## 50.2. Release 1.2

```text id="vu71h8"
- VMware provider
- WebMKS / MKS support
- VMware snapshots
- VMware Tools status
- vCenter inventory
```

---

## 50.3. Release 1.3

```text id="swxckv"
- Proxmox provider
- VNC/noVNC console
- Proxmox snapshots
- Proxmox HA metadata
- SPICE optional
```

---

## 50.4. Release 1.4

```text id="v2v0oz"
- libvirt/KVM provider
- VNC console
- SPICE console
- serial console
- domain snapshots where supported
```

---

## 50.5. Release 2.0

```text id="qaex8x"
- enterprise policy
- session recording
- forensic timeline
- multi-region relay
- self-hosted HA
- advanced SSO/MFA
- break-glass access
- compliance exports
```

---

## 50.6. Experimental Track

```text id="0hdgka"
- Hyper-V AF_HYPERV backend
- no-vNIC interactive experiments
- provider-native high-performance transports
- custom virtual channels
```

---

# 51. UI Requirements: Multi-provider Dashboard

## 51.1. Global dashboard

Должен показывать:

```text id="dwx1po"
- total providers;
- total hosts;
- total VMs;
- running/stopped/degraded VMs;
- active sessions;
- alerts;
- pending approvals;
- connector health;
- relay health.
```

---

## 51.2. Provider view

Для каждого provider:

```text id="5kx9db"
Provider: Hyper-V
Status: Healthy
Connectors: 3
Hosts: 8
VMs: 240
Active sessions: 12
Alerts: 2
```

---

## 51.3. VM table columns

```text id="4oyrta"
Name
Provider
Host
Cluster
Power
Preview
Recommended Mode
Guest IP
Tools
Tags
Last Session
Actions
```

---

## 51.4. VM filters

```text id="v8987d"
- provider type
- host
- cluster
- power state
- recommended mode
- tag
- no network
- rescue available
- RDP available
- policy blocked
- active session
```

---

## 51.5. Action buttons

```text id="3n6sb5"
Connect
Rescue
Preview
Power
Snapshots
Audit
Explain
```

---

# 52. Explainability Requirements

## 52.1. Explain API

```json id="kqfz9c"
{
  "vm_id": "vm-001",
  "question": "why_connect_unavailable"
}
```

Response:

```json id="zrb9w1"
{
  "summary": "Interactive console is unavailable.",
  "reasons": [
    {
      "code": "NO_GUEST_IP",
      "message": "RDP Relay is unavailable because no guest IP is detected."
    },
    {
      "code": "ENHANCED_UNAVAILABLE",
      "message": "Enhanced Session is not available for this VM."
    }
  ],
  "recommended_actions": [
    {
      "label": "Open Basic Rescue",
      "mode": "BasicRescue"
    }
  ]
}
```

---

## 52.2. Required reason quality

Каждая причина должна иметь:

```text id="8x0gco"
- machine-readable code;
- human-readable message;
- provider context;
- remediation if known;
- confidence.
```

---

# 53. Compliance и Enterprise Exports

## 53.1. Audit export

Поддержать экспорт:

```text id="bkl239"
- CSV
- JSONL
- SIEM webhook
- syslog
- OpenTelemetry events
```

---

## 53.2. Compliance reports

Отчеты:

```text id="l5fnly"
- sessions by user;
- sessions by VM;
- privileged actions;
- clipboard usage;
- denied access attempts;
- break-glass usage;
- policy changes;
- connector update history.
```

---

## 53.3. Retention

Policy-based retention:

```text id="a2aedm"
Audit metadata:
  default 365 days

Recordings:
  default 30–90 days

Security events:
  configurable
```

---

# 54. Failure Modes и UX

## 54.1. Connector offline

UI:

```text id="v2rqt0"
Connector offline.
Last seen: 12 minutes ago.
Available actions: view last known state, audit, retry.
```

---

## 54.2. Provider API unavailable

UI:

```text id="2vvpxx"
Provider API unavailable.
Inventory may be stale.
Last successful sync: 10:42.
```

---

## 54.3. Console backend failed

UI:

```text id="6wt186"
RDP Relay failed.
Reason: port 3389 unreachable.
Fallback available: BasicRescue.
```

---

## 54.4. Policy blocked

UI:

```text id="9z3x7f"
Access blocked by policy.
Reason: Production VMs require approval.
Action: Request access.
```

---

## 54.5. Session revoked

UI:

```text id="ulv88f"
Session ended.
Reason: Access was revoked by policy/admin.
```

---

# 55. Developer Guidelines

## 55.1. Naming

Universal core:

```text id="y8efo5"
VmInfo
HostInfo
ProviderType
SessionMode
CapabilityGraph
NodeAgent
Connector
```

Provider-specific:

```text id="3wttvr"
HyperVProvider
VMwareProvider
ProxmoxProvider
LibvirtProvider
```

Запрещено в core:

```text id="t1rrcb"
HyperVVmInfo
VMwareVmInfo
ProxmoxVmInfo
```

---

## 55.2. Provider metadata

Все provider-specific поля должны храниться в:

```rust id="fsvqiz"
provider_metadata: serde_json::Value
```

Пример:

```json id="rqd4b0"
{
  "hyperv": {
    "generation": 2,
    "enhanced_session_state": "available"
  }
}
```

---

## 55.3. No provider leakage

Client/Broker не должны иметь logic типа:

```rust id="azaf8h"
if provider_type == HyperV {
    show_enhanced_session_button();
}
```

Правильно:

```rust id="12e5xl"
if capability.enhanced_session.state == Available {
    show_mode_button(SessionMode::EnhancedSession);
}
```

---

## 55.4. Error handling

Запрещено:

```text id="zmtovt"
unwrap()
expect()
panic!()
```

в production path.

Исключения допускаются только в тестах или bootstrap-коде с понятным failure mode.

---

## 55.5. Unsafe Rust

`unsafe` допускается только:

```text id="b1wvgq"
- в provider-specific low-level modules;
- с documented safety comments;
- с wrapper API;
- с тестами.
```

---

# 56. Architectural Decision Records

## 56.1. ADR-001: Provider abstraction

Decision:

```text id="6fiosw"
Core platform works with Provider API, not with hypervisor-specific APIs.
```

Reason:

```text id="xu3c10"
Позволяет добавить VMware/Proxmox/libvirt без переписывания Broker/Client/Core.
```

---

## 56.2. ADR-002: No guest agent

Decision:

```text id="pvceqn"
Product does not require installing its own agent inside guest OS.
```

Reason:

```text id="d6kl39"
Главное конкурентное отличие и снижение operational friction.
```

---

## 56.3. ADR-003: Capability Graph

Decision:

```text id="hpiidm"
Every VM exposes capability graph instead of single console_mode.
```

Reason:

```text id="kkt5xj"
Разные provider’ы имеют разные backend’ы и ограничения.
```

---

## 56.4. ADR-004: Experimental isolation

Decision:

```text id="7372a4"
Risky backends like AF_HYPERV are experimental and disabled by default.
```

Reason:

```text id="exs5z9"
Production product must be useful without unsupported or unstable mechanisms.
```

---

## 56.5. ADR-005: Provider metadata

Decision:

```text id="feg4s8"
Provider-specific fields are stored in provider_metadata.
```

Reason:

```text id="y1erdl"
Core schema remains stable across providers.
```

---

# 57. Final Product Definition

Итоговый продукт:

```text id="9pltpx"
Universal Hypervisor Access Fabric
```

Описание:

```text id="zylqe9"
Универсальная zero-trust платформа доступа, rescue, live-preview и управления виртуальными машинами на разных гипервизорах без установки сторонних агентов внутрь гостевых ОС.
```

Первый supported provider:

```text id="b5v5ur"
Hyper-V
```

Следующие provider’ы:

```text id="6c32e8"
VMware vSphere / ESXi
Proxmox VE
KVM / QEMU / libvirt
```

Ключевые возможности:

```text id="ce7pvu"
- единый dashboard всех ВМ;
- live-preview;
- smart connect;
- rescue mode;
- RDP/VNC/SPICE/WebMKS/serial console;
- power management;
- snapshots/checkpoints;
- RBAC;
- ACL;
- JIT access;
- audit;
- clipboard policy;
- relay/NAT traversal;
- provider plugin model;
- enterprise readiness.
```

Главный технический принцип:

```text id="ir5zeo"
Новые гипервизоры добавляются как Provider Connector.
Core, Broker и Client не переписываются.
```

Главный продуктовый принцип:

```text id="fe8sdn"
Платформа должна быть полезной даже тогда, когда гостевая сеть ВМ недоступна, а установка агента внутрь гостевой ОС невозможна или запрещена.
```
# 58. User Flows

## 58.1. Flow: Первый запуск и регистрация организации

Цель: пользователь создает tenant и получает возможность подключить первый provider.

Шаги:

```text id="iigx2j"
1. Пользователь открывает Admin GUI Client или Web Portal.
2. Создает организацию / tenant.
3. Подтверждает email.
4. Включает MFA.
5. Создает первый workspace.
6. Выбирает provider: Hyper-V / VMware / Proxmox / libvirt.
7. Получает enrollment token для Connector.
8. Переходит к установке Connector.
```

Результат:

```text id="b8p3oc"
Tenant создан.
Первый admin user создан.
Enrollment token сгенерирован.
```

---

## 58.2. Flow: Установка Hyper-V Connector

Цель: подключить Hyper-V host к платформе.

Шаги:

```text id="0qnikw"
1. Админ скачивает signed MSI installer.
2. Запускает установку на Hyper-V host.
3. Вводит enrollment token.
4. Installer создает Windows Service.
5. Connector стартует.
6. Connector устанавливает mTLS-связь с Broker.
7. Broker регистрирует Connector.
8. Connector выполняет initial inventory.
9. В dashboard появляются host и VM.
```

Проверки:

```text id="ph9koj"
- Service установлен.
- Service работает в Session 0.
- Connector виден в Control Plane.
- Hyper-V provider initialized.
- WMI root\virtualization\v2 доступен.
- VM inventory получен.
```

Ошибки:

```text id="46wn2w"
INVALID_ENROLLMENT_TOKEN
BROKER_UNREACHABLE
WMI_ACCESS_DENIED
HYPERV_ROLE_NOT_FOUND
SERVICE_INSTALL_FAILED
```

---

## 58.3. Flow: Установка VMware Connector

Цель: подключить vCenter/ESXi.

Шаги:

```text id="6e0ihs"
1. Админ выбирает provider VMware.
2. Указывает vCenter endpoint.
3. Создает service account в vCenter.
4. Выдает нужные permissions.
5. Устанавливает Connector рядом с vCenter или в management network.
6. Connector проходит enrollment.
7. Connector проверяет vSphere API.
8. Broker получает hosts, clusters, VMs.
9. В dashboard появляется VMware provider.
```

Минимальные permissions:

```text id="hditsn"
- read inventory;
- read VM config;
- console interact;
- power operations where allowed;
- snapshot operations where allowed;
```

Ошибки:

```text id="7n65df"
VCENTER_AUTH_FAILED
VCENTER_UNREACHABLE
INSUFFICIENT_PRIVILEGES
WEBMKS_TICKET_DENIED
```

---

## 58.4. Flow: Установка Proxmox Connector

Цель: подключить Proxmox VE.

Шаги:

```text id="vj6sk1"
1. Админ выбирает provider Proxmox.
2. Указывает Proxmox API endpoint.
3. Создает Proxmox API token.
4. Настраивает permissions.
5. Устанавливает Connector.
6. Connector проверяет API.
7. Получает nodes, VMs, HA state.
8. Проверяет console backend: VNC/noVNC/SPICE.
9. Dashboard отображает Proxmox VM.
```

Ошибки:

```text id="hkk98r"
PROXMOX_AUTH_FAILED
PROXMOX_API_UNREACHABLE
PROXMOX_NODE_UNAVAILABLE
PROXMOX_VNC_TICKET_FAILED
```

---

## 58.5. Flow: Установка libvirt Connector

Цель: подключить KVM/libvirt host.

Шаги:

```text id="0k4lnw"
1. Админ устанавливает Linux Connector на KVM host.
2. Connector получает enrollment token.
3. Проверяет доступ к libvirt URI.
4. Получает domains.
5. Читает domain XML.
6. Определяет graphics devices: VNC/SPICE/serial.
7. Отправляет inventory в Broker.
```

Ошибки:

```text id="vxb3wh"
LIBVIRT_CONNECTION_FAILED
LIBVIRT_PERMISSION_DENIED
DOMAIN_XML_PARSE_FAILED
NO_CONSOLE_DEVICE
```

---

## 58.6. Flow: Smart Connect к VM

Цель: открыть лучший доступный режим.

Шаги:

```text id="3rq4lm"
1. Пользователь нажимает Connect на VM card.
2. Client отправляет OpenSession(requested_mode=Auto).
3. Broker проверяет пользователя.
4. Broker проверяет RBAC/ACL.
5. Broker применяет Policy Engine.
6. Broker запрашивает свежий CapabilityGraph.
7. Smart Connect выбирает backend.
8. Connector открывает provider-specific session.
9. Relay/Data Plane устанавливается.
10. Client выбирает нужный decoder.
11. Сессия становится Active.
12. Audit event записывается.
```

Примеры выбора:

```text id="7p53kz"
Hyper-V VM без IP:
  BasicRescue

VMware VM с WebMKS:
  WebMksConsole

Proxmox VM с noVNC:
  VncConsole

libvirt VM со SPICE:
  SpiceConsole

Любая VM с RDP:
  RdpRelay, если policy и score разрешают
```

---

## 58.7. Flow: Rescue VM with broken network

Цель: восстановить ВМ без сетевого доступа.

Шаги:

```text id="67tmml"
1. Dashboard показывает VM running, но guest IP отсутствует.
2. RDP Relay недоступен.
3. CapabilityGraph показывает BasicRescue = Available.
4. Админ открывает Rescue.
5. Client показывает thumbnail/preview.
6. Админ отправляет Ctrl+Alt+Del.
7. Админ вводит credentials вручную.
8. Админ выполняет команды восстановления сети.
9. Connector продолжает network probe.
10. Когда IP появляется, Broker обновляет CapabilityGraph.
11. UI предлагает Switch to RDP Relay.
12. Админ переключается на полноценную RDP-сессию.
```

Acceptance:

```text id="ifzmby"
- VM можно диагностировать без guest network.
- UI явно показывает, почему RDP был недоступен.
- После восстановления сети доступен переход на RDP Relay.
- Все rescue-действия попали в audit.
```

---

## 58.8. Flow: Доступ к production VM с approval

Цель: безопасный доступ к критичной ВМ.

Шаги:

```text id="ws8x05"
1. Пользователь нажимает Connect.
2. Policy Engine видит tag=production.
3. Требуется MFA и approval.
4. Пользователь проходит MFA.
5. Создается approval request.
6. Approver получает уведомление.
7. Approver одобряет доступ на 30 минут.
8. Broker выпускает short-lived session token.
9. Сессия открывается.
10. Recording/EventTimeline включается по policy.
11. По истечении TTL сессия завершается.
```

Audit:

```text id="1e7r3d"
- access requested;
- MFA passed;
- approval created;
- approval granted;
- session started;
- session ended;
- backend used;
- clipboard events metadata.
```

---

# 59. Sequence Diagrams

## 59.1. Connector Enrollment

```text id="ore23n"
Admin Portal          Broker             Connector
    |                   |                    |
    | create token      |                    |
    |------------------>|                    |
    | token             |                    |
    |<------------------|                    |
    |                   | install + token    |
    |                   |<-------------------|
    |                   | enroll request     |
    |                   |<-------------------|
    |                   | validate token     |
    |                   | create identity    |
    |                   | return cert/config |
    |                   |------------------->|
    |                   | mTLS connect       |
    |                   |<==================>|
    |                   | initial inventory  |
    |                   |<-------------------|
```

---

## 59.2. Inventory Sync

```text id="kk3rwh"
Broker              Connector              Provider API
  |                     |                       |
  | sync request        |                       |
  |-------------------->|                       |
  |                     | list hosts           |
  |                     |---------------------->|
  |                     | hosts                |
  |                     |<----------------------|
  |                     | list VMs             |
  |                     |---------------------->|
  |                     | VMs                  |
  |                     |<----------------------|
  | normalized inventory|                       |
  |<--------------------|                       |
  | update registry     |                       |
```

---

## 59.3. Capability Evaluation

```text id="bnsjhh"
Client              Broker              Connector             Provider
  |                   |                    |                    |
  | GetCapabilities   |                    |                    |
  |------------------>|                    |                    |
  |                   | request probe      |                    |
  |                   |------------------->|                    |
  |                   |                    | inventory probe    |
  |                   |                    |------------------->|
  |                   |                    | console probe      |
  |                   |                    |------------------->|
  |                   |                    | network probe      |
  |                   |                    |---- VM/host ------>|
  |                   | capability graph   |                    |
  |                   |<-------------------|                    |
  | apply policy      |                    |                    |
  | result            |                    |                    |
  |<------------------|                    |                    |
```

---

## 59.4. Open Interactive Session

```text id="2d6sg7"
Client              Broker              Relay              Connector          Provider/VM
  |                   |                  |                    |                   |
  | OpenSession       |                  |                    |                   |
  |------------------>|                  |                    |                   |
  | auth/policy       |                  |                    |                   |
  | choose backend    |                  |                    |                   |
  | create token      |                  |                    |                   |
  | prepare relay     |----------------->|                    |                   |
  | open provider     |--------------------------------------->|                   |
  |                   |                  |                    | open console       |
  |                   |                  |                    |------------------>|
  | session ready     |<---------------------------------------|                   |
  | session info      |<-----------------|                    |                   |
  | connect data plane|=================>|===================>|==================>|
```

---

## 59.5. Session Revoke

```text id="xrt0ax"
Admin/Broker        Broker             Connector             Client
    |                 |                    |                    |
    | revoke session  |                    |                    |
    |---------------->|                    |                    |
    | update state    |                    |                    |
    | revoke command  |------------------->|                    |
    |                 |                    | close backend      |
    |                 |                    |--------------------|
    | notify client   |--------------------------------------->|
    | audit event     |                    |                    |
```

---

# 60. API Endpoints

## 60.1. REST/gRPC design

Можно реализовать как:

```text id="82xhje"
Phase 1:
  REST + WebSocket streams

Phase 2:
  gRPC + bidirectional streams

Phase 3:
  QUIC data plane
```

Core API должен быть транспорт-независимым.

---

## 60.2. Providers API

```text id="b5w7rl"
GET    /api/v1/providers
POST   /api/v1/providers
GET    /api/v1/providers/{provider_id}
PATCH  /api/v1/providers/{provider_id}
DELETE /api/v1/providers/{provider_id}
```

---

## 60.3. Connectors API

```text id="yhclph"
POST   /api/v1/connectors/enroll
GET    /api/v1/connectors
GET    /api/v1/connectors/{connector_id}
POST   /api/v1/connectors/{connector_id}/rotate-token
POST   /api/v1/connectors/{connector_id}/restart
DELETE /api/v1/connectors/{connector_id}
```

---

## 60.4. Hosts API

```text id="02s98j"
GET /api/v1/hosts
GET /api/v1/hosts/{host_id}
GET /api/v1/hosts/{host_id}/health
GET /api/v1/hosts/{host_id}/vms
```

---

## 60.5. VMs API

```text id="zf4eo9"
GET  /api/v1/vms
GET  /api/v1/vms/{vm_id}
GET  /api/v1/vms/{vm_id}/capabilities
GET  /api/v1/vms/{vm_id}/preview
POST /api/v1/vms/{vm_id}/explain
```

---

## 60.6. VM Power API

```text id="f87xp0"
POST /api/v1/vms/{vm_id}/power/start
POST /api/v1/vms/{vm_id}/power/shutdown
POST /api/v1/vms/{vm_id}/power/poweroff
POST /api/v1/vms/{vm_id}/power/reset
POST /api/v1/vms/{vm_id}/power/suspend
POST /api/v1/vms/{vm_id}/power/resume
```

Все destructive operations должны требовать:

```text id="oa65e5"
- reason;
- confirmation token;
- policy check;
- audit.
```

---

## 60.7. Snapshots API

```text id="hfa0e1"
GET    /api/v1/vms/{vm_id}/snapshots
POST   /api/v1/vms/{vm_id}/snapshots
POST   /api/v1/vms/{vm_id}/snapshots/{snapshot_id}/revert
DELETE /api/v1/vms/{vm_id}/snapshots/{snapshot_id}
```

---

## 60.8. Sessions API

```text id="ff6y0z"
POST /api/v1/sessions
GET  /api/v1/sessions
GET  /api/v1/sessions/{session_id}
POST /api/v1/sessions/{session_id}/revoke
POST /api/v1/sessions/{session_id}/extend
GET  /api/v1/sessions/{session_id}/events
```

---

## 60.9. Approvals API

```text id="d8ajgi"
POST /api/v1/approvals
GET  /api/v1/approvals
GET  /api/v1/approvals/{approval_id}
POST /api/v1/approvals/{approval_id}/approve
POST /api/v1/approvals/{approval_id}/deny
```

---

## 60.10. Audit API

```text id="z5cfdu"
GET /api/v1/audit/events
GET /api/v1/audit/export
GET /api/v1/audit/sessions/{session_id}
```

---

## 60.11. Policies API

```text id="77cb9n"
GET    /api/v1/policies
POST   /api/v1/policies
GET    /api/v1/policies/{policy_id}
PATCH  /api/v1/policies/{policy_id}
DELETE /api/v1/policies/{policy_id}
POST   /api/v1/policies/evaluate
```

---

# 61. Message Contracts

## 61.1. ConnectorHello

```json id="cd8d5q"
{
  "type": "ConnectorHello",
  "protocol_version": "1.0",
  "connector_id": "connector-001",
  "provider_type": "hyperv",
  "connector_version": "1.0.0",
  "supported_features": [
    "inventory",
    "preview",
    "rdp_relay",
    "power_control"
  ],
  "host_info": {
    "hostname": "HV-01",
    "os": "Windows Server 2022"
  }
}
```

---

## 61.2. InventoryUpdate

```json id="lncry0"
{
  "type": "InventoryUpdate",
  "connector_id": "connector-001",
  "provider_type": "hyperv",
  "hosts": [],
  "vms": [],
  "timestamp": "2026-06-15T10:00:00Z"
}
```

---

## 61.3. CapabilityUpdate

```json id="emgybd"
{
  "type": "CapabilityUpdate",
  "vm_id": "vm-001",
  "provider_type": "hyperv",
  "capability_graph": {},
  "evaluated_at": "2026-06-15T10:00:00Z"
}
```

---

## 61.4. SessionOpenCommand

```json id="1iupgn"
{
  "type": "SessionOpenCommand",
  "session_id": "session-001",
  "vm_id": "vm-001",
  "provider_type": "hyperv",
  "selected_mode": "BasicRescue",
  "policy": {},
  "transport": {
    "relay_token": "short-lived-token",
    "relay_url": "wss://relay.example.com/session/session-001"
  }
}
```

---

## 61.5. SessionReady

```json id="1e6v0h"
{
  "type": "SessionReady",
  "session_id": "session-001",
  "selected_mode": "BasicRescue",
  "streams": [
    {
      "stream_id": "preview-001",
      "channel": "Preview",
      "codec": "webp",
      "direction": "server_to_client"
    },
    {
      "stream_id": "input-001",
      "channel": "Input",
      "direction": "client_to_server"
    }
  ]
}
```

---

## 61.6. SessionError

```json id="8a52kd"
{
  "type": "SessionError",
  "session_id": "session-001",
  "error": {
    "code": "RDP_PORT_CLOSED",
    "message": "RDP Relay unavailable because guest port 3389 is closed.",
    "provider_type": "hyperv",
    "retryable": false
  },
  "fallback_modes": ["BasicRescue", "PreviewOnly"]
}
```

---

## 61.7. AuditEventMessage

```json id="l6a3zp"
{
  "type": "AuditEvent",
  "event_id": "event-001",
  "tenant_id": "tenant-001",
  "session_id": "session-001",
  "vm_id": "vm-001",
  "event_type": "session.started",
  "severity": "info",
  "metadata": {
    "selected_mode": "BasicRescue"
  },
  "created_at": "2026-06-15T10:00:00Z"
}
```

---

# 62. Error Code Catalog

## 62.1. Auth errors

```text id="dkpvf3"
AUTH_INVALID_TOKEN
AUTH_EXPIRED_TOKEN
AUTH_MFA_REQUIRED
AUTH_MFA_FAILED
AUTH_DEVICE_NOT_TRUSTED
AUTH_SESSION_REVOKED
```

---

## 62.2. Authorization errors

```text id="bmiy3x"
ACCESS_DENIED
RBAC_DENIED
ACL_DENIED
POLICY_DENIED
APPROVAL_REQUIRED
APPROVAL_DENIED
JIT_EXPIRED
```

---

## 62.3. Provider errors

```text id="4sb76c"
PROVIDER_UNAVAILABLE
PROVIDER_AUTH_FAILED
PROVIDER_TIMEOUT
PROVIDER_RATE_LIMITED
PROVIDER_UNSUPPORTED_OPERATION
PROVIDER_VERSION_UNSUPPORTED
PROVIDER_NATIVE_ERROR
```

---

## 62.4. VM errors

```text id="tikssg"
VM_NOT_FOUND
VM_OFFLINE
VM_INVALID_STATE
VM_MIGRATING
VM_LOCKED
VM_SHIELDED_RESTRICTED
VM_TOOLS_NOT_RUNNING
```

---

## 62.5. Console errors

```text id="09cdr9"
CONSOLE_UNAVAILABLE
CONSOLE_BLOCKED_BY_POLICY
CONSOLE_TICKET_FAILED
CONSOLE_PROTOCOL_UNSUPPORTED
CONSOLE_DECODER_UNAVAILABLE
CONSOLE_RECONNECT_FAILED
```

---

## 62.6. RDP errors

```text id="kykzbr"
RDP_NO_GUEST_IP
RDP_PORT_CLOSED
RDP_AUTH_FAILED
RDP_TLS_FAILED
RDP_RELAY_FAILED
RDP_CLIPBOARD_BLOCKED
```

---

## 62.7. Preview errors

```text id="s9a6xv"
PREVIEW_UNAVAILABLE
PREVIEW_PROVIDER_TIMEOUT
PREVIEW_ENCODING_FAILED
PREVIEW_BLOCKED_BY_POLICY
PREVIEW_RATE_LIMITED
```

---

## 62.8. Hyper-V specific errors

```text id="mwbbpy"
HYPERV_WMI_UNAVAILABLE
HYPERV_WMI_ACCESS_DENIED
HYPERV_VMMS_UNAVAILABLE
HYPERV_THUMBNAIL_FAILED
HYPERV_ENHANCED_UNAVAILABLE
HYPERV_HEARTBEAT_MISSING
HYPERV_HVSOCKET_DISABLED
HYPERV_HVSOCKET_POC_NOT_PASSED
```

---

## 62.9. VMware specific errors

```text id="64akv5"
VMWARE_VCENTER_UNREACHABLE
VMWARE_AUTH_FAILED
VMWARE_PERMISSION_DENIED
VMWARE_WEBMKS_TICKET_FAILED
VMWARE_TOOLS_NOT_RUNNING
VMWARE_MOID_NOT_FOUND
```

---

## 62.10. Proxmox specific errors

```text id="03v6cm"
PROXMOX_API_UNREACHABLE
PROXMOX_AUTH_FAILED
PROXMOX_PERMISSION_DENIED
PROXMOX_VNC_TICKET_FAILED
PROXMOX_NODE_OFFLINE
PROXMOX_TASK_FAILED
```

---

## 62.11. libvirt specific errors

```text id="vg40qq"
LIBVIRT_CONNECTION_FAILED
LIBVIRT_PERMISSION_DENIED
LIBVIRT_DOMAIN_NOT_FOUND
LIBVIRT_NO_GRAPHICS_DEVICE
LIBVIRT_SNAPSHOT_UNSUPPORTED
LIBVIRT_OPERATION_FAILED
```

---

# 63. Capability Matrix by Provider

## 63.1. MVP capability matrix

```text id="eyv2bl"
Capability              Hyper-V MVP    VMware R1.2    Proxmox R1.3    libvirt R1.4
--------------------------------------------------------------------------------
Inventory               Yes            Yes            Yes             Yes
Power Control           Yes            Yes            Yes             Yes
Snapshots/Checkpoints   Yes            Yes            Yes             Partial
Live Preview            Yes            Partial        Partial         Partial
Basic Rescue            Yes            No             Partial         Partial
RDP Relay               Yes            Yes            Yes             Yes
VNC Console             No             No             Yes             Yes
SPICE Console           No             No             Optional        Optional
WebMKS Console          No             Yes            No              No
Serial Console          No             No             Optional        Yes
Clipboard               RDP only       WebMKS/RDP     VNC/SPICE/RDP   VNC/SPICE/RDP
Recording               Metadata       Metadata       Metadata        Metadata
Experimental            AF_HYPERV      None           None            None
```

---

## 63.2. Important interpretation

`Yes` означает:

```text id="tm2tt2"
Функция поддерживается provider’ом при корректной конфигурации окружения.
```

`Partial` означает:

```text id="jaqx6w"
Функция зависит от конфигурации VM/provider и должна отображаться через CapabilityGraph.
```

`Optional` означает:

```text id="iq8966"
Функция включается только если backend явно настроен и доступен.
```

---

# 64. Roadmap Backlog

## 64.1. Epic: Universal Core

Tasks:

```text id="iyvyim"
CORE-001 Create core-types crate
CORE-002 Create provider-api crate
CORE-003 Define ProviderType
CORE-004 Define VmInfo
CORE-005 Define HostInfo
CORE-006 Define SessionMode
CORE-007 Define CapabilityGraph
CORE-008 Define ProviderError
CORE-009 Implement ProviderRegistry
CORE-010 Add FakeProvider
CORE-011 Add provider contract tests
```

Acceptance:

```text id="8sl7bx"
FakeProvider can appear in dashboard without provider-specific UI changes.
```

---

## 64.2. Epic: Protocol

Tasks:

```text id="k01b9f"
PROTO-001 Define frame header
PROTO-002 Define control channel messages
PROTO-003 Define preview channel messages
PROTO-004 Define input channel messages
PROTO-005 Define clipboard channel messages
PROTO-006 Define audit channel messages
PROTO-007 Add version negotiation
PROTO-008 Add feature negotiation
PROTO-009 Add reconnect protocol
PROTO-010 Add binary stream multiplexing
```

Acceptance:

```text id="6o7kfq"
Client and Connector can maintain multiple streams over one logical session.
```

---

## 64.3. Epic: Broker MVP

Tasks:

```text id="aeff44"
BROKER-001 Tenant model
BROKER-002 User auth
BROKER-003 Provider registry
BROKER-004 Connector enrollment
BROKER-005 VM registry
BROKER-006 Capability storage
BROKER-007 Session broker
BROKER-008 Basic policy engine
BROKER-009 Audit log
BROKER-010 REST API
BROKER-011 WebSocket/gRPC connector channel
```

Acceptance:

```text id="k7tsjp"
User can enroll Connector, see VM list, open session and view audit event.
```

---

## 64.4. Epic: Relay MVP

Tasks:

```text id="skofmu"
RELAY-001 Session token validation
RELAY-002 Client connection
RELAY-003 Connector connection
RELAY-004 Stream pairing
RELAY-005 Backpressure
RELAY-006 Idle timeout
RELAY-007 Reconnect
RELAY-008 Bandwidth metrics
```

Acceptance:

```text id="m9y540"
Client can connect to Connector through Relay when direct path is unavailable.
```

---

## 64.5. Epic: Hyper-V Provider

Tasks:

```text id="ok2him"
HYPERV-001 Windows Service skeleton
HYPERV-002 Broker enrollment
HYPERV-003 WMI connection
HYPERV-004 List VMs
HYPERV-005 Map Msvm_ComputerSystem to VmInfo
HYPERV-006 Get Msvm_SummaryInformation
HYPERV-007 Build CapabilityGraph
HYPERV-008 Get thumbnails
HYPERV-009 Convert RGB565 thumbnails
HYPERV-010 Thumbnail stream
HYPERV-011 Power operations
HYPERV-012 Checkpoints
HYPERV-013 Keyboard rescue POC
HYPERV-014 RDP relay
HYPERV-015 Enhanced detection
HYPERV-016 AF_HYPERV feature flag
HYPERV-017 Contract tests
```

Acceptance:

```text id="upvz6p"
Hyper-V VM appears in universal dashboard and supports preview, power actions and Smart Connect fallback.
```

---

## 64.6. Epic: Client MVP

Tasks:

```text id="6r5rqv"
CLIENT-001 Login UI
CLIENT-002 Provider list
CLIENT-003 Host list
CLIENT-004 VM dashboard
CLIENT-005 VM card
CLIENT-006 Capability badges
CLIENT-007 Preview renderer
CLIENT-008 Smart Connect UI
CLIENT-009 RDP decoder integration
CLIENT-010 BasicRescue UI
CLIENT-011 Clipboard Manager via arboard
CLIENT-012 Audit/session panel
CLIENT-013 Explain modal
CLIENT-014 Diagnostics export
```

Acceptance:

```text id="q5z1w6"
Admin can see VM list, view thumbnails, click Connect, use selected session mode.
```

---

## 64.7. Epic: Policy and Security

Tasks:

```text id="06zgtb"
SEC-001 RBAC roles
SEC-002 ACL model
SEC-003 Session tokens
SEC-004 Connector mTLS
SEC-005 Clipboard policy
SEC-006 Destructive action confirmation
SEC-007 Audit every privileged action
SEC-008 MFA hook
SEC-009 Approval workflow skeleton
SEC-010 Secret storage abstraction
```

Acceptance:

```text id="qjd1x5"
Policy can block session mode, clipboard and power operations with visible reason.
```

---

# 65. Minimal First Milestone

## 65.1. Goal

Получить работающий вертикальный срез:

```text id="nbasxv"
Hyper-V VM -> Connector -> Broker -> Client Dashboard -> Preview -> Basic Action
```

---

## 65.2. Scope

Входит:

```text id="e051y7"
- core-types
- provider-api
- Hyper-V provider skeleton
- WMI list VMs
- thumbnail one-shot
- Broker minimal
- Client VM list
- Client thumbnail display
```

Не входит:

```text id="xg8jqe"
- full RDP
- relay
- policy engine full
- VMware/Proxmox/libvirt
- AF_HYPERV
```

---

## 65.3. Definition of Success

```text id="m1qrlk"
1. Hyper-V Connector connects to Broker.
2. Broker receives list of VMs.
3. Client displays VM list.
4. Client requests thumbnail.
5. Thumbnail appears in UI.
6. Core types do not contain Hyper-V-specific names.
```

---

# 66. Second Milestone

## 66.1. Goal

Добавить session model и Smart Connect skeleton.

Scope:

```text id="5t0sfm"
- CapabilityGraph for Hyper-V
- recommended_mode
- OpenSession API
- PreviewOnly session
- BasicRescue session skeleton
- Audit events
```

Success:

```text id="m509bb"
User clicks Connect and gets PreviewOnly/BasicRescue session based on capabilities.
```

---

# 67. Third Milestone

## 67.1. Goal

Добавить production-полезность для Hyper-V.

Scope:

```text id="3674cv"
- RDP Relay
- power operations
- checkpoints
- clipboard policy for RDP
- RBAC basic
- Relay basic
```

Success:

```text id="xnqijw"
User can connect to VM via RDP Relay through Host Connector and Broker.
```

---

# 68. Fourth Milestone

## 68.1. Goal

Доказать, что архитектура multi-provider.

Scope:

```text id="iq8z2e"
- Fake VMware provider
- Fake Proxmox provider
- UI displays multiple provider types
- Broker handles provider_type
- Smart Connect works with fake non-Hyper-V capabilities
```

Success:

```text id="tcdohi"
Client shows Hyper-V VM and Fake VMware VM in same dashboard without code changes in Client core.
```

---

# 69. Fifth Milestone

## 69.1. Goal

Начать реальный второй provider.

Рекомендуемый порядок:

```text id="ghjkok"
1. Proxmox Provider
или
2. VMware Provider
```

Выбор зависит от рынка:

```text id="a0ch5s"
VMware:
  enterprise-ready, коммерчески сильнее.

Proxmox:
  проще для dev/test, быстрее получить VNC/noVNC.
```

---

# 70. Architecture Review Checklist

Перед каждым релизом проверять:

```text id="i01db2"
- Нет ли provider-specific imports в core.
- Нет ли HyperV* naming в универсальных моделях.
- Все capabilities имеют reason_code.
- Unsupported features не скрыты молча.
- Policy применяется на Broker, а не только в UI.
- Audit пишется для privileged actions.
- Clipboard не работает без policy.
- Experimental backend отключен по умолчанию.
- Worker crash не роняет Connector.
- Secrets не попадают в logs.
```

---

# 71. Security Review Checklist

## 71.1. Authentication

```text id="ssnrm3"
- MFA tested
- expired tokens rejected
- revoked sessions terminated
- connector cert rotation tested
```

## 71.2. Authorization

```text id="z46wbw"
- user cannot access VM from another tenant
- user cannot bypass policy via API
- viewer cannot start session if not allowed
- destructive operations require permission
```

## 71.3. Transport

```text id="1g4kwk"
- TLS enabled
- mTLS connector-broker
- relay validates tokens
- no unauthenticated streams
```

## 71.4. Data protection

```text id="dfz73q"
- provider credentials encrypted
- clipboard content not logged
- console tickets not logged
- recordings encrypted
```

---

# 72. Open Technical Questions

## 72.1. Client rendering stack

Нужно выбрать GUI stack:

```text id="wwq69v"
Option A:
  Tauri + Rust backend + web UI

Option B:
  egui/eframe

Option C:
  native desktop UI

Option D:
  Electron + Rust core
```

Критерии:

```text id="p8wx6l"
- RDP/VNC/SPICE embedding;
- performance;
- cross-platform;
- update system;
- clipboard integration;
- rendering latency.
```

---

## 72.2. RDP implementation

Варианты:

```text id="uzgdtm"
- FreeRDP FFI/wrapper
- IronRDP
- custom minimal RDP client
```

Рекомендация:

```text id="ue6nk0"
Для MVP использовать mature implementation/wrapper.
Чистый RDP decoder делать только если есть отдельная команда и время.
```

---

## 72.3. VNC implementation

Варианты:

```text id="x2831u"
- Rust VNC client crate
- noVNC webview
- custom VNC decoder
```

Рекомендация:

```text id="kz32mk"
Для Proxmox/libvirt MVP быстрее использовать VNC adapter/noVNC-compatible path.
```

---

## 72.4. SPICE implementation

Варианты:

```text id="ak2kdx"
- native SPICE client integration
- web adapter if available
- defer SPICE to later release
```

Рекомендация:

```text id="do9yqv"
SPICE сделать optional после VNC.
```

---

## 72.5. WebMKS implementation

Варианты:

```text id="djfo48"
- browser/webview based WebMKS adapter
- native MKS protocol integration
- VMRC handoff as temporary fallback
```

Рекомендация:

```text id="x7x9cb"
Для VMware provider начать с WebMKS/webview adapter, потом улучшать.
```

---

## 72.6. Data plane transport

Варианты:

```text id="vw2r1k"
- WebSocket over TLS
- gRPC streaming
- QUIC
- custom TCP
```

Рекомендация:

```text id="ixs30c"
MVP:
  WebSocket/gRPC streaming

Production:
  QUIC or optimized multiplexed transport
```

---

## 72.7. Broker deployment

Варианты:

```text id="dr1ba5"
- SaaS cloud
- self-hosted single node
- self-hosted HA
```

Рекомендация:

```text id="3vfkx5"
Сразу проектировать так, чтобы self-hosted был возможен.
```

---

# 73. Non-Goals

На текущем этапе не делать:

```text id="j953jf"
- свой полноценный RDP stack с нуля;
- свой полноценный SPICE stack с нуля;
- обход protected console у shielded/protected VM;
- guest agent;
- arbitrary command execution на host;
- автоматический ввод паролей;
- хранение credentials пользователей;
- production AF_HYPERV без POC;
- full SIEM/SOC suite;
- собственный hypervisor management вместо существующих provider APIs.
```

---

# 74. Product Differentiators

## 74.1. Чем отличается от обычного RDP

```text id="qur7cd"
Обычный RDP:
  требует сеть внутри VM;
  слабо решает rescue;
  плохо работает как fleet dashboard;
  не дает единый audit для разных hypervisor.

Платформа:
  видит VM через provider layer;
  может дать preview/rescue;
  выбирает backend;
  централизует audit/policy;
  работает через relay/NAT.
```

---

## 74.2. Чем отличается от AnyDesk/RustDesk

```text id="ezs75l"
AnyDesk/RustDesk:
  требует agent/app внутри гостевой ОС.

Платформа:
  не требует guest agent;
  работает с VM как с объектом гипервизора;
  может помочь даже при сломанной сети VM;
  управляет power/snapshots;
  понимает provider capabilities.
```

---

## 74.3. Чем отличается от vCenter/Proxmox UI

```text id="ndd5jb"
vCenter/Proxmox UI:
  provider-specific;
  разные интерфейсы;
  нет единой политики доступа между гипервизорами;
  нет единого relay/data plane;
  нет единого audit UX.

Платформа:
  единый dashboard;
  единый Smart Connect;
  единая policy;
  единый audit;
  multi-provider access.
```

---

# 75. Business Packaging

## 75.1. Editions

```text id="zp2bha"
Community / Lab:
  single provider
  limited VMs
  no advanced audit

Professional:
  multiple providers
  relay
  RBAC
  audit

Enterprise:
  SSO/MFA
  JIT
  approval workflow
  recording
  self-hosted
  HA
  SIEM export

MSP:
  multi-tenant
  customer isolation
  delegated admins
  billing/reporting
```

---

## 75.2. Licensing metrics

Варианты:

```text id="7oc48f"
- per managed VM;
- per concurrent session;
- per provider connector;
- per admin user;
- MSP tenant-based;
- hybrid.
```

Рекомендация:

```text id="4mvnzc"
Для начала:
  per managed VM + included admin seats.

Для MSP:
  per tenant/customer + VM tiers.
```

---

# 76. Final Implementation Rule

Самое важное правило для разработки:

```text id="u0uffp"
Любая новая фича должна сначала быть описана в core capability model.
Только потом она реализуется в provider-specific module.
```

Пример:

Неправильно:

```text id="7amj3h"
Добавить кнопку "Hyper-V Enhanced".
```

Правильно:

```text id="36u0wr"
Добавить capability "EnhancedSession".
Hyper-V provider выставляет ее Available/Unavailable.
UI отображает кнопку, если capability available.
```

---

# 77. Final Architecture Guardrail

Платформа должна выдерживать тест:

```text id="zww74v"
Если завтра удалить providers/hyperv,
Broker, Client, Policy Engine, Audit и Relay должны продолжить компилироваться.
```

Если этот тест не проходит — архитектура снова стала Hyper-V-only и требует рефакторинга.

---

# 78. Final MVP Success Statement

MVP считается успешным, если:

```text id="tzp6qk"
1. Hyper-V подключен как первый provider.
2. ВМ отображаются в универсальном dashboard.
3. Live preview работает.
4. Smart Connect работает.
5. Есть хотя бы PreviewOnly, BasicRescue и RDP Relay.
6. Core не содержит Hyper-V-specific зависимостей.
7. FakeProvider подтверждает multi-provider архитектуру.
8. Audit и basic policy работают.
9. Clipboard находится только в Client.
10. AF_HYPERV отключен и помечен experimental.
```

---

# 79. Final Product Vision

Финальная версия продукта должна выглядеть так:

```text id="9hf9im"
Единая платформа, где админ видит все ВМ на Hyper-V, VMware, Proxmox и KVM,
понимает их состояние, открывает лучший доступный console/backend,
восстанавливает сломанные VM без guest agent,
а компания получает RBAC, JIT, audit, policy и relay.
```

Ключевой слоган:

```text id="d80r9l"
One secure access fabric for every VM, across every hypervisor, without guest agents.
```

Русская версия:

```text id="v174sr"
Единая безопасная платформа доступа ко всем ВМ на разных гипервизорах без установки агентов внутрь гостевых ОС.
```
