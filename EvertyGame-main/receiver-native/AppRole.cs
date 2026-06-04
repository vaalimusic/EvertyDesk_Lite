namespace ReceiverNative;

internal enum AppRole
{
    Receive,
    Send,
}

internal static class AppRoleExtensions
{
    public static string ToUiLabel(this AppRole role)
    {
        return role switch
        {
            AppRole.Receive => "Receive",
            AppRole.Send => "Send",
            _ => role.ToString(),
        };
    }
}
