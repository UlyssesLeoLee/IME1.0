//! IT-005: a secure field collects nothing - no Snapshot, no PredictAI, and no cached text.
//!
//! The design case (`00_Documents/02_UT_IT_ST/Programming_IME_IT_Test_Design_v1.0.md`, IT-005) says:
//!
//! * steps: "trigger an InputChanged in a mock secure text host"
//! * criterion: "no Snapshot / PredictAI is sent, the text is not cached"
//!
//! The rule of the product (DD-01 section 3.3) is:
//!
//! ```text
//! on_input(event):
//!   if secure_context OR app_policy=DISABLED:
//!       clear_snapshot_and_candidates(); return PASS_THROUGH
//! ```
//!
//! The text service asks the shared TASE policy whether it may collect here (`pime_tsf::tase_policy`); a protected
//! context is refused before any permission is considered. The text service has exactly one way to the core: the
//! prediction. When it never asks, no context can leave towards a Snapshot and no AI request can start; and the
//! machine clears the text it had (`clear_snapshot`).
//!
//! What this does NOT prove: that the Core refuses to take a Snapshot of a secure editor buffer on its own (that is
//! the bridge side, `pime-core`'s context broker, with its own cases), and that a real Windows secure field (a
//! credential UI, the UAC prompt) is detected by the host - the detection itself is `tip.rs::probe_secure`, which
//! needs a real COM host.

mod common;

use common::{FakeHost, Harness};
use pime_tsf::controller::TipConfig;

/// The mock secure text host of the design case: the host says the field is a password field.
///
/// The contract of the host is "fail closed": a host that cannot tell is treated the same way (`tip.rs` answers
/// `true` when the probe fails), so the model of "cannot tell" is the model of a password field.
fn secure_host() -> FakeHost {
    FakeHost {
        secure: true,
        ..FakeHost::with_doc("")
    }
}

/// The whole case: in a secure field nothing is collected, nothing is asked and nothing is kept.
#[test]
fn it_005_a_secure_field_sends_no_context_asks_no_ai_and_caches_no_text() {
    let mut harness = Harness::with(TipConfig::default(), secure_host());

    // the user types into the secure field
    harness.type_text("secret");
    let t = harness.transcript();

    // PASS_THROUGH: every key belongs to the host, nothing is eaten and nothing is written
    assert_eq!(t.passed, ["s", "e", "c", "r", "e", "t"], "{t:?}");
    assert!(t.eaten.is_empty(), "{t:?}");
    assert_eq!(t.doc, "secret", "{t:?}");
    assert!(t.commits.is_empty(), "{t:?}");

    // nothing is collected: the core is never asked, so no context can leave towards a Snapshot and no AI request
    // can start
    assert!(
        t.queries.is_empty(),
        "the engine was asked in a secure field: {:?}",
        t.queries
    );
    assert!(!t.visible, "no window over a secure field: {t:?}");
    assert_eq!(t.shows, 0, "no window was ever shown: {t:?}");

    // the text is not cached
    assert!(
        harness.ctl.machine().context().is_empty(),
        "the text is cached: {:?}",
        harness.ctl.machine().context()
    );
    assert!(
        harness.ctl.machine().candidates().is_empty(),
        "the candidates are kept: {:?}",
        harness.ctl.machine().candidates()
    );

    // the shared policy of TASE refuses this context, and the permissions that would start an AI request are off
    assert!(
        !pime_tsf::tase_policy::may_collect(false, true),
        "a password field is a protected context"
    );
    assert!(
        !pime_tsf::tase_policy::may_write(false, true),
        "nothing is written into a password field"
    );
    assert!(
        pime_tsf::tase_policy::refusal_reason(false, true).is_some(),
        "the reason of the refusal is named"
    );
    let policy = pime_tsf::tase_policy::policy();
    assert!(
        !policy.allow_excerpt_in_code_context,
        "the excerpt permission is off in the shipped product"
    );
    assert!(
        !policy.allow_agent_orchestration,
        "the agent permission is off in the shipped product"
    );
    assert!(
        !policy.may_share_with_agent(&pime_tsf::tase_policy::context_of(false, true)),
        "nothing of a protected context is shared with an agent"
    );

    // and an agent gets nothing out of a secure field either
    assert_eq!(
        pime_tsf::agent_bridge::may_write(true).err(),
        Some(pime_core::agent_proto::Error::Secure),
        "an agent may not write into a password field"
    );
}

/// The secure desktop is a protected context of its own.
#[test]
fn the_secure_desktop_collects_nothing_either() {
    let mut harness = Harness::new(TipConfig::default());
    // TF_TMAE_SECUREMODE: the thread manager says the thread runs on the secure desktop.
    harness.ctl.activate(true);
    harness.type_text("fn");
    let t = harness.transcript();
    assert!(
        t.queries.is_empty(),
        "the engine was asked: {:?}",
        t.queries
    );
    assert!(!t.visible, "{t:?}");
    assert_eq!(t.shows, 0, "{t:?}");
    assert!(
        harness.ctl.machine().context().is_empty(),
        "the text is cached: {:?}",
        harness.ctl.machine().context()
    );
    assert!(!pime_tsf::tase_policy::may_collect(true, false));
    let policy = pime_tsf::tase_policy::policy();
    assert!(
        !policy.may_share_with_agent(&pime_tsf::tase_policy::context_of(true, false)),
        "nothing of the secure desktop is shared with an agent"
    );
}

/// A field that turns into a password field while the candidates are on screen: they go away and the text is
/// forgotten (the `clear_snapshot_and_candidates` of DD-01 section 3.3).
#[test]
fn a_field_that_turns_into_a_password_field_clears_the_candidates_and_the_text() {
    let mut harness = Harness::new(TipConfig::default());
    harness.type_text("fn");
    let before = harness.transcript();
    assert!(before.visible, "the candidates are on screen");
    assert_eq!(
        before.queries.len(),
        2,
        "the engine was asked: {:?}",
        before.queries
    );

    // the same context becomes a password field, without a focus event
    harness.ctl.host_mut().secure = true;
    harness.type_text("x");
    let t = harness.transcript();

    assert!(!t.visible, "the candidates are gone: {t:?}");
    assert_eq!(
        t.shows, before.shows,
        "no window was shown in the secure field"
    );
    assert_eq!(
        t.queries.len(),
        2,
        "the engine was asked again in the secure field: {:?}",
        t.queries
    );
    assert!(
        harness.ctl.machine().context().is_empty(),
        "the text is cached: {:?}",
        harness.ctl.machine().context()
    );
    assert!(
        harness.ctl.machine().candidates().is_empty(),
        "the candidates are kept: {:?}",
        harness.ctl.machine().candidates()
    );
}
