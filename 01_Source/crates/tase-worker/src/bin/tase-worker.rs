//! The resident binary of the TASE worker.
//!
//! The binary speaks to the Hub of the local agents over the pipe of the logon session: it takes a
//! job out of the queue of the Hub, it runs it, and it tells the Hub what happened.
//!
//! ```text
//! tase-worker --help
//! tase-worker --logon-sid S-1-5-5-1000-1000
//! tase-worker --pipe-name \\.\pipe\PIME-IPC-1-S-1-5-5-1000-1000
//! ```
//!
//! # What the worker does with a job
//!
//! The job runs as a **preview**: the worker computes the diff of the executor and gives it back
//! to the Hub, and the Hub gives it to the client that asked for the job. The worker never writes
//! into a window of the user: the apply is the business of the client that holds the host
//! (`tase_apply` of the client side of the repository).
//!
//! # The pipe of the Hub
//!
//! The Hub of a logon session listens on the pipe of that session, and the name of the pipe is
//! computed out of the logon SID of the session (`tase-local-ipc` names it
//! `\\.\pipe\PIME-IPC-1-<logon sid>`). The worker therefore needs the logon SID of the session of
//! the Hub:
//!
//! * `--logon-sid <sid>` gives it;
//! * `--pipe-name <name>` gives the name of the pipe, and the logon SID is read out of it;
//! * the variable `TASE_LOGON_SID` is the default of both.
//!
//! The Hub prints the name of its pipe when it starts (`tase-local-hub --print-pipe-name`), so the
//! operator may copy it into the command line of the worker.
//!
//! # When there is no Hub
//!
//! The worker says so and stops: it does not pretend that it works, and it does not fall back to a
//! queue of its own.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tase_local_ipc::acl::PIPE_PREFIX;
use tase_local_ipc::{Timeouts, WindowsPipe};
use tase_worker::hub_link::{HubLink, LinkOutcome, PreviewRunner, WORKER_CAPABILITIES};
use tase_worker::lease::{Clock, SystemClock};
use tase_worker::main::StopSignal;

/// The name of the variable that holds the logon SID of the session of the Hub.
const LOGON_SID_VARIABLE: &str = "TASE_LOGON_SID";

/// What the binary was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// Print the usage and stop.
    Help,
    /// Print the version and stop.
    Version,
    /// Print the name of the pipe of the session and stop.
    PrintPipeName,
    /// Run the worker.
    Run,
}

/// What the command line says.
#[derive(Clone, Debug)]
struct Options {
    action: Action,
    worker_id: String,
    poll_interval_ms: u64,
    connect_timeout_ms: u64,
    logon_sid: Option<String>,
    pipe_name: Option<String>,
    stop_file: Option<PathBuf>,
    once: bool,
}

/// The usage of the binary.
const USAGE: &str = "\
TASE worker - the resident worker of TASE

Usage:
  tase-worker [options]

Options:
  -h, --help               print this usage and stop
  -V, --version            print the version and stop
      --logon-sid <sid>    the logon SID of the session of the Hub
                           (default: the TASE_LOGON_SID variable)
      --pipe-name <name>   the name of the pipe of the Hub; the logon SID is read out of it
      --print-pipe-name    print the name of the pipe of the session and stop
      --worker-id <id>     the id of the worker (default: tase-worker-1)
      --poll-ms <n>        how long the worker waits when there is nothing to do (default: 500)
      --connect-timeout-ms <n>  how long the worker waits for the pipe of the Hub (default: 2000)
      --stop-file <path>   the file whose presence asks the worker to stop
      --once               run one job and stop

The worker takes the jobs of the Hub one at a time, it runs them as a preview (the diff of the
executor) and it tells the Hub what happened. It never writes into a window of the user: the
apply is the business of the client that asked for the job.

