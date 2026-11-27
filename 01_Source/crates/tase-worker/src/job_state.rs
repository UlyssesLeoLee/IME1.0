//! The mapping between the state of a job of the contract layer and the state of a job of the store (detailed
//! design §2.6, §6.3 and §18.1).
//!
//! # Why there are two state machines
//!
//! The contract layer is a frozen surface: [`JobState`] has eight states and it is the state that the client reads
//! in `tase.job.get` (detailed design §2.6). The store has its own seven states ([`JobStatus`]): it keeps the
//! lease, the epoch and the attempt of a job, and not the evidence of the plan. This module is the layer between the
//! two, and it says what a state of one side means on the other side.
//!
//! # The table
//!
//! | the state of the contract layer | the state of the store | what it means |
//! |---|---|---|
//! | `QUEUED` | `QUEUED` | the job waits for a worker |
//! | `RUNNING` | `RUNNING` | a worker holds the lease of the job |
//! | `VALIDATING` | `VERIFYING` | the candidate is written and the checks run |
//! | `COMMITTING` | `VERIFYING` | the store has no state of its own for the commit |
//! | `COMMITTED` | `COMMITTED` | the job is done and the commit is on the ref |
//! | `FAILED` | `FAILED` | the job failed |
//! | `CANCELLED` | `CANCELLED` | the job was cancelled |
//! | `REBASED` | `QUEUED` | the store has no state of its own for the rebase |
//!
//! # The states that the store does not have
//!
//! `COMMITTING` and `REBASED` are states of the contract layer that the store does not have. They are not lost: the
//! jobs of those states are kept in the state of the store that comes with them (`VERIFYING` and `QUEUED`), and the
//! table says so with [`StoreState::Shared`]. A caller that asks for the jobs of a state of the contract layer
//! therefore gets the jobs of the state of the store that holds them.
//!
//! # The state that the contract layer does not have
//!
//! The store has `CONFLICT` and the contract layer does not: the contract layer is a frozen surface and it is not
//! changed here. A job that met a conflict is a job that failed, and the code of the failure of the job says that it
//! was a conflict (`SYMBOL_CONFLICT`), as detailed design §6.3 puts `CONFLICT` next to `FAILED` among the terminal
//! states.

use tase_contract::api::JobState;
use tase_local_store::JobStatus;

/// The state of the store that keeps the jobs of a state of the contract layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreState {
    /// The store has a state of its own for that state of the contract layer.
    Own(JobStatus),
    /// The store has no state of its own: the jobs of that state of the contract layer are kept in this state of
    /// the store.
    Shared(JobStatus),
}

impl StoreState {
    /// The state of the store that keeps the jobs.
    #[must_use]
    pub fn status(self) -> JobStatus {
        match self {
            StoreState::Own(status) | StoreState::Shared(status) => status,
        }
    }

    /// Whether the store has a state of its own for that state of the contract layer.
    #[must_use]
    pub fn is_own(self) -> bool {
        matches!(self, StoreState::Own(_))
    }
}

/// The mapping of one state of the contract layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateMapping {
    /// The state of the contract layer.
    pub contract: JobState,
    /// The state of the store that keeps the jobs of that state.
    pub store: StoreState,
}

/// Every state of the contract layer, with the state of the store that keeps it.
///
/// The table is the whole of the contract layer: a state that is added to the contract layer later has no state of
/// the store until this table says one.
pub const MAPPINGS: [StateMapping; 8] = [
    StateMapping {
        contract: JobState::Queued,
        store: StoreState::Own(JobStatus::Queued),
    },
    StateMapping {
        contract: JobState::Running,
        store: StoreState::Own(JobStatus::Running),
    },
    StateMapping {
        contract: JobState::Validating,
        store: StoreState::Own(JobStatus::Verifying),
    },
    StateMapping {
        contract: JobState::Committing,
        store: StoreState::Shared(JobStatus::Verifying),
    },
    StateMapping {
        contract: JobState::Committed,
        store: StoreState::Own(JobStatus::Committed),
    },
    StateMapping {
        contract: JobState::Failed,
        store: StoreState::Own(JobStatus::Failed),
    },
    StateMapping {
        contract: JobState::Cancelled,
        store: StoreState::Own(JobStatus::Cancelled),
    },
    StateMapping {
        contract: JobState::Rebased,
        store: StoreState::Shared(JobStatus::Queued),
    },
];

