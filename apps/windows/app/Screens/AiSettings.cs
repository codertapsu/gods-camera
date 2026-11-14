using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using uniffi.gc_app;
using static GodsCamera.Texts;

namespace GodsCamera.Screens;

/// <summary>
/// Settings for the language and image models (plan, Phase 5): the keys for
/// the cloud providers, which models are used, which features may send
/// anything to the cloud, and what it has cost. Keys are entered here and
/// kept in Windows Credential Manager; they are never shown again.
/// </summary>
internal sealed class AiSettings : StackPanel
{
    private static readonly (CloudProvider Provider, string Name, string Title)[] Clouds =
    {
        (CloudProvider.Anthropic, "anthropic", "Claude (Anthropic)"),
        (CloudProvider.OpenAi, "openai", "OpenAI"),
        (CloudProvider.Gemini, "gemini", "Gemini (Google)"),
    };

    private static readonly (AiFeature Feature, string Words)[] Features =
    {
        (AiFeature.CleanUp, T("Cleaning up scanned text")),
        (AiFeature.Summary, T("Summaries")),
        (AiFeature.Translation, T("Translations")),
        (AiFeature.Extraction, T("Key facts")),
        (AiFeature.Questions, T("Questions about a document")),
        (AiFeature.HardPages, T("Reading pages the text reader could not")),
        (AiFeature.TryOn, T("Try-on pictures")),
        (AiFeature.NameScan, T("Naming 3D scans")),
    };

    private readonly Shell shell;
    private readonly Dictionary<CloudProvider, (TextBlock State, Button Remove)> keys = new();
    private readonly TextBlock ollama = Ui.Secondary("", 13);
    private readonly ComboBox textModel = new() { MinWidth = 320 };
    private readonly ComboBox imageModel = new() { MinWidth = 320 };
    private readonly Dictionary<AiFeature, SwitchRow> switches = new();
    private readonly TextBlock cost = new() { TextWrapping = TextWrapping.Wrap };
    private List<ModelChoice?> textChoices = new();
    private List<ModelChoice?> imageChoices = new();
    private bool showing;

    public AiSettings(Shell shell)
    {
        this.shell = shell;
        Spacing = 16;

        var providers = new StackPanel { Spacing = 10 };
        foreach (var (provider, _, title) in Clouds)
        {
            var state = Ui.Secondary("", 13);
            var enter = Ui.PlainButton(T("Enter key…"), () => EnterKey(provider, title));
            var remove = Ui.PlainButton(T("Remove"), () =>
            {
                shell.Assistant.RemoveKey(provider);
                Refresh(listModels: true);
            });
            keys[provider] = (state, remove);
            providers.Children.Add(Ui.Spread(Ui.Column(2, Ui.Text(title, 14, true), state), Ui.Row(8, enter, remove)));
        }
        providers.Children.Add(Ui.Spread(Ui.Column(2, Ui.Text(T("Ollama on this PC"), 14, true), ollama), Ui.PlainButton(T("Look again"), () => Refresh(listModels: true))));
        Children.Add(new Card(
            T("AI models"), Ui.IconKey, providers,
            new Hint(T("A key is kept in Windows Credential Manager and never shown again. Local models need none: start Ollama on this PC and its models appear here."), Ui.IconLock)));

        textModel.SelectionChanged += (_, _) =>
        {
            if (!showing && textModel.SelectedIndex >= 0 && textModel.SelectedIndex < textChoices.Count) shell.Assistant.SetTextModel(textChoices[textModel.SelectedIndex]);
        };
        imageModel.SelectionChanged += (_, _) =>
        {
            if (!showing && imageModel.SelectedIndex >= 0 && imageModel.SelectedIndex < imageChoices.Count) shell.Assistant.SetImageModel(imageChoices[imageModel.SelectedIndex]);
        };
        Children.Add(new Card(
            T("Which models"), Ui.IconAssistant,
            Ui.Column(4, Ui.Secondary(T("For documents"), 13), textModel),
            Ui.Column(4, Ui.Secondary(T("For pictures"), 13), imageModel)));

        var list = new StackPanel { Spacing = 6 };
        foreach (var (feature, words) in Features)
        {
            var row = new SwitchRow(words, on => shell.Assistant.SetCloudAllowed(feature, on));
            switches[feature] = row;
            list.Children.Add(row);
        }
        Children.Add(new Card(
            T("What may use cloud models"), Ui.IconLock,
            new Hint(T("Off: the feature uses only models on your own computers, and nothing leaves this PC. On: what the feature needs, such as a document's text or a photo, is sent to the provider of the model chosen above.")),
            list));
        Children.Add(new Card(T("Cost"), Ui.IconInfo, cost, new Hint(T("Estimates from the providers' price lists. Local models cost nothing."))));
        Refresh(listModels: true);
    }

