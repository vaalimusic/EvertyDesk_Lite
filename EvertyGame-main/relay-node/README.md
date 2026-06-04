# Everty Relay Node

Minimal UDP EVRT relay for the current managed-session MVP.

## What it does

- registers itself in `control-plane`
- sends periodic relay heartbeat
- accepts `relay_register` control packets from sender / receiver
- forwards EVRT UDP packets between registered sender and receiver endpoints for the same `sessionId`
- answers `nat_probe` with the observed public UDP endpoint of the caller

## Run locally

```powershell
dotnet run --project relay-node/Everty.RelayNode.csproj -- `
  --control-plane http://127.0.0.1:5180 `
  --udp-port 6200 `
  --public-address 127.0.0.1 `
  --display-name "Local Relay" `
  --region global
```

The same settings can be provided through environment variables:

- `EVERTY_RELAY_CONTROL_PLANE`
- `EVERTY_RELAY_UDP_PORT`
- `EVERTY_RELAY_PUBLIC_ADDRESS`
- `EVERTY_RELAY_DISPLAY_NAME`
- `EVERTY_RELAY_REGION`

## Docker

From the repository root:

```powershell
docker compose up --build relay-node
```

The compose stack exposes relay UDP on `127.0.0.1:6200/udp` and points it at the `control-plane` service.

## Published local service

For a deployment-like local run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-platform.ps1
powershell -ExecutionPolicy Bypass -File scripts/run-relay-node.ps1
```

More deployment notes are in `docs/deployment.md`.

Environment template:

- `deploy/relay-node.env.example`

## Current limitation

- this is relay fallback only
- there is no direct hole punching yet
- this is still a lightweight custom NAT probe, not full TURN/STUN/ICE
- relay state is in-memory and intended for MVP / local development
