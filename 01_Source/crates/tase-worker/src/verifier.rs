//! The verifier of a job: the gates of the sandbox, the assertions of the acceptance file and the tokens and the
//! cost of the job (detailed design §7.2, §9 and §10).
//!
//! # The gates
//!
//! Detailed design §7.2 says that the compiler, the formatter and the tests are started from a **fixed allowlist**,
//! with a given working directory and a given argument vector, and never from a shell string. This module is the
//! door that enforces it: a command whose program is not in the allowlist, or whose working directory is outside of
//! the workspace, is refused before it is started.
//!
//! The module does **not** start a process: the door to the sandbox is the [`CommandRunner`] of the caller, so that a
//! test answers what it wants without a process, and so that the isolation itself stays where it belongs (the
//! operating system and the runner of the commands).
//!
//! # The acceptance file
//!
//! The assertions of the verifier are the assertions of the acceptance file of detailed design §10.1: a name, a state
//! and a message. The summary is the one of `tase-metrics`: a suite is green when no assertion failed **and** no
//! assertion did not run. A gate that could not be started is therefore `NOT_RUN` and never `PASS`.
//!
//! # The metrics of the job
//!
//! The tokens of the job come from the meter of `tase-metrics`, the cost from the table of the prices, and the
//! verdict of the budget from the budget of the job. The three numbers are different things and the verifier keeps
//! them apart: the tokens are a volume, the cost is a price and the latency is a duration (detailed design §9.2).

use std::path::{Path, PathBuf};

use tase_contract::TaseError;
use tase_contract::api::CheckStatus;
use tase_metrics::aci::{AciReport, AciSummary, Assertion, summarise, to_json};
use tase_metrics::meter::{BudgetVerdict, Meter, TokenBudget, Usage};
use tase_metrics::pricing::PriceTable;

/// The programs the sandbox may start.
///
/// The list is the one of detailed design §7.2: the formatter, the compiler and the test runner of the language of the
/// repository. A program that is not in the list is refused.
pub const ALLOWED_COMMANDS: &[&str] = &["cargo", "rustc", "rustfmt"];

/// A command of the sandbox: the program, the arguments and the working directory.
///
/// The arguments are a vector and never a shell string: nothing of the arguments is ever interpreted by a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxCommand {
    /// The program that is started.
    pub program: String,
    /// The arguments of the program.
    pub args: Vec<String>,
    /// The directory the program runs in.
    pub cwd: PathBuf,
}

impl SandboxCommand {
    /// A command of the sandbox.
    ///
    /// The program has to be in the allowlist and the working directory has to be inside the root of the workspace.
    pub fn new(
        program: &str,
        args: &[&str],
        cwd: impl Into<PathBuf>,
        root: &Path,
    ) -> Result<SandboxCommand, TaseError> {
        if !ALLOWED_COMMANDS.contains(&program) {
            return Err(TaseError::policy_rejected(format!(
                "the program {program} is not in the allowlist of the sandbox"
            )));
        }
        let cwd = cwd.into();
        if !cwd.starts_with(root) {
            return Err(TaseError::policy_rejected(format!(
                "the directory {} is outside of the workspace",
                cwd.display()
            )));
        }
        Ok(SandboxCommand {
            program: program.to_string(),
            args: args.iter().map(|argument| argument.to_string()).collect(),
            cwd,
        })
    }

    /// The command as the argument vector it is started with.
    pub fn argv(&self) -> Vec<&str> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.as_str());
        for argument in &self.args {
            argv.push(argument.as_str());
        }
        argv
    }
}

/// What a command of the sandbox answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    /// The code the command ended with.
    pub exit_code: i32,
    /// What the command wrote on its standard output.
    pub stdout: String,
    /// What the command wrote on its standard error.
    pub stderr: String,
    /// Whether the command ran out of time.
    pub timed_out: bool,
}

