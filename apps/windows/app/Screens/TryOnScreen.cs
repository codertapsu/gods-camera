using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>
/// Try-on (plan, Phase 9): a garment on a photo of a person, or another
/// hairstyle, made as a picture by the image model chosen in Settings. The
/// photos go to that model only where Settings allows the cloud for try-on.
/// </summary>
internal sealed class TryOnScreen : UserControl, IScreen
{
    private static readonly string[] PhotoTypes = { ".jpg", ".jpeg", ".png" };
    private static readonly string[] Styles =
    {
        T("Short bob"), T("Long soft waves"), T("Pixie cut"), T("Curly, shoulder length"), T("Buzz cut"), T("Ponytail"),
        T("Blonde"), T("Chestnut brown"), T("Black"), T("Silver grey"),
    };

    private readonly Shell shell;
    private string? person;
    private string? garment;
    private readonly Image personPicture = new() { Stretch = Stretch.UniformToFill };
    private readonly Image garmentPicture = new() { Stretch = Stretch.UniformToFill };
    private readonly TextBox style = new() { PlaceholderText = T("For example: short curly hair, dark brown"), HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly Button tryOn;
    private readonly Button showHair;

    private readonly Image made = new() { Stretch = Stretch.Uniform, MaxHeight = 520 };
    private readonly Hint status = new();
    private readonly StackPanel working;
    private readonly StackPanel gallery = new() { Orientation = Orientation.Horizontal, Spacing = 8 };
    private string galleryKey = "";
    private string? shown;

    private ulong task;
    private readonly DispatcherQueueTimer follow;

    public TryOnScreen(Shell shell)
    {
        this.shell = shell;
        var column = new StackPanel { Spacing = 16, Padding = new Thickness(24), MaxWidth = 1000, HorizontalAlignment = HorizontalAlignment.Left };
        column.Children.Add(new ScreenHeader(T("Try-on"), T("See a garment or a hairstyle on a photo before you buy it or cut it.")));

        Border Frame(Image image) => new()
        {
            Width = 150,
            Height = 190,
            CornerRadius = new CornerRadius(12),
            Background = Ui.Brush("ControlFillColorSecondaryBrush"),
            Child = image,
        };
        var choosePerson = Ui.IconButton(Ui.IconFace, T("Photo of the person…"), async () =>
        {
            var files = await shell.PickFiles(false, PhotoTypes);
            if (files.Count == 0) return;
            person = files[0];
            personPicture.Source = new BitmapImage(new Uri(person));
            Refresh();
        });
        var chooseGarment = Ui.IconButton(Ui.IconShirt, T("Photo of the garment…"), async () =>
        {
            var files = await shell.PickFiles(false, PhotoTypes);
            if (files.Count == 0) return;
            garment = files[0];
            garmentPicture.Source = new BitmapImage(new Uri(garment));
            Refresh();
        });
        tryOn = Ui.PlainButton(T("Try it on"), () =>
        {
            if (person != null && garment != null) Start(new PictureTask.TryOn(person, garment));
        });
        tryOn.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        var photos = Ui.Row(16, Ui.Column(8, Frame(personPicture), choosePerson), Ui.Column(8, Frame(garmentPicture), chooseGarment));
        column.Children.Add(new Card(
            T("A garment"), Ui.IconShirt, photos,
            new Hint(T("A photo of the person from head to knee, standing straight, works best; the garment on its own, laid flat or on a hanger.")),
            tryOn));

        var chips = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        foreach (var preset in Styles)
        {
            var words = preset;
            var chip = Ui.PlainButton(words, () => style.Text = style.Text.Trim().Length == 0 ? words : style.Text.Trim() + ", " + words);
            chip.Padding = new Thickness(10, 4, 10, 4);
            chips.Children.Add(chip);
        }
        showHair = Ui.PlainButton(T("Show me"), () =>
        {
            if (person != null && style.Text.Trim().Length > 0) Start(new PictureTask.Hairstyle(person, style.Text.Trim()));
        });
        showHair.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        style.TextChanged += (_, _) => Refresh();
        column.Children.Add(new Card(
            T("A hairstyle"), Ui.IconWand,
            new Hint(T("Uses the photo of the person chosen above.")),
            style,
            new ScrollViewer { Content = chips, HorizontalScrollMode = ScrollMode.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollMode = ScrollMode.Disabled },
            showHair));

        working = Ui.Row(10, new ProgressRing { Width = 18, Height = 18, IsActive = true }, Ui.Secondary(T("Making the picture. This takes up to a minute."), 13));
        var open = Ui.PlainButton(T("Open"), () => { if (shown != null) Shell.Open(shown); });
        var showFile = Ui.PlainButton(T("Show in File Explorer"), () => { if (shown != null) Shell.ShowInExplorer(shown); });
        var delete = Ui.PlainButton(T("Delete"), () =>
        {
            if (shown == null) return;
            try { File.Delete(shown); } catch (IOException) { }
            shown = null;
            made.Source = null;
            galleryKey = "";
            Refresh();
        });
        column.Children.Add(new Card(
            T("Pictures"), Ui.IconPicture, working, status, made, Ui.Row(8, open, showFile, delete),
            new ScrollViewer { Content = gallery, HorizontalScrollMode = ScrollMode.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollMode = ScrollMode.Disabled },
            new Hint(T("Use photos of yourself, or of someone who agreed to it. The pictures are made by the model chosen in Settings and kept on this PC."), Ui.IconLock)));
        Content = new ScrollViewer { Content = column, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };

        follow = DispatcherQueue.GetForCurrentThread().CreateTimer();
        follow.Interval = TimeSpan.FromMilliseconds(400);
        follow.Tick += (_, _) => Follow();
        working.Visible(false);
        Refresh();
    }

    private int ticks;

    public void Update(AppState state)
    {
        if (++ticks % 10 == 1) Refresh();
    }

    private string PicturesFolder => System.IO.Path.Combine(shell.DataFolder, "Assistant", "Pictures");

    private void Refresh()
    {
        var busy = task != 0 && shell.Assistant.Task(task).Running;
        tryOn.IsEnabled = !busy && person != null && garment != null;
        showHair.IsEnabled = !busy && person != null && style.Text.Trim().Length > 0;
        if (!busy && shown == null && status.Visibility == Visibility.Collapsed)
        {
            var model = shell.Assistant.ImageModel();
            status.Visible(true);
            status.Set(model == null
                ? T("No picture model yet. Enter a key for OpenAI or Gemini in Settings, under AI models.")
                : shell.Assistant.CloudAllowed(AiFeature.TryOn)
                    ? T("Pictures are made by {0} · {1}.", model.Provider, model.Model)
                    : T("Try-on sends the photos to {0}. Allow that in Settings, under AI models, first.", model.Provider));
        }
        var files = Directory.Exists(PicturesFolder)
            ? Directory.GetFiles(PicturesFolder, "*.png").OrderByDescending(File.GetLastWriteTimeUtc).Take(12).ToArray()
            : Array.Empty<string>();
        var key = string.Join("|", files);
        if (key == galleryKey) return;
        galleryKey = key;
        gallery.Children.Clear();
        foreach (var file in files)
        {
            var thumbnail = new Image { Source = new BitmapImage(new Uri(file)) { DecodePixelWidth = 160 }, Width = 80, Height = 100, Stretch = Stretch.UniformToFill };
            var button = new Button { Content = thumbnail, Padding = new Thickness(2) };
            var path = file;
            button.Click += (_, _) => Show(path);
            gallery.Children.Add(button);
        }
        if (shown == null && files.Length > 0) Show(files[0]);
    }

    private void Show(string file)
    {
        shown = file;
        made.Source = new BitmapImage(new Uri(file));
    }

    private void Start(PictureTask what)
    {
        if (task != 0) shell.Assistant.Forget(task);
        task = shell.Assistant.StartPicture(what);
        working.Visible(true);
        status.Visible(false);
        tryOn.IsEnabled = showHair.IsEnabled = false;
        follow.Start();
    }

    private void Follow()
    {
        var state = shell.Assistant.Task(task);
        if (state.Running) return;
        follow.Stop();
        working.Visible(false);
        if (state.Problem != null)
        {
            status.Visible(true);
            status.Set(AssistantScreen.ProblemWords(state.Problem), Ui.IconWarning, Tone.Warning);
        }
        else if (state.Result?.Picture is string picture)
        {
            status.Visible(false);
            galleryKey = "";
            Show(picture);
        }
        Refresh();
    }
}
