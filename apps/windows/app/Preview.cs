using System.Runtime.InteropServices;
using System.Runtime.InteropServices.WindowsRuntime;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera;

/// <summary>
/// The outgoing picture as something the window can show. One of these serves
/// every screen: they all show the same picture.
/// </summary>
internal sealed class PreviewSource
{
    // The core copies the picture straight into this memory: four bytes a
    // pixel, blue, green, red, unused. Room for the largest picture it makes.
    private readonly byte[] pixels = new byte[1920 * 1080 * 4];
    private ulong shown;

    public WriteableBitmap? Bitmap { get; private set; }

    /// <summary>Raised when the picture's size changed and `Bitmap` is a new one.</summary>
    public event Action? BitmapChanged;

    [DllImport("gc_ffi", CallingConvention = CallingConvention.Cdecl)]
    private static extern unsafe ulong gc_app_preview(byte* into, ulong capacity, uint* width, uint* height);

    /// <summary>Fetch the newest picture, if there is a newer one than the one shown.</summary>
    public unsafe void Refresh()
    {
        uint width = 0, height = 0;
        ulong number;
        fixed (byte* into = pixels)
        {
            number = gc_app_preview(into, (ulong)pixels.Length, &width, &height);
        }
        if (number == 0 || number == shown || width == 0 || height == 0) return;
        shown = number;
        if (Bitmap == null || Bitmap.PixelWidth != width || Bitmap.PixelHeight != height)
        {
            Bitmap = new WriteableBitmap((int)width, (int)height);
            BitmapChanged?.Invoke();
        }
        using (var stream = Bitmap.PixelBuffer.AsStream())
        {
            stream.Write(pixels, 0, (int)(width * height * 4));
        }
        Bitmap.Invalidate();
    }
}

/// <summary>The live picture with its state drawn on it. Shared by the screens that show it.</summary>
internal sealed class PreviewPane : Grid
{
    private readonly Image image = new() { Stretch = Stretch.UniformToFill };
    private readonly StackPanel badges = new() { Orientation = Orientation.Horizontal, Spacing = 6 };
    private readonly Border rate;
    private readonly TextBlock rateText = new() { FontSize = 12, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) };
    private readonly StackPanel empty;
    private readonly TextBlock emptyText = new() { FontSize = 14, TextWrapping = TextWrapping.Wrap, TextAlignment = TextAlignment.Center, Foreground = new SolidColorBrush(Ui.Rgb(255, 255, 255, 216)) };
    private string badgeKey = "";

    public PreviewPane(PreviewSource source)
    {
        // Sixteen to nine, whatever the width.
        SizeChanged += (_, e) => Height = e.NewSize.Width * 9 / 16;
        var frame = new Border
        {
            CornerRadius = new CornerRadius(12),
            Background = new SolidColorBrush(Microsoft.UI.Colors.Black),
            Child = image,
        };
        Children.Add(frame);

        empty = Ui.Column(10, Ui.Icon(Ui.IconNoVideo, 34, new SolidColorBrush(Ui.Rgb(255, 255, 255, 178))), emptyText);
        empty.HorizontalAlignment = HorizontalAlignment.Center;
        empty.VerticalAlignment = VerticalAlignment.Center;
        empty.Padding = new Thickness(24);
        Children.Add(empty);

        badges.HorizontalAlignment = HorizontalAlignment.Left;
        badges.VerticalAlignment = VerticalAlignment.Top;
        badges.Margin = new Thickness(10);
        Children.Add(badges);

        rate = new Border
        {
            CornerRadius = new CornerRadius(10),
            Background = new SolidColorBrush(Ui.Rgb(0, 0, 0, 115)),
            Padding = new Thickness(8, 3, 8, 3),
            Margin = new Thickness(10),
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Bottom,
            Child = rateText,
        };
        Children.Add(rate);

        image.Source = source.Bitmap;
        source.BitmapChanged += () => image.Source = source.Bitmap;
    }

    private static Border Badge(string text, Windows.UI.Color colour) => new()
    {
        CornerRadius = new CornerRadius(10),
        Background = new SolidColorBrush(Windows.UI.Color.FromArgb(235, colour.R, colour.G, colour.B)),
        Padding = new Thickness(9, 3, 9, 4),
        Child = new TextBlock { Text = text, FontSize = 11, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) },
    };

    public void Update(AppState state)
    {
        var idle = state.Mode == EngineMode.Idle;
        image.Visible(!idle);
        empty.Visible(idle);
        if (idle)
        {
            emptyText.Text = state.SourceProblem switch
            {
                SourceProblem.NoFileChosen => T("Choose a file to show."),
                SourceProblem.FileUnreadable => T("That file cannot be shown."),
                SourceProblem.CameraNotAllowed => T("God's Camera needs your permission to use the camera."),
                SourceProblem.NoCamera => T("No camera is attached to this PC."),
                SourceProblem.CameraRefused => T("Windows does not let desktop apps use cameras. Turn that on under Privacy & security, Camera."),
                SourceProblem.CameraUnavailable => T("The camera cannot be used. Another app may have it, or it was unplugged."),
                _ => T("Waiting for the picture…"),
            };
        }

        // The badges are made anew only when what they say changes.
        var recorded = state.IsRecording ? (int)state.RecordedSeconds : -1;
        var key = $"{state.Mode}|{state.Swapping}|{state.PictureHidden}|{recorded}|{state.Listening}";
        if (key != badgeKey)
        {
            badgeKey = key;
            badges.Children.Clear();
            switch (state.Mode)
            {
                case EngineMode.Looping:
                    badges.Children.Add(Badge(T("LOOPING"), Ui.Red));
                    if (state.Listening) badges.Children.Add(Badge(T("LISTENING"), Ui.Indigo));
                    break;
                case EngineMode.Live:
                    badges.Children.Add(Badge(T("LIVE"), Ui.Green));
                    if (state.Swapping) badges.Children.Add(Badge(T("AI FACE"), Ui.Purple));
                    if (state.PictureHidden) badges.Children.Add(Badge(T("BLURRED"), Ui.Indigo));
                    break;
                default:
                    badges.Children.Add(Badge(T("NO PICTURE"), Ui.Grey));
                    break;
            }
            if (state.IsRecording) badges.Children.Add(Badge(T("REC {0} s", recorded), Ui.Orange));
        }

        rate.Visible(!idle && state.FramesOut > 0 && state.SourceFps > 0);
        rateText.Text = T("{0} pictures a second", Math.Round(state.SourceFps));
    }
}