impl CommandResult {
    /// A command that passed.
    pub fn passed() -> CommandResult {
        CommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    /// A command that failed with a code.
    pub fn failed(exit_code: i32) -> CommandResult {
        CommandResult {
            exit_code,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }
}

/// The door to the sandbox: the part that starts the command.
pub trait CommandRunner {
    /// Starts the command and waits for it.
    fn run(&mut self, command: &SandboxCommand) -> Result<CommandResult, TaseError>;
}

/// One gate of a job: the name of the check and the command that has to pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    /// The name of the assertion of the gate.
    pub name: String,
    /// The command of the gate.
    pub command: SandboxCommand,
}

impl Gate {
    /// A gate of a job.
    pub fn new(name: &str, command: SandboxCommand) -> Gate {
        Gate {
            name: name.to_string(),
            command,
        }
    }
}

/// What the verification of a job found.
#[derive(Clone, Debug, PartialEq)]
pub struct Verification {
    /// The name of the suite.
    pub suite: String,
    /// The assertions of the suite.
    pub report: AciReport,
    /// The summary of the assertions.
    pub summary: AciSummary,
    /// The tokens of the job.
    pub usage: Usage,
    /// The tokens that are counted once for the budget of the job.
    pub billable_tokens: u64,
    /// The cost of the job in dollars; `None` when the price of the model is unknown.
    pub cost: Option<f64>,
    /// How long the job took, in milliseconds.
    pub latency_ms: u64,
    /// The verdict of the budget of the job.
    pub budget: BudgetVerdict,
    /// Whether the job may go on: the gates passed and the budget was not exceeded.
    pub ok: bool,
}

impl Verification {
    /// The report of the verification as the JSON of the acceptance file.
    pub fn to_json(&self) -> String {
        to_json(&self.report)
    }
}

/// The verifier of a job.
#[derive(Clone, Debug)]
pub struct Verifier {
    suite: String,
    gates: Vec<Gate>,
    prices: PriceTable,
    provider: String,
    model: String,
    budget: TokenBudget,
}

impl Verifier {
    /// A verifier with the gates of a job.
    pub fn new(suite: &str, gates: Vec<Gate>) -> Verifier {
        Verifier {
            suite: suite.to_string(),
            gates,
            prices: PriceTable::default(),
            provider: String::new(),
            model: String::new(),
            budget: TokenBudget::new(u64::MAX),
        }
    }

    /// The verifier with the table of the prices of the model of the job.
    pub fn with_prices(mut self, prices: PriceTable, provider: &str, model: &str) -> Verifier {
        self.prices = prices;
        self.provider = provider.to_string();
        self.model = model.to_string();
        self
    }

    /// The verifier with the budget of the job.
    pub fn with_budget(mut self, budget: TokenBudget) -> Verifier {
        self.budget = budget;
        self
    }

    /// The gates of the verifier.
    pub fn gates(&self) -> &[Gate] {
        &self.gates
    }

    /// Runs the gates and summarises what they found.
    ///
    /// The time of the job is given by the caller: the verifier never reads the clock.
    pub fn verify(
        &self,
        runner: &mut dyn CommandRunner,
        meter: &Meter,
        started_at_ms: u64,
        now_ms: u64,
    ) -> Verification {
        let mut assertions = Vec::with_capacity(self.gates.len());
        for gate in &self.gates {
            let assertion = match runner.run(&gate.command) {
                Ok(result) if result.timed_out => {
                    Assertion::failed(&gate.name, "the command ran out of time")
                }
                Ok(result) if result.exit_code == 0 => Assertion::passed(&gate.name),
                Ok(result) => Assertion::failed(
                    &gate.name,
                    &format!("the command ended with the code {}", result.exit_code),
                ),
                Err(error) => Assertion {
                    name: gate.name.clone(),
                    status: CheckStatus::NotRun,
                    message: Some(error.message),
                },
            };
            assertions.push(assertion);
        }

        let report = AciReport::new(&self.suite, assertions);
        let summary = summarise(&report);
        let usage = meter.total();
        let billable_tokens = meter.billable_tokens();
        let cost = self.prices.cost_of(&usage, &self.provider, &self.model);
        let budget = self.budget.check(billable_tokens);
        let ok = summary.ok && budget.is_allowed();

        Verification {
            suite: self.suite.clone(),
            report,
            summary,
            usage,
            billable_tokens,
            cost,
            latency_ms: now_ms.saturating_sub(started_at_ms),
            budget,
            ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use tase_metrics::meter::Usage;
    use tase_metrics::pricing::PriceEntry;

    use super::*;

    /// The root of the workspace of the tests.
    fn root() -> PathBuf {
        PathBuf::from("/workspace")
    }

    fn command(program: &str) -> SandboxCommand {
        SandboxCommand::new(program, &["check"], root().join("crate"), &root())
            .expect("the command of the sandbox is built")
    }

    /// A runner that answers what the test tells it to answer.
    struct ScriptedRunner {
        answers: Vec<Result<CommandResult, TaseError>>,
        commands: Vec<SandboxCommand>,
    }

    impl ScriptedRunner {
        fn new(answers: Vec<Result<CommandResult, TaseError>>) -> ScriptedRunner {
            ScriptedRunner {
                answers,
                commands: Vec::new(),
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&mut self, command: &SandboxCommand) -> Result<CommandResult, TaseError> {
            self.commands.push(command.clone());
            if self.answers.is_empty() {
                return Ok(CommandResult::passed());
            }
            if self.answers.len() == 1 {
                return self.answers[0].clone();
            }
            self.answers.remove(0)
        }
    }

    fn meter_with(tokens: u64) -> Meter {
        let mut meter = Meter::new();
        meter.record(Usage::new(tokens, tokens / 10, tokens / 2, None), 1_000);
        meter
    }

    fn prices() -> PriceTable {
        PriceTable::new(vec![PriceEntry::new("openai", "gpt-test", 1.0, 2.0, 0.5)])
    }

    #[test]
    fn a_program_that_is_not_in_the_allowlist_is_refused() {
        let error = SandboxCommand::new("sh", &["-c", "echo"], root().join("crate"), &root())
            .expect_err("the program is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert!(error.message.contains("allowlist"));
    }

    #[test]
    fn a_directory_outside_of_the_workspace_is_refused() {
        let error = SandboxCommand::new("cargo", &["check"], PathBuf::from("/etc"), &root())
            .expect_err("the directory is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
    }

    #[test]
    fn the_command_is_a_vector_and_never_a_shell_string() {
        let command = command("cargo");
        assert_eq!(command.argv(), ["cargo", "check"]);
        assert_eq!(command.program, "cargo");
    }

    #[test]
    fn a_gate_that_passes_gives_a_passed_assertion() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![Gate::new("cargo-check", command("cargo"))],
        );
        let mut runner = ScriptedRunner::new(vec![Ok(CommandResult::passed())]);
        let meter = Meter::new();

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(verification.summary.passed, 1);
        assert_eq!(verification.summary.failed, 0);
        assert_eq!(verification.summary.total, 1);
        assert!(verification.summary.ok);
        assert!(verification.ok);
        assert_eq!(verification.latency_ms, 1_000);
    }

    #[test]
    fn a_gate_that_fails_makes_the_suite_red() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![Gate::new("cargo-check", command("cargo"))],
        );
        let mut runner = ScriptedRunner::new(vec![Ok(CommandResult::failed(101))]);
        let meter = Meter::new();

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(verification.summary.failed, 1);
        assert!(!verification.summary.ok);
        assert!(!verification.ok);
    }

    #[test]
    fn a_gate_that_did_not_run_is_not_a_passed_gate() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![Gate::new("cargo-check", command("cargo"))],
        );
        let mut runner = ScriptedRunner::new(vec![Err(TaseError::worker_unavailable(
            "the sandbox is not there",
        ))]);
        let meter = Meter::new();

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(verification.summary.not_run, 1);
        assert_eq!(verification.summary.passed, 0);
        assert!(
            !verification.summary.ok,
            "a check that did not run is not green"
        );
    }

