//! The tools that talk to the Hub of TASE: the client hands a job to the Hub, the Hub queues it, a worker
//! computes it, and the client comes back for the answer.
//!
//! These tools are **not** protocol operations of the input method: they do not go through the bridge of the text
//! service and they never touch the focused window. They speak the pipe of the logon session (the frames of
//! `tase-local-ipc`), the same pipe the Hub listens on.
//!
//! # Where the pipe comes from
//!
//! The Hub of a logon session listens on `\\.\pipe\PIME-IPC-1-<logon sid>`. **A client cannot read its own logon
//! SID** (the transport of TASE reads the token of the user, not the logon SID of the session: defect D-48), so
//! the SID is a configuration: the `logon_sid` argument of the tool, or the variable `TASE_LOGON_SID`. When
//! neither is there, the tools answer with a message that says what to do - they never guess a name.
//!
//! # What the worker does with the job
//!
//! The worker computes the **diff** of the change and reports it back. It does **not** write the window of the
//! user: that is the job of `tase_apply`, which the client calls with the diff it got here.

use std::time::Duration;

use pime_core::json::Json;
use tase_local_ipc::{Timeouts, WindowsPipe};

use crate::hub_client::{CLIENT_CAPABILITIES, HubLink, JobState, logon_sid_of};

/// The prefix of the names of the tools that talk to the Hub.
pub const TOOL_PREFIX: &str = "tase_";

/// The name of the tool that hands a job to the Hub.
pub const SUBMIT: &str = "tase_submit";

/// The name of the tool that reads the state of a job.
pub const RESULT: &str = "tase_result";

/// The name of the variable that holds the logon SID of the session of the Hub.
pub const LOGON_SID_VARIABLE: &str = crate::hub_client::LOGON_SID_VARIABLE;

/// The deadlines of the pipe: the Hub answers at once, and a client does not wait for a minute for it.
fn timeouts() -> Timeouts {
    Timeouts {
        handshake: Duration::from_secs(2),
        request: Duration::from_secs(10),
        write: Duration::from_secs(2),
        read: Duration::from_secs(2),
    }
}

/// The tools of the Hub, as `tools/list` shows them.
pub fn tool_list() -> Vec<Json> {
    vec![
        Json::obj(vec![
            ("name", Json::str(SUBMIT)),
            (
                "description",
                Json::str(
                    "Hand a job to the Local Agent Hub of TASE: the Hub queues it, a worker computes the diff of \
                     the change, and tase_result reads the answer back. The worker does NOT write the window of \
                     the user - apply the diff with tase_apply. The Hub of the logon session has to run, and the \
                     logon SID of its session has to be known (the logon_sid argument, or the TASE_LOGON_SID \
                     variable).",
                ),
            ),
            (
                "inputSchema",
                Json::obj(vec![
                    ("type", Json::str("object")),
                    ("required", Json::Arr(Vec::new())),
                    (
                        "properties",
                        Json::Obj(vec![
                            ("documents".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("array")),
                                    (
                                        "description",
                                        Json::str(
                                            "the documents the job works on: the path, the base version, the \
                                             text before and the text after",
                                        ),
                                    ),
                                ]),
                            ),
                            ("job_type".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("string")),
                                    (
                                        "description",
                                        Json::str("what the job computes: preview, prepare, verify or commit"),
                                    ),
                                ]),
                            ),
                            ("plan_digest".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("string")),
                                    (
                                        "description",
                                        Json::str("the digest of the plan the job runs"),
                                    ),
                                ]),
                            ),
                            ("workspace_id".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("string")),
                                    (
                                        "description",
                                        Json::str("the workspace the job works in"),
                                    ),
                                ]),
                            ),
                            ("logon_sid".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("string")),
                                    (
                                        "description",
                                        Json::str(
                                            "the logon SID of the session of the Hub; the variable \
                                             TASE_LOGON_SID is the default",
                                        ),
                                    ),
                                ]),
                            ),
                        ]),
                    ),
                ]),
            ),
        ]),
        Json::obj(vec![
            ("name", Json::str(RESULT)),
            (
                "description",
                Json::str(
                    "Read the state of a job of the Local Agent Hub and, when it is done, its result (the diff of \
                     the change). Call it again while the state is QUEUED or RUNNING.",
                ),
            ),
            (
                "inputSchema",
                Json::obj(vec![
                    ("type", Json::str("object")),
                    ("required", Json::Arr(Vec::new())),
                    (
                        "properties",
                        Json::Obj(vec![
                            ("job_id".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("string")),
                                    ("description", Json::str("the job the answer is about")),
                                ]),
                            ),
                            ("logon_sid".to_string(),
                                Json::obj(vec![
                                    ("type", Json::str("string")),
                                    (
                                        "description",
                                        Json::str(
                                            "the logon SID of the session of the Hub; the variable \
                                             TASE_LOGON_SID is the default",
                                        ),
                                    ),
                                ]),
                            ),
                        ]),
                    ),
                ]),
            ),
        ]),
    ]
}

