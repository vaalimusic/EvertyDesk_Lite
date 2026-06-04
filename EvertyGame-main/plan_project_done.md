# Выполненные шаги по плану

## 2026-04-10

### 1. Первая backend-волна вокруг существующего стриминга

Сделано:

- добавлен отдельный backend-проект `control-plane/` на ASP.NET Core
- добавлен launch profile для локального dev URL:
  - `http://127.0.0.1:5180`
- реализованы endpoint-ы:
  - `GET /api/health`
  - `POST /api/hosts/register`
  - `POST /api/hosts/{hostId}/heartbeat`
  - `GET /api/hosts`
  - `GET /api/hosts/{hostId}`
  - `POST /api/sessions`
  - `GET /api/sessions/{sessionId}`
  - `POST /api/sessions/{sessionId}/activate`
  - `POST /api/sessions/{sessionId}/stop`
  - `POST /api/telemetry/session`

### 2. Session-aware host layer

Сделано:

- добавлен host-facing endpoint:
  - `GET /api/hosts/{hostId}/lease?hostSecret=...`
- backend теперь чистит истёкшие lease/session и освобождает host
- `receiver-native` получил встроенный `ControlPlaneAgent`
- host agent умеет:
  - регистрироваться
  - слать heartbeat
  - polling active lease
  - слать базовый sender telemetry snapshot в backend

### 2.1 Backend-managed sender lifecycle

Сделано:

- session теперь может нести:
  - receiver endpoint клиента
  - desired stream settings:
    - `width`
    - `height`
    - `fps`
    - `bitrate`
  - `codec preference`
- host lease возвращает эти параметры хосту
- `receiver-native` теперь умеет:
  - автостарт sender по lease
  - автостоп sender при пропаже/смене lease
  - использовать текущий UI profile как baseline и применять lease overrides

Проверено runtime smoke test-ом:

- host register
- session create с `receiverAddress/receiverPort`
- lease fetch с возвратом `desired stream settings`

### 3. Desktop UI / HUD интеграция

Сделано:

- в `receiver-native` добавлено поле `Control`
- в sender HUD добавлены:
  - `Control plane`
  - `Host registration`
  - `Lease status`
  - `Lease session`
  - `Lease client`
  - `Lease receiver`
  - `Lease codec`
  - `Lease profile`
  - `Lease expires`

### 4. Android backend-driven client flow

Сделано:

- в Android `PC Receiver` добавлен client для `control-plane`
- приложение теперь умеет:
  - загрузить список host-ов из backend
  - выбрать host
  - создать session прямо из Android UI
  - передать в session:
    - receiver endpoint Android-устройства
    - desired stream profile
    - codec preference
  - остановить backend session из Android UI
- старый manual `Listen` path оставлен рабочим как fallback

### 4.1 Auth / device binding / unattended gating

Сделано:

- в `control-plane` добавлены:
  - `POST /api/auth/device-login`
  - `GET /api/auth/me`
- client-facing backend endpoint-ы теперь защищены bearer token-ом
- session теперь привязана к устройству клиента (`createdByDeviceId`)
- Android `PC Receiver` автоматически делает device login и использует bearer token для:
  - host list
  - session create
  - session stop
- unattended sender auto-run на host разрешён только для authenticated session lease

### 4.2 Desktop backend-driven client flow + session activation

Сделано:

- `receiver-native` в роли `Receive` теперь умеет backend-driven client flow:
  - device-based login в `control-plane`
  - загрузка host list из desktop UI
  - создание managed session прямо из WinForms UI
  - авто-передача desktop receiver endpoint в session
  - stop backend session из desktop UI
  - stop backend session при уходе из роли `Receive`
- desktop client теперь делает `session activate` после того, как локальный receiver уже слушает порт
- Android `PC Receiver` тоже теперь делает `session activate` после локального старта receiver
- в `control-plane` добавлен endpoint:
  - `GET /api/sessions/{sessionId}/connect`
- client-ы теперь умеют читать `connect instructions` для session и уже получают route kind (`direct_host_push`) как задел под relay/NAT phase

### 4.3 Relay / NAT groundwork

Сделано:

- в `control-plane` добавлены relay node endpoint-ы:
  - `GET /api/relay`
  - `POST /api/relay/register`
  - `POST /api/relay/{relayId}/heartbeat`
- backend теперь умеет хранить relay inventory и выбирать route plan для session:
  - `direct_host_push`
  - `direct_fallback`
  - `relay_assigned`
- session / lease / connect instructions теперь несут:
  - `routeKind`
  - `relayEndpoint`
  - `relayRegion`
- `receiver-native` host HUD теперь показывает lease route и relay endpoint
- `receiver-native` в роли `Receive` теперь умеет:
  - задавать `client region`
  - задавать `prefer relay`
  - видеть route в managed desktop flow
- Android `PC Receiver` managed flow теперь тоже умеет:
  - задавать `client region`
  - задавать `prefer relay`
  - видеть route / relay path в backend status

Важно:

- это ещё не реальный UDP relay transport
- это routing groundwork в control plane, чтобы следующий шаг уже строился на готовой session route model, а не на голом direct-only path

### 4.4 Real relay transport

Сделано:

- добавлен отдельный `relay-node/` как минимальный UDP EVRT relay
- relay node теперь:
  - регистрируется в `control-plane`
  - шлёт heartbeat
  - принимает `relay_register`
  - форвардит EVRT UDP пакеты между sender и receiver в рамках `sessionId`
- в `receiver-native` sender добавлен relay-aware transport path:
  - sender может стартовать по lease в route `relay_assigned`
  - sender регистрируется в relay как `sender`
- в `receiver-native` desktop receiver добавлен relay-aware managed receive path:
  - desktop receiver читает relay route из `connect instructions`
  - desktop receiver регистрируется в relay как `receiver`
  - control/feedback channel идёт через relay до прихода первого stream packet
- Android `PC Receiver` тоже получил relay-aware transport path:
  - managed session теперь настраивает relay route в controller
  - Android receiver регистрируется в relay и отправляет control/feedback через relay

Проверено runtime smoke test-ом:

- `control-plane` поднят локально
- `relay-node` зарегистрировался в backend
- backend выдал `relay_assigned`
- sender-side UDP packet дошёл до receiver-side через relay
- receiver-side control packet дошёл обратно до sender-side через relay

### 4.5 Refresh token auth slice

Сделано:

- в `control-plane` добавлен:
  - `POST /api/auth/refresh`
- backend теперь хранит:
  - access token
  - refresh token
- refresh token ротируется при refresh
- `receiver-native` desktop client теперь:
  - сохраняет refresh token
  - пытается получить новый access token через refresh
  - делает повторный `device-login` только как fallback
- Android `PC Receiver` client тоже теперь:
  - сохраняет refresh token
  - использует refresh path вместо постоянного нового `device-login`

### 4.6 User auth slice

Сделано:

- в `control-plane` добавлены user-facing endpoint-ы:
  - `POST /api/auth/users/register`
  - `POST /api/auth/users/login`
  - `POST /api/auth/users/refresh`
  - `GET /api/auth/users/me`
- backend теперь умеет:
  - хранить users
  - выдавать отдельные user access/refresh tokens
  - авторизовывать client routes как по device token, так и по user token
