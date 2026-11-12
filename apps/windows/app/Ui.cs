using Microsoft.UI;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Storage.Streams;
using Windows.UI;

namespace GodsCamera;

/// <summary>How a line of explanation is coloured.</summary>
internal enum Tone { Plain, Good, Warning, Bad }

/// <summary>The small building blocks every screen is made of.</summary>
internal static class Ui
{
    // Segoe Fluent Icons, which every Windows 11 has.
    public const string IconCamera = "\uE714";
    public const string IconLoop = "\uE8EE";
    public const string IconFace = "\uE77B";
    public const string IconScanner = "\uE8FE";
    public const string IconSettings = "\uE713";
    public const string IconPeople = "\uE716";
    public const string IconTablet = "\uE70A";
    public const string IconDocument = "\uE8A5";
    public const string IconApps = "\uE71D";
    public const string IconRecord = "\uE7C8";
    public const string IconStop = "\uE71A";
    public const string IconPlay = "\uE768";
    public const string IconVideo = "\uE714";
    public const string IconFilm = "\uE8B2";
    public const string IconRepeat = "\uE8EE";
    public const string IconSaved = "\uE8F1";
    public const string IconAdd = "\uE710";
    public const string IconDelete = "\uE74D";
    public const string IconMore = "\uE712";
    public const string IconCheck = "\uE73E";
    public const string IconGood = "\uE930";
    public const string IconWarning = "\uE7BA";
    public const string IconError = "\uEA39";
    public const string IconInfo = "\uE946";
    public const string IconIdea = "\uEA80";
    public const string IconWand = "\uE771";
    public const string IconHidden = "\uED1A";
    public const string IconLock = "\uE72E";
    public const string IconHelp = "\uE897";
    public const string IconNoVideo = "\uE714";
    public const string IconCircle = "\uEA3A";
    public const string IconCheckedCircle = "\uE930";
    public const string IconWrench = "\uE90F";

    public static Brush Brush(string name) => (Brush)Application.Current.Resources[name];

    public static Brush ToneBrush(Tone tone) => tone switch
    {
        Tone.Good => Brush("SystemFillColorSuccessBrush"),
        Tone.Warning => Brush("SystemFillColorCautionBrush"),
        Tone.Bad => Brush("SystemFillColorCriticalBrush"),
        _ => Brush("TextFillColorSecondaryBrush"),
    };

    public static FontIcon Icon(string glyph, double size = 16, Brush? brush = null)
    {
        var icon = new FontIcon { Glyph = glyph, FontSize = size };
        if (brush != null) icon.Foreground = brush;
        return icon;
    }

    public static TextBlock Text(string text, double size = 14, bool strong = false, Brush? brush = null)
    {
        var block = new TextBlock { Text = text, FontSize = size, TextWrapping = TextWrapping.Wrap };
        if (strong) block.FontWeight = FontWeights.SemiBold;
        if (brush != null) block.Foreground = brush;
        return block;
    }

    public static TextBlock Secondary(string text, double size = 12) => Text(text, size, false, Brush("TextFillColorSecondaryBrush"));

    public static StackPanel Column(double spacing, params UIElement[] children)
    {
        var panel = new StackPanel { Spacing = spacing };
        foreach (var child in children) panel.Children.Add(child);
        return panel;
    }

