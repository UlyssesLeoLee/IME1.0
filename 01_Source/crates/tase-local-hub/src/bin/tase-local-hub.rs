//! The resident binary of the TASE Local Agent Hub.
//!
//! One process per login session (detailed design §17.2). The binary does three things and nothing
//! else: it computes the namespace of the session, it starts the Hub (or connects to the Hub that is
//! already running), and it waits for the connections of the agents until it is asked to stop.
//!
//! ```text
//! tase-local-hub --help
//! tase-local-hub --print-pipe-name
//! tase-local-hub --data-dir D:\IDE1.0\tase
//! ```

#![cfg_attr(not(windows), forbid(unsafe_code))]

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tase_local_hub::{Hub, HubConfig, HubRole, SchedulerKind, platform_identity, start_or_connect};
use tase_local_store::Store;

/// Whether the user asked the Hub to stop.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// What the binary was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// Print the usage and stop.
    Help,
    /// Print the version and stop.
    Version,
    /// Print the name of the pipe of the namespace and stop.
    PrintPipeName,
    /// Start the Hub and wait for the connections.
    Run,
}

/// What the command line says.
#[derive(Clone, Debug)]
struct Options {
    action: Action,
    config: HubConfig,
}

/// The usage of the binary.
const USAGE: &str = "\
TASE Local Agent Hub - the resident local multi-agent service of TASE

Usage:
  tase-local-hub [options]

Options:
  -h, --help              print this usage and stop
  -V, --version           print the version and stop
      --print-pipe-name   print the name of the pipe of the logon session and stop
      --data-dir <dir>    the directory where the Hub keeps its data
      --max-connections <n>  the most connections at the same time (default 64)
      --max-jobs <n>      the most jobs that are not finished at the same time (default 16)
      --scheduler <kind>  the scheduler of the jobs: fair or round-robin (default fair)
      --agent-weight <n>  the share of an agent in the queue of its class (default 1)

The Hub serves one logon session: the name of its pipe is computed out of the logon SID of the
process, and a second Hub of the same session connects to the first one instead of writing into the
same files.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("tase-local-hub: {reason}");
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
            println!("tase-local-hub {}", tase_local_hub::VERSION);
            ExitCode::SUCCESS
        }
        Action::PrintPipeName => match pipe_name(&options.config) {
            Ok(name) => {
                println!("{name}");
                ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("tase-local-hub: {reason}");
                ExitCode::FAILURE
            }
        },
        Action::Run => match run(&options.config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(reason) => {
                eprintln!("tase-local-hub: {reason}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Reads the command line.
fn parse(args: &[String]) -> Result<Options, String> {
    let mut action = Action::Run;
    let mut config = HubConfig::default();
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "-h" | "--help" => action = Action::Help,
            "-V" | "--version" => action = Action::Version,
            "--print-pipe-name" => action = Action::PrintPipeName,
            "--data-dir" => {
                let value = value_of(args, index + 1, "--data-dir")?;
                config.data_dir = std::path::PathBuf::from(value);
                index += 1;
            }
            "--max-connections" => {
                let value = value_of(args, index + 1, "--max-connections")?;
                config.max_connections = value
                    .parse::<u32>()
                    .map_err(|_| "--max-connections wants a number".to_string())?;
                index += 1;
            }
            "--max-jobs" => {
                let value = value_of(args, index + 1, "--max-jobs")?;
                config.max_concurrent_jobs = value
                    .parse::<u32>()
                    .map_err(|_| "--max-jobs wants a number".to_string())?;
                index += 1;
            }
            "--scheduler" => {
                let value = value_of(args, index + 1, "--scheduler")?;
                config.scheduler = SchedulerKind::from_name(&value).ok_or_else(|| {
                    format!("--scheduler wants fair or round-robin, not {value}")
                })?;
                index += 1;
            }
            "--agent-weight" => {
                let value = value_of(args, index + 1, "--agent-weight")?;
                config.agent_weight = value
                    .parse::<u32>()
                    .map_err(|_| "--agent-weight wants a number".to_string())?;
                index += 1;
            }
            other => return Err(format!("unknown option {other}")),
        }
        index += 1;
    }
    Ok(Options { action, config })
}

/// The value of an option that wants one.
fn value_of(args: &[String], index: usize, name: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{name} wants a value"))
}

/// The name of the pipe of the namespace of the process.
fn pipe_name(config: &HubConfig) -> Result<String, String> {
    let identity = platform_identity();
    let logon_sid = identity.logon_sid();
    if logon_sid.trim().is_empty() {
        return Err(
            "the logon session of the process cannot be read; the hub refuses to open a pipe to everybody"
                .to_string(),
        );
    }
    config.pipe_name(&logon_sid).map_err(|error| error.message)
}

/// Starts the Hub and waits for the connections.
fn run(config: &HubConfig) -> Result<(), String> {
    config.validate().map_err(|error| error.message)?;
    let identity = platform_identity();
    let mut handle = start_or_connect(config).map_err(|error| error.message)?;
    if handle.role() == HubRole::Client {
        println!(
            "本登录会话中已有一个 Hub 在运行（管道名 {}），本次进程不重复启动。",
            handle.pipe_name()
        );
        return Ok(());
    }

    let listener = handle
        .take_listener()
        .ok_or_else(|| "the pipe of the hub could not be opened".to_string())?;

    let mut hub = Hub::new(config.clone(), identity.as_ref()).map_err(|error| error.message)?;
    hub = hub.with_listener(listener);

    // the persistence is optional: a hub without a store still serves the agents, it only forgets
    // everything when it ends
    match open_store(config) {
        Ok(store) => {
            hub = hub.with_store(store).map_err(|error| error.message)?;
        }
        Err(reason) => {
            eprintln!("tase-local-hub: the store is not available ({reason}); the hub runs without persistence");
        }
    }

    let now_ms = hub.now_ms();
    match hub.recover(now_ms) {
        Ok(report) => {
            if report.recovered_jobs > 0 {
                println!("恢复了 {} 个未完成的作业。", report.recovered_jobs);
            }
        }
        Err(error) => eprintln!("tase-local-hub: the recovery failed ({})", error.message),
    }

    install_console_handler();
    let stop = hub.stop_flag();
    std::thread::spawn(move || {
        while !STOP_REQUESTED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        stop.store(true, Ordering::SeqCst);
    });

    println!(
        "Hub 已启动，管道名是 {}，互斥体名是 {}。",
        handle.pipe_name(),
        handle.mutex_name()
    );

    hub.run().map_err(|error| error.message)?;
    println!("Hub 已停止。");
    Ok(())
}

/// Opens the store of the Hub.
fn open_store(config: &HubConfig) -> Result<Store, String> {
    let path = config.store_path();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("the data directory could not be made: {error}"))?;
    }
    Store::open(&path).map_err(|error| error.message)
}

