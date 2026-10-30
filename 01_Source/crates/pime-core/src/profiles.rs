//! DD-01 section 3.1: Windows language profile table (UT-001). Pure data and validation; no Windows API.
//!
//! The CLSID and the three profile GUIDs are the TEMPORARY prototype values that already appear in
//! `windows/tsf/PimeTsf.cpp` (see `windows/tsf/README.md`: "The prototype GUID values are temporary until
//! product-release ID governance"). They must be replaced by frozen, build-manifest GUIDs before release.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
        Guid {
            data1,
            data2,
            data3,
            data4,
        }
    }

    /// `{43F95ED1-95B4-421D-8F5A-837128C2D854}` (the form used in the registry).
    pub fn to_braced_string(&self) -> String {
        let d = &self.data4;
        format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }

    pub fn parse(s: &str) -> Option<Guid> {
        let t = s.strip_prefix('{')?.strip_suffix('}')?;
        let parts: Vec<&str> = t.split('-').collect();
        if parts.len() != 5
            || parts
                .iter()
                .zip([8, 4, 4, 4, 12])
                .any(|(p, n)| p.len() != n || !p.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return None;
        }
        let tail = format!("{}{}", parts[3], parts[4]);
        let mut data4 = [0u8; 8];
        for (i, b) in data4.iter_mut().enumerate() {
            *b = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Guid::new(
            u32::from_str_radix(parts[0], 16).ok()?,
            u16::from_str_radix(parts[1], 16).ok()?,
            u16::from_str_radix(parts[2], 16).ok()?,
            data4,
        ))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_braced_string())
    }
}

/// `PIME_CLSID` (one COM class shared by all three profiles).
pub const PIME_CLSID: Guid = Guid::new(
    0x43f95ed1,
    0x95b4,
    0x421d,
    [0x8f, 0x5a, 0x83, 0x71, 0x28, 0xc2, 0xd8, 0x54],
);
pub const PROFILE_ZH: Guid = Guid::new(
    0x223fd919,
    0xf8cb,
    0x476c,
    [0xa8, 0x52, 0x35, 0x1e, 0x73, 0x81, 0xc8, 0x01],
);
pub const PROFILE_EN: Guid = Guid::new(
    0x68ed92de,
    0x77cf,
    0x43b1,
    [0xa7, 0xb2, 0xd3, 0xbe, 0x26, 0x86, 0xe2, 0xa0],
);
pub const PROFILE_JA: Guid = Guid::new(
    0x22e54b52,
    0x5082,
    0x42d3,
    [0x9e, 0x41, 0x1c, 0x2e, 0x0c, 0xe7, 0x4d, 0xfb],
);

pub const LANGID_ZH_CN: u16 = 0x0804;
pub const LANGID_EN_US: u16 = 0x0409;
pub const LANGID_JA_JP: u16 = 0x0411;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageProfile {
    pub name: &'static str,
    pub langid: u16,
    pub clsid: Guid,
    pub profile: Guid,
    pub description: &'static str,
}

