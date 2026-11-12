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
    string? DemoPhoto, string FolderMark, string? StatusFile, bool TryOut, bool ScannerOn, bool ScannerDemo);

/// <summary>The window: a list of screens at the left, and the chosen screen beside it.</summary>
internal sealed class Shell : Window
{
    public DesktopApp App { get; }
    public Scanner Scanner { get; }
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
    private AppState? last;

    // The notification-area icon, the hot keys, and what closing the window means.
    private readonly IntPtr handle;
    private readonly bool resident;
    private readonly Native.WindowProc messages;
    private IntPtr previousMessages;
    private readonly uint taskbarCreated = Native.RegisterWindowMessageW("TaskbarCreated");
    private TrayIcon? tray;
    private bool hidden;
    private bool quitting;
    private bool toldAboutTray;
    private int pollsWhileHidden;

    /// <summary>
    /// The message a second copy of the app sends to the first: "show
    /// yourself". It carries the mark of the data folder in its name, so only
    /// the copy on the same data hears it.
    /// </summary>
    public static uint ShowYourselfMessage(string folderMark) => Native.RegisterWindowMessageW($"GodsCamera.ShowYourself.{folderMark}");

    /// <summary>
    /// The message that asks the running copy to end, as Quit in the icon's
    /// menu does: for an installer that is about to replace the app's files.
    /// </summary>
    public static uint QuitMessage(string folderMark) => Native.RegisterWindowMessageW($"GodsCamera.Quit.{folderMark}");

    private readonly uint showYourself;
    private readonly uint quitMessage;
    private readonly string? statusFile;
    private int pollsSinceStatus;

    private const int HotKeyRecord = 1, HotKeyLoop = 2;

    /// <summary>Which hot keys Windows gave the app. Another program may hold one already.</summary>
    public bool RecordKeyWorks { get; private set; }
    public bool LoopKeyWorks { get; private set; }

    /// <summary>The app asked for hot keys at all. A run that only takes pictures of the screens does not.</summary>
    public bool AskedForHotKeys => resident;

