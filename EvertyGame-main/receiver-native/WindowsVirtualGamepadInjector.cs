using Nefarius.ViGEm.Client;
using Nefarius.ViGEm.Client.Targets;
using Nefarius.ViGEm.Client.Targets.DualShock4;

namespace ReceiverNative;

internal sealed class WindowsVirtualGamepadInjector : IDisposable
{
    private const ushort DpadUp = 0x0001;
    private const ushort DpadDown = 0x0002;
    private const ushort DpadLeft = 0x0004;
    private const ushort DpadRight = 0x0008;
    private const ushort Start = 0x0010;
    private const ushort Back = 0x0020;
    private const ushort LeftThumb = 0x0040;
    private const ushort RightThumb = 0x0080;
    private const ushort LeftShoulder = 0x0100;
    private const ushort RightShoulder = 0x0200;
    private const ushort A = 0x1000;
    private const ushort B = 0x2000;
    private const ushort X = 0x4000;
    private const ushort Y = 0x8000;
    private const ushort Ds4Square = 0x0010;
    private const ushort Ds4Cross = 0x0020;
    private const ushort Ds4Circle = 0x0040;
    private const ushort Ds4Triangle = 0x0080;
    private const ushort Ds4ShoulderLeft = 0x0100;
    private const ushort Ds4ShoulderRight = 0x0200;
    private const ushort Ds4TriggerLeft = 0x0400;
    private const ushort Ds4TriggerRight = 0x0800;
    private const ushort Ds4Share = 0x1000;
    private const ushort Ds4Options = 0x2000;
    private const ushort Ds4ThumbLeft = 0x4000;
    private const ushort Ds4ThumbRight = 0x8000;

    private sealed class ControllerSlot
    {
        public IDualShock4Controller Controller { get; init; } = default!;
        public long LastSequence { get; set; }
    }

    private readonly object _sync = new();
    private ViGEmClient? _client;
    private readonly Dictionary<int, ControllerSlot> _controllers = new();

    public string Status { get; private set; } = "Unavailable";
    public string LastInputSummary { get; private set; } = "-";

    public bool TryApply(RemoteGamepadStateMessage state)
    {
        lock (_sync)
        {
            if (!EnsureClientLocked())
            {
                return false;
            }

            var controllerId = Math.Clamp(state.ControllerId, 0, 3);
            var slot = GetOrCreateControllerLocked(controllerId);
            if (slot is null)
            {
                return false;
            }

            if (state.Seq <= slot.LastSequence)
            {
                return false;
            }

            ApplyButtons(slot.Controller, state.Buttons);
            slot.Controller.SetSliderValue(DualShock4Slider.LeftTrigger, state.LeftTrigger);
            slot.Controller.SetSliderValue(DualShock4Slider.RightTrigger, state.RightTrigger);
            slot.Controller.SetAxisValue(DualShock4Axis.LeftThumbX, ToDualShockAxis(state.LeftThumbX));
            slot.Controller.SetAxisValue(DualShock4Axis.LeftThumbY, ToDualShockAxis(state.LeftThumbY));
              slot.Controller.SetAxisValue(DualShock4Axis.RightThumbX, ToDualShockAxis(state.RightThumbX));
              slot.Controller.SetAxisValue(DualShock4Axis.RightThumbY, ToDualShockAxis(state.RightThumbY));

              slot.LastSequence = state.Seq;
              Status = _controllers.Count == 1 ? "DualShock 4 virtual pad" : $"{_controllers.Count} DualShock 4 virtual pads";
              var summary = DescribeState(state);
              if (HasMeaningfulInput(state))
              {
                  LastInputSummary = summary;
              }
            return true;
        }
    }

    public void ReleaseAll()
    {
        lock (_sync)
        {
            foreach (var slot in _controllers.Values)
            {
                ApplyButtons(slot.Controller, 0);
                slot.Controller.SetSliderValue(DualShock4Slider.LeftTrigger, 0);
                slot.Controller.SetSliderValue(DualShock4Slider.RightTrigger, 0);
                slot.Controller.SetAxisValue(DualShock4Axis.LeftThumbX, 128);
                slot.Controller.SetAxisValue(DualShock4Axis.LeftThumbY, 128);
                slot.Controller.SetAxisValue(DualShock4Axis.RightThumbX, 128);
                slot.Controller.SetAxisValue(DualShock4Axis.RightThumbY, 128);
            }
            LastInputSummary = "-";
        }
    }

    public void ResetSession()
    {
        lock (_sync)
        {
            ReleaseAll();
            foreach (var slot in _controllers.Values)
            {
                slot.LastSequence = 0;
            }
        }
    }

    private void ApplyButtons(IDualShock4Controller controller, ushort buttons)
    {
        ushort ds4Buttons = 0;
        if ((buttons & A) != 0) ds4Buttons |= Ds4Cross;
        if ((buttons & B) != 0) ds4Buttons |= Ds4Circle;
        if ((buttons & X) != 0) ds4Buttons |= Ds4Square;
        if ((buttons & Y) != 0) ds4Buttons |= Ds4Triangle;
        if ((buttons & LeftShoulder) != 0) ds4Buttons |= Ds4ShoulderLeft;
        if ((buttons & RightShoulder) != 0) ds4Buttons |= Ds4ShoulderRight;
        if ((buttons & LeftThumb) != 0) ds4Buttons |= Ds4ThumbLeft;
        if ((buttons & RightThumb) != 0) ds4Buttons |= Ds4ThumbRight;
        if ((buttons & Start) != 0) ds4Buttons |= Ds4Options;
        if ((buttons & Back) != 0) ds4Buttons |= Ds4Share;

        controller.SetButtonsFull(ds4Buttons);
        controller.SetSpecialButtonsFull(0);
        controller.SetDPadDirection(ToDualShockDpad(buttons));
    }