    public static StackPanel Row(double spacing, params UIElement[] children)
    {
        var panel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = spacing };
        foreach (var child in children)
        {
            if (child is FrameworkElement element) element.VerticalAlignment = VerticalAlignment.Center;
            panel.Children.Add(child);
        }
        return panel;
    }

    /// <summary>`left` at the left, `right` at the right edge, on one line.</summary>
    public static Grid Spread(UIElement left, UIElement right)
    {
        var grid = new Grid { ColumnSpacing = 10 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        if (left is FrameworkElement l) l.VerticalAlignment = VerticalAlignment.Center;
        if (right is FrameworkElement r) r.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn((FrameworkElement)right, 1);
        grid.Children.Add(left);
        grid.Children.Add(right);
        return grid;
    }

    /// <summary>
    /// A row to click: `leading` at the left and `words` beside it, taking the
    /// rest of the width and wrapping inside it.
    /// </summary>
    public static Button RowButton(UIElement leading, UIElement words, Action clicked)
    {
        var content = new Grid { ColumnSpacing = 10 };
        content.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        content.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        if (leading is FrameworkElement first) first.VerticalAlignment = VerticalAlignment.Center;
        if (words is FrameworkElement second)
        {
            second.VerticalAlignment = VerticalAlignment.Center;
            Grid.SetColumn(second, 1);
        }
        content.Children.Add(leading);
        content.Children.Add(words);
        var button = new Button
        {
            Content = content,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            Padding = new Thickness(4),
        };
        button.Click += (_, _) => clicked();
        return button;
    }

    /// <summary>A button with an icon before its words.</summary>
    public static Button IconButton(string glyph, string text, Action clicked)
    {
        var button = new Button { Content = Row(8, Icon(glyph, 15), new TextBlock { Text = text }) };
        button.Click += (_, _) => clicked();
        return button;
    }

    public static Button PlainButton(string text, Action clicked)
    {
        var button = new Button { Content = text };
        button.Click += (_, _) => clicked();
        return button;
    }

    public static void Visible(this UIElement element, bool visible) =>
        element.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>A small picture from PNG bytes, or nothing if there are none.</summary>
    public static async Task<ImageSource?> Picture(byte[] png)
    {
        if (png.Length == 0) return null;
        try
        {
            using var stream = new InMemoryRandomAccessStream();
            using (var writer = new DataWriter(stream.GetOutputStreamAt(0)))
            {
                writer.WriteBytes(png);
                await writer.StoreAsync();
                await writer.FlushAsync();
                writer.DetachStream();
            }
            var bitmap = new BitmapImage();
            await bitmap.SetSourceAsync(stream);
            return bitmap;
        }
        catch (Exception)
        {
            return null;
        }
    }

    public static Color Rgb(byte r, byte g, byte b, byte a = 255) => Color.FromArgb(a, r, g, b);

    public static readonly Color Green = Rgb(0x1E, 0x9E, 0x4A);
    public static readonly Color Red = Rgb(0xD1, 0x34, 0x38);
    public static readonly Color Grey = Rgb(0x70, 0x70, 0x70);
    public static readonly Color Purple = Rgb(0x8A, 0x4F, 0xD8);
    public static readonly Color Indigo = Rgb(0x4F, 0x5B, 0xD5);
    public static readonly Color Orange = Rgb(0xE0, 0x7A, 0x10);
}

/// <summary>
/// A switch with its words beside it. The words wrap; the switch's own label
/// would be cut off instead.
/// </summary>
internal sealed class SwitchRow : Grid
{
    private readonly ToggleSwitch toggle = new() { OnContent = "", OffContent = "", MinWidth = 0, Margin = new Thickness(0, -4, 0, -4) };
    private readonly TextBlock words = new() { TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
    private bool shown;

    public SwitchRow(string text, Action<bool> changed)
    {
        ColumnSpacing = 4;
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(52) });
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        words.Text = text;
        SetColumn(words, 1);
        Children.Add(toggle);
        Children.Add(words);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(toggle, text);
        // The event also fires when the screen sets the switch from the app's
        // state; only a change away from what was shown is the user's.
        toggle.Toggled += (_, _) =>
        {
            if (toggle.IsOn == shown) return;
            shown = toggle.IsOn;
            changed(toggle.IsOn);
        };
        // The words are part of the switch: a click on them flips it.
        words.Tapped += (_, _) => toggle.IsOn = !toggle.IsOn;
    }

    public bool IsOn => toggle.IsOn;

    public double TextSize
    {
        set => words.FontSize = value;
    }

    /// <summary>Set the switch from the app's state without telling the app about it.</summary>
    public void Show(bool on)
    {
        shown = on;
        if (toggle.IsOn != on) toggle.IsOn = on;
    }
}

