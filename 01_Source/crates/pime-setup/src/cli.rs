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

pub const USAGE: &str = "IME1.0 setup (pre-release, not code signed)

Usage: pime-setup.exe [options]
  (no options)     interactive install with the installation wizard
  --silent, /S     no prompts
  --prefix <dir>   where the program is installed. The product folder \"IME1.0\" is appended to it,
                   so \"D:\\\" becomes \"D:\\IME1.0\" (default: <64-bit Program Files>\\IME1.0)
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
    /// `--log <file>` is the elevated child's report file, created by the parent: the child opens it without creating
    /// it and refuses anything that is not that plain, empty file (a link or a pre-existing file planted at the name
    /// would otherwise be written to with administrator rights).
    fn open(path: Option<&Path>) -> Result<Reporter, String> {
        let file = path.map(real::open_log_for_child).transpose()?;
        Ok(Reporter { file })
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
    /// The installation window already showed the failure to the person (no second message box).
    shown: bool,
}

fn failure(code: i32, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
        localized: false,
        shown: false,
    }
}

struct Finish {
    restart_needed: bool,
    notes: Vec<String>,
    /// The installation window already showed the result to the person (no second message box).
    shown: bool,
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
    // Before anything else: an elevated child that cannot trust its log file does nothing at all.
    let mut rep = match Reporter::open(args.log.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "pime-setup: {e}");
            return EXIT_FAILED;
        }
    };
    let interactive = !args.silent && !args.dry_run;
    if interactive {
        real::free_console_if_alone();
    }
    let lang = real::ui_language();
    let system = match real::system_paths() {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "pime-setup: {e}");
            return EXIT_FAILED;
        }
    };
    let outcome = if args.uninstall {
        uninstall_flow(&args, &system, lang, interactive, &mut rep)
    } else {
        install_flow(&args, &system, lang, interactive, &mut rep)
    };
    present(outcome, &args, lang, interactive, &mut rep)
}

/// `Ok(None)` is a dry run (already printed).
type Flow = Result<Option<(Plan, Finish)>, Failure>;

