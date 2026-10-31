//! Runs a [`Plan`] against a [`Backend`].
//!
//! Install is transactional: every step that changes something pushes its inverse on an undo stack first, a failing
//! step unwinds the stack, and the old files are only deleted once everything succeeded (a file that exists is never
//! overwritten in place: the new content is written beside it, the old file is renamed aside, and the rename is what
//! can be undone). Replacing works even when the old file is a DLL mapped into running programs, because Windows
//! allows renaming such a file but not overwriting or deleting it. Uninstall has no rollback: it removes what the
//! manifest lists, keeps going past problems and reports them, so a damaged install can always be removed.

use crate::backend::{Backend, RegValue, is_in_use};
use crate::payload::Payload;
use crate::plan::{
    ARP_KEY, Action, FileSource, MANIFEST_NAME, Manifest, Plan, Step, TEXT_SERVICE_CLSID,
    inside_prefix, is_within, path_key, same_path,
};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub struct Inputs<'a> {
    pub payload: &'a Payload,
    /// Bytes of the running setup (copied to the install folder as the uninstaller).
    pub self_exe: &'a [u8],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum VscodeStatus {
    #[default]
    NotRequested,
    Done,
    NotFound,
    Failed(String),
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub warnings: Vec<String>,
    /// A file could not be replaced or deleted because a program is using it: Windows finishes the job at the next
    /// restart, and programs that already loaded the old DLL keep it until they are restarted.
    pub restart_needed: bool,
    pub vscode: VscodeStatus,
}

#[derive(Debug)]
pub struct ExecError {
    /// 1-based, 0 for the checks before the first step.
    pub step: usize,
    pub what: String,
    pub cause: String,
    pub rollback_problems: Vec<String>,
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "step {} ({}) failed: {}.",
            self.step, self.what, self.cause
        )?;
        if self.rollback_problems.is_empty() {
            write!(f, " Everything that had been done was rolled back.")
        } else {
            write!(
                f,
                " Rolling back left problems: {}.",
                self.rollback_problems.join("; ")
            )
        }
    }
}

impl std::error::Error for ExecError {}

pub fn regsvr32_meaning(code: i32) -> &'static str {
    match code {
        1 => "invalid argument",
        2 => "OleInitialize failed",
        3 => "the DLL could not be loaded",
        4 => "the DLL is not a COM server (no DllRegisterServer export)",
        5 => "DllRegisterServer / DllUnregisterServer reported a failure",
        _ => "unknown regsvr32 error",
    }
}

enum Undo {
    RemoveFile(PathBuf),
    RestoreFile {
        dest: PathBuf,
        backup: PathBuf,
    },
    RemoveDir(PathBuf),
    Unregister(PathBuf),
    /// The text service was registered before this install: put the old registration back once the old DLL is back.
    ReRegisterAfterRestore(PathBuf),
    DeleteKey(String),
    RestoreKey {
        key: String,
        values: Vec<(String, RegValue)>,
    },
}

fn io_text(e: &io::Error) -> String {
    format!("{e}")
}

fn side_name(dest: &Path, suffix: &str, n: u32) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(suffix);
    if n > 0 {
        name.push(n.to_string());
    }
    PathBuf::from(name)
}

struct Runner<'a> {
    plan: &'a Plan,
    inputs: &'a Inputs<'a>,
    b: &'a mut dyn Backend,
    log: &'a mut dyn FnMut(&str),
    undo: Vec<Undo>,
    /// Old versions of replaced files; removed once the install succeeded.
    backups: Vec<PathBuf>,
    created_dirs: Vec<PathBuf>,
    prior: Option<Manifest>,
    out: Outcome,
}

/// Runs the plan. `log` receives one line per step and per notable event.
pub fn execute(
    plan: &Plan,
    inputs: &Inputs,
    backend: &mut dyn Backend,
    log: &mut dyn FnMut(&str),
) -> Result<Outcome, ExecError> {
    match plan.action {
        Action::Install => install(plan, inputs, backend, log),
        Action::Uninstall => Ok(uninstall(plan, backend, log)),
    }
}