/// The answer of a tool of the Hub, in the shape of MCP (`content` / `isError` / `structuredContent`).
///
/// `None` when the tool is not one of the Hub.
pub fn call(name: &str, arguments: &Json) -> Option<Json> {
    if name != SUBMIT && name != RESULT {
        return None;
    }
    let answer = match name {
        SUBMIT => submit(arguments),
        _ => result(arguments),
    };
    let failed = answer.get("ok").and_then(Json::as_bool) == Some(false);
    Some(result_of(&answer, failed))
}

/// Hands a job to the Hub.
fn submit(arguments: &Json) -> Json {
    let Some(sid) = sid_of(arguments) else {
        return missing_sid();
    };
    let Ok(mut link) = connect(&sid) else {
        return missing_hub();
    };
    if let Err(error) = link.handshake() {
        return failure(&error);
    }
    let workspace = arguments
        .get("workspace_id")
        .and_then(Json::as_str)
        .unwrap_or("ws_default");
    if let Err(error) = link.register("pime-agent", workspace) {
        return failure(&error);
    }
    let mut params = Json::Obj(Vec::new());
    if let Json::Obj(pairs) = &mut params {
        for (key, value) in [
            (
                "idempotency_key",
                arguments
                    .get("idempotency_key")
                    .and_then(Json::as_str)
                    .map(Json::str)
                    .unwrap_or_else(|| Json::str("pime-agent-job")),
            ),
            ("job_type",
                arguments
                    .get("job_type")
                    .and_then(Json::as_str)
                    .map(Json::str)
                    .unwrap_or_else(|| Json::str("preview")),
            ),
            ("plan_digest",
                arguments
                    .get("plan_digest")
                    .and_then(Json::as_str)
                    .map(Json::str)
                    .unwrap_or_else(|| Json::str("")),
            ),
            ("documents",
                arguments
                    .get("documents")
                    .cloned()
                    .unwrap_or_else(|| Json::Arr(Vec::new())),
            ),
        ] {
            pairs.push((key.to_string(), value));
        }
    }
    match link.submit(&params) {
        Ok(job) => Json::obj(vec![
            ("ok", Json::Bool(true)),
            ("job_id", Json::str(&job.job_id)),
            ("status", Json::str(&job.status)),
            ("deduplicated", Json::Bool(job.deduplicated)),
        ]),
        Err(error) => failure(&error),
    }
}

/// Reads the state of a job, and its result when it is done.
fn result(arguments: &Json) -> Json {
    let Some(sid) = sid_of(arguments) else {
        return missing_sid();
    };
    let Some(job_id) = arguments.get("job_id").and_then(Json::as_str) else {
        return Json::obj(vec![
            ("ok", Json::Bool(false)),
            ("error", Json::str("INVALID_ARGUMENT")),
            ("message", Json::str("the tool needs the job_id of the job")),
        ]);
    };
    let Ok(mut link) = connect(&sid) else {
        return missing_hub();
    };
    if let Err(error) = link.handshake() {
        return failure(&error);
    }
    match link.state(job_id) {
        Ok(state) => state_of(&state),
        Err(error) => failure(&error),
    }
}

