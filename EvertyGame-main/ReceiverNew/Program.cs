namespace ReceiverNative;

internal static class Program
{
    [STAThread]
    private static void Main()
    {
        ReceiverTrace.Initialize();
        Application.ThreadException += (_, args) => ReceiverTrace.Log(args.Exception, "ReceiverNew UI thread exception");
        AppDomain.CurrentDomain.UnhandledException += (_, args) =>
        {
            if (args.ExceptionObject is Exception exception)
            {
                ReceiverTrace.Log(exception, "ReceiverNew unhandled exception");
            }
            else
            {
                ReceiverTrace.Log($"ReceiverNew unhandled non-exception object: {args.ExceptionObject}");
            }
        };
        TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            ReceiverTrace.Log(args.Exception, "ReceiverNew unobserved task exception");
            args.SetObserved();
        };

        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        Application.Run(new ReceiverNewForm());
    }
}
