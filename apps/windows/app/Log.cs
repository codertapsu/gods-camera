using System.Globalization;

namespace GodsCamera;

/// <summary>
/// The app's log: a plain text file in its data folder, Logs\gods-camera.log.
/// It holds what the app did and what went wrong. It never holds a picture, a
/// face, or the name of a file the user chose.
/// </summary>
internal static class Log
{
    private const long Longest = 1_000_000;
    private static readonly object Turn = new();
    private static string? file;

    /// <summary>Where the log is, once `Start` was called.</summary>
    public static string? File => file;

    public static void Start(string dataFolder)
    {
        try
        {
            var folder = Path.Combine(dataFolder, "Logs");
            Directory.CreateDirectory(folder);
            file = Path.Combine(folder, "gods-camera.log");
            // Keep the newer half when the file has grown long.
            var info = new FileInfo(file);
            if (info.Exists && info.Length > Longest)
            {
                var text = System.IO.File.ReadAllText(file);
                System.IO.File.WriteAllText(file, text[(text.Length / 2)..]);
            }
        }
        catch (Exception)
        {
            // Without a log the app still runs.
            file = null;
        }
    }

    public static void Write(string words)
    {
        if (file == null) return;
        var line = DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss.fff", CultureInfo.InvariantCulture) + "  " + words + Environment.NewLine;
        lock (Turn)
        {
            try
            {
                System.IO.File.AppendAllText(file, line);
            }
            catch (IOException)
            {
                // A line lost is better than an app stopped.
            }
        }
    }

    public static void Problem(string where, Exception problem) => Write($"PROBLEM in {where}: {problem}");
}
