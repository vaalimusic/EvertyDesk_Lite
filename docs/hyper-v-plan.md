# ТЕХНИЧЕСКОЕ ЗАДАНИЕ

## Фича: Zero-Trust Hyper-V Access Fabric

### Безагентный доступ, rescue, live-preview и интерактивное управление ВМ Hyper-V через Host Agent

---

## 0. Краткое резюме

Необходимо реализовать платформу удаленного доступа и управления виртуальными машинами Hyper-V без установки сторонних агентов внутрь гостевых ОС.

Единственная обязательная точка установки в инфраструктуре клиента — **Hyper-V Host Agent**, работающий как Windows Service на Hyper-V хосте. Агент взаимодействует с Hyper-V через локальные поддерживаемые интерфейсы Windows: WMI/CIM, Hyper-V WMI Provider, VMMS, RDP relay и, на experimental-этапе, Hyper-V Sockets.

Система должна обеспечивать:

* инвентаризацию ВМ;
* live-dashboard с миниатюрами экранов ВМ;
* power/control операции;
* rescue-доступ при отключенной сети ВМ;
* интерактивный доступ через лучший доступный backend;
* fallback при недоступности интерактивного режима;
* clipboard policy;
* аудит;
* RBAC;
* JIT-доступ;
* поддержку NAT traversal/relay;
* готовность к enterprise-развертыванию.

Ключевой принцип:
**агентов внутри гостевых ОС не устанавливать.**

---

# 1. Термины и определения

## 1.1. Admin GUI Client

Клиентское приложение администратора. Запускается на рабочей станции администратора. Отвечает за:

* dashboard UI;
* отображение live thumbnails;
* декодирование интерактивных потоков;
* обработку keyboard/mouse input;
* локальный clipboard через `arboard`;
* session UX;
* локальное применение части политик.

## 1.2. Hyper-V Host Agent

Служба Windows, устанавливаемая на Hyper-V хост. Отвечает за:

* локальный доступ к WMI/CIM;
* inventory ВМ;
* thumbnail polling;
* VM power operations;
* backend selection;
* RDP relay;
* Enhanced Session backend;
* experimental AF_HYPERV backend;
* локальный audit buffer;
* metrics;
* безопасный канал к Broker/Control Plane.

## 1.3. Control Plane / Broker

Центральный сервис управления доступом. Может быть облачным или self-hosted. Отвечает за:

* регистрацию host agents;
* identity;
* SSO/MFA;
* RBAC;
* ACL;
* policy engine;
* session brokerage;
* audit log;
* fleet health;
* update channel;
* relay coordination.

## 1.4. Relay / Edge Network

Слой транспорта для соединения Client ↔ Host Agent в условиях NAT/firewall.

Поддерживает:

* direct connection where possible;
* relay fallback;
* NAT traversal;
* regional routing;
* bandwidth shaping;
* reconnect;
* session continuity.

## 1.5. Guest Agent

Любое стороннее ПО удаленного доступа внутри гостевой ОС.
В рамках данной фичи установка Guest Agent запрещена.

## 1.6. Supported Backend

Backend, который строится на поддерживаемых и воспроизводимых механизмах Windows/Hyper-V/RDP.

## 1.7. Experimental Backend

Backend, работающий только за feature flag и после отдельного POC. Не является обязательным для MVP и не может быть единственной основой production-функциональности.

---

# 2. Цели проекта

## 2.1. Основная цель

Создать безагентную платформу доступа к ВМ Hyper-V, позволяющую администратору видеть, диагностировать и управлять ВМ даже в ситуациях, когда гостевая ОС недоступна по сети.

## 2.2. Продуктовая цель

Сделать не просто “remote desktop”, а **Hyper-V Rescue & Access Cockpit**:

* видеть живое состояние ВМ;
* понимать, почему интерактивный доступ доступен или недоступен;
* восстанавливать ВМ с поломанной сетью;
* переключаться между backend’ами автоматически;
* иметь аудит и политики доступа.

## 2.3. Техническая цель

Сформировать архитектуру, где:

* dashboard и management работают через supported Hyper-V/WMI APIs;
* интерактивная сессия выбирается через Capability Engine;
* AF_HYPERV является experimental backend, а не единственным production-core;
* система не зависит от `vmconnect.exe`, `PrintWindow`, Win32 window capture и GUI-сессии на сервере.

---

# 3. Область действия

## 3.1. Входит в scope

* Windows Service Agent для Hyper-V host.
* GUI-клиент администратора.
* Собственный защищенный сетевой протокол.
* Broker/Control Plane.
* Relay/NAT traversal.
* WMI/CIM inventory.
* WMI thumbnails.
* WMI keyboard rescue.
* RDP relay backend.
* Enhanced Session backend.
* AF_HYPERV experimental backend.
* Clipboard через GUI-клиент.
* RBAC.
* Audit.
* Policy Engine.
* Metrics/observability.
* Cluster awareness.
* Shielded VM awareness.
* Session capability graph.

## 3.2. Не входит в scope MVP

* Установка ПО внутрь гостевых ОС.
* Обход security-механизмов Shielded VM.
* Гарантированный 60 FPS для всех ВМ.
* Гарантированное интерактивное управление ВМ без vNIC через AF_HYPERV.
* Поддержка произвольного shell execution на host.
* Полноценный file transfer внутрь ВМ без отдельной политики и канала.
* Перехват GUI-окон на host через Win32 API.

## 3.3. Experimental scope

Следующие пункты допускаются только как experimental:

* чтение интерактивного потока через AF_HYPERV;
* no-vNIC interactive session без стандартного RDP/Enhanced path;
* clipboard через AF_HYPERV;
* input injection через AF_HYPERV;
* прямое взаимодействие с непубличными VMConnect-совместимыми endpoint’ами.

---

# 4. Высокоуровневая архитектура

```text
[Admin GUI Client]
  ├── Dashboard UI
  ├── Session UX
  ├── RDP / Enhanced decoder
  ├── Clipboard via arboard
  ├── Input capture
  ├── Local cache
  └── Local policy enforcement

        ⇅ E2E encrypted data plane / relay / NAT traversal

[Relay / Edge Network]
  ├── Direct path negotiation
  ├── Relay fallback
  ├── Regional routing
  ├── Connection failover
  └── Bandwidth shaping

        ⇅ secure session transport

[Control Plane / Broker]
  ├── Identity / SSO / MFA
  ├── Device enrollment
  ├── Host registry
  ├── VM registry
  ├── RBAC / ACL
  ├── Policy engine
  ├── Session broker
  ├── Approval workflow
  ├── Audit log
  ├── Recording metadata
  ├── Agent update service
  └── Fleet health

        ⇅ secure agent control channel

[Hyper-V Host Agent]
  ├── Agent Core
  ├── Capability Engine
  ├── WMI/CIM Provider
  ├── Thumbnail Engine
  ├── Keyboard Rescue Engine
  ├── RDP Relay Backend
  ├── Enhanced Session Backend
  ├── AF_HYPERV Experimental Backend
  ├── Cluster Awareness
  ├── Shielded VM Awareness
  ├── Per-session Worker Sandbox
  ├── Metrics / ETW / Event Log
  ├── Local policy cache
  └── Secure key storage

        ⇅ local host APIs

[Hyper-V / Windows Server]
  ├── VMMS
  ├── WMI root\virtualization\v2
  ├── VM thumbnails
  ├── VM power/checkpoint control
  ├── Guest RDP/xRDP when available
  ├── Failover Cluster integration
  ├── Guarded Fabric / Shielded VM constraints
  └── Hyper-V sockets experimental path
```

