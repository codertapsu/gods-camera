using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>The scanner. On Windows it is not built yet; the screen says so.</summary>
internal sealed class ScannerScreen : UserControl, IScreen
{
    public ScannerScreen()
    {
        var column = new StackPanel { Spacing = 16, Padding = new Thickness(24), MaxWidth = 720, HorizontalAlignment = HorizontalAlignment.Left };
        column.Children.Add(new ScreenHeader(T("Scanner"), T("Photograph pages with your phone or tablet. The text arrives here.")));
        column.Children.Add(new Card(
            T("Not in this version yet"), Ui.IconInfo,
            new Hint(T("The scanner works in the Mac app today. The Windows app gets it once reading text from pictures is built for Windows."))));
        Content = column;
    }

    public void Update(AppState state)
    {
    }
}

/// <summary>The camera's state, the data the app keeps, and where things are.</summary>
internal sealed class SettingsScreen : UserControl, IScreen
{
    private readonly Shell shell;
    private readonly TextBlock cameraState = new() { TextWrapping = TextWrapping.Wrap };

    public SettingsScreen(Shell shell)
    {
        this.shell = shell;
        var column = new StackPanel { Spacing = 16, Padding = new Thickness(24), MaxWidth = 720, HorizontalAlignment = HorizontalAlignment.Left };
        column.Children.Add(new ScreenHeader(T("Settings"), T("The camera, your data and help.")));

        column.Children.Add(new Card(
            T("The camera"), Ui.IconCamera,
            Labelled(T("State"), cameraState),
            Labelled(T("Version"), new TextBlock { Text = shell.Version })));

        var delete = Ui.PlainButton(T("Delete all my data…"), DeleteEverything);
        column.Children.Add(new Card(
            T("Your data"), Ui.IconLock,
            new Hint(T("Saved faces and loops stay on this PC. Saved faces are encrypted, with the key in Windows Credential Manager.")),
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
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(120) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.Children.Add(Ui.Secondary(label, 13));
        Grid.SetColumn(value, 1);
        grid.Children.Add(value);
        return grid;
    }

    private async void DeleteEverything()
    {
        var words = Ui.Text(T("Saved faces, their encryption key, saved loops and what the app remembered are removed from this PC. This cannot be undone."));
        words.MaxWidth = 420;
        if (await shell.Ask(T("Delete every saved face and loop?"), words, T("Delete everything"), T("Cancel")))
        {
            shell.App.DeleteAllData();
        }
    }

    public void Update(AppState state)
    {
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
