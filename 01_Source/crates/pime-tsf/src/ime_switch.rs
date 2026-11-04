//! Switching the active input method: the one-key switch between IME1.0 and the input method the user came from.
//!
//! The user is writing in the Japanese input method and wants to write code: one key (Ctrl+F12) brings IME1.0, the
//! same key brings the Japanese input method back. The decision is pure (`pime_core::imeswitch`); this module is the
//! Windows part: which profile is active now, how another one is activated, and where the profile we came from is
//! remembered between the presses.
//!
//! Everything that can fail degrades to "nothing happened": a switch that cannot be read or written must never
//! leave the user without an input method, and it must never take the focus away from the program they are writing
//! in.

use pime_core::imeswitch::{self, Profile, Switch};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::UI::TextServices::{ITfInputProcessorProfiles, TF_PROFILETYPE_INPUTPROCESSOR};

use crate::{CLSID_PIME, PROFILE_EN, PROFILE_JA, PROFILE_ZH};

/// The registry key where the profile we came from is remembered.
const SUBKEY: &str = "Software\\IME1.0";
/// The value that holds it.
const VALUE: &str = "LastProfile";

/// The text of a GUID in the form Windows shows it in: `{43f95ed1-95b4-421d-8f5a-837128c2d854}`.
pub fn guid_to_string(guid: &GUID) -> String {
    let u = guid.to_u128();
    format!(
        "{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}",
        (u >> 96) as u32,
        (u >> 80) & 0xffff,
        (u >> 64) & 0xffff,
        (u >> 48) & 0xffff,
        u & 0xffff_ffff_ffff
    )
}

/// The GUID of a text in the form Windows shows it in. None for anything that is not one.
pub fn string_to_guid(text: &str) -> Option<GUID> {
    let s = text.trim().trim_start_matches('{').trim_end_matches('}');
    let (d1, rest) = s.split_once('-')?;
    let (d2, rest) = rest.split_once('-')?;
    let (d3, rest) = rest.split_once('-')?;
    let (d4, d5) = rest.split_once('-')?;
    if d1.len() != 8 || d2.len() != 4 || d3.len() != 4 || d4.len() != 4 || d5.len() != 12 {
        return None;
    }
    let d1 = u32::from_str_radix(d1, 16).ok()?;
    let d2 = u16::from_str_radix(d2, 16).ok()?;
    let d3 = u16::from_str_radix(d3, 16).ok()?;
    let d4 = u16::from_str_radix(d4, 16).ok()?;
    let d5 = u64::from_str_radix(d5, 16).ok()?;
    Some(GUID::from_u128(
        ((d1 as u128) << 96)
            | ((d2 as u128) << 80)
            | ((d3 as u128) << 64)
            | ((d4 as u128) << 48)
            | d5 as u128,
    ))
}

/// The profiles of IME1.0 itself.
pub fn own_profiles() -> Vec<Profile> {
    imeswitch::own_profiles(
        &guid_to_string(&CLSID_PIME),
        &guid_to_string(&PROFILE_ZH),
        &guid_to_string(&PROFILE_EN),
        &guid_to_string(&PROFILE_JA),
    )
}

/// The profile manager of TSF.
fn profiles() -> Option<ITfInputProcessorProfiles> {
    // SAFETY: a plain in-process object creation; the interface is released when it goes out of scope.
    let created = unsafe {
        CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &windows::Win32::UI::TextServices::CLSID_TF_InputProcessorProfiles,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    };
    created.ok()
}

/// The profile that is active now, if TSF can say.
///
/// Every text service is asked in turn, because TSF has no "who is active" call that does not need the class of the
/// text service in question.
pub fn active_profile() -> Option<Profile> {
    let manager = profiles()?;
    // SAFETY: the manager is a live object; `EnumInputProcessorInfo` returns an owned enumerator.
    let enumerator = unsafe { manager.EnumInputProcessorInfo() }.ok()?;
    let mut clsid = GUID::zeroed();
    // SAFETY: the slice is writable and holds exactly one GUID.
    let fetched = unsafe { enumerator.Next(std::slice::from_mut(&mut clsid), None) };
    if fetched.is_err() {
        return None;
    }
    let mut langid = 0u16;
    let mut profile = GUID::zeroed();
    // SAFETY: both out pointers are live locals.
    let ok = unsafe { manager.GetActiveLanguageProfile(&clsid, &mut langid, &mut profile) };
    if ok.is_err() {
        return None;
    }
    Some(Profile::new(
        &guid_to_string(&clsid),
        &guid_to_string(&profile),
        langid,
    ))
}

/// Activate a profile of a text service.
pub fn activate(profile: &Profile) -> Result<(), SwitchError> {
    let Some(clsid) = string_to_guid(&profile.clsid) else {
        return Err(SwitchError::NotAProfile);
    };
    let Some(guid) = string_to_guid(&profile.profile) else {
        return Err(SwitchError::NotAProfile);
    };
    let manager = profiles().ok_or(SwitchError::NoManager)?;
    // SAFETY: the manager is live and both GUIDs are valid locals.
    unsafe { manager.ActivateLanguageProfile(&clsid, profile.langid, &guid) }
        .map_err(|_| SwitchError::Refused)
}

/// Why a switch did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchError {
    /// The text of the profile was not a GUID.
    NotAProfile,
    /// TSF could not be reached.
    NoManager,
    /// TSF refused the activation (the profile is not enabled for this user, for example).
    Refused,
}