---

# 5. Архитектурные принципы

## 5.1. No Guest Agent

Внутри гостевой ОС запрещено устанавливать сторонний агент удаленного доступа.

Допускается использование встроенных компонентов гостевой ОС:

* Remote Desktop Services;
* xRDP;
* Hyper-V Integration Services;
* стандартные драйверы и службы ОС.

## 5.2. Host Agent as Trusted Access Point

Все операции с ВМ выполняются через Host Agent, установленный на Hyper-V хост.

Host Agent должен работать:

* как Windows Service;
* в Session 0;
* без GUI;
* без зависимости от интерактивной сессии пользователя;
* без запуска `vmconnect.exe`;
* без `PrintWindow`;
* без screen scraping.

## 5.3. Smart Backend Selection

Система не должна иметь один жестко заданный способ подключения. Вместо этого используется Capability Engine, который выбирает лучший доступный backend.

## 5.4. Explicit Fallback

Если интерактивный режим недоступен, UI обязан показывать причину и предлагать fallback.

Пример:

```text
Enhanced Session unavailable:
Remote Desktop Services disabled inside guest.

RDP Relay unavailable:
No reachable guest IP from host.

Preview available:
WMI thumbnail backend active.

Recommended action:
Open Basic Rescue.
```

## 5.5. Experimental Isolation

AF_HYPERV backend не должен влиять на стабильность production-функций. Он должен быть:

* за feature flag;
* отключен по умолчанию;
* изолирован в worker;
* покрыт отдельным POC checklist;
* отключаемым через policy.

---

# 6. Режимы подключения

## 6.1. Mode 0: Offline

Условия:

* VM выключена;
* VM saved;
* VM не имеет активного display state.

Доступные операции:

* start;
* view configuration;
* view checkpoints;
* view last known state;
* view audit history.

Недоступно:

* live thumbnail;
* interactive session;
* keyboard rescue.

## 6.2. Mode 1: PreviewOnly

Условия:

* VM запущена или paused;
* thumbnail доступен;
* интерактивный backend недоступен.

Доступные операции:

* live thumbnail;
* power actions;
* checkpoints;
* status/heartbeat;
* diagnostics.

Недоступно:

* полноценный keyboard/mouse control;
* clipboard;
* 30 FPS.

Целевой FPS:

* 1–2 FPS.

## 6.3. Mode 2: BasicRescue

Условия:

* VM запущена;
* WMI thumbnail доступен;
* доступен WMI keyboard backend или иной supported rescue-input path;
* interactive backend недоступен или degraded.

Доступные операции:

* live thumbnail;
* send Ctrl+Alt+Del;
* send virtual keys;
* type text;
* controlled paste-as-keystrokes;
* power actions;
* checkpoints.

Назначение:

* восстановление сети;
* включение RDP;
* исправление boot/login issues;
* минимальная диагностика.

Ограничения:

* не является полноценным remote desktop;
* не гарантирует плавный UI;
* не поддерживает полноценный clipboard;
* не гарантирует мышь во всех сценариях.

## 6.4. Mode 3: RdpRelay

Условия:

* VM имеет IP;
* RDP/xRDP включен;
* Host Agent может достучаться до guest endpoint;
* policy разрешает RDP relay.

Путь:

```text
Admin GUI Client ⇄ Broker/Relay ⇄ Host Agent ⇄ VM:3389
```

Доступные операции:

* полноценная RDP-сессия;
* keyboard/mouse;
* clipboard через RDP virtual channel;
* optional audio;
* optional session recording;
* reconnect.

Ограничения:

* зависит от vNIC;
* зависит от firewall внутри guest;
* зависит от RDP/xRDP service.

## 6.5. Mode 4: EnhancedSession

Условия:

* VM поддерживает Enhanced Session;
* host policy разрешает Enhanced Session;
* guest OS поддерживает нужный стек;
* Remote Desktop Services доступны внутри VM;
* Capability Engine подтверждает EnhancedSessionModeState.

Доступные операции:

* интерактивная сессия;
* keyboard/mouse;
* clipboard при поддержке канала;
* local resources при разрешении policy.

Ограничения:

* не универсально для всех ОС;
* зависит от настроек host и guest;
* не должно определяться только по heartbeat.

## 6.6. Mode 5: ExperimentalHvSocket

Условия:

* feature flag включен;
* tenant policy разрешает experimental backend;
* POC checklist пройден;
* известен endpoint/service GUID;
* доказан стабильный duplex stream;
* доказан reconnect;
* доказан input path;
* clipboard явно supported или disabled.

Статус:

* не является production-core;
* не является обязательным для MVP;
* не должен быть единственным путем интерактивного доступа.

---

# 7. Capability Engine

## 7.1. Назначение

Capability Engine определяет, какие режимы доступны для каждой VM, с какой уверенностью, по какой причине и какие remediation steps возможны.

## 7.2. Модель данных

```rust
pub enum CapabilityState {
    Available,
    Degraded,
    BlockedByPolicy,
    Unsupported,
    Unknown,
    Experimental,
}

pub enum SessionMode {
    Offline,
    PreviewOnly,
    BasicRescue,
    RdpRelay,
    EnhancedSession,
    ExperimentalHvSocket,
}

pub struct Capability {
    pub state: CapabilityState,
    pub confidence: u8,
    pub reason_code: String,
    pub human_reason: String,
    pub remediation: Option<String>,
}

pub struct VmCapabilityGraph {
    pub vm_id: String,
    pub host_id: String,
    pub preview: Capability,
    pub keyboard_rescue: Capability,
    pub rdp_relay: Capability,
    pub enhanced_session: Capability,
    pub hv_socket: Capability,
    pub clipboard: Capability,
    pub recording: Capability,
    pub constraints: Vec<String>,
    pub recommended_mode: SessionMode,
}
```

## 7.3. Источники данных

Capability Engine использует:

* `Msvm_ComputerSystem`;
* `Msvm_SummaryInformation`;
* `Msvm_VirtualSystemSettingData`;
* `Msvm_VirtualSystemManagementService`;
* network probe с host до guest;
* RDP port check;
* integration services state;
* EnhancedSessionModeState;
* policy engine;
* shielded VM metadata;
* cluster owner node;
* local agent health.

## 7.4. Запрещенная логика

Запрещено считать:

```text
Heartbeat == Ok => Enhanced Session доступен
```

Корректная логика:

```text
Enhanced Session candidate =
  heartbeat OK
  AND EnhancedSessionModeState indicates availability
  AND host policy allows enhanced session
  AND guest requirements satisfied
  AND product policy allows interactive access
```

## 7.5. Примеры reason_code

```text
VM_OFFLINE
VM_BOOTING
NO_HEARTBEAT
PREVIEW_AVAILABLE
PREVIEW_NOT_SUPPORTED
RDP_PORT_CLOSED
NO_GUEST_IP
RDP_BLOCKED_BY_POLICY
ENHANCED_DISABLED_ON_HOST
ENHANCED_UNAVAILABLE_FOR_VM
GUEST_RDS_DISABLED
SHIELDED_VM_RESTRICTED
HVSOCKET_EXPERIMENTAL_DISABLED
HVSOCKET_POC_NOT_PASSED
CLUSTER_OWNER_CHANGED
```

---

# 8. Admin GUI Client

## 8.1. Назначение

Admin GUI Client является основным интерфейсом администратора.

Функции:

* авторизация пользователя;
* отображение fleet dashboard;
* отображение VM cards;
* открытие preview/rescue/RDP/enhanced sessions;
* обработка input;
* локальный clipboard;
* session recording indicator;
* policy prompts;
* diagnostics UI.

## 8.2. Компоненты

```text
Admin GUI Client
  ├── Auth Client
  ├── Dashboard UI
  ├── VM Card Renderer
  ├── Session UX
  ├── RDP Decoder Adapter
  ├── Clipboard Manager
  ├── Input Manager
  ├── Policy Enforcement Adapter
  ├── Local Cache
  ├── Metrics Reporter
  └── Crash Reporter
```

## 8.3. Dashboard UI

Dashboard должен показывать:

* список хостов;
* список ВМ;
* live thumbnails;
* power state;
* heartbeat;
* enhanced state;
* IP addresses;
* recommended connection mode;
* warning badges;
* cluster owner;
* checkpoint count;
* replication status при наличии;
* last session;
* policy constraints.

## 8.4. VM Card

Каждая VM card должна содержать:

```text
- VM name
- VM ID
- Host
- Owner node
- Power state
- Heartbeat
- Thumbnail
- Recommended mode
- Capability badges
- Connect button
- Rescue button
- Explain button
- More actions
```

## 8.5. Session UX

При нажатии Connect клиент не должен слепо открывать один backend. Он должен запросить у Broker/Agent capability graph и выбрать лучший режим.

Порядок предпочтения:

```text
1. EnhancedSession, если доступен и policy разрешает
2. RdpRelay, если доступен и policy разрешает
3. BasicRescue
4. PreviewOnly
5. Offline view
```

ExperimentalHvSocket может быть выше Enhanced/RDP только если:

* включен feature flag;
* прошел POC;
* policy разрешает;
* выбран explicit experimental mode.

## 8.6. Clipboard Manager

Требования:

* использовать `arboard` только на стороне GUI-клиента;
* не использовать `arboard` в Host Agent;
* gracefully disable clipboard при отсутствии GUI-сессии;
* поддерживать text-only mode;
* поддерживать max clipboard size;
* поддерживать policy prompt before paste;
* не отправлять clipboard автоматически без policy approval.

Режимы:

```text
RDP Relay:
  clipboard через RDP clipboard virtual channel.

Enhanced Session:
  clipboard через supported RDP/enhanced path.

Basic Rescue:
  paste-as-keystrokes только по явному действию пользователя.

PreviewOnly:
  clipboard unavailable.

ExperimentalHvSocket:
  clipboard disabled до отдельного POC.
```

## 8.7. Input Manager

Input Manager должен поддерживать:

* keyboard input;
* mouse input для интерактивных backend’ов;
* special key sequences;
* Ctrl+Alt+Del;
* paste-as-keystrokes в BasicRescue;
* rate limiting;
* input audit metadata.

Запрещено:

* отправлять произвольные команды на host;
* выполнять shell execution через Agent;
* обходить guest authentication.

---

# 9. Hyper-V Host Agent

## 9.1. Назначение

Host Agent является локальным privileged-компонентом на Hyper-V host.

Функции:

* локальный доступ к Hyper-V APIs;
* inventory;
* thumbnails;
* power operations;
* checkpoints;
* capability evaluation;
* session backends;
* metrics;
* audit buffer;
* secure communication with Broker.

## 9.2. Требования к службе

Host Agent должен:

* работать как Windows Service;
* запускаться в Session 0;
* не требовать интерактивного пользователя;
* не использовать GUI APIs для screen capture;
* не запускать `vmconnect.exe`;
* не использовать `PrintWindow`;
* не зависеть от desktop clipboard;
* автоматически перезапускаться при crash;
* иметь health endpoint;
* иметь local logs;
* иметь rate limits.

## 9.3. Компоненты Agent

```text
Hyper-V Host Agent
  ├── Agent Core
  ├── Secure Transport
  ├── Enrollment Client
  ├── Policy Cache
  ├── Capability Engine
  ├── WMI/CIM Provider
  ├── VM Inventory Service
  ├── Thumbnail Engine
  ├── Keyboard Rescue Engine
  ├── Power Control Service
  ├── Checkpoint Service
  ├── RDP Relay Backend
  ├── Enhanced Session Backend
  ├── AF_HYPERV Experimental Backend
  ├── Worker Supervisor
  ├── Metrics Collector
  ├── Audit Buffer
  ├── ETW/EventLog Adapter
  └── Secure Key Store
```

## 9.4. Runtime

Рекомендуемый стек:

```text
Language:
  Rust

Async runtime:
  Tokio

Windows API:
  windows-rs / windows crate

WMI/CIM:
  native COM/WMI integration or safe wrapper

Transport:
  TLS 1.3 over TCP
  optional QUIC
  optional relay protocol

Serialization:
  protobuf / postcard / bincode with versioning

Logging:
  tracing
  Windows Event Log adapter
  optional ETW
```

## 9.5. Worker Isolation

Agent Core не должен выполнять тяжелые session operations напрямую.

Нужно разделить:

```text
Agent Core:
  lightweight control plane, policy, routing.

Thumbnail Worker Pool:
  polling thumbnails.

Session Worker:
  RDP relay / Enhanced / rescue session.

Experimental Worker:
  AF_HYPERV backend.

Metrics Worker:
  async metrics collection.
```

Требования:

* падение worker не должно ронять Agent Core;
* session worker должен иметь timeout;
* thumbnail worker должен иметь CPU budget;
* experimental worker должен быть killable;
* backpressure обязателен.

---

# 10. WMI/CIM Inventory

## 10.1. Назначение

Сбор актуальных данных о ВМ на Hyper-V host.

## 10.2. Namespace

Использовать:

```text
root\virtualization\v2
```

## 10.3. Основные классы

Использовать:

```text
Msvm_ComputerSystem
Msvm_SummaryInformation
Msvm_VirtualSystemSettingData
Msvm_VirtualSystemManagementService
```