    public Shell(Launch launch)
    {
        DataFolder = launch.DataFolder;
        PackFolders = launch.PackFolders;
        App = new DesktopApp(new AppConfig(launch.DataFolder, launch.PackFolders, launch.CacheFolder, launch.TryOut));
        // The program that reads text from photos is beside this one, and finds its models where the app does.
        var reader = new List<string>();
        var readerProgram = System.IO.Path.Combine(AppContext.BaseDirectory, "gc-ocr.exe");
        if (File.Exists(readerProgram))
        {
            reader.Add(readerProgram);
            foreach (var folder in launch.PackFolders)
            {
                reader.Add("--packs");
                reader.Add(folder);
            }
        }
        Scanner = new Scanner(new ScannerConfig(launch.DataFolder, reader.ToArray(), launch.PackFolders, Environment.ProcessPath ?? "", launch.TryOut));
        if (launch.ScannerOn) Scanner.SetWanted(true);
        else Scanner.Start();
        handle = WinRT.Interop.WindowNative.GetWindowHandle(this);
        // A run that only takes pictures of the app stays out of the notification area.
        resident = launch.SnapshotFolder == null;
        messages = OnMessage;
        showYourself = ShowYourselfMessage(launch.FolderMark);
        quitMessage = QuitMessage(launch.FolderMark);
        statusFile = launch.StatusFile;

        Title = "God's Camera";
        var iconFile = System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", "app.ico");
        if (File.Exists(iconFile)) AppWindow.SetIcon(iconFile);
        // 1180 by 780 at the size text is shown on this screen.
        var scale = Native.GetDpiForWindow(handle) / 96.0;
        AppWindow.Resize(new Windows.Graphics.SizeInt32((int)(1180 * scale), (int)(780 * scale)));

        navigation.PaneDisplayMode = NavigationViewPaneDisplayMode.Left;
        navigation.IsBackButtonVisible = NavigationViewBackButtonVisible.Collapsed;
        navigation.IsSettingsVisible = false;
        navigation.IsPaneToggleButtonVisible = false;
        navigation.OpenPaneLength = 216;
        Add("camera", T("Camera"), Ui.IconCamera, () => new CameraScreen(this));
        Add("loop", T("Loop"), Ui.IconLoop, () => new LoopScreen(this));
        Add("face", T("Face"), Ui.IconFace, () => new FaceScreen(this));
        Add("scanner", T("Scanner"), Ui.IconScanner, () => new ScannerScreen(this));
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
        Log.Write($"started: version {Version}, camera for other apps: {App.State().VirtualCamera}, pictures of the screens only: {!resident}");
        var scanner = Scanner.State();
        Log.Write($"scanner: wanted {scanner.Wanted}, running {scanner.Running}, devices on the network: {scanner.Network}, can read text: {scanner.ReaderMissing == null}");
        if (resident)
        {
            previousMessages = Native.SetWindowLongPtrW(
                handle, Native.GWLP_WNDPROC, System.Runtime.InteropServices.Marshal.GetFunctionPointerForDelegate(messages));
            tray = new TrayIcon(handle, iconFile);
            tray.Add("God's Camera");
            // Control, Alt and Shift with R or L: free in the meeting apps, and reachable with one hand.
            const uint together = Native.MOD_CONTROL | Native.MOD_ALT | Native.MOD_SHIFT | Native.MOD_NOREPEAT;
            RecordKeyWorks = Native.RegisterHotKey(handle, HotKeyRecord, together, 'R');
            LoopKeyWorks = Native.RegisterHotKey(handle, HotKeyLoop, together, 'L');
            Log.Write($"hot keys: record {(RecordKeyWorks ? "ours" : "taken by another program")}, loop {(LoopKeyWorks ? "ours" : "taken by another program")}");
            // Closing the window leaves the app running, so that other apps keep
            // their camera. Quit, in the icon's menu, ends it.
            AppWindow.Closing += (_, closing) =>
            {
                if (quitting || last is not { KeepRunning: true }) return;
                closing.Cancel = true;
                HideToTray();
            };
        }
        Closed += (_, _) =>
        {
            stateTimer.Stop();
            pictureTimer.Stop();
            if (resident)
            {
                Native.UnregisterHotKey(handle, HotKeyRecord);
                Native.UnregisterHotKey(handle, HotKeyLoop);
                if (previousMessages != IntPtr.Zero) Native.SetWindowLongPtrW(handle, Native.GWLP_WNDPROC, previousMessages);
                tray?.Dispose();
            }
            Scanner.Dispose();
            App.Dispose();
        };
        Poll();

        if (launch.SnapshotFolder != null) _ = TakeSnapshots(launch.SnapshotFolder, launch.DemoPhoto, launch.ScannerDemo);
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

    // The notification area, the hot keys, and one copy at a time

    private void HideToTray()
    {
        hidden = true;
        AppWindow.Hide();
        pictureTimer.Stop();
        // With the window gone, the picture is made only while an app reads the camera.
        App.SetPreviewWanted(false);
        if (!toldAboutTray)
        {
            toldAboutTray = true;
            tray?.Tell(
                T("God's Camera is still running"),
                T("Other apps can keep using the camera. To end it, right-click this icon and choose Quit."));
        }
    }

    private void ShowWindow()
    {
        hidden = false;
        App.SetPreviewWanted(true);
        AppWindow.Show();
        if (Native.IsIconic(handle)) Native.ShowWindow(handle, Native.SW_RESTORE);
        Native.SetForegroundWindow(handle);
        pictureTimer.Start();
        Poll();
    }

    private void Quit()
    {
        quitting = true;
        Close();
    }

    private void TrayMenu()
    {
        if (tray == null || last == null) return;
        var recording = last.IsRecording;
        var looping = last.Mode == EngineMode.Looping;
        var chosen = tray.Menu(
            (1, T("Open God's Camera")),
            (0, ""),
            (2, recording ? T("Stop recording") : T("Record a loop")),
            (3, looping ? T("Back to live") : T("Play the loop")),
            (0, ""),
            (4, T("Quit")));
        switch (chosen)
        {
            case 1: ShowWindow(); break;
            case 2: App.ToggleRecording(); break;
            case 3: App.ToggleLoop(); break;
            case 4: Quit(); break;
        }
    }

    /// <summary>The window's messages, for the few that WinUI does not hand on.</summary>
    private IntPtr OnMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam)
    {
        // Nothing thrown here may reach Windows: it called this, and would end the app.
        try
        {
            if (message == Native.WM_HOTKEY)
            {
                var key = wParam.ToInt64();
                if (key == HotKeyRecord) App.ToggleRecording();
                if (key == HotKeyLoop) App.ToggleLoop();
                return IntPtr.Zero;
            }
            if (message == TrayIcon.Message)
            {
                // What happened to the icon is in the low half of the last number.
                var what = (uint)(lParam.ToInt64() & 0xFFFF);
                if (what == Native.WM_LBUTTONUP) ShowWindow();
                if (what == Native.WM_RBUTTONUP || what == Native.WM_CONTEXTMENU) TrayMenu();
                return IntPtr.Zero;
            }
            if (message == showYourself)
            {
                ShowWindow();
                return IntPtr.Zero;
            }
            if (message == quitMessage)
            {
                Quit();
                return IntPtr.Zero;
            }
            if (message == taskbarCreated)
            {
                // Explorer was started again and has forgotten every icon.
                tray?.Add("God's Camera");
            }
        }
        catch (Exception problem)
        {
            Log.Problem("a window message", problem);
        }
        return Native.CallWindowProcW(previousMessages, window, message, wParam, lParam);
    }

