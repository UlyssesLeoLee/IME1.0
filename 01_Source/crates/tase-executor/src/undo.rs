//! The undo description: the inverse of an apply, as one logical transaction for the host editor.
//!
//! Detailed design §8.1 says that a plan is shown in the local editor as **one logical undo transaction**. This
//! repository has no `Document` type and no undo stack (the mapping document §7-G1): the host editor owns the stack,
//! so the executor cannot push onto it. What the executor can do is **describe** the undo, exactly and completely,
//! so that the host can perform it in one gesture:
//!
//! * every document of the plan carries the edits that turn the new text back into the base text;
//! * the offsets of those edits are the offsets of the **new** text, because that is the text the host has when the
//!   user presses undo;
//! * the transaction carries the hash the document must have for the undo to be safe: an undo that would overwrite
//!   a change the user made after the apply is refused, like the rebase of detailed design §5.6 is.

use pime_core::sha256::sha256_hex;
use pime_core::textrange::Edit;

use crate::apply::ApplyPlan;
use crate::preview::Hunk;
use crate::Result;

/// The length of a text in UTF-16 code units: the unit of every offset of this crate.
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The inverse edits of a set of hunks: they turn the new text back into the base text.
///
/// The hunks are the ones of the base text, in the order of the file. The offset of a hunk in the new text is the
/// offset of the hunk in the base text plus the difference the earlier hunks made: a hunk that adds three units moves
/// everything after it three units to the right, and a hunk that removes them moves it to the left.
pub fn inverse_edits_of(hunks: &[Hunk]) -> Vec<Edit> {
    let mut edits = Vec::with_capacity(hunks.len());
    let mut delta: i64 = 0;
    for hunk in hunks {
        let start = (hunk.start_offset as i64 + delta).max(0) as usize;
        let end = start + utf16_len(&hunk.new_text);
        edits.push(Edit {
            start,
            end,
            text: hunk.old_text.clone(),
        });
        delta += utf16_len(&hunk.new_text) as i64 - (hunk.end_offset - hunk.start_offset) as i64;
    }
    edits
}

/// The inverse of one document of an apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoDocument {
    /// The path of the file.
    pub path: String,
    /// The hash the document must have for the undo to be safe: the hash the apply left it with.
    pub expected_sha256: String,
    /// The edits that restore the text of the base.
    pub edits: Vec<Edit>,
}

/// The undo of a plan: one transaction that covers every document of the apply.
///
/// The transaction is one object on purpose: the host undoes the whole plan in one gesture, and it never undoes half
/// a plan by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoTransaction {
    /// The digest of the plan this transaction undoes.
    pub plan_digest: String,
    /// The commit the plan started from.
    pub base_commit: String,
    /// The documents, in the order of the apply.
    pub documents: Vec<UndoDocument>,
}

impl UndoTransaction {
    /// The transaction that undoes an apply.
    pub fn of(plan: &ApplyPlan) -> UndoTransaction {
        let documents = plan
            .documents
            .iter()
            .map(|document| UndoDocument {
                path: document.path.clone(),
                expected_sha256: document.new_sha256.clone(),
                edits: document.inverse_edits.clone(),
            })
            .collect();
        UndoTransaction {
            plan_digest: plan.plan_digest.clone(),
            base_commit: plan.base_commit.clone(),
            documents,
        }
    }

    /// The id of the transaction: the digest of its content.
    ///
    /// The id needs no clock and no random source: the same apply always gives the same transaction and the same id,
    /// and two different transactions never share one.
    pub fn id(&self) -> String {
        let mut text = String::new();
        text.push_str(&self.plan_digest);
        text.push('\u{0}');
        text.push_str(&self.base_commit);
        text.push('\n');
        for document in &self.documents {
            text.push_str(&document.path);
            text.push('\u{0}');
            text.push_str(&document.expected_sha256);
            text.push('\u{0}');
            for edit in &document.edits {
                text.push_str(&format!(
                    "{}:{}:{} ",
                    edit.start,
                    edit.end,
                    utf16_len(&edit.text)
                ));
            }
            text.push('\n');
        }
        format!("sha256:{}", sha256_hex(&text))
    }

    /// Whether the transaction has nothing to undo.
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// The document of a path.
    pub fn document(&self, path: &str) -> Option<&UndoDocument> {
        self.documents.iter().find(|d| d.path == path)
    }

