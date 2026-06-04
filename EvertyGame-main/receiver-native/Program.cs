namespace ReceiverNative;

internal static class Program
{
    [STAThread]
    private static void Main(string[] args)
    {
        ReceiverTrace.Initialize();
        Application.ThreadException += (_, args) => ReceiverTrace.Log(args.Exception, "UI thread exception");
        AppDomain.CurrentDomain.UnhandledException += (_, args) =>
        {
            if (args.ExceptionObject is Exception exception)
            {
                ReceiverTrace.Log(exception, "Unhandled exception");
            }
            else
            {
                ReceiverTrace.Log($"Unhandled non-exception object: {args.ExceptionObject}");
            }
        };
        TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            ReceiverTrace.Log(args.Exception, "Unobserved task exception");
            args.SetObserved();
        };

        System.Windows.Forms.Application.EnableVisualStyles();
        System.Windows.Forms.Application.SetCompatibleTextRenderingDefault(false);

        if (TryCreateLaunchOptions(args, out var launchOptions))
        {
            System.Windows.Forms.Application.Run(new MainForm(launchOptions));
            return;
        }

        if (Environment.GetEnvironmentVariable("EVERTY_RECEIVER_NATIVE_LEGACY_UI") == "1")
        {
            System.Windows.Forms.Application.Run(new MainForm());
            return;
        }

        var app = new System.Windows.Application
        {
            ShutdownMode = System.Windows.ShutdownMode.OnMainWindowClose,
        };
        app.Run(new CommercialShellWindow());
    }

    private static bool TryCreateLaunchOptions(string[] args, out MainFormLaunchOptions launchOptions)
    {
        var roleText = Environment.GetEnvironmentVariable("EVERTY_RECEIVER_ROLE");
        var controlPlaneUrl = Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_URL");
        var advancedMode = string.Equals(Environment.GetEnvironmentVariable("EVERTY_ADVANCED_MODE"), "1", StringComparison.Ordinal);
        var lockRoleSelection = string.Equals(Environment.GetEnvironmentVariable("EVERTY_LOCK_ROLE"), "1", StringComparison.Ordinal);

        for (var index = 0; index < args.Length; index++)
        {
            var arg = args[index];
            if (string.Equals(arg, "--role", StringComparison.OrdinalIgnoreCase) && index + 1 < args.Length)
            {
                roleText = args[++index];
                continue;
            }

            if (string.Equals(arg, "--control-plane-url", StringComparison.OrdinalIgnoreCase) && index + 1 < args.Length)
            {
                controlPlaneUrl = args[++index];
                continue;
            }

            if (string.Equals(arg, "--advanced", StringComparison.OrdinalIgnoreCase))
            {
                advancedMode = true;
                continue;
            }

            if (string.Equals(arg, "--lock-role", StringComparison.OrdinalIgnoreCase))
            {
                lockRoleSelection = true;
            }
        }

        if (!TryParseRole(roleText, out var role))
        {
            launchOptions = new MainFormLaunchOptions(AppRole.Send);
            return false;
        }

        launchOptions = new MainFormLaunchOptions(
            InitialRole: role,
            ControlPlaneUrl: string.IsNullOrWhiteSpace(controlPlaneUrl) ? null : controlPlaneUrl.Trim(),
            AdvancedMode: advancedMode,
            LockRoleSelection: lockRoleSelection);
        return true;
    }

    private static bool TryParseRole(string? value, out AppRole role)
    {
        role = AppRole.Send;
        if (string.IsNullOrWhiteSpace(value))
        {
            return false;
        }

        return value.Trim().ToLowerInvariant() switch
        {
            "host" or "send" or "sender" => TryAssignRole(AppRole.Send, out role),
            "client" or "receive" or "receiver" => TryAssignRole(AppRole.Receive, out role),
            _ => false,
        };
    }

    private static bool TryAssignRole(AppRole value, out AppRole role)
    {
        role = value;
        return true;
    }
}