/// The state of a job, in the shape of the answer.
fn state_of(state: &JobState) -> Json {
    let mut pairs: Vec<(String, Json)> = vec![
        ("ok".to_string(), Json::Bool(true)),
        ("status".to_string(), Json::str(&state.status)),
    ];
    if let Some(result) = &state.result {
        pairs.push(("result".to_string(), result.clone()));
    }
    Json::Obj(pairs)
}

/// The logon SID of the session of the Hub: the argument of the tool, or the variable.
fn sid_of(arguments: &Json) -> Option<String> {
    let argument = arguments.get("logon_sid").and_then(Json::as_str);
    let variable = std::env::var(LOGON_SID_VARIABLE).ok();
    logon_sid_of(argument, None, variable.as_deref()).ok()
}

/// Connects to the pipe of the Hub.
#[cfg(windows)]
fn connect(logon_sid: &str) -> Result<HubLink<WindowsPipe>, String> {
    let transport = WindowsPipe::connect(logon_sid, Duration::from_secs(2))
        .map_err(|error| format!("the pipe of the hub is not there: {error}"))?;
    Ok(HubLink::new(
        transport,
        "pime-agent",
        &CLIENT_CAPABILITIES,
        timeouts(),
    ))
}

/// Connects to the pipe of the Hub: the pipe is a Windows thing.
#[cfg(not(windows))]
fn connect(_logon_sid: &str) -> Result<HubLink<WindowsPipe>, String> {
    Err("the pipe of the hub of TASE is a Windows thing".to_string())
}

/// The answer when the logon SID of the session of the Hub is not known.
fn missing_sid() -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str("INVALID_ARGUMENT")),
        (
            "message",
            Json::str(&format!(
                "the client needs the logon SID of the session of the Hub: give the logon_sid argument, or set \
                 the {LOGON_SID_VARIABLE} variable"
            )),
        ),
    ])
}

/// The answer when the Hub is not there.
fn missing_hub() -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str("WORKER_UNAVAILABLE")),
        (
            "message",
            Json::str(
                "there is no Hub on the pipe of this session: start the Hub of the session, or give the right \
                 logon SID",
            ),
        ),
    ])
}

/// The answer when the Hub refused.
fn failure(error: &tase_contract::TaseError) -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str(error.code.as_str())),
        ("message", Json::str(&error.message)),
    ])
}

/// The answer of a tool: the compact JSON answer is the text, the answer itself is the structure.
fn result_of(answer: &Json, failed: bool) -> Json {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_tools_are_declared() {
        let names: Vec<String> = tool_list()
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Json::as_str).map(str::to_string))
            .collect();
        assert_eq!(names, vec![SUBMIT.to_string(), RESULT.to_string()]);
    }

    #[test]
    fn a_tool_of_another_family_is_not_ours() {
        assert!(call("ime_insert_text", &Json::Obj(Vec::new())).is_none());
        assert!(call("catalog.search", &Json::Obj(Vec::new())).is_none());
    }

    #[test]
    fn the_answer_without_the_logon_sid_says_what_to_do() {
        // the variable is not set in the tests
        let answer = call(SUBMIT, &Json::Obj(Vec::new())).expect("the tool answers");
        let failed = answer.get("isError").and_then(Json::as_bool) == Some(true);
        assert!(failed, "the tool could not do anything");
        let text = answer
            .get("content")
            .and_then(Json::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("text"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(text.contains("logon SID"), "{text}");
        assert!(text.contains(LOGON_SID_VARIABLE), "{text}");
    }

    #[test]
    fn the_result_tool_needs_the_id_of_the_job() {
        let answer = call(RESULT, &Json::Obj(Vec::new())).expect("the tool answers");
        let failed = answer.get("isError").and_then(Json::as_bool) == Some(true);
        assert!(failed, "the tool could not do anything");
    }
}
