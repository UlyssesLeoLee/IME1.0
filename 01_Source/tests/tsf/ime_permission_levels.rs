//! IT-013: the L1 of the text service (the nearby part only) against the L3 of the bridge (the write).
//!
//! The design case (`00_Documents/02_UT_IT_ST/Programming_IME_IT_Test_Design_v1.0.md`, IT-013) says:
//!
//! * steps: "the same editor, once without the bridge and once with it"
//! * criterion: "without the bridge only the nearby permission is granted, the bridge obtains L3; no fake full text"
//!
//! Both halves are proved here, in the core:
//!
//! * **without the bridge** the text service holds nothing but the characters the user typed, and it holds a
//!   bounded window of them: the nearby part, never a whole document, never a character that was not typed;
//! * **with the bridge** the permission broker of the product (`crates/pime-core/src/grants.rs`, the mirror of
//!   `editor-bridges/vscode/src/core/grants.js`) hands out L3 and refuses to let an L1 grant stand in for it - so the
//!   bridge can only write with the level that was granted to it.
//!
//! The target is wired in `crates/pime-tsf/Cargo.toml` (`[[test]] ime_permission_levels`).
//!
//! NOTE (D-138): on the machine this was built on, the security scanner quarantines a **freshly linked** test binary
//! ("could not execute process ... (never executed)", and the file is gone afterwards), and the gate's cargo retry
//! only knows `os error 5`, not this error. The binary of this target has been linked and run there, so it is let
//! through; on a machine where the scanner still holds a fresh binary, run this target once by hand
//! (`cargo test --test ime_permission_levels`) before the gate runs. Widening the retry is D-138.

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
///
/// The size of the window is an implementation detail of the core (`MAX_CONTEXT_CHARS` is private to `pime-core`
/// and the design fixes no number), so the test pins the property the design states - the held text is a bounded
/// window of what was typed, it is the END of it, and it holds nothing that was not typed - and not the number.
#[test]
fn it_013_without_the_bridge_only_the_nearby_part_is_held() {
    let mut machine = InputMachine::new(SilentCore, "rust");
    // the host granted the nearby range: Direct -> Observing. Without it the machine stays in Direct, hands every
    // key back to the host and keeps nothing at all.
    machine.grant_observation(true);
    let mut now = 1000u64;
    // the user types more than the machine may keep
    for _ in 0..4000 {
        now += 1;
        machine.on_key(KeyEvent::ch('x'), now);
    }
    // and then as much again: a window does not grow with the input
    for _ in 0..4000 {
        now += 1;
        machine.on_key(KeyEvent::ch('y'), now);
    }
    let held = machine.context().chars().count();
    assert!(held > 0, "the machine kept nothing at all, so this test would prove nothing");
    assert!(
        held < 8000,
        "the machine holds the whole text: the nearby part is not a window"
    );
    // the nearby part is the END of what was typed: the beginning ('x') is gone, so nothing but the end is held
    assert!(
        machine.context().chars().all(|c| c == 'y'),
        "the machine still holds the beginning of what was typed"
    );
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
