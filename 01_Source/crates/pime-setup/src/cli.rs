//! Command line, prompts, elevation and the final report. The decisions (what to install, in which order, how to undo)
//! live in `plan` and `exec`; this module only wires them to the real machine.
//!
//! Elevation: when the plan writes HKLM or a protected folder and the process is not elevated, the setup restarts
//! itself with "runas" (one UAC prompt) as `--silent ... --vscode-deferred --log <file>`, waits, and shows the child's
//! log. The one step that must run as the *person who started the setup* (`code --install-extension`, which installs
//! into the user's profile) is done by this non-elevated parent after the child succeeded.

use crate::backend::Backend;
use crate::exec::{self, Inputs, Outcome};
use crate::payload::{Payload, VSIX};
use crate::plan::{
    self, InstallOptions, MANIFEST_NAME, Manifest, Plan, SystemPaths, UninstallOptions, VscodeMode,
};
use crate::real::{self, ElevationError, Icon, RealBackend};
use crate::text::{Lang, Msg, text};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
/// `ERROR_CANCELLED`: the user answered No, or declined the UAC prompt.
pub const EXIT_CANCELLED: i32 = 1223;
/// `ERROR_SUCCESS_REBOOT_REQUIRED`: done, but a restart or sign-out is needed to finish.
pub const EXIT_RESTART: i32 = 3010;

pub const USAGE: &str = "Programming IME setup (pre-release, not code signed)

Usage: pime-setup.exe [options]
  (no options)     interactive install with a few prompts
  --silent, /S     no prompts
  --prefix <dir>   install folder (default: %ProgramFiles%\\Programming IME)
  --dry-run        print the plan and exit; nothing is changed
  --no-register    do not register the text service (regsvr32)
  --no-arp         do not add the Add/Remove Programs entry
  --no-vscode      do not install the VS Code extension
  --uninstall      remove an existing install (reads install-manifest.json)
  --help           this text

Exit codes: 0 ok, 1 failed (an install is rolled back), 2 bad arguments or a refused folder,
1223 cancelled, 3010 ok but a restart or sign-out is needed to finish.
";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub uninstall: bool,
    pub silent: bool,
    pub dry_run: bool,
    pub prefix: Option<PathBuf>,
    pub no_register: bool,
    pub no_arp: bool,
    pub no_vscode: bool,
    pub help: bool,
    /// Internal (used by the elevated child): append the report to this file.
    pub log: Option<PathBuf>,
    /// Internal (used by the elevated child): leave `code` to the non-elevated parent.
    pub vscode_deferred: bool,
}

pub fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = raw.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match name {
            "--silent" | "/S" | "/s" | "/silent" => a.silent = true,
            "--dry-run" => a.dry_run = true,
            "--no-register" => a.no_register = true,
            "--no-arp" => a.no_arp = true,
            "--no-vscode" => a.no_vscode = true,
            "--uninstall" => a.uninstall = true,
            "--vscode-deferred" => a.vscode_deferred = true,
            "--help" | "-h" | "/?" => a.help = true,
            "--prefix" => a.prefix = Some(PathBuf::from(value("--prefix")?)),
            "--log" => a.log = Some(PathBuf::from(value("--log")?)),
            _ => return Err(format!("unknown option {arg}")),
        }
        if inline.is_some() && !matches!(name, "--prefix" | "--log") {
            return Err(format!("{name} does not take a value"));
        }
    }
    if a.uninstall && (a.no_register || a.no_arp) {
        return Err("--no-register and --no-arp only apply to an install".into());
    }
    if a.no_vscode && a.vscode_deferred {
        return Err("--no-vscode and --vscode-deferred exclude each other".into());
    }
    Ok(a)
}

fn say(s: &str) {
    let _ = writeln!(std::io::stdout(), "{s}");
}

/// Writes to the console and, when asked, to a log file (the elevated child's console is invisible).
struct Reporter {
    file: Option<std::fs::File>,
}

impl Reporter {
    fn open(path: Option<&Path>) -> Reporter {
        let file = path.and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        Reporter { file }
    }

    fn line(&mut self, s: &str) {
        say(s);
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{s}");
        }
    }
}

struct Failure {
    code: i32,
    message: String,
    /// The message is already a complete sentence in the user's language (no "the setup did not finish" wrapper).
    localized: bool,
}

fn failure(code: i32, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
        localized: false,
    }
}

struct Finish {
    restart_needed: bool,
    notes: Vec<String>,
}

