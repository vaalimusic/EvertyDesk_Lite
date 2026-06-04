using System.Runtime.InteropServices;

namespace ReceiverNative;

internal sealed class WindowsRemoteInputInjector
{
    private readonly HashSet<int> _pressedKeys = new();
    private readonly HashSet<RemoteMouseButtonKind> _pressedMouseButtons = new();
    private readonly object _sync = new();
    private long _lastSequence;

    public bool TryApply(RemoteInputControlMessage message, Rectangle monitorBounds)
    {
        lock (_sync)
        {
            if (message.Seq <= _lastSequence)
            {
                return false;
            }

            _lastSequence = message.Seq;
            ApplyMessageLocked(message, monitorBounds);
            return true;
        }
    }

    public void ReleaseAll()
    {
        lock (_sync)
        {
            ReleaseAllLocked();
        }
    }

    public void ResetSession()
    {
        lock (_sync)
        {
            ReleaseAllLocked();
            _lastSequence = 0;
        }
    }

    private void ApplyMessageLocked(RemoteInputControlMessage message, Rectangle monitorBounds)
    {
        switch (message)
        {
            case RemoteMouseMoveAbsolute absoluteMove:
                ApplyAbsoluteMouseMove(absoluteMove, monitorBounds);
                break;

            case RemoteMouseMoveRelative relativeMove:
                SendMouseMove(relativeMove.Dx, relativeMove.Dy);
                break;

            case RemoteMouseButtonMessage buttonMessage:
                SendMouseButton(buttonMessage.Button, buttonMessage.Pressed);
                if (buttonMessage.Pressed)
                {
                    _pressedMouseButtons.Add(buttonMessage.Button);
                }
                else
                {
                    _pressedMouseButtons.Remove(buttonMessage.Button);
                }
                break;

            case RemoteMouseWheelMessage wheelMessage:
                SendMouseWheel(wheelMessage.Delta);
                break;

            case RemoteKeyMessage keyMessage:
                SendKeyboard(keyMessage.VirtualKey, keyUp: !keyMessage.Pressed);
                if (keyMessage.Pressed)
                {
                    _pressedKeys.Add(keyMessage.VirtualKey);
                }
                else
                {
                    _pressedKeys.Remove(keyMessage.VirtualKey);
                }
                break;

            case RemoteReleaseAllMessage:
                ReleaseAll();
                break;
        }
    }

    private void ReleaseAllLocked()
    {
        foreach (var vkey in _pressedKeys.ToArray())
        {
            SendKeyboard(vkey, keyUp: true);
            _pressedKeys.Remove(vkey);
        }

        foreach (var button in _pressedMouseButtons.ToArray())
        {
            SendMouseButton(button, buttonDown: false);
            _pressedMouseButtons.Remove(button);
        }
    }

    private static void ApplyAbsoluteMouseMove(RemoteMouseMoveAbsolute move, Rectangle monitorBounds)
    {
        if (monitorBounds.Width <= 0 || monitorBounds.Height <= 0)
        {
            return;
        }

        var clampedX = Math.Clamp(move.X, 0.0, 1.0);
        var clampedY = Math.Clamp(move.Y, 0.0, 1.0);
        var targetX = monitorBounds.Left + (int)Math.Round(clampedX * Math.Max(0, monitorBounds.Width - 1));
        var targetY = monitorBounds.Top + (int)Math.Round(clampedY * Math.Max(0, monitorBounds.Height - 1));
        NativeMethods.SetCursorPos(targetX, targetY);
    }

    private static void SendMouseMove(int dx, int dy)
    {
        var input = new INPUT
        {
            type = NativeMethods.INPUT_MOUSE,
            U = new InputUnion
            {
                mi = new MOUSEINPUT
                {
                    dx = dx,
                    dy = dy,
                    mouseData = 0,
                    dwFlags = NativeMethods.MOUSEEVENTF_MOVE,
                    time = 0,
                    dwExtraInfo = IntPtr.Zero,
                },
            },
        };

        NativeMethods.SendInput(1, new[] { input }, Marshal.SizeOf<INPUT>());
    }

