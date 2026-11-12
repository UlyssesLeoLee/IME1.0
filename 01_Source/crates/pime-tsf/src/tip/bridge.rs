//! The AI bridge, on the side of the input method: what an agent asks the text service to do.
//!
//! The window itself (the `WM_COPYDATA` channel between the processes) lives in [`crate::agent_bridge`]; this
//! module is the host side of it: the requests that arrive there are answered here, on the thread of the host, where
//! the document, the caret and the selection live. The answers follow the rules of the product: nothing is ever
//! written into a password field, and a write that the host refuses is reported as refused instead of being retried
//! behind the user's back.

use std::mem::ManuallyDrop;

use windows::Win32::UI::TextServices::TF_ANCHOR_END;

use pime_core::agent_proto::{Error as BridgeError, Op as BridgeOp, Reply as BridgeReply, Request as BridgeRequest};

use crate::controller::{HostError, HostText};

use super::{Inner, SYNC_READ, SYNC_WRITE, TsfHost, caret_range, refused, run_session, selection_of};

/// The largest selection that is read for an AI agent (in UTF-16 units). A selection of a megabyte is not something to
/// hand to an agent: it is far more likely a mistake than an intention.
const MAX_SELECTION_CHARS: usize = 1 << 20;

/// How much of the text before the caret is read for the demonstration (in UTF-16 units).
///
/// The demonstration asks whether the document holds a domain header of its own: only the piece it writes
/// into matters, and a document may be long.
const BEFORE_CARET_CHARS: usize = 4096;

/// What an AI agent asks the input method to do, answered on the thread of the host.
///
/// The agent does not touch the document itself: it sends a request to the bridge window of this process, and the
/// window procedure runs here, on the UI thread of the application that has the focus, where the text service and the
/// TSF context live.
impl TsfHost {
    /// The text that is selected in the document (empty when nothing is selected).
    pub fn selection_text(&mut self) -> Option<String> {
        let ctx = self.context.clone()?;
        let ctx2 = ctx.clone();
        run_session(&ctx, self.client_id, SYNC_READ, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let range = caret_range(&ctx2, ec).map_err(refused)?;
                if range.IsEmpty(ec).map_err(refused)?.as_bool() {
                    return Ok(String::new());
                }
                // The interface does not say how long the text is: the buffer grows until everything fits.
                let mut buffer = vec![0u16; 1024];
                let got;
                loop {
                    let mut copied = 0u32;
                    range.GetText(ec, 0, &mut buffer, &mut copied).map_err(refused)?;
                    let count = (copied as usize).min(buffer.len());
                    if count < buffer.len() || buffer.len() >= MAX_SELECTION_CHARS {
                        got = count;
                        break;
                    }
                    buffer.resize(buffer.len() * 2, 0);
                }
                if got >= MAX_SELECTION_CHARS {
                    return Err(HostError::Refused); // a selection of a megabyte is not something to hand to an agent
                }
                buffer.truncate(got);
                Ok(String::from_utf16_lossy(&buffer))
            }
        })
        .ok()
    }

    /// Replace what is selected with the text. An empty selection means "insert at the caret".
    pub fn replace_selection(&mut self, text: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        let (ctx2, wide) = (ctx.clone(), text.encode_utf16().collect::<Vec<u16>>());
        run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let range = caret_range(&ctx2, ec).map_err(refused)?;
                range.SetText(ec, 0, &wide).map_err(refused)?;
                range.Collapse(ec, TF_ANCHOR_END).map_err(refused)?;
                let selection = selection_of(range);
                let set = ctx2.SetSelection(ec, std::slice::from_ref(&selection));
                drop(ManuallyDrop::into_inner(selection.range));
                set.map_err(refused)
            }
        })
    }

    /// The text that is already in the document, just before the caret.
    ///
    /// The demonstration asks it whether the document holds a domain header of its own: only the last window
    /// before the caret is read, because the question is about the piece the demonstration writes into, and
    /// a document may be long.
    pub fn text_before_caret(&mut self) -> Option<String> {
        let ctx = self.context.clone()?;
        let ctx2 = ctx.clone();
        run_session(&ctx, self.client_id, SYNC_READ, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let caret = caret_range(&ctx2, ec).map_err(refused)?;
                let range = caret.Clone().map_err(refused)?;
                // the window before the caret: the shift stops at the beginning of the document
                let wanted = -(BEFORE_CARET_CHARS as i32);
                let mut moved = 0i32;
                range
                    .ShiftStart(ec, wanted, &mut moved, std::ptr::null())
                    .map_err(refused)?;
                let count = moved.unsigned_abs() as usize;
                if count == 0 {
                    return Ok(String::new());
                }
                let mut buffer = vec![0u16; count];
                let mut got = 0u32;
                range.GetText(ec, 0, &mut buffer, &mut got).map_err(refused)?;
                let got = (got as usize).min(count);
                buffer.truncate(got);
                Ok(String::from_utf16_lossy(&buffer))
            }
        })
        .ok()
    }
}

