//! The command line front-end: `pime-ime <op> [arguments]`.
//!
//! The CLI is for the clients that cannot speak MCP (a script, a batch file, a shell). It exposes the same
//! operations as the MCP server and the local server, because all three call the same dispatch.
//!
//! The default output is one short line for a human; `--json` prints the raw JSON answer, which is what a script
//! reads. The exit code is 0 when the operation worked, 1 when it failed (the error code is printed) and 2 when the
//! command line itself is wrong.

use crate::serve;
use crate::{dispatch_op, Backend, Identity, Op};
use pime_core::json::{self, Json, Limits};
use std::path::PathBuf;

/// The exit code of a command line that is wrong.
pub const EXIT_USAGE: i32 = 2;

/// One command of the CLI.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The text of the help.
    Help,
    /// The state of the input method and of the core.
    Status,
    /// Write text at the caret of the focused document.
    Insert {
        /// The text to write.
        text: String,
        /// The language of the write.
        language: Option<String>,
        /// Whether the selection is replaced instead of the caret.
        replace: bool,
    },
    /// Read the text that is selected in the focused document.
    Read,
    /// Switch the language of the input method.
    Language {
        /// The language to switch to.
        language: String,
    },
    /// The candidates of the built-in dictionary.
    Candidates {
        /// The pinyin or the romaji that was typed.
        text: String,
        /// The dictionary to ask.
        language: Option<String>,
    },
    /// The code candidates of the core engine.
    Predict {
        /// The prefix that was typed.
        text: String,
        /// The language of the code.
        language: Option<String>,
        /// How many candidates to answer with.
        limit: Option<i64>,
    },
    /// Validate a snippet with the core.
    Validate {
        /// The snippet.
        text: String,
        /// The language of the snippet.
        language: String,
        /// The dialect of the snippet.
        dialect: Option<Json>,
    },
    /// Store the words an AI proposes in the word bank of the user.
    Curate {
        /// The file that carries the proposals.
        file: PathBuf,
    },
    /// The words the user typed and that had no candidate.
    Misses,
    /// The language of a file.
    Resolve {
        /// The path of the file.
        path: String,
    },
    /// The secret scan of a text.
    Scan {
        /// The text to scan.
        text: String,
    },
    /// The local server (the named pipe and the HTTP server).
    Serve {
        /// Whether the named pipe is served.
        pipe: bool,
        /// The address of the HTTP server.
        http: Option<String>,
        /// The bearer token of the HTTP server.
        token: Option<String>,
    },
}

/// One invocation of the CLI.
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    /// The command.
    pub command: Command,
    /// Whether the raw JSON answer is printed instead of the short line.
    pub json: bool,
    /// Who this client says it is (the provider, the agent and the instance of it).
    pub identity: Identity,
}

