# Everty Product-Phase Release Readiness

Current release candidate scope:

- Windows sender/receiver native streaming experiment.
- Android PC receiver managed flow.
- Control-plane managed sessions, relay routing, route policy diagnostics, marketplace offers, billing ledger, payment provider adapter, operator dashboard/CLI.
- Relay-node UDP relay service.
- Local demo onboarding through seeded users `admin/admin` and `test/test`.
- Simple desktop/Android pairing flow through regular host inventory from `GET /api/hosts`.

Primary release commands:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/smoke-product-readiness.ps1 -SkipPaymentProvider
powershell -ExecutionPolicy Bypass -File scripts/audit-release-hygiene.ps1
powershell -ExecutionPolicy Bypass -File scripts/audit-commit-scope.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5206 -ProviderPort 5207 -FailAction capture -FailOnce
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1 -Version 0.1.0-product -Channel local-product
powershell -ExecutionPolicy Bypass -File scripts/smoke-published-platform.ps1
```

Published artifacts:

- `artifacts/platform/control-plane`
- `artifacts/platform/relay-node`
- `artifacts/platform/publish-manifest.json`

Operator surfaces:

- `GET /admin`
- `scripts/control-plane-admin.ps1`
- `scripts/smoke-product-readiness.ps1`

Key environment templates:

- `deploy/control-plane.env.example`
- `deploy/relay-node.env.example`
- `deploy/docker-compose.env.example`

Release hygiene checks:

- `scripts/audit-release-hygiene.ps1`
- `scripts/audit-commit-scope.ps1`
- `docker compose --env-file deploy/docker-compose.env.example config`

Release handoff note:

- `docs/release-candidate-handoff.md`
- `docs/local-runbook-ru.md`

Known remaining product gaps:

- Real payment provider contract must be mapped to the chosen provider API; current `external_http` mode is a generic adapter boundary.
- Windows/Android desktop-native distribution packaging is still separate from platform service publish.
- Public deployment still needs real domain/TLS/secrets management outside the local/Docker Compose profile.
