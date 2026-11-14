# The scanned documents for an assistant: `gc mcp`

`gc mcp` offers the documents God's Camera made from scanned pages to an assistant that speaks the Model Context Protocol, such as Claude Desktop or Claude Code (plan, Phase 10). The assistant starts it as a command and talks to it on standard input and output, one JSON-RPC message a line. It listens on no port, and starts nothing.

## Tools

| Tool | What it does |
|---|---|
| `list_documents` | The documents, newest first: id, title (the first line), state (`done`, `running`, `queued` or `failed`), when it was made, how many photos went in, and why a failed one failed. |
| `read_document` | One document's text, as Markdown (`format: "markdown"`, the default) or plain text (`format: "text"`). |
| `export_document` | One document saved to a new file as a Word document (`docx`) or a PDF (`pdf`). The path must be absolute and end in the format's extension; an existing file is never replaced. |

It reads the desktop app's hub folder: `%LOCALAPPDATA%\GodsCamera\Hub` on Windows, `~/Library/Application Support/GodsCamera/Hub` on the Mac, or the folder given with `--hub-dir`. Whatever it reads goes to the assistant it is connected to, and from there to that assistant's model: connect it only to an assistant you would show the documents to.

## Connecting it

Build the tool once: `cargo build --release -p gc-cli` in `core`. Then add it to the assistant as a command. For Claude Desktop, in its `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "gods-camera": {
      "command": "D:\\path\\to\\gods-camera\\core\\target\\release\\gc.exe",
      "args": ["mcp"]
    }
  }
}
```

For Claude Code: `claude mcp add gods-camera -- /path/to/core/target/release/gc mcp`.

## Checked

`core/gc-cli/tests/mcp.rs` starts the real binary and drives it as an assistant does: the handshake in two protocol versions, the tool list, every tool on a hub folder laid out as the app's, a path given where an id belongs, an existing file that must stay, and a tool or method that does not exist.