/// Parses the arguments of the command line (without the name of the program).
pub fn parse_args(args: &[String]) -> Result<Invocation, String> {
    let Some(first) = args.first() else {
        return Err("there is no operation".to_string());
    };
    if is_help(first) {
        return Ok(Invocation {
            command: Command::Help,
            json: false,
            identity: Identity::default(),
        });
    }
    let op = first.clone();
    let mut json = false;
    let mut text: Option<String> = None;
    let mut language: Option<String> = None;
    let mut path: Option<String> = None;
    let mut file: Option<String> = None;
    let mut dialect: Option<Json> = None;
    let mut limit: Option<i64> = None;
    let mut replace = false;
    let mut pipe = false;
    let mut http: Option<String> = None;
    let mut token: Option<String> = None;
    let mut provider: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut instance: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--json" => json = true,
            "--replace" | "-r" => replace = true,
            "--pipe" => pipe = true,
            "--text" | "-t" => text = Some(value_of(args, &mut index, arg)?),
            "--language" | "-l" => language = Some(value_of(args, &mut index, arg)?),
            "--path" => path = Some(value_of(args, &mut index, arg)?),
            "--file" | "-f" => file = Some(value_of(args, &mut index, arg)?),
            "--limit" => {
                let value = value_of(args, &mut index, arg)?;
                limit = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| format!("the limit {value} is not a number"))?,
                );
            }
            "--dialect" => {
                let value = value_of(args, &mut index, arg)?;
                let parsed = json::parse(value.trim(), Limits::DEFAULT)
                    .map_err(|error| format!("the dialect is not JSON: {error}"))?;
                if parsed.as_object().is_none() {
                    return Err("the dialect is not a JSON object".to_string());
                }
                dialect = Some(parsed);
            }
            "--http" => http = Some(value_of(args, &mut index, arg)?),
            "--token" => token = Some(value_of(args, &mut index, arg)?),
            "--provider" => provider = Some(value_of(args, &mut index, arg)?),
            "--agent" => agent = Some(value_of(args, &mut index, arg)?),
            "--instance" => instance = Some(value_of(args, &mut index, arg)?),
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("there is no option named {other}"));
            }
            other => positional.push(other.to_string()),
        }
        index += 1;
    }

    if is_help(positional.first().map(String::as_str).unwrap_or("")) {
        return Ok(Invocation {
            command: Command::Help,
            json,
            identity: Identity::default(),
        });
    }

    let command = match op.as_str() {
        "status" => Command::Status,
        "read" => Command::Read,
        "misses" => Command::Misses,
        "insert" => Command::Insert {
            text: need(text, positional.first(), "--text", "insert")?,
            language,
            replace,
        },
        "language" => Command::Language {
            language: need(language, positional.first(), "--language", "language")?,
        },
        "candidates" => Command::Candidates {
            text: need(text, positional.first(), "--text", "candidates")?,
            language,
        },
        "predict" => Command::Predict {
            text: need(text, positional.first(), "--text", "predict")?,
            language,
            limit,
        },
        "validate" => Command::Validate {
            text: need(text, positional.first(), "--text", "validate")?,
            language: need(language, None, "--language", "validate")?,
            dialect,
        },
        "curate" => Command::Curate {
            file: PathBuf::from(need(file, positional.first(), "--file", "curate")?),
        },
        "resolve" => Command::Resolve {
            path: need(path, positional.first(), "--path", "resolve")?,
        },
        "scan" => Command::Scan {
            text: need(text, positional.first(), "--text", "scan")?,
        },
        "serve" => Command::Serve {
            pipe,
            http,
            token,
        },
        other => return Err(format!("there is no operation named {other}")),
    };
    Ok(Invocation {
        command,
        json,
        identity: Identity {
            provider: provider.unwrap_or_default(),
            agent: agent.unwrap_or_default(),
            instance: instance.unwrap_or_default(),
        },
    })
}

/// Whether the argument asks for the help.
fn is_help(arg: &str) -> bool {
    arg == "--help" || arg == "-h" || arg == "help"
}

/// The value of an option: the argument that follows it.
fn value_of(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    let next = *index + 1;
    let Some(value) = args.get(next) else {
        return Err(format!("the option {option} needs a value"));
    };
    *index = next;
    Ok(value.clone())
}

/// The value of an operation: the option first, the first argument otherwise.
fn need(
    option_value: Option<String>,
    positional: Option<&String>,
    option: &str,
    op: &str,
) -> Result<String, String> {
    if let Some(value) = option_value.as_ref().filter(|value| !value.is_empty()) {
        return Ok(value.clone());
    }
    if let Some(value) = positional.filter(|value| !value.is_empty()) {
        return Ok(value.clone());
    }
    Err(format!("the operation {op} needs a value ({option} or an argument)"))
}

/// The help of the CLI.
pub fn usage() -> String {
    let mut out = String::new();
    out.push_str("pime-ime - the agent front-end of the IME1.0 input method\n\n");
    out.push_str("usage: pime-ime <op> [arguments] [--json]\n\n");
    out.push_str("operations:\n");
    out.push_str("  status                  the state of the input method and of the core\n");
    out.push_str("  insert <text>           write the text at the caret of the focused document\n");
    out.push_str("                          (--language <lang>, --replace)\n");
    out.push_str("  read                    the text that is selected in the focused document\n");
    out.push_str("  language <language>     switch the language (zh-CN, en-US, ja-JP)\n");
    out.push_str("  candidates <text>       the candidates of the built-in dictionary (--language <lang>)\n");
    out.push_str("  predict <text>          the code candidates of the engine (--language, --limit)\n");
    out.push_str("  validate --text <text>  validate a snippet (--language <lang>, --dialect <json>)\n");
    out.push_str("  curate --file <file>    store the words an AI proposes in the word bank\n");
    out.push_str("  misses                  the words the user typed and that had no candidate\n");
    out.push_str("  resolve <path>          the language of a file\n");
    out.push_str("  scan --text <text>      scan a text for secrets\n");
    out.push_str("  serve                   the local server (--pipe, --http <addr>, --token <token>)\n\n");
    out.push_str("options:\n");
    out.push_str("  --json                  print the raw JSON answer\n");
    out.push_str("  --provider <name>       the provider of this client (openai, anthropic, ...)\n");
    out.push_str("  --agent <name>          the name of the agent that writes (claude-code, cursor, ...)\n");
    out.push_str("  --instance <id>         the instance of this client (it keeps the same mouth of light)\n");
    out.push_str("  --help                  this text\n\n");
    out.push_str("exit codes: 0 = the operation worked, 1 = the operation failed, 2 = the command line is wrong\n");
    out
}