Допускается использование дополнительных классов Hyper-V WMI Provider для:

* keyboard;
* synthetic devices;
* networking;
* checkpoints;
* replication;
* cluster mapping.

## 10.4. VmInfo

```rust
pub struct VmInfo {
    pub vm_id: String,
    pub name: String,
    pub host_id: String,
    pub owner_node: Option<String>,
    pub enabled_state: VmPowerState,
    pub heartbeat: Option<HeartbeatState>,
    pub enhanced_session_state: Option<EnhancedSessionState>,
    pub integration_services: Vec<IntegrationServiceInfo>,
    pub guest_ips: Vec<String>,
    pub generation: Option<u8>,
    pub uptime_seconds: Option<u64>,
    pub cpu_usage_percent: Option<f32>,
    pub memory_assigned_mb: Option<u64>,
    pub checkpoint_count: Option<u32>,
    pub replication_state: Option<String>,
    pub is_shielded: Option<bool>,
    pub recommended_mode: SessionMode,
    pub capabilities: VmCapabilityGraph,
}
```

## 10.5. Polling

Inventory polling должен быть адаптивным:

```text
Active dashboard:
  every 2–5 seconds for status metadata.

Inactive dashboard:
  every 10–30 seconds.

Host under load:
  exponential backoff.

Session active:
  priority updates for selected VM.
```

## 10.6. Запрещено

* хардкодить VM state;
* использовать static config вместо WMI;
* запускать PowerShell subprocess на hot path;
* парсить вывод GUI/CLI, если доступен WMI/CIM API.

---

# 11. Thumbnail Engine

## 11.1. Назначение

Предоставлять live-preview экранов ВМ для dashboard и rescue-режима.

## 11.2. Источник данных

Использовать метод:

```text
Msvm_VirtualSystemManagementService.GetVirtualSystemThumbnailImage
```

## 11.3. Формат

Ожидаемый формат входных данных:

```text
raw RGB565
```

Agent должен конвертировать изображение в формат, пригодный для передачи клиенту.

## 11.4. Pipeline

```text
WMI Thumbnail
  -> raw RGB565
  -> decode to RGB/RGBA
  -> optional resize
  -> duplicate frame detection
  -> encode WebP/JPEG/PNG
  -> send to client
```

## 11.5. Размеры

Рекомендуемые размеры:

```text
Dashboard card:
  320x180

Focused preview:
  640x360 or 800x450

Rescue mode:
  configurable, default 800x450
```

## 11.6. Частота

```text
Dashboard active:
  1.5–2 seconds per VM

Dashboard inactive:
  5–10 seconds per VM

VM off/saved:
  no polling

Selected rescue VM:
  1–2 FPS target

Host under load:
  adaptive backoff
```

## 11.7. CPU Budget

Agent должен иметь глобальный CPU budget для thumbnail engine.

Требования:

```text
15+ VM thumbnails:
  host CPU overhead target <= 5%

Duplicate frame:
  не отправлять повторный кадр, если image hash не изменился.

Slow client:
  drop old frames, keep latest.
```

## 11.8. Ошибки

Agent должен различать:

```text
AccessDenied
NotSupported
Timeout
InvalidState
VmUnavailable
HostOverloaded
WmiFailure
```

UI должен показывать human-readable reason.

---

# 12. Keyboard Rescue Engine

## 12.1. Назначение

Обеспечить минимальный rescue-control в ситуациях, когда полноценный интерактивный backend недоступен.

## 12.2. Возможности

Минимальный набор:

```text
- Send Ctrl+Alt+Del
- Send Enter
- Send Escape
- Send Tab
- Send function keys
- Type ASCII text
- Type limited Unicode text where supported
- Paste-as-keystrokes
```

## 12.3. UX

Basic Rescue UI должен отображать:

```text
- live thumbnail
- virtual keyboard actions
- send Ctrl+Alt+Del
- type text box
- paste as keystrokes
- delay between characters
- status of last input operation
```

## 12.4. Ограничения

Basic Rescue:

* не гарантирует мышь;
* не гарантирует 30 FPS;
* не поддерживает настоящий clipboard;
* не должен имитировать полноценный RDP;
* должен иметь явный warning в UI.

## 12.5. Audit

Каждое rescue-действие должно попадать в audit metadata:

```text
timestamp
user_id
vm_id
host_id
action_type
backend
success/failure
```

Для typed text audit не должен хранить секреты по умолчанию.
Допускается хранить только metadata:

```text
typed_text_length
redacted=true
```

---

# 13. RDP Relay Backend

## 13.1. Назначение

Обеспечить интерактивный доступ к гостевой ОС через RDP/xRDP, если VM доступна с Hyper-V host по сети.

## 13.2. Схема

```text
Admin GUI Client
  ⇄ encrypted app protocol
Host Agent
  ⇄ TCP
Guest VM RDP/xRDP endpoint
```

## 13.3. Probe

Перед открытием backend Agent должен проверить:

```text
- наличие guest IP;
- reachability с host;
- port availability;
- policy;
- user authorization;
- optional TLS/RDP negotiation metadata.
```

## 13.4. Transport

Host Agent выступает как TCP relay/proxy.

Требования:

* backpressure;
* timeout;
* reconnect;
* traffic accounting;
* session termination on policy revoke;
* no plaintext relay outside local host-to-guest segment unless allowed.

## 13.5. Client Decoder

Admin GUI Client должен декодировать RDP stream через:

```text
- FreeRDP wrapper
или
- Rust RDP implementation
```

Требования:

* поддержка keyboard/mouse;
* поддержка resolution resize;
* поддержка clipboard virtual channel;
* graceful reconnect;
* FPS/latency metrics.

## 13.6. Clipboard

В RDP Relay clipboard должен идти через RDP Clipboard Virtual Channel.

Policy должна уметь:

```text
- disable clipboard;
- text only;
- max size;
- block file copy;
- require confirmation;
- log clipboard event metadata.
```

---

# 14. Enhanced Session Backend

## 14.1. Назначение

Предоставить интерактивный доступ через Enhanced Session, если host и guest поддерживают данный режим.

## 14.2. Условия доступности

Enhanced Session может быть доступна только если:

```text
- VM running;
- heartbeat OK;
- EnhancedSessionModeState indicates availability;
- host policy allows enhanced sessions;
- guest OS supports required components;
- Remote Desktop Services available where required;
- product policy allows this backend.
```

## 14.3. Запрещено

Запрещено считать Enhanced Session доступной только по:

```text
Heartbeat == Ok
```

## 14.4. UX

UI должен показывать:

```text
Enhanced Session: Available
или
Enhanced Session: Unavailable — reason
```

Примеры причин:

```text
ENHANCED_DISABLED_ON_HOST
ENHANCED_UNAVAILABLE_FOR_VM
GUEST_RDS_DISABLED
UNSUPPORTED_GUEST_OS
BLOCKED_BY_POLICY
UNKNOWN_STATE
```

## 14.5. Clipboard