/// Installs the handler of the console, so that Ctrl+C ends the Hub in an orderly way.
#[cfg(windows)]
fn install_console_handler() {
    use windows::Win32::Foundation::{BOOL, FALSE, TRUE};
    use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT};

    // SAFETY: the handler only sets an atomic flag, which is safe from any thread; the signature is
    // the one the console of Windows asks for.
    unsafe extern "system" fn handler(kind: u32) -> BOOL {
        if kind == CTRL_C_EVENT
            || kind == CTRL_BREAK_EVENT
            || kind == CTRL_CLOSE_EVENT
            || kind == CTRL_LOGOFF_EVENT
            || kind == CTRL_SHUTDOWN_EVENT
        {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            TRUE
        } else {
            FALSE
        }
    }

    // SAFETY: the handler is a plain function that lives for the whole run of the process.
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(handler), TRUE);
    }
}

/// Without a console of Windows, Ctrl+C ends the process the way the system says.
#[cfg(not(windows))]
fn install_console_handler() {}

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
    fn the_dry_run_of_the_pipe_name_is_asked_for() {
        let options = parse(&args(&["--print-pipe-name"])).expect("the command line is read");
        assert_eq!(options.action, Action::PrintPipeName);
    }

    #[test]
    fn the_data_directory_is_read() {
        let options = parse(&args(&["--data-dir", "D:/tase"])).expect("the command line is read");
        assert_eq!(options.action, Action::Run);
        assert_eq!(options.config.data_dir, std::path::PathBuf::from("D:/tase"));
    }

    #[test]
    fn the_limits_are_read() {
        let options = parse(&args(&["--max-connections", "8", "--max-jobs", "3"]))
            .expect("the command line is read");
        assert_eq!(options.config.max_connections, 8);
        assert_eq!(options.config.max_concurrent_jobs, 3);
    }

    #[test]
    fn an_option_without_its_value_is_refused() {
        let error = parse(&args(&["--data-dir"])).expect_err("the command line is refused");
        assert!(error.contains("--data-dir"), "{error}");
    }

    #[test]
    fn an_unknown_option_is_refused() {
        let error = parse(&args(&["--nonsense"])).expect_err("the command line is refused");
        assert!(error.contains("--nonsense"), "{error}");
    }

    #[test]
    fn a_limit_that_is_not_a_number_is_refused() {
        let error = parse(&args(&["--max-jobs", "many"])).expect_err("the command line is refused");
        assert!(error.contains("--max-jobs"), "{error}");
    }

    #[test]
    fn the_usage_names_the_options() {
        for option in [
            "--help",
            "--version",
            "--print-pipe-name",
            "--data-dir",
            "--max-connections",
            "--max-jobs",
            "--scheduler",
            "--agent-weight",
        ] {
            assert!(USAGE.contains(option), "the usage must name {option}");
        }
    }

    #[test]
    fn the_scheduler_of_the_jobs_is_read() {
        let options = parse(&args(&["--scheduler", "fair"])).expect("the command line is read");
        assert_eq!(options.config.scheduler, SchedulerKind::Fair);
        let options = parse(&args(&["--scheduler", "round-robin"]))
            .expect("the command line is read");
        assert_eq!(options.config.scheduler, SchedulerKind::RoundRobin);
    }

    #[test]
    fn a_scheduler_that_is_not_ours_is_refused() {
        let error = parse(&args(&["--scheduler", "nonsense"])).expect_err("the scheduler is refused");
        assert!(error.contains("--scheduler"), "{error}");
    }

    #[test]
    fn the_weight_of_an_agent_is_read() {
        let options = parse(&args(&["--agent-weight", "3"])).expect("the command line is read");
        assert_eq!(options.config.agent_weight, 3);
    }

    #[test]
    fn a_weight_that_is_not_a_number_is_refused() {
        let error = parse(&args(&["--agent-weight", "heavy"])).expect_err("the weight is refused");
        assert!(error.contains("--agent-weight"), "{error}");
    }

    #[test]
    fn the_version_is_the_version_of_the_crate() {
        assert_eq!(tase_local_hub::VERSION, "0.1.0");
    }
}
