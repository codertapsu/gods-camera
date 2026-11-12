using System.Runtime.InteropServices;

namespace GodsCamera;

/// <summary>
/// The few things the window needs from Windows that WinUI does not offer:
/// an icon in the notification area, hot keys that work while another app
/// has the keyboard, and a say in the window's own messages.
/// </summary>
internal static class Native
{
    public const uint WM_HOTKEY = 0x0312;
    public const uint WM_APP = 0x8000;
    public const uint WM_LBUTTONUP = 0x0202;
    public const uint WM_RBUTTONUP = 0x0205;
    public const uint WM_CONTEXTMENU = 0x007B;
    public const uint WM_NULL = 0x0000;

    public const uint MOD_ALT = 0x0001;
    public const uint MOD_CONTROL = 0x0002;
    public const uint MOD_SHIFT = 0x0004;
    public const uint MOD_NOREPEAT = 0x4000;

    public const uint NIM_ADD = 0, NIM_MODIFY = 1, NIM_DELETE = 2, NIM_SETVERSION = 4;
    public const uint NIF_MESSAGE = 0x01, NIF_ICON = 0x02, NIF_TIP = 0x04, NIF_INFO = 0x10, NIF_SHOWTIP = 0x80;
    public const uint NOTIFYICON_VERSION_4 = 4;
    public const uint NIIF_INFO = 0x01, NIIF_RESPECT_QUIET_TIME = 0x80;

    public const uint MF_STRING = 0x0000, MF_SEPARATOR = 0x0800, MF_GRAYED = 0x0001;
    public const uint TPM_RIGHTBUTTON = 0x0002, TPM_RETURNCMD = 0x0100, TPM_NONOTIFY = 0x0080;

    public const int SW_RESTORE = 9;
    public const int SM_CXSMICON = 49, SM_CYSMICON = 50;
    public const uint IMAGE_ICON = 1, LR_LOADFROMFILE = 0x0010;

    public static readonly IntPtr HWND_BROADCAST = new(0xFFFF);

    public const int GWLP_WNDPROC = -4;

