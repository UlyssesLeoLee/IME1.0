//! The MCP server: JSON-RPC 2.0 over stdio, one JSON object per line (no Content-Length framing).
//!
//! This is the decisive channel: the description of a tool travels into the context of the model, so the tool list
//! says *when* a tool is to be used and not only what it does. The answers are small for the same reason.
//!
//! The tools are declared once, in [`TOOLS`]. The list the server publishes, the schema of every tool and the
//! operation every tool calls are all read from that one declaration, so the server cannot drift away from it.

use crate::{dispatch_op, Backend, Op, VERSION};
use pime_core::json::{self, Json, Limits};
use std::io::{BufRead, BufReader, Write};

/// The name of the server.
pub const SERVER_NAME: &str = "pime-agent";

/// The version of the MCP protocol the server speaks when the client asks for another one.
pub const DEFAULT_PROTOCOL: &str = "2024-11-05";

/// The versions of the protocol the server answers with when the client asks for them.
pub const SUPPORTED_PROTOCOLS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

/// The JSON-RPC error code of a malformed request.
pub const INVALID_REQUEST: i64 = -32600;

/// The JSON-RPC error code of a method the server does not know.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// The JSON-RPC error code of a line that is not JSON.
pub const PARSE_ERROR: i64 = -32700;

/// The text that tells the model what the input method can do. It must stay below 4096 characters.
pub const INSTRUCTIONS: &str = concat!(
    "This server gives you the input method (IME) of the user: a TSF text service that lives inside the focused ",
    "application.\n\n",
    "Use ime_insert_text to write into the window that has the focus - code, Chinese or Japanese - in ONE call. ",
    "It is not a keystroke simulation: the text service writes the text into the document at the caret, so CJK text ",
    "and long code blocks cost one tool call and no simulated keys. With replace=true the current selection is ",
    "overwritten instead of the caret.\n\n",
    "Use ime_read_selection to read what the user has marked, ime_set_language to switch between zh-CN, en-US and ",
    "ja-JP, ime_candidates to see what the built-in dictionary offers for a pinyin or a romaji prefix, ime_predict ",
    "for the code completions of the engine, and ime_validate to check a snippet before it is written.\n\n",
    "ime_curate_dictionary stores the words you propose in the word bank of the user. The storage is written by the ",
    "text service: in this build the tool answers UNSUPPORTED and nothing is stored.\n\n",
    "Everything is local: nothing leaves the machine. When the input method is not running in the focused window the ",
    "answer is NO_IME: ask the user to click into the window he wants written into and try again."
);

/// One parameter of a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    /// The name of the parameter.
    pub name: &'static str,
    /// The JSON type of the parameter.
    pub kind: &'static str,
    /// Whether the parameter has to be there.
    pub required: bool,
    /// What the parameter is for (the model reads it).
    pub description: &'static str,
}

/// One tool of the agent API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tool {
    /// The name of the tool.
    pub name: &'static str,
    /// The operation of the agent API the tool calls.
    pub op: Op,
    /// What the tool does and when it is to be used (the model reads it).
    pub description: &'static str,
    /// The parameters of the tool.
    pub params: &'static [Param],
}

