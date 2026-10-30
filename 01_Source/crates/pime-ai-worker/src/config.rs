//! Command line and the test-mode gate.
//!
//! The product is started without any flag: system trust store, https only, Credential Manager namespace
//! `ProgrammingIME`, persistent credentials. Every flag below exists for the automated tests, loosens something and
//! refuses to start unless the environment variable `PIME_WORKER_TEST_MODE=1` is also set, so a flag that leaks into a
//! shortcut or a script cannot silently weaken a real installation.
#![forbid(unsafe_code)]

pub const TEST_MODE_ENV: &str = "PIME_WORKER_TEST_MODE";
pub const DEFAULT_NAMESPACE: &str = "ProgrammingIME";
/// A test namespace must start with this, so a test can never address a real credential.
pub const TEST_NAMESPACE_PREFIX: &str = "PIME-TEST-";

pub const USAGE: &str = "usage: pime-ai-worker [--help]\n\
\n\
JSON Lines over stdin/stdout (see docs/AI_WORKER.md). The product runs without options.\n\
\n\
Test-only options (each is refused unless PIME_WORKER_TEST_MODE=1 is set in the environment):\n\
  --insecure-test-ca              accept an untrusted certificate authority, for loopback hosts only\n\
  --test-allow-loopback-http      accept plain http for 127.x.x.x and localhost\n\
  --test-credential-namespace N   use Credential Manager target names N/<provider> with session\n\
                                  persistence (N must start with PIME-TEST-)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub insecure_test_ca: bool,
    pub allow_loopback_http: bool,
    /// Credential Manager target names are `<namespace>/<provider>`.
    pub namespace: String,
    /// Test namespaces use session persistence: the entries disappear at log-off even if a test is killed.
    pub session_credentials: bool,
    pub test_mode: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            insecure_test_ca: false,
            allow_loopback_http: false,
            namespace: DEFAULT_NAMESPACE.to_string(),
            session_credentials: false,
            test_mode: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// `--help` was requested.
    Help,
    /// The message is fixed text and never echoes an argument value.
    Invalid(&'static str),
}

fn valid_namespace(ns: &str) -> bool {
    ns.starts_with(TEST_NAMESPACE_PREFIX)
        && ns.len() <= 64
        && ns
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Parses the arguments (without the program name). `test_mode_env` is the value of `PIME_WORKER_TEST_MODE`; it is a
/// parameter so the gate can be tested without touching the process environment.
pub fn parse_config<I>(args: I, test_mode_env: Option<&str>) -> Result<Config, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let test_mode = test_mode_env == Some("1");
    let mut cfg = Config {
        test_mode,
        ..Config::default()
    };
    let mut args = args.into_iter();
    let mut used_test_flag = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(ConfigError::Help),
            "--insecure-test-ca" => {
                cfg.insecure_test_ca = true;
                used_test_flag = true;
            }
            "--test-allow-loopback-http" => {
                cfg.allow_loopback_http = true;
                used_test_flag = true;
            }
            "--test-credential-namespace" => {
                let Some(ns) = args.next() else {
                    return Err(ConfigError::Invalid("missing value for an option"));
                };
                if !valid_namespace(&ns) {
                    return Err(ConfigError::Invalid(
                        "the test credential namespace must start with PIME-TEST-",
                    ));
                }
                cfg.namespace = ns;
                cfg.session_credentials = true;
                used_test_flag = true;
            }
            _ => return Err(ConfigError::Invalid("unknown option (see --help)")),
        }
    }
    if used_test_flag && !test_mode {
        return Err(ConfigError::Invalid(
            "a test-only option was given but PIME_WORKER_TEST_MODE=1 is not set; refusing to start",
        ));
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: Option<&str>) -> Result<Config, ConfigError> {
        parse_config(args.iter().map(|s| s.to_string()), env)
    }

    #[test]
    fn the_product_default_relaxes_nothing() {
        let c = parse(&[], None).unwrap();
        assert_eq!(c, Config::default());
        assert!(!c.insecure_test_ca && !c.allow_loopback_http && !c.session_credentials);
        assert_eq!(c.namespace, "ProgrammingIME");
        // test mode alone (the variable without any flag) relaxes nothing either
        let c = parse(&[], Some("1")).unwrap();
        assert!(c.test_mode && !c.insecure_test_ca && !c.allow_loopback_http);
        assert_eq!(c.namespace, "ProgrammingIME");
    }

    #[test]
    fn every_test_flag_is_refused_without_the_environment_variable() {
        for args in [
            vec!["--insecure-test-ca"],
            vec!["--test-allow-loopback-http"],
            vec!["--test-credential-namespace", "PIME-TEST-abc"],
            vec!["--test-allow-loopback-http", "--insecure-test-ca"],
        ] {
            for env in [
                None,
                Some(""),
                Some("0"),
                Some("true"),
                Some("yes"),
                Some("1 "),
            ] {
                assert!(parse(&args, env).is_err(), "{args:?} with {env:?}");
            }
        }
    }

    #[test]
    fn the_flags_work_in_test_mode() {
        let c = parse(&["--insecure-test-ca"], Some("1")).unwrap();
        assert!(c.insecure_test_ca && !c.allow_loopback_http && !c.session_credentials);
        let c = parse(&["--test-allow-loopback-http"], Some("1")).unwrap();
        assert!(c.allow_loopback_http && !c.insecure_test_ca);
        let c = parse(
            &[
                "--test-credential-namespace",
                "PIME-TEST-0a1b2c",
                "--insecure-test-ca",
            ],
            Some("1"),
        )
        .unwrap();
        assert_eq!(c.namespace, "PIME-TEST-0a1b2c");
        assert!(c.session_credentials && c.insecure_test_ca);
    }

    #[test]
    fn a_test_namespace_can_never_address_a_real_credential() {
        for ns in [
            "ProgrammingIME",
            "PIME-TEST",
            "pime-test-abc",
            "PIME-TEST-a/b",
            "PIME-TEST-a b",
            "PIME-TEST-*",
            "PIME-TEST-\u{e9}",
            "",
        ] {
            let e = parse(&["--test-credential-namespace", ns], Some("1")).unwrap_err();
            assert!(matches!(e, ConfigError::Invalid(_)), "{ns:?}");
        }
        assert!(parse(&["--test-credential-namespace"], Some("1")).is_err());
        let long = format!("PIME-TEST-{}", "a".repeat(60));
        assert!(parse(&["--test-credential-namespace", long.as_str()], Some("1")).is_err());
    }

    #[test]
    fn unknown_options_and_help() {
        assert_eq!(parse(&["--help"], None), Err(ConfigError::Help));
        assert_eq!(parse(&["-h"], Some("1")), Err(ConfigError::Help));
        assert!(matches!(
            parse(&["--frobnicate"], None),
            Err(ConfigError::Invalid(_))
        ));
        assert!(matches!(
            parse(&["positional"], Some("1")),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn error_messages_never_echo_arguments() {
        let secret = "sk-proj-AbCdEf0123456789AbCdEf";
        let e = parse(&[secret], None).unwrap_err();
        let ConfigError::Invalid(msg) = e else {
            panic!()
        };
        assert!(!msg.contains(secret));
        let e = parse(&["--test-credential-namespace", secret], Some("1")).unwrap_err();
        let ConfigError::Invalid(msg) = e else {
            panic!()
        };
        assert!(!msg.contains(secret));
    }
}
