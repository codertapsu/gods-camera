using System.Globalization;
using System.Text.Json;

namespace GodsCamera;

/// <summary>
/// The app's words. The code is written in English; `T("Camera")` gives the
/// Vietnamese where the app runs in Vietnamese. Assets\vi.json holds it and is
/// written by tools\sync-windows-strings.py, which fails if a text has none.
/// </summary>
internal static class Texts
{
    private static Dictionary<string, string> vietnamese = new();

    /// <summary>The app is shown in Vietnamese.</summary>
    public static bool Vietnamese { get; private set; }

    /// <summary>
    /// Choose the language: `language` if given ("vi" or "en"), otherwise the
    /// language Windows is shown in.
    /// </summary>
    public static void Load(string? language)
    {
        var wanted = language ?? CultureInfo.CurrentUICulture.TwoLetterISOLanguageName;
        Vietnamese = string.Equals(wanted, "vi", StringComparison.OrdinalIgnoreCase);
        if (!Vietnamese) return;
        try
        {
            var file = Path.Combine(AppContext.BaseDirectory, "Assets", "vi.json");
            vietnamese = JsonSerializer.Deserialize<Dictionary<string, string>>(File.ReadAllText(file)) ?? new();
        }
        catch (Exception)
        {
            // Without the file the app speaks English, which is better than not starting.
            vietnamese = new();
        }
    }

    /// <summary>`english` in the app's language, with `values` in its places {0}, {1}.</summary>
    public static string T(string english, params object[] values)
    {
        var text = Vietnamese && vietnamese.TryGetValue(english, out var found) ? found : english;
        return values.Length == 0 ? text : string.Format(CultureInfo.CurrentCulture, text, values);
    }

    /// <summary>A number with one digit after the point, as the app's language writes it.</summary>
    public static string OneDecimal(double value) => value.ToString("0.0", CultureInfo.CurrentCulture);
}
