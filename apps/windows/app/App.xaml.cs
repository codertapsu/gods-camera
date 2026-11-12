using Microsoft.UI.Xaml;

namespace GodsCamera;

/// <summary>
/// The app's start.
///
///   GodsCamera.exe [--data FOLDER] [--packs FOLDER]... [--language en|vi] [--theme light|dark] [--snapshots FOLDER [--demo PHOTO]]
///
///   --data       where settings, saved faces and saved loops are kept;
///                otherwise %LOCALAPPDATA%\GodsCamera
///   --packs      a folder with model files, looked in before the usual ones;
///                may be given more than once
///   --language   show the app in this language instead of the one Windows is shown in
///   --theme      show the app light or dark instead of as Windows is set
///   --status     write what the app is doing to this file, once a second, as JSON:
///                for development and for tests that cannot look at the window
///   --snapshots  save a picture of every screen into the folder as PNG, then close
///   --demo       with --snapshots: first record a loop and add this photo as a face
///
/// For development also `--core-check FILE`: write the shared core's version
/// to the file and leave, without a window.
/// </summary>
public partial class App : Application
{
    private Window? window;
    private Mutex? onlyCopy;

    public App()
    {
        InitializeComponent();
        // Light or dark has to be chosen before the first window is made.
        switch (Value(Environment.GetCommandLineArgs(), "--theme"))
        {
            case "light": RequestedTheme = ApplicationTheme.Light; break;
            case "dark": RequestedTheme = ApplicationTheme.Dark; break;
        }
    }

    private static string? Value(string[] arguments, string name)
    {
        var at = Array.IndexOf(arguments, name);
        return at >= 0 && at + 1 < arguments.Length ? arguments[at + 1] : null;
    }

    private static IEnumerable<string> Values(string[] arguments, string name)
    {
        for (var at = 0; at + 1 < arguments.Length; at++)
        {
            if (arguments[at] == name) yield return arguments[at + 1];
        }
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var arguments = Environment.GetCommandLineArgs();
        if (Value(arguments, "--core-check") is { } check)
        {
            File.WriteAllText(check, uniffi.gc_ffi.GcFfiMethods.CoreVersion());
            Exit();
            return;
        }

        var home = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "GodsCamera");
        var data = Path.GetFullPath(Value(arguments, "--data") ?? home);
        // One copy at a time on one data folder: a second start brings the
        // first copy's window forward and leaves. A copy on a data folder of
        // its own, as for pictures or a try-out, runs beside it and never
        // disturbs it. A run that only takes pictures is over in seconds and
        // is let through.
        var folderMark = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(
            System.Text.Encoding.UTF8.GetBytes(data.ToLowerInvariant())))[..16];
        if (Value(arguments, "--snapshots") == null)
        {
            onlyCopy = new Mutex(true, $"Local\\GodsCamera.OneCopy.{folderMark}", out var first);
            if (!first)
            {
                Native.PostMessageW(Native.HWND_BROADCAST, Shell.ShowYourselfMessage(folderMark), IntPtr.Zero, IntPtr.Zero);
                Exit();
                return;
            }
        }
        // Model files: the folders named on the command line, then the ones
        // that came with the app, then the ones added to it later.
        var packs = Values(arguments, "--packs")
            .Append(Path.Combine(AppContext.BaseDirectory, "Packs"))
            .Append(Path.Combine(home, "Packs"))
            .Select(Path.GetFullPath)
            .ToArray();
        Texts.Load(Value(arguments, "--language"));
        Log.Start(data);
        // Whatever goes wrong unnoticed is at least written down.
        UnhandledException += (_, problem) => Log.Problem("the window", problem.Exception);
        var launch = new Launch(
            data, packs, Path.Combine(data, "Cache", "Models"),
            Value(arguments, "--language"), Value(arguments, "--theme"), Value(arguments, "--snapshots"),
            Value(arguments, "--demo"), folderMark, Value(arguments, "--status"));
        try
        {
            window = new Shell(launch);
            window.Activate();
        }
        catch (Exception problem)
        {
            Log.Problem("starting", problem);
            throw;
        }
    }
}
