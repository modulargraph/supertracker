//! The command line.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: supertracker [options] [project.supertracker]

  --ui <folder>            Load the interface from this folder instead of the one next to the
                           executable (also: SUPERTRACKER_UI_DIR).
  --serve [port]           No window: serve the interface and the bridge on 127.0.0.1 (a free port
                           unless one is given) and print the URL to open, access token included.
                           Stop with `quit` on standard input.
  --dialog-answers <file>  Answer file choosers from a JSON array instead of showing them: each
                           chooser takes the next element, a path, or null for Cancel
                           (also: SUPERTRACKER_DIALOG_ANSWERS). For tests.
  --mcp                    No window: serve the Model Context Protocol on standard input and output
                           for an AI client, and pass its tool calls to the running application
                           through the local control socket.
  --ctl <method> [json]    Call one method of the running application through the control socket
                           and print the result, e.g. --ctl summary or
                           --ctl readPattern '{\"pattern\":0}'. --ctl help lists the methods.
  --mk3-probe <seconds> [--exclusive] [--raw]
                           Maschine MK3 hardware diagnostic without a window: prints every control
                           event and lights the LEDs; with --exclusive it takes over the screens
                           (Native Instruments' agents are stopped and restored); --raw prints
                           every input report.
  --help, --version

Settings and the recovery copy live in the support folder (SUPERTRACKER_SUPPORT_DIR overrides it);
so does the control socket (SUPERTRACKER_CONTROL_SOCKET overrides it).
SUPERTRACKER_TRACE=1 logs every bridge request and its outcome to standard error.
SUPERTRACKER_AUDIO_DEVICE=null renders to no audio device; SUPERTRACKER_MIDI=off opens no MIDI device.";

/// What the command line asked for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub ui: Option<PathBuf>,
    /// `Some(0)`: serve on a free port.
    pub serve: Option<u16>,
    pub dialog_answers: Option<PathBuf>,
    /// A project file to open at startup.
    pub open: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    Run(Options),
    /// `--mcp`: the MCP server on standard input and output.
    Mcp,
    /// `--ctl <method> [json]`: the arguments after the flag.
    Control(Vec<String>),
    /// `--mk3-probe <seconds> [--exclusive] [--raw]`.
    Mk3Probe { seconds: f64, exclusive: bool, raw: bool },
    Help,
    Version,
}

