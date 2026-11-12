using System.Runtime.InteropServices;
using GodsCamera.Screens;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using uniffi.gc_app;
using Windows.Graphics.Imaging;
using Windows.Storage.Pickers;
using static GodsCamera.Texts;

namespace GodsCamera;

/// <summary>How the app was started.</summary>
internal sealed record Launch(
    string DataFolder, string[] PackFolders, string CacheFolder, string? Language, string? Theme, string? SnapshotFolder,
    string? DemoPhoto);

/// <summary>The window: a list of screens at the left, and the chosen screen beside it.</summary>
internal sealed class Shell : Window
{
    public DesktopApp App { get; }
    public PreviewSource Preview { get; } = new();
    public string DataFolder { get; }
    public string[] PackFolders { get; }
    public string Version { get; } = typeof(Shell).Assembly.GetName().Version?.ToString(3) ?? "?";
    public XamlRoot Root => Content.XamlRoot;

    private readonly NavigationView navigation = new();
    private readonly Dictionary<string, UserControl> screens = new();
    private readonly Microsoft.UI.Xaml.Shapes.Ellipse statusDot = new() { Width = 9, Height = 9 };
    private readonly TextBlock statusText = new() { FontSize = 13, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock watchersText = new() { FontSize = 12, TextWrapping = TextWrapping.Wrap };
    private readonly DispatcherQueueTimer stateTimer;
    private readonly DispatcherQueueTimer pictureTimer;
    private IScreen? current;

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr window);

    public Shell(Launch launch)
    {
        DataFolder = launch.DataFolder;
        PackFolders = launch.PackFolders;
        App = new DesktopApp(new AppConfig(launch.DataFolder, launch.PackFolders, launch.CacheFolder));

        Title = "God's Camera";
        // 1180 by 780 at the size text is shown on this screen.
        var scale = GetDpiForWindow(WinRT.Interop.WindowNative.GetWindowHandle(this)) / 96.0;
        AppWindow.Resize(new Windows.Graphics.SizeInt32((int)(1180 * scale), (int)(780 * scale)));

        navigation.PaneDisplayMode = NavigationViewPaneDisplayMode.Left;
        navigation.IsBackButtonVisible = NavigationViewBackButtonVisible.Collapsed;
        navigation.IsSettingsVisible = false;
        navigation.IsPaneToggleButtonVisible = false;
        navigation.OpenPaneLength = 216;
        Add("camera", T("Camera"), Ui.IconCamera, () => new CameraScreen(this));
        Add("loop", T("Loop"), Ui.IconLoop, () => new LoopScreen(this));
        Add("face", T("Face"), Ui.IconFace, () => new FaceScreen(this));
        Add("scanner", T("Scanner"), Ui.IconScanner, () => new ScannerScreen());
        Add("settings", T("Settings"), Ui.IconSettings, () => new SettingsScreen(this));
        navigation.PaneFooter = StatusCard();
        navigation.SelectionChanged += (_, change) =>
        {
            if (change.SelectedItem is NavigationViewItem item) Show((string)item.Tag);
        };
        // The window's own see-through material cannot be drawn into a picture
        // of the window, so for pictures the screens stand on a plain background.
        var root = new Grid();
        root.Children.Add(navigation);
        if (launch.SnapshotFolder == null)
        {
            SystemBackdrop = new MicaBackdrop();
        }
        else
        {
            root.Background = Ui.Brush("SolidBackgroundFillColorBaseBrush");
        }
        Content = root;
        navigation.SelectedItem = navigation.MenuItems[0];

        var queue = DispatcherQueue.GetForCurrentThread();
        stateTimer = queue.CreateTimer();
        stateTimer.Interval = TimeSpan.FromMilliseconds(100);
        stateTimer.Tick += (_, _) => Poll();
        stateTimer.Start();
        pictureTimer = queue.CreateTimer();
        pictureTimer.Interval = TimeSpan.FromMilliseconds(33);
        pictureTimer.Tick += (_, _) => Preview.Refresh();
        pictureTimer.Start();
        Closed += (_, _) =>
        {
            stateTimer.Stop();
            pictureTimer.Stop();
            App.Dispose();
        };
        Poll();

        if (launch.SnapshotFolder != null) _ = TakeSnapshots(launch.SnapshotFolder, launch.DemoPhoto);
    }