/// The state of the contract layer of a job of the store.
///
/// The store keeps the job that validates and the job that commits in one state (`VERIFYING`) and it cannot tell the
/// two apart: the state of the contract layer that is reported for that state is `VALIDATING`, the first of the two
/// phases.
///
/// The contract layer has no state of a conflict: a job that met a conflict is a job that failed, and the code of
/// the failure of the job says that it was a conflict.
#[must_use]
pub fn contract_state_of(status: JobStatus) -> JobState {
    match status {
        JobStatus::Queued => JobState::Queued,
        JobStatus::Running => JobState::Running,
        JobStatus::Verifying => JobState::Validating,
        JobStatus::Committed => JobState::Committed,
        JobStatus::Failed => JobState::Failed,
        JobStatus::Cancelled => JobState::Cancelled,
        JobStatus::Conflict => JobState::Failed,
    }
}

/// The state of the store that keeps the jobs of a state of the contract layer.
///
/// `None` when the contract layer has a state that the mapping does not know: the mapping is the whole of the
/// contract layer, and a state that is added to it later has no state of the store until the table says one.
#[must_use]
pub fn store_status_of(state: JobState) -> Option<JobStatus> {
    MAPPINGS
        .iter()
        .find(|mapping| mapping.contract == state)
        .map(|mapping| mapping.store.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state of the store.
    const STORE_STATES: [JobStatus; 7] = [
        JobStatus::Queued,
        JobStatus::Running,
        JobStatus::Verifying,
        JobStatus::Committed,
        JobStatus::Failed,
        JobStatus::Cancelled,
        JobStatus::Conflict,
    ];

    #[test]
    fn every_state_of_the_contract_layer_has_a_state_in_the_store() {
        // the table holds every state of the contract layer, once each
        assert_eq!(MAPPINGS.len(), JobState::ALL.len());
        for state in JobState::ALL {
            let found: Vec<&StateMapping> = MAPPINGS
                .iter()
                .filter(|mapping| mapping.contract == state)
                .collect();
            assert_eq!(found.len(), 1, "the state {state} is mapped once");
            let mapping = found[0];

            // the state of the store that the table says is the state the lookup gives
            assert_eq!(store_status_of(state), Some(mapping.store.status()));

            // a state of the store that has a state of its own comes back to the state it came from
            if mapping.store.is_own() {
                assert_eq!(
                    contract_state_of(mapping.store.status()),
                    state,
                    "the state {state} does not come back from the store"
                );
            }
        }
    }

    #[test]
    fn the_states_that_the_store_does_not_have_are_the_ones_of_the_table() {
        // the store has no state of its own for the commit and for the rebase
        let shared: Vec<JobState> = MAPPINGS
            .iter()
            .filter(|mapping| !mapping.store.is_own())
            .map(|mapping| mapping.contract)
            .collect();
        assert_eq!(shared, vec![JobState::Committing, JobState::Rebased]);

        // the jobs of those states are kept in the state of the store that comes with them
        assert_eq!(
            store_status_of(JobState::Committing),
            Some(JobStatus::Verifying)
        );
        assert_eq!(
            store_status_of(JobState::Rebased),
            Some(JobStatus::Queued)
        );
    }

    #[test]
    fn every_state_of_the_store_is_kept_by_a_state_of_the_contract_layer() {
        // every state of the store but the conflict is in the table: the contract layer has no state of a conflict
        for status in STORE_STATES {
            let kept = MAPPINGS
                .iter()
                .any(|mapping| mapping.store.status() == status);
            assert_eq!(
                kept,
                status != JobStatus::Conflict,
                "the state {status:?} is not kept by the table"
            );
        }

        // and the conflict is reported as a failure: the code of the failure says that it was a conflict
        assert_eq!(contract_state_of(JobStatus::Conflict), JobState::Failed);
        assert_eq!(contract_state_of(JobStatus::Failed), JobState::Failed);
    }

    #[test]
    fn the_state_of_the_store_that_verifies_is_the_one_that_validates() {
        // the store keeps the job that validates and the job that commits in one state and it cannot tell the two
        // apart: the state of the contract layer is the first of the two phases
        assert_eq!(
            contract_state_of(JobStatus::Verifying),
            JobState::Validating
        );
    }
}