Clipboard в Enhanced Session должен использовать поддерживаемый RDP/enhanced path.
Если такой канал недоступен, clipboard должен быть disabled или переведен в paste-as-keystrokes mode по явному действию пользователя.

---

# 15. AF_HYPERV Experimental Backend

## 15.1. Статус

AF_HYPERV backend является experimental и не входит в production promise MVP.

## 15.2. Цель POC

Проверить возможность стабильного интерактивного transport path через Hyper-V Sockets без guest agent.

## 15.3. Ограничения

До завершения POC запрещено обещать:

```text
- гарантированный RDP stream через AF_HYPERV;
- 30–60 FPS через AF_HYPERV;
- clipboard через AF_HYPERV;
- input injection через AF_HYPERV;
- работу для всех гостевых ОС;
- работу без vNIC во всех случаях.
```

## 15.4. POC Checklist

AF_HYPERV backend считается кандидатом на production только если выполнены все пункты:

```text
1. Найден documented или legal-safe endpoint/service GUID.
2. Подтвержден стабильный connect.
3. Подтвержден стабильный disconnect.
4. Подтвержден reconnect.
5. Подтвержден duplex stream.
6. Описан protocol framing.
7. Подтвержден input path.
8. Подтвержден display path.
9. Clipboard либо подтвержден, либо явно disabled.
10. Работает после guest reboot.
11. Работает после VM pause/resume.
12. Работает после save/restore.
13. Поведение при live migration описано.
14. Worker crash не роняет Agent Core.
15. Backend можно отключить через policy.
16. Нет зависимости от vmconnect.exe.
17. Нет зависимости от GUI session на host.
18. Нет reverse-engineering риска, блокирующего production.
```

## 15.5. Feature Flag

AF_HYPERV должен управляться флагами:

```text
hv_socket_backend.enabled
hv_socket_backend.allowed_tenants
hv_socket_backend.allowed_hosts
hv_socket_backend.allowed_vms
hv_socket_backend.experimental_warning_required
```

## 15.6. UI Warning

При использовании backend UI должен явно показывать:

```text
Experimental backend active.
Stability and compatibility are not guaranteed.
```

---

# 16. Control Plane / Broker

## 16.1. Назначение

Control Plane является центральной точкой управления доступом.

Функции:

* enrollment host agents;
* user identity;
* SSO/MFA;
* RBAC;
* policy engine;
* session brokerage;
* relay coordination;
* audit;
* update distribution;
* fleet health.

## 16.2. Host Registry

Control Plane хранит:

```rust
pub struct HostRecord {
    pub host_id: String,
    pub tenant_id: String,
    pub hostname: String,
    pub os_version: String,
    pub agent_version: String,
    pub hyperv_version: Option<String>,
    pub cluster_id: Option<String>,
    pub last_seen_at: DateTimeUtc,
    pub health: HostHealth,
    pub capabilities: Vec<String>,
}
```

## 16.3. VM Registry

Control Plane хранит lightweight metadata:

```rust
pub struct VmRecord {
    pub vm_id: String,
    pub host_id: String,
    pub cluster_id: Option<String>,
    pub owner_node: Option<String>,
    pub name: String,
    pub power_state: String,
    pub last_seen_at: DateTimeUtc,
    pub tags: Vec<String>,
    pub policy_refs: Vec<String>,
}
```

## 16.4. Session Broker

Session Broker отвечает за:

```text
- проверку прав пользователя;
- выбор host agent;
- выбор backend;
- выдачу short-lived session token;
- настройку relay path;
- применение session policy;
- завершение сессии при revoke.
```

## 16.5. Policy Engine

Policy Engine должен поддерживать:

```text
- per-tenant policies;
- per-host policies;
- per-VM policies;
- per-user/group policies;
- time-based policies;
- JIT policies;
- clipboard policies;
- recording policies;
- experimental backend policies.
```

## 16.6. Approval Workflow

Для sensitive VM должна быть поддержка approval:

```text
User requests access
  -> Approver receives request
  -> Approver grants temporary access
  -> Session token issued
  -> Audit log created
```

## 16.7. Audit Log

Control Plane должен хранить audit events.

Пример:

```rust
pub struct AuditEvent {
    pub event_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub host_id: Option<String>,
    pub vm_id: Option<String>,
    pub session_id: Option<String>,
    pub event_type: String,
    pub backend: Option<String>,
    pub timestamp: DateTimeUtc,
    pub metadata: serde_json::Value,
}
```

## 16.8. Update Channel

Control Plane должен обеспечивать:

```text
- signed agent updates;
- staged rollout;
- canary hosts;
- rollback;
- version pinning;
- compatibility checks;
- forced security update.
```

---

# 17. Relay / Edge Network

## 17.1. Назначение

Обеспечить соединение между Admin GUI Client и Host Agent в сетях с NAT/firewall.

## 17.2. Режимы

```text
Direct:
  client connects directly to agent where allowed.

Reverse tunnel:
  agent maintains outbound connection to broker/relay.

Relay:
  both client and agent connect outbound to edge node.

Hybrid:
  start via relay, upgrade to direct if possible.
```

## 17.3. Требования

Relay должен поддерживать:

```text
- authentication;
- session token validation;
- bandwidth accounting;
- idle timeout;
- reconnect;
- regional routing;
- congestion control;
- backpressure;
- no unauthorized cross-tenant routing.
```

## 17.4. Security

Relay не должен иметь возможности инициировать сессию без Broker authorization.

Опционально:

```text
- E2E encryption between Client and Agent;
- relay sees metadata only;
- session content opaque to relay.
```

---

# 18. Security Model

## 18.1. Identity

Поддержать:

```text
- local product account;
- SSO;
- MFA;
- service account for agent;
- device enrollment.
```

## 18.2. Authorization

RBAC роли:

```text
Viewer:
  view dashboard and thumbnails only.

Operator:
  connect, rescue, power operations allowed by policy.

Admin:
  manage policies, agents, users.

Auditor:
  view audit and recordings.

BreakGlass:
  emergency access with mandatory audit and optional approval.
```

## 18.3. Per-VM ACL

Доступ должен назначаться на:

```text
- tenant;
- host;
- cluster;
- VM;
- tag;
- user;
- group.
```

## 18.4. JIT Access

Временный доступ:

```text
- duration;
- reason;
- approver;
- allowed backend;
- clipboard policy;
- recording policy.
```

## 18.5. Clipboard Policy

```text
Disabled
TextOnly
TextWithConfirmation
FullRdpClipboard
BlockedForSensitiveVm
```

File clipboard должен быть disabled by default.

## 18.6. Session Recording Policy

```text
Disabled
MetadataOnly
EventLogOnly
VideoOptional
VideoRequired
```

## 18.7. Agent Hardening

Host Agent должен:

```text
- использовать подписанный бинарник;
- проверять подпись обновлений;
- хранить ключи через защищенное хранилище Windows;
- не принимать произвольные команды;
- иметь allowlist операций;
- иметь rate limits;
- логировать privileged actions;
- поддерживать local tamper detection;
- не открывать inbound port без явной настройки.
```

