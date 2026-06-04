# Everty Deployment Notes

This document covers the current product-phase deployment paths for the managed platform services.

Release readiness notes:

- `docs/release-readiness.md`
- `docs/local-runbook-ru.md`

## Local Dev

For source-level development, use `dotnet run` directly:

```powershell
dotnet run --project control-plane/Everty.ControlPlane.csproj --urls http://127.0.0.1:5180
dotnet run --project relay-node/Everty.RelayNode.csproj -- --control-plane http://127.0.0.1:5180
```

For the current local demo flow, control-plane also seeds demo users by default:

- `admin / admin`
- `test / test`

The simple desktop/Android client flow now uses the regular host inventory from `GET /api/hosts`. Marketplace offers and `/admin` remain operator-only surfaces and are no longer required for first-time local pairing.

## Published Local Services

First publish both platform services:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1
```

Run the control plane:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/run-control-plane.ps1
```

Run the relay node:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/run-relay-node.ps1
```

The run scripts execute the published DLLs from `artifacts/platform/*`. This keeps the local service run path closer to a server deployment than `dotnet run`.

To start both services in the background for local operator testing:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/start-platform-local.ps1 -PublishFirst
```

To stop them:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/stop-platform-local.ps1
```

Local logs and pid files are written under `artifacts/platform/logs` and `artifacts/platform/run`.

## Operator CLI

Open the lightweight operator dashboard:

```text
http://127.0.0.1:5180/admin
```

The dashboard exposes managed sessions, marketplace offers, billing settlement, and billing reconciliation retry actions.

The operator CLI uses `EVERTY_CONTROL_PLANE_OPERATOR_KEY` or `-OperatorKey`.

```powershell
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command summary
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command billing-summary
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command billing-provider
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command billing-accounts
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command billing-ledger -Limit 50
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command billing-reconciliation
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command sessions
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command set-offer -HostId <hostId> -PricePerHour 2.50 -Currency USD -Description "1080p60 NVENC host"
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command settle-billing -SessionId <sessionId> -Reason operator_settle
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command retry-billing -SessionId <sessionId> -RetryAction auto -Reason provider_recovered
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command stop-session -SessionId <sessionId> -Reason operator_stop
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command set-host -HostId <hostId> -Availability Disabled
powershell -ExecutionPolicy Bypass -File scripts/control-plane-admin.ps1 -Command set-relay -RelayId <relayId> -Availability Disabled
```

## Publish Artifacts

Portable framework-dependent publish:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1
```

Runtime-specific publish:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1 -Runtime win-x64
```

Self-contained runtime-specific publish:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1 -Runtime win-x64 -SelfContained
```

Output defaults to:

- `artifacts/platform/control-plane`
- `artifacts/platform/relay-node`
- `artifacts/platform/publish-manifest.json`

The publish script deletes and recreates each service output folder before publishing to avoid stale binaries.

The publish manifest records:

- build version / channel
- git commit and dirty-worktree flag
- target runtime and self-contained mode
- published service output paths

## Docker Compose

```powershell
docker compose up --build
```

Services:

- `control-plane`: `http://127.0.0.1:5180`
- `relay-node`: `127.0.0.1:6200/udp`

Compose can also be driven through an explicit env file:

```powershell
docker compose --env-file deploy/docker-compose.env.example config
docker compose --env-file deploy/docker-compose.env.example up --build
```

## Important Environment Variables

Control plane:

- `ASPNETCORE_URLS`
- `EVERTY_CONTROL_PLANE_STATE_PATH`
- `EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS`
- `EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS`
- `EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES`
- `EVERTY_CONTROL_PLANE_DEMO_AUTH_ENABLED`
- `EVERTY_CONTROL_PLANE_OPERATOR_KEY`
- `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER`
- `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT`
- `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY`

Payment provider modes:

- `manual`: local/manual ledger mode.
- `external_stub`: provider-shaped ids without external HTTP calls.
- `external_http`: hold/capture/settle are posted to `EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT`.

Smoke-test the external provider path and failure policy:

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5204 -ProviderPort 5205 -FailAction capture
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5206 -ProviderPort 5207 -FailAction capture -FailOnce
```

Relay node:

- `EVERTY_RELAY_CONTROL_PLANE`
- `EVERTY_RELAY_UDP_PORT`
- `EVERTY_RELAY_PUBLIC_ADDRESS`
- `EVERTY_RELAY_DISPLAY_NAME`
- `EVERTY_RELAY_REGION`

Environment templates:

- `deploy/control-plane.env.example`
- `deploy/relay-node.env.example`
- `deploy/docker-compose.env.example`

## Smoke Tests

Product-readiness aggregator:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/smoke-product-readiness.ps1
```

Useful switches:

- `-SkipPaymentProvider`: skip the slower external provider callback smokes.
- `-SkipDockerCompose`: skip `docker compose config` when Docker is unavailable.
- `-IncludePublishedPlatform`: also run `scripts/smoke-published-platform.ps1`; requires `scripts/publish-platform.ps1` first.

Release-hygiene audit:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/audit-release-hygiene.ps1
powershell -ExecutionPolicy Bypass -File scripts/audit-commit-scope.ps1
```

The audits check that release/deployment docs and env templates exist, that local publish artifacts remain ignored by Git, and how the current worktree splits into subsystem commit buckets.

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-ready.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-persistence.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-security.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-admin.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-admin-dashboard.ps1
powershell -ExecutionPolicy Bypass -File scripts/smoke-control-plane-admin-cli.ps1
```

With a control plane already running:

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-route-policy.ps1
```

For published service startup:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1
powershell -ExecutionPolicy Bypass -File scripts/smoke-published-platform.ps1
```
