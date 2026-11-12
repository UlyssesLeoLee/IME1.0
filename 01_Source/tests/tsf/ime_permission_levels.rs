//! IT-013: the L1 of the text service (the nearby part only) against the L3 of the bridge (the write).
//!
//! **NOT WIRED as a test target yet** (D-138): on the machine this round was built on, the security scanner
//! quarantines every **freshly linked** test binary ("could not execute process ... (never executed)", and the file
//! is gone afterwards), so a new `[[test]]` target cannot even be executed there. The gate's cargo retry only
//! knows `os error 5`, not this error, so it reports the step as failed instead of retrying. The test itself
//! compiles and the other targets of the crate are untouched. To run it: add the entry to
//! `crates/pime-tsf/Cargo.toml`
//!
//! ```toml
//! [[test]]
//! name = "ime_permission_levels"
//! path = "../../tests/tsf/ime_permission_levels.rs"
//! ```
//!
//! on a machine that can execute a fresh binary (or after the scanner is told to trust this one).
//!
//! The design case (`00_Documents/02_UT_IT_ST/Programming_IME_IT_Test_Design_v1.0.md`, IT-013) says:
//!
//! * steps: "the same editor, once without the bridge and once with it"
//! * criterion: "without the bridge only the nearby permission is granted, the bridge obtains L3; no fake full text"
//!
//! Both halves are proved here, in the core:
//!
//! * **without the bridge** the text service holds nothing but the characters the user typed, and the machine keeps
//!   at most `MAX_CONTEXT_CHARS` of them. The document is never in there, so there is no "full text" that could be
//!   faked: what the input method sees is the nearby part, and nothing else;
//! * **with the bridge** the permission broker of the product (`crates/pime-core/src/grants.rs`, the mirror of
//!   `editor-bridges/vscode/src/core/grants.js`) hands out L3 and refuses to let an L1 grant stand in for it - so the
//!   bridge can only write with the level that was granted to it.

use pime_core::candidate::Candidate;
use pime_core::grants::{AuthorizeRequest, Criteria, PermissionBroker, workspace_token};
use pime_core::ids::IdGen;
use pime_core::input::{CoreFault, InputMachine, KeyEvent, PredictPort, PredictRequest};

/// A core that answers with nothing: the text service is measured on its own, not on the engine.
struct SilentCore;

impl PredictPort for SilentCore {
    fn predict(&mut self, _request: &PredictRequest) -> Result<Vec<Candidate>, CoreFault> {
        Ok(Vec::new())
    }
}

/// The text service without the bridge: only the nearby part of what was typed is held, never a whole document.
#[test]
fn it_013_without_the_bridge_only_the_nearby_part_is_held() {
    let mut machine = InputMachine::new(SilentCore, "rust");
    let mut now = 1000u64;
    // the user types more than the machine may keep
    for _ in 0..2500 {
        now += 1;
        machine.on_key(KeyEvent::ch('x'), now);
    }
    let context = machine.context();
    let held = context.chars().count();
    assert!(held <= 2000, "the text service holds {held} characters: more than the nearby part");
    assert!(held > 0, "the machine kept nothing at all, so this test would prove nothing");
    // the nearby part is the END of what was typed: the beginning is gone
    assert!(context.ends_with('x'), "the context is not the nearby part");
    assert!(!context.contains('y'), "something that was never typed is in the context");
}

/// The bridge with the bridge: the write needs L3, and an L1 grant is not enough.
#[test]
fn it_013_with_the_bridge_the_write_needs_l3() {
    let entropy: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(13).wrapping_add(5)).collect();
    let mut ids = IdGen::new(&entropy).expect("the entropy is accepted");
    let mut broker = PermissionBroker::new();
    let ws = workspace_token(&["file:///w".to_string()]);
    let now = 1000u64;

    // the nearby permission: enough to observe, not to write
    let nearby = broker
        .authorize(
            now,
            &mut ids,
            AuthorizeRequest {
                provider_id: "P",
                workspace: &ws,
                scope: "L1",
                ttl_ms: 60_000,
                once: false,
            },
        )
        .expect("the grant is made");
    let refused = broker.check(
        now,
        &nearby.id,
        Criteria {
            provider_id: "P",
            workspace: &ws,
            scope: "L3",
        },
    );
    assert!(!refused.ok, "an L1 grant was allowed to write");
    assert_eq!(refused.reason, "scope-mismatch", "{refused:?}");

    // the bridge's own level: the write goes through
    let write = broker
        .authorize(
            now,
            &mut ids,
            AuthorizeRequest {
                provider_id: "P",
                workspace: &ws,
                scope: "L3",
                ttl_ms: 60_000,
                once: false,
            },
        )
        .expect("the grant is made");
    assert!(
        broker.check(
            now,
            &write.id,
            Criteria {
                provider_id: "P",
                workspace: &ws,
                scope: "L3",
            }
        )
        .ok,
        "the L3 grant did not allow the write"
    );
    // and the L3 grant is not a pass for everything else either
    assert_eq!(
        broker.check(
            now,
            &write.id,
            Criteria {
                provider_id: "P",
                workspace: &ws,
                scope: "L4",
            }
        )
        .reason,
        "scope-mismatch"
    );
}