/// The guidance that follows a successful install: the setup never changes the default input method and never
/// activates a profile, so the person has to add the IME to their language list themselves.
///
/// `None` when nothing may be shown: in the silent mode an exit code is the only output, a dry run changes nothing,
/// and an uninstall has no IME to add.
fn next_steps(version: &str, lang: Lang, args: &Args) -> Option<String> {
    if args.silent || args.dry_run || args.uninstall {
        return None;
    }
    Some(text(lang, Msg::NextSteps, &[version]))
}

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
            if interactive && !finish.shown {
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
                // Guidance only: a message box that cannot be shown must not turn a finished install into a failure.
                if let Some(guidance) = next_steps(&plan.version, lang, args) {
                    real::message_box(&title, guidance.trim_end(), Icon::Info, false);
                }
            }
            if finish.restart_needed {
                EXIT_RESTART
            } else {
                EXIT_OK
            }
        }
        Err(f) => {
            rep.line(&format!("FAILED: {}", f.message));
            if interactive && !f.shown && (f.code != EXIT_CANCELLED || f.localized) {
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
    // The place that was chosen and the folder the program goes to are not always the same: the product folder is
    // appended to it, so the program never lands in the root of a drive.
    let chosen = args
        .prefix
        .clone()
        .unwrap_or_else(|| system.default_prefix());
    let prefix = plan::resolve_install_dir(&chosen, system)
        .map_err(|e| failure(EXIT_USAGE, e.to_string()))?;
    let options = InstallOptions {
        register: !args.no_register,
        arp: !args.no_arp,
        vscode: vscode_mode(args),
        release,
        // The installation time is read here, as late as possible: the entry that carries it is written by the steps
        // that follow. It comes from outside the plan, so the plan itself stays reproducible.
        system: SystemPaths {
            local_time: real::local_time(),
            ..system.clone()
        },
    };
    let plan =
        plan::install_plan(&prefix, &options).map_err(|e| failure(EXIT_USAGE, e.to_string()))?;

    if args.dry_run {
        let code = RealBackend::new().find_vscode();
        let mut notes = vec![format!(
            "payload: {} files, version {}, SHA-256 of every file verified",
            options.release.files.len(),
            plan.version
        )];
        // The place that was chosen and the folder the program goes to are not always the same: the rule is
        // written down, so that a script that reads the plan knows where the program ended up.
        if !plan::same_path(&prefix, &chosen) {
            notes.push(format!(
                "the product folder \"{}\" was added: {} -> {}",
                plan::PRODUCT_FOLDER,
                chosen.display(),
                prefix.display()
            ));
        }
        notes.push(match (&code, options.vscode) {
            (_, VscodeMode::Skip) => "VS Code: not touched (--no-vscode)".to_string(),
            (Some(c), _) => format!("VS Code: `code` found at {}", c.display()),
            (None, _) => "VS Code: `code` not found, the extension would be skipped".to_string(),
        });
        rep.line(&plan::render(&plan, &notes));
        return Ok(None);
    }

    if interactive {
        // The window asks where the program goes and what it should do, then shows the install.
        return wizard_install(args, system, lang, &exe, &bytes, payload, options, prefix);
    }

    // The warnings of the plan are reported by `install_core` itself.
    let mut report = |s: &str| rep.line(s);
    let finish = install_core(
        args,
        &plan,
        &options,
        &exe,
        &bytes,
        &payload,
        lang,
        &mut report,
    )?;
    Ok(Some((plan, finish)))
}

/// The install that the window drives: the person chooses the folder and the options, the window shows what happens.
#[allow(clippy::too_many_arguments)]
fn wizard_install(
    args: &Args,
    system: &SystemPaths,
    lang: Lang,
    exe: &Path,
    bytes: &[u8],
    payload: crate::payload::Payload,
    options: InstallOptions,
    prefix: PathBuf,
) -> Flow {
    let version = options.release.version.clone();
    // The three checkboxes start in the state the command line asked for (`--no-register` and friends).
    let wizard_options = crate::wizard::Options {
        register: !args.no_register,
        arp: !args.no_arp,
        vscode: !args.no_vscode,
    };
    // What is already on the machine? When the program is installed, the window says so and offers to repair that
    // install, to install it again or to remove it - the way the setup of another program does it.
    let folders = crate::detect::candidates(&prefix, system, Some(exe));
    let existing = crate::detect::detect(&folders, &RealBackend::new());
    // The uninstall of that install: the plan says what would be removed, and the window shows it before it does.
    let uninstall_options = UninstallOptions {
        vscode: vscode_mode(args),
        system: system.clone(),
    };
    let uninstall_plan = existing
        .as_ref()
        .and_then(|e| e.manifest.as_ref())
        .and_then(|m| plan::uninstall_plan(m, &uninstall_options).ok());
    let uninstall_summary = uninstall_plan.as_ref().map(|p| {
        let notes = p
            .warnings
            .iter()
            .map(|w| format!("\nWARNING: {w}"))
            .collect::<String>();
        text(
            lang,
            Msg::WizardUninstallIntro,
            &[&p.version, &p.prefix.display().to_string(), &notes],
        )
    });
    let Some(mut wizard) = crate::wizard::Wizard::new(
        lang,
        &version,
        &prefix,
        wizard_options,
        system,
        existing.clone(),
        uninstall_summary,
    ) else {
        return Err(failure(
            EXIT_FAILED,
            "the installation window could not be created",
        ));
    };
    let Some(choice) = wizard.welcome() else {
        return Err(failure(EXIT_CANCELLED, "cancelled"));
    };
    // The person chose to remove the program that is already there.
    if let crate::wizard::Choice::Uninstall = choice {
        let Some(manifest) = existing.as_ref().and_then(|e| e.manifest.clone()) else {
            return Err(failure(
                EXIT_FAILED,
                "the install that is on the machine has no manifest: there is nothing that says what it put there, so it cannot be removed file by file",
            ));
        };
        let Some(plan) = uninstall_plan else {
            return Err(failure(
                EXIT_FAILED,
                "the uninstall of the install that is on the machine cannot be planned",
            ));
        };
        return uninstall_in(
            &mut wizard,
            args,
            exe,
            plan,
            manifest,
            uninstall_options,
            lang,
        );
    }
    let (install, job) = match choice {
        crate::wizard::Choice::Install(install) => (install, crate::wizard::Job::Install),
        crate::wizard::Choice::Repair(install) => (install, crate::wizard::Job::Repair),
        crate::wizard::Choice::Reinstall(install) => (install, crate::wizard::Job::Reinstall),
        crate::wizard::Choice::Uninstall => unreachable!("the uninstall is handled above"),
    };
    let options = InstallOptions {
        register: install.register,
        arp: install.arp,
        vscode: if install.vscode {
            VscodeMode::Install
        } else {
            VscodeMode::Skip
        },
        release: options.release,
        system: SystemPaths {
            local_time: real::local_time(),
            ..system.clone()
        },
    };
    let plan = plan::install_plan(&install.prefix, &options)
        .map_err(|e| failure(EXIT_USAGE, e.to_string()))?;

    // A reinstall removes the old install first: what is left of it (files that are no longer shipped, the entry of
    // "Apps & Features") must not survive into the new one.
    let old = if job == crate::wizard::Job::Reinstall {
        match existing.as_ref().and_then(|e| e.manifest.clone()) {
            Some(manifest) => match plan::uninstall_plan(&manifest, &uninstall_options) {
                Ok(old_plan) => Some((old_plan, manifest)),
                Err(e) => {
                    return Err(failure(
                        EXIT_USAGE,
                        format!("the reinstall cannot be planned: {e}"),
                    ));
                }
            },
            None => {
                return Err(failure(
                    EXIT_FAILED,
                    "the install that is on the machine has no manifest: a reinstall cannot remove it first",
                ));
            }
        }
    } else {
        None
    };

    // The install runs on a worker thread; the window stays alive and shows every step. The steps themselves are the
    // ones of the console mode (`install_core`), so the window also covers the case where Windows asks for rights.
    let (tx, rx) = std::sync::mpsc::channel();
    let args = args.clone();
    let exe = exe.to_path_buf();
    let bytes = bytes.to_vec();
    let worker_plan = plan.clone();
    let worker_options = options.clone();
    wizard.progress(move |report| {
        let outcome = match &old {
            Some((old_plan, old_manifest)) => {
                // The old install goes first. If it does not go, the new one does not come on top of it.
                match uninstall_core(
                    &args,
                    &exe,
                    old_plan,
                    &uninstall_options,
                    old_manifest,
                    lang,
                    report,
                ) {
                    Ok(_) => install_core(
                        &args,
                        &worker_plan,
                        &worker_options,
                        &exe,
                        &bytes,
                        &payload,
                        lang,
                        report,
                    ),
                    Err(f) => Err(f),
                }
            }
            None => install_core(
                &args,
                &worker_plan,
                &worker_options,
                &exe,
                &bytes,
                &payload,
                lang,
                report,
            ),
        };
        let _ = tx.send(outcome);
    });
    match rx.recv() {
        Ok(Ok(mut finish)) => {
            let mut notes = finish.notes.clone();
            if finish.restart_needed {
                notes.push(text(lang, Msg::RestartNeeded, &[]));
            }
            // The last page carries the result and the guidance: the window is the only place the person looks.
            let body = format!(
                "{}\n\n{}",
                text(
                    lang,
                    Msg::InstallDone,
                    &[
                        &plan.version,
                        &plan.prefix.display().to_string(),
                        &notes.join("\n"),
                    ],
                ),
                text(lang, Msg::NextSteps, &[&plan.version]),
            );
            wizard.done(true, body.trim_end());
            finish.shown = true;
            Ok(Some((plan, finish)))
        }
        Ok(Err(f)) => {
            wizard.done(false, &f.message);
            Err(Failure { shown: true, ..f })
        }
        Err(_) => {
            let message = "the install stopped before it could report a result";
            wizard.done(false, message);
            Err(Failure {
                code: EXIT_FAILED,
                message: message.to_string(),
                localized: false,
                shown: true,
            })
        }
    }
}

/// The core of the install: the same steps in the console and in the window.
#[allow(clippy::too_many_arguments)]
fn install_core(
    args: &Args,
    plan: &Plan,
    options: &InstallOptions,
    exe: &Path,
    bytes: &[u8],
    payload: &crate::payload::Payload,
    lang: Lang,
    report: &mut dyn FnMut(&str),
) -> Result<Finish, Failure> {
    for w in &plan.warnings {
        report(&format!("WARNING: {w}"));
    }
    if plan.requires_admin && !real::is_elevated() {
        let mut child = vec![
            "--silent".to_string(),
            "--prefix".into(),
            plan.prefix.display().to_string(),
        ];
        child.extend(child_flags(args));
        let mut finish = run_child(exe, child, lang, report)?;
        if options.vscode == VscodeMode::Install
            && let Some(id) = &options.release.extension_id
        {
            let mut warnings = Vec::new();
            exec::install_vscode_extension(
                &mut RealBackend::new(),
                &plan.prefix.join(VSIX),
                id,
                report,
                &mut warnings,
            );
            report_warnings(report, &mut finish, warnings);
        }
        return Ok(finish);
    }
    let mut backend = RealBackend::new();
    let inputs = crate::exec::Inputs {
        payload,
        self_exe: bytes,
    };
    let outcome = exec::execute(plan, &inputs, &mut backend, report)
        .map_err(|e| failure(EXIT_FAILED, e.to_string()))?;
    Ok(finish_of(outcome, report))
}

fn finish_of(outcome: Outcome, rep: &mut dyn FnMut(&str)) -> Finish {
    let mut finish = Finish {
        restart_needed: outcome.restart_needed,
        notes: Vec::new(),
        shown: false,
    };
    report_warnings(rep, &mut finish, outcome.warnings);
    finish
}

fn report_warnings(rep: &mut dyn FnMut(&str), finish: &mut Finish, warnings: Vec<String>) {
    for w in warnings {
        let line = format!("WARNING: {w}");
        rep(&line);
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
    rep: &mut dyn FnMut(&str),
) -> Result<Finish, Failure> {
    // The report file is made here, by the person's own process, under a random name, and kept open (nobody can delete
    // or replace it) until the child is done; the child only opens it, it never creates it.
    let mut log = real::PrivateLog::create().map_err(|e| {
        failure(
            EXIT_FAILED,
            format!("cannot create the report file for the elevated setup: {e}"),
        )
    })?;
    child.push("--log".into());
    child.push(log.path().display().to_string());
    rep("administrator rights are needed: asking Windows for permission (UAC prompt)");
    let code = match real::run_elevated(exe, &child) {
        Ok(code) => code as i32,
        Err(ElevationError::Declined) => {
            return Err(Failure {
                code: EXIT_CANCELLED,
                message: text(lang, Msg::ElevationDeclined, &[]),
                localized: true,
                shown: false,
            });
        }
        Err(ElevationError::Failed(m)) => {
            return Err(failure(
                EXIT_FAILED,
                format!("could not start the elevated setup: {m}"),
            ));
        }
    };
    let log = log.read_all();
    for l in echoed_lines(&log) {
        rep(l);
    }
    interpret_child(code, &log)
}

/// The child's log lines that the parent shows. The child's own `DONE:` / `FAILED:` summary is left out: the parent
/// prints its own, and showing both would report the result twice.
fn echoed_lines(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| !l.starts_with("DONE: ") && !l.starts_with("FAILED: "))
        .collect()
}

/// Turns the elevated child's exit code and log into this process's result: 0 and 3010 are success (3010 asks for a
/// restart), anything else is a failure whose reason is the last `FAILED:` line of the log.
fn interpret_child(code: i32, log: &str) -> Result<Finish, Failure> {
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
            shown: false,
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
    // Where to look for the install manifest: the folder that was named, the folder the program goes to when the
    // product folder is appended to it, the folder this program runs from (the uninstaller copy), the default
    // folder and the folder of an installation from before the rename. The first one that has a manifest wins.
    let beside = exe
        .parent()
        .filter(|d| d.join(MANIFEST_NAME).is_file())
        .map(Path::to_path_buf);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = &args.prefix {
        candidates.push(p.clone());
        if let Ok(resolved) = plan::resolve_install_dir(p, system) {
            candidates.push(resolved);
        }
    }
    if let Some(b) = beside {
        candidates.push(b);
    }
    candidates.push(system.default_prefix());
    candidates.push(system.legacy_prefix());
    let mut found: Option<(PathBuf, String)> = None;
    for candidate in &candidates {
        let manifest_path = candidate.join(MANIFEST_NAME);
        if let Ok(text) = std::fs::read_to_string(&manifest_path) {
            found = Some((candidate.clone(), text));
            break;
        }
    }
    let (prefix, manifest_text) = found.ok_or_else(|| {
        let looked = candidates
            .iter()
            .map(|c| c.join(MANIFEST_NAME).display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        failure(
            EXIT_FAILED,
            format!(
                "no install manifest at {looked}: is {} installed? Use --prefix <dir> if it is elsewhere",
                plan::PRODUCT
            ),
        )
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
        // The window asks for the confirmation, shows every step and the result on its last page. No message box.
        return wizard_uninstall(args, system, lang, &exe, plan, manifest, options);
    }
    // The report of the uninstall goes to the console and to the log of the elevated child.
    let mut report = |s: &str| rep.line(s);
    let finish = uninstall_core(args, &exe, &plan, &options, &manifest, lang, &mut report)?;
    Ok(Some((plan, finish)))
}

/// The uninstall itself: the same steps in the console and in the window.
#[allow(clippy::too_many_arguments)]
fn uninstall_core(
    args: &Args,
    exe: &Path,
    plan: &Plan,
    options: &UninstallOptions,
    manifest: &Manifest,
    lang: Lang,
    report: &mut dyn FnMut(&str),
) -> Result<Finish, Failure> {
    for w in &plan.warnings {
        report(&format!("WARNING: {w}"));
    }
    if plan.requires_admin && !real::is_elevated() {
        let mut child = vec![
            "--silent".to_string(),
            "--prefix".into(),
            plan.prefix.display().to_string(),
        ];
        child.extend(child_flags(args));
        let mut finish = run_child(exe, child, lang, report)?;
        if options.vscode == VscodeMode::Install
            && let Some(id) = &manifest.vscode_extension
        {
            let mut warnings = Vec::new();
            exec::uninstall_vscode_extension(&mut RealBackend::new(), id, report, &mut warnings);
            report_warnings(report, &mut finish, warnings);
        }
        return Ok(finish);
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
    let outcome = exec::execute(plan, &inputs, &mut backend, report)
        .map_err(|e| failure(EXIT_FAILED, e.to_string()))?;
    Ok(finish_of(outcome, report))
}

/// The uninstall that the window drives: the person confirms on the first page, the window shows every step and the
/// result on the last one.
#[allow(clippy::too_many_arguments)]
fn wizard_uninstall(
    args: &Args,
    system: &SystemPaths,
    lang: Lang,
    exe: &Path,
    plan: Plan,
    manifest: Manifest,
    options: UninstallOptions,
) -> Flow {
    let version = plan.version.clone();
    let notes = plan
        .warnings
        .iter()
        .map(|w| format!("\nWARNING: {w}"))
        .collect::<String>();
    let summary = text(
        lang,
        Msg::WizardUninstallIntro,
        &[&version, &plan.prefix.display().to_string(), &notes],
    );
    let Some(mut wizard) =
        crate::wizard::Wizard::new_uninstall(lang, &version, &plan.prefix, summary, system)
    else {
        return Err(failure(
            EXIT_FAILED,
            "the installation window could not be created",
        ));
    };
    // The person confirms on the first page.
    if wizard.welcome().is_none() {
        return Err(failure(EXIT_CANCELLED, "cancelled"));
    }
    uninstall_in(
        &mut wizard,
        args,
        exe,
        plan,
        manifest,
        options,
        lang,
    )
}

/// The uninstall that the window drives: the window shows every step and the result on its last one.
///
/// The window is given, not made here, and the person has already confirmed: the uninstall can be asked for from the
/// page of the program that is already installed, and that page lives in the same window.
#[allow(clippy::too_many_arguments)]
fn uninstall_in(
    wizard: &mut crate::wizard::Wizard,
    args: &Args,
    exe: &Path,
    plan: Plan,
    manifest: Manifest,
    options: UninstallOptions,
    lang: Lang,
) -> Flow {
    // The uninstall runs on a worker thread; the window stays alive and shows every step.
    let (tx, rx) = std::sync::mpsc::channel();
    let args = args.clone();
    let exe = exe.to_path_buf();
    let worker_plan = plan.clone();
    let worker_manifest = manifest.clone();
    let worker_options = options.clone();
    wizard.progress(move |report| {
        let outcome = uninstall_core(
            &args,
            &exe,
            &worker_plan,
            &worker_options,
            &worker_manifest,
            lang,
            report,
        );
        let _ = tx.send(outcome);
    });
    match rx.recv() {
        Ok(Ok(mut finish)) => {
            let mut notes = finish.notes.clone();
            if finish.restart_needed {
                notes.push(text(lang, Msg::RestartNeeded, &[]));
            }
            let body = text(lang, Msg::UninstallDone, &[&notes.join("\n")]);
            wizard.done(true, body.trim_end());
            finish.shown = true;
            Ok(Some((plan, finish)))
        }
        Ok(Err(f)) => {
            wizard.done(false, &f.message);
            Err(Failure { shown: true, ..f })
        }
        Err(_) => {
            let message = "the uninstall stopped before it could report a result";
            wizard.done(false, message);
            Err(Failure {
                code: EXIT_FAILED,
                message: message.to_string(),
                localized: false,
                shown: true,
            })
        }
    }
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

    const CHILD_LOG: &str = "[1/10] create folder C:\\x\nWARNING: VS Code was not found\nDONE: IME1.0 0.2.0 installed (C:\\x)\n";

    #[test]
    fn st_035_the_elevated_childs_exit_code_and_log_become_the_parents_result() {
        let ok = interpret_child(EXIT_OK, CHILD_LOG).ok().unwrap();
        assert!(!ok.restart_needed);
        assert_eq!(ok.notes, ["WARNING: VS Code was not found"]);

        let restart = interpret_child(EXIT_RESTART, CHILD_LOG).ok().unwrap();
        assert!(
            restart.restart_needed,
            "3010 asks for a sign-out or restart"
        );

        // any other code is a failure carrying the child's own reason (the LAST FAILED line) and its exit code
        let log = "[8/10] register\nFAILED: step 8 failed: regsvr32 exited with code 5\n";
        let f = interpret_child(EXIT_FAILED, log).err().unwrap();
        assert_eq!(f.code, EXIT_FAILED);
        assert_eq!(f.message, "step 8 failed: regsvr32 exited with code 5");
        assert!(!f.localized);
        let two = "FAILED: first\nFAILED: second\n";
        assert_eq!(
            interpret_child(EXIT_USAGE, two).err().unwrap().message,
            "second"
        );

        // a child that died without writing a log still yields a clear failure, never a success
        let f = interpret_child(-1, "").err().unwrap();
        assert_eq!(f.code, -1);
        assert!(f.message.contains("exit code -1"));
        assert!(interpret_child(EXIT_FAILED, "").is_err());
    }

    #[test]
    fn st_035_the_child_summary_is_not_echoed_a_second_time() {
        assert_eq!(
            echoed_lines(CHILD_LOG),
            [
                "[1/10] create folder C:\\x",
                "WARNING: VS Code was not found"
            ]
        );
        assert!(echoed_lines("FAILED: boom\nDONE: x\n").is_empty());
    }

    #[test]
    fn st_035_the_guidance_after_an_install_is_shown_only_in_the_interactive_mode() {
        // a double-click: the guidance is shown, in the language of the user interface
        let interactive = parse("").unwrap();
        let guidance = next_steps("0.2.0", Lang::En, &interactive).expect("the guidance");
        assert!(guidance.contains("0.2.0"));
        assert!(guidance.contains("Time & language"));

        // the silent mode shows nothing at all: an exit code is the only output
        assert!(next_steps("0.2.0", Lang::En, &parse("--silent").unwrap()).is_none());
        assert!(next_steps("0.2.0", Lang::En, &parse("/S").unwrap()).is_none());
        // a dry run changes nothing, and an uninstall has no IME to add
        assert!(next_steps("0.2.0", Lang::En, &parse("--dry-run").unwrap()).is_none());
        assert!(next_steps("0.2.0", Lang::En, &parse("--uninstall").unwrap()).is_none());
    }
}