/// Runs the CLI and returns the exit code.
pub fn run(args: &[String], backend: &Backend) -> i32 {
    let invocation = match parse_args(args) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("pime-ime: {message}");
            eprint!("{}", usage());
            return EXIT_USAGE;
        }
    };
    if invocation.command == Command::Help {
        print!("{}", usage());
        return 0;
    }
    // The client says who it is: the input method gives it a mouth of light of its own.
    let backend = backend.with_identity(invocation.identity.clone());
    if let Command::Serve {
        pipe,
        http,
        token,
    } = &invocation.command
    {
        return serve::serve(*pipe, http.as_deref(), token.as_deref(), &backend);
    }
    let (op, params) = match build(&invocation.command) {
        Ok(pair) => pair,
        Err(message) => {
            eprintln!("pime-ime: {message}");
            return 1;
        }
    };
    let answer = dispatch_op(op, &params, &backend);
    let worked = answer.get("ok").and_then(Json::as_bool) == Some(true);
    if invocation.json {
        println!("{}", answer.to_json_string());
    } else if worked {
        println!("{}", human_line(op, &answer));
    } else {
        let code = answer.get("error").and_then(Json::as_str).unwrap_or("ERROR");
        let message = answer.get("message").and_then(Json::as_str).unwrap_or("");
        eprintln!("{code}: {message}");
    }
    if worked { 0 } else { 1 }
}

/// The parameters of a command.
fn build(command: &Command) -> Result<(Op, Json), String> {
    let pairs = match command {
        Command::Help | Command::Serve { .. } => {
            return Err("there is nothing to do".to_string());
        }
        Command::Status => Vec::new(),
        Command::Insert {
            text,
            language,
            replace,
        } => vec![
            ("text", Json::str(text)),
            ("language", language.as_deref().map_or(Json::Null, Json::str)),
            ("replace", Json::Bool(*replace)),
        ],
        Command::Read => Vec::new(),
        Command::Language { language } => vec![("language", Json::str(language))],
        Command::Candidates { text, language } => vec![
            ("text", Json::str(text)),
            ("language", language.as_deref().map_or(Json::Null, Json::str)),
        ],
        Command::Predict {
            text,
            language,
            limit,
        } => vec![
            ("text", Json::str(text)),
            ("language", language.as_deref().map_or(Json::Null, Json::str)),
            ("limit", limit.map_or(Json::Null, Json::int)),
        ],
        Command::Validate {
            text,
            language,
            dialect,
        } => vec![
            ("text", Json::str(text)),
            ("language", Json::str(language)),
            ("dialect", dialect.clone().unwrap_or(Json::Null)),
        ],
        Command::Curate { file } => {
            let proposals = read_proposals(file)?;
            vec![("proposals", proposals)]
        }
        Command::Misses => Vec::new(),
        Command::Resolve { path } => vec![("path", Json::str(path))],
        Command::Scan { text } => vec![("text", Json::str(text))],
    };
    let op = match command {
        Command::Help | Command::Serve { .. } => Op::Status,
        Command::Status => Op::Status,
        Command::Insert { .. } => Op::Insert,
        Command::Read => Op::Read,
        Command::Language { .. } => Op::Language,
        Command::Candidates { .. } => Op::Candidates,
        Command::Predict { .. } => Op::Predict,
        Command::Validate { .. } => Op::Validate,
        Command::Curate { .. } => Op::Curate,
        Command::Misses => Op::Misses,
        Command::Resolve { .. } => Op::Resolve,
        Command::Scan { .. } => Op::Scan,
    };
    Ok((op, Json::obj(pairs)))
}

/// The proposals of a file (a JSON array of objects).
fn read_proposals(file: &std::path::Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(file).map_err(|error| {
        format!(
            "the proposals file could not be read: {}",
            crate::os_detail(&error)
        )
    })?;
    let value = json::parse(text.trim(), Limits::DEFAULT)
        .map_err(|error| format!("the proposals file is not JSON: {error}"))?;
    match value {
        Json::Arr(items) => Ok(Json::Arr(items)),
        _ => Err("the proposals file is not a JSON array".to_string()),
    }
}