- session ownership теперь может быть не только device-based, но и user-based
- `receiver-native` desktop client теперь умеет:
  - логин / регистрацию user-а в `control-plane`
  - сохранять user refresh token
  - предпочитать user refresh path, а не device fallback
  - показывать `Control auth` в HUD
- Android `PC Receiver` теперь тоже умеет:
  - логин / регистрацию user-а в `control-plane`
  - сохранять user refresh token
  - использовать user auth path перед device fallback

### 4.7 NAT probing groundwork

Сделано:

- session / lease / connect instructions теперь несут:
  - `probeEndpoint`
  - `probeToken`
  - `natStatus`
- `relay-node` теперь умеет `nat_probe` / `nat_probe_ack`
  - и возвращает sender/receiver их observed public UDP endpoint
- `receiver-native` host agent теперь:
  - автоматически делает NAT probe по active lease
  - публикует host probe result обратно в backend
  - показывает `Lease probe` / `Lease NAT` в sender HUD
- `receiver-native` desktop receiver managed flow теперь:
  - делает client-side NAT probe после `session create`
  - публикует probe result в backend
- Android `PC Receiver` managed flow теперь:
  - делает client-side NAT probe после `session create`
  - публикует probe result в backend
- backend теперь считает `natStatus` по session:
  - `probe_unavailable`
  - `probe_pending`
  - `same_public_ip`
  - `punch_candidate`

### 5. Проверка

Проверено:

- `dotnet build control-plane/Everty.ControlPlane.csproj`
- `dotnet build receiver-native/ReceiverNative.csproj --no-restore`
- `dotnet build relay-node/Everty.RelayNode.csproj`
- `./gradlew.bat --no-daemon :app:assembleDebug --console=plain`
- runtime smoke test:
  - device login
  - refresh token exchange
  - bearer-auth host list
  - host register
  - session create
  - session activate
  - session connect instructions
  - host lease fetch
  - telemetry ingest
  - session create с `receiver endpoint + desired stream settings`
  - unattended authorization in host lease
  - relay registration
  - relay-aware route assignment (`relay_assigned`) в session connect instructions и host lease
  - real UDP forwarding through relay in both directions:
    - sender -> receiver video packet
    - receiver -> sender control packet
  - user register / user refresh
  - user bearer-auth session create
  - NAT probe through relay node
  - `natStatus = same_public_ip` after host + client probe publish

### 6. Что ещё не сделано

Следующий блок:

- дальше:
  - реальный direct UDP hole punching поверх уже рабочего NAT probe groundwork
  - session route upgrade с fallback `direct -> relay`
  - desktop/Android polish поверх user-auth + managed sessions

### 7. Оценка прогресса

- streaming core: примерно `84%`
- Session MVP из плана: примерно `96%`
- весь большой план целиком: примерно `58%`

### 4.8 Direct UDP hole punching

Done:

- backend session route now upgrades from relay/direct fallback to `direct_punched` after both host and client NAT probes are published
- `relay-node` now acts as a working NAT probe echo endpoint and returns the observed public UDP endpoint
- sender/receiver clients publish NAT probe results back to control plane
- runtime smoke test verified the end state:
  - relay registered and online
  - host probe published
  - client probe published
  - connect instructions returned `routeKind = direct_punched`
  - `streamEndpoint.transport = udp-evrt-direct-punch`
  - `receiverEndpoint.transport = udp-evrt-direct-punch`
  - observed endpoints were stored in session NAT state

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `98%`
- весь большой план целиком: примерно `61%`

### 4.9 Managed session reconnect / resume

Done:

- desktop managed client now persists the active managed session and can resume it from saved state after restart
- desktop sender/receiver UI now exposes `Resume Managed`
- Android managed client now persists the active managed session and can resume it from saved state after restart
- Android managed flow now restores:
  - session id/token
  - host selection
  - route kind
  - NAT status
  - relay endpoint metadata
- on resume, both clients reuse `session activate` + `connect instructions` instead of forcing a new session create
- on stop / failure, managed session state is cleared from persisted storage

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `99%`
- весь большой план целиком: примерно `64%`

### 4.10 Managed session sync watchdog

Done:

- desktop managed flow now periodically syncs `connect instructions` while a managed session is active
- desktop sync refreshes:
  - route kind
  - NAT status
  - relay metadata
- desktop sync clears managed state if backend marks the session `Stopped` or `Expired`
- Android managed flow now periodically syncs `connect instructions` while a managed session is active
- Android sync refreshes:
  - route kind
  - NAT status
  - relay route configuration
  - persisted managed session state
- both clients now have a small resilience layer on top of `direct_punched` / `relay_assigned` instead of only one-shot startup logic

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `99%`
- весь большой план целиком: примерно `66%`

### 4.11 Session keepalive heartbeat

Done:

- backend now exposes `POST /api/sessions/{sessionId}/keepalive`
- keepalive extends the session lease expiry for active managed sessions
- desktop managed client now sends keepalive during periodic sync
- Android managed client now sends keepalive during periodic sync
- runtime smoke test verified that keepalive extends `ExpiresUtc`

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `68%`

### 4.12 Managed UX polish

Done:

- desktop control-plane auth now auto-refreshes host list after successful user login/register
- Android control-plane auth now auto-refreshes host list after successful user login/register
- desktop managed flow now feels less manual because auth immediately feeds the session picker
- Android managed flow now keeps the host picker in sync after auth without a separate reload step

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `70%`
### 4.13 Managed route fallback hardening

Done:

- control-plane now exposes an explicit managed session route fallback endpoint:
  - `POST /api/sessions/{sessionId}/route/fallback`
- the backend fallback path can downgrade an unstable managed session from stale `direct_punched` to:
  - `relay_assigned` when a relay is available
  - `direct_fallback` when no relay is online
- desktop managed sync now tracks consecutive keepalive/connect failures and triggers route fallback instead of waiting forever on a stale direct route
- Android managed sync now does the same fallback escalation on repeated sync failures
- both managed flows reset their retry streak on successful sync again
- runtime smoke test verified fallback promotion to `relay_assigned`

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `72%`

### 4.14 Managed reconnect backoff and status surfacing

Done:

- desktop managed sync now uses an adaptive backoff instead of a flat retry interval after failures
- Android managed sync now does the same adaptive backoff on repeated failures
- both managed UIs now surface:
  - current sync failure streak
  - next retry delay
- successful sync resets the backoff to the normal interval
- fallback route promotion still works after the failure streak reaches the threshold

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `74%`

### 4.15 Managed route state surfacing

Done:

- control-plane session responses now expose a computed `routeState` alongside `routeKind`
- `routeState` is computed from the current session lifecycle and route type:
  - `healthy`
  - `fallback`
  - `degraded`
  - `inactive`
  - `syncing`
- desktop managed flow now persists and displays `routeState` in status/HUD
- Android managed flow now persists and displays `routeState` in status text
- smoke test verified:
  - `connect.routeState = healthy`
  - `route/fallback.routeState = fallback`

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `75%`

### 4.16 Transport-loss status surfacing

Done:

- managed session state now keeps explicit route health metadata in the backend session contract
- desktop managed HUD now shows:
  - `routeKind`
  - `routeState`
  - sync failure streak
  - next retry delay