    /// Checks that the documents still are what the apply left them.
    ///
    /// The observed list holds the path and the digest of every document the caller wants to check. A document that
    /// changed after the apply is refused: the undo would throw away a change of the user.
    pub fn verify(&self, observed: &[(String, String)]) -> Result<()> {
        for document in &self.documents {
            match observed.iter().find(|(path, _)| path == &document.path) {
                None => {
                    return Err(tase_contract::TaseError::stale_base(format!(
                        "the document {} is not there any more",
                        document.path
                    )));
                }
                Some((_, digest)) if digest != &document.expected_sha256 => {
                    return Err(tase_contract::TaseError::stale_base(format!(
                        "the document {} changed after the apply",
                        document.path
                    )));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The text of a document before the apply.
    ///
    /// The edits of the transaction are the offsets of the text the apply produced, so the text that is given has to
    /// be that text: the host checks the hash of the document first.
    pub fn restore(&self, path: &str, current_text: &str) -> Result<String> {
        let document = self.document(path).ok_or_else(|| {
            tase_contract::TaseError::not_found(format!("the document {path} is not in the transaction"))
        })?;
        pime_core::textrange::apply_edits(current_text, &document.edits)
            .map_err(crate::from_core)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::ApplyPlan;

    const BASE: &str = "one\ntwo\nthree\nfour\n";
    const NEW: &str = "one\nTWO\nthree\nfour\nfive\n";

    fn plan_of(base: &str, new: &str) -> ApplyPlan {
        let document = crate::apply::document_of("src/main.rs", 1, base, new).expect("the document is built");
        ApplyPlan {
            base_commit: "commit-1".to_string(),
            plan_digest: "sha256:plan".to_string(),
            documents: vec![document],
        }
    }

    #[test]
    fn the_inverse_edits_of_the_hunks_restore_the_base_text() {
        let hunks = crate::preview::diff_of(BASE, NEW);
        let edits = inverse_edits_of(&hunks);
        let restored = pime_core::textrange::apply_edits(NEW, &edits).expect("the edits apply");
        assert_eq!(restored, BASE);
    }

    #[test]
    fn the_inverse_edits_are_the_offsets_of_the_new_text() {
        let hunks = crate::preview::diff_of(BASE, NEW);
        let edits = inverse_edits_of(&hunks);
        // the second hunk of this diff is at the end of the file: its offset moved by the delta of the first one
        assert_eq!(edits.len(), hunks.len());
        for (edit, hunk) in edits.iter().zip(hunks.iter()) {
            assert_eq!(edit.text, hunk.old_text);
            assert_eq!(edit.end - edit.start, utf16_len(&hunk.new_text));
        }
    }

    #[test]
    fn the_transaction_restores_every_document_of_the_apply() {
        let plan = plan_of(BASE, NEW);
        let transaction = UndoTransaction::of(&plan);
        assert!(!transaction.is_empty());
        assert_eq!(transaction.documents.len(), 1);
        let restored = transaction
            .restore("src/main.rs", NEW)
            .expect("the document is restored");
        assert_eq!(restored, BASE);
    }

    #[test]
    fn the_transaction_refuses_a_document_that_changed_after_the_apply() {
        let plan = plan_of(BASE, NEW);
        let transaction = UndoTransaction::of(&plan);
        let hash = sha256_hex(NEW);
        transaction
            .verify(&[("src/main.rs".to_string(), hash.clone())])
            .expect("the document is what the apply left");
        let error = transaction
            .verify(&[("src/main.rs".to_string(), sha256_hex("something else"))])
            .unwrap_err();
        assert_eq!(error.code, tase_contract::ErrorCode::StaleBase);
    }

    #[test]
    fn the_id_of_the_transaction_is_the_digest_of_its_content() {
        let plan = plan_of(BASE, NEW);
        let first = UndoTransaction::of(&plan);
        let second = UndoTransaction::of(&plan);
        assert_eq!(first.id(), second.id());
        assert!(first.id().starts_with("sha256:"));
        let other = plan_of(BASE, "one\ntwo\nthree\nfour\n");
        assert_ne!(first.id(), UndoTransaction::of(&other).id());
    }

    #[test]
    fn a_document_that_is_not_in_the_transaction_is_not_found() {
        let plan = plan_of(BASE, NEW);
        let transaction = UndoTransaction::of(&plan);
        let error = transaction.restore("src/other.rs", NEW).unwrap_err();
        assert_eq!(error.code, tase_contract::ErrorCode::NotFound);
    }
}
