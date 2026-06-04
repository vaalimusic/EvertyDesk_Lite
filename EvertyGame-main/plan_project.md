📄 Техническое задание
Проект: Distributed Low-Latency PC Streaming Platform

Домен можно использовать game.everty.ru
1. 🎯 Цель проекта

Создать платформу, позволяющую:

подключаться к удалённым ПК с минимальной задержкой
передавать видео, звук и ввод в реальном времени
сдавать ПК в аренду (по времени)
находить ближайший по задержке ПК
обеспечивать unattended доступ (без подтверждения на стороне ПК)
2. 🧩 Общая архитектура

Система состоит из 4 ключевых компонентов:

2.1 Host Agent (Windows)

Фоновое приложение на ПК-хосте:

управляет стримингом
регистрируется в системе
принимает команды от backend
обеспечивает unattended доступ
2.2 Client App (Android / PC)

Клиентское приложение:

поиск ПК
подключение
отображение стрима
отправка input
2.3 Control Plane (Backend)

Центральный сервер:

авторизация
управление сессиями
биллинг
matchmaking
контроль доступности ПК
2.4 Edge / Relay Nodes

Региональные узлы:

NAT traversal
UDP relay
latency probing
fallback соединение
3. 👤 Роли пользователей
3.1 Player (игрок)
ищет ПК
покупает время
подключается
3.2 Host (владелец ПК)
публикует ПК
задаёт цену
получает доход
3.3 Club Admin
управляет группой ПК
задаёт расписание
управляет тарифами
4. 🔐 Авторизация и безопасность
Требования:
JWT access + refresh tokens
device binding (привязка устройств)
2FA (опционально)
session tokens с TTL
Unattended доступ:
доступ только через backend
только при активной сессии
временные ключи (ephemeral)
whitelist пользователей (опционально)
5. 🖥️ Host Agent (Windows)
Функции:
5.1 Регистрация устройства
уникальный deviceId
привязка к аккаунту
отправка характеристик:
CPU
GPU
RAM
encoder support
5.2 Heartbeat

Каждые 5–10 секунд:

online/offline
загрузка CPU/GPU
network stats
доступность
5.3 Управление сессией
start session
stop session
подготовка окружения
очистка после сессии
5.4 Стриминг

(используется твой текущий стек)

захват экрана
кодирование (H264/H265)
UDP передача
обработка input
5.5 Unattended Mode
включение/выключение
защита через backend
отключение input вне сессии
6. 📱 Client App
Функции:
6.1 Авторизация
login/register
refresh token
6.2 Поиск ПК

Фильтры:

регион
цена
GPU
latency
6.3 Подключение
получение session token
запуск стрима
fallback на relay
6.4 Input
мышь
клавиатура
gamepad
6.5 UI
список ПК
статус подключения
latency / FPS overlay
7. 🔗 Соединение (Connection Flow)
Этапы:
7.1 Matchmaking
выбор ПК
резервирование
7.2 NAT traversal
обмен candidate’ами
тест RTT
7.3 Выбор маршрута
direct (предпочтительно)
relay (fallback)
7.4 Старт стрима
video/audio
input channel
7.5 Monitoring
latency
packet loss
adaptive bitrate (опционально)
8. 🌍 Edge / Relay Layer
Функции:
UDP relay
NAT traversal помощь
latency probes
routing
Требования:
низкая задержка
геораспределение
stateless (по возможности)
9. 🧠 Matchmaking
Алгоритм:
Шаг 1: фильтрация
регион
доступность
цена
Шаг 2: latency test
RTT до:
клиента
edge
хоста
Шаг 3: scoring

Формула:

score = latency + jitter + packet_loss + price + reliability
10. 💰 Биллинг
Функции:
оплата по времени (минуты)
резерв средств (hold)
списание во время сессии
возврат при ошибках
Для host:
баланс
выплаты
комиссия платформы
11. 🧼 Session Isolation (ВАЖНО)
Требования:
отдельный Windows user
очистка профиля после сессии
запрет доступа к файлам хоста
whitelist приложений
12. 📊 Telemetry

Сбор данных:

FPS
latency
packet loss
decode pressure
session duration
crash logs
13. 📦 Backend (Control Plane)
Сервисы:
Auth Service
Device Service
Session Service
Billing Service
Matchmaking Service
Telemetry Service
Хранилища:
PostgreSQL
Redis
Object Storage
14. ⚙️ Deployment
Central:
backend API
база данных
billing
Edge:
relay nodes
probe servers
15. 🚀 MVP Scope (что делать сначала)
Включить:
Host Agent
базовый backend
авторизация
direct + relay подключение
unattended mode
manual connect
Не делать сразу:
сложный marketplace
multi-user
сложный биллинг
глобальную инфраструктуру
16. 📈 Будущие улучшения
marketplace
рейтинги хостов
multi-session
adaptive bitrate
AI-based routing
mobile network optimization
🧠 Итог

Это ТЗ описывает:

архитектуру уровня стартапа
реальный путь к MVP
масштабируемую систему
Удобные скрипты для разворачивания на сервере

17. 🛠️ Исполняемый roadmap по текущему репозиторию

Важно: в текущем репозитории уже есть рабочее ядро стриминга:

- Windows sender / receiver-native
- Android Sender
- Android PC Receiver
- video/audio/input/gamepad
- latency telemetry

Поэтому первая волна реализации должна идти не со стриминга, а с платформенного слоя вокруг него.

17.1 Первая волна (делать сейчас)

Цель: собрать Session MVP, который связывает уже существующий стриминг-стек с backend.

Включить:

- Control Plane backend в этом же репозитории
- Host registration
- Host heartbeat
- Device inventory / capabilities
- Session create / stop / inspect
- Telemetry ingest
- manual direct connect через backend-issued session

Технические решения для первой волны:

- Backend stack: ASP.NET Core
- Размещение: отдельный проект в этом же репозитории
- Хранилище на старте: in-memory/dev storage
- Сеть первой волны: direct/manual connect
- Relay / NAT traversal: следующая фаза

17.2 Вторая волна

- Интеграция Host Agent в receiver-native
- Регистрация ПК в backend
- Heartbeat каждые 5–10 секунд
- Получение session lease с backend
- Защищённый unattended mode только при активной session lease

17.3 Третья волна

- Android / PC client login
- Получение списка доступных host machines
- Session start из клиента через backend
- Connect по session token вместо ручного ввода IP

17.4 Отложить после MVP

Не делать в первой реализации:

- billing
- marketplace
- сложный matchmaking
- глобальные relay regions
- multi-user / multi-tenant cloud
- полноценную session isolation

17.5 Критерий успеха первой волны

Система считается собранной на первом этапе, если:

- Host может зарегистрироваться в backend
- backend видит host online/offline по heartbeat
- client/backend может создать session
- session возвращает stream endpoint и session token
- существующий sender/receiver стек можно запускать в рамках этой session

17.6 Текущее состояние реализации

Уже сделано в репозитории:

- `control-plane/` добавлен как отдельный ASP.NET Core backend
- реализованы endpoint-ы:
  - host register
  - host heartbeat
  - host list/details
  - session create/details/activate/stop
  - telemetry ingest
- `receiver-native` уже умеет работать как базовый Host Agent:
  - регистрация хоста в control plane
  - heartbeat
  - polling active lease для host
  - базовый telemetry push sender snapshot в backend
  - auto-start / auto-stop sender по lease при включённом `Lease auto-run`
  - показ статуса control plane, host registration и lease в HUD
- Android `PC Receiver` уже умеет backend-driven client flow:
  - загрузка host list из control plane
  - создание session из Android UI
  - передача receiver endpoint и desired stream profile в lease
  - `session activate` после локального старта receiver
  - чтение `connect instructions` для session
  - выбор `client region`
  - флаг `prefer relay`
  - stop session из Android UI
- `receiver-native` в роли `Receive` уже умеет backend-driven desktop client flow:
  - device-based login в control plane
  - загрузка host list из desktop UI
  - создание managed session из WinForms UI
  - передача desktop receiver endpoint в backend session
  - `session activate` после локального старта receiver
  - выбор `client region`
  - флаг `prefer relay`
  - stop backend session при выходе из managed desktop flow
- в `control-plane` уже есть минимальный auth / device binding:
  - `device-login`
  - bearer token для client-facing endpoint-ов
  - device ownership у session
