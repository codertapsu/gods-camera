//! Helpers shared by the hub's integration tests.

use std::path::Path;

/// The two lines every page of the stand-in reader holds.
const FIRST_LINE: &str = "Dòng chữ thứ nhất của trang";
const SECOND_LINE: &str = "dòng chữ thứ hai của trang.";

/// `text` in plain ASCII for the inside of a JSON string: every other letter
/// becomes a JSON escape. Windows PowerShell reads a script that has no
/// byte-order mark in the machine's code page, which would turn Vietnamese
/// letters into nonsense; escapes read the same everywhere.
fn ascii_json(text: &str) -> String {
    let backslash = char::from(0x5c);
    let mut out = String::new();
    for unit in text.encode_utf16() {
        if unit < 0x80 {
            out.push(char::from(unit as u8));
        } else {
            out.push(backslash);
            out.push('u');
            out.push_str(&format!("{unit:04x}"));
        }
    }
    out
}

/// A stand-in for the text reader: a small script that prints one page per
/// picture, each with the same two lines, so the merged document must hold
/// each line once. With `fail` it prints an error and exits with a failure.
///
/// Returns the command the hub should run. The script is a shell script where
/// there is a shell, and a PowerShell script on Windows.
pub fn fake_reader(dir: &Path, fail: bool) -> Vec<String> {
    std::fs::create_dir_all(dir).unwrap();
    if cfg!(windows) {
        let script = dir.join(if fail { "reader-fails.ps1" } else { "reader.ps1" });
        let body = if fail {
            "[Console]::Error.WriteLine('boom')\nexit 3\n".to_string()
        } else {
            r#"$pages = foreach ($f in $args) {
  '{"path":"' + $f.Replace('\', '\\') + '","width":1000,"height":1000,"lines":[' +
  '{"text":"@FIRST@","confidence":0.9,"quad":[100,100,600,100,600,140,100,140],"title":false},' +
  '{"text":"@SECOND@","confidence":0.8,"quad":[100,160,600,160,600,200,100,200],"title":false}' +
  ']}'
}
[Console]::Out.Write('[' + ($pages -join ',') + ']')
"#
            .replace("@FIRST@", &ascii_json(FIRST_LINE))
            .replace("@SECOND@", &ascii_json(SECOND_LINE))
        };
        assert!(body.is_ascii(), "the PowerShell script must be plain ASCII");
        std::fs::write(&script, body).unwrap();
        return ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]
            .iter()
            .map(|part| part.to_string())
            .chain([script.to_string_lossy().into_owned()])
            .collect();
    }
    let script = dir.join(if fail { "reader-fails.sh" } else { "reader.sh" });
    let body = if fail {
        "#!/bin/sh\necho 'boom' >&2\nexit 3\n".to_string()
    } else {
        r#"#!/bin/sh
printf '['
first=1
for f in "$@"; do
  [ $first = 1 ] || printf ','
  first=0
  printf '{"path":"%s","width":1000,"height":1000,"lines":[' "$f"
  printf '{"text":"@FIRST@","confidence":0.9,"quad":[100,100,600,100,600,140,100,140],"title":false},'
  printf '{"text":"@SECOND@","confidence":0.8,"quad":[100,160,600,160,600,200,100,200],"title":false}'
  printf ']}'
done
printf ']'
"#
        .replace("@FIRST@", FIRST_LINE)
        .replace("@SECOND@", SECOND_LINE)
    };
    std::fs::write(&script, body).unwrap();
    vec!["/bin/sh".into(), script.to_string_lossy().into_owned()]
}
