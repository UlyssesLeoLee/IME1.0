//! IT-002: installing, switching and removing the text service leave every other input method alone.
//!
//! The design case (`00_Documents/02_UT_IT_ST/Programming_IME_IT_Test_Design_v1.0.md`, IT-002) says:
//!
//! * steps: "record every existing input method before PIME is installed; compare after the switch and after the removal"
//! * criterion: "the GUIDs and the default settings of the input methods that are not this product are preserved as
//!   they were"
//!
//! The product touches the machine in exactly two places, and both are driven from here:
//!
//! * `pime_tsf::register` - the COM subtree of our own CLSID and our own TSF profiles and categories
//!   (`registry_plan` / `removal_plan` + `register_with` / `unregister_with` over a `Backend`);
//! * `pime_tsf::ime_switch` - the decision of the switch key, which is pure (`pime_core::imeswitch`).
//!
//! The test drives the real code over a model of the registry that holds three input methods of other vendors and the
//! user's own input-method settings (the preload lists, the CTF entries, the default IME). The comparison is the one
//! the design case asks for: everything that is not ours must be byte-identical before and after.
//!
//! What this does NOT prove: that Windows itself (msctf, `ITfInputProcessorProfileMgr::RegisterProfile`) leaves the
//! other input methods alone. That half needs a Windows 11 VM with three real input methods and the signed installer
//! (the same environment IT-001 needs); the runbook is in `02_Verification/REAL_MACHINE_CHECKLIST.md`.

use std::collections::{BTreeMap, BTreeSet};

use pime_core::imeswitch::{Profile, Switch, decide, remember};
use pime_tsf::ime_switch::own_profiles;
use pime_tsf::register::{
    Backend, BackendError, CategorySpec, Plan, ProfileSpec, RegKey, Root, Scope, is_own_key,
    register_with, registry_plan, removal_plan, unregister_with,
};

/// The DLL of the product, as the installation registers it.
const DLL: &str = r"C:\Program Files\Programming IME\pime_tsf.dll";

/// The class of the text service of this product.
const OUR_CLSID: u128 = 0x43f95ed1_95b4_421d_8f5a_837128c2d854;

/// Another vendor's text service (Microsoft Pinyin) and its profile.
const OTHER_CLSID: &str = "{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}";
const OTHER_PROFILE: &str = "{82590C13-F4DD-44F4-BA1D-8667246FDF8E}";
/// A third one (the Japanese input method of another vendor).
const OTHER_CLSID_JA: &str = "{13A016DF-560B-46CD-947A-4C3AF1E0E35D}";
const OTHER_PROFILE_JA: &str = "{B9C8E7D6-5A43-4F21-9B30-6C2D1E4F8A57}";

/// The value of a GUID, as the model of the registry needs it.
fn guid_value(text: &str) -> u128 {
    pime_tsf::ime_switch::string_to_guid(text)
        .unwrap_or_else(|| panic!("{text} is not a GUID"))
        .to_u128()
}

// ------------------------------------------------------------------ the model of the machine

/// The registry of the machine, in memory: the keys with their values, the registered TSF profiles and categories.
#[derive(Default, Clone, Debug)]
struct ModelRegistry {
    /// key -> (value name -> value)
    values: BTreeMap<(Root, String), BTreeMap<String, String>>,
    /// the registered profiles: (text service, language, profile) -> description
    profiles: BTreeMap<(u128, u16, u128), String>,
    /// the registered categories: (text service, category)
    categories: BTreeSet<(u128, u128)>,
    /// the text services that exist
    text_services: BTreeSet<u128>,
}

impl ModelRegistry {
    fn set(&mut self, root: Root, path: &str, name: &str, value: &str) {
        self.values
            .entry((root, path.to_string()))
            .or_default()
            .insert(name.to_string(), value.to_string());
    }
}

