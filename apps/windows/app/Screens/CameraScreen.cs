using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>A screen that shows the app's state and is told when it changes.</summary>
internal interface IScreen
{
    void Update(AppState state);
}

/// <summary>
/// The camera as other apps see it: whether they can, whether one does, and
/// where the picture comes from.
/// </summary>
internal sealed class CameraScreen : UserControl, IScreen
{
    private static readonly SourceKind[] Sources = { SourceKind.Camera, SourceKind.Image, SourceKind.Video, SourceKind.Pattern };

    private readonly Shell shell;
    private readonly PreviewPane preview;
    private readonly SelectorBar sourceBar = new();
    private readonly Button choose;
    private readonly TextBlock fileName = Ui.Secondary("", 13);
    private readonly Hint sourceHint = new();
    private readonly SwitchRow mirror;
    private readonly Hint meeting = new();
    private readonly Hint meetingMore = new();
    private readonly Button openSettings;
    private readonly Hint readers = new();
    private bool showing;

    public CameraScreen(Shell shell)
    {
        this.shell = shell;
        preview = new PreviewPane(shell.Preview);
        var layout = new StudioLayout(T("Camera"), T("What other apps see when they choose God's Camera."), preview);

        foreach (var name in new[] { T("Camera"), T("Image"), T("Video"), T("Test pattern") })
        {
            sourceBar.Items.Add(new SelectorBarItem { Text = name });
        }
        sourceBar.SelectionChanged += (_, _) =>
        {
            if (showing) return;
            var index = sourceBar.Items.IndexOf(sourceBar.SelectedItem);
            if (index >= 0) shell.App.SetSource(Sources[index], null);
        };
        choose = Ui.PlainButton(T("Choose…"), ChooseFile);
        mirror = new SwitchRow(T("Mirror the picture"), on => shell.App.SetMirror(on));
        var chosen = Ui.Row(10, choose, fileName);
        layout.Under.Children.Add(sourceBar);
        mirror.Width = 220;
        layout.Under.Children.Add(Ui.Spread(Ui.Column(8, chosen, sourceHint), mirror));

        openSettings = Ui.PlainButton(T("Open Windows Settings"), () =>
            _ = Windows.System.Launcher.LaunchUriAsync(new Uri("ms-settings:privacy-webcam")));
        layout.Controls.Children.Add(new Card(T("In your meeting app"), Ui.IconPeople, meeting, meetingMore, openSettings));
        layout.Controls.Children.Add(new Card(T("Apps using the camera"), Ui.IconApps, readers));
        Content = layout;
    }

    private async void ChooseFile()
    {
        var files = await shell.PickFiles(false, ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff");
        if (files.Count > 0) shell.App.SetSource(SourceKind.Image, files[0]);
    }

    public void Update(AppState state)
    {
        preview.Update(state);

        showing = true;
        var index = Array.IndexOf(Sources, state.Source);
        if (sourceBar.Items.IndexOf(sourceBar.SelectedItem) != index) sourceBar.SelectedItem = sourceBar.Items[index];
        showing = false;
        mirror.Show(state.Mirror);

        var wantsFile = state.Source == SourceKind.Image;
        choose.Visible(wantsFile);
        fileName.Visible(wantsFile);
        fileName.Text = state.FileName ?? T("No file chosen");
        switch (state.Source)
        {
            case SourceKind.Pattern:
                sourceHint.Set(T("Colour bars with a moving block, for checking the camera in other apps."));
                break;
            case SourceKind.Image when state.SourceProblem == SourceProblem.FileUnreadable:
                sourceHint.Set(T("That file cannot be shown as a picture."), Ui.IconWarning, Tone.Warning);
                break;
            case SourceKind.Image:
                sourceHint.Set(T("A still picture, shown as your camera."));
                break;
            case SourceKind.Video:
                sourceHint.Set(T("Showing a video file is not in this version yet."), Ui.IconInfo);
                break;
            default:
                sourceHint.Set(T("Using a camera is not in this version yet. Choose Image or Test pattern."), Ui.IconInfo);
                break;
        }

        openSettings.Visible(state.VirtualCamera == VirtualCamera.Refused);
        meetingMore.Visible(state.VirtualCamera is VirtualCamera.Ready or VirtualCamera.NotInstalled or VirtualCamera.Failed);
        switch (state.VirtualCamera)
        {
            case VirtualCamera.Ready:
                meeting.Set(T("Ready. In Zoom, Teams, Meet or any other app, choose “God's Camera” as the camera."), Ui.IconGood, Tone.Good);
                meetingMore.Set(T("An app looks for cameras when it starts. If one does not list God's Camera, quit that app completely and open it again."));
                break;
            case VirtualCamera.NotInstalled:
                meeting.Set(T("Other apps cannot see God's Camera until it is installed."), Ui.IconWarning, Tone.Warning);
                meetingMore.Set(T("The camera is installed together with the app, by its installer. This copy was started without it."));
                break;
            case VirtualCamera.Refused:
                meeting.Set(T("Windows does not let desktop apps use cameras. Turn that on under Privacy & security, Camera."), Ui.IconWarning, Tone.Warning);
                break;
            case VirtualCamera.Busy:
                meeting.Set(T("Another copy of God's Camera is running and holds the camera. Close it, then start this one again."), Ui.IconWarning, Tone.Warning);
                break;
            case VirtualCamera.Failed:
                meeting.Set(T("Windows could not make the camera."), Ui.IconError, Tone.Bad);
                meetingMore.Set(state.VirtualCameraDetail);
                break;
            default:
                meeting.Set(T("Other apps cannot see God's Camera until it is installed."));
                break;
        }

        if (state.Watched)
        {
            readers.Set(T("An app is using the camera, at {0} × {1}.", state.OutputWidth, state.OutputHeight), Ui.IconGood, Tone.Good);
        }
        else
        {
            readers.Set(T("No app is using the camera right now."));
        }
    }
}