pub fn run(raw: Vec<String>) -> i32 {
    let args = match parse_args(&raw) {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "pime-setup: {e}\n\n{USAGE}");
            return EXIT_USAGE;
        }
    };
    if args.help {
        say(USAGE);
        return EXIT_OK;
    }
    let interactive = !args.silent && !args.dry_run;
    if interactive {
        real::free_console_if_alone();
    }
    let lang = real::ui_language();
    let system = SystemPaths::from_env();
    let mut rep = Reporter::open(args.log.as_deref());
    let outcome = if args.uninstall {
        uninstall_flow(&args, &system, lang, interactive, &mut rep)
    } else {
        install_flow(&args, &system, lang, interactive, &mut rep)
    };
    present(outcome, &args, lang, interactive, &mut rep)
}

/// `Ok(None)` is a dry run (already printed).
type Flow = Result<Option<(Plan, Finish)>, Failure>;

fn present(flow: Flow, args: &Args, lang: Lang, interactive: bool, rep: &mut Reporter) -> i32 {
    let title = text(lang, Msg::Title, &[]);
    match flow {
        Ok(None) => EXIT_OK,
        Ok(Some((plan, mut finish))) => {
            if finish.restart_needed {
                finish.notes.push(text(lang, Msg::RestartNeeded, &[]));
            }
            let notes = finish.notes.join("\n");
            let verb = if args.uninstall {
                "uninstalled"
            } else {
                "installed"
            };
            rep.line(&format!(
                "DONE: {} {} {verb} ({})",
                plan::PRODUCT,
                plan.version,
                plan.prefix.display()
            ));
            if interactive {
                let body = if args.uninstall {
                    text(lang, Msg::UninstallDone, &[&notes])
                } else {
                    text(
                        lang,
                        Msg::InstallDone,
                        &[&plan.version, &plan.prefix.display().to_string(), &notes],
                    )
                };
                real::message_box(&title, body.trim_end(), Icon::Info, false);
            }
            if finish.restart_needed {
                EXIT_RESTART
            } else {
                EXIT_OK
            }
        }
        Err(f) => {
            rep.line(&format!("FAILED: {}", f.message));
            if interactive && (f.code != EXIT_CANCELLED || f.localized) {
                let body = if f.localized {
                    f.message.clone()
                } else {
                    text(lang, Msg::Failed, &[&f.message])
                };
                real::message_box(&title, &body, Icon::Error, false);
            }
            f.code
        }
    }
}

fn vscode_mode(args: &Args) -> VscodeMode {
    if args.no_vscode {
        VscodeMode::Skip
    } else if args.vscode_deferred {
        VscodeMode::Defer
    } else {
        VscodeMode::Install
    }
}

fn install_flow(
    args: &Args,
    system: &SystemPaths,
    lang: Lang,
    interactive: bool,
    rep: &mut Reporter,
) -> Flow {
    let (exe, bytes) = real::current_exe_bytes()
        .map_err(|e| failure(EXIT_FAILED, format!("cannot read this program: {e}")))?;
    let payload = match Payload::from_exe(&bytes) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(failure(
                EXIT_FAILED,
                "this pime-setup.exe carries no payload (it is the unpacked stub, or the download is incomplete). Use ProgrammingIME-<version>-win-x64-setup.exe, built by scripts/build-installer.js",
            ));
        }
        Err(e) => return Err(failure(EXIT_FAILED, e.to_string())),
    };
    let release = payload
        .release()
        .map_err(|e| failure(EXIT_FAILED, e.to_string()))?;
    let prefix = args
        .prefix
        .clone()
        .unwrap_or_else(|| system.default_prefix());
    let options = InstallOptions {
        register: !args.no_register,
        arp: !args.no_arp,
        vscode: vscode_mode(args),
        release,
        system: system.clone(),
    };
    let plan =
        plan::install_plan(&prefix, &options).map_err(|e| failure(EXIT_USAGE, e.to_string()))?;

    if args.dry_run {
        let code = RealBackend::new().find_vscode();
        let notes = [
            format!(
                "payload: {} files, version {}, SHA-256 of every file verified",
                options.release.files.len(),
                plan.version
            ),
            match (&code, options.vscode) {
                (_, VscodeMode::Skip) => "VS Code: not touched (--no-vscode)".to_string(),
                (Some(c), _) => format!("VS Code: `code` found at {}", c.display()),
                (None, _) => {
                    "VS Code: `code` not found, the extension would be skipped".to_string()
                }
            },
        ];
        rep.line(&plan::render(&plan, &notes));
        return Ok(None);
    }

    if interactive {
        let notes = plan
            .warnings
            .iter()
            .map(|w| format!("\nWARNING: {w}"))
            .collect::<String>();
        let ask = text(
            lang,
            Msg::ConfirmInstall,
            &[&plan.version, &plan.prefix.display().to_string(), &notes],
        );
        if !real::message_box(&text(lang, Msg::Title, &[]), &ask, Icon::Question, true) {
            return Err(failure(EXIT_CANCELLED, "cancelled"));
        }
    }

    for w in &plan.warnings {
        rep.line(&format!("WARNING: {w}"));
    }
    if plan.requires_admin && !real::is_elevated() {
        let mut child = vec![
            "--silent".to_string(),
            "--prefix".into(),
            plan.prefix.display().to_string(),
        ];
        child.extend(child_flags(args));
        let mut finish = run_child(&exe, child, lang, rep)?;
        if options.vscode == VscodeMode::Install
            && let Some(id) = &options.release.extension_id
        {
            let mut warnings = Vec::new();
            exec::install_vscode_extension(
                &mut RealBackend::new(),
                &plan.prefix.join(VSIX),
                id,
                &mut |s| rep.line(s),
                &mut warnings,
            );
            report_warnings(rep, &mut finish, warnings);
        }
        return Ok(Some((plan, finish)));
    }

    let mut backend = RealBackend::new();
    let inputs = Inputs {
        payload: &payload,
        self_exe: &bytes,
    };
    let outcome = exec::execute(&plan, &inputs, &mut backend, &mut |s| rep.line(s))
        .map_err(|e| failure(EXIT_FAILED, e.to_string()))?;
    Ok(Some((plan, finish_of(outcome, rep))))
}