fn read_prior_manifest(
    b: &dyn Backend,
    plan: &Plan,
    log: &mut dyn FnMut(&str),
) -> Option<Manifest> {
    let path = plan.prefix.join(MANIFEST_NAME);
    if !b.exists(&path) {
        return None;
    }
    let text = b
        .read_file(&path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    match text.map(|t| Manifest::parse(&t, &plan.prefix)) {
        Some(Ok(m)) => Some(m),
        _ => {
            log(
                "note: the existing install manifest is unreadable and is ignored (files of an older version that are no longer shipped stay in place)",
            );
            None
        }
    }
}

/// Entries of the pending-rename list that would act on a file inside `prefix` at the next restart. Folders (the
/// prefix itself and its parents) are harmless, Windows only removes a folder that is empty by then, and so are the
/// names this installer renamed its in-use files to (no install ever writes such a name).
fn pending_conflicts(entries: &[String], prefix: &Path) -> Vec<String> {
    entries
        .iter()
        .filter(|e| !e.is_empty() && is_within(Path::new(e), prefix, false) && !is_aside_name(e))
        .cloned()
        .collect()
}

/// Windows may still have deletions of files inside the install folder queued for the next restart (from an older
/// setup that scheduled the paths of live files, or from anything else). Installing now would let them delete the new
/// files at boot, so the install is refused until the restart. Only plans that need administrator rights look: only an
/// elevated process can queue such an entry, and an install into a temporary folder stays off HKLM altogether.
fn refuse_while_deletions_are_pending(
    plan: &Plan,
    b: &dyn Backend,
    log: &mut dyn FnMut(&str),
) -> Result<(), ExecError> {
    if !plan.requires_admin {
        return Ok(());
    }
    let entries = match b.pending_file_renames() {
        Ok(entries) => entries,
        Err(e) => {
            log(&format!(
                "note: could not read the list of file operations Windows has queued for the next restart ({}); continuing",
                io_text(&e)
            ));
            return Ok(());
        }
    };
    let hits = pending_conflicts(&entries, &plan.prefix);
    let Some(first) = hits.first() else {
        return Ok(());
    };
    Err(ExecError {
        step: 0,
        what: "pre-flight check".into(),
        cause: format!(
            "Windows still has {} file operation(s) inside {} queued for the next restart (for example {first}), and they would delete or replace the files installed now; restart the computer and run the setup again",
            hits.len(),
            plan.prefix.display()
        ),
        rollback_problems: Vec::new(),
    })
}

fn install(
    plan: &Plan,
    inputs: &Inputs,
    b: &mut dyn Backend,
    log: &mut dyn FnMut(&str),
) -> Result<Outcome, ExecError> {
    refuse_while_deletions_are_pending(plan, b, log)?;
    let prior = read_prior_manifest(b, plan, log);
    // Another install elsewhere owns the Add/Remove Programs entry (and the registration): installing here would orphan it.
    if plan
        .steps
        .iter()
        .any(|s| matches!(s, Step::WriteArp { .. }))
        && let Ok(Some(values)) = b.reg_read_key(ARP_KEY)
        && let Some((_, RegValue::Str(location))) =
            values.iter().find(|(n, _)| n == "InstallLocation")
        && !same_path(Path::new(location), &plan.prefix)
    {
        return Err(ExecError {
            step: 0,
            what: "pre-flight check".into(),
            cause: format!(
                "{} is already installed in {location}; uninstall it first or install into the same folder",
                crate::plan::PRODUCT
            ),
            rollback_problems: Vec::new(),
        });
    }
    let mut r = Runner {
        plan,
        inputs,
        b,
        log,
        undo: Vec::new(),
        backups: Vec::new(),
        created_dirs: Vec::new(),
        prior,
        out: Outcome::default(),
    };
    for (i, step) in plan.steps.iter().enumerate() {
        (r.log)(&format!(
            "[{}/{}] {}",
            i + 1,
            plan.steps.len(),
            step.describe()
        ));
        if let Err(cause) = r.run_step(step) {
            let rollback_problems = r.rollback();
            return Err(ExecError {
                step: i + 1,
                what: step.describe(),
                cause,
                rollback_problems,
            });
        }
    }
    r.commit();
    Ok(r.out)
}

impl Runner<'_> {
    fn run_step(&mut self, step: &Step) -> Result<(), String> {
        match step {
            Step::CreateDir(path) => {
                self.create_dir_all(path)?;
                // An install folder that already is a junction would send every file somewhere else.
                self.b.check_plain(path, path)
            }
            Step::PlaceFile { dest, source } => {
                self.b.check_plain(&self.plan.prefix, dest)?;
                let bytes = self.content(source)?;
                self.place_file(dest, &bytes)
            }
            Step::RegisterTextService { dll } => {
                self.b.check_plain(&self.plan.prefix, dll)?;
                // Undo first: a failing regsvr32 may still have written part of the registration.
                let was_registered = self
                    .prior
                    .as_ref()
                    .is_some_and(|m| m.text_service_dlls.iter().any(|d| same_path(d, dll)));
                self.undo.push(if was_registered {
                    Undo::ReRegisterAfterRestore(dll.clone())
                } else {
                    Undo::Unregister(dll.clone())
                });
                let args = ["/s".to_string(), dll.display().to_string()];
                match self.b.run(&self.plan.regsvr32, &args) {
                    Ok(0) => Ok(()),
                    Ok(code) => Err(format!(
                        "regsvr32 exited with code {code}: {}",
                        regsvr32_meaning(code)
                    )),
                    Err(e) => Err(format!("regsvr32 could not be started: {}", io_text(&e))),
                }
            }
            Step::WriteArp { key, values } => {
                let previous = self.b.reg_read_key(key).map_err(|e| io_text(&e))?;
                self.undo.push(match previous {
                    Some(values) => Undo::RestoreKey {
                        key: key.clone(),
                        values,
                    },
                    None => Undo::DeleteKey(key.clone()),
                });
                self.b.reg_write_key(key, values).map_err(|e| io_text(&e))
            }
            Step::InstallVscodeExtension { vsix, extension_id } => {
                let status = install_vscode_extension(
                    self.b,
                    vsix,
                    extension_id,
                    self.log,
                    &mut self.out.warnings,
                );
                self.out.vscode = status;
                Ok(())
            }
            other => Err(format!(
                "a step that belongs to uninstalling was found in an install plan: {}",
                other.describe()
            )),
        }
    }

    fn content(&self, source: &FileSource) -> Result<Vec<u8>, String> {
        match source {
            FileSource::Payload(name) => self
                .inputs
                .payload
                .get(name)
                .map(|e| e.data.clone())
                .ok_or_else(|| format!("{name} is not in the payload")),
            FileSource::SelfExe => Ok(self.inputs.self_exe.to_vec()),
            FileSource::Manifest => {
                let mut manifest = self.plan.manifest.clone();
                let mut dirs: Vec<PathBuf> = self
                    .prior
                    .iter()
                    .flat_map(|m| m.directories.iter().cloned())
                    .collect();
                for d in &self.created_dirs {
                    if !dirs.iter().any(|x| same_path(x, d)) {
                        dirs.push(d.clone());
                    }
                }
                manifest.directories = dirs;
                Ok(manifest.to_json().into_bytes())
            }
        }
    }

    /// `create_dir_all`, remembering which folders did not exist (the manifest lists them, uninstall removes them).
    fn create_dir_all(&mut self, path: &Path) -> Result<(), String> {
        let mut missing = Vec::new();
        let mut at = Some(path);
        while let Some(p) = at {
            if self.b.exists(p) {
                if !self.b.is_dir(p) {
                    return Err(format!("{} exists and is not a folder", p.display()));
                }
                break;
            }
            missing.push(p.to_path_buf());
            at = p.parent().filter(|q| !q.as_os_str().is_empty());
        }
        for dir in missing.into_iter().rev() {
            self.b
                .create_dir(&dir)
                .map_err(|e| format!("cannot create {}: {}", dir.display(), io_text(&e)))?;
            self.undo.push(Undo::RemoveDir(dir.clone()));
            self.created_dirs.push(dir);
        }
        Ok(())
    }

    fn place_file(&mut self, dest: &Path, bytes: &[u8]) -> Result<(), String> {
        if self.b.is_dir(dest) {
            return Err(format!("{} is a folder", dest.display()));
        }
        if !self.b.exists(dest) {
            self.undo.push(Undo::RemoveFile(dest.to_path_buf())); // before the write: a partial file must go too
            return self
                .b
                .write_file(dest, bytes)
                .map_err(|e| format!("cannot write {}: {}", dest.display(), io_text(&e)));
        }
        if self.b.read_file(dest).is_ok_and(|old| old == bytes) {
            (self.log)(&format!("      unchanged: {}", dest.display()));
            return Ok(());
        }
        // Replace: write beside, move the old file aside, move the new one in.
        let fresh = side_name(dest, ".pime-new", 0);
        if self.b.exists(&fresh) {
            let _ = self.b.remove_file(&fresh);
        }
        self.undo.push(Undo::RemoveFile(fresh.clone()));
        self.b
            .write_file(&fresh, bytes)
            .map_err(|e| format!("cannot write {}: {}", fresh.display(), io_text(&e)))?;
        let backup = (0..20u32)
            .map(|n| side_name(dest, ".pime-old", n))
            .find(|p| !self.b.exists(p))
            .ok_or_else(|| {
                format!(
                    "too many leftover .pime-old files next to {}",
                    dest.display()
                )
            })?;
        self.b.rename(dest, &backup).map_err(|e| {
            format!(
                "cannot move {} out of the way: {}",
                dest.display(),
                io_text(&e)
            )
        })?;
        self.undo.push(Undo::RestoreFile {
            dest: dest.to_path_buf(),
            backup: backup.clone(),
        });
        self.b.rename(&fresh, dest).map_err(|e| {
            format!(
                "cannot move the new {} into place: {}",
                dest.display(),
                io_text(&e)
            )
        })?;
        self.backups.push(backup);
        Ok(())
    }

    fn rollback(&mut self) -> Vec<String> {
        (self.log)("rolling back");
        let mut problems = Vec::new();
        let mut deferred = Vec::new();
        while let Some(undo) = self.undo.pop() {
            let result: Result<(), String> =
                match undo {
                    Undo::RemoveFile(p) => match self.b.remove_file(&p) {
                        Err(e) if e.kind() != io::ErrorKind::NotFound => {
                            Err(format!("cannot remove {}: {}", p.display(), io_text(&e)))
                        }
                        _ => Ok(()),
                    },
                    Undo::RestoreFile { dest, backup } => match self.b.remove_file(&dest) {
                        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(format!(
                            "cannot remove the new {}: {}",
                            dest.display(),
                            io_text(&e)
                        )),
                        _ => self.b.rename(&backup, &dest).map_err(|e| {
                            format!("cannot restore {}: {}", dest.display(), io_text(&e))
                        }),
                    },
                    Undo::RemoveDir(p) => self.b.remove_dir(&p).map_err(|e| {
                        format!("cannot remove folder {}: {}", p.display(), io_text(&e))
                    }),
                    Undo::Unregister(dll) => self.regsvr32(&["/u", "/s"], &dll),
                    Undo::ReRegisterAfterRestore(dll) => {
                        deferred.push(dll);
                        Ok(())
                    }
                    Undo::DeleteKey(key) => self
                        .b
                        .reg_delete_key(&key)
                        .map_err(|e| format!("cannot delete HKLM\\{key}: {}", io_text(&e))),
                    Undo::RestoreKey { key, values } => self
                        .b
                        .reg_delete_key(&key)
                        .and_then(|()| self.b.reg_write_key(&key, &values))
                        .map_err(|e| format!("cannot restore HKLM\\{key}: {}", io_text(&e))),
                };
            if let Err(p) = result {
                (self.log)(&format!("      rollback problem: {p}"));
                problems.push(p);
            }
        }
        for dll in deferred {
            if let Err(p) = self.regsvr32(&["/s"], &dll) {
                problems.push(p);
            }
        }
        problems
    }

    fn regsvr32(&mut self, flags: &[&str], dll: &Path) -> Result<(), String> {
        self.b.check_plain(&self.plan.prefix, dll)?;
        let mut args: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
        args.push(dll.display().to_string());
        match self.b.run(&self.plan.regsvr32, &args) {
            Ok(0) => Ok(()),
            Ok(code) => Err(format!(
                "regsvr32 {} exited with code {code}",
                flags.join(" ")
            )),
            Err(e) => Err(format!("regsvr32 could not be started: {}", io_text(&e))),
        }
    }

    /// Everything worked: drop the old versions and the files an older release shipped that this one does not.
    fn commit(&mut self) {
        let mut doomed: Vec<PathBuf> = std::mem::take(&mut self.backups);
        if let Some(prior) = &self.prior {
            let keep: Vec<String> = self
                .plan
                .manifest
                .files
                .iter()
                .map(|f| path_key(f))
                .collect();
            doomed.extend(
                prior
                    .files
                    .iter()
                    .filter(|f| {
                        !keep.contains(&path_key(f)) && inside_prefix(f, &self.plan.prefix, false)
                    })
                    .cloned(),
            );
        }
        for path in doomed {
            if !self.b.exists(&path) {
                continue;
            }
            if let Err(why) = self.b.check_plain(&self.plan.prefix, &path) {
                self.out.warnings.push(format!(
                    "{} was left in place (not deleted): {why}",
                    path.display()
                ));
                continue;
            }
            if let Err(e) = self.b.remove_file(&path) {
                defer_file_delete(self.b, &path, &e, self.log, &mut self.out);
            }
        }
    }
}

