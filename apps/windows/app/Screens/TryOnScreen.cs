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
    private FaceConsent personConsent = FaceConsent.Myself;
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

    private readonly StackPanel wardrobe = new() { Orientation = Orientation.Horizontal, Spacing = 10 };
    private readonly Hint wardrobeEmpty = new();
    private string wardrobeKey = "";

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
            // As with a face: a photo of a person is used only once the user says whose it is.
            if (await AskWhose() is not FaceConsent consent) return;
            person = files[0];
            personConsent = consent;
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
            if (person != null && garment != null) Start(new PictureTask.TryOn(person, garment, personConsent));
        });
        tryOn.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        var photos = Ui.Row(16, Ui.Column(8, Frame(personPicture), choosePerson), Ui.Column(8, Frame(garmentPicture), chooseGarment));
        column.Children.Add(new Card(
            T("A garment"), Ui.IconShirt, photos,
            new Hint(T("A photo of the person from head to knee, standing straight, works best; the garment on its own, laid flat or on a hanger.")),
            tryOn));

        // The wardrobe: garments kept for trying on, named and tagged by the model.
        var addGarments = Ui.IconButton(Ui.IconAdd, T("Add to the wardrobe…"), async () =>
        {
            var files = await shell.PickFiles(true, PhotoTypes);
            foreach (var file in files)
            {
                var problem = shell.Assistant.AddToWardrobe(file);
                if (problem != null) Log.Write($"adding to the wardrobe: {problem}");
            }
            wardrobeKey = "";
            Refresh();
        });
        wardrobeEmpty.Set(T("Keep photos of your clothes here, to try them on with one click. A model names and tags each one, where Settings lets it see the photos."));
        column.Children.Add(new Card(
            T("Wardrobe"), Ui.IconSaved, wardrobeEmpty,
            new ScrollViewer { Content = wardrobe, HorizontalScrollMode = ScrollMode.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollMode = ScrollMode.Disabled },
            addGarments));

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
            if (person != null && style.Text.Trim().Length > 0) Start(new PictureTask.Hairstyle(person, style.Text.Trim(), personConsent));
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
            try
            {
                File.Delete(shown);
                File.Delete(System.IO.Path.ChangeExtension(shown, ".json"));
            }
            catch (IOException) { }
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
        RefreshWardrobe();
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

    /// <summary>The garments kept, each a button that chooses it for trying on.</summary>
    private void RefreshWardrobe()
    {
        var items = shell.Assistant.Wardrobe();
        var key = string.Join("|", items.Select(item => $"{item.Id}:{item.Name}:{string.Join(",", item.Tags)}")) + "#" + garment;
        if (key == wardrobeKey) return;
        wardrobeKey = key;
        wardrobeEmpty.Visible(items.Length == 0);
        wardrobe.Children.Clear();
        foreach (var item in items)
        {
            var picture = new Image { Source = new BitmapImage(new Uri(item.Picture)) { DecodePixelWidth = 200 }, Width = 96, Height = 120, Stretch = Stretch.UniformToFill };
            var words = Ui.Column(
                2,
                Ui.Text(item.Name.Length > 0 ? item.Name : T("Being looked at…"), 12, item.Picture == garment),
                Ui.Secondary(string.Join(" · ", item.Tags), 11));
            words.Width = 110;
            var choose = new Button { Content = Ui.Column(6, picture, words), Padding = new Thickness(6) };
            var path = item.Picture;
            choose.Click += (_, _) =>
            {
                garment = path;
                garmentPicture.Source = new BitmapImage(new Uri(path));
                wardrobeKey = "";
                Refresh();
            };
            ToolTipService.SetToolTip(choose, T("Try this on"));
            var id = item.Id;
            var menu = new MenuFlyout();
            var remove = new MenuFlyoutItem { Text = T("Take out of the wardrobe") };
            remove.Click += (_, _) =>
            {
                shell.Assistant.RemoveFromWardrobe(id);
                if (garment == path)
                {
                    garment = null;
                    garmentPicture.Source = null;
                }
                wardrobeKey = "";
                Refresh();
            };
            menu.Items.Add(remove);
            choose.ContextFlyout = menu;
            if (item.Picture == garment)
            {
                choose.BorderBrush = Ui.Brush("AccentFillColorDefaultBrush");
                choose.BorderThickness = new Thickness(2);
            }
            wardrobe.Children.Add(choose);
        }
    }

    /// <summary>Whose photo of a person this is; null when the user does not say.</summary>
    private async Task<FaceConsent?> AskWhose()
    {
        var whose = new RadioButtons { Header = T("Whose photo is this?") };
        whose.Items.Add(T("This is me."));
        whose.Items.Add(T("It is someone else, and I have their permission."));
        whose.Items.Add(T("It is AI-generated and shows no real person."));
        var body = new StackPanel { Spacing = 12, MaxWidth = 440 };
        body.Children.Add(whose);
        body.Children.Add(Ui.Secondary(T("The photo goes to the picture model chosen in Settings. If none of these is true, do not use the photo."), 13));
        var dialog = new ContentDialog
        {
            XamlRoot = shell.Root,
            Title = T("Use this photo"),
            Content = body,
            PrimaryButtonText = T("Use it"),
            CloseButtonText = T("Cancel"),
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        whose.SelectionChanged += (_, _) => dialog.IsPrimaryButtonEnabled = whose.SelectedIndex >= 0;
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return null;
        return whose.SelectedIndex switch
        {
            1 => FaceConsent.Permission,
            2 => FaceConsent.Synthetic,
            _ => FaceConsent.Myself,
        };
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