- unattended доступ уже ограничен:
  - sender auto-run только для authenticated session lease
- session lease уже умеет нести:
  - receiver endpoint клиента
  - desired stream settings (`width/height/fps/bitrate`)
  - codec preference
- в `control-plane` уже есть session connect groundwork:
  - `GET /api/sessions/{sessionId}/connect`
  - route kinds:
    - `direct_host_push`
    - `direct_fallback`
    - `relay_assigned`
  - `relayEndpoint` / `relayRegion` в session route metadata
- в `control-plane` уже есть relay inventory groundwork:
  - `GET /api/relay`
  - `POST /api/relay/register`
  - `POST /api/relay/{relayId}/heartbeat`
- `receiver-native` sender HUD уже показывает lease route / lease relay
- добавлен `relay-node/` как минимальный рабочий UDP relay:
  - relay register / heartbeat в backend
  - `relay_register` handshake
  - форвардинг EVRT UDP packet-ов между sender и receiver
- `receiver-native` sender уже умеет реально использовать route `relay_assigned`
- `receiver-native` в роли `Receive` уже умеет реально идти через relay в managed session
- Android `PC Receiver` тоже уже умеет реально идти через relay в managed session
- auth слой уже расширен до device + refresh:
  - `POST /api/auth/refresh`
  - desktop client refresh path
  - Android client refresh path
- user auth поверх device MVP уже добавлен:
  - `POST /api/auth/users/register`
  - `POST /api/auth/users/login`
  - `POST /api/auth/users/refresh`
  - desktop / Android client user login flow
- NAT probing groundwork уже добавлен:
  - `probeEndpoint` / `probeToken` / `natStatus` в session metadata
  - `relay-node` отвечает на `nat_probe`
  - host agent и managed clients уже умеют публиковать probe result
- локальный dev URL control plane зафиксирован через launch profile:
  - `http://127.0.0.1:5180`

Следующий исполняемый шаг:

- direct UDP hole punching уже поднят и smoke-tested до `direct_punched`
- reconnect/resume managed session уже добавлен
- keepalive heartbeat уже добавлен
- managed UX polish уже начат
- следующий шаг: hardening around real-world transport loss/reconnect и затем финальный polish managed flow
- next focus: transport-loss hardening on real lossy links, then security / routing policy hardening and product polish

- session health surfacing already exists in the managed contract and HUD
- health-aware fallback policy is now active in the managed sync loop
- Android `receiver_feedback` is now published back into control-plane and feeds the health score
- session telemetry is now session-authenticated via `sessionToken`
- session telemetry is now also gated by live session state (`session_inactive` for stopped/expired sessions)
- control-plane telemetry retention is now bounded and prunes stale events
- next focus refined: real lossy-link transport hardening, then security / routing policy hardening and product polish

- routeVersion guards are now in the managed sync loop on both desktop and Android
- next focus refined: real lossy-link transport hardening, then security / routing policy hardening and product polish

- receiver health now includes a windowed `queueDropBurst` signal in addition to cumulative `queueDrops`
- next focus refined: real lossy-link transport hardening, then security / routing policy hardening and product polish

- receiver-side degraded streak is now real again and can trigger managed fallback
- next focus refined: real lossy-link transport hardening, then security / routing policy hardening and product polish

- backend route fallback now has an explicit cooldown contract and both desktop/Android managed clients respect it
- next focus refined: lossy-link transport hardening under real packet loss and reconnect jitter, then security / routing policy hardening and product polish