/// Reads the arguments after the program name. Environment variables fill what the flags left out.
pub fn parse(arguments: impl IntoIterator<Item = OsString>, environment: impl Fn(&str) -> Option<OsString>) -> Result<Parsed, String> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter().peekable();
    // The probe's own flags: (seconds, exclusive, raw).
    let mut probe: (Option<f64>, bool, bool) = (None, false, false);
    while let Some(argument) = arguments.next() {
        let Some(flag) = argument.to_str().filter(|text| text.starts_with("--")) else {
            if options.open.replace(PathBuf::from(&argument)).is_some() {
                return Err("Only one project file can be opened at startup.".to_string());
            }
            continue;
        };
        let mut value = |what: &str| arguments.next().map(PathBuf::from).ok_or_else(|| format!("{flag} needs {what}."));
        match flag {
            "--help" => return Ok(Parsed::Help),
            "--version" => return Ok(Parsed::Version),
            "--mcp" => return Ok(Parsed::Mcp),
            // Everything after --ctl belongs to the call (the JSON may start with anything).
            "--ctl" => return Ok(Parsed::Control(arguments.by_ref().map(|argument| argument.to_string_lossy().into_owned()).collect())),
            "--mk3-probe" => {
                let seconds = arguments.next().and_then(|text| text.to_str().and_then(|text| text.parse::<f64>().ok())).filter(|seconds| seconds.is_finite() && *seconds > 0.0 && *seconds <= 3600.0);
                probe.0 = Some(seconds.ok_or("--mk3-probe needs a number of seconds (up to 3600).")?);
            }
            "--exclusive" => probe.1 = true,
            "--raw" => probe.2 = true,
            "--ui" => options.ui = Some(value("a folder")?),
            "--dialog-answers" => options.dialog_answers = Some(value("a file")?),
            "--serve" => {
                let port = arguments.peek().and_then(|next| next.to_str()).and_then(|text| text.parse::<u16>().ok());
                if port.is_some() {
                    arguments.next();
                }
                options.serve = Some(port.unwrap_or(0));
            }
            // macOS passes a process serial number to applications started from Finder.
            _ if flag.starts_with("-psn_") => {}
            _ => return Err(format!("Unknown option {flag}.")),
        }
    }
    match probe {
        (Some(seconds), exclusive, raw) => return Ok(Parsed::Mk3Probe { seconds, exclusive, raw }),
        (None, false, false) => {}
        (None, ..) => return Err("--exclusive and --raw belong to --mk3-probe.".to_string()),
    }
    options.ui = options.ui.or_else(|| environment("SUPERTRACKER_UI_DIR").map(PathBuf::from));
    options.dialog_answers = options.dialog_answers.or_else(|| environment("SUPERTRACKER_DIALOG_ANSWERS").map(PathBuf::from));
    Ok(Parsed::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(arguments: &[&str]) -> Result<Parsed, String> {
        parse(arguments.iter().map(OsString::from), |_| None)
    }

    #[test]
    fn flags_and_the_project_file_are_read() {
        assert_eq!(run(&[]), Ok(Parsed::Run(Options::default())));
        assert_eq!(
            run(&["--ui", "/ui", "--serve", "8123", "--dialog-answers", "/a.json", "/music/Song.supertracker"]),
            Ok(Parsed::Run(Options {
                ui: Some("/ui".into()),
                serve: Some(8123),
                dialog_answers: Some("/a.json".into()),
                open: Some("/music/Song.supertracker".into()),
            }))
        );
        // The port is optional; what follows --serve is then something else.
        assert_eq!(run(&["--serve", "Song.supertracker"]), Ok(Parsed::Run(Options { serve: Some(0), open: Some("Song.supertracker".into()), ..Options::default() })));
        assert_eq!(run(&["--serve"]), Ok(Parsed::Run(Options { serve: Some(0), ..Options::default() })));
        assert_eq!(run(&["--help", "--nonsense"]), Ok(Parsed::Help));
        assert_eq!(run(&["--version"]), Ok(Parsed::Version));
    }

    #[test]
    fn the_modes_without_a_window_are_recognized() {
        assert_eq!(run(&["--mcp"]), Ok(Parsed::Mcp));
        assert_eq!(run(&["--ctl", "summary"]), Ok(Parsed::Control(vec!["summary".to_string()])));
        assert_eq!(
            run(&["--ctl", "writeSteps", r#"{"steps":[{"track":0,"row":0,"note":"C-4"}]}"#]),
            Ok(Parsed::Control(vec!["writeSteps".to_string(), r#"{"steps":[{"track":0,"row":0,"note":"C-4"}]}"#.to_string()]))
        );
        // Without a method, the control client itself explains the usage.
        assert_eq!(run(&["--ctl"]), Ok(Parsed::Control(Vec::new())));
        assert_eq!(run(&["--ctl", "help", "--version"]), Ok(Parsed::Control(vec!["help".to_string(), "--version".to_string()])), "what follows --ctl is the call");
        assert_eq!(run(&["--mk3-probe", "10"]), Ok(Parsed::Mk3Probe { seconds: 10.0, exclusive: false, raw: false }));
        assert_eq!(run(&["--mk3-probe", "2.5", "--exclusive", "--raw"]), Ok(Parsed::Mk3Probe { seconds: 2.5, exclusive: true, raw: true }));
        assert_eq!(run(&["--raw", "--mk3-probe", "1"]), Ok(Parsed::Mk3Probe { seconds: 1.0, exclusive: false, raw: true }));
        assert_eq!(run(&["--mk3-probe"]), Err("--mk3-probe needs a number of seconds (up to 3600).".to_string()));
        assert_eq!(run(&["--mk3-probe", "soon"]), Err("--mk3-probe needs a number of seconds (up to 3600).".to_string()));
        assert_eq!(run(&["--exclusive"]), Err("--exclusive and --raw belong to --mk3-probe.".to_string()));
    }

    #[test]
    fn mistakes_are_explained() {
        assert_eq!(run(&["--ui"]), Err("--ui needs a folder.".to_string()));
        assert_eq!(run(&["--frobnicate"]), Err("Unknown option --frobnicate.".to_string()));
        assert_eq!(run(&["a.supertracker", "b.supertracker"]), Err("Only one project file can be opened at startup.".to_string()));
    }

    #[test]
    fn the_environment_fills_in_what_flags_left_out() {
        let environment = |name: &str| match name {
            "SUPERTRACKER_UI_DIR" => Some(OsString::from("/env-ui")),
            "SUPERTRACKER_DIALOG_ANSWERS" => Some(OsString::from("/env-answers.json")),
            _ => None,
        };
        let Ok(Parsed::Run(options)) = parse([OsString::from("--ui"), OsString::from("/flag-ui")], environment) else { panic!() };
        assert_eq!(options.ui, Some("/flag-ui".into()));
        assert_eq!(options.dialog_answers, Some("/env-answers.json".into()));
    }
}