    private readonly Dictionary<string, Func<UserControl>> makers = new();

    private void Add(string tag, string title, string glyph, Func<UserControl> make)
    {
        makers[tag] = make;
        navigation.MenuItems.Add(new NavigationViewItem { Content = title, Tag = tag, Icon = Ui.Icon(glyph) });
    }

    private void Show(string tag)
    {
        // A screen is made the first time it is shown, and kept.
        if (!screens.TryGetValue(tag, out var screen))
        {
            screen = makers[tag]();
            screens[tag] = screen;
        }
        navigation.Content = screen;
        current = screen as IScreen;
        Poll();
    }

    /// <summary>What the camera is doing, always in view at the foot of the list.</summary>
    private Border StatusCard()
    {
        watchersText.Foreground = Ui.Brush("TextFillColorSecondaryBrush");
        // The dot keeps its place and the words beside it wrap: in Vietnamese they are longer.
        var state = new Grid { ColumnSpacing = 7 };
        state.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        state.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        statusDot.VerticalAlignment = VerticalAlignment.Top;
        statusDot.Margin = new Thickness(0, 5, 0, 0);
        Grid.SetColumn(statusText, 1);
        state.Children.Add(statusDot);
        state.Children.Add(statusText);
        return new Border
        {
            Margin = new Thickness(12),
            Padding = new Thickness(10),
            CornerRadius = new CornerRadius(10),
            Background = Ui.Brush("SubtleFillColorSecondaryBrush"),
            Child = Ui.Column(5, state, watchersText),
        };
    }

    private void Poll()
    {
        AppState state;
        try
        {
            state = App.State();
        }
        catch (Exception)
        {
            // The window is closing and the app is gone.
            return;
        }
        statusDot.Fill = new SolidColorBrush(state.Mode switch
        {
            EngineMode.Live => Ui.Green,
            EngineMode.Looping => Ui.Red,
            _ => Ui.Grey,
        });
        statusText.Text = state.Mode switch
        {
            EngineMode.Live => state.Swapping ? T("Live, face swapped") : T("Live"),
            EngineMode.Looping => T("Playing a loop"),
            _ => T("Camera off"),
        };
        watchersText.Text = state.Watched ? T("An app is using the camera") : T("No app is using the camera");
        current?.Update(state);
    }

    // What screens ask of the window

