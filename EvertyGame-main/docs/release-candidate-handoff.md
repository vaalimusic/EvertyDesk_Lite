# Everty Release Candidate Handoff

This handoff note is for the current local product-phase release candidate.

## Release Candidate Scope

- `control-plane` ASP.NET Core service
- `relay-node` UDP relay service
- operator dashboard at `GET /admin`
- operator CLI in `scripts/control-plane-admin.ps1`
- managed desktop/Android control-plane client flow
- marketplace/billing/provider adapter surfaces
- simple local onboarding with demo users `admin/admin` and `test/test`
- first-pair host discovery through regular `/api/hosts`, without requiring marketplace offers

## Recommended Validation Order

```powershell
powershell -ExecutionPolicy Bypass -File scripts/smoke-product-readiness.ps1 -SkipPaymentProvider
powershell -ExecutionPolicy Bypass -File scripts/audit-release-hygiene.ps1
powershell -ExecutionPolicy Bypass -File scripts/audit-commit-scope.ps1
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1 -Version 0.1.0-product -Channel local-product
powershell -ExecutionPolicy Bypass -File scripts/smoke-published-platform.ps1
```

For payment-provider callback verification:

```powershell
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1
powershell -ExecutionPolicy Bypass -File control-plane/smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5206 -ProviderPort 5207 -FailAction capture -FailOnce
```

## Operator Entry Points

- dashboard: `http://127.0.0.1:5180/admin`
- readiness: `http://127.0.0.1:5180/api/ready`
- local compose env file: `deploy/docker-compose.env.example`
- local user runbook: `docs/local-runbook-ru.md`

## Commit Framing Guidance

Recommended commit grouping:

1. repo root / deploy / docs / scripts
2. control-plane / relay-node
3. receiver-native / Android managed clients

Use `scripts/audit-commit-scope.ps1` before the final staging pass so the worktree is grouped by subsystem instead of by accidental file order.

## Known Non-Release Items

- real provider-specific payment integration is still outside the generic `external_http` adapter
- desktop/mobile app distribution packaging is still separate from platform service publish
- public TLS/domain/secrets rollout is still deployment work, not part of the local release-candidate runbook