## 18.8. Shielded VM Awareness

Если VM является shielded или имеет ограничения guarded fabric, продукт должен:

* уважать ограничения;
* не пытаться обходить protected boundary;
* скрывать недоступные действия;
* показывать reason;
* логировать попытки запрещенного доступа.

---

# 19. Session Recording и Forensics

## 19.1. Уровни записи

```text
Level 0:
  no recording.

Level 1:
  metadata only.

Level 2:
  event timeline.

Level 3:
  video/session recording.
```

## 19.2. Metadata

Записывать:

```text
- user;
- VM;
- host;
- backend;
- start time;
- end time;
- IP/client device;
- policy;
- approval id;
- clipboard events metadata;
- power actions.
```

## 19.3. Event Timeline

Пример:

```text
14:03 Session started via BasicRescue
14:04 Ctrl+Alt+Del sent
14:05 Paste blocked by policy
14:06 Network restored
14:07 Switched to RDP Relay
14:11 Checkpoint created
14:22 Session ended
```

## 19.4. Privacy

По умолчанию не хранить:

* plaintext clipboard;
* typed passwords;
* full keyboard stream;
* sensitive screen recording без policy.

---

# 20. Cluster Awareness

## 20.1. Назначение

Поддержать Hyper-V hosts в Failover Cluster.

## 20.2. Требования

Agent/Broker должен уметь:

```text
- определять owner node VM;
- понимать cluster membership;
- обновлять VM location;
- поддерживать reconnect после live migration;
- отображать cluster health;
- избегать подключения к неправильному host;
- корректно завершать session при migration conflict.
```

## 20.3. UX

VM card должна показывать:

```text
Owner node:
  HV-NODE-02

Cluster:
  PROD-HV-CLUSTER

Migration:
  in progress / completed / failed
```

---

# 21. Observability

## 21.1. Agent Metrics

Собирать:

```text
- agent CPU/RAM;
- WMI query duration;
- thumbnail polling latency;
- thumbnail encode time;
- frames sent/dropped;
- RDP relay throughput;
- session latency;
- input latency;
- reconnect count;
- worker crash count;
- policy cache age;
- broker connection health.
```

## 21.2. Host Metrics

Собирать при наличии:

```text
- host CPU;
- host memory pressure;
- VMMS status;
- disk/storage latency;
- Hyper-V service health;
- cluster state;
- network switch state.
```

## 21.3. Client Metrics

Собирать:

```text
- decode FPS;
- render FPS;
- input latency;
- network RTT;
- clipboard failures;
- reconnect attempts;
- crash reports.
```

## 21.4. Logging

Использовать структурированные logs:

```text
timestamp
level
component
tenant_id
host_id
vm_id
session_id
event
fields
```

## 21.5. Windows Event Log / ETW

Agent должен уметь писать critical/security events в Windows Event Log.
Для performance diagnostics желательно предусмотреть ETW-compatible tracing adapter.

---

# 22. Сетевой протокол приложения

## 22.1. Общие требования

Протокол должен поддерживать:

```text
- multiplexed streams;
- request/response;
- server push events;
- binary frame transport;
- backpressure;
- compression;
- versioning;
- heartbeats;
- reconnect;
- session resumption.
```

## 22.2. Каналы

```text
Control Channel:
  session setup, capabilities, policy, commands.

Thumbnail Channel:
  low-FPS image frames.

Interactive Channel:
  RDP/Enhanced stream or backend-specific stream.

Input Channel:
  keyboard/mouse events.

Clipboard Channel:
  policy-controlled clipboard events.

Metrics Channel:
  telemetry and health.

Audit Channel:
  reliable event delivery.
```

## 22.3. Frame Header

Пример:

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

## 22.4. Versioning

Каждое сообщение должно иметь версию.

Требования:

* backward compatibility на N-1 версию;
* graceful error при unsupported version;
* capability negotiation;
* feature flags.

## 22.5. Compression

Для thumbnail:

```text
WebP/JPEG/PNG payload-level compression
```

Для control messages:

```text
no compression или lightweight compression
```

Для RDP relay:

```text
не сжимать повторно, если RDP уже использует собственное сжатие
```

---

# 23. API-контракты

## 23.1. GetHosts

Request:

```json
{
  "tenant_id": "t1"
}
```

Response:

```json
{
  "hosts": [
    {
      "host_id": "h1",
      "hostname": "HV-01",
      "agent_version": "1.0.0",
      "health": "Healthy",
      "last_seen_at": "2026-06-15T10:00:00Z"
    }
  ]
}
```

## 23.2. GetVMs

Request:

```json
{
  "host_id": "h1"
}
```

Response:

```json
{
  "vms": [
    {
      "vm_id": "vm-001",
      "name": "DC-01",
      "power_state": "Running",
      "heartbeat": "Ok",
      "recommended_mode": "RdpRelay"
    }
  ]
}
```

## 23.3. GetVmCapabilityGraph

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
    "enhanced_session": {
      "state": "Degraded",
      "confidence": 70,
      "reason_code": "ENHANCED_UNAVAILABLE_FOR_VM"
    },
    "hv_socket": {
      "state": "Experimental",
      "confidence": 20,
      "reason_code": "HVSOCKET_EXPERIMENTAL_DISABLED"
    }
  }
}
```

## 23.4. OpenSession

Request:

```json
{
  "vm_id": "vm-001",
  "requested_mode": "Auto",
  "reason": "Network recovery",
  "client_capabilities": {
    "rdp_decoder": true,
    "clipboard_text": true
  }
}
```

Response:

```json
{
  "session_id": "s-001",
  "selected_mode": "BasicRescue",
  "policy": {
    "clipboard": "TextWithConfirmation",
    "recording": "EventLogOnly",
    "max_duration_seconds": 3600
  },
  "transport": {
    "relay_required": true,
    "token": "short-lived-token"
  }
}
```

## 23.5. StartThumbnailStream

Request:

```json
{
  "vm_id": "vm-001",
  "width": 320,
  "height": 180,
  "target_fps": 1
}
```

Frame Event:

```json
{
  "session_id": "s-001",
  "vm_id": "vm-001",
  "frame_id": 1001,
  "format": "webp",
  "width": 320,
  "height": 180,
  "timestamp": "2026-06-15T10:00:00Z"
}
```

## 23.6. SendRescueInput

Request:

```json
{
  "session_id": "s-001",
  "input_type": "TypeText",
  "text": "ipconfig /renew",
  "redact_audit": true
}
```

Response:

```json
{
  "ok": true,
  "action_id": "a-001"
}
```

---

# 24. Производительность

## 24.1. Dashboard

Acceptance target:

```text
15+ VM thumbnails:
  <= 5% additional host CPU under normal conditions

Thumbnail FPS:
  1–2 FPS active dashboard

Inactive:
  adaptive backoff

Duplicate frame:
  no retransmit