- control-plane now emits backend-driven `routeActionHint` / `routeActionReason` in managed session responses
- desktop and Android managed flows now use that shared policy signal instead of relying only on local degraded streak heuristics
- control-plane now also emits backend-driven `recommendedSyncDelaySeconds` so managed polling cadence is part of the shared session contract
- control-plane now also exposes transport-loss severity and receiver/sender telemetry freshness in the managed contract
- desktop and Android now surface that shared loss/staleness contract directly in managed UX
- managed routes now also support backend-driven recovery from fallback/degraded back to direct through a shared `route/recover` policy
- route hysteresis is now explicit in both directions: fallback cooldown and recovery cooldown are both part of the managed contract
- route control is now policy-guarded and every managed fallback/recovery writes an audit trail (`kind/reason/actor/time`) into session state
- route actions are now also short-rate-limited in backend and session telemetry ingest is restricted to an allowlist with bounded payload sanitization
- managed session contract now also carries NAT probe freshness/age, and backend direct recovery no longer trusts stale `same_public_ip` observations
- control-plane now also enforces one live session per actor and uses load-aware relay selection instead of naive region-only picks
- control-plane now also rate-limits rapid session-create churn per actor and coalesces hot-path session telemetry samples instead of appending all of them
- route decisions are now also time-hysteresis based through backend fallback/recovery readiness windows, not only momentary health states
- route decisions now also have a dedicated transport anomaly contract:
  - `TransportAnomalyKind`
  - `TransportAnomalyReason`
  - `TransportAnomalyConfidence`
- control-plane now distinguishes stale telemetry, queue-drop bursts, receiver pressure, decode/present jitter, low decode FPS, and high video/input tail estimates
- desktop and Android managed clients now parse and surface that anomaly contract
- route policy now uses anomaly-specific signals directly:
  - direct recovery is blocked while actionable anomaly is still active
  - high-confidence anomalies shorten fallback warm-up and managed sync cadence
  - medium-confidence anomalies keep the safer warm-up but still mark the session degraded
- route policy now also has a safe read-only diagnostics endpoint:
  - `GET /api/sessions/{sessionId}/route/policy?sessionToken=...`
  - exposes action hint/reason, anomaly kind/reason/confidence, warm-up windows, cooldowns, telemetry ages, and NAT probe freshness
- desktop and Android control-plane clients now have typed methods for the route policy diagnostics endpoint without adding extra polling to active managed sync
- route policy contract now has a local smoke script:
  - `control-plane/smoke-route-policy.ps1`
  - verifies device login, host register, managed session create, route-policy read, required fields, and cleanup stop
- current platform-core phase is effectively complete; next focus should be a new product phase rather than more session plumbing:
  - production persistence
  - deployment / environments
  - managed UX polish
  - security hardening
  - billing / marketplace / multi-host operations
- product phase has started with control-plane file-backed persistence:
  - startup snapshot load
  - atomic snapshot save after successful mutating API requests
  - default `%LOCALAPPDATA%\Everty\ControlPlane\state.json`
  - `EVERTY_CONTROL_PLANE_STATE_PATH` override
  - `control-plane/smoke-persistence.ps1` restart smoke
- control-plane now also has deployment readiness checks:
  - `GET /api/ready`
  - verifies persistence path writability
  - returns `503` with diagnostics when persistence is unavailable
  - `control-plane/smoke-ready.ps1` verifies readiness under a temporary state path
- deployment profile has started:
  - `.dockerignore`
  - `control-plane/Dockerfile`
  - `relay-node/Dockerfile`
  - repository-level `docker-compose.yml`
  - compose healthcheck through `/api/ready`
  - relay-node env fallback through `EVERTY_RELAY_*`
- service packaging has started:
  - `scripts/publish-platform.ps1`
  - `scripts/run-control-plane.ps1`
  - `scripts/run-relay-node.ps1`
  - `scripts/smoke-published-platform.ps1`
  - `scripts/start-platform-local.ps1`
  - `scripts/stop-platform-local.ps1`
  - `docs/deployment.md`
  - local publish output ignored through `artifacts/`
- release configuration has started:
  - `Directory.Build.props`
  - `artifacts/platform/publish-manifest.json` generation during publish
  - `deploy/control-plane.env.example`
  - `deploy/relay-node.env.example`
- security hardening has started:
  - configurable control-plane access/refresh token TTL
  - configurable request body cap
  - default security headers for API responses
  - `GET /api/config/runtime`
  - `control-plane/smoke-security.ps1`
- Docker Compose and control-plane Dockerfile now carry the same security/runtime defaults
- authenticated operator controls have started:
  - `EVERTY_CONTROL_PLANE_OPERATOR_KEY`
  - `GET /api/admin/summary`
  - `GET /api/admin/sessions`
  - `POST /api/admin/hosts/{hostId}/availability`
  - `POST /api/admin/relays/{relayId}/availability`
  - `POST /api/admin/sessions/{sessionId}/stop`
  - `control-plane/smoke-admin.ps1`