The Hub of a logon session listens on the pipe of that session, and the name of the pipe is
computed out of the logon SID of the session: \\\\.\\pipe\\PIME-IPC-1-<logon sid>. The worker needs
that logon SID: give it with --logon-sid, give the name of the pipe with --pipe-name, or set the
TASE_LOGON_SID variable. The Hub prints the name of its pipe when it starts.
When there is no Hub on the pipe, the worker says so and stops: it does not pretend that it works.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("tase-worker: {reason}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match options.action {
        Action::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("tase-worker {}", tase_worker::WORKER_VERSION);
            ExitCode::SUCCESS
        }
        Action::PrintPipeName => match logon_sid_of(&options).map(|sid| pipe_name_of(&sid)) {
            Ok(name) => {
                println!("{name}");
                ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("tase-worker: {reason}");
                ExitCode::from(2)
            }
        },
        Action::Run => {
            // The logon SID of the session of the Hub is a configuration: when it is missing, the
            // worker says so and stops with the code of a bad command line.
            let logon_sid = match logon_sid_of(&options) {
                Ok(sid) => sid,
                Err(reason) => {
                    eprintln!("tase-worker: {reason}");
                    return ExitCode::from(2);
                }
            };
            match run(&options, &logon_sid) {
                Ok(()) => ExitCode::SUCCESS,
                Err(reason) => {
                    eprintln!("tase-worker: {reason}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Reads the command line.
fn parse(args: &[String]) -> Result<Options, String> {
    let mut action = Action::Run;
    let mut worker_id = "tase-worker-1".to_string();
    let mut poll_interval_ms = 500;
    let mut connect_timeout_ms = 2_000;
    let mut logon_sid = None;
    let mut pipe_name = None;
    let mut stop_file = None;
    let mut once = false;

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "-h" | "--help" => action = Action::Help,
            "-V" | "--version" => action = Action::Version,
            "--print-pipe-name" => action = Action::PrintPipeName,
            "--once" => once = true,
            "--logon-sid" => {
                logon_sid = Some(value_of(args, index + 1, "--logon-sid")?);
                index += 1;
            }
            "--pipe-name" => {
                pipe_name = Some(value_of(args, index + 1, "--pipe-name")?);
                index += 1;
            }
            "--stop-file" => {
                stop_file = Some(PathBuf::from(value_of(args, index + 1, "--stop-file")?));
                index += 1;
            }
            "--worker-id" => {
                worker_id = value_of(args, index + 1, "--worker-id")?;
                index += 1;
            }
            "--poll-ms" => {
                poll_interval_ms = number_of(args, index + 1, "--poll-ms")?;
                index += 1;
            }
            "--connect-timeout-ms" => {
                connect_timeout_ms = number_of(args, index + 1, "--connect-timeout-ms")?;
                index += 1;
            }
            other => return Err(format!("unknown option {other}")),
        }
        index += 1;
    }

    Ok(Options {
        action,
        worker_id,
        poll_interval_ms,
        connect_timeout_ms,
        logon_sid,
        pipe_name,
        stop_file,
        once,
    })
}

/// The value of an option that wants one.
fn value_of(args: &[String], index: usize, name: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{name} wants a value"))
}

/// The value of an option that wants a number.
fn number_of(args: &[String], index: usize, name: &str) -> Result<u64, String> {
    let value = value_of(args, index, name)?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{name} wants a number"))
}

/// The logon SID of the session of the Hub.
///
/// The command line comes first, then the variable of the environment. The name of the pipe is
/// the other way round: it holds the logon SID at its end.
fn logon_sid_of(options: &Options) -> Result<String, String> {
    if let Some(sid) = &options.logon_sid {
        let sid = sid.trim();
        if !sid.is_empty() {
            return Ok(sid.to_string());
        }
    }
    if let Some(name) = &options.pipe_name {
        return logon_sid_of_pipe(name);
    }
    match std::env::var(LOGON_SID_VARIABLE) {
        Ok(sid) if !sid.trim().is_empty() => Ok(sid.trim().to_string()),
        _ => Err(format!(
            "the worker needs the logon SID of the session of the Hub: give --logon-sid <sid>, give \
             --pipe-name <name>, or set the {LOGON_SID_VARIABLE} variable"
        )),
    }
}

/// The logon SID out of the name of the pipe of the Hub.
fn logon_sid_of_pipe(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    let Some(sid) = trimmed.strip_prefix(PIPE_PREFIX) else {
        return Err(format!(
            "the pipe of the hub is named {PIPE_PREFIX}<logon sid>: {trimmed} is not such a name"
        ));
    };
    let sid = sid.trim();
    if sid.is_empty() {
        return Err("the name of the pipe holds no logon SID".to_string());
    }
    Ok(sid.to_string())
}

/// The name of the pipe of a logon session.
fn pipe_name_of(logon_sid: &str) -> String {
    tase_local_ipc::acl::pipe_name(logon_sid)
}

/// Runs the worker.
fn run(options: &Options, logon_sid: &str) -> Result<(), String> {
    let pipe_name = pipe_name_of(logon_sid);

    // The pipe of the Hub. There is no Hub on it: the worker says so and stops, it does not
    // pretend that it works.
    let transport = WindowsPipe::connect(
        logon_sid,
        Duration::from_millis(options.connect_timeout_ms),
    )
    .map_err(|error| {
        format!(
            "there is no Hub on the pipe {pipe_name}: {error}. Start the Hub of the session, or \
             give the right logon SID with --logon-sid."
        )
    })?;

    let mut link = HubLink::new(
        transport,
        &options.worker_id,
        &WORKER_CAPABILITIES,
        Timeouts::default(),
    );
    link.handshake().map_err(|error| {
        format!("the Hub of {pipe_name} did not greet the worker: {}", error.message)
    })?;
    println!(
        "the worker {} is connected to the Hub on {pipe_name}",
        options.worker_id
    );

    let clock = SystemClock;
    let stop = StopSignal::new();
    let signal = stop.clone();
    let stop_file = options.stop_file.clone();
    let poll_interval_ms = options.poll_interval_ms;
    let mut runner = PreviewRunner;
    let mut processed = 0u32;

    loop {
        if stop.is_stopped() {
            break;
        }
        if stop_file.as_ref().is_some_and(|path| path.exists()) {
            signal.stop();
            break;
        }
        let outcome = link
            .run_once(&mut runner, clock.now_ms())
            .map_err(|error| format!("the link to the Hub broke: {}", error.message))?;
        match &outcome {
            LinkOutcome::Idle => {
                // the queue is empty: the worker waits, it never waits inside the library
                std::thread::sleep(Duration::from_millis(poll_interval_ms));
            }
            LinkOutcome::Completed { job_id } => {
                processed = processed.saturating_add(1);
                println!("the job {job_id} is done: the preview is in the Hub");
            }
            LinkOutcome::Failed {
                job_id,
                code,
                message,
            } => {
                processed = processed.saturating_add(1);
                eprintln!("the job {job_id} failed ({}): {message}", code.as_str());
            }
            LinkOutcome::Rejected { job_id, reason } => {
                eprintln!("the Hub refused the report of the job {job_id}: {reason}");
            }
        }
        if options.once {
            break;
        }
    }

    let _ = link.close("the worker stops");
    println!("the worker stopped after {processed} jobs");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn the_help_is_the_default_of_the_help_option() {
        let options = parse(&args(&["--help"])).expect("the command line is read");
        assert_eq!(options.action, Action::Help);
        let options = parse(&args(&["-h"])).expect("the command line is read");
        assert_eq!(options.action, Action::Help);
    }

    #[test]
    fn the_version_is_asked_for() {
        let options = parse(&args(&["--version"])).expect("the command line is read");
        assert_eq!(options.action, Action::Version);
    }

    #[test]
    fn the_pipe_name_is_asked_for() {
        let options = parse(&args(&["--print-pipe-name"])).expect("the command line is read");
        assert_eq!(options.action, Action::PrintPipeName);
    }

    #[test]
    fn the_configuration_is_read() {
        let options = parse(&args(&[
            "--worker-id",
            "worker-7",
            "--poll-ms",
            "250",
            "--connect-timeout-ms",
            "1000",
            "--logon-sid",
            "S-1-5-5-1000-1000",
        ]))
        .expect("the command line is read");

        assert_eq!(options.worker_id, "worker-7");
        assert_eq!(options.poll_interval_ms, 250);
        assert_eq!(options.connect_timeout_ms, 1000);
        assert_eq!(options.logon_sid.as_deref(), Some("S-1-5-5-1000-1000"));
    }

    #[test]
    fn the_stop_file_is_read() {
        let options =
            parse(&args(&["--stop-file", "D:/tase/stop"])).expect("the command line is read");
        assert_eq!(options.stop_file, Some(PathBuf::from("D:/tase/stop")));
    }

    #[test]
    fn the_one_job_option_is_read() {
        let options = parse(&args(&["--once"])).expect("the command line is read");
        assert!(options.once);
    }

    #[test]
    fn an_option_without_its_value_is_refused() {
        let error = parse(&args(&["--logon-sid"])).expect_err("the command line is refused");
        assert!(error.contains("--logon-sid"), "{error}");
    }

    #[test]
    fn an_option_that_wants_a_number_is_refused_when_it_is_not_one() {
        let error =
            parse(&args(&["--poll-ms", "many"])).expect_err("the command line is refused");
        assert!(error.contains("--poll-ms"), "{error}");
    }

    #[test]
    fn an_unknown_option_is_refused() {
        let error = parse(&args(&["--nonsense"])).expect_err("the command line is refused");
        assert!(error.contains("--nonsense"), "{error}");
    }

    #[test]
    fn the_usage_names_the_options() {
        for option in [
            "--help",
            "--version",
            "--logon-sid",
            "--pipe-name",
            "--print-pipe-name",
            "--worker-id",
            "--poll-ms",
            "--connect-timeout-ms",
            "--stop-file",
            "--once",
        ] {
            assert!(USAGE.contains(option), "the usage must name {option}");
        }
    }

    #[test]
    fn the_usage_names_the_default_of_the_pipe() {
        assert!(USAGE.contains(PIPE_PREFIX), "the usage must name the pipe");
        assert!(USAGE.contains(LOGON_SID_VARIABLE), "the usage must name the variable");
    }

    #[test]
    fn the_version_of_the_binary_is_the_version_of_the_crate() {
        assert_eq!(tase_worker::WORKER_VERSION, "1.1.0");
    }

    /// The logon SID is read out of the name of the pipe of the Hub.
    #[test]
    fn the_logon_sid_is_read_out_of_the_name_of_the_pipe() {
        let name = pipe_name_of("S-1-5-5-1000-1000");
        assert_eq!(name, r"\\.\pipe\PIME-IPC-1-S-1-5-5-1000-1000");
        assert_eq!(
            logon_sid_of_pipe(&name).expect("the sid is read"),
            "S-1-5-5-1000-1000"
        );
    }

    /// A name that is not the name of the pipe of the Hub is refused.
    #[test]
    fn a_name_that_is_not_the_pipe_of_the_hub_is_refused() {
        let error = logon_sid_of_pipe(r"\\.\pipe\other").expect_err("the name is refused");
        assert!(error.contains("logon sid"), "{error}");
        let error = logon_sid_of_pipe(PIPE_PREFIX).expect_err("the name is refused");
        assert!(error.contains("logon SID"), "{error}");
    }

    /// The command line comes before the variable of the environment.
    #[test]
    fn the_logon_sid_of_the_command_line_wins() {
        let options = parse(&args(&["--logon-sid", "S-1-5-5-2000-2000"]))
            .expect("the command line is read");
        assert_eq!(
            logon_sid_of(&options).expect("the sid is found"),
            "S-1-5-5-2000-2000"
        );
    }
}
