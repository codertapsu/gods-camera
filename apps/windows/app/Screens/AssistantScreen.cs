using Microsoft.UI.Dispatching;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>
/// The assistant for scanned documents (plan, Phase 5): choose a document,
/// then have its recognition mistakes cleaned up, see a summary, a
/// translation, its key facts, or ask a question about it. The model is the
/// one chosen in Settings; the cloud is used only where Settings allows it.
/// </summary>
internal sealed class AssistantScreen : UserControl, IScreen
{
    private readonly Shell shell;
    private readonly StackPanel documentList = new() { Spacing = 4 };
    private readonly Hint noDocuments = new();
    private string documentsKey = "";
    private string? chosen;

    private readonly TextBlock title = new() { FontSize = 22, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock text = new() { FontSize = 14, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
    private readonly ComboBox language = new() { MinWidth = 150 };
    private readonly TextBox question = new() { PlaceholderText = T("Ask about this document…"), HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly List<Button> actions = new();
    private readonly TextBlock modelLine = Ui.Secondary("", 12);

    private readonly StackPanel resultPanel = new() { Spacing = 10 };
    private readonly TextBlock resultTitle = Ui.Text("", 15, true);
    private readonly RichTextBlock result = new() { FontSize = 14, IsTextSelectionEnabled = true, TextWrapping = TextWrapping.Wrap };
    private readonly Hint resultHint = new();
    private readonly StackPanel working;
    private readonly TextBlock workingText = Ui.Secondary("", 13);
    private readonly Button stop;
    private readonly StackPanel resultButtons;
    private string resultText = "";

    private ulong task;
    private readonly DispatcherQueueTimer follow;

    public AssistantScreen(Shell shell)
    {
        this.shell = shell;
        language.Items.Add(T("In Vietnamese"));
        language.Items.Add(T("In English"));
        language.SelectedIndex = Texts.Vietnamese ? 0 : 1;

        // At the left: the scanned documents.
        var left = new StackPanel { Spacing = 12, Padding = new Thickness(20, 20, 16, 20) };
        left.Children.Add(new ScreenHeader(T("Assistant"), T("Have a model clean up, sum up, translate or answer questions about your scanned documents.")));
        left.Children.Add(Ui.Text(T("Documents"), 15, true));
        noDocuments.Set(T("Scan a document with your phone or tablet first: it appears here."));
        left.Children.Add(noDocuments);
        left.Children.Add(documentList);

        // At the right: the document, what can be done with it, and what came of it.
        Button Action(string glyph, string words, Func<DocumentTask> make, string? tip = null)
        {
            var button = Ui.IconButton(glyph, words, () => Start(make(), words));
            if (tip != null) ToolTipService.SetToolTip(button, tip);
            actions.Add(button);
            return button;
        }
        var cleanUp = Action(Ui.IconWand, T("Clean up"), () => new DocumentTask.CleanUp(), T("Fix what text recognition got wrong, and nothing else. The changes are marked."));
        var summary = Action(Ui.IconDocument, T("Summarise"), () => new DocumentTask.Summary(language.SelectedIndex == 1));
        var translate = Action(Ui.IconAssistant, T("Translate"), () => new DocumentTask.Translate(language.SelectedIndex == 1), T("Into the language chosen beside it."));
        var facts = Action(Ui.IconCheck, T("Key facts"), () => new DocumentTask.KeyFacts(), T("Dates, people, places, organisations, amounts and contacts."));
        var ask = Ui.PlainButton(T("Ask"), () =>
        {
            if (question.Text.Trim().Length > 0) Start(new DocumentTask.Ask(question.Text.Trim()), T("Answer"));
        });
        ask.Style = (Style)Application.Current.Resources["AccentButtonStyle"];
        actions.Add(ask);
        question.KeyDown += (_, key) =>
        {
            if (key.Key == Windows.System.VirtualKey.Enter && question.Text.Trim().Length > 0) Start(new DocumentTask.Ask(question.Text.Trim()), T("Answer"));
        };
        var askRow = Ui.Spread(question, ask);

        stop = Ui.PlainButton(T("Stop"), () => shell.Assistant.Cancel(task));
        working = Ui.Row(10, new ProgressRing { Width = 18, Height = 18, IsActive = true }, workingText, stop);
        var copy = Ui.IconButton(Ui.IconCopy, T("Copy"), () => CopyText(resultText));
        var save = Ui.PlainButton(T("Save as…"), SaveResult);
        resultButtons = Ui.Row(8, copy, save);
        resultPanel.Children.Add(resultTitle);
        resultPanel.Children.Add(working);
        resultPanel.Children.Add(resultHint);
        resultPanel.Children.Add(result);
        resultPanel.Children.Add(resultButtons);
        var resultCard = new Card(T("Result"), Ui.IconIdea, resultPanel);

        var right = new StackPanel { Spacing = 14, Padding = new Thickness(24, 20, 24, 24) };
        right.Children.Add(title);
        right.Children.Add(new Border
        {
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(16),
            BorderThickness = new Thickness(1),
            Background = Ui.Brush("CardBackgroundFillColorDefaultBrush"),
            BorderBrush = Ui.Brush("CardStrokeColorDefaultBrush"),
            MaxHeight = 300,
            Child = new ScrollViewer { Content = text, VerticalScrollBarVisibility = ScrollBarVisibility.Auto },
        });
        right.Children.Add(Ui.Row(8, cleanUp, summary, translate, facts, language));
        right.Children.Add(askRow);
        right.Children.Add(modelLine);
        right.Children.Add(resultCard);

        var layout = new Grid();
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(320) });
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var leftSide = new Border
        {
            BorderBrush = Ui.Brush("DividerStrokeColorDefaultBrush"),
            BorderThickness = new Thickness(0, 0, 1, 0),
            Child = new ScrollViewer { Content = left, VerticalScrollBarVisibility = ScrollBarVisibility.Auto },
        };
        var rightSide = new ScrollViewer { Content = right, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
        Grid.SetColumn(rightSide, 1);
        layout.Children.Add(leftSide);
        layout.Children.Add(rightSide);
        Content = layout;

        follow = DispatcherQueue.GetForCurrentThread().CreateTimer();
        follow.Interval = TimeSpan.FromMilliseconds(250);
        follow.Tick += (_, _) => Follow();
        ShowResult(null, null);
        Refresh();
    }

    public void Update(AppState state)
    {
        // About once a second: the documents and the model in use.
        if (++ticks % 10 == 1) Refresh();
    }

    private int ticks;

    private void Refresh()
    {
        DocumentInfo[] documents;
        try
        {
            documents = shell.Scanner.State().Documents.Where(document => document.Status == DocumentStatus.Done).ToArray();
        }
        catch (Exception)
        {
            return;
        }
        noDocuments.Visible(documents.Length == 0);
        var key = string.Join("|", documents.Select(document => document.Id)) + "#" + chosen;
        if (key != documentsKey)
        {
            documentsKey = key;
            documentList.Children.Clear();
            if (chosen == null || documents.All(document => document.Id != chosen)) Choose(documents.FirstOrDefault());
            foreach (var document in documents)
            {
                var id = document.Id;
                var picked = id == chosen;
                var words = Ui.Column(
                    1,
                    Ui.Text(document.Title.Length > 0 ? document.Title : T("Document"), 14, picked),
                    Ui.Secondary(DateTimeOffset.FromUnixTimeSeconds((long)document.ReceivedAt).LocalDateTime.ToString("g", System.Globalization.CultureInfo.CurrentCulture)));
                var row = Ui.RowButton(Ui.Icon(Ui.IconDocument, 16), words, () =>
                {
                    Choose(documents.First(one => one.Id == id));
                    documentsKey = "";
                    Refresh();
                });
                documentList.Children.Add(new Border
                {
                    CornerRadius = new CornerRadius(8),
                    Background = picked ? Ui.Brush("SubtleFillColorSecondaryBrush") : new SolidColorBrush(Microsoft.UI.Colors.Transparent),
                    Child = row,
                });
            }
        }
        var model = shell.Assistant.TextModel();
        modelLine.Text = model != null
            ? T("Model: {0} · {1}. Change it in Settings, under AI models.", model.Provider, model.Model)
            : T("No model yet. Enter a key in Settings, under AI models, or start Ollama on this PC.");
        var busy = task != 0 && shell.Assistant.Task(task).Running;
        foreach (var action in actions) action.IsEnabled = chosen != null && !busy;
    }

    private void Choose(DocumentInfo? document)
    {
        chosen = document?.Id;
        title.Text = document == null ? T("No document chosen") : (document.Title.Length > 0 ? document.Title : T("Document"));
        text.Text = document == null ? "" : shell.Scanner.DocumentText(document.Id);
    }

    private void Start(DocumentTask what, string heading)
    {
        if (chosen == null) return;
        if (task != 0) shell.Assistant.Forget(task);
        task = shell.Assistant.StartDocument(what, text.Text);
        resultTitle.Text = heading;
        ShowResult(null, null);
        working.Visible(true);
        workingText.Text = T("Working…");
        foreach (var action in actions) action.IsEnabled = false;
        follow.Start();
    }

    private void Follow()
    {
        var state = shell.Assistant.Task(task);
        if (state.Running)
        {
            workingText.Text = state.Total > 1 ? T("Working… part {0} of {1}", state.Done + 1, state.Total) : T("Working…");
            return;
        }
        follow.Stop();
        foreach (var action in actions) action.IsEnabled = chosen != null;
        ShowResult(state.Result, state.Problem);
    }

    private void ShowResult(TaskResult? done, AssistantProblem? problem)
    {
        working.Visible(false);
        result.Blocks.Clear();
        resultButtons.Visible(done != null);
        resultHint.Visible(problem != null || done == null);
        if (problem != null)
        {
            resultHint.Set(ProblemWords(problem), Ui.IconWarning, Tone.Warning);
            return;
        }
        if (done == null)
        {
            resultHint.Set(T("Choose what to do with the document above. Nothing is sent before you do."));
            return;
        }
        resultText = done.Text;
        var paragraph = new Paragraph();
        if (done.Changes.Length > 0)
        {
            // The changes as they read in the cleaned text: what went out struck through in red, what came in green.
            foreach (var change in done.Changes)
            {
                var run = new Run { Text = change.Text };
                if (change.Kind == "removed")
                {
                    run.Foreground = new SolidColorBrush(Ui.Red);
                    run.TextDecorations = Windows.UI.Text.TextDecorations.Strikethrough;
                }
                else if (change.Kind == "added")
                {
                    run.Foreground = new SolidColorBrush(Ui.Green);
                    run.FontWeight = FontWeights.SemiBold;
                }
                paragraph.Inlines.Add(run);
            }
            resultHint.Visible(true);
            resultHint.Set(
                done.Flagged
                    ? T("This clean-up changed more than recognition mistakes usually account for. Read it before you use it.")
                    : T("Struck through in red: what recognition got wrong. In green: what replaces it."),
                done.Flagged ? Ui.IconWarning : Ui.IconInfo, done.Flagged ? Tone.Warning : Tone.Plain);
        }
        else
        {
            paragraph.Inlines.Add(new Run { Text = done.Text });
        }
        result.Blocks.Add(paragraph);
        foreach (var quote in done.Quotes)
        {
            var quoted = new Paragraph { Margin = new Thickness(12, 8, 0, 0) };
            quoted.Inlines.Add(new Run { Text = "“" + quote + "”", Foreground = Ui.Brush("TextFillColorSecondaryBrush") });
            result.Blocks.Add(quoted);
        }
    }

    /// <summary>What went wrong, in plain words, with where to put it right.</summary>
    public static string ProblemWords(AssistantProblem problem) => problem switch
    {
        AssistantProblem.NoKey => T("No model to use yet. Enter a key in Settings, under AI models, or start Ollama on this PC."),
        AssistantProblem.KeyRefused => T("The provider did not accept the key. Check it in Settings, under AI models."),
        AssistantProblem.RateLimited limited => limited.Seconds is uint seconds
            ? T("Too many requests just now. Try again in {0} seconds.", seconds)
            : T("Too many requests just now. Try again in a minute."),
        AssistantProblem.Temporary => T("The model is busy or cannot be reached. Try again later."),
        AssistantProblem.BadRequest bad => T("The model did not take the request: {0}", bad.Message),
        AssistantProblem.Refused refused => T("{0} declined. Try another model, in Settings under AI models.", refused.Model),
        AssistantProblem.CloudNotAllowed => T("This stays on your computer for now. In Settings, under AI models, allow cloud models for it, or use a local model."),
        AssistantProblem.Cancelled => T("Stopped."),
        AssistantProblem.Other other => other.Message,
        _ => "",
    };

    private static void CopyText(string words)
    {
        var package = new Windows.ApplicationModel.DataTransfer.DataPackage();
        package.SetText(words);
        Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(package);
    }

    private async void SaveResult()
    {
        var picker = new Windows.Storage.Pickers.FileSavePicker
        {
            SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.DocumentsLibrary,
            SuggestedFileName = resultTitle.Text,
        };
        picker.FileTypeChoices.Add(T("Text"), new List<string> { ".txt" });
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(shell));
        var file = await picker.PickSaveFileAsync();
        if (file == null) return;
        await File.WriteAllTextAsync(file.Path, resultText);
        Shell.ShowInExplorer(file.Path);
    }
}