- operator action smoke now covers session stop, host disable, and relay disable
- operator CLI convenience has started:
  - `scripts/control-plane-admin.ps1`
  - `scripts/smoke-control-plane-admin-cli.ps1`
- operator UI convenience has started:
  - `GET /admin`
  - `control-plane/smoke-admin-dashboard.ps1`
- first marketplace skeleton has started:
  - persisted host marketplace offers
  - `POST /api/admin/hosts/{hostId}/offer`
  - authenticated `GET /api/marketplace/hosts`
  - operator dashboard offer form
  - operator CLI `set-offer`
  - admin/CLI/dashboard smoke coverage
- billing ledger/payment-hold skeleton has started:
  - persisted billing accounts and session records
  - automatic hold creation on session create
  - capture/finalize on stop
  - settlement endpoint
  - billing summary and session readout
  - operator dashboard billing controls
  - session hourly rate snapshot
  - operator billing accounts and ledger inspection
- managed UX now uses marketplace host listing first:
  - desktop receiver host dropdown shows offer price when available
  - Android receiver host list shows offer price when available
  - both clients fall back to legacy host listing for older control-plane builds
- billing provider integration prep has started:
  - `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER`
  - provider-aware billing session metadata
  - provider hold/capture/settlement ids
  - operator `billing-provider` command
  - deployment defaults for manual provider mode
- payment provider adapter boundary has started:
  - `IPaymentProvider`
  - manual provider adapter for hold/capture/settle operations
  - runtime config exposes payment provider and provider mode
  - billing lifecycle now goes through the adapter instead of direct provider id generation
- external payment provider integration has started:
  - `external_stub` mode for non-manual provider names without endpoint
  - `external_http` mode when `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT` is configured
  - provider HTTP callback contract for hold/capture/settle
  - runtime/admin diagnostics for endpoint/external-call state
  - `control-plane/smoke-payment-provider.ps1`
- payment provider failure/audit hardening has started:
  - hold failure blocks session creation before the host is reserved
  - capture/settle failure is recorded into billing session state
  - billing details now expose last payment error and payment attempt timestamp
  - provider smoke covers forced capture failure
- payout/settlement reconciliation tooling has started:
  - `GET /api/admin/billing/reconciliation`
  - `POST /api/admin/billing/sessions/{sessionId}/retry`
  - CLI commands `billing-reconciliation` and `retry-billing`
  - provider smoke covers one-shot capture failure and retry recovery
- billing reconciliation UI surfacing has started:
  - `/admin` dashboard shows billing reconciliation actions
  - dashboard can retry failed capture/settle actions
  - dashboard smoke checks reconciliation/retry markers
- release readiness sweep has started:
  - `scripts/smoke-product-readiness.ps1`
  - aggregates platform builds, compose config, readiness, persistence, security, admin, dashboard, CLI, and route policy smokes
  - payment provider smokes remain covered separately and can be included when needed
- final packaging/release readiness has started:
  - `scripts/publish-platform.ps1 -Version 0.1.0-product -Channel local-product`
  - `scripts/smoke-published-platform.ps1`
  - `docs/release-readiness.md`
- final repo hygiene / commit-ready sweep has started:
  - `deploy/docker-compose.env.example`
  - `scripts/audit-release-hygiene.ps1`
  - `scripts/audit-commit-scope.ps1`
  - `docker compose --env-file deploy/docker-compose.env.example config`
  - `scripts/smoke-product-readiness.ps1` now includes release hygiene audit
- release candidate handoff has started:
  - `docs/release-candidate-handoff.md`
  - validation order for product readiness, publish, and payment-provider retry smoke
  - commit grouping guidance for the dirty worktree
- simple local onboarding has started:
  - local/dev demo users `admin/admin` and `test/test`
  - simple desktop/Android pairing path through `GET /api/hosts`
  - Russian quick-login onboarding and advanced-mode split in desktop/Android clients
  - local runbook now documents the no-marketplace first-pair flow
- current product phase progress: approximately `100%`
- next product-phase focus: post-RC packaging/distribution work outside the current product-phase scope
