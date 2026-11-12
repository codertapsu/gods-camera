using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>Recording a loop, playing it, and the loops kept for later.</summary>
internal sealed class LoopScreen : UserControl, IScreen
{
    private static readonly LoopStyle[] Styles = { LoopStyle.Automatic, LoopStyle.Forward, LoopStyle.PingPong };

    private readonly Shell shell;
    private readonly PreviewPane preview;
    private readonly Button record;
    private readonly FontIcon recordIcon = Ui.Icon(Ui.IconRecord, 16);
    private readonly TextBlock recordText = new();
    private readonly Button play;
    private readonly FontIcon playIcon = Ui.Icon(Ui.IconPlay, 16);
    private readonly TextBlock playText = new();

    private readonly TextBlock clipName = Ui.Text("", 14, true);
    private readonly TextBlock clipLength = Ui.Secondary("", 13);
    private readonly Hint lighting = new();
    private readonly Hint noClip = new();
    private readonly TextBox saveName = new() { MinWidth = 150 };
    private readonly Grid saveRow;

    private readonly ComboBox style = new() { HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly Hint styleHint = new();
    private readonly SwitchRow badge;

    private readonly Card savedCard;
    private readonly StackPanel savedList = new() { Spacing = 6 };
    private string savedKey = "";
    private bool showing;

    public LoopScreen(Shell shell)
    {
        this.shell = shell;
        preview = new PreviewPane(shell.Preview);
        var layout = new StudioLayout(T("Loop"), T("Record a few seconds of yourself and play them as your camera picture."), preview);

        record = new Button { Content = Ui.Row(8, recordIcon, recordText), MinWidth = 170, Padding = new Thickness(14, 9, 14, 9) };
        record.Click += (_, _) => shell.App.ToggleRecording();
        play = new Button
        {
            Content = Ui.Row(8, playIcon, playText),
            MinWidth = 170,
            Padding = new Thickness(14, 9, 14, 9),
            Style = (Style)Application.Current.Resources["AccentButtonStyle"],
        };
        play.Click += (_, _) => shell.App.ToggleLoop();
        layout.Under.Children.Add(Ui.Row(12, record, play));
        layout.Under.Children.Add(new Hint(
            T("Sit still for the first and the last second of a recording. The loop then ends as it began, and nobody sees it start again."),
            Ui.IconIdea));

        saveName.PlaceholderText = T("Name");
        var save = Ui.PlainButton(T("Save"), () =>
        {
            if (shell.App.SaveClip(saveName.Text)) saveName.Text = "";
        });
        saveRow = Ui.Spread(saveName, save);
        lighting.Set(T("The room looks brighter or darker than in this loop."), Ui.IconWarning, Tone.Warning);
        noClip.Set(T("No loop yet. Click Record a loop, wait a few seconds, and click Stop recording."));
        layout.Controls.Children.Add(new Card(T("This loop"), Ui.IconFilm, Ui.Column(3, clipName, clipLength), lighting, saveRow, noClip));

        foreach (var name in new[] { T("Automatic"), T("Start to end"), T("Back and forth") }) style.Items.Add(name);
        style.SelectionChanged += (_, _) =>
        {
            if (!showing && style.SelectedIndex >= 0) shell.App.SetLoopStyle(Styles[style.SelectedIndex]);
        };
        badge = new SwitchRow(T("Show a LOOP badge to others"), on => shell.App.SetShowLoopBadge(on));
        layout.Controls.Children.Add(new Card(T("How it repeats"), Ui.IconRepeat, style, styleHint, badge));

        savedCard = new Card(T("Saved loops"), Ui.IconSaved, savedList);
        layout.Controls.Children.Add(savedCard);
        Content = layout;
    }

    public void Update(AppState state)
    {
        preview.Update(state);

        record.IsEnabled = state.CanRecord || state.IsRecording;
        recordIcon.Glyph = state.IsRecording ? Ui.IconStop : Ui.IconRecord;
        recordText.Text = state.IsRecording ? T("Stop recording") : T("Record a loop");
        var looping = state.Mode == EngineMode.Looping;
        play.IsEnabled = state.CanLoop;
        playIcon.Glyph = looping ? Ui.IconVideo : Ui.IconPlay;
        playText.Text = looping ? T("Back to live") : T("Play the loop");

        var clip = state.Clip;
        clipName.Visible(clip != null);
        clipLength.Visible(clip != null);
        noClip.Visible(clip == null);
        lighting.Visible(state.LightingDiffers);
        saveRow.Visible(clip != null && !clip.Saved);
        if (clip != null)
        {
            clipName.Text = clip.Name;
            clipLength.Text = T("Recorded {0} s · loops {1} s", OneDecimal(clip.Duration), OneDecimal(clip.LoopDuration));
        }

        showing = true;
        var index = Array.IndexOf(Styles, state.LoopStyle);
        if (style.SelectedIndex != index) style.SelectedIndex = index;
        showing = false;
        badge.Show(state.ShowLoopBadge);
        styleHint.Set(state.LoopStyle switch
        {
            LoopStyle.Forward => T("Plays to the end, then fades back to the start."),
            LoopStyle.PingPong => T("Plays to the end, then backwards to the start. There is no jump, but movement runs in reverse half the time."),
            _ when clip == null => T("Start to end when a loop ends as it began. Otherwise back and forth, which has no jump."),
            _ when clip.EndsAsItBegan => T("This loop ends as it began, so it plays start to end."),
            _ => T("You moved between the start and the end of this loop, so it plays back and forth, without a jump."),
        });

        // The list is made anew only when the loops or the one in hand change.
        savedCard.Visible(state.SavedClips.Length > 0);
        var key = string.Join("|", state.SavedClips.Select(saved => $"{saved.Id}:{saved.Name}")) + "#" + clip?.Id;
        if (key == savedKey) return;
        savedKey = key;
        savedList.Children.Clear();
        foreach (var saved in state.SavedClips)
        {
            var chosen = clip?.Id == saved.Id;
            var mark = Ui.Icon(
                chosen ? Ui.IconCheckedCircle : Ui.IconCircle, 16,
                Ui.Brush(chosen ? "AccentTextFillColorPrimaryBrush" : "TextFillColorTertiaryBrush"));
            var words = Ui.Column(1, Ui.Text(saved.Name), Ui.Secondary(T("{0} s", OneDecimal(saved.Duration))));
            var id = saved.Id;
            var select = Ui.RowButton(mark, words, () => shell.App.SelectSavedClip(id));
            var delete = new Button { Content = Ui.Icon(Ui.IconDelete, 14) };
            ToolTipService.SetToolTip(delete, T("Delete this loop"));
            delete.Click += (_, _) => shell.App.DeleteSavedClip(id);
            savedList.Children.Add(Ui.Spread(select, delete));
        }
    }
}