/// The tools of the server. This is the one declaration every list is read from.
pub const TOOLS: [Tool; 7] = [
    Tool {
        name: "ime_insert_text",
        op: Op::Insert,
        description: "Write text into the window that has the focus, in ONE call - no keystroke simulation, so CJK text and long code blocks cost one tool call. Use it to put the code, the Chinese or the Japanese you produced into the editor the user is working in. With replace=true the current selection is overwritten instead of the caret.",
        params: &[
            Param {
                name: "text",
                kind: "string",
                required: true,
                description: "The text to write, as it is.",
            },
            Param {
                name: "language",
                kind: "string",
                required: false,
                description: "The language of the write: zh-CN, en-US or ja-JP.",
            },
            Param {
                name: "replace",
                kind: "boolean",
                required: false,
                description: "Replace the current selection instead of writing at the caret.",
            },
            Param {
                name: "agent",
                kind: "string",
                required: false,
                description: "The name of the sub-agent that writes. Two sub-agents of the same provider are two different speakers: each of them gets a laser mouth of its own colour on the screen of the user.",
            },
        ],
    },
    Tool {
        name: "ime_read_selection",
        op: Op::Read,
        description: "Read the text that is selected in the window that has the focus. Use it to see what the user marked before you change it.",
        params: &[],
    },
    Tool {
        name: "ime_set_language",
        op: Op::Language,
        description: "Switch the language of the input method (zh-CN, en-US, ja-JP). Use it when the user asks for another language, before a write that depends on it.",
        params: &[Param {
            name: "language",
            kind: "string",
            required: true,
            description: "The language to switch to: zh-CN, en-US or ja-JP.",
        }],
    },
    Tool {
        name: "ime_candidates",
        op: Op::Candidates,
        description: "The candidates of the built-in dictionary for a pinyin (ni) or a romaji (konnichiiha) prefix. Use it to offer the Chinese or the Japanese the user meant.",
        params: &[
            Param {
                name: "text",
                kind: "string",
                required: true,
                description: "The pinyin or the romaji that was typed.",
            },
            Param {
                name: "language",
                kind: "string",
                required: false,
                description: "The dictionary to ask: zh-CN (default), en-US or ja-JP.",
            },
        ],
    },
    Tool {
        name: "ime_predict",
        op: Op::Predict,
        description: "The code candidates of the engine for the prefix that was typed. Use it to offer the completions of the input method.",
        params: &[
            Param {
                name: "text",
                kind: "string",
                required: true,
                description: "The prefix that was typed.",
            },
            Param {
                name: "language",
                kind: "string",
                required: false,
                description: "The language of the code (rust, sql, ...).",
            },
            Param {
                name: "limit",
                kind: "integer",
                required: false,
                description: "How many candidates to answer with.",
            },
        ],
    },
    Tool {
        name: "ime_validate",
        op: Op::Validate,
        description: "Validate a code snippet with the engine and report the issues it found. Use it before you write code into the focused window.",
        params: &[
            Param {
                name: "text",
                kind: "string",
                required: true,
                description: "The snippet to validate.",
            },
            Param {
                name: "language",
                kind: "string",
                required: true,
                description: "The language of the snippet (rust, sql, ...).",
            },
            Param {
                name: "dialect",
                kind: "object",
                required: false,
                description: "The dialect of the snippet, as an object of strings.",
            },
        ],
    },
    Tool {
        name: "ime_curate_dictionary",
        op: Op::Curate,
        description: "Store the words you propose in the word bank of the user, so the input method offers them next time. The storage is written by the text service: in this build the tool answers UNSUPPORTED and nothing is stored.",
        params: &[Param {
            name: "proposals",
            kind: "array",
            required: true,
            description: "The words to store: objects with a word, a reading and a reason.",
        }],
    },
];

/// The names of the tools, read from the one declaration.
pub fn tool_names() -> Vec<&'static str> {
    TOOLS.iter().map(|tool| tool.name).collect()
}

/// The JSON schema of a tool, built from its parameters.
pub fn tool_schema(tool: &Tool) -> Json {
    let properties: Vec<(String, Json)> = tool
        .params
        .iter()
        .map(|param| {
            (
                param.name.to_string(),
                Json::obj(vec![
                    ("type", Json::str(param.kind)),
                    ("description", Json::str(param.description)),
                ]),
            )
        })
        .collect();
    let required: Vec<Json> = tool
        .params
        .iter()
        .filter(|param| param.required)
        .map(|param| Json::str(param.name))
        .collect();
    Json::obj(vec![
        ("type", Json::str("object")),
        ("properties", Json::Obj(properties)),
        ("required", Json::Arr(required)),
        ("additionalProperties", Json::Bool(false)),
    ])
}