    private static string DescribeState(RemoteGamepadStateMessage state)
    {
        var parts = new List<string>(8);
        if ((state.Buttons & A) != 0) parts.Add("Cross");
        if ((state.Buttons & B) != 0) parts.Add("Circle");
        if ((state.Buttons & X) != 0) parts.Add("Square");
        if ((state.Buttons & Y) != 0) parts.Add("Triangle");
        if ((state.Buttons & LeftShoulder) != 0) parts.Add("L1");
        if ((state.Buttons & RightShoulder) != 0) parts.Add("R1");
        if ((state.Buttons & LeftThumb) != 0) parts.Add("L3");
        if ((state.Buttons & RightThumb) != 0) parts.Add("R3");
        if ((state.Buttons & Start) != 0) parts.Add("Options");
        if ((state.Buttons & Back) != 0) parts.Add("Share");
        if ((state.Buttons & DpadUp) != 0) parts.Add("Up");
        if ((state.Buttons & DpadDown) != 0) parts.Add("Down");
        if ((state.Buttons & DpadLeft) != 0) parts.Add("Left");
        if ((state.Buttons & DpadRight) != 0) parts.Add("Right");

        var buttonsSummary = parts.Count > 0 ? string.Join("+", parts) : "none";
        var axisParts = new List<string>(4);
        if (Math.Abs(state.LeftThumbX) >= 4096 || Math.Abs(state.LeftThumbY) >= 4096)
        {
            axisParts.Add($"LS({state.LeftThumbX},{state.LeftThumbY})");
        }

        if (Math.Abs(state.RightThumbX) >= 4096 || Math.Abs(state.RightThumbY) >= 4096)
        {
            axisParts.Add($"RS({state.RightThumbX},{state.RightThumbY})");
        }

        if (state.LeftTrigger >= 8 || state.RightTrigger >= 8)
        {
            axisParts.Add($"T({state.LeftTrigger},{state.RightTrigger})");
        }

        var axisSummary = axisParts.Count > 0 ? $" {string.Join(" ", axisParts)}" : string.Empty;
        return $"pad{state.ControllerId + 1} 0x{state.Buttons:X4} {buttonsSummary}{axisSummary}";
    }

    private static bool HasMeaningfulInput(RemoteGamepadStateMessage state)
    {
        return state.Buttons != 0 ||
            state.LeftTrigger >= 8 ||
            state.RightTrigger >= 8 ||
            Math.Abs(state.LeftThumbX) >= 4096 ||
            Math.Abs(state.LeftThumbY) >= 4096 ||
            Math.Abs(state.RightThumbX) >= 4096 ||
            Math.Abs(state.RightThumbY) >= 4096;
    }

    private static DualShock4DPadDirection ToDualShockDpad(ushort buttons)
    {
        var up = (buttons & DpadUp) != 0;
        var down = (buttons & DpadDown) != 0;
        var left = (buttons & DpadLeft) != 0;
        var right = (buttons & DpadRight) != 0;

        if (up && right) return DualShock4DPadDirection.Northeast;
        if (up && left) return DualShock4DPadDirection.Northwest;
        if (down && right) return DualShock4DPadDirection.Southeast;
        if (down && left) return DualShock4DPadDirection.Southwest;
        if (up) return DualShock4DPadDirection.North;
        if (down) return DualShock4DPadDirection.South;
        if (left) return DualShock4DPadDirection.West;
        if (right) return DualShock4DPadDirection.East;
        return DualShock4DPadDirection.None;
    }

    private static byte ToDualShockAxis(short value)
    {
        var normalized = (value - short.MinValue) / (double)ushort.MaxValue;
        return (byte)Math.Clamp((int)Math.Round(normalized * 255.0), 0, 255);
    }

    private bool EnsureClientLocked()
    {
        if (_client is not null)
        {
            return true;
        }

        try
        {
            _client = new ViGEmClient();
            Status = "DualShock 4 virtual pad";
            LastInputSummary = "-";
            return true;
        }
        catch (Exception ex)
        {
            Status = $"ViGEm unavailable: {ex.Message}";
            LastInputSummary = "-";
            _client = null;
            return false;
        }
    }

    private ControllerSlot? GetOrCreateControllerLocked(int controllerId)
    {
        if (_controllers.TryGetValue(controllerId, out var existing))
        {
            return existing;
        }

        try
        {
            var controller = _client!.CreateDualShock4Controller();
            controller.AutoSubmitReport = true;
            controller.Connect();
            var slot = new ControllerSlot { Controller = controller };
            _controllers[controllerId] = slot;
            return slot;
        }
        catch (Exception ex)
        {
            Status = $"ViGEm unavailable: {ex.Message}";
            return null;
        }
    }

    public void Dispose()
    {
        lock (_sync)
        {
            foreach (var slot in _controllers.Values)
            {
                try
                {
                    ApplyButtons(slot.Controller, 0);
                    slot.Controller.Disconnect();
                }
                catch
                {
                }
            }

            _controllers.Clear();
            _client?.Dispose();
            _client = null;
            Status = "Unavailable";
            LastInputSummary = "-";
        }
    }
}
