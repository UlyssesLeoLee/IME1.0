//! The resident binary of the TASE worker.
//!
//! The binary does four things and nothing else: it reads its configuration, it opens the store of the worker and
//! runs the recovery of the state, it takes the jobs of the queue one at a time, and it stops in an orderly way when
//! it is asked to.
//!
//! ```text
//! tase-worker --help
//! tase-worker --data-dir D:\IDE1.0\tase --worker-id worker-1
//! ```
//!
//! The host editor is not connected to the worker yet (the pipe of the local hub is not there), so every job of the
//! binary is refused with `WORKER_UNAVAILABLE` and is tried again after the backoff. The skeleton of the worker is
//! the part that is here; the door to the host belongs to `tase-local-ipc` and to `tase-local-hub`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tase_contract::TaseError;
use tase_local_store::Store;
use tase_worker::consumer::{
    Consumer, MemoryTaskSource, StoreTaskSource, Task, TaskExecutor, TaskResult, TaskSource,
};
use tase_worker::lease::{Clock, SystemClock};
use tase_worker::main::{StopSignal, Worker, WorkerConfig};
use tase_worker::recovery::reconcile_store;

/// What the binary was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// Print the usage and stop.
    Help,
    /// Print the version and stop.
    Version,
    /// Run the worker.
    Run,
}