- Android managed status now shows the same route state / backoff information
- both clients now surface transport degradation and recovery in a way the user can actually read
- all builds passed after the contract update

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `76%`

### 4.17 Session health surfacing

Done:

- control-plane session contract now exposes computed session health metadata:
  - `healthy`
  - `degraded`
  - `syncing`
  - `inactive`
- health is derived from:
  - route state
  - host online status
  - telemetry freshness
  - receiver decode / encode pressure
  - pulse / input estimates
- desktop managed HUD now shows session health and the reason string
- Android managed status now shows the same session health and reason string
- clients now parse and persist the health fields in the managed session contract
- control-plane, desktop sender and Android receiver all build cleanly after the contract change

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `77%`

### 4.18 Health-aware fallback policy

Done:

- managed session sync now watches `sessionHealth` in addition to raw sync failures
- if health stays degraded for multiple syncs, desktop and Android can proactively request route fallback
- degraded health now causes a visible streak in the managed UI, so transport loss is not silent
- fallback state still persists through the managed session contract and HUD
- control-plane, desktop sender and Android receiver all still build cleanly after the policy update

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `78%`

### 4.19 Session-authenticated telemetry

Done:

- control-plane telemetry ingest now requires a valid `sessionToken` when telemetry is tied to a session
- Android `receiver_feedback` now includes `sessionToken`
- Windows host-agent `sender_snapshot` telemetry now also includes `sessionToken`
- backend session health continues to prefer fresh `receiver_feedback`, but only from authenticated sessions
- this closes the obvious gap where session telemetry could be posted without proof of session ownership

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `80%`

### 4.21 Control-plane telemetry retention

Done:

- control-plane now prunes stale telemetry older than 14 days
- telemetry list is capped to a bounded size so backend memory stays predictable
- this is a small but real durability hardening step for long-running managed sessions

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `81%`

### 4.19 Receiver feedback telemetry ingestion

Done:

- Android `PC Receiver` now publishes live `receiver_feedback` into `control-plane`
- backend session health now prefers `receiver_feedback` over stale sender snapshot data
- managed route state and fallback policy still remain visible in the UI
- the managed contract continues to build cleanly after the telemetry wiring

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `79%`

### 4.20 Session telemetry gated by active session

Done:

- control-plane now rejects session telemetry for stopped/expired sessions with `session_inactive`
- the telemetry ingest path is now bound not only to `sessionToken` but also to session liveness
- this reduces stale telemetry noise after disconnects and keeps health signals tied to live sessions

Updated progress after this slice:

- streaming core: примерно `84%`
- Session MVP из плана: примерно `100%`
- весь большой план целиком: примерно `80%`

### 4.22 Managed route version hardening

Done:

- managed session route responses now carry a monotonic `routeVersion`
- desktop managed sync ignores stale connect/fallback responses that are older than the current route version
- Android managed sync does the same and also persists route version in managed session state
- persisted managed session resume now restores the latest route version baseline before re-activating a session
- this closes the last obvious stale-route overwrite path in the managed session loop

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `82%`

### 4.23 Receiver drop burst health signal

Done:

- Android receiver telemetry now includes a windowed `queueDropBurst` signal in addition to the cumulative `queueDrops`
- control-plane session health now treats repeated drop bursts as a degraded transport signal
- this gives the managed fallback loop an earlier and less noisy indication of lossy links
- the new signal is still session-authenticated and only accepted for live sessions

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `83%`

### 4.24 Degraded streak reconnect hardening

Done:

- Android managed sync now keeps a real `sessionHealthDegradedStreak` instead of zeroing it immediately after increment
- health-based fallback can now actually trigger after repeated degraded syncs on the receiver side
- this makes the managed reconnect policy respond to persistent transport loss instead of only hard failures
- the same path still stays routeVersion-safe and session-authenticated

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `84%`

### 4.25 Backend fallback cooldown and client cooldown awareness

Done:

- control-plane now applies a backend-side cooldown after route fallback, so repeated fallback requests do not thrash the session route on bad links
- session responses now expose `routeFallbackCount` and `routeFallbackCooldownSeconds`
- desktop managed flow now respects fallback cooldown, shows it in status/HUD, and stops hammering backend fallback while cooldown is active
- Android managed flow now persists the same cooldown state and respects it before issuing health-based or failure-based fallback
- this turns fallback policy into a session-level contract instead of only a client-side heuristic

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `85%`

### 4.26 Backend-driven route action hints

Done:

- control-plane session responses now include `routeActionHint` and `routeActionReason`
- backend computes action hints from session health and fallback cooldown instead of leaving fallback policy fully to local client heuristics
- desktop managed flow now persists and shows route action state in HUD/status and can trigger fallback from backend guidance
- Android managed flow now does the same and respects backend action hints before relying on local degraded streak alone
- this moves reconnect and fallback policy closer to a shared control-plane contract

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `86%`

### 4.27 Backend-driven managed sync cadence

Done:

- control-plane session responses now include `recommendedSyncDelaySeconds`
- backend computes managed sync cadence from route action, fallback cooldown, and current session health instead of leaving polling intervals fully local
- desktop managed flow now persists the recommended sync cadence, shows it in HUD/status, and uses it for its next sync interval
- Android managed flow now does the same and follows backend cadence on active, resumed, synced, and fallback states
- this removes another piece of split reconnect policy between desktop and Android clients

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `87%`

### 4.28 Telemetry freshness and transport-loss surfacing

Done:

- control-plane managed session responses now include `transportLossLevel`, `receiverTelemetryAgeSeconds`, and `senderTelemetryAgeSeconds`
- backend now classifies loss severity from live receiver telemetry instead of exposing only aggregate health
- desktop managed HUD/status now shows loss level and telemetry age from the shared backend contract
- Android managed flow now persists and displays the same transport-loss / telemetry-age state
- this gives both clients a common view of whether the problem is active packet loss, severe pressure, or stale telemetry

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `88%`

### 4.29 Managed transport-loss severity contract

Done:

- control-plane now classifies managed transport state into a shared `transportLossLevel`
- managed session responses now also carry `receiverTelemetryAgeSeconds` and `senderTelemetryAgeSeconds`
- desktop managed HUD/status now surfaces loss severity and telemetry freshness from the backend contract
- Android managed flow persists and displays the same fields during active, resumed, synced, and fallback states
- this closes another gap where clients previously had to infer whether the issue was active loss or stale telemetry

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `89%`

### 4.30 Route recovery hysteresis and backend-driven direct recovery

Done:

- control-plane now exposes a dedicated `route/recover` endpoint for managed sessions
- session state now tracks `RouteRecoveryCount` and `RouteRecoveryCooldownSeconds`
- backend `routeActionHint` can now emit `direct_recovery_recommended` when fallback/degraded routes become healthy enough for direct recovery
- desktop managed flow now calls route recovery from backend guidance and persists the updated recovery counters/cooldown
- Android managed flow now does the same and uses the shared backend recovery policy instead of local-only heuristics
- this adds hysteresis in both directions: controlled fallback and controlled recovery

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `90%`

### 4.31 Managed recovery cooldown and bidirectional route hysteresis

Done:

- control-plane route policy is now bidirectional: it can explicitly recommend fallback and explicit direct recovery
- session state now tracks `RouteRecoveryCount` and `RouteRecoveryCooldownSeconds`
- desktop managed flow now follows backend `direct_recovery_recommended` guidance and executes `route/recover`
- Android managed flow now does the same through the same backend contract
- managed HUD/status now shows recovery counters/cooldown alongside fallback counters/cooldown
- this gives the route layer real hysteresis in both directions instead of only one-way degradation

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `91%`

### 4.32 Route policy guardrails and route-action audit trail

Done:

- control-plane route endpoints now enforce policy guardrails and return `route_policy_blocked` when fallback/recovery is not actually recommended by current session state
- managed route actions now write an explicit audit trail into session state:
  - `LastRouteActionKind`
  - `LastRouteActionReason`
  - `LastRouteActionActor`
  - `LastRouteActionUtc`
- desktop managed flow now persists and surfaces that last route action in HUD/state
- Android managed flow now persists the same audit fields and includes them in backend status text
- this hardens route control and makes it visible who changed route policy and why

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `92%`

### 4.33 Route-action rate limiting and telemetry allowlist hardening

Done:

- control-plane route actions now have a short backend-side rate limit so fallback/recover cannot thrash continuously under noisy clients or unstable sync loops
- `route/fallback` and `route/recover` now return `route_action_rate_limited` during that short suppression window
- session telemetry ingest now accepts only the current approved session event/source pairs:
  - `receiver_feedback` from `android_pc_receiver`
  - `sender_snapshot` from `receiver-native-host-agent`
- session telemetry payload is now sanitized and bounded:
  - capped key count
  - capped key length
  - capped string value length
  - non-primitive values flattened safely
- this hardens the control plane against route spam and unbounded / unexpected session telemetry input

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `93%`

### 4.34 NAT probe freshness contract and direct-recovery hardening

Done:

- control-plane managed session contract now exposes:
  - `HostNatProbeAgeSeconds`
  - `ClientNatProbeAgeSeconds`
  - `NatProbeFresh`
- backend direct recovery now requires not only `same_public_ip`, but also fresh NAT probes on both sides
- backend `routeActionHint` now explains when direct recovery is blocked by stale NAT probe data instead of silently acting as if recovery were unavailable
- desktop managed HUD/status now shows NAT probe age/freshness
- Android managed flow now persists and shows the same NAT probe freshness contract
- this hardens `direct_recovery_recommended` against stale hole-punch observations on unstable WAN paths

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `94%`

### 4.35 Actor session quota and relay load-aware routing policy

Done:

- control-plane now prevents one actor from silently creating multiple parallel active sessions at once
- session create now returns `actor_session_exists` if the same authenticated device/user already owns another live session
- relay selection is now load-aware instead of region-only:
  - prefers same client region
  - then host region
  - then lower current assigned session count
  - avoids saturated relays when alternatives exist
- probe relay and fallback relay selection now use the same shared relay policy helper
- relay inventory now exposes `AssignedSessionCount` and `Saturated`, so relay policy is no longer invisible
- this hardens the control plane against session churn and reduces naive relay hot-spotting

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `95%`

### 4.36 Session-create anti-churn and telemetry coalescing

Done:

- control-plane now rate-limits repeated `session create` attempts for the same authenticated actor over a short cooldown window
- session create now returns `session_create_rate_limited` instead of allowing rapid churn loops
- session telemetry ingest now coalesces hot-path samples for:
  - `receiver_feedback`
  - `sender_snapshot`
- within the short coalescing window, the latest sample replaces the previous one instead of endlessly growing telemetry history
- this reduces backend churn from noisy managed loops and keeps the telemetry list more representative of current state

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `96%`

### 4.37 Time-based route hysteresis windows

Done:

- control-plane now tracks route readiness over time instead of reacting only to momentary state
- managed session contract now exposes:
  - `RouteFallbackReadyDurationSeconds`
  - `RouteRecoveryReadyDurationSeconds`
- backend fallback recommendation now requires sustained degraded health for a warm-up window before emitting `fallback_recommended`
- backend direct recovery now requires sustained healthy transport / fresh NAT evidence for a warm-up window before emitting `direct_recovery_recommended`
- desktop managed HUD now shows these readiness windows directly
- Android managed flow now persists and displays the same readiness durations in backend status
- this makes route transitions less twitchy on noisy WAN links with short spikes and brief recoveries

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `97%`

### 4.38 Transport anomaly classification contract

Done:

- control-plane now classifies the dominant transport anomaly from live receiver/sender telemetry instead of exposing only a broad `transportLossLevel`
- managed session responses now include:
  - `TransportAnomalyKind`
  - `TransportAnomalyReason`
  - `TransportAnomalyConfidence`
- backend anomaly classification currently distinguishes:
  - awaiting / stale telemetry
  - critical or high receiver pressure
  - queue-drop bursts
  - present/decode jitter
  - low receiver decode FPS
  - high video/input tail estimates
  - nominal transport
- desktop managed client now parses, persists, and shows the transport anomaly in the sender HUD
- Android managed client now parses and keeps the same anomaly contract in active managed session state
- this makes the WAN hardening layer more actionable: the route policy can now reason about the likely cause of degradation instead of only seeing `nominal/elevated/severe/stale`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `98%`

### 4.39 Anomaly-aware route policy

Done:

- control-plane now uses `TransportAnomalyKind/Reason/Confidence` as an input to managed route decisions, not only as diagnostics
- direct route recovery is now blocked while an actionable anomaly is still active, even when broad `transportLossLevel` is nominal
- high-confidence anomalies now shorten fallback warm-up from `8s` to `5s`
- medium-confidence actionable anomalies keep the safer `8s` warm-up
- managed sync cadence now tightens to:
  - `5s` for high-confidence actionable anomalies
  - `8s` for medium-confidence actionable anomalies
- session health now degrades on actionable anomaly signals such as receiver pressure, queue-drop bursts, decode/present jitter, low decode FPS, and high video/input tail
- this makes fallback/recovery behavior less generic and more aligned with the real WAN failure mode reported by receiver telemetry

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `99%`

### 4.40 Route policy diagnostics endpoint

Done:

- control-plane now exposes a read-only route policy diagnostic endpoint:
  - `GET /api/sessions/{sessionId}/route/policy?sessionToken=...`
- the endpoint is session-authenticated through the same `TryAuthorizeSessionAction` path as connect/fallback/recover
- the response returns the active managed route policy snapshot:
  - route kind/state/version
  - session health/reason
  - route action hint/reason
  - recommended sync cadence
  - transport loss level
  - transport anomaly kind/reason/confidence
  - actionable/high-confidence anomaly flags
  - fallback/recovery warm-up and ready durations
  - fallback/recovery cooldowns
  - receiver/sender telemetry ages
  - NAT probe freshness and ages
- this gives us a safe smoke-test surface for final WAN policy behavior without mutating route state or forcing fallback/recovery transitions
- control-plane README now documents the route policy diagnostics surface
- runtime smoke verified:
  - `/api/health` starts cleanly
  - `/api/sessions/nope/route/policy` is wired and returns `404` for missing session instead of failing route binding/startup

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `99.5%`

### 4.41 Client-side route policy diagnostics access

Done:

- desktop control-plane client now has a typed route policy diagnostics model:
  - `DesktopControlPlaneRoutePolicy`
  - `GetRoutePolicyAsync(...)`
- Android control-plane client now has the matching typed route policy diagnostics model:
  - `ControlPlaneRoutePolicy`
  - `getRoutePolicy(...)`
- both client layers parse the read-only policy response without reusing connect-instructions parsing, because policy diagnostics intentionally does not include stream endpoint fields
- this makes the backend route diagnostics endpoint available to managed clients/debug tooling without adding extra polling to the active stream sync loop
- build verification passed for control-plane, relay-node, receiver-native, and Android app

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `99.7%`

### 4.42 Route policy smoke script

Done:

- added `control-plane/smoke-route-policy.ps1`
- the script performs an end-to-end local route-policy contract smoke:
  - device login
  - temporary host registration
  - short managed session create
  - `GET /api/sessions/{sessionId}/route/policy?sessionToken=...`
  - required field validation
  - session stop cleanup
- control-plane README now documents the smoke command
- runtime smoke passed against a real temporary session:
  - `routeKind = direct_host_push`
  - `routeActionHint = wait_for_telemetry`
  - `transportAnomalyKind = awaiting_telemetry`
  - `fallbackWarmupSeconds = 8`
  - `recoveryWarmupSeconds = 12`
- build verification passed for control-plane, relay-node, receiver-native, and Android app

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- whole big plan: approximately `100%` for the current platform-core phase

## 5. Product Phase

### 5.1 Control-plane file-backed persistence

Done:

- control-plane now loads a state snapshot at startup
- control-plane now automatically writes an atomic JSON snapshot after successful mutating API requests
- default snapshot path:
  - `%LOCALAPPDATA%\Everty\ControlPlane\state.json`
- override path:
  - `EVERTY_CONTROL_PLANE_STATE_PATH`
- persisted state includes:
  - devices
  - device access/refresh tokens
  - users
  - user access/refresh tokens
  - relays
  - hosts
  - sessions
  - telemetry events
- added `control-plane/smoke-persistence.ps1`
- persistence smoke passed:
  - start backend with temporary snapshot path
  - register device and host
  - stop backend
  - restart backend
  - verify the host is restored from the snapshot
- control-plane README now documents persistence path and smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `5%`

### 5.2 Control-plane readiness checks

Done:

- added `GET /api/ready`
- readiness now verifies that the configured persistence path is writable
- readiness response includes:
  - `Ready`
  - `PersistencePath`
  - `PersistenceWritable`
  - `PersistenceError`
  - registered host count
  - active session count
- unhealthy readiness returns `503` with diagnostic details
- added `control-plane/smoke-ready.ps1`
- readiness smoke passed with a temporary `EVERTY_CONTROL_PLANE_STATE_PATH`
- control-plane README now documents health/readiness endpoints and smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `8%`

### 5.3 Docker deployment profile

Done:

- added repository-level `.dockerignore`
- added `control-plane/Dockerfile`
- added `relay-node/Dockerfile`
- added repository-level `docker-compose.yml`
- compose stack includes:
  - `control-plane` on `5180/tcp`
  - `relay-node` on `6200/udp`
  - named volume `control-plane-data`
  - control-plane `/api/ready` healthcheck
- control-plane container uses:
  - `ASPNETCORE_URLS=http://+:5180`
  - `EVERTY_CONTROL_PLANE_STATE_PATH=/data/state.json`
- relay-node now supports env fallback for:
  - `EVERTY_RELAY_CONTROL_PLANE`
  - `EVERTY_RELAY_UDP_PORT`
  - `EVERTY_RELAY_PUBLIC_ADDRESS`
  - `EVERTY_RELAY_DISPLAY_NAME`
  - `EVERTY_RELAY_REGION`
- `docker compose config` passed
- control-plane and relay-node README files now document Docker usage and env knobs

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `12%`

### 5.4 Published service run path and deployment guide

Done:

- added deployment-oriented publish/run path for platform services
- `scripts/publish-platform.ps1` now publishes:
  - `control-plane`
  - `relay-node`
  - default output: `artifacts/platform/*`
- publish script now recreates each service output directory before publish to avoid stale binaries
- `scripts/run-control-plane.ps1` now runs the published `Everty.ControlPlane.dll`
- `scripts/run-relay-node.ps1` now runs the published `Everty.RelayNode.dll`
- run scripts now fail fast when artifacts are missing and instruct to run publish first
- `docs/deployment.md` now separates source-level `dotnet run`, published local services, publish artifacts, Docker Compose, and smoke tests
- `.gitignore` now excludes local `artifacts/`
- control-plane and relay-node README files now point to the published local service flow

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `16%`

### 5.5 Published platform smoke automation

Done:

- added `scripts/smoke-published-platform.ps1`
- the smoke script validates the deployment artifact run path without relying on `dotnet run`
- the smoke script checks:
  - published `control-plane` artifact exists
  - published `relay-node` artifact exists
  - published control-plane starts on a temporary local URL
  - `GET /api/ready` returns ready
  - published relay-node starts against that control-plane
  - relay process stays alive for the startup window
- the smoke script restores environment variables after running
- the smoke script force-cleans temporary child processes in `finally`
- `docs/deployment.md` now documents the published platform smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `18%`

### 5.6 Release metadata and publish manifest

Done:

- added repository-level `.NET` build metadata through `Directory.Build.props`
- release builds now have shared metadata:
  - `VersionPrefix=0.1.0`
  - `Company=Everty`
  - `Product=EvertyGame`
  - git repository metadata
  - deterministic builds
  - CI build flag support through `CI=true`
- `scripts/publish-platform.ps1` now accepts:
  - `-Version`
  - `-Channel`
- publish now writes `artifacts/platform/publish-manifest.json`
- publish manifest includes:
  - schema
  - UTC build timestamp
  - version/channel
  - runtime/self-contained mode
  - git commit / short commit
  - dirty worktree flag
  - published service outputs
- deployment docs now describe the manifest

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `21%`

### 5.7 Deployment environment templates

Done:

- added `deploy/control-plane.env.example`
- added `deploy/relay-node.env.example`
- control-plane env template includes:
  - `ASPNETCORE_URLS`
  - `EVERTY_CONTROL_PLANE_STATE_PATH`
- relay-node env template includes:
  - `EVERTY_RELAY_CONTROL_PLANE`
  - `EVERTY_RELAY_UDP_PORT`
  - `EVERTY_RELAY_PUBLIC_ADDRESS`
  - `EVERTY_RELAY_DISPLAY_NAME`
  - `EVERTY_RELAY_REGION`
- deployment docs now point to both env templates
- service README files now reference their env templates

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `23%`

### 5.8 Local operator start/stop scripts

Done:

- added `scripts/start-platform-local.ps1`
- added `scripts/stop-platform-local.ps1`
- start script can optionally publish first through `-PublishFirst`
- start script runs published control-plane and relay-node in the background
- start script waits for control-plane `/api/ready` before starting relay-node
- start script writes:
  - pid files to `artifacts/platform/run`
  - logs to `artifacts/platform/logs`
  - default persisted state to `artifacts/platform/data/state.json`
- stop script shuts down relay-node first, then control-plane
- deployment docs now document the local operator start/stop flow

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `27%`