/// The machine of the design case: three input methods that are not ours, and the settings of the user.
fn machine_with_other_input_methods() -> ModelRegistry {
    let mut registry = ModelRegistry::default();

    // -- another vendor's text service: its COM keys and its CTF entry
    let pinyin = format!(r"Software\Classes\CLSID\{OTHER_CLSID}");
    registry.set(Root::LocalMachine, &pinyin, "", "Microsoft Pinyin");
    registry.set(
        Root::LocalMachine,
        &format!(r"{pinyin}\InprocServer32"),
        "",
        r"C:\Windows\System32\msinput.dll",
    );
    registry.set(
        Root::LocalMachine,
        &format!(r"{pinyin}\InprocServer32"),
        "ThreadingModel",
        "Both",
    );
    let pinyin_tip = format!(r"Software\Microsoft\CTF\TIP\{OTHER_CLSID}");
    registry.set(Root::LocalMachine, &pinyin_tip, "", "Microsoft Pinyin");
    registry.set(
        Root::LocalMachine,
        &format!(r"{pinyin_tip}\LanguageProfile\0x0804\{OTHER_PROFILE}"),
        "",
        "Microsoft Pinyin",
    );

    // -- a third one: the Japanese input method of another vendor
    let japanese = format!(r"Software\Classes\CLSID\{OTHER_CLSID_JA}");
    registry.set(Root::LocalMachine, &japanese, "", "IME for Japanese");
    registry.set(
        Root::LocalMachine,
        &format!(r"{japanese}\InprocServer32"),
        "",
        r"C:\Windows\System32\IME\IMEJP.DLL",
    );

    // -- the settings of the user: the lists of the input methods and the default one
    registry.set(
        Root::CurrentUser,
        r"Control Panel\Input Method\Preload",
        "",
        &format!("1:0409:{OTHER_CLSID}\r\n2:0804:{OTHER_CLSID}\r\n3:0411:{OTHER_CLSID_JA}\r\n"),
    );
    registry.set(
        Root::CurrentUser,
        r"Keyboard Layout\Preload",
        "",
        "00000409\r\n",
    );
    registry.set(
        Root::CurrentUser,
        r"Control Panel\Input Method\Hot Keys",
        "",
        "00000409\r\n",
    );
    registry.set(
        Root::CurrentUser,
        &format!(r"Software\Microsoft\CTF\Assemblies\{OTHER_CLSID}"),
        "",
        "Default IME",
    );
    registry.set(
        Root::CurrentUser,
        r"Software\Microsoft\CTF\LangInfo",
        "Default",
        "0409",
    );

    // -- the same three input methods on the side of TSF (the profiles and the categories of the other vendors)
    registry.profiles.insert(
        (guid_value(OTHER_CLSID), 0x0804, guid_value(OTHER_PROFILE)),
        "Microsoft Pinyin".to_string(),
    );
    registry.profiles.insert(
        (
            guid_value(OTHER_CLSID_JA),
            0x0411,
            guid_value(OTHER_PROFILE_JA),
        ),
        "IME for Japanese".to_string(),
    );
    registry.categories.insert((
        guid_value(OTHER_CLSID),
        0x13a016df_560b_46cd_947a_4c3af1e0e35d,
    ));
    registry.text_services.insert(guid_value(OTHER_CLSID));
    registry.text_services.insert(guid_value(OTHER_CLSID_JA));
    registry
}

/// The backend of the test: the model of the registry behind the `Backend` contract of the registration.
#[derive(Default)]
struct ModelBackend {
    registry: ModelRegistry,
    /// every mutating call: what was done and the key it named (None when the call has no key).
    writes: Vec<(String, Option<RegKey>)>,
    /// the number of the mutating call that fails (1-based), counted from the start of the run.
    fail_at: Option<usize>,
}

impl ModelBackend {
    /// A mutating call: it is recorded, and it fails when it is the one the test injects the failure into.
    fn mutating(&mut self, what: &str, key: Option<&RegKey>) -> Result<(), BackendError> {
        self.writes.push((what.to_string(), key.cloned()));
        if self.fail_at == Some(self.writes.len()) {
            // ERROR_ACCESS_DENIED: the step is refused by Windows.
            return Err(BackendError::Failed(windows::core::HRESULT::from_win32(5)));
        }
        Ok(())
    }

    /// Everything that does not belong to this product, as a sorted list of lines: the comparison of the design case.
    fn foreign_state(&self) -> Vec<String> {
        let mut out = Vec::new();
        for ((root, path), values) in &self.registry.values {
            let key = RegKey {
                root: *root,
                path: path.clone(),
            };
            if is_own_key(&key) {
                continue;
            }
            for (name, value) in values {
                out.push(format!("{root:?} {path} [{name}] = {value}"));
            }
        }
        for ((clsid, langid, profile), description) in &self.registry.profiles {
            if *clsid == OUR_CLSID {
                continue;
            }
            out.push(format!(
                "profile {clsid:032x} {langid:#06x} {profile:032x} = {description}"
            ));
        }
        for (clsid, category) in &self.registry.categories {
            if *clsid == OUR_CLSID {
                continue;
            }
            out.push(format!("category {clsid:032x} {category:032x}"));
        }
        for clsid in &self.registry.text_services {
            if *clsid == OUR_CLSID {
                continue;
            }
            out.push(format!("text service {clsid:032x}"));
        }
        out.sort();
        out
    }

    /// Whether a key of this product is left in the model.
    fn own_keys_left(&self) -> Vec<String> {
        self.registry
            .values
            .keys()
            .filter(|(root, path)| {
                is_own_key(&RegKey {
                    root: *root,
                    path: path.clone(),
                })
            })
            .map(|(root, path)| format!("{root:?} {path}"))
            .collect()
    }
}