/// The short line a human reads.
fn human_line(op: Op, answer: &Json) -> String {
    match op {
        Op::Status => {
            let ime = answer.get("ime");
            let core = answer.get("core");
            format!(
                "ime: {} | core: {}",
                part(ime, "available", "language"),
                part(core, "available", "version")
            )
        }
        Op::Insert => {
            let inserted = answer.get("inserted").and_then(Json::as_i64).unwrap_or(0);
            let language = answer.get("language").and_then(Json::as_str).unwrap_or("");
            if language.is_empty() {
                format!("inserted {inserted}")
            } else {
                format!("inserted {inserted} ({language})")
            }
        }
        Op::Read => {
            let text = answer.get("text").and_then(Json::as_str).unwrap_or("");
            if text.is_empty() {
                "(nothing is selected)".to_string()
            } else {
                text.to_string()
            }
        }
        Op::Language => format!(
            "language: {}",
            answer.get("language").and_then(Json::as_str).unwrap_or("")
        ),
        Op::Candidates | Op::Predict => {
            let list = answer
                .get("candidates")
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(candidate_text)
                        .collect::<Vec<&str>>()
                        .join(" ")
                })
                .unwrap_or_default();
            if list.is_empty() {
                "(no candidate)".to_string()
            } else {
                list
            }
        }
        Op::Validate => {
            let result = answer.get("result");
            let level = result
                .and_then(|r| r.get("level"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let issues = result
                .and_then(|r| r.get("issues"))
                .and_then(Json::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            if issues == 0 {
                format!("valid ({level})")
            } else {
                format!("{issues} issue(s) ({level})")
            }
        }
        Op::Resolve => {
            let decision = answer.get("decision");
            let language = decision
                .and_then(|d| d.get("language"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let reason = decision
                .and_then(|d| d.get("reason"))
                .and_then(Json::as_str)
                .unwrap_or("");
            if reason.is_empty() {
                language.to_string()
            } else {
                format!("{language} ({reason})")
            }
        }
        Op::Scan => {
            let result = answer.get("result");
            let findings = result
                .and_then(|r| r.get("findings"))
                .and_then(Json::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let highest = result
                .and_then(|r| r.get("highest"))
                .and_then(Json::as_str)
                .unwrap_or("");
            if findings == 0 {
                "no secret found".to_string()
            } else {
                format!("{findings} finding(s) (highest: {highest})")
            }
        }
        Op::Curate | Op::Misses => answer
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

/// The text of a candidate: a word of the dictionary, or the code the engine proposes.
fn candidate_text(candidate: &Json) -> Option<&str> {
    match candidate {
        Json::Str(text) => Some(text.as_str()),
        Json::Obj(_) => candidate
            .get("text")
            .and_then(Json::as_str)
            .or_else(|| candidate.get("label").and_then(Json::as_str)),
        _ => None,
    }
}

/// One part of the status line.
fn part(value: Option<&Json>, available: &str, detail: &str) -> String {
    let Some(value) = value else {
        return "unknown".to_string();
    };
    if value.get(available).and_then(Json::as_bool) == Some(true) {
        let extra = value.get(detail).and_then(Json::as_str).unwrap_or("");
        if extra.is_empty() {
            "ready".to_string()
        } else {
            format!("ready ({extra})")
        }
    } else {
        let code = value.get("error").and_then(Json::as_str).unwrap_or("ERROR");
        format!("not available ({code})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_operations_of_the_documentation_are_parsed() {
        let parsed = parse_args(&args(&["status"])).expect("status");
        assert_eq!(parsed.command, Command::Status);
        let parsed = parse_args(&args(&["insert", "hello"])).expect("insert");
        assert_eq!(
            parsed.command,
            Command::Insert {
                text: "hello".to_string(),
                language: None,
                replace: false
            }
        );
        let parsed = parse_args(&args(&["insert", "hello", "--language", "zh-CN"])).expect("insert");
        assert_eq!(
            parsed.command,
            Command::Insert {
                text: "hello".to_string(),
                language: Some("zh-CN".to_string()),
                replace: false
            }
        );
        let parsed = parse_args(&args(&["insert", "replacement", "--replace"])).expect("insert");
        assert_eq!(
            parsed.command,
            Command::Insert {
                text: "replacement".to_string(),
                language: None,
                replace: true
            }
        );
        let parsed = parse_args(&args(&["read"])).expect("read");
        assert_eq!(parsed.command, Command::Read);
        let parsed = parse_args(&args(&["language", "ja-JP"])).expect("language");
        assert_eq!(
            parsed.command,
            Command::Language {
                language: "ja-JP".to_string()
            }
        );
        let parsed = parse_args(&args(&["candidates", "ni", "--language", "zh-CN"])).expect("candidates");
        assert_eq!(
            parsed.command,
            Command::Candidates {
                text: "ni".to_string(),
                language: Some("zh-CN".to_string())
            }
        );
        let parsed = parse_args(&args(&["predict", "fn ", "--language", "rust"])).expect("predict");
        assert_eq!(
            parsed.command,
            Command::Predict {
                text: "fn ".to_string(),
                language: Some("rust".to_string()),
                limit: None
            }
        );
        let parsed = parse_args(&args(&["validate", "--language", "sql", "--text", "SELECT 1"])).expect("validate");
        assert_eq!(
            parsed.command,
            Command::Validate {
                text: "SELECT 1".to_string(),
                language: "sql".to_string(),
                dialect: None
            }
        );
        let parsed = parse_args(&args(&["curate", "--file", "proposals.json"])).expect("curate");
        assert_eq!(
            parsed.command,
            Command::Curate {
                file: PathBuf::from("proposals.json")
            }
        );
        let parsed = parse_args(&args(&["misses"])).expect("misses");
        assert_eq!(parsed.command, Command::Misses);
        let parsed = parse_args(&args(&["resolve", "src/main.rs"])).expect("resolve");
        assert_eq!(
            parsed.command,
            Command::Resolve {
                path: "src/main.rs".to_string()
            }
        );
        let parsed = parse_args(&args(&["scan", "--text", "token=abc"])).expect("scan");
        assert_eq!(
            parsed.command,
            Command::Scan {
                text: "token=abc".to_string()
            }
        );
    }

    #[test]
    fn the_server_is_parsed_with_its_options() {
        let parsed = parse_args(&args(&[
            "serve",
            "--pipe",
            "--http",
            "127.0.0.1:8731",
            "--token",
            "secret",
        ]))
        .expect("serve");
        assert_eq!(
            parsed.command,
            Command::Serve {
                pipe: true,
                http: Some("127.0.0.1:8731".to_string()),
                token: Some("secret".to_string()),
            }
        );
    }

    #[test]
    fn a_wrong_command_line_is_refused() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&args(&["nonsense"])).is_err());
        assert!(parse_args(&args(&["insert"])).is_err());
        assert!(parse_args(&args(&["validate", "--text", "x"])).is_err());
        assert!(parse_args(&args(&["insert", "--language"])).is_err());
        assert!(parse_args(&args(&["insert", "--nonsense"])).is_err());
    }

    #[test]
    fn the_help_is_asked_for_and_lists_the_operations() {
        let parsed = parse_args(&args(&["--help"])).expect("help");
        assert_eq!(parsed.command, Command::Help);
        let parsed = parse_args(&args(&["help"])).expect("help");
        assert_eq!(parsed.command, Command::Help);
        let text = usage();
        for op in Op::all() {
            assert!(text.contains(op.as_str()), "{} is not in the help", op.as_str());
        }
    }

    /// The client names itself on the command line: the input method gives it a mouth of light of its own.
    #[test]
    fn the_client_names_itself_on_the_command_line() {
        let parsed = parse_args(&args(&[
            "insert",
            "hello",
            "--provider",
            "anthropic",
            "--agent",
            "claude-code",
            "--instance",
            "0123456789abcdef",
        ]))
        .expect("insert");
        assert_eq!(parsed.identity.provider, "anthropic");
        assert_eq!(parsed.identity.agent, "claude-code");
        assert_eq!(parsed.identity.instance, "0123456789abcdef");
    }

    /// The instance of the client is the one that is given: the same client keeps the same mouth of light.
    #[test]
    fn the_instance_of_the_client_can_be_given() {
        let parsed = parse_args(&args(&["insert", "hello", "--instance", "keep-me"])).expect("insert");
        assert_eq!(parsed.identity.instance, "keep-me");
        let resolved = parsed.identity.resolve();
        assert_eq!(resolved.instance, "keep-me", "the name of the client is kept");
        assert!(!resolved.provider.is_empty(), "the provider has a default");
        assert!(!resolved.agent.is_empty(), "the agent has a default");
    }

    #[test]
    fn the_json_flag_is_remembered() {
        let parsed = parse_args(&args(&["status", "--json"])).expect("status");
        assert!(parsed.json);
    }

    #[test]
    fn the_parameters_of_the_commands_are_the_parameters_of_the_operations() {
        let (op, params) = build(&Command::Insert {
            text: "hello".to_string(),
            language: Some("zh-CN".to_string()),
            replace: true,
        })
        .expect("insert");
        assert_eq!(op, Op::Insert);
        assert_eq!(params.get("text").and_then(Json::as_str), Some("hello"));
        assert_eq!(params.get("language").and_then(Json::as_str), Some("zh-CN"));
        assert_eq!(params.get("replace"), Some(&Json::Bool(true)));
    }
}
