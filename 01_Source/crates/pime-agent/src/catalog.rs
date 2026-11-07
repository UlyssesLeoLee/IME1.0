//! The catalogue of the standard assets, seen from the agent.
//!
//! The catalogue is a **local** thing: it lives in the files of the repository, not in the text service of the
//! input method. The tools that read it are therefore **not** protocol operations: they are answered here, from the
//! catalogue on the disk, and they never touch the focused window.
//!
//! Where the catalogue is: the environment variable `PIME_TASE_CATALOG` points at it; otherwise a few places next to
//! the program are tried (the development layout and the place an installed copy would sit). When it is not there
//! (the installed program does not carry the catalogue yet), the tools answer with a clear message instead of
//! pretending that the catalogue is empty.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pime_core::json::Json;
use tase_catalog_mcp::CatalogAdapter;

/// The prefix of the names of the tools that come from the catalogue.
pub const TOOL_PREFIX: &str = "catalog.";

/// The places the catalogue may be, in the order they are tried.
fn candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("PIME_TASE_CATALOG")
        && !dir.is_empty()
    {
        out.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        // next to the program, and a few levels up (dist/, the crate, the repository)
        for _ in 0..5 {
            let Some(current) = dir.as_ref() else {
                break;
            };
            out.push(current.join("assets").join("tase-catalog"));
            out.push(current.join("01_Source").join("assets").join("tase-catalog"));
            dir = current.parent().map(Path::to_path_buf);
        }
    }
    // the development layout: the crate of the agent sits in 01_Source/crates/pime-agent, the catalogue in
    // 01_Source/assets/tase-catalog
    out.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("tase-catalog"),
    );
    out
}

/// The catalogue, loaded once (it does not change while the program runs).
pub fn adapter() -> Option<&'static CatalogAdapter> {
    static ADAPTER: OnceLock<Option<CatalogAdapter>> = OnceLock::new();
    ADAPTER
        .get_or_init(|| {
            for dir in candidates() {
                if dir.is_dir()
                    && let Ok(adapter) = CatalogAdapter::load(&dir)
                {
                    return Some(adapter);
                }
            }
            None
        })
        .as_ref()
}

/// Whether the catalogue is on this machine.
pub fn available() -> bool {
    adapter().is_some()
}

/// The tools of the catalogue, as `tools/list` shows them.
///
/// Empty when the catalogue is not on this machine: the list of the tools stays the one of the input method, and the
/// model is told in the instructions that the catalogue is not there.
pub fn tool_list() -> Vec<Json> {
    let Some(adapter) = adapter() else {
        return Vec::new();
    };
    adapter
        .as_mcp_tool_list()
        .get("tools")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The answer of a tool of the catalogue, in the shape of MCP (`content` / `isError` / `structuredContent`).
///
/// `None` when the tool is not one of the catalogue.
pub fn call(name: &str, arguments: &Json) -> Option<Json> {
    if !name.starts_with(TOOL_PREFIX) {
        return None;
    }
    let Some(adapter) = adapter() else {
        let answer = Json::obj(vec![
            ("ok", Json::Bool(false)),
            ("error", Json::str("NOT_FOUND")),
            (
                "message",
                Json::str(
                    "the catalogue of the standard assets is not on this machine: set PIME_TASE_CATALOG to the \
                     directory that holds catalog.json",
                ),
            ),
        ]);
        return Some(result_of(&answer, true));
    };
    let params = Json::obj(vec![
        ("name", Json::str(name)),
        ("arguments", arguments.clone()),
    ]);
    Some(adapter.call_as_mcp("tools/call", &params))
}

/// The instructions of the catalogue, added to the ones of the server.
pub fn instructions() -> Option<String> {
    let adapter = adapter()?;
    Some(adapter.instructions().to_string())
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

    /// The tools of the catalogue are only published when the catalogue is there.
    #[test]
    fn the_tools_of_the_catalogue_are_published_when_it_is_there() {
        // the catalogue of the repository is next to the crate of the agent
        assert!(
            available(),
            "the catalogue of the repository must be found from the crate of the agent"
        );
        let tools = tool_list();
        assert_eq!(tools.len(), 4, "the catalogue has four tools");
        let names: Vec<String> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Json::as_str))
            .map(|name| name.to_string())
            .collect();
        for expected in [
            "catalog.search",
            "catalog.recommend",
            "catalog.instantiate",
            "catalog.describe",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "the tool {expected} is missing: {names:?}"
            );
        }
    }

    /// A tool of the catalogue answers with the shape of MCP.
    #[test]
    fn a_tool_of_the_catalogue_answers_in_the_shape_of_mcp() {
        let arguments = Json::obj(vec![
            ("query", Json::str("最短路径")),
            ("language", Json::str("rust")),
        ]);
        let answer = call("catalog.search", &arguments).expect("the catalogue answers");
        assert_eq!(
            answer.get("isError").and_then(Json::as_bool),
            Some(false),
            "the search must not fail: {}",
            answer.to_json_string()
        );
        assert!(
            answer.get("content").and_then(Json::as_array).is_some(),
            "the answer carries the content"
        );
        let structured = answer
            .get("structuredContent")
            .and_then(|value| value.get("count"))
            .and_then(Json::as_i64)
            .unwrap_or(0);
        assert!(structured > 0, "the catalogue has assets: {structured}");
    }

    /// A tool of the input method is not one of the catalogue.
    #[test]
    fn a_tool_of_the_input_method_is_not_one_of_the_catalogue() {
        assert!(call("ime_insert_text", &Json::Obj(Vec::new())).is_none());
    }
}