### 5.9 Control-plane runtime security configuration

Done:

- added `ControlPlaneOptions` loaded from environment variables
- control-plane now supports configurable token lifetimes:
  - `EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS`
  - `EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS`
- control-plane now supports configurable request body cap:
  - `EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES`
- Kestrel request body size limit now follows that setting
- middleware now rejects oversized requests with `413` and `request_too_large`
- middleware now emits security headers:
  - `X-Content-Type-Options: nosniff`
  - `X-Frame-Options: DENY`
  - `Referrer-Policy: no-referrer`
  - `Permissions-Policy: camera=(), microphone=(), geolocation=()`
  - `Cache-Control: no-store` on `/api`
- added public non-secret runtime config endpoint:
  - `GET /api/config/runtime`
- control-plane env template and README now document the new knobs

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `31%`

### 5.10 Control-plane security smoke

Done:

- added `control-plane/smoke-security.ps1`
- the smoke starts control-plane with temporary security env values
- the smoke validates:
  - `/api/config/runtime` reflects token TTL and max request body config
  - `/api/health` includes security headers
  - oversized auth request is rejected with `413`
- deployment docs now include the security smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `34%`

### 5.11 Container security config alignment

Done:

- Docker Compose control-plane service now carries the same security/runtime env defaults as the env template
- control-plane Dockerfile now declares defaults for:
  - `EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS`
  - `EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS`
  - `EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES`
- this keeps local publish, env-template, and Docker deployment paths aligned

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `36%`

### 5.12 Authenticated operator diagnostics API

Done:

- added optional operator API auth through `EVERTY_CONTROL_PLANE_OPERATOR_KEY`
- added operator key support through:
  - `X-Everty-Operator-Key`
  - `Authorization: Bearer`
- added `GET /api/admin/summary`
- added `GET /api/admin/sessions`
- admin responses expose operational diagnostics without returning device secrets, relay secrets, session tokens, refresh tokens, or access tokens
- `/api/config/runtime` now reports whether operator auth is configured without returning the key
- control-plane README documents operator endpoints and auth headers
- env template, Dockerfile, and Docker Compose now include operator key settings

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `40%`

### 5.13 Operator API smoke

Done:

- added `control-plane/smoke-admin.ps1`
- smoke validates:
  - runtime config reports operator auth configured
  - admin summary without key returns `401`
  - admin summary with `X-Everty-Operator-Key` returns success
  - admin sessions with bearer operator key returns success
- deployment docs now include the admin smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `42%`

### 5.14 Authenticated operator actions

Done:

- added `POST /api/admin/hosts/{hostId}/availability`
- added `POST /api/admin/relays/{relayId}/availability`
- added `POST /api/admin/sessions/{sessionId}/stop`
- operator host action can set:
  - `Offline`
  - `Online`
  - `Busy`
  - `Disabled`
- operator relay action can set:
  - `Offline`
  - `Online`
  - `Disabled`
- operator session stop uses the same session cleanup pattern as normal session stop
- host disable/offline is guarded when a host has an active session:
  - returns conflict unless `StopActiveSession=true`
  - if explicit, stops the active session before changing availability
- control-plane README now documents the new operator action endpoints

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `46%`

### 5.15 Operator action smoke coverage

Done:

- extended `control-plane/smoke-admin.ps1`
- admin smoke now creates a temporary device, host, relay, and session
- admin smoke now validates:
  - operator session stop changes session status to `Stopped`
  - operator host availability can disable the host
  - operator relay availability can disable the relay
- this confirms the new operator actions work through HTTP, not only compile

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `48%`

### 5.16 Operator CLI

Done:

- added `scripts/control-plane-admin.ps1`
- CLI supports:
  - `summary`
  - `sessions`
  - `stop-session`
  - `set-host`
  - `set-relay`
- CLI reads operator key from:
  - `-OperatorKey`
  - `EVERTY_CONTROL_PLANE_OPERATOR_KEY`
- deployment docs now include CLI examples for diagnostics and actions

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `51%`

### 5.17 Operator CLI smoke

Done:

- added `scripts/smoke-control-plane-admin-cli.ps1`
- smoke creates a temporary device, host, relay, and session
- smoke validates the CLI commands:
  - `summary`
  - `sessions`
  - `stop-session`
  - `set-host`
  - `set-relay`
- deployment docs now include the CLI smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `53%`

### 5.18 Operator dashboard

Done:

- added `GET /admin`
- dashboard is a lightweight built-in operator console with no external frontend build step
- dashboard supports:
  - operator key entry
  - summary metrics
  - session list
  - stop session
  - disable host
  - disable relay
- dashboard calls the existing authenticated operator API and does not introduce a second admin protocol
- control-plane README and deployment docs now point to `/admin`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `57%`

### 5.19 Operator dashboard smoke

Done:

- added `control-plane/smoke-admin-dashboard.ps1`
- smoke validates:
  - dashboard endpoint returns success
  - content type is `text/html`
  - HTML contains expected operator console markers
  - dashboard JS references the admin API endpoints/actions
- deployment docs now include the dashboard smoke command

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `59%`

### 5.20 Marketplace host offer skeleton

Done:

- added persisted host marketplace offers in control-plane state
- added `POST /api/admin/hosts/{hostId}/offer`
- added authenticated `GET /api/marketplace/hosts`
- marketplace host offers expose host identity, region, endpoint, capabilities, encoder list, price, currency, and description
- operator dashboard can now save/list/unlist a host offer
- admin summary now reports total/listed marketplace offers

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `62%`

### 5.21 Marketplace smoke and CLI coverage

Done:

- extended `scripts/control-plane-admin.ps1` with `set-offer`
- extended `control-plane/smoke-admin.ps1` to verify admin offer creation and marketplace listing through client auth
- extended `scripts/smoke-control-plane-admin-cli.ps1` to verify `set-offer`
- extended dashboard smoke markers for the offer form/action
- documented marketplace/operator offer endpoints and CLI usage

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `64%`

### 5.22 Billing ledger and hold skeleton

Done:

- added persisted billing state to control-plane snapshots
- added billing accounts, billing session records, and ledger entries
- added automatic hold creation on session create
- added capture/finalization on session stop
- added settlement endpoint to move captured value into host balance
- admin session summaries now expose billing state directly
- operator dashboard now shows billing state per session and can settle it

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `68%`

### 5.23 Billing smoke and CLI coverage

Done:

- added `GET /api/admin/billing/summary`
- added `GET /api/billing/sessions/{sessionId}`
- added `POST /api/admin/billing/sessions/{sessionId}/settle`
- extended `scripts/control-plane-admin.ps1` with:
  - `billing-summary`
  - `settle-billing`
- extended `control-plane/smoke-admin.ps1` to verify:
  - hold creation
  - billing session readout
  - settlement
- extended `scripts/smoke-control-plane-admin-cli.ps1` to verify billing summary and settlement
- dashboard smoke now checks billing endpoint markers

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `68%`

### 5.24 Billing rate snapshot hardening

Done:

- billing sessions now snapshot `HourlyRate` at session creation time
- capture now uses the session billing snapshot instead of the current mutable host offer price
- client billing session details now include:
  - hourly rate
  - platform commission rate
- this prevents old sessions from being recalculated with a newer marketplace price

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `70%`

