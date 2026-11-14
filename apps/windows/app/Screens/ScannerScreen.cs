using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>The scanner: connecting a phone or tablet, and the documents it has sent.</summary>
internal sealed class ScannerScreen : UserControl, IScreen
{
    private readonly Shell shell;
    private readonly SwitchRow wanted;
    private readonly Hint problem = new();
    private readonly Hint whenOff = new();
    private readonly StackPanel whenOn = new() { Spacing = 12 };
    private readonly StackPanel deviceList = new() { Spacing = 10 };
    private readonly Hint noDevices = new();
    private readonly Button connect;
    private readonly Hint network = new();
    private readonly Button allow;
    private readonly Button networkSettings;
    private readonly Hint reader = new();
    private readonly TextBlock address = new() { FontSize = 12, IsTextSelectionEnabled = true, TextWrapping = TextWrapping.Wrap };
    private readonly StackPanel documentList = new() { Spacing = 10 };
    private readonly Hint noDocuments = new();

    private string devicesShown = "";
    private string documentsShown = "";
    private string? openDocument;
    private bool pairing;
    private int polls;
    private DateTime notAnnouncedSince = DateTime.MaxValue;
    private NetworkState? pictured;

    public ScannerScreen(Shell shell)
    {
        this.shell = shell;
        var column = new StackPanel { Spacing = 16, Padding = new Thickness(24), MaxWidth = 980, HorizontalAlignment = HorizontalAlignment.Left };
        column.Children.Add(new ScreenHeader(T("Scanner"), T("Photograph pages with your phone or tablet. The text arrives here.")));

        wanted = new SwitchRow(T("Receive scans on this PC"), on =>
        {
            shell.Scanner.SetWanted(on);
            Refresh();
        });
        whenOff.Set(T("Turn this on, then connect your phone or tablet with a six-digit code. Both must be on the same Wi-Fi network."));
        noDevices.Set(T("No phone or tablet is connected yet."));
        connect = Ui.IconButton(Ui.IconAdd, T("Connect a phone or tablet…"), Pair);
        connect.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        allow = Ui.PlainButton(T("Allow devices on my network…"), async () =>
        {
            allow!.IsEnabled = false;
            await shell.AllowDevices();
            allow.IsEnabled = true;
            Refresh();
        });
        networkSettings = Ui.PlainButton(T("Open Windows Settings"), () => Shell.Open("ms-settings:network"));
        var development = new Expander
        {
            Header = T("For development"),
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Content = Ui.Column(
                8,
                new Hint(T("An Android device attached for debugging reaches the scanner at this address, once its localhost is led here with adb reverse.")),
                address),
        };
        foreach (var part in new UIElement[] { deviceList, noDevices, connect, network, allow, networkSettings, reader, development })
        {
            whenOn.Children.Add(part);
        }
        var devices = new Card(T("Phones and tablets"), Ui.IconPeople, wanted, problem, whenOn, whenOff);

        var steps = new Card(
            T("How it works"), Ui.IconIdea,
            Step(1, T("Open God's Camera on your phone or tablet and photograph the pages. Long pages can be taken in several overlapping photos.")),
            Step(2, T("Tap Send. The photos go to this PC over your Wi-Fi network, not through the internet.")),
            Step(3, T("This PC reads the text, joins the photos into one document, and shows it below.")));

        var top = new Grid { ColumnSpacing = 16 };
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        devices.VerticalAlignment = VerticalAlignment.Top;
        steps.VerticalAlignment = VerticalAlignment.Top;
        Grid.SetColumn(steps, 1);
        top.Children.Add(devices);
        top.Children.Add(steps);
        column.Children.Add(top);

        noDocuments.Set(T("Documents you scan appear here."));
        column.Children.Add(new Card(T("Documents"), Ui.IconDocument, noDocuments, documentList));
        Content = new ScrollViewer { Content = column, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
        Refresh();
    }

    private static Grid Step(int number, string words)
    {
        var grid = new Grid { ColumnSpacing = 10 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var badge = new Border
        {
            Width = 24,
            Height = 24,
            CornerRadius = new CornerRadius(12),
            Background = Ui.Brush("AccentFillColorSecondaryBrush"),
            VerticalAlignment = VerticalAlignment.Top,
            Child = new TextBlock
            {
                Text = number.ToString(CultureInfo.CurrentCulture),
                FontSize = 13,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = Ui.Brush("TextOnAccentFillColorPrimaryBrush"),
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
            },
        };
        var text = Ui.Text(words, 13);
        Grid.SetColumn(text, 1);
        grid.Children.Add(badge);
        grid.Children.Add(text);
        return grid;
    }

    /// <summary>
    /// For pictures of the app: show the screen as it is when the network is
    /// in this state, whatever state it is in. A copy on a data folder of its
    /// own never meets the firewall, so its pictures could not show those
    /// words otherwise. `null` goes back to the truth.
    /// </summary>
    public void Picture(NetworkState? network)
    {
        pictured = network;
        Refresh();
    }

    /// <summary>The window asks ten times a second; the scanner is asked twice a second.</summary>
    public void Update(AppState state)
    {
        if (polls++ % 5 == 0) Refresh();
    }

    private void Refresh()
    {
        ScannerState state;
        try
        {
            state = shell.Scanner.State();
        }
        catch (Exception)
        {
            // The window is closing and the scanner is gone.
            return;
        }
        if (pictured is { } forced) state = state with { Network = forced };
        wanted.Show(state.Wanted);
        problem.Visible(state.Problem != null);
        if (state.Problem != null)
        {
            problem.Set(
                state.Problem == ScannerProblem.NoToken
                    ? T("The scanner could not start. Windows did not let the app keep its key in Credential Manager.")
                    : T("The scanner could not start. Another program may be using its port."),
                Ui.IconWarning, Tone.Warning);
        }
        whenOn.Visible(state.Running);
        whenOff.Visible(!state.Running);
        if (state.Running) ShowDevices(state);
        ShowDocuments(state);
    }

    // Devices

    private void ShowDevices(ScannerState state)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        // The minute a device was last used in is part of what is shown.
        var key = string.Join("|", state.Devices.Select(device => $"{device.Id}:{device.Name}:{(device.LastSeen is { } seen ? (now - (long)seen) / 60 : -1)}"));
        if (key != devicesShown)
        {
            devicesShown = key;
            deviceList.Children.Clear();
            foreach (var device in state.Devices) deviceList.Children.Add(DeviceRow(device, now));
        }
        noDevices.Visible(state.Devices.Length == 0);
        connect.IsEnabled = state.Network == NetworkState.Ready;

        // Windows needs a moment to announce the hub; only a lasting silence is worth a line.
        if (state.Network != NetworkState.Ready || state.Announced) notAnnouncedSince = DateTime.MaxValue;
        else if (notAnnouncedSince == DateTime.MaxValue) notAnnouncedSince = DateTime.UtcNow;
        var silent = DateTime.UtcNow - notAnnouncedSince > TimeSpan.FromSeconds(8);

        allow.Visible(state.Network == NetworkState.FirewallNeeded && shell.CarriesNetworkScript);
        networkSettings.Visible(state.Network == NetworkState.PublicNetwork);
        switch (state.Network)
        {
            case NetworkState.FirewallNeeded:
                network.Set(
                    T("Windows Firewall does not let phones and tablets reach God's Camera yet. Allow it once, for private networks such as your home Wi-Fi. Windows asks for an administrator's permission."),
                    Ui.IconWarning, Tone.Warning);
                break;
            case NetworkState.PublicNetwork:
                network.Set(
                    T("Windows treats the network this PC is on as public, so phones and tablets cannot connect. Set your Wi-Fi to a private network in Windows Settings, then turn this switch off and on."),
                    Ui.IconWarning, Tone.Warning);
                break;
            case NetworkState.Failed:
                network.Set(
                    T("This PC could not open its network port, so devices cannot connect with a code. Another program may be using port {0}.", 47801),
                    Ui.IconWarning, Tone.Warning);
                break;
            default:
                network.Set(
                    silent
                        ? T("Windows has not announced this PC on the network, so a phone or tablet may not find it by itself.")
                        : T("Your phone or tablet finds this PC as “{0}”.", state.Name),
                    silent ? Ui.IconWarning : null, silent ? Tone.Warning : Tone.Plain);
                break;
        }
        reader.Visible(state.ReaderMissing != null);
        if (state.ReaderMissing != null)
        {
            reader.Set(
                T("This copy of the app cannot read text yet: the model files for reading are missing.") + (state.ReaderMissing.Length > 0 ? $" ({state.ReaderMissing})" : ""),
                Ui.IconWarning, Tone.Warning);
        }
        address.Text = state.LocalAddress;
    }

    private Grid DeviceRow(DeviceInfo device, long now)
    {
        var row = new Grid { ColumnSpacing = 10 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var icon = Ui.Icon(Ui.IconTablet, 22, Ui.Brush("AccentTextFillColorPrimaryBrush"));
        icon.VerticalAlignment = VerticalAlignment.Center;
        var name = Ui.Text(device.Name, 14, true);
        name.TextWrapping = TextWrapping.NoWrap;
        name.TextTrimming = TextTrimming.CharacterEllipsis;
        var words = Ui.Column(2, name, Ui.Secondary(LastSeen(device, now)));
        words.VerticalAlignment = VerticalAlignment.Center;
        var remove = Ui.PlainButton(T("Remove"), () => Remove(device));
        remove.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(words, 1);
        Grid.SetColumn(remove, 2);
        row.Children.Add(icon);
        row.Children.Add(words);
        row.Children.Add(remove);
        return row;
    }

    private static string LastSeen(DeviceInfo device, long now)
    {
        if (device.LastSeen is not { } seen) return T("Connected, not used yet");
        var ago = now - (long)seen;
        if (ago < 90) return T("Active now");
        if (ago < 2 * 60 * 60) return T("Last used {0} minutes ago", ago / 60);
        if (ago < 2 * 24 * 60 * 60) return T("Last used {0} hours ago", ago / 3600);
        return T("Last used on {0}", DateTimeOffset.FromUnixTimeSeconds((long)seen).LocalDateTime.ToString("d", CultureInfo.CurrentCulture));
    }

    private async void Remove(DeviceInfo device)
    {
        var words = Ui.Text(T("The device can no longer send scans to this PC. To use it again, connect it with a new code."));
        words.MaxWidth = 420;
        if (await shell.Ask(T("Remove {0}?", device.Name), words, T("Remove"), T("Cancel")))
        {
            shell.Scanner.RemoveDevice(device.Id);
            Refresh();
        }
    }

    // The code a device connects with

    public async void Pair()
    {
        if (pairing || !shell.Scanner.BeginPairing()) return;
        pairing = true;
        try
        {
            // One dialog per code: when a code has run out, the user may ask for the next.
            while (await ShowCode() && shell.Scanner.BeginPairing())
            {
            }
        }
        finally
        {
            shell.Scanner.EndPairing();
            pairing = false;
            Refresh();
        }
    }

    /// <summary>Show the code and follow what becomes of it. True if the user asks for a new code.</summary>
    private async Task<bool> ShowCode()
    {
        var body = new StackPanel { Spacing = 16, Width = 430 };
        var dialog = new ContentDialog { XamlRoot = shell.Root, Content = body, CloseButtonText = T("Cancel") };
        var remaining = Ui.Secondary("", 13);
        var shown = "";
        void Follow()
        {
            var state = shell.Scanner.State();
            var kind = state.JustPaired != null ? "paired" : state.PairingCode != null ? state.PairingCode : state.PairingEnded ? "ended" : "gone";
            if (state.PairingCode != null)
            {
                var left = state.PairingSecondsLeft;
                remaining.Text = T("The code works for {0} more and for one device.", $"{left / 60}:{left % 60:00}");
            }
            if (kind == shown) return;
            shown = kind;
            body.Children.Clear();
            dialog.PrimaryButtonText = "";
            if (state.JustPaired != null)
            {
                var done = Ui.Icon(Ui.IconGood, 48, new SolidColorBrush(Ui.Green));
                done.HorizontalAlignment = HorizontalAlignment.Center;
                body.Children.Add(done);
                body.Children.Add(Centred(Ui.Text(T("{0} is connected", state.JustPaired), 20, true)));
                body.Children.Add(Centred(Ui.Secondary(T("It can send scans to this PC whenever both are on the same Wi-Fi network."), 14)));
                dialog.CloseButtonText = T("Done");
            }
            else if (state.PairingCode != null)
            {
                body.Children.Add(Ui.Text(T("Connect a phone or tablet"), 20, true));
                body.Children.Add(Step(1, T("Open God's Camera on the phone or tablet, on the same Wi-Fi network as this PC.")));
                body.Children.Add(Step(2, T("Tap “Connect to a computer” and choose “{0}”.", state.Name)));
                body.Children.Add(Step(3, T("Enter this code there.")));
                body.Children.Add(Digits(state.PairingCode));
                body.Children.Add(Centred(remaining));
                if (state.Addresses.Length > 0)
                {
                    body.Children.Add(Ui.Secondary(T("If this PC is not in the list there, tap “Enter the computer's address instead” and enter {0}.", state.Addresses[0]), 13));
                }
                dialog.CloseButtonText = T("Cancel");
            }
            else if (state.PairingEnded)
            {
                var late = Ui.Icon(Ui.IconWarning, 42, new SolidColorBrush(Ui.Orange));
                late.HorizontalAlignment = HorizontalAlignment.Center;
                body.Children.Add(late);
                body.Children.Add(Centred(Ui.Text(T("This code no longer works"), 20, true)));
                body.Children.Add(Centred(Ui.Secondary(T("It ran out, or a wrong code was entered too many times."), 14)));
                dialog.PrimaryButtonText = T("Show a new code");
                dialog.DefaultButton = ContentDialogButton.Primary;
                dialog.CloseButtonText = T("Cancel");
            }
            else
            {
                // The scanner was turned off under the dialog.
                dialog.Hide();
            }
        }
        var timer = DispatcherQueue.CreateTimer();
        timer.Interval = TimeSpan.FromMilliseconds(500);
        timer.Tick += (_, _) => Follow();
        Follow();
        timer.Start();
        var answer = await dialog.ShowAsync();
        timer.Stop();
        return answer == ContentDialogResult.Primary && shown == "ended";
    }

    private static TElement Centred<TElement>(TElement element) where TElement : FrameworkElement
    {
        element.HorizontalAlignment = HorizontalAlignment.Center;
        if (element is TextBlock text) text.TextAlignment = TextAlignment.Center;
        return element;
    }

    /// <summary>The six digits, each in a box of its own, in two groups of three.</summary>
    private static StackPanel Digits(string code)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Center };
        var place = 0;
        foreach (var digit in code.Where(char.IsDigit))
        {
            row.Children.Add(new Border
            {
                Width = 50,
                Height = 62,
                CornerRadius = new CornerRadius(10),
                BorderThickness = new Thickness(1),
                BorderBrush = Ui.Brush("CardStrokeColorDefaultBrush"),
                Background = Ui.Brush("ControlFillColorDefaultBrush"),
                Margin = new Thickness(0, 0, place == 2 ? 12 : 0, 0),
                Child = new TextBlock
                {
                    Text = digit.ToString(),
                    FontSize = 36,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    HorizontalAlignment = HorizontalAlignment.Center,
                    VerticalAlignment = VerticalAlignment.Center,
                },
            });
            place++;
        }
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(row, code);
        return row;
    }

