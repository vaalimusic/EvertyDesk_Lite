# Everty Control Plane

Minimal ASP.NET Core backend for the first platform wave around the existing Everty streaming stack.

## Current scope

- host registration
- host heartbeat
- host lease polling
- host inventory
- readiness checks
- device auth + refresh
- user auth + refresh
- relay node register / heartbeat / inventory
- session create / inspect / activate / stop
- session connect instructions
- session route policy diagnostics
- session NAT probe ingest / state
- telemetry ingest
- receiver endpoint in session lease
- desired stream settings in session lease
- in-memory storage for local development

## Run locally

```powershell
dotnet run --project control-plane/Everty.ControlPlane.csproj
```

Default local URL:

- `http://127.0.0.1:5180`

## Docker

From the repository root:

```powershell
docker compose up --build
```

The compose stack exposes:

- control-plane HTTP: `http://127.0.0.1:5180`
- relay UDP: `127.0.0.1:6200/udp`

Control-plane container settings:

- `ASPNETCORE_URLS=http://+:5180`
- `EVERTY_CONTROL_PLANE_STATE_PATH=/data/state.json`

The compose file mounts `/data` to the named volume `control-plane-data`.

## Published local service

For a deployment-like local run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1
powershell -ExecutionPolicy Bypass -File scripts/run-control-plane.ps1
```

More deployment notes are in `docs/deployment.md`.

Environment template:

- `deploy/control-plane.env.example`

Runtime configuration endpoint:

- `GET /api/config/runtime`

Marketplace endpoints:

- `GET /api/marketplace/hosts`

Billing endpoints:

- `GET /api/admin/billing/summary`
- `GET /api/admin/billing/provider`
- `GET /api/admin/billing/accounts`
- `GET /api/admin/billing/ledger?limit=100`
- `GET /api/admin/billing/reconciliation`
- `POST /api/admin/billing/sessions/{sessionId}/settle`
- `POST /api/admin/billing/sessions/{sessionId}/retry`
- `GET /api/billing/sessions/{sessionId}`

Operator endpoints:

- `GET /admin`
- `GET /api/admin/summary`
- `GET /api/admin/sessions`
- `POST /api/admin/hosts/{hostId}/offer`
- `POST /api/admin/hosts/{hostId}/availability`
- `POST /api/admin/relays/{relayId}/availability`
- `POST /api/admin/sessions/{sessionId}/stop`
- `POST /api/admin/billing/sessions/{sessionId}/settle`
- `POST /api/admin/billing/sessions/{sessionId}/retry`

The built-in `/admin` dashboard surfaces managed sessions, marketplace offers, billing settlement, and billing reconciliation retry actions.

Operator endpoints require either:

- `X-Everty-Operator-Key: <EVERTY_CONTROL_PLANE_OPERATOR_KEY>`
- `Authorization: Bearer <EVERTY_CONTROL_PLANE_OPERATOR_KEY>`

Security/runtime environment variables:

- `EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS`
- `EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS`
- `EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES`
- `EVERTY_CONTROL_PLANE_OPERATOR_KEY`
- `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER`
- `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT`
- `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY`

Payment provider modes:

- `manual`: default local/manual ledger mode.
- `external_stub`: set a non-`manual` provider name without an endpoint to generate provider-shaped references without making external calls.
- `external_http`: set both `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER=<provider>` and `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT=https://...`; hold/capture/settle are posted as JSON to the endpoint.

Payment provider failure policy:

- hold failure blocks session creation with `payment_provider_hold_failed`.
- capture/settle failures are recorded on the billing session as `Failed` with `LastPaymentError` and `LastPaymentAttemptUtc`.
- `control-plane/smoke-payment-provider.ps1` verifies both the success path and forced provider failure path.

## Health and readiness

- `GET /api/health` is the lightweight liveness check.
- `GET /api/ready` verifies that the backend can write to the configured persistence path.

To smoke-test readiness locally:

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-ready.ps1
```

For a broader product-readiness sweep from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/smoke-product-readiness.ps1
```

## Route policy smoke

With the control plane running locally:

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-route-policy.ps1
```

The script registers a temporary device/host, creates a short managed session, calls:

- `GET /api/sessions/{sessionId}/route/policy?sessionToken=...`

and verifies the route-policy contract fields without forcing fallback/recovery.

## Persistence smoke

The control plane persists state to:

- `EVERTY_CONTROL_PLANE_STATE_PATH`, when set
- otherwise `%LOCALAPPDATA%\Everty\ControlPlane\state.json`

To verify restart persistence locally:

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-persistence.ps1
```

The script starts the backend with a temporary snapshot path, registers a host, restarts the backend, and verifies that the host is restored from the snapshot.

## Receiver Native integration

`receiver-native` can act as a basic host agent.

To enable it:

1. Run the control plane.
2. Start `ReceiverNative.exe`.
3. Leave the app in `Send`.
4. Fill the `Control` field with `http://127.0.0.1:5180`.

The sender HUD will then show:

- `Control plane`
- `Host registration`
- `Lease status`
- `Lease session`
- `Lease receiver`
- `Lease profile`

## Android client flow

`PC Receiver` on Android now supports:

- loading host list from control plane
- creating session from the app
- passing receiver endpoint + desired stream profile into the lease
- activating the session after the local receiver is ready
- reading direct connect instructions for the session
- stopping the backend session from the app

## Desktop client flow

`receiver-native` in `Receive` role now supports:

- device-based control-plane login
- loading host list from the backend
- starting a managed session from the desktop UI
- setting client region and relay preference
- activating the session after the local receiver starts listening
- reading direct connect instructions for the session
- stopping the backend session from the desktop UI or when leaving `Receive`

## Auth model

Current MVP auth is device-based:

- `POST /api/auth/device-login`
- `POST /api/auth/refresh`
- `GET /api/auth/me`

User auth is now also available:

- `POST /api/auth/users/register`
- `POST /api/auth/users/login`
- `POST /api/auth/users/refresh`
- `GET /api/auth/users/me`

Client-facing routes now require `Authorization: Bearer <accessToken>`:

- marketplace host offers
- billing session details
- host list/details
- relay list
- session create/details/connect/activate/stop
- session NAT probe publish
- session route policy diagnostics

Host-side routes still use host secret:

- register
- heartbeat
- lease polling

## Important limitation

This slice now already provides:

- host inventory
- relay inventory
- active lease visibility for the host
- basic sender telemetry to the backend
- backend-managed sender auto-start/auto-stop when a lease includes receiver endpoint and desired stream settings
- unattended sender auto-run only for authenticated client-created lease
- route metadata in session/lease:
  - `direct_host_push`
  - `direct_fallback`
  - `relay_assigned`
- NAT probe metadata in session/lease/connect:
  - `probeEndpoint`
  - `probeToken`
  - `natStatus`
- real relay transport can now be provided by `relay-node/`
- desktop receiver / Android receiver can now bind session route to relay and register themselves there
- sender can now use relay route from host lease and register itself there
- host agent / desktop receiver / Android receiver can now publish NAT probe results
- managed route policy now exposes explicit diagnostics through:
  - `GET /api/sessions/{sessionId}/route/policy?sessionToken=...`
  - route action hint/reason
  - transport loss level
  - transport anomaly kind/reason/confidence
  - fallback/recovery readiness windows
  - fallback/recovery cooldowns
  - NAT probe freshness

The current platform-core slice now has managed session lifecycle, relay fallback, direct punch metadata, route recovery/fallback policy, and route policy diagnostics. The next slice is product hardening: real WAN failure testing, security/routing policy tightening, and managed UX polish.