    private int ticks;

    /// <summary>About every two seconds while Settings is shown.</summary>
    public void Tick()
    {
        if (++ticks % 20 == 0) Refresh(listModels: false);
    }

    private async void EnterKey(CloudProvider provider, string title)
    {
        var box = new PasswordBox { PlaceholderText = T("Paste the key here"), Width = 380 };
        var body = Ui.Column(10, Ui.Text(T("The key is kept in Windows Credential Manager and never shown again.")), box);
        if (!await shell.Ask(T("The key for {0}", title), body, T("Save"), T("Cancel"))) return;
        var problem = shell.Assistant.SetKey(provider, box.Password);
        box.Password = "";
        if (problem != null)
        {
            await shell.Ask(T("The key was not saved"), Ui.Text(problem), T("OK"), T("Cancel"));
        }
        Refresh(listModels: true);
    }

    private async void Refresh(bool listModels)
    {
        var assistant = shell.Assistant;
        var providers = assistant.Providers();
        foreach (var (provider, name, _) in Clouds)
        {
            var known = providers.FirstOrDefault(one => one.Name == name);
            var (state, remove) = keys[provider];
            state.Text = known?.HasKey == true ? T("Key saved") : T("No key");
            remove.Visible(known?.HasKey == true);
        }
        showing = true;
        foreach (var (feature, _) in Features) switches[feature].Show(assistant.CloudAllowed(feature));
        showing = false;
        var usage = assistant.Usage();
        cost.Text = T("Today: about {0} ({1} requests). This month: about {2} ({3} requests).",
            usage.TodayCost.ToString("$0.00"), usage.TodayRequests, usage.MonthCost.ToString("$0.00"), usage.MonthRequests)
            + (usage.Incomplete ? " " + T("Some models have no price, so the real cost is higher.") : "");
        if (!listModels) return;

        // Listing models asks each provider, so it runs away from the window's thread.
        var named = providers.Where(one => one.HasKey || one.Local).Select(one => one.Name).ToArray();
        var lists = await Task.Run(() => named.Select(name => (name, list: assistant.Models(name))).ToArray());
        var local = lists.FirstOrDefault(one => one.name == "ollama");
        ollama.Text = local.list == null || local.list.Problem != null
            ? T("Not running")
            : T("Running, with {0} models", local.list.Models.Length);

        textChoices = new List<ModelChoice?> { null };
        imageChoices = new List<ModelChoice?> { null };
        foreach (var (name, list) in lists)
        {
            if (list.Problem != null) continue;
            textChoices.AddRange(list.Models.Select(model => (ModelChoice?)new ModelChoice(name, model)));
            imageChoices.AddRange(list.ImageModels.Select(model => (ModelChoice?)new ModelChoice(name, model)));
        }
        showing = true;
        Fill(textModel, textChoices, assistant.TextModel());
        Fill(imageModel, imageChoices, assistant.ImageModel());
        showing = false;
    }

    private static void Fill(ComboBox box, List<ModelChoice?> choices, ModelChoice? current)
    {
        box.Items.Clear();
        foreach (var choice in choices)
        {
            box.Items.Add(choice == null ? T("The first provider with a key") : $"{choice.Provider} · {choice.Model}");
        }
        // The model in use may not be in a provider's list, such as the default before the list was read.
        if (current != null && !choices.Any(choice => choice?.Provider == current.Provider && choice?.Model == current.Model))
        {
            choices.Add(current);
            box.Items.Add($"{current.Provider} · {current.Model}");
        }
        box.SelectedIndex = current == null ? 0 : choices.FindIndex(choice => choice?.Provider == current.Provider && choice?.Model == current.Model);
    }
}