### 5.25 Billing operator ledger visibility

Done:

- added `GET /api/admin/billing/accounts`
- added `GET /api/admin/billing/ledger?limit=100`
- added `billing-accounts` and `billing-ledger` to `scripts/control-plane-admin.ps1`
- extended admin smoke to verify:
  - account creation
  - ledger hold entry creation
  - hourly rate snapshot on billing session details
- extended admin CLI smoke to verify:
  - billing accounts command
  - billing ledger command
- deployment docs and control-plane README now document the new billing inspection commands/endpoints

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `72%`

### 5.26 Marketplace-aware managed host list

Done:

- desktop managed receiver now loads hosts from `GET /api/marketplace/hosts` first
- Android PC receiver now loads hosts from `GET /api/marketplace/hosts` first
- both clients keep a fallback to legacy `GET /api/hosts`
- host models now carry:
  - price per hour
  - currency
  - description
- desktop host dropdown now includes price when available
- Android host list now shows price when available

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `75%`

### 5.27 Provider-neutral payment contract

Done:

- added `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER`
- added `GET /api/admin/billing/provider`
- billing sessions now carry provider metadata:
  - `PaymentProvider`
  - `ProviderHoldId`
  - `ProviderCaptureId`
  - `ProviderSettlementId`
- hold/capture/settle ledger notes now include the provider
- existing manual billing flow remains the default provider mode
- old billing snapshots remain readable through provider fallback to `manual`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `78%`

### 5.28 Payment provider smoke and deployment defaults

Done:

- added `billing-provider` to `scripts/control-plane-admin.ps1`
- extended admin smoke to verify:
  - manual provider config
  - provider hold id
  - provider capture id
  - provider settlement id
- extended admin CLI smoke to verify `billing-provider`
- added payment provider env default to:
  - `deploy/control-plane.env.example`
  - `control-plane/Dockerfile`
  - `docker-compose.yml`
- deployment docs and control-plane README now document the payment provider env/endpoint

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `78%`

### 5.29 Payment provider adapter boundary

Done:

- added the `IPaymentProvider` runtime boundary
- added the manual payment provider adapter
- routed billing hold/capture/settle through the provider adapter
- removed direct provider id generation from the billing lifecycle path
- runtime config now exposes:
  - payment provider
  - payment provider mode
- non-`manual` provider names now report `external_stub` mode until a real provider implementation is added
- existing manual ledger and settlement semantics remain unchanged

Validation:

- `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
- `control-plane\smoke-admin.ps1`
- `scripts\smoke-control-plane-admin-cli.ps1`
- `control-plane\smoke-security.ps1`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `81%`

### 5.30 External HTTP payment provider mode

Done:

- added payment provider endpoint configuration:
  - `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT`
  - `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY`
- expanded the provider adapter with modes:
  - `manual`
  - `external_stub`
  - `external_http`
- when `external_http` is active, billing operations now POST JSON to the configured provider endpoint for:
  - `hold`
  - `capture`
  - `settle`
- provider responses now supply the stored provider reference ids
- billing provider diagnostics now expose:
  - `EndpointConfigured`
  - `ExternalCallsEnabled`
- runtime config now exposes payment provider endpoint readiness without leaking the API key

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `84%`

### 5.31 Payment provider smoke and deployment coverage

Done:

- added `control-plane/smoke-payment-provider.ps1`
- the smoke starts a local fake payment provider callback endpoint
- the smoke starts control-plane in `external_http` mode
- the smoke verifies:
  - runtime config reports `external_http`
  - admin billing provider reports external calls enabled
  - session create triggers external `hold`
  - admin session stop triggers external `capture`
  - admin billing settle triggers external `settle`
  - provider callback reference ids are persisted into billing session state
- deployment/env defaults now include:
  - `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT`
  - `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY`
- Docker Compose and control-plane Dockerfile now carry the new payment provider env variables
- control-plane README and deployment docs document the payment provider modes/env knobs

Validation:

- `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
- `docker compose config`
- `control-plane\smoke-payment-provider.ps1`
- `control-plane\smoke-admin.ps1`
- `control-plane\smoke-security.ps1`
- `scripts\smoke-control-plane-admin-cli.ps1`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `85%`

### 5.32 Payment provider failure policy

Done:

- hold failures now block session creation with `payment_provider_hold_failed`
- hold failure is handled before the session is committed and before the host is marked busy
- capture failures no longer escape as raw unhandled provider exceptions
- settle failures no longer mutate host balance or mark billing as settled
- capture/settle provider failures are recorded as billing failure state instead of being silent

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `87%`

### 5.33 Billing payment audit fields and failure smoke

Done:

- billing session details now expose:
  - `LastPaymentError`
  - `LastPaymentAttemptUtc`
- billing records now persist the last payment attempt timestamp
- billing records now persist the last provider error in a bounded form
- billing ledger now records:
  - `capture_failed`
  - `settle_failed`
- extended `control-plane/smoke-payment-provider.ps1` with forced failure mode:
  - `-FailAction capture`
- the forced capture failure smoke verifies:
  - provider callback was attempted
  - billing state becomes `Failed`
  - provider capture id is not incorrectly written
  - last payment error is visible on the billing session
- control-plane README and deployment docs now document payment provider failure policy

Validation:

- `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
- `control-plane\smoke-payment-provider.ps1`
- `control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5204 -ProviderPort 5205 -FailAction capture`
- `control-plane\smoke-admin.ps1`
- `scripts\smoke-control-plane-admin-cli.ps1`
- `control-plane\smoke-security.ps1`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `88%`

### 5.34 Billing reconciliation operator API

Done:

- added `GET /api/admin/billing/reconciliation`
- reconciliation items now expose:
  - session id
  - host id
  - billing status
  - session status
  - payment provider
  - required action
  - provider ids
  - last payment error
  - last payment attempt timestamp
- reconciliation currently flags:
  - failed capture retry
  - failed settle retry
  - captured sessions awaiting settlement
  - held sessions whose stream session is already stopped/expired
- added `POST /api/admin/billing/sessions/{sessionId}/retry`
- retry supports:
  - `auto`
  - `capture`
  - `settle`
- retry blocks capture when the stream session is not stopped/expired yet

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `90%`

### 5.35 Billing reconciliation CLI and retry smoke

Done:

- added operator CLI command:
  - `billing-reconciliation`
- added operator CLI command:
  - `retry-billing`
- extended CLI smoke to cover `billing-reconciliation`
- extended payment provider smoke with:
  - `-FailOnce`
- provider smoke now verifies:
  - forced capture failure appears in reconciliation as `capture`
  - `retry` can recover the failed capture after the provider becomes healthy
  - billing transitions from `Failed` to `Captured`
  - provider capture id is written only after successful retry
- control-plane README and deployment docs now document the reconciliation/retry surfaces

Validation:

- `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
- `control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5206 -ProviderPort 5207 -FailAction capture -FailOnce`
- `scripts\smoke-control-plane-admin-cli.ps1`
- `control-plane\smoke-admin.ps1`
- `control-plane\smoke-security.ps1`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `91%`

### 5.36 Billing reconciliation dashboard surfacing

Done:

- `/admin` dashboard now includes a `Billing Reconciliation` panel
- dashboard loads:
  - `GET /api/admin/billing/reconciliation`
- dashboard shows reconciliation rows with:
  - session id
  - billing/session status
  - payment provider
  - capture/settle action required
  - hold/captured/settled amounts
  - last provider error
  - last payment attempt timestamp
- dashboard can retry billing through:
  - `POST /api/admin/billing/sessions/{sessionId}/retry`
- summary metrics now include the count of billing actions requiring operator attention
- control-plane README and deployment docs now mention dashboard reconciliation/retry support

### 5.37 Dashboard reconciliation smoke coverage

Done:

- extended `control-plane/smoke-admin-dashboard.ps1`
- dashboard smoke now verifies markers for:
  - `GET /api/admin/billing/reconciliation`
  - `operator_console_retry`
- the smoke confirms the dashboard HTML route exposes the new reconciliation surface

Validation:

- `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
- `control-plane\smoke-admin-dashboard.ps1`
- `control-plane\smoke-admin.ps1`
- `scripts\smoke-control-plane-admin-cli.ps1`
- `control-plane\smoke-security.ps1`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `93%`

### 5.38 Product readiness smoke aggregator

Done:

- added `scripts/smoke-product-readiness.ps1`
- the aggregator runs:
  - `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
  - `dotnet build relay-node\Everty.RelayNode.csproj --no-restore`
  - `docker compose config`
  - `control-plane\smoke-ready.ps1`
  - `control-plane\smoke-persistence.ps1`
  - `control-plane\smoke-security.ps1`
  - `control-plane\smoke-admin.ps1`
  - `control-plane\smoke-admin-dashboard.ps1`
  - `scripts\smoke-control-plane-admin-cli.ps1`
  - `control-plane\smoke-route-policy.ps1` under a temporary control-plane process
- the aggregator supports:
  - `-SkipPaymentProvider`
  - `-SkipDockerCompose`
  - `-IncludePublishedPlatform`
- route policy smoke is now runnable inside the readiness sweep without requiring a manually started backend

### 5.39 Product readiness documentation and payment verification

Done:

- deployment docs now document `scripts/smoke-product-readiness.ps1`
- control-plane README now links to the broader product-readiness sweep
- product readiness sweep passed in platform mode:
  - `scripts\smoke-product-readiness.ps1 -SkipPaymentProvider`
- external payment provider happy-path smoke passed separately
- external payment provider retry smoke passed separately

Validation:

- `scripts\smoke-product-readiness.ps1 -SkipPaymentProvider`
- `control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5234 -ProviderPort 5235`
- `control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5236 -ProviderPort 5237 -FailAction capture -FailOnce`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `96%`

### 5.40 Published artifact release pass

Done:

- published platform services with explicit product-phase metadata:
  - `scripts\publish-platform.ps1 -Version 0.1.0-product -Channel local-product`
- verified published artifact startup:
  - `scripts\smoke-published-platform.ps1 -Url http://127.0.0.1:5238 -RelayUdpPort 6238`
- publish manifest was generated at:
  - `artifacts/platform/publish-manifest.json`
- published outputs were refreshed:
  - `artifacts/platform/control-plane`
  - `artifacts/platform/relay-node`

### 5.41 Release readiness notes cleanup

Done:

- added `docs/release-readiness.md`
- release readiness notes now list:
  - release candidate scope
  - primary release commands
  - published artifact locations
  - operator surfaces
  - env templates
  - known remaining product gaps
- cleaned up the payment provider env/smoke section in `docs/deployment.md`
- deployment docs now point to release readiness notes

Validation:

- `scripts\publish-platform.ps1 -Version 0.1.0-product -Channel local-product`
- `scripts\smoke-published-platform.ps1 -Url http://127.0.0.1:5238 -RelayUdpPort 6238`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `98%`

### 5.42 Docker Compose env-file alignment

Done:

- `docker-compose.yml` now uses environment interpolation instead of hardcoded local-only values
- added `deploy/docker-compose.env.example`
- compose config can now be resolved through:
  - `docker compose --env-file deploy/docker-compose.env.example config`
- control-plane/relay compose ports and runtime env values are now overridable without editing the compose file

### 5.43 Release hygiene audit script

Done:

- added `scripts/audit-release-hygiene.ps1`
- the audit verifies:
  - required deployment/release docs exist
  - required env templates exist
  - local publish artifacts remain ignored by Git
  - publish manifest path is ignored by Git
- the audit also reports tracked/untracked change counts for the current worktree

### 5.44 Product readiness sweep hygiene coverage

Done:

- `scripts/smoke-product-readiness.ps1` now runs:
  - `scripts\audit-release-hygiene.ps1`
- readiness sweep now resolves Docker Compose through:
  - `docker compose --env-file deploy/docker-compose.env.example config`
- deployment docs and release-readiness notes now document:
  - `deploy/docker-compose.env.example`
  - `scripts\audit-release-hygiene.ps1`

Validation:

- `docker compose --env-file deploy/docker-compose.env.example config`
- `scripts\audit-release-hygiene.ps1`
- `scripts\smoke-product-readiness.ps1 -SkipPaymentProvider`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `99%`

### 5.45 Commit scope audit

Done:

- added `scripts/audit-commit-scope.ps1`
- the audit groups the current git worktree by subsystem:
  - repo root / deploy / docs / scripts
  - control-plane / relay-node
  - receiver-native / Android app
- the audit returns:
  - tracked/untracked totals
  - per-bucket counts
  - sample paths
  - recommended commit order

### 5.46 Release candidate handoff note

Done:

- added `docs/release-candidate-handoff.md`
- handoff note now captures:
  - release candidate scope
  - recommended validation order
  - payment-provider verification commands
  - operator entry points
  - commit framing guidance
  - explicit non-release items
- release-readiness and deployment docs now point to:
  - `scripts\audit-commit-scope.ps1`
  - `docs\release-candidate-handoff.md`

Validation:

- `scripts\audit-commit-scope.ps1`
- `scripts\audit-release-hygiene.ps1`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `100%` for the current release-candidate scope

### 5.47 Simple local UX and demo-auth onboarding

Done:

- control-plane now seeds demo users in local/dev mode:
  - `admin/admin`
  - `test/test`
- runtime config now exposes:
  - `demoAuthEnabled`
- desktop and Android simple mode now load hosts from:
  - `GET /api/hosts`
- marketplace offers are no longer required for the first local pairing flow
- Android `PC Receiver` now exposes a simpler Russian onboarding flow:
  - server
  - quick demo login
  - host list
  - connect
- desktop `receiver-native` now exposes:
  - quick demo login buttons
  - simple mode by default
  - clearer host-side registration / waiting-for-client status
  - advanced mode toggle for manual controls
- local runbook and deployment docs now describe the new simple flow

Validation:

- `dotnet build control-plane\Everty.ControlPlane.csproj --no-restore`
- `dotnet build receiver-native\ReceiverNative.csproj --no-restore -p:UseAppHost=false`
- `.\gradlew.bat --no-daemon :app:assembleDebug --console=plain`

Updated progress after this slice:

- streaming core: approximately `84%`
- Session MVP from the plan: approximately `100%`
- platform-core phase: `100%`
- product phase: approximately `100%` for the current local/demo release-candidate scope