fn finish_of(outcome: Outcome, rep: &mut Reporter) -> Finish {
    let mut finish = Finish {
        restart_needed: outcome.restart_needed,
        notes: Vec::new(),
    };
    report_warnings(rep, &mut finish, outcome.warnings);
    finish
}

fn report_warnings(rep: &mut Reporter, finish: &mut Finish, warnings: Vec<String>) {
    for w in warnings {
        let line = format!("WARNING: {w}");
        rep.line(&line);
        finish.notes.push(line);
    }
}

/// The flags the elevated child inherits (it always gets `--silent`, `--prefix` and a log file on top).
fn child_flags(args: &Args) -> Vec<String> {
    let mut v = Vec::new();
    if args.uninstall {
        v.push("--uninstall".to_string());
    }
    for (on, flag) in [
        (args.no_register, "--no-register"),
        (args.no_arp, "--no-arp"),
        (args.no_vscode, "--no-vscode"),
        (!args.no_vscode, "--vscode-deferred"),
    ] {
        if on {
            v.push(flag.to_string());
        }
    }
    v
}

/// Restarts this program elevated with `child` arguments and waits; turns its exit code and log into a result.
fn run_child(
    exe: &Path,
    mut child: Vec<String>,
    lang: Lang,
    rep: &mut Reporter,
) -> Result<Finish, Failure> {
    let log_path = std::env::temp_dir().join(format!("pime-setup-{}.log", std::process::id()));
    child.push("--log".into());
    child.push(log_path.display().to_string());
    rep.line("administrator rights are needed: asking Windows for permission (UAC prompt)");
    let code = match real::run_elevated(exe, &child) {
        Ok(code) => code as i32,
        Err(ElevationError::Declined) => {
            return Err(Failure {
                code: EXIT_CANCELLED,
                message: text(lang, Msg::ElevationDeclined, &[]),
                localized: true,
            });
        }
        Err(ElevationError::Failed(m)) => {
            return Err(failure(
                EXIT_FAILED,
                format!("could not start the elevated setup: {m}"),
            ));
        }
    };
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    for l in log
        .lines()
        .filter(|l| !l.starts_with("DONE: ") && !l.starts_with("FAILED: "))
    {
        rep.line(l);
    }
    let pick = |prefix: &str| -> Vec<String> {
        log.lines()
            .filter(|l| l.starts_with(prefix))
            .map(str::to_string)
            .collect()
    };
    match code {
        EXIT_OK | EXIT_RESTART => Ok(Finish {
            restart_needed: code == EXIT_RESTART,
            notes: pick("WARNING: "),
        }),
        _ => {
            let reason = pick("FAILED: ")
                .pop()
                .map(|l| l.trim_start_matches("FAILED: ").to_string());
            Err(failure(
                code,
                reason
                    .unwrap_or_else(|| format!("the elevated setup stopped with exit code {code}")),
            ))
        }
    }
}