    /// <summary>Ask a question in a dialog. True if the user chose `yes`.</summary>
    public async Task<bool> Ask(string title, UIElement content, string yes, string no)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = Root,
            Title = title,
            Content = content,
            PrimaryButtonText = yes,
            CloseButtonText = no,
            DefaultButton = ContentDialogButton.Primary,
        };
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
    }

    /// <summary>Let the user choose one file or several of the given kinds. Empty if they chose none.</summary>
    public async Task<List<string>> PickFiles(bool several, params string[] kinds)
    {
        var picker = new FileOpenPicker { SuggestedStartLocation = PickerLocationId.PicturesLibrary, ViewMode = PickerViewMode.Thumbnail };
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        foreach (var kind in kinds) picker.FileTypeFilter.Add(kind);
        var chosen = new List<string>();
        if (several)
        {
            foreach (var file in await picker.PickMultipleFilesAsync()) chosen.Add(file.Path);
        }
        else if (await picker.PickSingleFileAsync() is { } file)
        {
            chosen.Add(file.Path);
        }
        return chosen;
    }

    public void OpenFolder(string folder)
    {
        try
        {
            Directory.CreateDirectory(folder);
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo("explorer.exe", $"\"{folder}\"") { UseShellExecute = true });
        }
        catch (Exception)
        {
            // Not being able to show a folder is no reason to trouble the user further.
        }
    }

    // Pictures of the app

    /// <summary>
    /// Save a picture of every screen into `folder` as PNG, then close. For
    /// looking at the app without sitting in front of it.
    ///
    /// With `demoPhoto`, a photo of a face, the app is also put through its
    /// paces first: a loop is recorded and played, the photo is added as a face
    /// and shown as the picture, and the swap is turned on. Run it on a data
    /// folder of its own.
    /// </summary>
    private async Task TakeSnapshots(string folder, string? demoPhoto)
    {
        try
        {
            Directory.CreateDirectory(folder);
            // Let the first pictures arrive, so the preview is not empty.
            await Task.Delay(2500);
            if (demoPhoto != null) await Demonstrate(folder, demoPhoto);
            foreach (var item in navigation.MenuItems.OfType<NavigationViewItem>())
            {
                await Picture(folder, (string)item.Tag, item);
            }
        }
        catch (Exception problem)
        {
            await File.WriteAllTextAsync(System.IO.Path.Combine(folder, "failed.txt"), problem.ToString());
        }
        Close();
    }

    /// <summary>Show a screen, give it a moment, and save a picture of the window.</summary>
    private async Task Picture(string folder, string name, NavigationViewItem? screen = null)
    {
        if (screen != null) navigation.SelectedItem = screen;
        await Task.Delay(900);
        Poll();
        Preview.Refresh();
        await Task.Delay(300);
        await Save((FrameworkElement)Content, System.IO.Path.Combine(folder, $"{name}.png"));
        // A dialog is not part of the window's own picture; it gets one of its own.
        var number = 0;
        foreach (var popup in VisualTreeHelper.GetOpenPopupsForXamlRoot(Root))
        {
            if (popup.Child is FrameworkElement child && child.ActualWidth > 0)
            {
                number++;
                await Save(child, System.IO.Path.Combine(folder, $"{name}-dialog{number}.png"));
            }
        }
    }

    private NavigationViewItem Screen(string tag) => navigation.MenuItems.OfType<NavigationViewItem>().First(item => (string)item.Tag == tag);

    private async Task Until(Func<AppState, bool> done, int seconds)
    {
        var until = DateTime.UtcNow.AddSeconds(seconds);
        while (!done(App.State()))
        {
            if (DateTime.UtcNow > until) throw new TimeoutException("the app did not get there in time");
            await Task.Delay(100);
        }
    }

    private async Task Demonstrate(string folder, string photo)
    {
        // A loop: record two seconds, look at it, play it, and go back.
        navigation.SelectedItem = Screen("loop");
        await Until(state => state.CanRecord, 20);
        App.ToggleRecording();
        await Until(state => state.RecordedSeconds >= 1.0, 20);
        await Picture(folder, "loop-recording");
        await Until(state => state.RecordedSeconds >= 2.0, 20);
        App.ToggleRecording();
        await Until(state => state.Clip != null, 20);
        App.SetShowLoopBadge(true);
        App.ToggleLoop();
        await Until(state => state.Mode == EngineMode.Looping, 20);
        await Picture(folder, "loop-playing");
        App.SaveClip(T("Sample: at my desk"));
        App.ToggleLoop();
        await Until(state => state.Mode == EngineMode.Live, 20);
        App.SetShowLoopBadge(false);

        // A face: read the photo, look at what was found, save it.
        navigation.SelectedItem = Screen("face");
        App.AddPhotos(new[] { photo }, null);
        await Until(state => state.Pending != null || state.FaceNotice != null, 120);
        await Picture(folder, "face-adding");
        App.SavePending(T("Sample face"), FaceConsent.Synthetic);
        foreach (var popup in VisualTreeHelper.GetOpenPopupsForXamlRoot(Root))
        {
            if (popup.Child is ContentDialog dialog) dialog.Hide();
        }

        // The swap, on a picture that has a face in it.
        App.SetSource(SourceKind.Image, photo);
        App.AcceptPolicy();
        await Until(state => state.SwapState is SwapState.On or SwapState.Missing or SwapState.Failed, 180);
        try
        {
            await Until(state => state.Swapping, 60);
        }
        catch (TimeoutException)
        {
            // The picture will show what the screen says instead.
        }
        await Task.Delay(1500);
    }

    private static async Task Save(FrameworkElement element, string file)
    {
        var render = new RenderTargetBitmap();
        await render.RenderAsync(element);
        var pixels = await render.GetPixelsAsync();
        using var stream = new Windows.Storage.Streams.InMemoryRandomAccessStream();
        var encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, stream);
        encoder.SetPixelData(
            BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, (uint)render.PixelWidth, (uint)render.PixelHeight, 96, 96,
            System.Runtime.InteropServices.WindowsRuntime.WindowsRuntimeBufferExtensions.ToArray(pixels));
        await encoder.FlushAsync();
        stream.Seek(0);
        var bytes = new byte[stream.Size];
        await stream.AsStreamForRead().ReadExactlyAsync(bytes);
        await File.WriteAllBytesAsync(file, bytes);
    }
}
