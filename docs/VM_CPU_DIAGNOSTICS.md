# VM / VMware CPU diagnostics

Use this when a tester says EvertyDesk or a VM console loads the CPU.

## Quick answer

Yes, this class of issue is possible.

The main risks are:

- GUI/event loop busy polling;
- repainting at 60 FPS while the provider can only produce slow screenshots;
- draining and rendering every queued VM frame instead of only the newest one;
- provider CLI calls such as `vmrun list` hanging or being executed too often;
- the hypervisor process itself (`vmware-vmx`, `VirtualBoxVM`, `vmconnect`, `mstsc`)
  consuming CPU outside EvertyDesk.

## Current mitigations

Desktop Next RDP/VM viewer:

- uses `ControlFlow::Wait`, not `ControlFlow::Poll`;
- wakes with `WaitUntil` on the next poll deadline;
- polls at 16 ms, i.e. 60 FPS budget, not 8 ms / 125 FPS;
- drains queued frames to the newest frame and renders only that frame.

Archived egui Lite VM UI:

- RDP/VRDE sessions still request repaint at 16 ms;
- Hyper-V WMI thumbnail preview requests repaint at 50 ms;
- VirtualBox screenshot preview requests repaint at `SCREENSHOT_INTERVAL`
  currently 200 ms;
- VMware `vmrun list` has a 4 second timeout and 30 second inventory cache.

## Collect a diagnostic bundle

```powershell
.\scripts\collect-next-diagnostics.ps1 -CpuSampleSeconds 10
```

This creates:

```text
diagnostics/next-<timestamp>/
diagnostics/next-<timestamp>.zip
```

The zip contains:

- `system.json` with CPU, OS, GPU, git/build context;
- `processes-top-cumulative.csv`;
- `vm-cpu-sample.csv`;
- `vmware.json`;
- EvertyDesk Next logs from `%LOCALAPPDATA%\EvertyDesk\`.

## CPU-only sample

```powershell
.\scripts\diagnose-vm-cpu.ps1 -Seconds 10
```

Watch these columns:

- `CpuPercent`: measured over the sample window, normalized by logical CPU count;
- `ProcessName`;
- `Path`.

If `evertydesk-rdp-viewer` is high while `vmware-vmx` is low, inspect the VM
viewer. If `vmware-vmx` is high while EvertyDesk is low, the load is in VMware
or the guest workload.

## Interpretation

Evidence that points at EvertyDesk:

- `evertydesk-launcher`, `evertydesk-viewer`, or `evertydesk-rdp-viewer` has
  sustained CPU in the sample;
- CPU drops immediately when the EvertyDesk VM viewer closes;
- `desktop-next.log` shows repeated reconnect/render errors.

Evidence that points at VMware:

- `vmware-vmx` has sustained CPU while EvertyDesk is near zero;
- `vmrun` appears repeatedly or hangs;
- CPU remains after EvertyDesk closes.

Evidence that points at the guest:

- `vmware-vmx` is high only when the VM is doing visible work;
- the guest OS task manager also shows load;
- CPU falls when the guest is idle/suspended.

## Notes

The full VMware WebMKS console path is not implemented yet. Current VMware
support in the repo is inventory/capability skeleton plus `vmrun list` discovery
in the archived Lite VM list. So a report that says "VMware console" usually
means either:

- external VMware Workstation/Player console;
- `vmware-vmx` load from the guest;
- Lite dashboard/inventory discovery;
- an RDP session to a VMware guest, not WebMKS.