/// Answer a request of an AI agent that came through the bridge window.
///
/// The rules are the ones of the product: nothing is ever written into a password field, and a write that the host
/// refuses is reported as refused instead of being retried behind the user's back.
pub(super) fn answer_agent(inner: &mut Inner, request: BridgeRequest) -> BridgeReply {
    let mut reply = answer_the_request(inner, &request);
    // The answer says who the request was from: the agent learns that its own mouth is the one that wrote.
    reply.source = request.source.clone();
    reply
}

/// The work of the bridge: what the request of the agent asks for.
fn answer_the_request(inner: &mut Inner, request: &BridgeRequest) -> BridgeReply {
    match request.op {
        BridgeOp::Insert => {
            if request.text.is_empty() {
                return BridgeReply::failed(BridgeError::Malformed, "there is no text to write");
            }
            if let Err(error) = crate::agent_bridge::may_write(inner.controller.host_mut().is_secure_field()) {
                return BridgeReply::failed(
                    error,
                    "the field is a password field: nothing is written into it",
                );
            }
            let text = request.text.clone();
            // Where the text lands: the caret before the write is the place the character appears at.
            let anchor = inner.controller.host_mut().caret_anchor();
            let written = if request.replace {
                inner.controller.host_mut().replace_selection(&text)
            } else {
                inner.controller.host_mut().commit("", &text)
            };
            match written {
                Ok(()) => {
                    // The light of the agent: the beam comes out of the mouth of that agent and has its colour, so
                    // that the user sees at once which agent wrote the character.
                    if let Some(anchor) = anchor
                        && let Ok(mut laser) = inner.laser.try_borrow_mut()
                    {
                        laser.fire_for(
                            &request.source.key(),
                            request.source.display_name(),
                            anchor,
                        );
                    }
                    let mut reply = BridgeReply::ok();
                    reply.inserted = text.chars().count();
                    reply.language = Some(inner.language.clone());
                    reply
                }
                Err(HostError::Mismatch) => BridgeReply::failed(
                    BridgeError::Refused,
                    "there is a selection: the text would have replaced it",
                ),
                Err(HostError::Refused) => {
                    BridgeReply::failed(BridgeError::Refused, "the host refused the write")
                }
            }
        }
        BridgeOp::Read => match inner.controller.host_mut().selection_text() {
            Some(text) => {
                let mut reply = BridgeReply::ok();
                reply.text = text;
                reply
            }
            None => BridgeReply::failed(BridgeError::NoContext, "there is no document to read"),
        },
        BridgeOp::Language => {
            let Some(language) = request.language.clone() else {
                return BridgeReply::failed(BridgeError::Malformed, "no language was named");
            };
            if !matches!(language.as_str(), "zh-CN" | "en-US" | "ja-JP") {
                return BridgeReply::failed(
                    BridgeError::Malformed,
                    "the language is not one of the three of the product",
                );
            }
            inner.switch_language(&language);
            let mut reply = BridgeReply::ok();
            reply.language = Some(inner.language.clone());
            reply
        }
        BridgeOp::Status => {
            let mut reply = BridgeReply::ok();
            reply.language = Some(inner.language.clone());
            reply.message = if inner.controller.input_mode() {
                "the input mode is on".to_string()
            } else {
                "the direct mode is on".to_string()
            };
            reply
        }
        // The two operations of the word bank need the storage of the word bank, which is not written yet.
        BridgeOp::Misses => BridgeReply::failed(
            BridgeError::Unsupported,
            "the words the user did not find are not collected in this build",
        ),
        BridgeOp::Curate => BridgeReply::failed(
            BridgeError::Unsupported,
            "the word bank of the user is not written in this build",
        ),
    }
}
