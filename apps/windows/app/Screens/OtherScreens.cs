using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>The camera's state, the data the app keeps, and where things are.</summary>
internal sealed class SettingsScreen : UserControl, IScreen
{
    private readonly Shell shell;
    private readonly TextBlock cameraState = new() { TextWrapping = TextWrapping.Wrap };
    private readonly Button remove;
    private readonly Hint removeHint = new();
    private readonly SwitchRow keepRunning;

    public SettingsScreen(Shell shell)
    {
        this.shell = shell;
        var column = new StackPanel { Spacing = 16, Padding = new Thickness(24), MaxWidth = 720, HorizontalAlignment = HorizontalAlignment.Left };
        column.Children.Add(new ScreenHeader(T("Settings"), T("The camera, your data and help.")));

        remove = Ui.PlainButton(T("Remove the camera…"), async () =>
        {
            remove!.IsEnabled = false;
            await shell.InstallCamera(true);
            remove.IsEnabled = true;
        });
        removeHint.Set(T("Other apps no longer list God's Camera. Windows asks for an administrator's permission."));
        column.Children.Add(new Card(
            T("The camera"), Ui.IconCamera,
            Labelled(T("State"), cameraState),
            Labelled(T("Version"), new TextBlock { Text = shell.Version }),
            remove, removeHint));

        keepRunning = new SwitchRow(T("Keep running when the window is closed"), on => shell.App.SetKeepRunning(on));
        column.Children.Add(new Card(
            T("When the window is closed"), Ui.IconApps,
            keepRunning,
            new Hint(T("The camera exists for other apps only while God's Camera runs. With this on, closing the window leaves an icon beside the clock; Quit in its menu ends the app. Your own camera is switched off while no app uses God's Camera."))));

        static TextBlock Keys(string keys, bool works) => new()
        {
            Text = works ? keys : keys + "  (" + T("taken by another program") + ")",
            FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Consolas"),
        };
        column.Children.Add(new Card(
            T("Help"), Ui.IconHelp,
            Labelled(T("Record or stop a loop"), Keys("Ctrl+Alt+Shift+R", shell.RecordKeyWorks || !shell.AskedForHotKeys)),
            Labelled(T("Play the loop or go back to live"), Keys("Ctrl+Alt+Shift+L", shell.LoopKeyWorks || !shell.AskedForHotKeys)),
            new Hint(T("These keys work while another app has the keyboard, for use during a call.")),
            Ui.PlainButton(T("Save a diagnostics report…"), SaveDiagnostics),
            new Hint(T("A diagnostics report holds the app's state and its log. It contains no pictures and no faces."))));

        var delete = Ui.PlainButton(T("Delete all my data…"), DeleteEverything);
        column.Children.Add(new Card(
            T("Your data"), Ui.IconLock,
            new Hint(T("Saved faces, loops and scanned documents stay on this PC. Saved faces are encrypted, with the key in Windows Credential Manager.")),
            delete));

        var open = Ui.PlainButton(T("Open the data folder"), () => shell.OpenFolder(shell.DataFolder));
        column.Children.Add(new Card(
            T("For development"), Ui.IconWrench,
            Labelled(T("Data folder"), Path(shell.DataFolder)),
            Labelled(T("Model files"), Path(string.Join("\n", shell.PackFolders))),
            open));
        Content = new ScrollViewer { Content = column, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
    }

    private static TextBlock Path(string text) => new()
    {
        Text = text,
        FontSize = 12,
        TextWrapping = TextWrapping.Wrap,
        IsTextSelectionEnabled = true,
        Foreground = Ui.Brush("TextFillColorSecondaryBrush"),
    };

    private static Grid Labelled(string label, FrameworkElement value)
    {
        var grid = new Grid { ColumnSpacing = 12 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(230) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.Children.Add(Ui.Secondary(label, 13));
        Grid.SetColumn(value, 1);
        grid.Children.Add(value);
        return grid;
    }

    /// <summary>Write the diagnostics report where the user chooses, and show it in File Explorer.</summary>
    private async void SaveDiagnostics()
    {
        var picker = new Windows.Storage.Pickers.FileSavePicker
        {
            SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.Desktop,
            SuggestedFileName = $"GodsCamera-diagnostics-{DateTime.Now:yyyy-MM-dd-HHmm}",
        };
        picker.FileTypeChoices.Add(T("Text"), new List<string> { ".txt" });
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(shell));
        var file = await picker.PickSaveFileAsync();
        if (file == null) return;
        try
        {
            await File.WriteAllTextAsync(file.Path, Diagnostics.Report(shell));
            Shell.ShowInExplorer(file.Path);
        }
        catch (Exception problem)
        {
            Log.Problem("saving the diagnostics report", problem);
        }
    }

    private async void DeleteEverything()
    {
        var words = Ui.Text(T("Saved faces, their encryption key, saved loops, scanned documents and what the app remembered are removed from this PC. Phones and tablets that were connected must be connected again. This cannot be undone."));
        words.MaxWidth = 420;
        if (await shell.Ask(T("Delete everything the app has saved?"), words, T("Delete everything"), T("Cancel")))
        {
            shell.App.DeleteAllData();
            shell.Scanner.DeleteEverything();
        }
    }

    public void Update(AppState state)
    {
        keepRunning.Show(state.KeepRunning);
        var removable = state.VirtualCamera == VirtualCamera.Ready && shell.CarriesCamera;
        remove.Visible(removable);
        removeHint.Visible(removable);
        cameraState.Text = state.VirtualCamera switch
        {
            VirtualCamera.Ready => T("Installed"),
            VirtualCamera.NotInstalled => T("Not installed"),
            VirtualCamera.Refused => T("Windows does not let desktop apps use cameras. Turn that on under Privacy & security, Camera."),
            VirtualCamera.Busy => T("Another copy of God's Camera is running and holds the camera. Close it, then start this one again."),
            VirtualCamera.Failed => T("Windows could not make the camera.") + " " + state.VirtualCameraDetail,
            _ => T("Not installed"),
        };
    }
}