impl Backend for ModelBackend {
    fn read_string(&self, key: &RegKey, name: &str) -> Option<String> {
        self.registry
            .values
            .get(&(key.root, key.path.clone()))
            .and_then(|values| values.get(name))
            .cloned()
    }

    fn profile_registered(&mut self, profile: &ProfileSpec) -> bool {
        self.registry
            .profiles
            .contains_key(&(OUR_CLSID, profile.langid, profile.guid.to_u128()))
    }

    fn category_registered(&mut self, category: &CategorySpec) -> bool {
        self.registry
            .categories
            .contains(&(OUR_CLSID, category.guid.to_u128()))
    }

    fn set_string(&mut self, key: &RegKey, name: &str, value: &str) -> Result<(), BackendError> {
        self.mutating("set", Some(key))?;
        self.registry
            .values
            .entry((key.root, key.path.clone()))
            .or_default()
            .insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete_value(&mut self, key: &RegKey, name: &str) -> Result<(), BackendError> {
        self.mutating("delete value", Some(key))?;
        match self
            .registry
            .values
            .get_mut(&(key.root, key.path.clone()))
            .and_then(|values| values.remove(name))
        {
            Some(_) => Ok(()),
            None => Err(BackendError::NotFound),
        }
    }

    fn delete_tree(&mut self, key: &RegKey) -> Result<(), BackendError> {
        self.mutating("delete tree", Some(key))?;
        let prefix = key.path.to_lowercase();
        let doomed: Vec<(Root, String)> = self
            .registry
            .values
            .keys()
            .filter(|(root, path)| {
                *root == key.root
                    && (path.to_lowercase() == prefix
                        || path.to_lowercase().starts_with(&format!("{prefix}\\")))
            })
            .cloned()
            .collect();
        if doomed.is_empty() {
            return Err(BackendError::NotFound);
        }
        for entry in doomed {
            self.registry.values.remove(&entry);
        }
        Ok(())
    }

    fn register_profile(&mut self, profile: &ProfileSpec) -> Result<(), BackendError> {
        self.mutating("register profile", None)?;
        self.registry.profiles.insert(
            (OUR_CLSID, profile.langid, profile.guid.to_u128()),
            profile.description.to_string(),
        );
        self.registry.text_services.insert(OUR_CLSID);
        Ok(())
    }

    fn unregister_profile(&mut self, profile: &ProfileSpec) -> Result<(), BackendError> {
        self.mutating("unregister profile", None)?;
        match self
            .registry
            .profiles
            .remove(&(OUR_CLSID, profile.langid, profile.guid.to_u128()))
        {
            Some(_) => Ok(()),
            None => Err(BackendError::NotFound),
        }
    }

    fn register_category(&mut self, category: &CategorySpec) -> Result<(), BackendError> {
        self.mutating("register category", None)?;
        self.registry
            .categories
            .insert((OUR_CLSID, category.guid.to_u128()));
        self.registry.text_services.insert(OUR_CLSID);
        Ok(())
    }

    fn unregister_category(&mut self, category: &CategorySpec) -> Result<(), BackendError> {
        self.mutating("unregister category", None)?;
        if self
            .registry
            .categories
            .remove(&(OUR_CLSID, category.guid.to_u128()))
        {
            Ok(())
        } else {
            Err(BackendError::NotFound)
        }
    }

    fn unregister_text_service(&mut self) -> Result<(), BackendError> {
        self.mutating("unregister text service", None)?;
        if self.registry.text_services.remove(&OUR_CLSID) {
            Ok(())
        } else {
            Err(BackendError::NotFound)
        }
    }
}

/// The backend of the test, with the machine of the design case.
fn backend() -> ModelBackend {
    ModelBackend {
        registry: machine_with_other_input_methods(),
        ..ModelBackend::default()
    }
}

/// The input method the user came from: another vendor's Chinese profile.
fn other_input_method() -> Profile {
    Profile::new(
        &pime_tsf::ime_switch::guid_to_string(
            &pime_tsf::ime_switch::string_to_guid(OTHER_CLSID).expect("a GUID"),
        ),
        &pime_tsf::ime_switch::guid_to_string(
            &pime_tsf::ime_switch::string_to_guid(OTHER_PROFILE).expect("a GUID"),
        ),
        0x0804,
    )
}

// ------------------------------------------------------------------ the tests

/// The whole case: the installation, the switch and the removal leave every other input method as it was.
#[test]
fn it_002_installing_switching_and_removing_leaves_every_other_input_method_untouched() {
    let mut backend = backend();
    let before = backend.foreign_state();
    assert!(
        !before.is_empty(),
        "the machine of the design case has the input methods of other vendors"
    );

    // -- 1. the installation
    let plan: Plan = registry_plan(DLL, Scope::Machine).expect("the plan of the installation");
    register_with(&mut backend, &plan).expect("the installation succeeds");
    assert_eq!(
        backend.foreign_state(),
        before,
        "the installation changed something that does not belong to this product"
    );
    assert!(
        !backend.own_keys_left().is_empty(),
        "our own COM keys are there"
    );

    // -- 2. the switch: the key brings our profile and brings the other one back exactly as it was
    let ours = own_profiles();
    let other = other_input_method();
    match decide(Some(&other), None, &ours) {
        Switch::ToOurs(target) => assert_eq!(
            target.langid, 0x0804,
            "the switch goes to the profile of the language that is active now"
        ),
        Switch::Nowhere => panic!("the switch must have a target"),
        Switch::ToPrevious(target) => panic!("the switch must not go back: {target:?}"),
    }
    assert_eq!(
        remember(Some(&other), None, &ours),
        Some(other.clone()),
        "the input method we came from is remembered exactly as it was"
    );
    match decide(Some(&ours[0]), Some(&other), &ours) {
        Switch::ToPrevious(target) => assert_eq!(
            target, other,
            "the second press brings the very same input method back"
        ),
        other_switch => {
            panic!("the switch must go back to the previous input method: {other_switch:?}")
        }
    }
    assert_eq!(
        backend.foreign_state(),
        before,
        "the switch changed something that does not belong to this product"
    );

    // -- 3. the removal
    unregister_with(&mut backend, &removal_plan(Scope::Machine)).expect("the removal succeeds");
    assert_eq!(
        backend.foreign_state(),
        before,
        "the removal changed something that does not belong to this product"
    );
    assert!(
        backend.own_keys_left().is_empty(),
        "nothing of ours is left: {:?}",
        backend.own_keys_left()
    );
    assert!(
        !backend
            .registry
            .profiles
            .keys()
            .any(|(clsid, _, _)| *clsid == OUR_CLSID),
        "our own profiles are gone"
    );
    assert!(
        !backend
            .registry
            .categories
            .iter()
            .any(|(clsid, _)| *clsid == OUR_CLSID),
        "our own categories are gone"
    );
    assert!(
        !backend.registry.text_services.contains(&OUR_CLSID),
        "our own text service is gone"
    );

    // -- and no step of the run ever named a key of another product
    for (what, key) in &backend.writes {
        if let Some(key) = key {
            assert!(
                is_own_key(key),
                "{what} named a key that does not belong to this product: {key:?}"
            );
        }
    }
}

/// The plan of the installation names our own keys and nothing else - not the settings of the user.
#[test]
fn the_plan_of_the_installation_names_only_our_own_keys() {
    for scope in [Scope::Machine, Scope::User] {
        let plan = registry_plan(DLL, scope).expect("the plan of the installation");
        for op in &plan.registry {
            assert!(
                is_own_key(&op.key),
                "{scope:?}: the plan writes a key that is not ours: {op:?}"
            );
            let path = op.key.path.to_lowercase();
            assert!(
                !path.contains("control panel") && !path.contains("keyboard layout"),
                "{scope:?}: the plan touches the settings of the input methods of the user: {op:?}"
            );
        }
    }
}

/// A failed installation rolls back: the other input methods are still untouched.
#[test]
fn a_failed_installation_leaves_the_other_input_methods_untouched() {
    let mut backend = backend();
    let before = backend.foreign_state();
    backend.fail_at = Some(2); // the second step of the run is refused by Windows
    let plan = registry_plan(DLL, Scope::Machine).expect("the plan of the installation");
    let error = register_with(&mut backend, &plan).expect_err("the run fails");
    assert_eq!(
        error.rollback_failures.len(),
        0,
        "the rollback itself went through"
    );
    assert_eq!(
        backend.foreign_state(),
        before,
        "the failed installation changed something that does not belong to this product"
    );
    assert!(
        backend.own_keys_left().is_empty(),
        "the rollback took our own keys away again: {:?}",
        backend.own_keys_left()
    );
}

/// The three profiles of the product are one per language, and the removal names the same three.
#[test]
fn the_three_profiles_of_the_product_are_the_same_before_and_after_the_installation() {
    let plan = registry_plan(DLL, Scope::Machine).expect("the plan of the installation");
    let removal = removal_plan(Scope::Machine);
    let languages = |plan: &Plan| -> Vec<u16> { plan.profiles.iter().map(|p| p.langid).collect() };
    assert_eq!(languages(&plan), vec![0x0804, 0x0409, 0x0411]);
    assert_eq!(languages(&plan), languages(&removal));
}