/// The profile the switch goes to, and the one to remember afterwards.
///
/// None when there is nowhere to go: the key must never leave the user without an input method.
pub fn next_switch() -> Option<(Profile, Option<Profile>)> {
    let ours = own_profiles();
    let current = active_profile();
    let previous = remembered_profile();
    let target = match imeswitch::decide(current.as_ref(), previous.as_ref(), &ours) {
        Switch::ToOurs(p) | Switch::ToPrevious(p) => p,
        Switch::Nowhere => return None,
    };
    let to_remember = imeswitch::remember(current.as_ref(), previous.as_ref(), &ours);
    Some((target, to_remember))
}

/// Switch now: activate the profile the switch goes to and remember where the user came from.
///
/// Returns the profile that is active now (the one that was switched to).
pub fn switch_now() -> Option<Profile> {
    let (target, to_remember) = next_switch()?;
    if let Some(p) = to_remember {
        remember_profile(&p);
    } else {
        forget_profile();
    }
    activate(&target).ok()?;
    Some(target)
}

/// The profile the user came from, as it was remembered.
pub fn remembered_profile() -> Option<Profile> {
    read_value().and_then(|text| Profile::from_record(&text))
}

/// Remember the profile the user came from.
pub fn remember_profile(profile: &Profile) {
    write_value(&profile.to_record());
}

/// Forget the profile the user came from.
pub fn forget_profile() {
    delete_value();
}

/// The registry key of the settings, created if it is not there.
fn open_key(write: bool) -> Option<HKEY> {
    let subkey: Vec<u16> = SUBKEY.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = HKEY::default();
    // SAFETY: `subkey` is NUL-terminated and outlives the call; `handle` is a live out pointer.
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            if write { KEY_SET_VALUE } else { KEY_QUERY_VALUE },
            None,
            &mut handle,
            None,
        )
    };
    if created != ERROR_SUCCESS {
        return None;
    }
    Some(handle)
}

/// Read the remembered profile.
fn read_value() -> Option<String> {
    let handle = open_key(false)?;
    let value: Vec<u16> = VALUE.encode_utf16().chain(std::iter::once(0)).collect();
    let mut size = 0u32;
    // SAFETY: the key is live and the value name is NUL-terminated; no data is read yet.
    let read = unsafe {
        RegGetValueW(
            handle,
            PCWSTR::null(),
            PCWSTR(value.as_ptr()),
            windows::Win32::System::Registry::RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if read != ERROR_SUCCESS || size == 0 {        // SAFETY: closes the handle opened above, once.
        let _ = unsafe { RegCloseKey(handle) };
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer is writable for `size` bytes and the key is live.
    let read = unsafe {
        RegGetValueW(
            handle,
            PCWSTR::null(),
            PCWSTR(value.as_ptr()),
            windows::Win32::System::Registry::RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<core::ffi::c_void>()),
            Some(&mut size),
        )
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
    if read != ERROR_SUCCESS {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..buffer.len().saturating_sub(1)]);
    let text = text.trim_end_matches('\0').trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Write the remembered profile.
fn write_value(text: &str) {
    let Some(handle) = open_key(true) else {
        return;
    };
    let value: Vec<u16> = VALUE.encode_utf16().chain(std::iter::once(0)).collect();
    let data: Vec<u8> = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    // SAFETY: the key is live, the name is NUL-terminated and the data is the UTF-16 text with its terminator.
    let _ = unsafe {
        RegSetValueExW(
            handle,
            PCWSTR(value.as_ptr()),
            0,
            REG_SZ,
            Some(data.as_slice()),
        )
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
}

/// Forget the remembered profile.
fn delete_value() {
    let Some(handle) = open_key(true) else {
        return;
    };
    let value: Vec<u16> = VALUE.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the key is live and the value name is NUL-terminated.
    let _ = unsafe { windows::Win32::System::Registry::RegDeleteValueW(handle, PCWSTR(value.as_ptr())) };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
}

/// The profile type of a text service (TSF calls it a category).
pub const CATEGORY_INPUT_PROCESSOR: usize = TF_PROFILETYPE_INPUTPROCESSOR as usize;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guid_survives_the_round_trip_through_its_text() {
        for guid in [CLSID_PIME, PROFILE_ZH, PROFILE_EN, PROFILE_JA] {
            let text = guid_to_string(&guid);
            assert!(text.starts_with('{') && text.ends_with('}'), "{text}");
            assert_eq!(string_to_guid(&text), Some(guid), "{text}");
        }
    }

    #[test]
    fn the_text_of_a_guid_is_the_one_windows_shows() {
        assert_eq!(
            guid_to_string(&CLSID_PIME),
            "{43f95ed1-95b4-421d-8f5a-837128c2d854}"
        );
        assert_eq!(
            guid_to_string(&PROFILE_ZH),
            "{223fd919-f8cb-476c-a852-351e7381c801}"
        );
    }

    #[test]
    fn text_that_is_not_a_guid_is_refused() {
        for text in [
            "",
            "not a guid",
            "{43f95ed1-95b4-421d-8f5a}",
            "{43f95ed1-95b4-421d-8f5a-837128c2d854-extra}",
            "{zzzzzzzz-95b4-421d-8f5a-837128c2d854}",
        ] {
            assert_eq!(string_to_guid(text), None, "{text}");
        }
    }

    #[test]
    fn our_own_profiles_are_the_three_languages() {
        let ours = own_profiles();
        assert_eq!(ours.len(), 3);
        assert!(ours.iter().all(|p| p.is_of(&guid_to_string(&CLSID_PIME))));
        let languages: Vec<u16> = ours.iter().map(|p| p.langid).collect();
        assert_eq!(languages, vec![0x0804, 0x0409, 0x0411]);
    }
}
