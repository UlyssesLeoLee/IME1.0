//! The predictor of the text service: the language of the code and the plugin languages that are on.
//!
//! The core asks for the candidates of **one** profile. A plugin language is a profile of its own - a whole
//! language, with its own rules, its own dialects and its own validator - so every plugin that is on is asked for
//! as well, and its candidates are offered next to the ones of the language of the code.
//!
//! The set of the plugins that are on is **the one of the engine**, not a copy of it: the settings window writes it
//! and the engine is the only place that keeps it, so there is nothing here that could go out of date.

use pime_core::candidate::Candidate;
use pime_core::input::{CoreFault, LocalPredictor, PredictPort, PredictRequest};

/// The predictor of the text service.
pub struct PluginPredictor {
    /// The engine of the core, asked for one profile at a time.
    base: LocalPredictor,
}

impl PluginPredictor {
    /// A predictor that offers the language of the code and the plugin languages that are on.
    pub fn new() -> PluginPredictor {
        PluginPredictor { base: LocalPredictor }
    }
}

impl PredictPort for PluginPredictor {
    fn predict(&mut self, request: &PredictRequest) -> Result<Vec<Candidate>, CoreFault> {
        // The language of the code comes first: it is the one the user is writing in.
        let mut out = self.base.predict(request)?;
        for plugin in pime_core::predict::engine().plugin_languages() {
            if plugin == request.language {
                continue;
            }
            let mut plugin_request = request.clone();
            plugin_request.language = plugin;
            // A plugin that has been turned off in the meantime simply has no candidates.
            if let Ok(mut more) = self.base.predict(&plugin_request) {
                out.append(&mut more);
            }
        }
        Ok(out)
    }
}

/// The lock of the tests that touch the state of the engine.
///
/// The engine of the core is a process-wide singleton: a test that changes the set of the plugins changes it for
/// every other test of the process as well, and the tests that run at the same time would see each other's set.
/// Every test that changes the set takes this lock, and every test that reads the set takes it as well.
#[cfg(test)]
pub(crate) fn engine_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_candidates_of_a_plugin_language_are_offered_next_to_the_ones_of_the_code() {
        let _guard = engine_lock();
        let engine = pime_core::predict::engine();
        let before = engine.plugin_languages();
        engine.set_plugin_languages(&["plsql".to_string()]);
        let mut predictor = PluginPredictor::new();
        let request = PredictRequest {
            language: "rust".to_string(),
            before: String::new(),
            prefix: "sel".to_string(),
        };
        let candidates = predictor.predict(&request);
        // the set is put back, so that the other tests see the state the product ships with
        engine.set_plugin_languages(&before);
        let candidates = candidates.expect("the engine answered");
        assert!(
            candidates.iter().any(|c| c.profile_id == "plsql"),
            "the PL/SQL plugin is on: its candidates have to be there"
        );
    }

    #[test]
    fn a_plugin_that_is_off_has_no_candidates() {
        let _guard = engine_lock();
        let engine = pime_core::predict::engine();
        let before = engine.plugin_languages();
        engine.set_plugin_languages(&[]);
        let mut predictor = PluginPredictor::new();
        let request = PredictRequest {
            language: "rust".to_string(),
            before: String::new(),
            prefix: "sel".to_string(),
        };
        let candidates = predictor.predict(&request);
        engine.set_plugin_languages(&before);
        let candidates = candidates.expect("the engine answered");
        assert!(
            !candidates.iter().any(|c| c.profile_id == "plsql"),
            "no plugin is on: there is no plugin candidate"
        );
    }
}