/// The marker of a file that was renamed aside to be deleted at the next restart: `<name>.pime-del-<pid>-<n>`.
const ASIDE_MARK: &str = ".pime-del-";

/// A name next to `path` that nothing uses (the process id keeps two setups apart, `n` two files of one run; a name
/// that is still around from an earlier run, pids are reused, is skipped).
fn aside_name(b: &dyn Backend, path: &Path) -> Option<PathBuf> {
    let pid = std::process::id();
    (0..1000u32)
        .map(|n| {
            let mut name = path.as_os_str().to_owned();
            name.push(format!("{ASIDE_MARK}{pid}-{n}"));
            PathBuf::from(name)
        })
        .find(|p| !b.exists(p))
}

/// True for `...<ASIDE_MARK><digits>-<digits>` (any case): a name this installer made up, which no install ever writes.
fn is_aside_name(entry: &str) -> bool {
    let lower = entry.to_lowercase();
    let Some(at) = lower.rfind(ASIDE_MARK) else {
        return false;
    };
    let mut parts = lower[at + ASIDE_MARK.len()..].split('-');
    let digits =
        |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// A file that cannot be deleted right now (a running image such as `uninstall.exe`, a DLL that programs have loaded).
/// Windows can finish the job at the next restart, but `MoveFileEx(DELAY_UNTIL_REBOOT)` deletes whatever is at the
/// scheduled PATH by then, so the original path is never scheduled: a setup run before the restart would have its
/// freshly installed file deleted at boot. The file is renamed to a unique name in the same folder (Windows allows that
/// for a mapped image) and THAT name is scheduled. Returns the scheduled name; when the file cannot be moved aside it
/// is left alone, reported, and nothing is scheduled.
fn defer_file_delete(
    b: &mut dyn Backend,
    path: &Path,
    cause: &io::Error,
    log: &mut dyn FnMut(&str),
    out: &mut Outcome,
) -> Option<PathBuf> {
    let Some(aside) = aside_name(b, path) else {
        out.warnings.push(format!(
            "could not delete {} ({}) and there are too many leftover {ASIDE_MARK}* files next to it to move it aside; delete it yourself after the programs that use it are closed",
            path.display(),
            io_text(cause)
        ));
        return None;
    };
    if let Err(e) = b.rename(path, &aside) {
        out.warnings.push(format!(
            "could not delete {} ({}) nor move it aside ({}), so it was NOT scheduled for deletion at the next restart (that would also delete a file installed there later); delete it yourself after the programs that use it are closed",
            path.display(),
            io_text(cause),
            io_text(&e)
        ));
        return None;
    }
    match b.schedule_delete_on_reboot(&aside) {
        Ok(()) => {
            log(&format!(
                "      {} is in use ({}); it was renamed to {} and that file is deleted at the next restart",
                path.display(),
                io_text(cause),
                aside.display()
            ));
            out.restart_needed = true;
            Some(aside)
        }
        Err(e) => {
            out.warnings.push(format!(
                "could not delete {} ({}): it was renamed to {} but its deletion at the next restart could not be scheduled ({}); delete that file yourself",
                path.display(),
                io_text(cause),
                aside.display(),
                io_text(&e)
            ));
            None
        }
    }
}

/// `code --install-extension <vsix> --force`, only when `code` exists. Never fails the install.
pub fn install_vscode_extension(
    b: &mut dyn Backend,
    vsix: &Path,
    extension_id: &str,
    log: &mut dyn FnMut(&str),
    warnings: &mut Vec<String>,
) -> VscodeStatus {
    let Some(code) = b.find_vscode() else {
        warnings.push(format!(
            "VS Code (the `code` command) was not found, so extension {extension_id} was not installed. Later: code --install-extension \"{}\" --force",
            vsix.display()
        ));
        return VscodeStatus::NotFound;
    };
    let args = [
        "--install-extension".to_string(),
        vsix.display().to_string(),
        "--force".to_string(),
    ];
    match b.run(&code, &args) {
        Ok(0) => {
            log(&format!("      installed VS Code extension {extension_id}"));
            VscodeStatus::Done
        }
        Ok(n) => {
            let m = format!(
                "`code --install-extension` exited with code {n}; the extension was not installed (the rest of the install is fine)"
            );
            warnings.push(m.clone());
            VscodeStatus::Failed(m)
        }
        Err(e) => {
            let m = format!(
                "`code` could not be run ({}); the extension was not installed (the rest of the install is fine)",
                io_text(&e)
            );
            warnings.push(m.clone());
            VscodeStatus::Failed(m)
        }
    }
}

/// `code --uninstall-extension <id>`; the extension may have been removed by hand, so a failure is only reported.
pub fn uninstall_vscode_extension(
    b: &mut dyn Backend,
    extension_id: &str,
    log: &mut dyn FnMut(&str),
    warnings: &mut Vec<String>,
) -> VscodeStatus {
    let Some(code) = b.find_vscode() else {
        warnings.push(format!("VS Code was not found: remove the extension yourself with: code --uninstall-extension {extension_id}"));
        return VscodeStatus::NotFound;
    };
    match b.run(
        &code,
        &[
            "--uninstall-extension".to_string(),
            extension_id.to_string(),
        ],
    ) {
        Ok(0) => {
            log(&format!("      removed VS Code extension {extension_id}"));
            VscodeStatus::Done
        }
        other => {
            let m = format!(
                "the VS Code extension {extension_id} could not be removed ({}); remove it yourself with: code --uninstall-extension {extension_id}",
                match other {
                    Ok(n) => format!("exit code {n}"),
                    Err(e) => io_text(&e),
                }
            );
            warnings.push(m.clone());
            VscodeStatus::Failed(m)
        }
    }
}

/// `regsvr32 /u` could not be run on `dll`, so its registration may still be in the registry. The installer does not
/// remove registry keys of the text service itself (that is the DLL's own `DllUnregisterServer`, in crates/pime-tsf);
/// it tells the person how to finish by hand.
fn unregister_skipped(dll: &Path, why: &str) -> String {
    format!(
        "{}: {why}, so the text service was NOT unregistered and its registration may still be in the registry. To clean up, either run this setup again (it puts the DLL back and registers it) and then uninstall again, or, as administrator: reg delete \"HKLM\\Software\\Classes\\CLSID\\{TEXT_SERVICE_CLSID}\" /f /reg:64 and, as a last resort for the input method entries that Windows keeps for it: reg delete \"HKLM\\SOFTWARE\\Microsoft\\CTF\\TIP\\{TEXT_SERVICE_CLSID}\" /f /reg:64 (see docs/INSTALLER.md, section 6)",
        dll.display()
    )
}

/// Where the checks for links start: the highest folder this installer created (the install folder itself when it
/// created no parent), so that a link anywhere from there down to the file is found, not only one below the install
/// folder. The manifest is validated: its folders are the install folder or parents of it.
fn check_root(plan: &Plan) -> &Path {
    plan.manifest
        .directories
        .iter()
        .filter(|d| is_within(&plan.prefix, d, true))
        .min_by_key(|d| path_key(d).len())
        .map_or(plan.prefix.as_path(), |d| d.as_path())
}

fn uninstall(plan: &Plan, b: &mut dyn Backend, log: &mut dyn FnMut(&str)) -> Outcome {
    let root = check_root(plan);
    let mut out = Outcome::default();
    // Files and folders that Windows will delete at the next restart; a folder that only holds those is scheduled too.
    let mut pending: Vec<PathBuf> = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        log(&format!(
            "[{}/{}] {}",
            i + 1,
            plan.steps.len(),
            step.describe()
        ));
        match step {
            Step::UninstallVscodeExtension { extension_id } => {
                out.vscode = uninstall_vscode_extension(b, extension_id, log, &mut out.warnings);
            }
            Step::UnregisterTextService { dll } => {
                if !b.exists(dll) {
                    out.warnings.push(unregister_skipped(
                        dll,
                        "the DLL file is already gone, so regsvr32 /u has nothing to load",
                    ));
                    continue;
                }
                if let Err(why) = b.check_plain(root, dll) {
                    out.warnings.push(unregister_skipped(
                        dll,
                        &format!("regsvr32 /u was not run on it: {why}"),
                    ));
                    continue;
                }
                let args = [
                    "/u".to_string(),
                    "/s".to_string(),
                    dll.display().to_string(),
                ];
                match b.run(&plan.regsvr32, &args) {
                    Ok(0) => {}
                    Ok(code) => out.warnings.push(format!(
                        "regsvr32 /u exited with code {code} ({}); the files are removed anyway so that the uninstall can finish",
                        regsvr32_meaning(code)
                    )),
                    Err(e) => out.warnings.push(format!("regsvr32 could not be started ({}); the files are removed anyway", io_text(&e))),
                }
            }
            Step::DeleteArp { key } => {
                if let Err(e) = b.reg_delete_key(key) {
                    out.warnings
                        .push(format!("could not delete HKLM\\{key}: {}", io_text(&e)));
                }
            }
            Step::RemoveFile(path) => {
                if !b.exists(path) {
                    continue;
                }
                if let Err(why) = b.check_plain(root, path) {
                    out.warnings.push(format!(
                        "{} was left in place (not deleted): {why}",
                        path.display()
                    ));
                    continue;
                }
                if let Err(e) = b.remove_file(path)
                    && let Some(aside) = defer_file_delete(b, path, &e, log, &mut out)
                {
                    pending.push(aside);
                }
            }
            Step::RemoveDir(dir) => {
                if !b.exists(dir) {
                    continue;
                }
                if let Err(why) = b.check_plain(root, dir) {
                    out.warnings.push(format!(
                        "folder {} was left in place (not removed): {why}",
                        dir.display()
                    ));
                    continue;
                }
                if !b.is_dir(dir) {
                    continue;
                }
                match b.dir_is_empty(dir) {
                    Ok(true) => {
                        if let Err(e) = b.remove_dir(dir) {
                            schedule_dir(b, dir, &e, log, &mut out, &mut pending);
                        }
                    }
                    Ok(false) if pending.iter().any(|p| is_within(p, dir, false)) => {
                        let cause = io::Error::from_raw_os_error(145);
                        schedule_dir(b, dir, &cause, log, &mut out, &mut pending);
                    }
                    Ok(false) => out.warnings.push(format!(
                        "{} was kept: it contains files that this installer did not create",
                        dir.display()
                    )),
                    Err(e) => out.warnings.push(format!(
                        "cannot inspect {}: {}",
                        dir.display(),
                        io_text(&e)
                    )),
                }
            }
            other => out.warnings.push(format!(
                "ignored a step that belongs to installing: {}",
                other.describe()
            )),
        }
    }
    out
}

