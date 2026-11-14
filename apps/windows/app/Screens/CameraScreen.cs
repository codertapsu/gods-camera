using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>A screen that shows the app's state and is told when it changes.</summary>
internal interface IScreen
{
    void Update(AppState state);
}

/// <summary>
/// The camera, the loop and the face swap on one screen, so that they are
/// used together (plan, section 6.12): the picture others see, the controls
/// used during a call under it, and everything else in three tabs at the right.
/// </summary>
internal sealed class CameraScreen : UserControl, IScreen
{
    private readonly Shell shell;
    private readonly PreviewPane preview;
    private readonly CallControls controls;
    private readonly Hint status = new();
    private readonly ProgressRing loading = new() { Width = 16, Height = 16, IsActive = true };
    private readonly Button allow;
    private readonly Button meeting;
    private readonly Microsoft.UI.Xaml.Shapes.Ellipse meetingDot = new() { Width = 8, Height = 8 };
    private readonly TextBlock meetingText = new() { FontSize = 13 };

    private readonly SelectorBar tabs = new();
    private readonly ScrollViewer tabArea = new() { VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
    private readonly Dictionary<string, UIElement> panels = new();
    private readonly FacePanel face;
    private readonly LoopPanel loop;
    private readonly SourcePanel source;

    public CameraScreen(Shell shell)
    {
        this.shell = shell;
        preview = new PreviewPane(shell.Preview);
        face = new FacePanel(shell);
        loop = new LoopPanel(shell);
        source = new SourcePanel(shell);
        controls = new CallControls(shell, face, this);
        preview.Children.Add(controls.OnPicture);

        // At the left: a short title, the picture, the controls under it and a line saying how it goes.
        meeting = new Button
        {
            Content = Ui.Row(7, meetingDot, meetingText),
            Background = Ui.Brush("SubtleFillColorSecondaryBrush"),
            BorderThickness = new Thickness(0),
            CornerRadius = new CornerRadius(14),
            Padding = new Thickness(11, 4, 12, 5),
        };
        ToolTipService.SetToolTip(meeting, T("In your meeting app"));
        meeting.Click += (_, _) => ShowTab("camera");
        var title = Ui.Column(2,
            new TextBlock { Text = T("Camera"), FontSize = 26, FontWeight = Microsoft.UI.Text.FontWeights.Bold },
            Ui.Secondary(T("What others see in your calls: your camera, a face in place of yours, or a loop."), 13));
        allow = Ui.PlainButton(T("Allow camera access"), () => shell.App.AllowCamera());
        allow.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        var statusRow = new Grid { ColumnSpacing = 10 };
        statusRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        statusRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        statusRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        loading.VerticalAlignment = VerticalAlignment.Center;
        status.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(status, 1);
        Grid.SetColumn(allow, 2);
        statusRow.Children.Add(loading);
        statusRow.Children.Add(status);
        statusRow.Children.Add(allow);

        var left = new StackPanel { Spacing = 14, Padding = new Thickness(24, 20, 24, 24) };
        left.Children.Add(Ui.Spread(title, meeting));
        left.Children.Add(preview);
        left.Children.Add(controls);
        left.Children.Add(statusRow);

        // At the right: the tabs.
        panels["face"] = face;
        panels["loop"] = loop;
        panels["camera"] = source;
        foreach (var (tag, words, glyph) in new[] { ("face", T("Face"), Ui.IconFace), ("loop", T("Loop"), Ui.IconLoop), ("camera", T("Camera"), Ui.IconCamera) })
        {
            tabs.Items.Add(new SelectorBarItem { Text = words, Tag = tag, Icon = Ui.Icon(glyph) });
        }
        tabs.SelectionChanged += (_, _) =>
        {
            if (tabs.SelectedItem is SelectorBarItem item) tabArea.Content = panels[(string)item.Tag];
        };
        tabs.Margin = new Thickness(12, 10, 12, 0);
        tabArea.Padding = new Thickness(16, 6, 18, 18);
        var right = new Grid();
        right.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        right.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        Grid.SetRow(tabArea, 1);
        right.Children.Add(tabs);
        right.Children.Add(tabArea);
        var side = new Border
        {
            BorderBrush = Ui.Brush("DividerStrokeColorDefaultBrush"),
            BorderThickness = new Thickness(1, 0, 0, 0),
            Child = right,
        };

        var layout = new Grid();
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star), MinWidth = 500 });
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(360) });
        Grid.SetColumn(side, 1);
        layout.Children.Add(new ScrollViewer { Content = left, VerticalScrollBarVisibility = ScrollBarVisibility.Auto });
        layout.Children.Add(side);
        Content = layout;
        ShowTab("face");
    }

    /// <summary>Show one of the tabs at the right: "face", "loop" or "camera".</summary>
    public void ShowTab(string tag)
    {
        var item = tabs.Items.FirstOrDefault(one => (string)one.Tag == tag);
        if (item != null) tabs.SelectedItem = item;
    }

    public void Update(AppState state)
    {
        preview.Update(state);
        controls.Update(state);
        face.Update(state);
        loop.Update(state);
        source.Update(state);
        UpdateStatus(state);
        UpdateMeeting(state);
    }

    /// <summary>The one line under the controls: what matters most right now.</summary>
    private void UpdateStatus(AppState state)
    {
        loading.Visible(state.SwapState == SwapState.Loading);
        allow.Visible(state.Source == SourceKind.Camera && state.SourceProblem == SourceProblem.CameraNotAllowed);
        if (state.IsRecording)
        {
            status.Set(T("Recording. Sit still for the last second, then click Stop recording."), Ui.IconRecord, Tone.Plain);
            return;
        }
        if (state.SwapWanted || state.SwapState is SwapState.Loading or SwapState.Missing or SwapState.Failed)
        {
            switch (state.SwapState)
            {
                case SwapState.Loading:
                    status.Set(T("Getting the models ready. The first time takes a little longer."));
                    return;
                case SwapState.Missing:
                    status.Set(T("A model file is not installed: {0}", state.SwapDetail), Ui.IconWarning, Tone.Warning);
                    return;
                case SwapState.Failed:
                    status.Set(state.SwapDetail, Ui.IconError, Tone.Bad);
                    return;
                case SwapState.On when state.SwapError != null:
                    status.Set(state.SwapError, Ui.IconError, Tone.Bad);
                    return;
                case SwapState.On when state.Mode == EngineMode.Looping:
                    status.Set(T("Playing the loop. The swap keeps following your face, so going back to live does not jump."), Ui.IconLoop);
                    return;
                case SwapState.On when state.Swapping:
                    status.Set(
                        T("Swapping. A new face picture every {0} ms, {1} ms behind the camera.", Math.Round(state.SwapMs), Math.Round(state.SwapAgeMs)),
                        Ui.IconGood, Tone.Good);
                    return;
                case SwapState.On when state.PictureHidden:
                    status.Set(T("The swap is not on your face, so the picture is blurred."), Ui.IconHidden, Tone.Warning);
                    return;
                case SwapState.On when state.FaceFound:
                    status.Set(T("Getting the first swapped face…"));
                    return;
                case SwapState.On:
                    status.Set(T("No face in the picture. Others see the picture as it is."), Ui.IconInfo);
                    return;
            }
        }
        if (state.Mode == EngineMode.Looping)
        {
            status.Set(T("Playing the loop. Others see the recording until you go back to live."), Ui.IconLoop);
            return;
        }
        switch (state.SourceProblem)
        {
            case SourceProblem.CameraNotAllowed:
                status.Set(T("God's Camera needs your permission to use the camera."), Ui.IconInfo);
                return;
            case SourceProblem.NoCamera:
                status.Set(T("No camera is attached to this PC."), Ui.IconWarning, Tone.Warning);
                return;
            case SourceProblem.CameraRefused:
                status.Set(T("Windows does not let desktop apps use cameras. Turn that on under Privacy & security, Camera."), Ui.IconWarning, Tone.Warning);
                return;
            case SourceProblem.CameraUnavailable:
                status.Set(T("The camera cannot be used. Another app may have it, or it was unplugged."), Ui.IconWarning, Tone.Warning);
                return;
            case SourceProblem.FileUnreadable:
                status.Set(T("That file cannot be shown."), Ui.IconWarning, Tone.Warning);
                return;
            case SourceProblem.NoFileChosen:
                status.Set(T("Choose a file to show."), Ui.IconInfo);
                return;
        }
        status.Set(state.Faces.Length == 0
            ? T("Add a face first: click Add a face and choose photos of the person.")
            : T("The swap is off. Others see your own face."));
    }

    /// <summary>The small label at the top right: can meeting apps use the camera, and does one.</summary>
    private void UpdateMeeting(AppState state)
    {
        var (words, colour) = state.VirtualCamera switch
        {
            VirtualCamera.Ready when state.Watched => (T("An app is using the camera"), Ui.Green),
            VirtualCamera.Ready => (T("No app is using the camera"), Ui.Grey),
            VirtualCamera.NotInstalled => (T("Not installed"), Ui.Orange),
            VirtualCamera.Busy => (T("Another copy of God's Camera is running and holds the camera. Close it, then start this one again."), Ui.Orange),
            VirtualCamera.Refused => (T("Windows does not let desktop apps use cameras. Turn that on under Privacy & security, Camera."), Ui.Red),
            _ => (T("Windows could not make the camera."), Ui.Red),
        };
        if (meetingText.Text != words) meetingText.Text = words;
        meetingText.MaxWidth = 260;
        meetingText.TextTrimming = TextTrimming.CharacterEllipsis;
        meetingDot.Fill = new SolidColorBrush(colour);
    }
}