fn uninstall_flow(
    args: &Args,
    system: &SystemPaths,
    lang: Lang,
    interactive: bool,
    rep: &mut Reporter,
) -> Flow {
    let exe = std::env::current_exe()
        .map_err(|e| failure(EXIT_FAILED, format!("cannot locate this program: {e}")))?;
    // Where to look: --prefix, else the folder this program runs from (the uninstaller copy), else the default folder.
    let beside = exe
        .parent()
        .filter(|d| d.join(MANIFEST_NAME).is_file())
        .map(Path::to_path_buf);
    let prefix = args
        .prefix
        .clone()
        .or(beside)
        .unwrap_or_else(|| system.default_prefix());
    let manifest_path = prefix.join(MANIFEST_NAME);
    let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        failure(EXIT_FAILED, format!("no install manifest at {} ({e}): is {} installed in that folder? Use --prefix <dir> if it is elsewhere", manifest_path.display(), plan::PRODUCT))
    })?;
    let manifest = Manifest::parse(&manifest_text, &prefix)
        .map_err(|e| failure(EXIT_FAILED, e.to_string()))?;
    let options = UninstallOptions {
        vscode: vscode_mode(args),
        system: system.clone(),
    };
    let plan = plan::uninstall_plan(&manifest, &options)
        .map_err(|e| failure(EXIT_USAGE, e.to_string()))?;

    if args.dry_run {
        rep.line(&plan::render(&plan, &[]));
        return Ok(None);
    }
    if interactive {
        let ask = text(
            lang,
            Msg::ConfirmUninstall,
            &[&plan.version, &plan.prefix.display().to_string()],
        );
        if !real::message_box(&text(lang, Msg::Title, &[]), &ask, Icon::Question, true) {
            return Err(failure(EXIT_CANCELLED, "cancelled"));
        }
    }
    if plan.requires_admin && !real::is_elevated() {
        let mut child = vec![
            "--silent".to_string(),
            "--prefix".into(),
            plan.prefix.display().to_string(),
        ];
        child.extend(child_flags(args));
        let mut finish = run_child(&exe, child, lang, rep)?;
        if options.vscode == VscodeMode::Install
            && let Some(id) = &manifest.vscode_extension
        {
            let mut warnings = Vec::new();
            exec::uninstall_vscode_extension(
                &mut RealBackend::new(),
                id,
                &mut |s| rep.line(s),
                &mut warnings,
            );
            report_warnings(rep, &mut finish, warnings);
        }
        return Ok(Some((plan, finish)));
    }
    let mut backend = RealBackend::new();
    // An uninstall needs no payload; the plan only removes what the manifest lists.
    let empty = Payload {
        entries: Vec::new(),
    };
    let inputs = Inputs {
        payload: &empty,
        self_exe: &[],
    };
    let outcome = exec::execute(&plan, &inputs, &mut backend, &mut |s| rep.line(s))
        .map_err(|e| failure(EXIT_FAILED, e.to_string()))?;
    Ok(Some((plan, finish_of(outcome, rep))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        let v: Vec<String> = s.split_whitespace().map(str::to_string).collect();
        parse_args(&v)
    }

    #[test]
    fn st_035_all_documented_options_parse() {
        let a =
            parse("--silent --prefix D:\\Tools\\PIME --no-register --no-arp --no-vscode --dry-run")
                .unwrap();
        assert!(a.silent && a.no_register && a.no_arp && a.no_vscode && a.dry_run && !a.uninstall);
        assert_eq!(a.prefix, Some(PathBuf::from("D:\\Tools\\PIME")));
        assert!(parse("/S").unwrap().silent && parse("/s").unwrap().silent);
        assert_eq!(
            parse("--prefix=E:\\X").unwrap().prefix,
            Some(PathBuf::from("E:\\X"))
        );
        assert!(parse("--uninstall").unwrap().uninstall);
        assert_eq!(parse("").unwrap(), Args::default());
        // a folder with spaces arrives as ONE argument
        let spaced =
            parse_args(&["--prefix".to_string(), "D:\\My Tools\\PIME".to_string()]).unwrap();
        assert_eq!(spaced.prefix, Some(PathBuf::from("D:\\My Tools\\PIME")));
    }

    #[test]
    fn st_035_bad_arguments_are_refused_not_guessed() {
        assert!(parse("--bogus").is_err());
        assert!(parse("--prefix").unwrap_err().contains("needs a value"));
        assert!(parse("--silent=yes").is_err());
        assert!(parse("--uninstall --no-register").is_err());
        assert!(parse("--no-vscode --vscode-deferred").is_err());
    }

    #[test]
    fn st_035_a_double_click_is_interactive_and_a_temp_prefix_run_never_elevates() {
        // no arguments: interactive. The elevation decision is the plan's (see plan tests); here: flags reach the child.
        assert!(!parse("").unwrap().silent);
        let a = parse("--no-register --no-arp --no-vscode").unwrap();
        assert_eq!(
            child_flags(&a),
            ["--no-register", "--no-arp", "--no-vscode"]
        );
        let b = parse("").unwrap();
        assert_eq!(child_flags(&b), ["--vscode-deferred"]);
        let u = parse("--uninstall").unwrap();
        assert_eq!(child_flags(&u), ["--uninstall", "--vscode-deferred"]);
    }
}