/// The list of the tools, as `tools/list` answers it.
pub fn tool_list_json() -> Json {
    let tools: Vec<Json> = TOOLS
        .iter()
        .map(|tool| {
            Json::obj(vec![
                ("name", Json::str(tool.name)),
                ("description", Json::str(tool.description)),
                ("inputSchema", tool_schema(tool)),
            ])
        })
        .collect();
    Json::obj(vec![("tools", Json::Arr(tools))])
}

/// The answer of one request line. `None` when the line is a notification (it has no answer).
pub fn handle_line(line: &str, backend: &Backend) -> Option<String> {
    // a host that writes a byte order mark must not break the first line
    let line = line.trim_start_matches('\u{feff}');
    let value = match json::parse(line, Limits::DEFAULT) {
        Ok(value) => value,
        Err(error) => {
            let answer = error_response(
                None,
                PARSE_ERROR,
                &format!("Parse error: the line is not valid JSON ({error})"),
            );
            return Some(answer.to_json_string());
        }
    };
    handle_request(&value, backend)
}

/// The answer of one request object. `None` when the request is a notification (it has no answer).
pub fn handle_request(value: &Json, backend: &Backend) -> Option<String> {
    if value.as_object().is_none() {
        let answer = error_response(
            None,
            INVALID_REQUEST,
            "Invalid Request: the request is not a JSON object",
        );
        return Some(answer.to_json_string());
    }
    let id = value.get("id");
    let method = value.get("method").and_then(Json::as_str).unwrap_or("");
    if method.is_empty() {
        let answer = error_response(
            id,
            INVALID_REQUEST,
            "Invalid Request: the method is missing",
        );
        return Some(answer.to_json_string());
    }
    if method.starts_with("notifications/") || id.is_none() {
        // a notification has no answer
        return None;
    }
    let params = value.get("params").cloned().unwrap_or(Json::Null);
    let answer = match method {
        "initialize" => response(id, initialize_result(&params, backend)),
        "tools/list" => response(id, tool_list_json()),
        "tools/call" => response(id, tools_call(&params, backend)),
        "resources/list" => response(
            id,
            Json::obj(vec![("resources", Json::Arr(Vec::new()))]),
        ),
        "prompts/list" => response(id, Json::obj(vec![("prompts", Json::Arr(Vec::new()))])),
        "ping" => response(id, Json::Obj(Vec::new())),
        _ => error_response(
            id,
            METHOD_NOT_FOUND,
            &format!("Method not found: there is no method named {method}"),
        ),
    };
    Some(answer.to_json_string())
}

/// The answer of `initialize`.
///
/// The name the client gives in `clientInfo` is the name of the agent that writes: the input method gives that
/// agent a mouth of light of its own, and the user sees who is writing.
fn initialize_result(params: &Json, backend: &Backend) -> Json {
    if let Some(name) = params
        .get("clientInfo")
        .and_then(|info| info.get("name"))
        .and_then(Json::as_str)
        .filter(|name| !name.is_empty())
    {
        backend.set_client_name(name);
    }
    let asked = params.get("protocolVersion").and_then(Json::as_str).unwrap_or("");
    let version = if SUPPORTED_PROTOCOLS.contains(&asked) {
        asked
    } else {
        DEFAULT_PROTOCOL
    };
    Json::obj(vec![
        ("protocolVersion", Json::str(version)),
        (
            "capabilities",
            Json::obj(vec![
                ("tools", Json::Obj(Vec::new())),
                ("resources", Json::Obj(Vec::new())),
            ]),
        ),
        (
            "serverInfo",
            Json::obj(vec![
                ("name", Json::str(SERVER_NAME)),
                ("version", Json::str(VERSION)),
            ]),
        ),
        ("instructions", Json::str(INSTRUCTIONS)),
    ])
}