    // Documents

    private void ShowDocuments(ScannerState state)
    {
        noDocuments.Visible(state.Documents.Length == 0);
        var key = string.Join("|", state.Documents.Select(document => $"{document.Id}:{document.Status}:{(int)(document.Progress * 100)}:{document.Title}")) + "/" + openDocument;
        if (key == documentsShown) return;
        documentsShown = key;
        documentList.Children.Clear();
        foreach (var document in state.Documents)
        {
            if (documentList.Children.Count > 0)
            {
                documentList.Children.Add(new Border { Height = 1, Background = Ui.Brush("DividerStrokeColorDefaultBrush") });
            }
            documentList.Children.Add(DocumentRow(document));
            if (openDocument == document.Id && document.Status == DocumentStatus.Done)
            {
                documentList.Children.Add(new Border
                {
                    CornerRadius = new CornerRadius(8),
                    BorderThickness = new Thickness(1),
                    BorderBrush = Ui.Brush("CardStrokeColorDefaultBrush"),
                    Background = Ui.Brush("ControlFillColorDefaultBrush"),
                    MaxHeight = 280,
                    Child = new ScrollViewer
                    {
                        VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                        Padding = new Thickness(12),
                        Content = new TextBlock { Text = shell.Scanner.DocumentText(document.Id), TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true },
                    },
                });
            }
        }
    }