```

## 24.2. Interactive

RDP/Enhanced target:

```text
>= 30 FPS target
input lag < 50 ms target
60 FPS best effort
```

BasicRescue target:

```text
1–2 FPS
input confirmation within 500 ms where possible
```

## 24.3. Agent

Agent must:

```text
- avoid blocking Tokio runtime;
- isolate blocking WMI calls;
- use worker pool;
- apply bounded queues;
- drop stale frames;
- enforce per-VM limits;
- enforce per-host CPU budget.
```

---

# 25. Надежность

## 25.1. Reconnect

Система должна поддерживать:

```text
- client reconnect;
- agent reconnect to broker;
- relay reconnect;
- session timeout;
- session resume where backend allows;
- fallback to preview if interactive backend fails.
```

## 25.2. Crash Isolation

Падение session worker не должно:

* останавливать Agent Core;
* ломать другие сессии;
* ломать thumbnail engine;
* оставлять dangling session без cleanup.

## 25.3. Backpressure

При медленной сети:

```text
Thumbnail:
  drop old frames, keep latest.

Interactive:
  apply transport backpressure.

Audit:
  reliable local buffer with retry.
```

## 25.4. Offline Broker

Agent должен иметь local policy cache.

При недоступности Broker:

```text
- новые privileged sessions запрещены или ограничены policy;
- уже открытые сессии работают до TTL;
- audit buffer сохраняется локально;
- при восстановлении связи audit отправляется в Broker.
```

---

# 26. Безопасность операций с ВМ

## 26.1. Power Operations

Поддерживаемые операции:

```text
Start
Shutdown
TurnOff
Reset
Save
Pause
Resume
```

Каждая операция требует:

* authorization;
* policy check;
* audit event;
* confirmation для destructive actions.

## 26.2. Checkpoints

Поддерживаемые операции:

```text
List checkpoints
Create checkpoint
Apply checkpoint
Delete checkpoint
Rename checkpoint
```

Требования:

* confirmation for apply/delete;
* audit;
* optional approval for production VMs;
* policy to disable checkpoints.

## 26.3. Destructive Action Protection

Для production/sensitive VM:

```text
Reset
TurnOff
ApplyCheckpoint
DeleteCheckpoint
```

должны требовать:

* explicit confirmation;
* optional MFA;
* optional approval;
* reason field.

---

# 27. UI/UX требования

## 27.1. Dashboard

Главные элементы:

```text
- Host list
- VM grid/list
- Live thumbnails
- Status badges
- Search/filter
- Group by host/cluster/tag
- Sort by state/risk/last activity
```

## 27.2. VM Details

Показывать:

```text
- Summary
- Capabilities
- Sessions
- Checkpoints
- Network
- Integration services
- Audit
- Diagnostics
```

## 27.3. Explain Button

Для каждой недоступной функции должна быть кнопка Explain.

Пример:

```text
Why can't I open RDP?
- VM has no IP address visible to host.
- RDP port 3389 is not reachable.
- Recommended: Open Basic Rescue.
```

## 27.4. Smart Connect

Кнопка Connect должна:

```text
1. запросить Capability Graph;
2. применить policy;
3. выбрать лучший backend;
4. показать confirmation, если нужно;
5. открыть session.
```

## 27.5. Rescue Wizard

Сценарий восстановления сети:

```text
1. Show thumbnail.
2. Send Ctrl+Alt+Del.
3. Type credentials manually.
4. Open Run dialog or shell.
5. Paste/type remediation command.
6. Agent detects IP.
7. UI suggests switch to RDP Relay.
```

---

# 28. Совместимость

## 28.1. Host OS

Целевые host OS:

```text
Windows Server 2016+
Windows Server 2019
Windows Server 2022
Windows Server 2025
Windows 10/11 Pro/Enterprise with Hyper-V for dev/test
```

## 28.2. Guest OS

Поддержка по уровням:

```text
Windows guest:
  Preview, BasicRescue, RDP Relay, Enhanced candidate.

Linux guest:
  Preview, BasicRescue where possible, RDP/xRDP Relay if configured.

Other OS:
  PreviewOnly unless supported backend exists.