/// The answer of `tools/call`: the tool runs and its answer is the content.
fn tools_call(params: &Json, backend: &Backend) -> Json {
    let name = params.get("name").and_then(Json::as_str).unwrap_or("");
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        let answer = Json::obj(vec![
            ("ok", Json::Bool(false)),
            ("error", Json::str("UNSUPPORTED")),
            ("message", Json::str(&format!("there is no tool named {name}"))),
        ]);
        return tool_result(&answer, true);
    };
    let arguments = match params.get("arguments") {
        None | Some(Json::Null) => Json::Obj(Vec::new()),
        Some(value) => value.clone(),
    };
    let answer = dispatch_op(tool.op, &arguments, backend);
    let failed = answer.get("ok").and_then(Json::as_bool) == Some(false);
    tool_result(&answer, failed)
}

/// The answer of a tool: the compact JSON answer is the text, the answer itself is the structure.
fn tool_result(answer: &Json, failed: bool) -> Json {
    Json::obj(vec![
        (
            "content",
            Json::Arr(vec![Json::obj(vec![
                ("type", Json::str("text")),
                ("text", Json::str(&answer.to_json_string())),
            ])]),
        ),
        ("isError", Json::Bool(failed)),
        ("structuredContent", answer.clone()),
    ])
}

/// The answer of a JSON-RPC request.
fn response(id: Option<&Json>, result: Json) -> Json {
    let mut pairs = vec![("jsonrpc", Json::str("2.0"))];
    if let Some(id) = id {
        pairs.push(("id", id.clone()));
    }
    pairs.push(("result", result));
    Json::obj(pairs)
}

/// The answer of a JSON-RPC request that could not be answered.
fn error_response(id: Option<&Json>, code: i64, message: &str) -> Json {
    let mut pairs = vec![("jsonrpc", Json::str("2.0"))];
    pairs.push((
        "id",
        id.cloned().unwrap_or(Json::Null),
    ));
    pairs.push((
        "error",
        Json::obj(vec![
            ("code", Json::int(code)),
            ("message", Json::str(message)),
        ]),
    ));
    Json::obj(pairs)
}