/// <summary>A rounded panel that groups related controls under a heading.</summary>
internal sealed class Card : Grid
{
    public StackPanel Body { get; } = new() { Spacing = 12 };

    public Card(string title, string glyph, params UIElement[] children)
    {
        HorizontalAlignment = HorizontalAlignment.Stretch;
        var heading = Ui.Row(8, Ui.Icon(glyph, 16, Ui.Brush("AccentTextFillColorPrimaryBrush")), Ui.Text(title, 15, true));
        var panel = new StackPanel { Spacing = 12 };
        panel.Children.Add(heading);
        panel.Children.Add(Body);
        foreach (var child in children) Body.Children.Add(child);
        Children.Add(new Border
        {
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(16),
            BorderThickness = new Thickness(1),
            Background = Ui.Brush("CardBackgroundFillColorDefaultBrush"),
            BorderBrush = Ui.Brush("CardStrokeColorDefaultBrush"),
            Child = panel,
        });
    }
}

/// <summary>A line of explanation under a control, with an optional icon.</summary>
internal sealed class Hint : Grid
{
    private readonly FontIcon icon = new() { FontSize = 14, VerticalAlignment = VerticalAlignment.Top, Margin = new Thickness(0, 2, 0, 0) };
    private readonly TextBlock text = new() { FontSize = 13, TextWrapping = TextWrapping.Wrap };

    public Hint(string words = "", string? glyph = null, Tone tone = Tone.Plain)
    {
        ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        SetColumn(text, 1);
        Children.Add(icon);
        Children.Add(text);
        Set(words, glyph, tone);
    }

    public void Set(string words, string? glyph = null, Tone tone = Tone.Plain)
    {
        if (text.Text != words) text.Text = words;
        var brush = Ui.ToneBrush(tone);
        text.Foreground = brush;
        icon.Foreground = brush;
        icon.Visible(glyph != null);
        // Without an icon the words start at the edge, in line with what is above them.
        ColumnSpacing = glyph != null ? 7 : 0;
        if (glyph != null && icon.Glyph != glyph) icon.Glyph = glyph;
    }
}

/// <summary>The title and one-line description at the top of a screen.</summary>
internal sealed class ScreenHeader : StackPanel
{
    public ScreenHeader(string title, string subtitle)
    {
        Spacing = 4;
        Children.Add(new TextBlock { Text = title, FontSize = 30, FontWeight = FontWeights.Bold });
        Children.Add(Ui.Text(subtitle, 14, false, Ui.Brush("TextFillColorSecondaryBrush")));
    }
}

/// <summary>
/// A screen with the live picture on the left and its controls in a column on the right.
/// </summary>
internal sealed class StudioLayout : Grid
{
    public StackPanel Under { get; } = new() { Spacing = 14 };
    public StackPanel Controls { get; } = new() { Spacing = 14, Padding = new Thickness(18) };

    public StudioLayout(string title, string subtitle, PreviewPane preview)
    {
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star), MinWidth = 480 });
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(360) });
        var left = new StackPanel { Spacing = 18, Padding = new Thickness(24) };
        left.Children.Add(new ScreenHeader(title, subtitle));
        left.Children.Add(preview);
        left.Children.Add(Under);
        Children.Add(new ScrollViewer { Content = left, VerticalScrollBarVisibility = ScrollBarVisibility.Auto });
        var right = new Border
        {
            BorderBrush = Ui.Brush("DividerStrokeColorDefaultBrush"),
            BorderThickness = new Thickness(1, 0, 0, 0),
            Child = new ScrollViewer { Content = Controls, VerticalScrollBarVisibility = ScrollBarVisibility.Auto },
        };
        SetColumn(right, 1);
        Children.Add(right);
    }
}
