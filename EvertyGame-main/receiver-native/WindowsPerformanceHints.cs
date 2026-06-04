using System.Diagnostics;
using System.Runtime;
using System.Runtime.InteropServices;

namespace ReceiverNative;

internal sealed class WindowsPerformanceHints : IDisposable
{
    private ProcessPriorityClass? _originalPriorityClass;
    private bool? _originalPriorityBoostEnabled;
    private GCLatencyMode _originalGcLatencyMode = GCLatencyMode.Interactive;
    private bool _timerResolutionRaised;
    private bool _enabled;

    public void Enable()
    {
        if (_enabled)
        {
            return;
        }

        _originalGcLatencyMode = GCSettings.LatencyMode;
        try
        {
            GCSettings.LatencyMode = GCLatencyMode.SustainedLowLatency;
        }
        catch
        {
        }

        try
        {
            var process = Process.GetCurrentProcess();
            _originalPriorityClass = process.PriorityClass;
            if (process.PriorityClass < ProcessPriorityClass.High)
            {
                process.PriorityClass = ProcessPriorityClass.High;
            }
        }
        catch
        {
        }

        try
        {
            var process = Process.GetCurrentProcess();
            _originalPriorityBoostEnabled = process.PriorityBoostEnabled;
            process.PriorityBoostEnabled = true;
        }
        catch
        {
        }

        try
        {
            _timerResolutionRaised = timeBeginPeriod(1) == 0;
        }
        catch
        {
            _timerResolutionRaised = false;
        }

        _enabled = true;
    }

    public void Disable()
    {
        if (!_enabled)
        {
            return;
        }

        if (_timerResolutionRaised)
        {
            try
            {
                timeEndPeriod(1);
            }
            catch
            {
            }
            _timerResolutionRaised = false;
        }

        if (_originalPriorityBoostEnabled is not null)
        {
            try
            {
                Process.GetCurrentProcess().PriorityBoostEnabled = _originalPriorityBoostEnabled.Value;
            }
            catch
            {
            }
        }

        if (_originalPriorityClass is not null)
        {
            try
            {
                Process.GetCurrentProcess().PriorityClass = _originalPriorityClass.Value;
            }
            catch
            {
            }
        }

        try
        {
            GCSettings.LatencyMode = _originalGcLatencyMode;
        }
        catch
        {
        }

        _enabled = false;
    }

    public void Dispose()
    {
        Disable();
    }

    [DllImport("winmm.dll", EntryPoint = "timeBeginPeriod")]
    private static extern uint timeBeginPeriod(uint period);

    [DllImport("winmm.dll", EntryPoint = "timeEndPeriod")]
    private static extern uint timeEndPeriod(uint period);
}
