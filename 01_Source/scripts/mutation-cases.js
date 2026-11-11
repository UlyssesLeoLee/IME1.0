'use strict';
// The mutations of the red-green check: every case breaks one place of the implementation on purpose and names the
// suite that has to notice.
//
// `from` must be a snippet that is in the file **exactly once** (the harness replaces the first occurrence and says
// so when the text is not there any more), `to` is the broken version. The cases are the behaviours the product
// cannot be wrong about: the rules of the domains, the keys, the modes, the safety, the ranking.
//
// A case that is MISSED is a hole in the test suite: it is reported, it is not hidden.
const CORE = {kind: 'rust', crate: 'pime-core'};
const TSF = {kind: 'rust', crate: 'pime-tsf'};
const JS = file => ({kind: 'js', file, env: {PIME_CORE_EXE: 'C:\\cargo-target-ime\\debug\\pime-core.exe'}});

module.exports = [
  // ---- the rules of the .NEMA domains ---------------------------------------------------------------------
  {
    id: 'nema-tab',
    what: 'a domain header needs a real tab (a space is not one)',
    file: '01_Source/crates/pime-core/src/nema.rs',
    from: 'let rest = line.strip_prefix(HEADER_TAB)?;',
    to: "let rest = line.strip_prefix(' ')?;",
    suite: CORE,
  },
  {
    id: 'nema-revision-zero',
    what: 'the revision of a domain is a positive integer',
    file: '01_Source/crates/pime-core/src/nema.rs',
    from: 'if revision == 0 {',
    to: 'if revision == u32::MAX {',
    suite: CORE,
  },
  {
    id: 'nema-trailing',
    what: 'nothing may follow the revision of a domain header',
    file: '01_Source/crates/pime-core/src/nema.rs',
    from: 'if revision.is_empty() || !revision.bytes().all(|byte| byte.is_ascii_digit()) {',
    to: 'if revision.is_empty() {',
    suite: CORE,
  },
  {
    id: 'nema-graph-marker',
    what: 'the mark of a graph domain touches its id',
    file: '01_Source/crates/pime-core/src/nema.rs',
    from: "let rest = rest.strip_prefix('>').unwrap_or(rest);",
    to: "let rest = rest.strip_prefix(' ').unwrap_or(rest);",
    suite: CORE,
  },
  {
    id: 'nema-scope',
    what: 'a domain reaches up to the header of the next one',
    file: '01_Source/crates/pime-core/src/nema.rs',
    from: '.map(|(line_start, ..)| *line_start)',
    to: '.map(|(line_start, ..)| *line_start + 1)',
    suite: CORE,
  },
  {
    id: 'romaji-a',
    what: 'the romaji table: a alone is あ (the file carries い today)',
    file: '01_Source/crates/pime-core/src/cjk/romaji-kana.txt',
    from: 'a:い\n',
    to: 'a:あ\n',
    suite: CORE,
  },

  // ---- the rules of the core ------------------------------------------------------------------------------
  {
    id: 'textrange-surrogate',
    what: 'an offset never splits a surrogate pair',
    file: '01_Source/crates/pime-core/src/textrange.rs',
    from: 'if !boundary_u(t, offset) {',
    to: 'if false {',
    suite: CORE,
  },
  {
    id: 'predict-invalid',
    what: 'a candidate that does not validate is not offered',
    file: '01_Source/crates/pime-core/src/predict.rs',
    from: 'out.retain(|c| c.validation != Validation::Invalid);',
    to: 'out.retain(|c| true);',
    suite: CORE,
  },
  {
    id: 'imeswitch-cycle',
    what: 'the language key walks through the three languages',
    file: '01_Source/crates/pime-core/src/imeswitch.rs',
    from: 'Some(index) => LANGUAGES[(index + 1) % LANGUAGES.len()],',
    to: 'Some(index) => LANGUAGES[index % LANGUAGES.len()],',
    suite: CORE,
  },
  {
    id: 'budget-limit',
    what: 'the budget of the AI is refused when it is used up',
    file: '01_Source/crates/pime-core/src/budget.rs',
    from: 'if self.max_requests.is_some_and(|m| self.requests + 1 > m)',
    to: 'if self.max_requests.is_some_and(|m| false)',
    suite: CORE,
  },
  {
    id: 'grants-mismatch',
    what: 'a grant is refused when the provider does not match',
    file: '01_Source/crates/pime-core/src/grants.rs',
    from: 'if g.provider_id != c.provider_id {',
    to: 'if false {',
    suite: CORE,
  },

  // ---- the keys and the modes of the text service --------------------------------------------------------
  {
    id: 'keymap-mode-chord',
    what: 'Ctrl+F12 switches between the code input and the natural language input',
    file: '01_Source/crates/pime-tsf/src/keymap.rs',
    from: 'code == vk::F12 && mods.ctrl && !mods.alt && !mods.shift && !mods.win',
    to: 'code == vk::F12 && mods.ctrl && !mods.alt && !mods.shift && mods.win',
    suite: TSF,
  },
  {
    id: 'keymap-alt-f1',
    what: 'Alt+F1 switches the input mode as well',
    file: '01_Source/crates/pime-tsf/src/keymap.rs',
    from: 'code == vk::F1 && mods.alt && !mods.ctrl && !mods.shift && !mods.win',
    to: 'code == vk::F1 && mods.alt && !mods.ctrl && !mods.shift && mods.win',
    suite: TSF,
  },
  {
    id: 'controller-default-mode',
    what: 'the factory mode is the code input',
    file: '01_Source/crates/pime-tsf/src/controller.rs',
    from: '            // The code input is the factory mode (the user decided that on 2026-11-11): the input method never\n            // eats a code character, so the code goes into the document exactly as it was typed. The natural\n            // language input (typing opens a composition, the candidate list takes the arrows, Tab, Space and\n            // Enter) is one Ctrl+F12 away - and Alt+F1 does the same.\n            input_mode: false,',
    to: '            // The code input is the factory mode (the user decided that on 2026-11-11): the input method never\n            // eats a code character, so the code goes into the document exactly as it was typed. The natural\n            // language input (typing opens a composition, the candidate list takes the arrows, Tab, Space and\n            // Enter) is one Ctrl+F12 away - and Alt+F1 does the same.\n            input_mode: true,',
    suite: TSF,
  },
  {
    id: 'controller-secure-chord',
    what: 'the mode key stays with the host in a password field',
    file: '01_Source/crates/pime-tsf/src/controller.rs',
    from: '        if is_input_mode_chord(key.code, key.mods) || is_mode_chord(key.code, key.mods) {\n            // Ctrl+F12 (the key the user asked for) and Alt+F1 switch between the code input and the natural\n            // language input; the panel offers the same as a row.\n            self.seen = None;\n            if self.secure {',
    to: '        if is_input_mode_chord(key.code, key.mods) || is_mode_chord(key.code, key.mods) {\n            // Ctrl+F12 (the key the user asked for) and Alt+F1 switch between the code input and the natural\n            // language input; the panel offers the same as a row.\n            self.seen = None;\n            if false {',
    suite: TSF,
  },

  // ---- the panel and the demonstration ---------------------------------------------------------------------
  {
    id: 'laser-demo-action',
    what: 'a click on the row of the demonstration asks for the demonstration',
    file: '01_Source/crates/pime-tsf/src/laser.rs',
    from: 'PanelItem::Demo => Some(PanelAction::Demo),',
    to: 'PanelItem::Demo => Some(PanelAction::Settings),',
    suite: TSF,
  },
  {
    id: 'demo-body-length',
    what: 'every agent writes exactly twenty characters',
    file: '01_Source/crates/pime-tsf/src/demo.rs',
    from: 'let body = format!("agent{number:02}_runs_nema_ok");',
    to: 'let body = format!("agent{number:02}_runs_nema_ok!");',
    suite: TSF,
  },
  {
    id: 'demo-header-tab',
    what: 'the header of a domain starts with a real tab',
    file: '01_Source/crates/pime-tsf/src/demo.rs',
    from: 'format!("\\t{} {}", self.domain_id, self.revision)',
    to: 'format!(" {} {}", self.domain_id, self.revision)',
    suite: TSF,
  },
  {
    id: 'demo-note',
    what: 'the address of the interpreter is only written when there is no domain header',
    file: '01_Source/crates/pime-tsf/src/demo.rs',
    from: 'add_note: !nema::has_domain_header(text),',
    to: 'add_note: true,',
    suite: TSF,
  },
  {
    id: 'demo-revision',
    what: 'a domain that is written again gets the next revision',
    file: '01_Source/crates/pime-tsf/src/demo.rs',
    from: '.map_or(1, |highest| highest.saturating_add(1))',
    to: '.map_or(1, |_highest| 1)',
    suite: TSF,
  },
  {
    id: 'demo-secure',
    what: 'a password field gets nothing at all',
    file: '01_Source/crates/pime-tsf/src/demo.rs',
    from: 'if host.is_secure_field() {',
    to: 'if false {',
    suite: TSF,
  },
  {
    id: 'mouths-user-mouth',
    what: 'the mouth of the human does not follow the demonstration',
    file: '01_Source/crates/pime-tsf/src/mouths.rs',
    // The place of the human's mouth is decided in `make` (the user always keeps the place he put the mouth at),
    // so this is the place where the behaviour really lives.
    from: 'let is_user = key == USER_KEY;',
    to: 'let is_user = false;',
    suite: TSF,
  },
  {
    id: 'panel-preview-does-not-open',
    what: 'the preview of the code input does not open the panel',
    file: '01_Source/crates/pime-tsf/src/laser_ui.rs',
    from: 'if self.panel.is_some() && self.list_takes_keys {',
    to: 'if self.panel.is_some() {',
    suite: TSF,
  },

  // ---- the JavaScript mirror of the core -------------------------------------------------------------------
  {
    id: 'js-candidates-risk',
    what: 'the weight of the risk note',
    file: '01_Source/editor-bridges/vscode/src/core/candidates.js',
    from: 'const RISK_PENALTY_NOTED = -0.1;',
    to: 'const RISK_PENALTY_NOTED = -0.2;',
    suite: JS('ut-tase-candidates.test.js'),
  },
  {
    id: 'js-candidates-segment',
    what: 'the weight of the segments',
    file: '01_Source/editor-bridges/vscode/src/core/candidates.js',
    from: 'const APPLICABILITY_SEGMENT_WEIGHT = 0.6;',
    to: 'const APPLICABILITY_SEGMENT_WEIGHT = 0.7;',
    suite: JS('ut-tase-candidates.test.js'),
  },
  {
    id: 'js-candidates-tie',
    what: 'the tie breaker of the ranking',
    file: '01_Source/editor-bridges/vscode/src/core/candidates.js',
    from: 'return compareIds(a.candidate.candidate_id, b.candidate.candidate_id);',
    to: 'return compareIds(b.candidate.candidate_id, a.candidate.candidate_id);',
    suite: JS('ut-tase-candidates.test.js'),
  },
  {
    id: 'js-candidates-password',
    what: 'the reason the candidates are off in a password field',
    file: '01_Source/editor-bridges/vscode/src/core/candidates.js',
    from: "const REASON_PASSWORD = 'password: the candidates are disabled in a password control';",
    to: "const REASON_PASSWORD = 'password: the candidates are off';",
    suite: JS('ut-tase-candidates.test.js'),
  },
  {
    id: 'js-textrange-surrogate',
    what: 'an offset never splits a surrogate pair',
    file: '01_Source/editor-bridges/vscode/src/core/textrange.js',
    from: "if (!isBoundary(text, offset)) throw fail('TARGET_RANGE_INVALID', 'position splits a surrogate pair');",
    to: "if (false) throw fail('TARGET_RANGE_INVALID', 'position splits a surrogate pair');",
    suite: JS('ut-edit.test.js'),
  },
  {
    id: 'js-transaction-apply',
    what: 'a failed syntax check disables the default apply',
    file: '01_Source/editor-bridges/vscode/src/core/transaction.js',
    from: "if (validationOk === false) throw fail('VALIDATION_FAILED', 'syntax check failed; default apply is disabled');",
    to: "if (false) throw fail('VALIDATION_FAILED', 'syntax check failed; default apply is disabled');",
    suite: JS('ut-edit.test.js'),
  },
  {
    id: 'js-languages-invalid',
    what: 'a candidate that does not validate is not offered',
    file: '01_Source/editor-bridges/vscode/src/core/languages.js',
    from: "const kept = out.filter(c => c.validation !== 'invalid');",
    to: 'const kept = out;',
    suite: JS('ut-language.test.js'),
  },
];