    public delegate IntPtr WindowProc(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct NotifyIconData
    {
        public uint cbSize;
        public IntPtr hWnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public uint dwState;
        public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public uint uVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public uint dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Point
    {
        public int X;
        public int Y;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern bool Shell_NotifyIconW(uint message, ref NotifyIconData data);

    // The window's own message handler is put in front of the one it had.
    // (The helper for this in comctl32 needs version 6 of that library, which
    // a program only gets by asking for it in its manifest.)
    [DllImport("user32.dll")]
    public static extern IntPtr SetWindowLongPtrW(IntPtr window, int index, IntPtr value);

    [DllImport("user32.dll")]
    public static extern IntPtr CallWindowProcW(IntPtr previous, IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool RegisterHotKey(IntPtr window, int id, uint modifiers, uint key);

    [DllImport("user32.dll")]
    public static extern bool UnregisterHotKey(IntPtr window, int id);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint RegisterWindowMessageW(string name);

    [DllImport("user32.dll")]
    public static extern bool PostMessageW(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr window, int command);

    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr window);

    [DllImport("user32.dll")]
    public static extern uint GetDpiForWindow(IntPtr window);

    [DllImport("user32.dll")]
    public static extern int GetSystemMetrics(int index);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr LoadImageW(IntPtr instance, string name, uint type, int width, int height, uint load);

    [DllImport("user32.dll")]
    public static extern bool DestroyIcon(IntPtr icon);

    [DllImport("user32.dll")]
    public static extern IntPtr CreatePopupMenu();

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern bool AppendMenuW(IntPtr menu, uint flags, UIntPtr id, string? text);

    [DllImport("user32.dll")]
    public static extern bool DestroyMenu(IntPtr menu);

    [DllImport("user32.dll")]
    public static extern int TrackPopupMenuEx(IntPtr menu, uint flags, int x, int y, IntPtr window, IntPtr reserved);

    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out Point point);
}

/// <summary>The app's icon in the notification area, beside the clock.</summary>
internal sealed class TrayIcon : IDisposable
{
    /// <summary>The message Windows sends the window when the icon is clicked.</summary>
    public const uint Message = Native.WM_APP + 1;

    private readonly IntPtr window;
    private readonly IntPtr icon;
    private bool added;
    private string tip = "";

    public TrayIcon(IntPtr window, string iconFile)
    {
        this.window = window;
        icon = Native.LoadImageW(
            IntPtr.Zero, iconFile, Native.IMAGE_ICON, Native.GetSystemMetrics(Native.SM_CXSMICON),
            Native.GetSystemMetrics(Native.SM_CYSMICON), Native.LR_LOADFROMFILE);
    }

    private Native.NotifyIconData Data(uint flags) => new()
    {
        cbSize = (uint)Marshal.SizeOf<Native.NotifyIconData>(),
        hWnd = window,
        uID = 1,
        uFlags = flags,
        uCallbackMessage = Message,
        hIcon = icon,
        szTip = tip,
        szInfo = "",
        szInfoTitle = "",
    };

    /// <summary>Put the icon there. Also after Explorer was restarted, which forgets it.</summary>
    public void Add(string words)
    {
        tip = Cut(words, 127);
        var data = Data(Native.NIF_MESSAGE | Native.NIF_ICON | Native.NIF_TIP | Native.NIF_SHOWTIP);
        added = Native.Shell_NotifyIconW(Native.NIM_ADD, ref data);
        data.uVersion = Native.NOTIFYICON_VERSION_4;
        Native.Shell_NotifyIconW(Native.NIM_SETVERSION, ref data);
    }

    /// <summary>The words shown when the pointer rests on the icon.</summary>
    public void SetTip(string words)
    {
        words = Cut(words, 127);
        if (!added || words == tip) return;
        tip = words;
        var data = Data(Native.NIF_TIP | Native.NIF_SHOWTIP);
        Native.Shell_NotifyIconW(Native.NIM_MODIFY, ref data);
    }

    /// <summary>Say something once, in a small notice above the icon.</summary>
    public void Tell(string title, string words)
    {
        if (!added) return;
        var data = Data(Native.NIF_INFO);
        data.szInfoTitle = Cut(title, 63);
        data.szInfo = Cut(words, 255);
        data.dwInfoFlags = Native.NIIF_INFO | Native.NIIF_RESPECT_QUIET_TIME;
        Native.Shell_NotifyIconW(Native.NIM_MODIFY, ref data);
    }

    private static string Cut(string words, int most) => words.Length <= most ? words : words[..most];

    /// <summary>
    /// Show a menu at the pointer and return the number of the chosen entry, or
    /// 0 for none. An entry is its number and its words; number 0 is a line.
    /// </summary>
    public int Menu(params (int Number, string Words)[] entries)
    {
        var menu = Native.CreatePopupMenu();
        foreach (var (number, words) in entries)
        {
            if (number == 0) Native.AppendMenuW(menu, Native.MF_SEPARATOR, UIntPtr.Zero, null);
            else Native.AppendMenuW(menu, Native.MF_STRING, (UIntPtr)(uint)number, words);
        }
        Native.GetCursorPos(out var at);
        // Without this the menu would stay open when the user clicks elsewhere.
        Native.SetForegroundWindow(window);
        var chosen = Native.TrackPopupMenuEx(
            menu, Native.TPM_RIGHTBUTTON | Native.TPM_RETURNCMD | Native.TPM_NONOTIFY, at.X, at.Y, window, IntPtr.Zero);
        Native.PostMessageW(window, Native.WM_NULL, IntPtr.Zero, IntPtr.Zero);
        Native.DestroyMenu(menu);
        return chosen;
    }

    public void Dispose()
    {
        if (added)
        {
            var data = Data(0);
            Native.Shell_NotifyIconW(Native.NIM_DELETE, ref data);
            added = false;
        }
        if (icon != IntPtr.Zero) Native.DestroyIcon(icon);
    }
}
