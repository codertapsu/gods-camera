using Microsoft.UI.Xaml;

namespace GodsCamera;

public partial class App : Application
{
    private Window? window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // `--core-check <file>`: write the core's version to the file and leave. A check that the
        // program can reach the shared core, without a window.
        var arguments = Environment.GetCommandLineArgs();
        var check = Array.IndexOf(arguments, "--core-check");
        if (check >= 0 && check + 1 < arguments.Length)
        {
            File.WriteAllText(arguments[check + 1], uniffi.gc_ffi.GcFfiMethods.CoreVersion());
            Exit();
            return;
        }
        window = new MainWindow();
        window.Activate();
    }
}