/// A folder is scheduled under its own path: Windows removes a folder at restart only while it is empty, so a
/// reinstall before the restart (whose files keep the folder non-empty) is safe.
fn schedule_dir(
    b: &mut dyn Backend,
    path: &Path,
    cause: &io::Error,
    log: &mut dyn FnMut(&str),
    out: &mut Outcome,
    pending: &mut Vec<PathBuf>,
) {
    let reason = if is_in_use(cause) || cause.raw_os_error() == Some(145) {
        "is in use"
    } else {
        "could not be deleted"
    };
    match b.schedule_delete_on_reboot(path) {
        Ok(()) => {
            log(&format!(
                "      {} {reason} ({}); it is deleted at the next restart",
                path.display(),
                io_text(cause)
            ));
            out.restart_needed = true;
            pending.push(path.to_path_buf());
        }
        Err(e) => out.warnings.push(format!(
            "could not delete {} ({}) nor schedule its deletion ({})",
            path.display(),
            io_text(cause),
            io_text(&e)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::payload::{self, Entry};
    use crate::plan::tests::{PREFIX, options, release, system};
    use crate::plan::{PayloadFile, UninstallOptions, VscodeMode, install_plan, uninstall_plan};

    const DLL: &str = r"C:\Program Files\Programming IME\pime_tsf.dll";

    fn machine() -> FakeBackend {
        FakeBackend::with_dirs(&[r"C:\", r"C:\Program Files", r"C:\Users\me", r"D:\"])
    }

    fn payload_for(version: &str, names: &[String]) -> Payload {
        Payload {
            entries: names
                .iter()
                .map(|n| {
                    let data = format!("{n}@{version}").into_bytes();
                    Entry {
                        name: n.clone(),
                        sha256: pime_core::sha256::sha256(&data),
                        data,
                    }
                })
                .collect(),
        }
    }

    /// Installs `version` into `prefix`; `extra` are additional payload files (a release that ships more files).
    fn install_into(
        b: &mut FakeBackend,
        prefix: &str,
        version: &str,
        extra: &[&str],
        tweak: impl Fn(&mut crate::plan::InstallOptions),
    ) -> Result<Outcome, ExecError> {
        let mut o = options();
        o.release = release(version);
        for e in extra {
            o.release.files.push(PayloadFile {
                name: e.to_string(),
                size: 10,
                sha256: [1; 32],
            });
        }
        tweak(&mut o);
        let names: Vec<String> = o.release.files.iter().map(|f| f.name.clone()).collect();
        let plan = install_plan(Path::new(prefix), &o).unwrap();
        let payload = payload_for(version, &names);
        let self_exe = format!("setup-{version}").into_bytes();
        execute(
            &plan,
            &Inputs {
                payload: &payload,
                self_exe: &self_exe,
            },
            b,
            &mut |_| {},
        )
    }

    fn install(b: &mut FakeBackend, version: &str) -> Result<Outcome, ExecError> {
        install_into(b, PREFIX, version, &[], |_| {})
    }

    fn uninstall(b: &mut FakeBackend, vscode: VscodeMode) -> Outcome {
        let text = String::from_utf8(b.read_file(&Path::new(PREFIX).join(MANIFEST_NAME)).unwrap())
            .unwrap();
        let manifest = Manifest::parse(&text, Path::new(PREFIX)).unwrap();
        let plan = uninstall_plan(
            &manifest,
            &UninstallOptions {
                vscode,
                system: system(),
            },
        )
        .unwrap();
        let none = Payload {
            entries: Vec::new(),
        };
        execute(
            &plan,
            &Inputs {
                payload: &none,
                self_exe: &[],
            },
            b,
            &mut |_| {},
        )
        .unwrap()
    }

    fn k(p: &str) -> String {
        path_key(Path::new(p))
    }

    fn file_names(b: &FakeBackend) -> Vec<String> {
        b.state.files.keys().cloned().collect()
    }

    #[test]
    fn st_035_a_fresh_install_creates_exactly_the_planned_state() {
        let mut b = machine();
        let out = install(&mut b, "0.2.0").unwrap();
        assert!(out.warnings.is_empty() && !out.restart_needed);
        assert_eq!(out.vscode, VscodeStatus::Done);
        let dir = |n: &str| k(&format!(r"{PREFIX}\{n}"));
        let mut expected = vec![
            dir("install-manifest.json"),
            dir("pime_tsf.dll"),
            dir("pime-core.exe"),
            dir("license"),
            dir("programming-ime.vsix"),
            dir("uninstall.exe"),
        ];
        expected.sort();
        assert_eq!(file_names(&b), expected);
        assert_eq!(
            b.file(&format!(r"{PREFIX}\pime_tsf.dll")).unwrap(),
            b"pime_tsf.dll@0.2.0"
        );
        assert_eq!(
            b.file(&format!(r"{PREFIX}\uninstall.exe")).unwrap(),
            b"setup-0.2.0"
        );
        assert!(b.state.registered.contains(&k(DLL)));
        let arp = b
            .state
            .registry
            .get(&ARP_KEY.to_lowercase())
            .expect("ARP key");
        assert!(arp.contains(&("DisplayVersion".to_string(), RegValue::Str("0.2.0".into()))));
        // the manifest on disk lists exactly the files that exist, and the folder the installer created
        let text = String::from_utf8(
            b.file(&format!(r"{PREFIX}\install-manifest.json"))
                .unwrap()
                .clone(),
        )
        .unwrap();
        let manifest = Manifest::parse(&text, Path::new(PREFIX)).unwrap();
        let mut listed: Vec<String> = manifest.files.iter().map(|f| path_key(f)).collect();
        listed.sort();
        assert_eq!(listed, expected);
        assert_eq!(manifest.directories, [PathBuf::from(PREFIX)]);
        // regsvr32 is the 64-bit one from System32, and the only other program run is `code`
        assert_eq!(b.commands.len(), 2);
        assert!(b.commands[0].starts_with(r"C:\Windows\System32\regsvr32.exe /s "));
        assert!(
            b.commands[1].contains("--install-extension") && b.commands[1].ends_with("--force")
        );
        assert!(!b.state.files.keys().any(|f| f.contains(".pime-")));
    }

    #[test]
    fn st_035_a_prefix_with_missing_parents_spaces_and_non_ascii_is_created_and_recorded() {
        let mut b = machine();
        let prefix = r"D:\日本語 フォルダ\Programming IME";
        install_into(&mut b, prefix, "0.2.0", &[], |o| {
            o.vscode = VscodeMode::Skip
        })
        .unwrap();
        let text = String::from_utf8(
            b.file(&format!(r"{prefix}\install-manifest.json"))
                .unwrap()
                .clone(),
        )
        .unwrap();
        let m = Manifest::parse(&text, Path::new(prefix)).unwrap();
        assert_eq!(
            m.directories,
            [PathBuf::from(r"D:\日本語 フォルダ"), PathBuf::from(prefix)]
        );
        assert!(b.commands[0].contains(r"D:\日本語 フォルダ\Programming IME\pime_tsf.dll"));
        // uninstalling removes both folders, in reverse order, and leaves the drive as it was
        let plan = uninstall_plan(
            &m,
            &UninstallOptions {
                vscode: VscodeMode::Skip,
                system: system(),
            },
        )
        .unwrap();
        let none = Payload {
            entries: Vec::new(),
        };
        execute(
            &plan,
            &Inputs {
                payload: &none,
                self_exe: &[],
            },
            &mut b,
            &mut |_| {},
        )
        .unwrap();
        assert!(b.state.files.is_empty());
        assert_eq!(b.state.dirs, machine().state.dirs);
    }

    #[test]
    fn it_039_a_failure_at_every_single_operation_of_a_fresh_install_rolls_back_to_the_initial_machine()
     {
        let initial = machine().state;
        let mut clean = machine();
        install(&mut clean, "0.2.0").unwrap();
        let total = clean.ops;
        assert_eq!(total, 10, "1 folder + 6 files + regsvr32 + registry + code");
        let mut failed = 0;
        for n in 1..=total {
            let mut b = machine();
            b.fail_at = Some(n);
            match install(&mut b, "0.2.0") {
                Err(e) => {
                    failed += 1;
                    assert!(e.cause.contains("injected failure"), "op {n}: {e}");
                    assert!(e.rollback_problems.is_empty(), "op {n}: {e}");
                    assert_eq!(
                        b.state, initial,
                        "op {n} failed ({e}) but the machine is not back to its initial state"
                    );
                }
                Ok(out) => {
                    // only the non-fatal VS Code step can fail without undoing the install; it is the very last operation
                    assert_eq!(n, total, "op {n} must not be survivable");
                    assert!(out.warnings.iter().any(|w| w.contains("could not be run")));
                    assert!(b.state.registered.contains(&k(DLL)));
                }
            }
        }
        assert_eq!(failed, total - 1);
    }

    fn old_install() -> FakeBackend {
        let mut b = machine();
        install_into(&mut b, PREFIX, "0.1.0", &["legacy.dll"], |_| {}).unwrap();
        b
    }

    #[test]
    fn it_039_a_failure_at_every_operation_of_an_upgrade_restores_the_old_install_exactly() {
        let base = old_install();
        let before = base.state.clone();
        assert!(
            before
                .files
                .contains_key(&k(&format!(r"{PREFIX}\legacy.dll")))
        );
        let mut clean = base.clone();
        clean.ops = 0;
        install(&mut clean, "0.2.0").unwrap();
        let total = clean.ops;
        for n in 1..=total {
            let mut b = base.clone();
            (b.ops, b.fail_at) = (0, Some(n));
            match install(&mut b, "0.2.0") {
                Err(e) => {
                    assert_eq!(
                        b.state, before,
                        "op {n} failed ({e}): the old install must be exactly back"
                    );
                    // the old registration is restored by registering the OLD dll again, never by unregistering
                    assert!(b.state.registered.contains(&k(DLL)), "op {n}");
                }
                Ok(out) => {
                    // a failing clean-up of the old files (scheduled for restart) or of the optional VS Code step
                    assert!(out.restart_needed || !out.warnings.is_empty(), "op {n}");
                    assert_eq!(b.file(DLL).unwrap(), b"pime_tsf.dll@0.2.0");
                    assert!(b.state.registered.contains(&k(DLL)));
                }
            }
        }
    }

    #[test]
    fn st_035_reinstalling_the_same_version_writes_nothing_and_is_idempotent() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        let first = b.state.clone();
        b.write_log.clear();
        b.commands.clear();
        let out = install(&mut b, "0.2.0").unwrap();
        assert_eq!(b.state, first);
        assert!(
            b.write_log.is_empty(),
            "unchanged files are not rewritten: {:?}",
            b.write_log
        );
        assert!(!out.restart_needed && out.warnings.is_empty());
        assert_eq!(
            b.commands.len(),
            2,
            "registration and the extension are simply refreshed"
        );
    }

    #[test]
    fn st_035_upgrade_replaces_files_removes_stale_ones_and_re_registers() {
        let mut b = old_install();
        let out = install(&mut b, "0.2.0").unwrap();
        assert!(!out.restart_needed && out.warnings.is_empty());
        assert_eq!(b.file(DLL).unwrap(), b"pime_tsf.dll@0.2.0");
        assert!(
            b.file(&format!(r"{PREFIX}\legacy.dll")).is_none(),
            "a file the new release does not ship is removed"
        );
        assert!(
            !b.state.files.keys().any(|f| f.contains(".pime-")),
            "no backups or temp files are left"
        );
        assert!(b.state.pending_delete.is_empty());
        let arp = &b.state.registry[&ARP_KEY.to_lowercase()];
        assert!(arp.contains(&("DisplayVersion".to_string(), RegValue::Str("0.2.0".into()))));
        // the uninstaller itself is the new one
        assert_eq!(
            b.file(&format!(r"{PREFIX}\uninstall.exe")).unwrap(),
            b"setup-0.2.0"
        );
    }

    #[test]
    fn st_035_upgrading_a_dll_that_programs_have_loaded_renames_it_aside_and_finishes_at_restart() {
        let mut b = old_install();
        b.lock(DLL); // mapped into running programs: cannot be overwritten or deleted
        let out = install(&mut b, "0.2.0").unwrap();
        assert!(
            out.restart_needed,
            "the user is told to restart or sign out"
        );
        assert_eq!(
            b.file(DLL).unwrap(),
            b"pime_tsf.dll@0.2.0",
            "the new DLL is in place at the registered path"
        );
        // the old DLL (still mapped) waits under a name of its own, and only that name is scheduled
        let scheduled = &b.state.pending_delete;
        assert_eq!(scheduled.len(), 1, "{scheduled:?}");
        assert!(is_aside_name(&scheduled[0]) && scheduled[0].starts_with(&k(DLL)));
        assert_eq!(
            b.state.files.get(&scheduled[0]).unwrap(),
            b"pime_tsf.dll@0.1.0"
        );
        assert!(
            b.file(&format!("{DLL}.pime-old")).is_none(),
            "the backup name is free again"
        );
        assert!(b.state.registered.contains(&k(DLL)));
        // ... and when Windows refuses to schedule it (not elevated) that is a warning, not a failure
        let mut c = old_install();
        c.lock(DLL);
        c.deny_schedule = true;
        let out = install(&mut c, "0.2.0").unwrap();
        assert!(
            !out.restart_needed
                && out
                    .warnings
                    .iter()
                    .any(|w| w.contains("could not be scheduled"))
        );
    }

    #[test]
    fn st_035_a_failing_regsvr32_rolls_everything_back_and_reports_the_reason() {
        // this is what happens today: the DLL's DllRegisterServer is not implemented yet
        let initial = machine().state;
        let mut b = machine();
        b.register_exit = 5;
        let e = install(&mut b, "0.2.0").unwrap_err();
        assert_eq!(e.step, 8);
        assert!(
            e.cause.contains("code 5") && e.cause.contains("DllRegisterServer"),
            "{e}"
        );
        assert_eq!(b.state, initial);
        // the rollback unregisters while the DLL still exists, and only then deletes it
        let at = |needle: &str| {
            b.events
                .iter()
                .position(|e| e.contains(needle))
                .unwrap_or_else(|| panic!("{needle} not in {:?}", b.events))
        };
        assert!(
            at("regsvr32.exe /u /s")
                < at("remove c:\\program files\\programming ime\\pime_tsf.dll"),
            "{:?}",
            b.events
        );

        let mut c = machine();
        c.register_exit = 3;
        assert!(
            install(&mut c, "0.2.0")
                .unwrap_err()
                .cause
                .contains("could not be loaded")
        );
    }

    #[test]
    fn it_039_a_failing_regsvr32_during_an_upgrade_leaves_the_old_files_and_registration() {
        let mut b = old_install();
        let before = b.state.clone();
        b.register_exit = 5;
        let e = install(&mut b, "0.2.0").unwrap_err();
        assert_eq!(b.state.files, before.files);
        assert_eq!(b.state.registry, before.registry);
        assert!(b.state.registered.contains(&k(DLL)));
        assert!(
            !e.rollback_problems.is_empty(),
            "re-registering the old DLL also failed with the same fake exit code, and that is reported"
        );
    }

    #[test]
    fn st_035_vscode_problems_never_fail_or_roll_back_the_install() {
        let mut b = machine();
        b.vscode = None;
        let out = install(&mut b, "0.2.0").unwrap();
        assert_eq!(out.vscode, VscodeStatus::NotFound);
        assert!(
            out.warnings[0].contains("was not found")
                && out.warnings[0].contains("--install-extension")
        );
        assert!(b.state.registered.contains(&k(DLL)) && b.commands.len() == 1);

        let mut c = machine();
        c.code_exit = 1;
        let out = install(&mut c, "0.2.0").unwrap();
        assert!(matches!(out.vscode, VscodeStatus::Failed(_)));
        assert!(c.state.files.contains_key(&k(DLL)));

        let mut d = machine();
        install_into(&mut d, PREFIX, "0.2.0", &[], |o| {
            o.vscode = VscodeMode::Skip
        })
        .unwrap();
        assert!(
            d.commands
                .iter()
                .all(|c| !c.contains("--install-extension"))
        );
    }

    #[test]
    fn ut_061_an_install_elsewhere_that_owns_the_add_remove_entry_is_refused_not_orphaned() {
        let mut b = machine();
        b.state.registry.insert(
            ARP_KEY.to_lowercase(),
            vec![(
                "InstallLocation".into(),
                RegValue::Str(r"E:\Somewhere\Else".into()),
            )],
        );
        let before = b.state.clone();
        let e = install(&mut b, "0.2.0").unwrap_err();
        assert_eq!(e.step, 0);
        assert!(e.cause.contains(r"E:\Somewhere\Else"));
        assert_eq!(b.state, before);
        // the same location (an upgrade) is fine, and a different spelling of it too
        b.state.registry.insert(
            ARP_KEY.to_lowercase(),
            vec![(
                "InstallLocation".into(),
                RegValue::Str(r"c:/program files/programming ime".into()),
            )],
        );
        assert!(install(&mut b, "0.2.0").is_ok());
    }

    #[test]
    fn st_035_a_no_register_no_arp_no_vscode_install_runs_no_process_and_writes_no_registry() {
        let mut b = machine();
        install_into(&mut b, PREFIX, "0.2.0", &[], |o| {
            (o.register, o.arp, o.vscode) = (false, false, VscodeMode::Skip)
        })
        .unwrap();
        assert!(
            b.commands.is_empty() && b.state.registry.is_empty() && b.state.registered.is_empty()
        );
        assert_eq!(b.state.files.len(), 6);
    }

    #[test]
    fn st_035_only_files_inside_the_prefix_are_ever_deleted_as_stale() {
        // an older manifest that (by tampering) lists a file elsewhere: the upgrade must not delete it
        let mut b = old_install();
        b.put(r"C:\Windows\System32\precious.dll", b"keep");
        let manifest_path = format!(r"{PREFIX}\install-manifest.json");
        let text = String::from_utf8(b.file(&manifest_path).unwrap().clone()).unwrap();
        let tampered = text.replace(
            "\"files\": [",
            "\"files\": [\n    \"C:\\\\Windows\\\\System32\\\\precious.dll\",",
        );
        b.put(&manifest_path, tampered.as_bytes());
        install(&mut b, "0.2.0").unwrap();
        assert_eq!(
            b.file(r"C:\Windows\System32\precious.dll").unwrap(),
            b"keep"
        );
    }

    #[test]
    fn st_035_uninstall_removes_exactly_the_manifest_and_unregisters_before_deleting_the_dll() {
        let mut b = machine();
        let initial = b.state.clone();
        install(&mut b, "0.2.0").unwrap();
        b.events.clear();
        let out = uninstall(&mut b, VscodeMode::Install);
        assert!(out.warnings.is_empty() && !out.restart_needed);
        assert_eq!(
            b.state, initial,
            "everything the install wrote is gone, including the folder it created"
        );
        let at = |needle: &str| {
            b.events
                .iter()
                .position(|e| e.contains(needle))
                .unwrap_or_else(|| panic!("{needle} not in {:?}", b.events))
        };
        assert!(
            at("regsvr32.exe /u /s")
                < at("remove c:\\program files\\programming ime\\pime_tsf.dll")
        );
        assert!(at("--uninstall-extension") < at("regsvr32.exe /u /s"));
        assert!(
            at("pime_tsf.dll") < at("uninstall.exe")
                && at("remove c:\\program files\\programming ime\\uninstall.exe")
                    < at("remove c:\\program files\\programming ime\\install-manifest.json")
        );
    }

    #[test]
    fn ut_061_uninstall_leaves_foreign_files_other_products_and_unrelated_registry_keys_alone() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.put(&format!(r"{PREFIX}\my-notes.txt"), b"user data");
        b.put(r"C:\Program Files\Other\other.dll", b"x");
        b.state.dirs.insert(k(r"C:\Program Files\Other"));
        let other_key = r"software\microsoft\windows\currentversion\uninstall\otherime";
        b.state.registry.insert(
            other_key.to_string(),
            vec![("DisplayName".into(), RegValue::Str("Other IME".into()))],
        );
        b.state
            .registered
            .insert(k(r"C:\Program Files\Other\other.dll"));
        let out = uninstall(&mut b, VscodeMode::Skip);
        assert!(
            b.file(&format!(r"{PREFIX}\my-notes.txt")).is_some(),
            "user data in the folder survives"
        );
        assert!(
            b.state.dirs.contains(&k(PREFIX)),
            "the folder is kept because it is not empty"
        );
        assert!(out.warnings.iter().any(|w| w.contains("was kept")));
        assert!(
            b.state.registry.contains_key(other_key)
                && !b.state.registry.contains_key(&ARP_KEY.to_lowercase())
        );
        assert_eq!(
            b.state.registered.len(),
            1,
            "only our own registration was removed"
        );
        assert!(b.file(r"C:\Program Files\Other\other.dll").is_some());
        assert!(b.commands.iter().all(|c| !c.contains("other.dll")));
        assert!(
            b.commands
                .iter()
                .all(|c| !c.contains("--uninstall-extension")),
            "--no-vscode"
        );
    }

    #[test]
    fn st_035_uninstall_with_a_locked_dll_and_a_running_uninstaller_finishes_at_restart() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.lock(DLL); // loaded by programs that used the input method
        b.lock(&format!(r"{PREFIX}\uninstall.exe")); // the running uninstaller cannot delete itself
        let out = uninstall(&mut b, VscodeMode::Install);
        assert!(
            out.restart_needed,
            "the user is told to sign out or restart"
        );
        assert!(out.warnings.is_empty());
        let pending = &b.state.pending_delete;
        assert_eq!(pending.len(), 3, "{pending:?}");
        // files first, the (then empty) folder last: Windows processes the list in this order
        assert_eq!(pending.last().unwrap(), &k(PREFIX));
        assert!(
            !b.state.registered.contains(&k(DLL)),
            "unregistered although the file stays until restart"
        );
        assert!(!b.state.registry.contains_key(&ARP_KEY.to_lowercase()));
        assert!(
            b.file(&format!(r"{PREFIX}\install-manifest.json"))
                .is_none()
        );
    }

    fn uninstaller() -> String {
        format!(r"{PREFIX}\uninstall.exe")
    }

    #[test]
    fn st_035_review_1_in_use_files_are_renamed_aside_and_only_that_name_is_scheduled() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.lock(DLL);
        b.lock(&uninstaller());
        let out = uninstall(&mut b, VscodeMode::Install);
        assert!(out.restart_needed && out.warnings.is_empty());
        let aside: Vec<&String> = b.state.files.keys().filter(|f| is_aside_name(f)).collect();
        assert_eq!(aside.len(), 2, "{aside:?}");
        let pending = &b.state.pending_delete;
        assert_eq!(
            pending.len(),
            3,
            "two renamed files and the folder: {pending:?}"
        );
        assert!(aside.iter().all(|a| pending.contains(a)));
        assert!(
            !pending.contains(&k(DLL)) && !pending.contains(&k(&uninstaller())),
            "the original paths are never scheduled: {pending:?}"
        );
        assert!(
            b.file(DLL).is_none() && b.file(&uninstaller()).is_none(),
            "the original names are free for a new install"
        );
    }

    #[test]
    fn st_035_review_1_a_reinstall_before_the_restart_is_not_undone_by_the_pending_deletions() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.lock(DLL);
        b.lock(&uninstaller());
        uninstall(&mut b, VscodeMode::Install);
        // the person runs the setup again before restarting: the entries that are queued are harmless to it
        install(&mut b, "0.3.0").unwrap();
        b.reboot();
        assert_eq!(b.file(DLL).unwrap(), b"pime_tsf.dll@0.3.0");
        assert_eq!(b.file(&uninstaller()).unwrap(), b"setup-0.3.0");
        assert!(
            b.file(&format!(r"{PREFIX}\install-manifest.json"))
                .is_some()
        );
        assert_eq!(b.state.files.len(), 6, "{:?}", file_names(&b));
        assert!(
            b.state.dirs.contains(&k(PREFIX)),
            "the queued removal of the folder fails while it is not empty"
        );
        assert!(b.state.registered.contains(&k(DLL)));
    }

    #[test]
    fn st_035_review_1_a_file_that_cannot_be_renamed_is_reported_and_never_scheduled_under_its_own_name()
     {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.lock_hard(&uninstaller()); // open without delete sharing: not even a rename works
        let out = uninstall(&mut b, VscodeMode::Install);
        assert!(!out.restart_needed);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("uninstall.exe") && w.contains("NOT scheduled")),
            "{:?}",
            out.warnings
        );
        assert!(
            b.state.pending_delete.is_empty(),
            "{:?}",
            b.state.pending_delete
        );
        assert!(b.file(&uninstaller()).is_some());
    }

    #[test]
    fn st_035_review_1_a_stale_file_that_is_in_use_is_renamed_aside_before_it_is_scheduled() {
        let mut b = old_install(); // 0.1.0 ships legacy.dll, 0.2.0 does not
        let legacy = format!(r"{PREFIX}\legacy.dll");
        b.lock(&legacy);
        let out = install(&mut b, "0.2.0").unwrap();
        assert!(out.restart_needed);
        assert!(!b.state.pending_delete.contains(&k(&legacy)));
        // a later release ships legacy.dll again: the restart must not delete that one
        install_into(&mut b, PREFIX, "0.3.0", &["legacy.dll"], |_| {}).unwrap();
        b.reboot();
        assert_eq!(b.file(&legacy).unwrap(), b"legacy.dll@0.3.0");
    }

    #[test]
    fn st_035_review_1_an_install_is_refused_while_deletions_inside_the_folder_are_queued() {
        for entry in [
            format!(r"{PREFIX}\uninstall.exe"), // a live path, as an older setup scheduled it
            format!(r"{PREFIX}\pime_tsf.dll.pime-old"),
            format!(r"{PREFIX}\sub\x.dll"),
            format!(r"{PREFIX}\x.pime-del-1-0\y.dll"), // not exactly one of our aside names
            format!(r"{PREFIX}\x.pime-del-1"),
        ] {
            let mut b = machine();
            b.foreign_pending.push(k(&entry));
            let before = b.state.clone();
            let e = install(&mut b, "0.2.0").unwrap_err();
            assert_eq!(e.step, 0, "{entry}");
            assert!(e.cause.contains("restart the computer"), "{entry}: {e}");
            assert!(e.cause.contains(&k(&entry)), "{entry}: {e}");
            assert_eq!(b.state, before, "{entry}: nothing was written");
        }
        // harmless: the names this installer made up, the folder and its parents, everything elsewhere
        let mut b = machine();
        b.foreign_pending = [
            format!(r"{PREFIX}\uninstall.exe.pime-del-12-3"),
            PREFIX.to_string(),
            r"C:\Program Files".to_string(),
            r"C:\Program Files\Other\x.dll".to_string(),
            format!(r"{PREFIX}2\x.dll"),
        ]
        .iter()
        .map(|p| k(p))
        .collect();
        assert!(install(&mut b, "0.2.0").is_ok());
        assert_eq!(b.pending_reads.get(), 1);
    }

    #[test]
    fn st_035_review_1_an_install_that_needs_no_administrator_rights_never_reads_the_pending_list()
    {
        let flags = |o: &mut crate::plan::InstallOptions| {
            (o.register, o.arp, o.vscode) = (false, false, VscodeMode::Skip)
        };
        let temp = r"C:\Users\me\AppData\Local\Temp\pime-test";
        let mut b = machine();
        b.foreign_pending.push(k(&format!(r"{temp}\uninstall.exe")));
        install_into(&mut b, temp, "0.2.0", &[], flags).unwrap();
        assert_eq!(
            b.pending_reads.get(),
            0,
            "no HKLM for a temp-folder install"
        );
        // the same plan in a protected folder needs administrator rights and does look
        let mut c = machine();
        install_into(&mut c, PREFIX, "0.2.0", &[], flags).unwrap();
        assert_eq!(c.pending_reads.get(), 1);
    }

    #[test]
    fn st_035_review_1_aside_names_are_recognised_exactly() {
        for yes in [
            r"c:\x\uninstall.exe.pime-del-12-3",
            r"C:\X\a.dll.pime-old.PIME-DEL-0-0",
        ] {
            assert!(is_aside_name(yes), "{yes}");
        }
        for no in [
            r"c:\x\uninstall.exe",
            r"c:\x\a.pime-del-",
            r"c:\x\a.pime-del-1",
            r"c:\x\a.pime-del-1-",
            r"c:\x\a.pime-del-a-1",
            r"c:\x\a.pime-del-1-2-3",
            r"c:\x\a.pime-del-1-2\b.dll",
            r"c:\x\a.pime-old",
        ] {
            assert!(!is_aside_name(no), "{no}");
        }
    }

    fn manifest_of(b: &FakeBackend, prefix: &str) -> Manifest {
        let text = String::from_utf8(b.read_file(&Path::new(prefix).join(MANIFEST_NAME)).unwrap())
            .unwrap();
        Manifest::parse(&text, Path::new(prefix)).unwrap()
    }

    fn uninstall_manifest(b: &mut FakeBackend, m: &Manifest) -> Outcome {
        let plan = uninstall_plan(
            m,
            &UninstallOptions {
                vscode: VscodeMode::Skip,
                system: system(),
            },
        )
        .unwrap();
        let none = Payload {
            entries: Vec::new(),
        };
        execute(
            &plan,
            &Inputs {
                payload: &none,
                self_exe: &[],
            },
            b,
            &mut |_| {},
        )
        .unwrap()
    }

    fn warned(out: &Outcome, needles: &[&str]) -> bool {
        out.warnings
            .iter()
            .any(|w| needles.iter().all(|n| w.contains(n)))
    }

    #[test]
    fn st_035_review_3_a_link_inside_the_install_folder_is_never_followed_by_the_uninstall() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        // `sub` looks like a folder of the install folder but is a junction: what is "below" it belongs elsewhere
        let sub = format!(r"{PREFIX}\sub");
        b.state.dirs.insert(k(&sub));
        b.link(&sub);
        b.put(&format!(r"{sub}\other.txt"), b"a file that is not ours");
        b.put(&format!(r"{sub}\pime_tsf.dll"), b"a different dll");
        let mut m = manifest_of(&b, PREFIX);
        m.files.push(PathBuf::from(format!(r"{sub}\other.txt")));
        m.text_service_dlls
            .push(PathBuf::from(format!(r"{sub}\pime_tsf.dll")));
        b.commands.clear();
        let out = uninstall_manifest(&mut b, &m);
        assert!(b.file(&format!(r"{sub}\other.txt")).is_some());
        assert!(b.file(&format!(r"{sub}\pime_tsf.dll")).is_some());
        assert!(b.state.dirs.contains(&k(&sub)));
        assert!(
            b.commands.iter().all(|c| !c.contains("sub")),
            "regsvr32 must not be run on a DLL behind a link: {:?}",
            b.commands
        );
        assert!(warned(
            &out,
            &["other.txt", "left in place", "reparse point"]
        ));
        assert!(warned(
            &out,
            &[r"sub\pime_tsf.dll", "NOT unregistered", "reparse point"]
        ));
        // the real files of the install are still removed, and the real DLL was unregistered
        assert!(b.file(DLL).is_none());
        assert!(!b.state.registered.contains(&k(DLL)));
        assert!(b.commands.iter().any(|c| c.contains("/u /s")));
    }

    #[test]
    fn st_035_review_3_a_link_in_place_of_a_listed_file_or_of_the_install_folder_is_left_alone() {
        // the DLL itself is a link
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.link(DLL);
        b.commands.clear();
        let out = uninstall(&mut b, VscodeMode::Skip);
        assert!(b.file(DLL).is_some(), "not deleted");
        assert!(
            b.commands.is_empty(),
            "no regsvr32 /u on a link: {:?}",
            b.commands
        );
        assert!(warned(&out, &["pime_tsf.dll", "NOT unregistered"]));
        assert!(b.file(&format!(r"{PREFIX}\pime-core.exe")).is_none());

        // the install folder itself is a link: nothing in it is touched, but the registry entry still goes
        let mut c = machine();
        install(&mut c, "0.2.0").unwrap();
        c.link(PREFIX);
        c.commands.clear();
        let before = c.state.files.clone();
        let out = uninstall(&mut c, VscodeMode::Skip);
        assert_eq!(
            c.state.files, before,
            "nothing below a linked install folder is deleted"
        );
        assert!(c.commands.is_empty());
        assert!(c.state.dirs.contains(&k(PREFIX)));
        assert!(warned(&out, &["reparse point"]));
        assert!(!c.state.registry.contains_key(&ARP_KEY.to_lowercase()));
    }

    #[test]
    fn st_035_review_3_a_link_in_a_parent_folder_the_installer_created_stops_everything_below_it() {
        let prefix = r"D:\Tools\Deep\PIME";
        let mut b = machine();
        install_into(&mut b, prefix, "0.2.0", &[], |o| {
            o.vscode = VscodeMode::Skip
        })
        .unwrap();
        let m = manifest_of(&b, prefix);
        assert_eq!(m.directories.len(), 3);
        // a link at the middle parent: the top parent is fine, the middle one and everything below it is not
        b.link(r"D:\Tools\Deep");
        b.commands.clear();
        let files = b.state.files.clone();
        let out = uninstall_manifest(&mut b, &m);
        assert_eq!(b.state.files, files, "nothing below the link is deleted");
        assert!(b.commands.is_empty(), "{:?}", b.commands);
        assert!(warned(&out, &[r"folder D:\Tools\Deep", "reparse point"]));
        assert!(b.state.dirs.contains(&k(r"D:\Tools\Deep")) && b.state.dirs.contains(&k(prefix)));
        // the top parent is no link, but it still holds the folders below it: kept
        assert!(b.state.dirs.contains(&k(r"D:\Tools")));
        assert!(warned(&out, &[r"D:\Tools", "was kept"]));
    }

    #[test]
    fn st_035_review_3_an_install_refuses_a_link_as_install_folder_or_as_a_file_to_replace() {
        // the install folder already is a junction
        let mut b = machine();
        b.state.dirs.insert(k(PREFIX));
        b.link(PREFIX);
        let before = b.state.clone();
        let e = install(&mut b, "0.2.0").unwrap_err();
        assert_eq!(e.step, 1, "{e}");
        assert!(e.cause.contains("reparse point"), "{e}");
        assert_eq!(b.state, before, "nothing was written through the link");

        // a file that is to be replaced is a link (a symbolic link would be written through)
        let mut c = old_install();
        c.link(DLL);
        let before = c.state.clone();
        let e = install(&mut c, "0.2.0").unwrap_err();
        assert!(e.cause.contains("reparse point"), "{e}");
        assert_eq!(c.state, before, "rolled back, the linked file untouched");
        assert!(e.rollback_problems.is_empty(), "{e}");

        // a stale file of an older release that is a link is not deleted by the clean-up
        let mut d = old_install();
        d.link(&format!(r"{PREFIX}\legacy.dll"));
        let out = install(&mut d, "0.2.0").unwrap();
        assert!(d.file(&format!(r"{PREFIX}\legacy.dll")).is_some());
        assert!(warned(
            &out,
            &["legacy.dll", "left in place", "reparse point"]
        ));
    }

    #[test]
    fn st_035_review_3_every_delete_rename_and_regsvr32_is_preceded_by_a_link_check() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.checks.borrow_mut().clear();
        uninstall(&mut b, VscodeMode::Skip);
        let checks = b.checks.borrow();
        let asked = |p: &str| checks.iter().any(|(_, path)| *path == k(p));
        for p in [
            DLL,
            &format!(r"{PREFIX}\uninstall.exe"),
            &format!(r"{PREFIX}\install-manifest.json"),
            PREFIX,
        ] {
            assert!(asked(p), "no link check for {p}: {checks:?}");
        }
        assert!(checks.iter().all(|(root, _)| *root == k(PREFIX)));
    }

    #[test]
    fn st_035_review_12_a_dll_that_is_already_gone_is_reported_with_the_manual_cleanup() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.state.files.remove(&k(DLL));
        b.commands.clear();
        let out = uninstall(&mut b, VscodeMode::Skip);
        assert!(b.commands.is_empty(), "regsvr32 cannot run without the DLL");
        let w = out
            .warnings
            .iter()
            .find(|w| w.contains("already gone"))
            .expect("a warning");
        for needle in [
            "NOT unregistered",
            "run this setup again",
            "reg delete \"HKLM\\Software\\Classes\\CLSID\\{43F95ED1-95B4-421D-8F5A-837128C2D854}\" /f /reg:64",
            "CTF\\TIP\\{43F95ED1-95B4-421D-8F5A-837128C2D854}",
            "docs/INSTALLER.md",
        ] {
            assert!(w.contains(needle), "{needle} missing from: {w}");
        }
        assert!(
            b.state.files.is_empty(),
            "the rest of the uninstall still happens"
        );
    }

    #[test]
    fn st_035_review_12_the_clsid_in_the_manual_cleanup_is_the_one_the_text_service_registers() {
        let lib = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../pime-tsf/src/lib.rs"
        ))
        .expect("crates/pime-tsf/src/lib.rs");
        let d = TEXT_SERVICE_CLSID
            .trim_matches(['{', '}'])
            .to_lowercase()
            .replace('-', "");
        let grouped = format!(
            "0x{}_{}_{}_{}_{}",
            &d[..8],
            &d[8..12],
            &d[12..16],
            &d[16..20],
            &d[20..]
        );
        assert!(
            lib.contains(&format!("CLSID_PIME: GUID = GUID::from_u128({grouped})")),
            "CLSID_PIME in pime-tsf differs from TEXT_SERVICE_CLSID {TEXT_SERVICE_CLSID}"
        );
    }

    #[test]
    fn st_035_a_failing_unregistration_does_not_block_the_uninstall_but_is_reported() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        b.unregister_exit = 5; // DllUnregisterServer is E_NOTIMPL in the current DLL
        let out = uninstall(&mut b, VscodeMode::Skip);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("regsvr32 /u exited with code 5"))
        );
        assert!(b.state.files.is_empty() && b.state.registry.is_empty());
        // a DLL that is already gone cannot be unregistered: also a warning, and the rest still happens
        let mut c = machine();
        install(&mut c, "0.2.0").unwrap();
        c.state.files.remove(&k(DLL));
        let out = uninstall(&mut c, VscodeMode::Skip);
        assert!(out.warnings.iter().any(|w| w.contains("already gone")));
        assert!(c.state.files.is_empty());
    }

    #[test]
    fn st_035_uninstalling_twice_is_harmless() {
        let mut b = machine();
        install(&mut b, "0.2.0").unwrap();
        let manifest = b
            .file(&format!(r"{PREFIX}\install-manifest.json"))
            .unwrap()
            .clone();
        uninstall(&mut b, VscodeMode::Skip);
        // a second run from a surviving copy of the manifest finds nothing and changes nothing
        b.put(&format!(r"{PREFIX}\install-manifest.json"), &manifest);
        b.state.dirs.insert(k(PREFIX));
        uninstall(&mut b, VscodeMode::Skip);
        assert!(b.state.files.is_empty());
        assert_eq!(b.state.dirs, machine().state.dirs);
    }

    #[test]
    fn st_035_payload_pack_and_install_agree_on_the_file_set() {
        // the names the installer places are exactly the names the payload format allows
        let plan = install_plan(Path::new(PREFIX), &options()).unwrap();
        for step in &plan.steps {
            if let Step::PlaceFile {
                source: FileSource::Payload(n),
                ..
            } = step
            {
                assert!(payload::valid_entry_name(n), "{n}");
            }
        }
    }
}
