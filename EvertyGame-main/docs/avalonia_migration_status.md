# Avalonia Desktop Migration Status

Status: 99%

Done

- New Avalonia shell created in `desktop-avalonia/`.
- Host control-plane wiring works.
- Host start/stop/refresh buttons wired.
- Desktop host/client contracts added.
- Launcher script added: `scripts/run-desktop-avalonia.ps1`.
- Build passes.
- Client flow now creates/stops managed sessions through control plane.
- Client preset selection added.
- Client HUD shows route / endpoint / codec after connect.
- Diagnostics drawer added to Avalonia shell.
- Playback surface now shows real host/client route, codec, endpoint, and encoder facts.
- Control plane session service extracted behind interface.
- Shared control-plane contracts extracted to `control-plane-contracts/`.
- Client connect/stop/session state now routed through service boundary.
- Windows sender runtime extracted behind interface adapter.
- Avalonia shell now depends on runtime/service abstractions instead of direct concrete session classes.
- Playback surface abstraction wired into shell.
- Host/client cards now show a shared playback surface placeholder.
- MainWindowViewModel no longer calls Windows sender static capture/probe APIs directly.
- Host control-plane agent extracted behind adapter.
- VM no longer owns concrete host agent directly.
- Managed session restore now runs on Avalonia startup with error handling.
- Advanced controls now collapsed by default in Avalonia shell.
- Avalonia UI prefs now persist `Advanced` and `Diagnostics` state across restarts.
- Host code now visible in hero and host card, with copy action.
- Client auto-selects host by saved short code after host refresh.
- Host/client actions now switch shell to the relevant tab automatically.
- Hotkeys added for host code and diagnostics copy.
- Escape collapses advanced/diagnostics; Ctrl+Shift+1/2 switch tabs.

Pending

- Cross-platform target cleanup.
- Final diagnostics drawer / advanced panel polish.