/// <summary>
/// The row of controls under the picture, the ones used during a call: the
/// swap with the chosen face, recording a loop, playing it, where the picture
/// comes from, and mirroring it.
/// </summary>
internal sealed class CallControls : Grid
{
    private static readonly string[] Pictures = { ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff" };
    private static readonly string[] Videos = { ".mp4", ".m4v", ".mov", ".wmv", ".avi", ".mkv" };

    private readonly Shell shell;
    private readonly FacePanel facePanel;
    private readonly CameraScreen screen;

    private readonly ToggleSplitButton swap = new();
    private readonly PersonPicture avatar = new() { Width = 26, Height = 26 };
    private readonly MenuFlyout faces = new() { Placement = FlyoutPlacementMode.BottomEdgeAlignedLeft };
    private string facesKey = "";
    private string avatarFor = "";

    private readonly Button record;
    private readonly FontIcon recordIcon = Ui.Icon(Ui.IconRecord, 15);
    private readonly TextBlock recordText = new();
    private readonly Button play;
    private readonly FontIcon playIcon = Ui.Icon(Ui.IconPlay, 15);
    private readonly TextBlock playText = new();

    private readonly DropDownButton source = new();
    private readonly FontIcon sourceIcon = Ui.Icon(Ui.IconCamera, 15);
    private readonly TextBlock sourceText = new() { MaxWidth = 190, TextTrimming = TextTrimming.CharacterEllipsis, FontSize = 13 };
    private readonly MenuFlyout sources = new() { Placement = FlyoutPlacementMode.BottomEdgeAlignedLeft };
    private string sourcesKey = "";

    private readonly ToggleButton mirror = new();
    private bool showing;

    /// <summary>The source and the mirror, to be laid on the picture.</summary>
    public StackPanel OnPicture { get; }

    public CallControls(Shell shell, FacePanel facePanel, CameraScreen screen)
    {
        this.shell = shell;
        this.facePanel = facePanel;
        this.screen = screen;

        // The swap, with the chosen face's picture; the arrow opens the faces.
        // Short words: in Vietnamese the three controls only just fit beside each other.
        swap.Content = Ui.Row(9, avatar, new TextBlock { Text = T("Face swap"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        swap.Padding = new Thickness(8, 5, 8, 5);
        swap.Flyout = faces;
        swap.IsCheckedChanged += async (_, _) =>
        {
            if (showing) return;
            var on = swap.IsChecked;
            if (on && shell.App.State().Faces.Length == 0) screen.ShowTab("face");
            await facePanel.TurnSwap(on);
        };
        ToolTipService.SetToolTip(swap, T("Show another face in place of yours, live."));

        record = new Button { Content = Ui.Row(8, recordIcon, recordText), Padding = new Thickness(12, 8, 12, 8) };
        record.Click += (_, _) => shell.App.ToggleRecording();
        ToolTipService.SetToolTip(record, T("Record or stop a loop") + "  (Ctrl+Alt+Shift+R)");

        play = new Button
        {
            Content = Ui.Row(8, playIcon, playText),
            Padding = new Thickness(12, 8, 12, 8),
            Style = (Style)Application.Current.Resources["AccentButtonStyle"],
        };
        play.Click += (_, _) => shell.App.ToggleLoop();
        ToolTipService.SetToolTip(play, T("Play the loop or go back to live") + "  (Ctrl+Alt+Shift+L)");

        // The source and the mirror are drawn on the picture itself, at its
        // lower left, light on a dark glass: they change the picture, and the
        // bar keeps the three controls used during a call.
        var glass = new SolidColorBrush(Ui.Rgb(0, 0, 0, 150));
        var white = new SolidColorBrush(Microsoft.UI.Colors.White);
        sourceIcon.Foreground = white;
        sourceText.Foreground = white;
        source.Content = Ui.Row(8, sourceIcon, sourceText);
        source.Padding = new Thickness(10, 5, 8, 5);
        source.Background = glass;
        source.Foreground = white;
        source.BorderThickness = new Thickness(0);
        source.CornerRadius = new CornerRadius(14);
        source.Flyout = sources;
        ToolTipService.SetToolTip(source, T("Where the picture comes from"));

        mirror.Content = Ui.Icon(Ui.IconMirror, 14);
        mirror.Padding = new Thickness(9, 6, 9, 6);
        mirror.Background = glass;
        mirror.Foreground = white;
        mirror.BorderThickness = new Thickness(0);
        mirror.CornerRadius = new CornerRadius(14);
        mirror.Click += (_, _) => shell.App.SetMirror(mirror.IsChecked == true);
        ToolTipService.SetToolTip(mirror, T("Mirror the picture"));
        OnPicture = Ui.Row(6, source, mirror);
        OnPicture.HorizontalAlignment = HorizontalAlignment.Left;
        OnPicture.VerticalAlignment = VerticalAlignment.Bottom;
        OnPicture.Margin = new Thickness(10);

        // The swap at the left, where the eye starts, then the loop's two.
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        foreach (var control in new FrameworkElement[] { swap, record, play })
        {
            control.VerticalAlignment = VerticalAlignment.Center;
            row.Children.Add(control);
        }
        // A card around the row, as the panels at the right have.
        Children.Add(new Border
        {
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(10),
            BorderThickness = new Thickness(1),
            Background = Ui.Brush("CardBackgroundFillColorDefaultBrush"),
            BorderBrush = Ui.Brush("CardStrokeColorDefaultBrush"),
            Child = row,
        });
    }

    public void Update(AppState state)
    {
        showing = true;
        if (swap.IsChecked != state.SwapWanted) swap.IsChecked = state.SwapWanted;
        mirror.IsChecked = state.Mirror;
        showing = false;
        UpdateFaces(state);

        // Recording shows in red: it is the one thing that should not be left on by mistake.
        record.IsEnabled = state.CanRecord || state.IsRecording;
        recordIcon.Glyph = state.IsRecording ? Ui.IconStop : Ui.IconRecord;
        recordText.Text = state.IsRecording ? T("Stop recording") : T("Record a loop");
        if (state.IsRecording)
        {
            record.Background = new SolidColorBrush(Ui.Red);
            record.Foreground = new SolidColorBrush(Microsoft.UI.Colors.White);
        }
        else
        {
            record.ClearValue(Control.BackgroundProperty);
            record.ClearValue(Control.ForegroundProperty);
        }
        var looping = state.Mode == EngineMode.Looping;
        play.IsEnabled = state.CanLoop;
        playIcon.Glyph = looping ? Ui.IconVideo : Ui.IconPlay;
        playText.Text = looping ? T("Back to live") : T("Play the loop");

        UpdateSources(state);
    }

    /// <summary>The faces in the swap's menu, and the chosen one's picture on the button.</summary>
    private void UpdateFaces(AppState state)
    {
        var key = string.Join("|", state.Faces.Select(one => $"{one.Id}:{one.Name}")) + "#" + state.SelectedFace;
        if (key == facesKey) return;
        facesKey = key;
        faces.Items.Clear();
        foreach (var one in state.Faces)
        {
            var id = one.Id;
            var item = new RadioMenuFlyoutItem { Text = one.Name, GroupName = "faces", IsChecked = one.Id == state.SelectedFace };
            item.Click += (_, _) => shell.App.SelectFace(id);
            faces.Items.Add(item);
        }
        if (state.Faces.Length > 0) faces.Items.Add(new MenuFlyoutSeparator());
        var add = new MenuFlyoutItem { Text = T("Add a face…"), Icon = Ui.Icon(Ui.IconAdd) };
        add.Click += (_, _) =>
        {
            screen.ShowTab("face");
            facePanel.ChoosePhotos(null);
        };
        faces.Items.Add(add);
        var options = new MenuFlyoutItem { Text = T("Face options…"), Icon = Ui.Icon(Ui.IconWand) };
        options.Click += (_, _) => screen.ShowTab("face");
        faces.Items.Add(options);

        var chosen = state.Faces.FirstOrDefault(one => one.Id == state.SelectedFace);
        avatar.DisplayName = chosen?.Name ?? "";
        if (chosen == null)
        {
            avatarFor = "";
            avatar.ProfilePicture = null;
        }
        else if (avatarFor != chosen.Id)
        {
            avatarFor = chosen.Id;
            FillAvatar(chosen.Id);
        }
    }

    private async void FillAvatar(string id)
    {
        var app = shell.App;
        var picture = await Ui.Picture(await Task.Run(() => app.FacePicture(id, null)));
        // Another face may have been chosen while this one was read.
        if (avatarFor == id) avatar.ProfilePicture = picture;
    }

    /// <summary>The source's name on its button, and the choices in its menu.</summary>
    private void UpdateSources(AppState state)
    {
        var cameraName = state.Cameras.FirstOrDefault(one => one.Id == state.Camera)?.Name;
        (sourceIcon.Glyph, sourceText.Text) = state.Source switch
        {
            SourceKind.Camera => (Ui.IconCamera, cameraName ?? T("Camera")),
            SourceKind.Image => (Ui.IconPicture, state.FileName ?? T("Image")),
            SourceKind.Video => (Ui.IconFilm, state.FileName ?? T("Video")),
            _ => (Ui.IconPattern, T("Test pattern")),
        };

        var key = $"{state.Source}|{state.Camera}|" + string.Join("|", state.Cameras.Select(one => one.Id + ":" + one.Name));
        if (key == sourcesKey) return;
        sourcesKey = key;
        sources.Items.Clear();
        if (state.Cameras.Length == 0)
        {
            var none = new RadioMenuFlyoutItem { Text = T("Camera"), GroupName = "source", IsChecked = state.Source == SourceKind.Camera };
            none.Click += (_, _) => shell.App.SetSource(SourceKind.Camera, null);
            sources.Items.Add(none);
        }
        foreach (var one in state.Cameras)
        {
            var id = one.Id;
            var item = new RadioMenuFlyoutItem
            {
                Text = one.Name,
                GroupName = "source",
                IsChecked = state.Source == SourceKind.Camera && state.Camera == one.Id,
            };
            item.Click += (_, _) =>
            {
                shell.App.SetSource(SourceKind.Camera, null);
                shell.App.SetCamera(id);
            };
            sources.Items.Add(item);
        }
        sources.Items.Add(new MenuFlyoutSeparator());
        var picture = new RadioMenuFlyoutItem { Text = T("A picture…"), GroupName = "source", IsChecked = state.Source == SourceKind.Image };
        picture.Click += async (_, _) => await ChooseFile(SourceKind.Image);
        var video = new RadioMenuFlyoutItem { Text = T("A video…"), GroupName = "source", IsChecked = state.Source == SourceKind.Video };
        video.Click += async (_, _) => await ChooseFile(SourceKind.Video);
        var pattern = new RadioMenuFlyoutItem { Text = T("Test pattern"), GroupName = "source", IsChecked = state.Source == SourceKind.Pattern };
        pattern.Click += (_, _) => shell.App.SetSource(SourceKind.Pattern, null);
        sources.Items.Add(picture);
        sources.Items.Add(video);
        sources.Items.Add(pattern);
    }

    private async Task ChooseFile(SourceKind kind)
    {
        var files = await shell.PickFiles(false, kind == SourceKind.Video ? Videos : Pictures);
        // Choosing nothing leaves the picture as it was; the menu is drawn again from the state.
        sourcesKey = "";
        if (files.Count > 0) shell.App.SetSource(kind, files[0]);
    }
}