/// Serves the MCP protocol on stdin and stdout until the input ends.
pub fn run_stdio(backend: &Backend) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            return Ok(());
        }
        // a host that writes a byte order mark (some shells do) must not break the first line
        let bytes = strip_bom(&buffer);
        let answer = match std::str::from_utf8(bytes) {
            Ok(text) => {
                let request = text.trim_end_matches(['\r', '\n']);
                if request.trim().is_empty() {
                    None
                } else {
                    handle_line(request, backend)
                }
            }
            Err(_) => Some(
                error_response(
                    None,
                    PARSE_ERROR,
                    "Parse error: the line is not valid UTF-8",
                )
                .to_json_string(),
            ),
        };
        if let Some(answer) = answer {
            output.write_all(answer.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

/// The byte order mark of the beginning of a stream, if there is one.
fn strip_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::FakeBackend;

    fn backend() -> FakeBackend {
        FakeBackend::new()
    }

    fn call(request: &str, backend: &FakeBackend) -> Option<Json> {
        handle_line(request, backend.backend())
            .map(|line| json::parse(&line, Limits::DEFAULT).expect("the answer is JSON"))
    }

    #[test]
    fn initialize_answers_the_protocol_the_capabilities_and_the_instructions() {
        let backend = backend();
        let answer = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            &backend,
        )
        .expect("an answer");
        let result = answer.get("result").expect("the result");
        assert_eq!(answer.get("jsonrpc").and_then(Json::as_str), Some("2.0"));
        assert_eq!(answer.get("id").and_then(Json::as_i64), Some(1));
        assert_eq!(
            result.get("protocolVersion").and_then(Json::as_str),
            Some("2025-06-18")
        );
        let capabilities = result.get("capabilities").expect("the capabilities");
        assert!(capabilities.get("tools").is_some());
        assert!(capabilities.get("resources").is_some());
        let info = result.get("serverInfo").expect("the name of the server");
        assert_eq!(info.get("name").and_then(Json::as_str), Some(SERVER_NAME));
        assert_eq!(info.get("version").and_then(Json::as_str), Some(VERSION));
        let instructions = result
            .get("instructions")
            .and_then(Json::as_str)
            .expect("the instructions");
        assert!(!instructions.is_empty());
        assert!(instructions.len() <= 4096, "the instructions are too long");
    }

    #[test]
    fn an_unknown_protocol_version_falls_back_to_the_one_of_the_server() {
        let backend = backend();
        let answer = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
            &backend,
        )
        .expect("an answer");
        assert_eq!(
            answer
                .get("result")
                .and_then(|r| r.get("protocolVersion"))
                .and_then(Json::as_str),
            Some(DEFAULT_PROTOCOL)
        );
    }

    #[test]
    fn a_notification_has_no_answer() {
        let backend = backend();
        assert!(call(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, &backend).is_none());
        assert!(call(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#, &backend).is_none());
    }

    #[test]
    fn an_unknown_method_is_a_json_rpc_error() {
        let backend = backend();
        let answer = call(r#"{"jsonrpc":"2.0","id":7,"method":"nope"}"#, &backend).expect("an answer");
        let error = answer.get("error").expect("the error");
        assert_eq!(error.get("code").and_then(Json::as_i64), Some(METHOD_NOT_FOUND));
        assert_eq!(answer.get("id").and_then(Json::as_i64), Some(7));
    }

    #[test]
    fn a_line_that_is_not_json_is_a_parse_error() {
        let backend = backend();
        let answer = call("not json", &backend).expect("an answer");
        let error = answer.get("error").expect("the error");
        assert_eq!(error.get("code").and_then(Json::as_i64), Some(PARSE_ERROR));
    }

    #[test]
    fn the_empty_methods_exist() {
        let backend = backend();
        let answer = call(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#, &backend).expect("an answer");
        assert_eq!(
            answer
                .get("result")
                .and_then(|r| r.get("resources"))
                .and_then(Json::as_array)
                .map(Vec::len),
            Some(0)
        );
        let answer = call(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list"}"#, &backend).expect("an answer");
        assert_eq!(
            answer
                .get("result")
                .and_then(|r| r.get("prompts"))
                .and_then(Json::as_array)
                .map(Vec::len),
            Some(0)
        );
        let answer = call(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, &backend).expect("an answer");
        assert!(answer.get("result").is_some());
    }

    /// The drift test: the list the server publishes is the list of the one declaration, and the names are unique.
    /// The name the client gives in `initialize` is the name of the agent that writes.
    #[test]
    fn the_name_of_the_client_becomes_the_name_of_the_agent() {
        let backend = backend();
        let _ = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"claude-code"}}}"#,
            &backend,
        );
        let _ = call(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ime_insert_text","arguments":{"text":"hello"}}}"#,
            &backend,
        );
        let request = backend.last_request().expect("the bridge was asked");
        assert_eq!(request.source.agent, "claude-code");
        assert!(
            !request.source.instance.is_empty(),
            "the client has an instance of its own"
        );
    }

    /// The parameter `agent` names the sub-agent that writes: two sub-agents of the same provider are two different
    /// sources, and therefore two different mouths of light.
    #[test]
    fn the_parameter_agent_names_the_sub_agent_that_writes() {
        let backend = backend();
        let _ = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ime_insert_text","arguments":{"text":"hello","agent":"planner"}}}"#,
            &backend,
        );
        let with_name = backend.last_request().expect("the bridge was asked");
        assert_eq!(with_name.source.agent, "planner");
        let _ = call(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ime_insert_text","arguments":{"text":"hello"}}}"#,
            &backend,
        );
        let without_name = backend.last_request().expect("the bridge was asked");
        assert_ne!(
            with_name.source.key(),
            without_name.source.key(),
            "the sub-agent and the process are two different speakers"
        );
    }

    #[test]
    fn the_published_tool_list_is_the_declared_one_and_the_names_are_unique() {
        let published = tool_list_json();
        let list = published
            .get("tools")
            .and_then(Json::as_array)
            .expect("the list of the tools");
        let names: Vec<&str> = list
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Json::as_str))
            .collect();
        let declared: Vec<&str> = tool_names();
        assert_eq!(names, declared, "the published list drifted from the declaration");
        assert_eq!(names.len(), TOOLS.len());
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "a tool name is used twice");
    }

    #[test]
    fn every_tool_has_a_name_a_description_and_a_schema_that_names_its_parameters() {
        let published = tool_list_json();
        let list = published
            .get("tools")
            .and_then(Json::as_array)
            .expect("the list of the tools");
        assert_eq!(list.len(), 7);
        for tool in list {
            let name = tool.get("name").and_then(Json::as_str).expect("the name");
            let description = tool
                .get("description")
                .and_then(Json::as_str)
                .expect("the description");
            assert!(!name.is_empty());
            assert!(description.len() > 20, "{name} has no real description");
            let schema = tool.get("inputSchema").expect("the schema");
            assert_eq!(schema.get("type").and_then(Json::as_str), Some("object"));
            let properties = schema.get("properties").expect("the properties");
            let required = schema.get("required").and_then(Json::as_array).expect("the required");
            let declaration = TOOLS
                .iter()
                .find(|candidate| candidate.name == name)
                .expect("the tool is declared");
            for param in declaration.params {
                assert!(
                    properties.get(param.name).is_some(),
                    "{name} does not declare {}",
                    param.name
                );
                if param.required {
                    assert!(
                        required.iter().any(|r| r.as_str() == Some(param.name)),
                        "{name} does not require {}",
                        param.name
                    );
                }
            }
            let required_names: Vec<&str> = required.iter().filter_map(|r| r.as_str()).collect();
            let expected: Vec<&str> = declaration
                .params
                .iter()
                .filter(|param| param.required)
                .map(|param| param.name)
                .collect();
            assert_eq!(required_names, expected, "{name} requires the wrong parameters");
        }
    }

    #[test]
    fn a_tool_call_answers_with_the_content_and_the_structure() {
        let backend = backend();
        let answer = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ime_candidates","arguments":{"text":"ni"}}}"#,
            &backend,
        )
        .expect("an answer");
        let result = answer.get("result").expect("the result");
        assert_eq!(result.get("isError"), Some(&Json::Bool(false)));
        let content = result.get("content").and_then(Json::as_array).expect("the content");
        let text = content[0].get("text").and_then(Json::as_str).expect("the text");
        assert!(text.contains("candidates"), "{text}");
        let structured = result.get("structuredContent").expect("the structure");
        assert_eq!(structured.get("ok"), Some(&Json::Bool(true)));
    }

    #[test]
    fn an_unknown_tool_is_a_tool_error() {
        let backend = backend();
        let answer = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ime_nope"}}"#,
            &backend,
        )
        .expect("an answer");
        let result = answer.get("result").expect("the result");
        assert_eq!(result.get("isError"), Some(&Json::Bool(true)));
        let content = result.get("content").and_then(Json::as_array).expect("the content");
        let text = content[0].get("text").and_then(Json::as_str).expect("the text");
        assert!(text.contains("UNSUPPORTED"), "{text}");
    }

    #[test]
    fn a_tool_that_fails_answers_with_is_error() {
        let backend = backend();
        let answer = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ime_insert_text","arguments":{}}}"#,
            &backend,
        )
        .expect("an answer");
        let result = answer.get("result").expect("the result");
        assert_eq!(result.get("isError"), Some(&Json::Bool(true)));
    }

    #[test]
    fn the_byte_order_mark_of_a_host_does_not_break_the_first_line() {
        assert_eq!(strip_bom(b"\xef\xbb\xbf{}"), b"{}");
        assert_eq!(strip_bom(b"{}"), b"{}");
        let backend = backend();
        let answer = call(
            "\u{feff}{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}",
            &backend,
        )
        .expect("an answer");
        assert_eq!(answer.get("id").and_then(Json::as_i64), Some(1));
    }
}
