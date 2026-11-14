using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
using uniffi.gc_app;

namespace GodsCamera;

/// <summary>
/// A report the user can save and send when something does not work: the
/// app's state and its log, as the Mac app's report has them. It holds no
/// picture, no face, no token and no names of devices; the user's folder is
/// written as %USERPROFILE%.
/// </summary>
internal static class Diagnostics
{
    public static string Report(Shell shell)
    {
        var text = new StringBuilder();
        void Line(string line) => text.Append(line).Append('\n');
        Line($"God's Camera diagnostics, {DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss zzz", CultureInfo.InvariantCulture)}");
        Line($"app {shell.Version}, {RuntimeInformation.ProcessArchitecture}, .NET {Environment.Version}");
        Line($"Windows {RuntimeInformation.OSDescription}, {Environment.ProcessorCount} processor threads, {GC.GetGCMemoryInfo().TotalAvailableMemoryBytes / (1024 * 1024)} MB of memory");
        Line(shell.AskedForHotKeys
            ? $"hot keys: record {(shell.RecordKeyWorks ? "ours" : "taken by another program")}, loop {(shell.LoopKeyWorks ? "ours" : "taken by another program")}"
            : "hot keys: not asked for (a run that only takes pictures)");
        try
        {
            var state = shell.App.State();
            Line($"engine: {state.Mode}, {state.FramesOut} pictures out, source at {state.SourceFps:0.0} a second, resting {state.Resting}");
            Line($"source: {state.Source}, problem {state.SourceProblem?.ToString() ?? "none"}, {state.Cameras.Length} camera(s) attached, mirror {state.Mirror}");
            Line($"camera for other apps: {state.VirtualCamera} {state.VirtualCameraDetail}, watched {state.Watched} at {state.OutputWidth} x {state.OutputHeight}");
            Line($"loop: recording {state.IsRecording}, loop in hand {state.Clip != null}, {state.SavedClips.Length} saved, style {state.LoopStyle}, back to live on speaking {state.BackToLiveOnVoice}, microphone {state.MicrophoneProblem?.ToString() ?? "fine"}");
            Line($"face swap: wanted {state.SwapWanted}, {state.SwapState} {state.SwapDetail}, swapping {state.Swapping}, {state.SwapMs:0} ms a swap, {state.SwapAgeMs:0} ms behind, error {state.SwapError ?? "none"}");
            Line($"faces: {state.Faces.Length} saved, quality {state.Quality}, sharpen {state.Sharpen:0.00}, colour {state.ColourMatch}, objects in front {state.KeepObjectsInFront}, real mouth {state.RealMouth}, blur {state.BlurWhenUnswapped}, AI badge {state.ShowAiBadge}");
        }
        catch (Exception problem)
        {
            Line($"the app's state could not be read: {problem.Message}");
        }
        try
        {
            var scanner = shell.Scanner.State();
            Line($"scanner: wanted {scanner.Wanted}, running {scanner.Running}, network {scanner.Network}, {scanner.Devices.Length} device(s), {scanner.Documents.Length} document(s), reader {(scanner.ReaderMissing == null ? "ready" : "missing " + scanner.ReaderMissing)}");
        }
        catch (Exception problem)
        {
            Line($"the scanner's state could not be read: {problem.Message}");
        }
        Line("");
        Line("--- log ---");
        try
        {
            Line(Log.File != null && File.Exists(Log.File) ? File.ReadAllText(Log.File) : "(no log file)");
        }
        catch (IOException problem)
        {
            Line($"(the log could not be read: {problem.Message})");
        }
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return string.IsNullOrEmpty(home) ? text.ToString() : text.ToString().Replace(home, "%USERPROFILE%", StringComparison.OrdinalIgnoreCase);
    }
}