```

## 28.3. Shielded VM

Shielded VM поддерживается только с учетом ограничений безопасности.
Если preview/console запрещены, UI показывает reason.

---

# 29. Packaging и Deployment

## 29.1. Host Agent Installer

Требования:

```text
- MSI installer;
- signed binary;
- service installation;
- enrollment token input;
- proxy settings;
- silent install;
- upgrade support;
- rollback support.
```

## 29.2. Client Installer

Требования:

```text
- signed installer;
- auto-update;
- crash reporting opt-in;
- local cache cleanup;
- support Windows first.
```

## 29.3. Broker Deployment

Варианты:

```text
Cloud SaaS
Self-hosted
Hybrid
```

Self-hosted должен поддерживать:

```text
- Docker/Kubernetes or Windows/Linux service;
- external database;
- TLS;
- backup/restore;
- HA mode.
```

---

# 30. Тестирование

## 30.1. Unit Tests

Покрыть:

```text
- capability decision logic;
- policy evaluation;
- frame protocol parser;
- thumbnail conversion;
- authorization checks;
- audit event creation;
- retry/backoff logic.
```

## 30.2. Integration Tests

Покрыть:

```text
- WMI inventory;
- thumbnail retrieval;
- power operations;
- checkpoint operations;
- RDP relay;
- clipboard policy;
- rescue input;
- agent reconnect;
- broker reconnect.
```

## 30.3. Performance Tests

Сценарии:

```text
- 15 VM thumbnails
- 50 VM inventory
- 100 VM inventory
- slow client
- high host CPU load
- relay bandwidth limit
- RDP session latency
```

## 30.4. Security Tests

Проверить:

```text
- unauthorized access denied;
- expired token rejected;
- cross-tenant access impossible;
- policy revoke terminates session;
- clipboard blocked by policy;
- destructive action requires confirmation;
- audit cannot be bypassed through normal API.
```

## 30.5. Chaos Tests

Сценарии:

```text
- kill session worker;
- restart agent;
- disconnect broker;
- disconnect relay;
- pause/resume VM;
- VM reboot;
- host high CPU;
- WMI timeout;
- cluster owner changed.
```

---

# 31. Acceptance Criteria

## 31.1. Безагентность

Гостевая ОС не содержит стороннего агента продукта.

Допускается использование встроенных возможностей гостевой ОС:

* RDP;
* xRDP;
* Hyper-V integration components;
* стандартные системные службы.

## 31.2. Host-only management

Inventory, power actions, checkpoints и thumbnails выполняются через Host Agent и локальные Hyper-V APIs.

## 31.3. vNIC independence

При отключенном vNIC у ВМ должны работать:

```text
- inventory;
- power state;
- heartbeat where available;
- thumbnail preview where supported;
- power actions;
- checkpoints;
- BasicRescue where input backend available.
```

Интерактивный 30 FPS без vNIC не является hard guarantee, кроме случаев, где supported Enhanced Session backend подтвержден Capability Engine.

## 31.4. Dashboard performance

Для 15+ ВМ:

```text
- live thumbnails 1–2 FPS;
- host CPU overhead target <= 5%;
- no stale frame buildup;
- adaptive backoff при нагрузке.
```

## 31.5. Interactive performance

Для RDP/Enhanced backend:

```text
- target >= 30 FPS;
- input lag target < 50 ms;
- reconnect supported;
- 60 FPS best effort.
```

## 31.6. Fallback

Если интерактивный доступ невозможен, UI должен:

```text
- не падать;
- показать причину;
- предложить PreviewOnly или BasicRescue;
- показать remediation steps.
```

## 31.7. Clipboard

```text
- arboard используется только в GUI Client;
- Host Agent не обращается к desktop clipboard;
- clipboard policy enforced;
- clipboard unavailable в PreviewOnly;
- BasicRescue paste только по явному действию.
```

## 31.8. Security

```text
- mTLS/session tokens;
- RBAC;
- per-VM ACL;
- audit log;
- policy checks;
- session timeout;
- destructive action confirmation;
- signed updates.
```

## 31.9. Stability

```text
- Host Agent работает как Windows Service;
- не зависит от GUI;
- не использует vmconnect.exe;
- не использует PrintWindow;
- crash одного worker не роняет Agent Core.
```

## 31.10. Experimental boundary

AF_HYPERV backend не считается production-ready, пока не пройден POC checklist.

---

# 32. Этапы реализации

## 32.1. Phase 0: Research & POC

Цель:

* подтвердить WMI thumbnail;
* подтвердить WMI inventory;
* подтвердить keyboard rescue feasibility;
* подтвердить RDP relay;
* отдельно исследовать AF_HYPERV.

Deliverables:

```text
- local CLI prototype;
- WMI thumbnail demo;
- inventory JSON output;
- RDP relay prototype;
- AF_HYPERV POC report;
- risk matrix.
```

## 32.2. Phase 1: MVP

Функции:

```text
- Host Agent Windows Service;
- Client dashboard;
- Broker minimal;
- host enrollment;
- VM inventory;
- thumbnails;
- power actions;
- BasicRescue;
- RDP Relay;
- basic RBAC;
- audit metadata.
```

Не входит:

```text
- production AF_HYPERV;
- full cluster support;
- session recording video;
- advanced policy workflow.
```

## 32.3. Phase 2: Production

Функции:

```text
- policy engine;
- relay network;
- SSO/MFA;
- session broker;
- clipboard policy;
- signed updates;
- metrics;
- worker isolation;
- Enhanced Session backend;
- improved rescue workflows.
```

## 32.4. Phase 3: Enterprise

Функции:

```text
- cluster awareness;
- shielded VM awareness;
- approval workflow;
- JIT access;
- session recording;
- forensic timeline;
- HA broker;
- self-hosted deployment;
- advanced observability.
```

## 32.5. Phase 4: Experimental Expansion

Функции:

```text
- AF_HYPERV backend after POC;
- no-vNIC interactive path where proven;
- advanced custom channels;
- future hypervisor adapters.
```

---

# 33. Риски и ограничения

## 33.1. AF_HYPERV Risk

Риск:

```text
AF_HYPERV может не предоставить стабильный supported path для чтения RDP/display stream без guest agent.
```

Митигация:

```text
- не делать AF_HYPERV core MVP;
- вынести в experimental backend;
- провести POC;
- иметь supported fallback.
```

## 33.2. Enhanced Session Risk

Риск:

```text
Enhanced Session зависит от host/guest configuration и не является универсальной.
```

Митигация:

```text
- Capability Engine;
- reason codes;
- fallback to RDP Relay / BasicRescue / PreviewOnly.
```

## 33.3. WMI Performance Risk

Риск:

```text
Частый polling thumbnails может нагрузить host.
```

Митигация:

```text
- adaptive polling;
- CPU budget;
- duplicate frame detection;
- worker pool;
- backoff.
```

## 33.4. Clipboard Security Risk

Риск:

```text
Clipboard может утекать между sensitive environments.
```

Митигация:

```text
- clipboard disabled by default for sensitive VM;
- text-only;
- confirmation;
- audit metadata;
- no file clipboard by default.
```

## 33.5. Privileged Agent Risk

Риск:

```text
Host Agent имеет высокие права на Hyper-V host.
```

Митигация:

```text
- signed binary;
- signed updates;
- allowlist operations;
- no arbitrary shell execution;
- audit;
- least privilege where possible;
- local hardening.
```

---

# 34. Требования к коду

## 34.1. Rust

Требования:

```text
- stable Rust;
- Tokio;
- no unsafe by default;
- unsafe only in isolated Windows API modules;
- structured errors;
- tracing instrumentation;
- integration tests;
- fuzzing for protocol parser.
```

## 34.2. Модульность

Рекомендуемая структура:

```text
crates/
  agent-core/
  agent-wmi/
  agent-thumbnail/
  agent-rdp-relay/
  agent-rescue/
  agent-hvsocket-experimental/
  client-ui/
  client-rdp/
  protocol/
  broker-api/
  policy-engine/
  audit/
  common-types/
```

## 34.3. Ошибки

Использовать typed errors:

```rust
pub enum AgentError {
    WmiUnavailable,
    AccessDenied,
    VmNotFound,
    BackendUnavailable,
    BlockedByPolicy,
    Timeout,
    ProtocolError,
    ExperimentalDisabled,
    UnsupportedGuest,
}
```

## 34.4. Observability

Каждый backend должен иметь tracing spans:

```text
session.open
session.close
thumbnail.poll
thumbnail.encode
wmi.query
rdp.relay.open
rdp.relay.bytes
policy.evaluate
audit.write
```

---

# 35. Запрещенные реализации

В production коде запрещены:

```text
- vmconnect.exe automation;
- PrintWindow;
- Win32 screen capture of VMConnect window;
- dependency on logged-in user session on host;
- host desktop clipboard;
- PowerShell subprocess on hot path;
- hardcoded VM flags;
- storing plaintext credentials;
- arbitrary command execution through agent;
- bypassing Shielded VM constraints;
- AF_HYPERV as mandatory backend before POC.
```

---

# 36. Итоговая формулировка фичи

Фича реализует **Zero-Trust Hyper-V Access Fabric** — платформу безагентного доступа к ВМ Hyper-V через Host Agent.

Production-core системы:

```text
- WMI/CIM inventory
- WMI thumbnails
- VM power/checkpoint control
- BasicRescue
- RDP Relay
- Enhanced Session where supported
- Capability Engine
- Policy Engine
- RBAC
- Audit
- Relay/NAT traversal
```

Experimental expansion:

```text
- AF_HYPERV direct transport
- no-vNIC interactive sessions
- custom VMBus-backed channels
```

Главный принцип:

```text
Система должна быть полезной и стабильной без AF_HYPERV.
AF_HYPERV должен усиливать продукт, но не быть единственной опорой.
```
