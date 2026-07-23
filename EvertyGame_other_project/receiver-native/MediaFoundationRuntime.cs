using SharpGen.Runtime;
using System.Threading;
using Vortice.MediaFoundation;

namespace ReceiverNative;

internal static class MediaFoundationRuntime
{
    private static int _refCount;

    public static void Acquire()
    {
        if (Interlocked.Increment(ref _refCount) == 1)
        {
            MediaFactory.MFStartup(true).CheckError();
        }
    }

    public static void Release()
    {
        if (Interlocked.Decrement(ref _refCount) == 0)
        {
            MediaFactory.MFShutdown().CheckError();
        }
    }
}
