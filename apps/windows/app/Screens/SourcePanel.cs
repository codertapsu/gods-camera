using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>
/// The Camera tab of the Camera screen: where the picture comes from, in
/// detail, and the camera as other apps see it: whether they can, and whether
/// one does.
/// </summary>
internal sealed class SourcePanel : StackPanel
{
    private static readonly SourceKind[] Sources = { SourceKind.Camera, SourceKind.Image, SourceKind.Video, SourceKind.Pattern };

    private readonly Shell shell;
    private readonly SelectorBar sourceBar = new();
    private static readonly string[] Pictures = { ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff" };
    private static readonly string[] Videos = { ".mp4", ".m4v", ".mov", ".wmv", ".avi", ".mkv" };

    private readonly ComboBox cameraBox = new();
    private readonly Button allow;
    private readonly Button choose;
    private readonly TextBlock fileName = Ui.Secondary("", 13);
    private string cameraKey = "";
    private CameraChoice[] cameraList = Array.Empty<CameraChoice>();
    private SourceKind shownSource;
    private int ticks;
    private readonly Hint sourceHint = new();
    private readonly SwitchRow mirror;
    private readonly Hint meeting = new();
    private readonly Hint meetingMore = new();
    private readonly Button openSettings;
    private readonly Button install;
    private readonly Hint readers = new();
    private bool showing;

    public SourcePanel(Shell shell)
    {
        this.shell = shell;
        Spacing = 14;

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
        allow = Ui.PlainButton(T("Allow camera access"), () => shell.App.AllowCamera());
        allow.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        cameraBox.SelectionChanged += (_, _) =>
        {
            if (showing) return;
            var index = cameraBox.SelectedIndex;
            if (index >= 0 && index < cameraList.Length) shell.App.SetCamera(cameraList[index].Id);
        };
        mirror = new SwitchRow(T("Mirror the picture"), on => shell.App.SetMirror(on));
        cameraBox.HorizontalAlignment = HorizontalAlignment.Stretch;
        Children.Add(new Card(
            T("Where the picture comes from"), Ui.IconCamera,
            sourceBar, allow, cameraBox, Ui.Row(10, choose, fileName), sourceHint, mirror));

        openSettings = Ui.PlainButton(T("Open Windows Settings"), () =>
            _ = Windows.System.Launcher.LaunchUriAsync(new Uri("ms-settings:privacy-webcam")));
        install = Ui.PlainButton(T("Install the camera"), async () =>
        {
            install!.IsEnabled = false;
            await shell.InstallCamera(false);
            install.IsEnabled = true;
        });
        install.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        Children.Add(new Card(T("In your meeting app"), Ui.IconPeople, meeting, install, meetingMore, openSettings));
        Children.Add(new Card(T("Apps using the camera"), Ui.IconApps, readers));
    }

    private async void ChooseFile()
    {
        var video = shownSource == SourceKind.Video;
        var files = await shell.PickFiles(false, video ? Videos : Pictures);
        if (files.Count > 0) shell.App.SetSource(video ? SourceKind.Video : SourceKind.Image, files[0]);
    }

    public void Update(AppState state)
    {
        showing = true;
        var index = Array.IndexOf(Sources, state.Source);
        if (sourceBar.Items.IndexOf(sourceBar.SelectedItem) != index) sourceBar.SelectedItem = sourceBar.Items[index];
        showing = false;
        mirror.Show(state.Mirror);

        shownSource = state.Source;
        var wantsFile = state.Source is SourceKind.Image or SourceKind.Video;
        choose.Visible(wantsFile);
        fileName.Visible(wantsFile);
        fileName.Text = state.FileName ?? T("No file chosen");

        // The camera is turned on only after the user has said it may be.
        var camera = state.Source == SourceKind.Camera;
        var notAllowed = camera && state.SourceProblem == SourceProblem.CameraNotAllowed;
        allow.Visible(notAllowed);
        cameraBox.Visible(camera && !notAllowed && state.Cameras.Length > 0);
        var key = string.Join("|", state.Cameras.Select(one => one.Id + ":" + one.Name));
        showing = true;
        if (key != cameraKey)
        {
            cameraKey = key;
            cameraList = state.Cameras;
            cameraBox.Items.Clear();
            foreach (var one in cameraList) cameraBox.Items.Add(one.Name);
        }
        var chosen = Array.FindIndex(cameraList, one => one.Id == state.Camera);
        if (cameraBox.SelectedIndex != chosen) cameraBox.SelectedIndex = chosen;
        showing = false;
        // A camera may be plugged in or pulled out while this screen is open.
        if (camera && ++ticks % 30 == 0) shell.App.RefreshCameras();

        switch (state.Source, state.SourceProblem)
        {
            case (SourceKind.Pattern, _):
                sourceHint.Set(T("Colour bars with a moving block, for checking the camera in other apps."));
                break;
            case (_, SourceProblem.FileUnreadable):
                sourceHint.Set(T("That file cannot be shown."), Ui.IconWarning, Tone.Warning);
                break;
            case (SourceKind.Image, _):
                sourceHint.Set(T("A still picture, shown as your camera."));
                break;
            case (SourceKind.Video, _):
                sourceHint.Set(T("A video file, played again and again as your camera."));
                break;
            case (_, SourceProblem.CameraNotAllowed):
                sourceHint.Set(T("God's Camera needs your permission to use the camera."));
                break;
            case (_, SourceProblem.NoCamera):
                sourceHint.Set(T("No camera is attached to this PC."), Ui.IconWarning, Tone.Warning);
                break;
            case (_, SourceProblem.CameraRefused):
                sourceHint.Set(T("Windows does not let desktop apps use cameras. Turn that on under Privacy & security, Camera."), Ui.IconWarning, Tone.Warning);
                break;
            case (_, SourceProblem.CameraUnavailable):
                sourceHint.Set(T("The camera cannot be used. Another app may have it, or it was unplugged."), Ui.IconWarning, Tone.Warning);
                break;
            default:
                sourceHint.Set(T("Your own camera, with whatever this app adds to it."));
                break;
        }

        openSettings.Visible(state.VirtualCamera == VirtualCamera.Refused);
        install.Visible(state.VirtualCamera == VirtualCamera.NotInstalled && shell.CarriesCamera);
        meetingMore.Visible(state.VirtualCamera is VirtualCamera.Ready or VirtualCamera.NotInstalled or VirtualCamera.Failed);
        switch (state.VirtualCamera)
        {
            case VirtualCamera.Ready:
                meeting.Set(T("Ready. In Zoom, Teams, Meet or any other app, choose “God's Camera” as the camera."), Ui.IconGood, Tone.Good);
                meetingMore.Set(T("An app looks for cameras when it starts. If one does not list God's Camera, quit that app completely and open it again."));
                break;
            case VirtualCamera.NotInstalled:
                meeting.Set(T("Other apps cannot see God's Camera until it is installed."), Ui.IconWarning, Tone.Warning);
                meetingMore.Set(shell.CarriesCamera
                    ? T("Windows asks for an administrator's permission. One file is copied to Program Files and registered as a camera.")
                    : T("This copy of the app does not carry the camera's file, so it cannot install it."));
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