/// The three profiles of the product: zh-CN 0x0804, en-US 0x0409, ja-JP 0x0411.
pub fn builtin_profiles() -> [LanguageProfile; 3] {
    [
        LanguageProfile {
            name: "zh-CN",
            langid: LANGID_ZH_CN,
            clsid: PIME_CLSID,
            profile: PROFILE_ZH,
            description: "Programming IME (zh-CN)",
        },
        LanguageProfile {
            name: "en-US",
            langid: LANGID_EN_US,
            clsid: PIME_CLSID,
            profile: PROFILE_EN,
            description: "Programming IME (en-US)",
        },
        LanguageProfile {
            name: "ja-JP",
            langid: LANGID_JA_JP,
            clsid: PIME_CLSID,
            profile: PROFILE_JA,
            description: "Programming IME (ja-JP)",
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    Empty,
    DuplicateProfileGuid(Guid),
    DuplicateLangId(u16),
    /// Profiles of one product must share a single CLSID.
    ClsidMismatch,
    /// A profile GUID may not equal the CLSID.
    ProfileEqualsClsid(Guid),
}

/// Checks a set of profiles: unique profile GUIDs, unique LANGIDs, one shared CLSID, no GUID reuse.
pub fn validate_profile_set(profiles: &[LanguageProfile]) -> Result<(), ProfileError> {
    let Some(first) = profiles.first() else {
        return Err(ProfileError::Empty);
    };
    for (i, p) in profiles.iter().enumerate() {
        if p.clsid != first.clsid {
            return Err(ProfileError::ClsidMismatch);
        }
        if p.profile == p.clsid {
            return Err(ProfileError::ProfileEqualsClsid(p.profile));
        }
        for q in &profiles[..i] {
            if q.profile == p.profile {
                return Err(ProfileError::DuplicateProfileGuid(p.profile));
            }
            if q.langid == p.langid {
                return Err(ProfileError::DuplicateLangId(p.langid));
            }
        }
    }
    Ok(())
}

pub fn is_own_guid(g: &Guid) -> bool {
    *g == PIME_CLSID || builtin_profiles().iter().any(|p| p.profile == *g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ut_001_three_profiles_with_expected_langids_one_clsid_distinct_guids() {
        let p = builtin_profiles();
        assert_eq!(
            (p[0].langid, p[1].langid, p[2].langid),
            (0x0804, 0x0409, 0x0411)
        );
        assert_eq!(
            (p[0].name, p[1].name, p[2].name),
            ("zh-CN", "en-US", "ja-JP")
        );
        assert!(
            p.iter().all(|x| x.clsid == PIME_CLSID),
            "all profiles share the same CLSID"
        );
        assert_ne!(p[0].profile, p[1].profile);
        assert_ne!(p[0].profile, p[2].profile);
        assert_ne!(p[1].profile, p[2].profile);
        assert!(p.iter().all(|x| x.profile != PIME_CLSID));
        assert_eq!(validate_profile_set(&p), Ok(()));
    }

    #[test]
    fn ut_001_duplicate_guid_is_rejected() {
        let mut p = builtin_profiles().to_vec();
        let dup = LanguageProfile {
            name: "xx",
            langid: 0x0407,
            ..p[0]
        };
        p.push(dup);
        assert_eq!(
            validate_profile_set(&p),
            Err(ProfileError::DuplicateProfileGuid(PROFILE_ZH))
        );
    }

    #[test]
    fn ut_001_other_invalid_sets_are_rejected() {
        let b = builtin_profiles();
        assert_eq!(validate_profile_set(&[]), Err(ProfileError::Empty));
        let other_clsid = LanguageProfile {
            clsid: PROFILE_EN,
            ..b[1]
        };
        assert_eq!(
            validate_profile_set(&[b[0], other_clsid]),
            Err(ProfileError::ClsidMismatch)
        );
        let same_lang = LanguageProfile {
            profile: Guid::new(1, 2, 3, [0; 8]),
            ..b[0]
        };
        assert_eq!(
            validate_profile_set(&[b[0], same_lang]),
            Err(ProfileError::DuplicateLangId(0x0804))
        );
        let clsid_as_profile = LanguageProfile {
            profile: PIME_CLSID,
            ..b[0]
        };
        assert_eq!(
            validate_profile_set(&[clsid_as_profile]),
            Err(ProfileError::ProfileEqualsClsid(PIME_CLSID))
        );
    }

    #[test]
    fn guid_text_round_trip() {
        assert_eq!(
            PIME_CLSID.to_braced_string(),
            "{43F95ED1-95B4-421D-8F5A-837128C2D854}"
        );
        for g in [PIME_CLSID, PROFILE_ZH, PROFILE_EN, PROFILE_JA] {
            assert_eq!(Guid::parse(&g.to_braced_string()), Some(g));
        }
        assert_eq!(Guid::parse("43F95ED1-95B4-421D-8F5A-837128C2D854"), None);
        assert_eq!(Guid::parse("{43F95ED1-95B4-421D-8F5A-837128C2D85}"), None);
    }
}