    private static string Title(DocumentInfo document) => document.Status switch
    {
        DocumentStatus.Done => document.Title.Length == 0 ? T("Document without text") : document.Title,
        DocumentStatus.Failed => T("Could not be read"),
        _ => T("Reading {0} %…", (int)(document.Progress * 100)),
    };

    private Grid DocumentRow(DocumentInfo document)
    {
        var row = new Grid { ColumnSpacing = 10 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var icon = Ui.Icon(document.Status == DocumentStatus.Failed ? Ui.IconWarning : Ui.IconDocument, 20, Ui.Brush("TextFillColorSecondaryBrush"));
        icon.VerticalAlignment = VerticalAlignment.Center;
        var title = Ui.Text(Title(document), 14, true);
        title.TextWrapping = TextWrapping.NoWrap;
        title.TextTrimming = TextTrimming.CharacterEllipsis;
        var received = DateTimeOffset.FromUnixTimeSeconds((long)document.ReceivedAt).LocalDateTime.ToString("g", CultureInfo.CurrentCulture);
        var words = Ui.Column(2, title, Ui.Secondary(received));
        words.VerticalAlignment = VerticalAlignment.Center;

        var id = document.Id;
        StackPanel buttons;
        if (document.Status == DocumentStatus.Done)
        {
            var open = openDocument == id;
            var read = Ui.PlainButton(open ? T("Hide") : T("Read"), () =>
            {
                openDocument = open ? null : id;
                Refresh();
            });
            var copy = Ui.PlainButton(T("Copy the text"), () => Copy(shell.Scanner.DocumentText(id)));
            var menu = new MenuFlyout();
            var file = System.IO.Path.Combine(document.Folder, "document.txt");
            var openFile = new MenuFlyoutItem { Text = T("Open the document") };
            openFile.Click += (_, _) => Shell.Open(file);
            var show = new MenuFlyoutItem { Text = T("Show in File Explorer") };
            show.Click += (_, _) => Shell.ShowInExplorer(file);
            var delete = new MenuFlyoutItem { Text = T("Delete") };
            delete.Click += (_, _) => Delete(id);
            menu.Items.Add(openFile);
            menu.Items.Add(show);
            menu.Items.Add(new MenuFlyoutSeparator());
            menu.Items.Add(delete);
            var more = new Button { Content = Ui.Icon(Ui.IconMore, 14), Flyout = menu };
            ToolTipService.SetToolTip(more, T("More for this document"));
            buttons = Ui.Row(8, read, copy, more);
        }
        else
        {
            var delete = new Button { Content = Ui.Icon(Ui.IconDelete, 14) };
            delete.Click += (_, _) => Delete(id);
            ToolTipService.SetToolTip(delete, T("Delete this document"));
            buttons = document.Status == DocumentStatus.Failed
                ? Ui.Row(8, delete)
                : Ui.Row(8, new ProgressBar { Width = 120, Minimum = 0, Maximum = 1, Value = document.Progress }, delete);
        }
        buttons.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(words, 1);
        Grid.SetColumn(buttons, 2);
        row.Children.Add(icon);
        row.Children.Add(words);
        row.Children.Add(buttons);
        return row;
    }

    private void Delete(string id)
    {
        if (openDocument == id) openDocument = null;
        shell.Scanner.DeleteDocument(id);
        Refresh();
    }

    private static void Copy(string text)
    {
        var package = new Windows.ApplicationModel.DataTransfer.DataPackage();
        package.SetText(text);
        Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(package);
    }
}