/// What the command line says.
#[derive(Clone, Debug)]
struct Options {
    action: Action,
    config: WorkerConfig,
    data_dir: Option<PathBuf>,
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
      --data-dir <dir>     the directory where the worker keeps its store
      --worker-id <id>      the id of the worker, the holder of its leases (default: tase-worker-1)
      --lease-seconds <n>   how long the lease of a job is good for (default: 30)
      --heartbeat-seconds <n>  how often the worker tells the queue that it is alive (default: 10)
      --poll-ms <n>         how long the worker waits when there is nothing to do (default: 500)
      --max-attempts <n>    how many times a job may be tried (default: 3)
      --stop-file <path>    the file whose presence asks the worker to stop
      --once                run one job and stop

The worker takes the jobs of the queue one at a time, it keeps the lease of the job it runs and it stops between two
jobs when it is asked to.
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
        Action::Run => match run(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(reason) => {
                eprintln!("tase-worker: {reason}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Reads the command line.
fn parse(args: &[String]) -> Result<Options, String> {
    let mut action = Action::Run;
    let mut config = WorkerConfig::default();
    let mut data_dir = None;
    let mut stop_file = None;
    let mut once = false;

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "-h" | "--help" => action = Action::Help,
            "-V" | "--version" => action = Action::Version,
            "--once" => once = true,
            "--data-dir" => {
                data_dir = Some(PathBuf::from(value_of(args, index + 1, "--data-dir")?));
                index += 1;
            }
            "--stop-file" => {
                stop_file = Some(PathBuf::from(value_of(args, index + 1, "--stop-file")?));
                index += 1;
            }
            "--worker-id" => {
                config.worker_id = value_of(args, index + 1, "--worker-id")?;
                index += 1;
            }
            "--lease-seconds" => {
                config.lease_ttl_ms = 1_000 * number_of(args, index + 1, "--lease-seconds")?;
                index += 1;
            }
            "--heartbeat-seconds" => {
                config.heartbeat_ms = 1_000 * number_of(args, index + 1, "--heartbeat-seconds")?;
                index += 1;
            }
            "--poll-ms" => {
                config.poll_interval_ms = number_of(args, index + 1, "--poll-ms")?;
                index += 1;
            }
            "--max-attempts" => {
                let value = number_of(args, index + 1, "--max-attempts")?;
                config.max_attempts = u32::try_from(value)
                    .map_err(|_| "--max-attempts wants a small number".to_string())?;
                index += 1;
            }
            other => return Err(format!("unknown option {other}")),
        }
        index += 1;
    }

    Ok(Options {
        action,
        config,
        data_dir,
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

/// The executor of a worker that has no host editor.
///
/// The pipe of the local hub does not exist yet, so the worker has no door to the editor: every job is refused with
/// `WORKER_UNAVAILABLE`, and the queue tries it again after the backoff. The worker never pretends that it wrote
/// something.
struct NoHostExecutor;

impl TaskExecutor for NoHostExecutor {
    fn execute(&mut self, task: &Task, _now_ms: u64) -> Result<TaskResult, TaseError> {
        Err(TaseError::worker_unavailable(format!(
            "the job {} has no host editor to run in",
            task.task_id
        )))
    }
}

/// Runs the worker.
fn run(options: &Options) -> Result<(), String> {
    let config = options.config.clone();
    config.validate().map_err(|error| error.message)?;
    let clock = SystemClock;
    let stop = StopSignal::new();
    let signal = stop.clone();

    let source: Box<dyn TaskSource> = match &options.data_dir {
        Some(dir) => {
            let path = dir.join("tase-worker.sqlite3");
            let mut store = Store::open(&path).map_err(|error| error.message)?;
            tase_local_store::migrations::apply(&mut store).map_err(|error| error.message)?;
            let report =
                reconcile_store(&mut store, clock.now_ms()).map_err(|error| error.message)?;
            println!(
                "the recovery took {} leases back and sent {} jobs back to the queue",
                report.expired_leases, report.requeued_jobs
            );
            Box::new(StoreTaskSource::new(
                store,
                &config.worker_id,
                config.lease_ttl_ms,
                config.retry_policy(),
            ))
        }
        None => Box::new(MemoryTaskSource::new(
            &config.worker_id,
            config.lease_ttl_ms,
        )),
    };

    let executor: Box<dyn TaskExecutor> = Box::new(NoHostExecutor);
    let consumer = Consumer::new(source, executor, config.retry_policy());
    let mut worker = Worker::new(config.clone(), consumer, stop);

    if options.once {
        let outcome = worker.step(clock.now_ms()).map_err(|error| error.message)?;
        println!("the worker ran one job: {outcome:?}");
        return Ok(());
    }

    let stop_file = options.stop_file.clone();
    let poll_interval_ms = config.poll_interval_ms;
    let shutdown = worker
        .run(&clock, move |_now_ms| {
            if stop_file.as_ref().is_some_and(|path| path.exists()) {
                signal.stop();
            } else {
                std::thread::sleep(Duration::from_millis(poll_interval_ms));
            }
        })
        .map_err(|error| error.message)?;

    println!("the worker stopped after {} jobs", shutdown.processed);
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
    fn the_configuration_is_read() {
        let options = parse(&args(&[
            "--data-dir",
            "D:/tase",
            "--worker-id",
            "worker-7",
            "--lease-seconds",
            "45",
            "--heartbeat-seconds",
            "5",
            "--poll-ms",
            "250",
            "--max-attempts",
            "5",
        ]))
        .expect("the command line is read");

        assert_eq!(options.data_dir, Some(PathBuf::from("D:/tase")));
        assert_eq!(options.config.worker_id, "worker-7");
        assert_eq!(options.config.lease_ttl_ms, 45_000);
        assert_eq!(options.config.heartbeat_ms, 5_000);
        assert_eq!(options.config.poll_interval_ms, 250);
        assert_eq!(options.config.max_attempts, 5);
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
        let error = parse(&args(&["--data-dir"])).expect_err("the command line is refused");
        assert!(error.contains("--data-dir"), "{error}");
    }

    #[test]
    fn an_option_that_wants_a_number_is_refused_when_it_is_not_one() {
        let error =
            parse(&args(&["--max-attempts", "many"])).expect_err("the command line is refused");
        assert!(error.contains("--max-attempts"), "{error}");
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
            "--data-dir",
            "--worker-id",
            "--lease-seconds",
            "--heartbeat-seconds",
            "--poll-ms",
            "--max-attempts",
            "--stop-file",
            "--once",
        ] {
            assert!(USAGE.contains(option), "the usage must name {option}");
        }
    }

    #[test]
    fn the_version_of_the_binary_is_the_version_of_the_crate() {
        assert_eq!(tase_worker::WORKER_VERSION, "1.1.0");
    }
}