    private static void SendMouseWheel(int delta)
    {
        var input = new INPUT
        {
            type = NativeMethods.INPUT_MOUSE,
            U = new InputUnion
            {
                mi = new MOUSEINPUT
                {
                    dx = 0,
                    dy = 0,
                    mouseData = delta,
                    dwFlags = NativeMethods.MOUSEEVENTF_WHEEL,
                    time = 0,
                    dwExtraInfo = IntPtr.Zero,
                },
            },
        };

        NativeMethods.SendInput(1, new[] { input }, Marshal.SizeOf<INPUT>());
    }

    private static void SendMouseButton(RemoteMouseButtonKind button, bool buttonDown)
    {
        var flags = button switch
        {
            RemoteMouseButtonKind.Left => buttonDown ? NativeMethods.MOUSEEVENTF_LEFTDOWN : NativeMethods.MOUSEEVENTF_LEFTUP,
            RemoteMouseButtonKind.Right => buttonDown ? NativeMethods.MOUSEEVENTF_RIGHTDOWN : NativeMethods.MOUSEEVENTF_RIGHTUP,
            RemoteMouseButtonKind.Middle => buttonDown ? NativeMethods.MOUSEEVENTF_MIDDLEDOWN : NativeMethods.MOUSEEVENTF_MIDDLEUP,
            RemoteMouseButtonKind.X1 or RemoteMouseButtonKind.X2 => buttonDown ? NativeMethods.MOUSEEVENTF_XDOWN : NativeMethods.MOUSEEVENTF_XUP,
            _ => 0u,
        };

        var mouseData = button switch
        {
            RemoteMouseButtonKind.X1 => NativeMethods.XBUTTON1,
            RemoteMouseButtonKind.X2 => NativeMethods.XBUTTON2,
            _ => 0,
        };

        var input = new INPUT
        {
            type = NativeMethods.INPUT_MOUSE,
            U = new InputUnion
            {
                mi = new MOUSEINPUT
                {
                    dx = 0,
                    dy = 0,
                    mouseData = mouseData,
                    dwFlags = flags,
                    time = 0,
                    dwExtraInfo = IntPtr.Zero,
                },
            },
        };

        NativeMethods.SendInput(1, new[] { input }, Marshal.SizeOf<INPUT>());
    }

    private static void SendKeyboard(int virtualKey, bool keyUp)
    {
        if (virtualKey <= 0)
        {
            return;
        }

        var input = new INPUT
        {
            type = NativeMethods.INPUT_KEYBOARD,
            U = new InputUnion
            {
                ki = new KEYBDINPUT
                {
                    wVk = (ushort)virtualKey,
                    wScan = 0,
                    dwFlags = keyUp ? NativeMethods.KEYEVENTF_KEYUP : 0u,
                    time = 0,
                    dwExtraInfo = IntPtr.Zero,
                },
            },
        };

        NativeMethods.SendInput(1, new[] { input }, Marshal.SizeOf<INPUT>());
    }

    private static class NativeMethods
    {
        public const int INPUT_MOUSE = 0;
        public const int INPUT_KEYBOARD = 1;

        public const uint MOUSEEVENTF_MOVE = 0x0001;
        public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
        public const uint MOUSEEVENTF_LEFTUP = 0x0004;
        public const uint MOUSEEVENTF_RIGHTDOWN = 0x0008;
        public const uint MOUSEEVENTF_RIGHTUP = 0x0010;
        public const uint MOUSEEVENTF_MIDDLEDOWN = 0x0020;
        public const uint MOUSEEVENTF_MIDDLEUP = 0x0040;
        public const uint MOUSEEVENTF_WHEEL = 0x0800;
        public const uint MOUSEEVENTF_XDOWN = 0x0080;
        public const uint MOUSEEVENTF_XUP = 0x0100;

        public const uint KEYEVENTF_KEYUP = 0x0002;

        public const int XBUTTON1 = 0x0001;
        public const int XBUTTON2 = 0x0002;

        [DllImport("user32.dll", SetLastError = true)]
        public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool SetCursorPos(int x, int y);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct INPUT
    {
        public int type;
        public InputUnion U;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct InputUnion
    {
        [FieldOffset(0)]
        public MOUSEINPUT mi;

        [FieldOffset(0)]
        public KEYBDINPUT ki;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MOUSEINPUT
    {
        public int dx;
        public int dy;
        public int mouseData;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct KEYBDINPUT
    {
        public ushort wVk;
        public ushort wScan;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }
}
