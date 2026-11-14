using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>
/// The Face tab of the Camera screen: the saved faces, adding one, how the
/// swapped face looks, and privacy. The switch itself is in the row of
/// controls under the picture, and the swap's state in the line under it.
/// </summary>
internal sealed class FacePanel : StackPanel
{
    private static readonly string[] PhotoTypes = { ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff" };

    private readonly Shell shell;

    private readonly StackPanel faceList = new() { Spacing = 4 };
    private readonly Button add;
    private readonly StackPanel reading;
    private readonly Hint notice = new();
    private string facesKey = "";

    private readonly ComboBox quality = new() { HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly StackPanel qualityRow;
    private readonly Slider sharpen = new() { Minimum = 0, Maximum = 1, StepFrequency = 0.05, MinWidth = 150 };
    private readonly SwitchRow colour;
    private readonly SwitchRow keepObjects;
    private readonly SwitchRow realMouth;
    private readonly SwitchRow blur;
    private readonly SwitchRow aiBadge;
    private Quality[] qualities = Array.Empty<Quality>();
    private bool showing;
    private bool dialogOpen;

    public FacePanel(Shell shell)
    {
        this.shell = shell;
        Spacing = 14;

        add = Ui.IconButton(Ui.IconAdd, T("Add a face…"), () => ChoosePhotos(null));
        reading = Ui.Row(8, new ProgressRing { Width = 16, Height = 16, IsActive = true }, Ui.Secondary(T("Reading the photos…"), 13));
        Children.Add(new Card(
            T("Faces"), Ui.IconFace, faceList, Ui.Row(10, add, reading),
            new Hint(T("Choose several photos of the same person at once: different angles, expressions and light. Together they give a better likeness than one photo.")),
            notice));

        quality.SelectionChanged += (_, _) =>
        {
            if (!showing && quality.SelectedIndex >= 0 && quality.SelectedIndex < qualities.Length)
            {
                shell.App.SetQuality(qualities[quality.SelectedIndex]);
            }
        };
        qualityRow = Ui.Column(4, Ui.Secondary(T("Quality"), 13), quality);
        sharpen.ValueChanged += (_, _) => OptionsChanged();
        colour = new SwitchRow(T("Match my skin tone"), _ => OptionsChanged());
        keepObjects = new SwitchRow(T("Keep hands and objects in front of the face"), _ => OptionsChanged());
        ToolTipService.SetToolTip(keepObjects, T("A hand, a cup or a microphone in front of your face stays in front of the swapped face."));
        realMouth = new SwitchRow(T("Show my real mouth"), _ => OptionsChanged());
        ToolTipService.SetToolTip(realMouth, T("Your own lips and teeth show through the swapped face. Speech looks more natural; the mouth is then yours, not the other face's."));
        Children.Add(new Card(
            T("How it looks"), Ui.IconWand, qualityRow, Ui.Column(4, Ui.Secondary(T("Sharpness"), 13), sharpen), colour, keepObjects, realMouth));

        blur = new SwitchRow(T("Blur the picture when the swap is not on my face"), _ => OptionsChanged());
        aiBadge = new SwitchRow(T("Show an AI badge to others"), _ => OptionsChanged());
        Children.Add(new Card(
            T("Privacy"), Ui.IconHidden, blur,
            new Hint(T("The swap leaves your face when you turn far to the side, when something hides most of it, or when it is too dark. With this on, others then see a blurred picture and never your own face.")),
            aiBadge));
    }

    // The switch and the options

    /// <summary>
    /// Turn the swap on or off. The rules for swapping a face are shown once,
    /// before the first time; the swap starts only when the user accepts them.
    /// </summary>
    public async Task TurnSwap(bool on)
    {
        if (shell.App.SetSwapWanted(on)) return;
        var rules = Ui.Column(
            10,
            Ui.Text(T("People in a call will see a face that is not the one in front of the camera. Please use this fairly.")),
            Rule(Ui.IconCheck, T("Use only your own face, a face you have permission to use, or an AI-generated face.")),
            Rule(Ui.IconWarning, T("Do not use it to pass as someone else, to deceive, or to harass.")),
            Rule(Ui.IconInfo, T("An “AI” badge is drawn on your picture. You can turn it off; consider telling people instead.")),
            Rule(Ui.IconLock, T("Saved faces are encrypted and stay on this PC.")));
        rules.MaxWidth = 440;
        if (await shell.Ask(T("Before you swap your face"), rules, T("I understand"), T("Not now")))
        {
            shell.App.AcceptPolicy();
        }
    }

    private static Grid Rule(string glyph, string words) => new Hint(words, glyph);

    private void OptionsChanged()
    {
        if (showing) return;
        shell.App.SetFaceOptions((float)sharpen.Value, colour.IsOn, keepObjects.IsOn, realMouth.IsOn, blur.IsOn, aiBadge.IsOn);
    }

    // Faces

    public async void ChoosePhotos(string? target)
    {
        var files = await shell.PickFiles(true, PhotoTypes);
        if (files.Count > 0) shell.App.AddPhotos(files.ToArray(), target);
    }

    private static string PhotoCount(int count) => count == 1 ? T("1 photo") : T("{0} photos", count);

    private static string ConsentLabel(FaceConsent consent) => consent switch
    {
        FaceConsent.Permission => T("Used with permission"),
        FaceConsent.Synthetic => T("AI-generated face"),
        _ => T("My own face"),
    };

    private async void Fill(Image image, Task<byte[]> picture)
    {
        image.Source = await Ui.Picture(await picture);
    }

    private static Image SmallPicture(double side, bool round) => new()
    {
        Width = side,
        Height = side,
        Stretch = Stretch.UniformToFill,
    };

    private Border Framed(Image image, double side, bool round) => new()
    {
        Width = side,
        Height = side,
        CornerRadius = new CornerRadius(round ? side / 2 : side * 0.22),
        Background = Ui.Brush("ControlFillColorSecondaryBrush"),
        Child = image,
    };

    private void BuildFaces(AppState state)
    {
        faceList.Children.Clear();
        var app = shell.App;
        foreach (var face in state.Faces)
        {
            var chosen = face.Id == state.SelectedFace;
            var id = face.Id;
            var picture = SmallPicture(44, true);
            Fill(picture, Task.Run(() => app.FacePicture(id, null)));

            var menu = new MenuFlyout();
            var addPhotos = new MenuFlyoutItem { Text = T("Add photos…"), IsEnabled = face.Photos.Length < app.MaxPhotos() };
            addPhotos.Click += (_, _) => ChoosePhotos(id);
            var rename = new MenuFlyoutItem { Text = T("Rename…") };
            var name = face.Name;
            rename.Click += (_, _) => Rename(id, name);
            var delete = new MenuFlyoutItem { Text = T("Delete") };
            delete.Click += (_, _) => app.DeleteFace(id);
            menu.Items.Add(addPhotos);
            menu.Items.Add(rename);
            menu.Items.Add(new MenuFlyoutSeparator());
            menu.Items.Add(delete);
            var more = new Button { Content = Ui.Icon(Ui.IconMore, 14), Flyout = menu };
            ToolTipService.SetToolTip(more, T("More for this face"));

            var words = Ui.Column(
                1, Ui.Text(face.Name, 14, true), Ui.Secondary($"{PhotoCount(face.Photos.Length)} · {ConsentLabel(face.Consent)}"));
            var select = Ui.RowButton(Framed(picture, 44, true), words, () => app.SelectFace(id));
            var trailing = chosen
                ? Ui.Row(8, Ui.Icon(Ui.IconCheckedCircle, 16, Ui.Brush("AccentTextFillColorPrimaryBrush")), more)
                : Ui.Row(8, more);
            var row = new StackPanel { Spacing = 8 };
            row.Children.Add(Ui.Spread(select, trailing));

            // The photos the chosen face is made from, with a way to add more.
            if (chosen)
            {
                var strip = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
                foreach (var photoId in face.Photos)
                {
                    var small = SmallPicture(34, false);
                    Fill(small, Task.Run(() => app.FacePicture(id, photoId)));
                    var frame = Framed(small, 34, false);
                    var remove = new MenuFlyoutItem { Text = T("Remove this photo"), IsEnabled = face.Photos.Length > 1 };
                    remove.Click += (_, _) => app.RemovePhoto(id, photoId);
                    var photoMenu = new MenuFlyout();
                    photoMenu.Items.Add(remove);
                    frame.ContextFlyout = photoMenu;
                    ToolTipService.SetToolTip(frame, T("Right-click to remove this photo"));
                    strip.Children.Add(frame);
                }
                if (face.Photos.Length < app.MaxPhotos())
                {
                    var another = new Button { Content = Ui.Icon(Ui.IconAdd, 12), Width = 34, Height = 34, Padding = new Thickness(0) };
                    ToolTipService.SetToolTip(another, T("Add more photos of this person"));
                    another.Click += (_, _) => ChoosePhotos(id);
                    strip.Children.Add(another);
                }
                row.Children.Add(new ScrollViewer
                {
                    Content = strip,
                    HorizontalScrollMode = ScrollMode.Auto,
                    HorizontalScrollBarVisibility = ScrollBarVisibility.Hidden,
                    VerticalScrollMode = ScrollMode.Disabled,
                    VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                });
            }
            faceList.Children.Add(new Border
            {
                CornerRadius = new CornerRadius(10),
                Padding = new Thickness(6),
                Background = chosen ? Ui.Brush("SubtleFillColorSecondaryBrush") : new SolidColorBrush(Microsoft.UI.Colors.Transparent),
                Child = row,
            });
        }
    }

    private async void Rename(string id, string name)
    {
        var box = new TextBox { Text = name, PlaceholderText = T("Name"), MinWidth = 260 };
        if (await shell.Ask(T("Rename the face"), box, T("Save"), T("Cancel")))
        {
            shell.App.RenameFace(id, box.Text);
        }
    }

    /// <summary>
    /// Shown after photos are read: which of them will be used, a name for the
    /// face, and the user's right to use it.
    /// </summary>
    private async void ShowPending(PendingInfo pending)
    {
        if (dialogOpen) return;
        dialogOpen = true;
        var app = shell.App;
        var addition = pending.Target != null;
        var used = pending.Photos.Count(photo => photo.Used);

        var photos = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        foreach (var photo in pending.Photos)
        {
            var picture = SmallPicture(76, false);
            var photoId = photo.Id;
            Fill(picture, Task.Run(() => app.PendingPicture(photoId)));
            var frame = Framed(picture, 76, false);
            frame.Opacity = photo.Used ? 1 : 0.35;
            ToolTipService.SetToolTip(frame, photo.FileName);
            var label = Ui.Text(photo.Used ? T("Used") : T("Left out"), 12, false, Ui.ToneBrush(photo.Used ? Tone.Plain : Tone.Warning));
            label.HorizontalAlignment = HorizontalAlignment.Center;
            photos.Children.Add(Ui.Column(5, frame, label));
        }

        var body = new StackPanel { Spacing = 14, MaxWidth = 500 };
        body.Children.Add(new ScrollViewer
        {
            Content = photos,
            HorizontalScrollMode = ScrollMode.Auto,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollMode = ScrollMode.Disabled,
            VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
            Padding = new Thickness(0, 0, 0, 10),
        });
        body.Children.Add(Ui.Text(
            used == 0 ? T("None of these photos shows the same person as the saved face.")
            : pending.Photos.Length == 1 ? T("One photo will be used.")
            : T("{0} of {1} photos will be used for this face.", used, pending.Photos.Length)));
        if (used < pending.Photos.Length)
        {
            body.Children.Add(new Hint(
                addition
                    ? T("Photos that show a different person from the saved face are left out.")
                    : T("Photos that show a different person from most of the others are left out."),
                Ui.IconWarning, Tone.Warning));
        }
        if (pending.Skipped.Length > 0)
        {
            body.Children.Add(new Hint(T("No face was found in: {0}", string.Join(", ", pending.Skipped)), Ui.IconInfo));
        }
        if (pending.Photos.Any(photo => photo.Used && photo.FacesInPhoto > 1))
        {
            body.Children.Add(new Hint(T("Where a photo shows several faces, the largest one was used.")));
        }

        var name = new TextBox { PlaceholderText = T("Name") };
        var whose = new RadioButtons { Header = T("Whose face is this?") };
        whose.Items.Add(T("This is me."));
        whose.Items.Add(T("It is someone else, and I have their permission."));
        whose.Items.Add(T("It is AI-generated and shows no real person."));
        if (!addition)
        {
            body.Children.Add(name);
            body.Children.Add(whose);
            body.Children.Add(Ui.Secondary(T("If none of these is true, do not add the face."), 13));
        }

        var dialog = new ContentDialog
        {
            XamlRoot = shell.Root,
            Title = addition ? T("Add photos to {0}", pending.TargetName ?? "") : T("Add this face"),
            Content = body,
            PrimaryButtonText = addition ? T("Add") : T("Save"),
            CloseButtonText = T("Cancel"),
            DefaultButton = ContentDialogButton.Primary,
            // A face cannot be saved without saying whose it is.
            IsPrimaryButtonEnabled = used > 0 && addition,
        };
        whose.SelectionChanged += (_, _) => dialog.IsPrimaryButtonEnabled = used > 0 && whose.SelectedIndex >= 0;
        var answer = await dialog.ShowAsync();
        if (answer == ContentDialogResult.Primary)
        {
            var consent = whose.SelectedIndex switch
            {
                1 => FaceConsent.Permission,
                2 => FaceConsent.Synthetic,
                _ => FaceConsent.Myself,
            };
            app.SavePending(name.Text, consent);
        }
        else
        {
            app.CancelPending();
        }
        dialogOpen = false;
    }

    public void Update(AppState state)
    {
        add.IsEnabled = !state.BusyAdding;
        reading.Visible(state.BusyAdding);
        notice.Visible(state.FaceNotice != null);
        switch (state.FaceNotice)
        {
            case FaceNotice.FacesCouldNotBeOpened: notice.Set(T("Saved faces could not be opened."), Ui.IconWarning, Tone.Warning); break;
            case FaceNotice.AddAFaceFirst: notice.Set(T("Add a face first, then turn the swap on."), Ui.IconWarning, Tone.Warning); break;
            case FaceNotice.NoFaceInThatPicture: notice.Set(T("No face was found in that picture."), Ui.IconWarning, Tone.Warning); break;
            case FaceNotice.NoFaceInThosePictures: notice.Set(T("No face was found in those pictures."), Ui.IconWarning, Tone.Warning); break;
            case FaceNotice.TooManyPhotos: notice.Set(T("A face can have up to {0} photos. The rest were not read.", shell.App.MaxPhotos()), Ui.IconWarning, Tone.Warning); break;
            case FaceNotice.FaceCouldNotBeSaved: notice.Set(T("The face could not be saved."), Ui.IconWarning, Tone.Warning); break;
            case FaceNotice.ModelMissing: notice.Set(T("A model file is not installed: {0}", state.SwapDetail), Ui.IconWarning, Tone.Warning); break;
        }

        var key = string.Join("|", state.Faces.Select(face => $"{face.Id}:{face.Name}:{string.Join(",", face.Photos)}")) + "#" + state.SelectedFace;
        if (key != facesKey)
        {
            facesKey = key;
            BuildFaces(state);
        }
        if (state.Pending != null) ShowPending(state.Pending);

        showing = true;
        qualityRow.Visible(state.Qualities.Length > 1);
        if (!qualities.SequenceEqual(state.Qualities))
        {
            qualities = state.Qualities;
            quality.Items.Clear();
            foreach (var one in qualities) quality.Items.Add(one == Quality.Sharper ? T("Sharper") : T("Standard"));
        }
        var index = Array.IndexOf(qualities, state.Quality);
        if (quality.SelectedIndex != index) quality.SelectedIndex = index;
        if (Math.Abs(sharpen.Value - state.Sharpen) > 0.001) sharpen.Value = state.Sharpen;
        colour.Show(state.ColourMatch);
        keepObjects.Visible(state.CanKeepObjectsInFront);
        keepObjects.Show(state.KeepObjectsInFront);
        realMouth.Show(state.RealMouth);
        blur.Show(state.BlurWhenUnswapped);
        aiBadge.Show(state.ShowAiBadge);
        showing = false;
    }
}