    #[test]
    fn the_assertions_and_the_tokens_and_the_cost_are_summarised() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![Gate::new("cargo-check", command("cargo"))],
        )
        .with_prices(prices(), "openai", "gpt-test")
        .with_budget(TokenBudget::new(10_000));
        let mut runner = ScriptedRunner::new(vec![Ok(CommandResult::passed())]);
        let meter = meter_with(1_000);

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(verification.summary.passed, 1);
        assert_eq!(verification.usage.input_tokens, 1_000);
        assert_eq!(verification.usage.output_tokens, 100);
        assert_eq!(verification.usage.cached_input_tokens, 500);
        assert_eq!(verification.billable_tokens, 1_100);
        // 1000 input tokens at 1 dollar the million, 100 output tokens at 2 dollars the million
        // and 500 cached tokens at 0.5 dollar the million
        let cost = verification.cost.expect("the price of the model is known");
        assert!((cost - 0.00145).abs() < 1e-9, "the cost is {cost}");
        assert_eq!(verification.budget, BudgetVerdict::Allowed);
        assert!(verification.ok);
    }

    #[test]
    fn a_job_that_used_too_many_tokens_is_not_green() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![Gate::new("cargo-check", command("cargo"))],
        )
        .with_budget(TokenBudget::new(100));
        let mut runner = ScriptedRunner::new(vec![Ok(CommandResult::passed())]);
        let meter = meter_with(1_000);

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(verification.budget, BudgetVerdict::Exceeded);
        assert!(!verification.ok, "the budget was exceeded");
    }

    #[test]
    fn an_unknown_price_is_not_a_cost_of_zero() {
        let verifier = Verifier::new("tase-worker", vec![]);
        let mut runner = ScriptedRunner::new(vec![]);
        let meter = meter_with(1_000);

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(verification.cost, None);
    }

    #[test]
    fn the_report_of_the_verification_is_the_json_of_the_acceptance_file() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![Gate::new("cargo-check", command("cargo"))],
        );
        let mut runner = ScriptedRunner::new(vec![Ok(CommandResult::passed())]);
        let meter = Meter::new();

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);
        let json = verification.to_json();

        assert!(json.contains("cargo-check"), "the json is {json}");
        assert!(json.contains("PASS"), "the json is {json}");
    }

    #[test]
    fn every_gate_is_run() {
        let verifier = Verifier::new(
            "tase-worker",
            vec![
                Gate::new("cargo-check", command("cargo")),
                Gate::new("cargo-test", command("cargo")),
            ],
        );
        let mut runner = ScriptedRunner::new(vec![
            Ok(CommandResult::passed()),
            Ok(CommandResult::failed(1)),
        ]);
        let meter = Meter::new();

        let verification = verifier.verify(&mut runner, &meter, 1_000, 2_000);

        assert_eq!(runner.commands.len(), 2);
        assert_eq!(verification.summary.passed, 1);
        assert_eq!(verification.summary.failed, 1);
        assert_eq!(verifier.gates().len(), 2);
    }
}