    private void Poll()
    {
        // Hidden, the state is asked only twice a second: enough to notice
        // that an app has started to read the camera.
        if (hidden && ++pollsWhileHidden % 5 != 0) return;
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
        last = state;
        tray?.SetTip(TrayWords(state));
        WriteStatus(state);
        if (hidden) return;
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

    /// <summary>What the app is doing, into the file named with --status, about once a second.</summary>
    private void WriteStatus(AppState state)
    {
        if (statusFile == null || (++pollsSinceStatus % (hidden ? 2 : 10) != 0)) return;
        var status = new Dictionary<string, object?>
        {
            ["mode"] = state.Mode.ToString(),
            ["source"] = state.Source.ToString(),
            ["source_problem"] = state.SourceProblem?.ToString(),
            ["frames_out"] = state.FramesOut,
            ["source_fps"] = Math.Round(state.SourceFps, 1),
            ["recording"] = state.IsRecording,
            ["has_clip"] = state.Clip != null,
            ["watched"] = state.Watched,
            ["resting"] = state.Resting,
            ["virtual_camera"] = state.VirtualCamera.ToString(),
            ["swap"] = state.SwapState.ToString(),
            ["window_hidden"] = hidden,
            ["tray_icon"] = tray != null,
            ["record_key"] = RecordKeyWorks,
            ["loop_key"] = LoopKeyWorks,
        };
        try
        {
            var scanner = Scanner.State();
            status["scanner_running"] = scanner.Running;
            status["scanner_network"] = scanner.Network.ToString();
            status["scanner_address"] = scanner.LocalAddress;
            status["scanner_can_read"] = scanner.ReaderMissing == null;
            status["documents"] = scanner.Documents.Length;
            status["documents_done"] = scanner.Documents.Count(document => document.Status == DocumentStatus.Done);
            status["newest_document"] = scanner.Documents.Length > 0 ? scanner.Documents[0].Title : null;
        }
        catch (Exception)
        {
            // The window is closing and the scanner is gone.
        }
        try
        {
            var partial = statusFile + ".partial";
            File.WriteAllText(partial, System.Text.Json.JsonSerializer.Serialize(status));
            File.Move(partial, statusFile, true);
        }
        catch (IOException)
        {
            // Whoever reads the file had it open; the next second writes it again.
        }
    }

    private static string TrayWords(AppState state)
    {
        var doing = state.Mode switch
        {
            EngineMode.Live => state.Swapping ? T("Live, face swapped") : T("Live"),
            EngineMode.Looping => T("Playing a loop"),
            _ => T("Camera off"),
        };
        var watched = state.Watched ? T("An app is using the camera") : T("No app is using the camera");
        return $"God's Camera\n{doing}\n{watched}";
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

    /// <summary>The camera's file and its install script are beside the program.</summary>
    public bool CarriesCamera => File.Exists(CameraFile) && File.Exists(CameraScript);

    private static string CameraFile => System.IO.Path.Combine(AppContext.BaseDirectory, "Camera", "GodsCameraSource.dll");
    private static string CameraScript => System.IO.Path.Combine(AppContext.BaseDirectory, "Camera", "register-windows-camera.ps1");

    /// <summary>
    /// Install the camera for other apps, or remove it. Windows asks for an
    /// administrator's permission: the camera service only loads a camera that
    /// is registered for the whole computer. Nothing happens if the user says no.
    /// </summary>
    public async Task InstallCamera(bool remove)
    {
        var arguments = $"-NoProfile -ExecutionPolicy Bypass -File \"{CameraScript}\" -Source \"{CameraFile}\"" + (remove ? " -Unregister" : "");
        try
        {
            var start = new System.Diagnostics.ProcessStartInfo("powershell.exe", arguments)
            {
                // "runas" is what brings up Windows' own question.
                Verb = "runas",
                UseShellExecute = true,
                WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden,
            };
            using var process = System.Diagnostics.Process.Start(start);
            if (process != null) await process.WaitForExitAsync();
        }
        catch (System.ComponentModel.Win32Exception)
        {
            // The user said no to Windows' question.
        }
        App.RetryVirtualCamera();
        Poll();
    }

    /// <summary>The script that tells Windows Firewall about the scanner is beside the program.</summary>
    public bool CarriesNetworkScript => File.Exists(NetworkScript);

    private static string NetworkScript => System.IO.Path.Combine(AppContext.BaseDirectory, "Network", "allow-windows-hub.ps1");

    /// <summary>
    /// Tell Windows Firewall to let phones and tablets on a private network
    /// reach the scanner. Windows asks for an administrator's permission.
    /// Nothing happens if the user says no.
    /// </summary>
    public async Task AllowDevices()
    {
        var arguments = $"-NoProfile -ExecutionPolicy Bypass -File \"{NetworkScript}\" -Program \"{Environment.ProcessPath}\" -Port 47801";
        try
        {
            var start = new System.Diagnostics.ProcessStartInfo("powershell.exe", arguments)
            {
                // "runas" is what brings up Windows' own question.
                Verb = "runas",
                UseShellExecute = true,
                WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden,
            };
            using var process = System.Diagnostics.Process.Start(start);
            if (process != null) await process.WaitForExitAsync();
        }
        catch (System.ComponentModel.Win32Exception)
        {
            // The user said no to Windows' question.
        }
        Scanner.RefreshNetwork();
        Log.Write($"asked Windows Firewall to let devices in; devices on the network: {Scanner.State().Network}");
    }

    /// <summary>Open a file with the program Windows has for it, or a page of Windows Settings.</summary>
    public static void Open(string what)
    {
        try
        {
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(what) { UseShellExecute = true });
        }
        catch (Exception)
        {
            // Nothing to open it with is no reason to trouble the user further.
        }
    }

    /// <summary>Show a file in File Explorer, selected.</summary>
    public static void ShowInExplorer(string file)
    {
        try
        {
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo("explorer.exe", $"/select,\"{file}\"") { UseShellExecute = true });
        }
        catch (Exception)
        {
        }
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
    /// folder of its own. With `scannerDemo`, or after that, the scanner is
    /// shown in use as well, which needs neither models nor the graphics card.
    /// </summary>
    private async Task TakeSnapshots(string folder, string? demoPhoto, bool scannerDemo)
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
            if (demoPhoto != null || scannerDemo) await DemonstrateScanner(folder);
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

    /// <summary>
    /// The scanner as it looks in use: turned on, with documents as a tablet
    /// would have sent them, and the code a device connects with. The hub of
    /// a try-out stays on this computer, on whatever ports are free.
    /// </summary>
    private async Task DemonstrateScanner(string folder)
    {
        Scanner.SetWanted(true);
        var jobs = System.IO.Path.Combine(DataFolder, "Hub", "jobs");
        void Document(string id, string status, double progress, string? text)
        {
            var job = System.IO.Path.Combine(jobs, id);
            Directory.CreateDirectory(job);
            if (text != null) File.WriteAllText(System.IO.Path.Combine(job, "document.txt"), text);
            File.WriteAllText(System.IO.Path.Combine(job, "job.json"), System.Text.Json.JsonSerializer.Serialize(new Dictionary<string, object?>
            {
                ["id"] = id,
                ["kind"] = "document",
                ["upload_id"] = "00000000-0000-4000-8000-000000000000",
                ["status"] = status,
                ["progress"] = progress,
                ["result"] = text != null ? new Dictionary<string, object?> { ["pictures"] = 6 } : null,
                ["files"] = text != null ? new[] { "document.md", "document.txt", "document.json" } : Array.Empty<string>(),
                ["error"] = null,
            }));
        }
        Document("11111111-1111-4111-8111-111111111111", "done", 1.0, T("Sample: a page from a newspaper") + "\n\n" + T("This is where the text of your scanned pages appears, joined into one document."));
        await Task.Delay(1100);
        Document("22222222-2222-4222-8222-222222222222", "running", 0.6, null);
        navigation.SelectedItem = Screen("scanner");
        await Task.Delay(1500);
        await Picture(folder, "scanner-on");
        if (screens.TryGetValue("scanner", out var screen) && screen is ScannerScreen scanner)
        {
            // What the screen says before Windows Firewall lets devices in,
            // and on a network that Windows treats as public.
            scanner.Picture(NetworkState.FirewallNeeded);
            await Picture(folder, "scanner-firewall");
            scanner.Picture(NetworkState.PublicNetwork);
            await Picture(folder, "scanner-public-network");
            scanner.Picture(null);
            scanner.Pair();
            await Task.Delay(1200);
            await Picture(folder, "scanner-code");
            foreach (var popup in VisualTreeHelper.GetOpenPopupsForXamlRoot(Root))
            {
                if (popup.Child is ContentDialog dialog) dialog.Hide();
            }
        }
        Scanner.SetWanted(false);
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
