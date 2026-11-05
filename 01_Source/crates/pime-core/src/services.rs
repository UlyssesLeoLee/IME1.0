//! The services the input method will talk to: the AI that tidies the dictionaries, the remote update of the
//! dictionaries and the account of the user.
//!
//! They are **interfaces without an implementation**. The shapes are decided here, so that the code that needs them
//! can be written and tested against them, and so that the way they will work is written down before somebody writes
//! them. Nothing in the product calls them yet, and nothing pretends that they work.
//!
//! The reason they are only shapes: an input method that needs a dictionary of its own must work **without** a
//! network and **without** an account. The dictionaries that ship with the product are the ones the user has on the
//! first day; these services are what comes later (a bigger dictionary, a dictionary that learns from the words the
//! user did not find, an account that carries the settings from one computer to another).
//!
//! The three services are independent of each other: the account is not needed for the update, and the AI is not
//! needed for either.

use std::fmt;

/// What went wrong in a service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceError {
    /// The service is not there: no network, the feature is turned off, or the user is not signed in.
    Unavailable,
    /// The answer of the service is not one this build understands (a newer version of the service).
    NotUnderstood(String),
    /// The service answered, but it refused the request.
    Refused(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Unavailable => write!(f, "the service is not available"),
            ServiceError::NotUnderstood(why) => write!(f, "the answer is not understood: {why}"),
            ServiceError::Refused(why) => write!(f, "the service refused: {why}"),
        }
    }
}

impl std::error::Error for ServiceError {}

/// Which dictionary a request is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictionaryKind {
    /// The Chinese characters (the pinyin table and the phrases).
    Pinyin,
    /// The Japanese words (the kana table and the words).
    Kana,
    /// The English words.
    English,
}

impl DictionaryKind {
    /// The name of the dictionary, as the file of the product names it.
    pub fn file_name(self) -> &'static str {
        match self {
            DictionaryKind::Pinyin => "pinyin",
            DictionaryKind::Kana => "kana",
            DictionaryKind::English => "english",
        }
    }
}

/// The request the AI gets: the words the user typed and did not find.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryRequest {
    /// The dictionary to tidy.
    pub kind: DictionaryKind,
    /// The words the user typed and did not find (the ones that are worth a candidate).
    pub missing: Vec<String>,
    /// How many words the AI may propose.
    pub limit: usize,
}

/// One word the AI proposes, with the reason it proposes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The word itself.
    pub word: String,
    /// Its reading (the pinyin or the kana).
    pub reading: String,
    /// Why the AI proposes it (the model said so; the product does not check it).
    pub reason: String,
}

/// What the AI answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryReport {
    /// The words it proposes.
    pub proposals: Vec<Proposal>,
    /// The version of the dictionary the proposals are for.
    pub base_version: String,
}

/// The dictionary of the next version, as the update service offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryRelease {
    /// The version of the dictionary.
    pub version: String,
    /// Which dictionary it is.
    pub kind: DictionaryKind,
    /// How big it is (the product checks it before it writes it).
    pub bytes: u64,
    /// The checksum of the file (the product checks it too: a dictionary that is not the one it asked for is
    /// thrown away).
    pub sha256: String,
    /// Where it is.
    pub url: String,
}

/// The account of the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// The name of the account.
    pub account: String,
    /// The secret of the user. It is never written to a file of the product in clear text.
    pub secret: String,
}

/// The session of a signed-in user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The token of the session.
    pub token: String,
    /// When it ends (the seconds since the epoch).
    pub expires_at: u64,
}

/// The service that tidies the dictionaries with the AI.
///
/// The AI proposes words; the product decides whether they go into the dictionary (the words the user did not find
/// are the ones that are worth a candidate, and nothing else is).
pub trait DictionaryCurator {
    /// Ask the AI for the words that are missing from the dictionary.
    fn curate(&self, request: &DictionaryRequest) -> Result<DictionaryReport, ServiceError>;
}

/// The service that offers the dictionaries of the next version.
///
/// The download is a separate call from the question about the newest version: the product asks first (it has to
/// know whether there is anything to download), and downloads only when there is.
pub trait DictionaryUpdater {
    /// The newest version of the dictionary, if there is one that is newer than the one the product has.
    fn latest(&self, kind: DictionaryKind) -> Result<Option<DictionaryRelease>, ServiceError>;
    /// The bytes of the release.
    fn download(&self, release: &DictionaryRelease) -> Result<Vec<u8>, ServiceError>;
}

/// The service of the account of the user.
///
/// The account is what carries the settings of the user from one computer to another (the way Sogou does it), and
/// what the user signs in with to use the services above. It is optional: the product works without it.
pub trait AccountService {
    /// Sign in and get a session.
    fn sign_in(&self, credentials: &Credentials) -> Result<Session, ServiceError>;
    /// Give the session back.
    fn sign_out(&self, session: &Session) -> Result<(), ServiceError>;
    /// The account the session belongs to.
    fn who(&self, session: &Session) -> Result<String, ServiceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A service that is not there: the state of every one of them before it is written.
    struct Nothing;

    impl DictionaryCurator for Nothing {
        fn curate(&self, _request: &DictionaryRequest) -> Result<DictionaryReport, ServiceError> {
            Err(ServiceError::Unavailable)
        }
    }

    impl DictionaryUpdater for Nothing {
        fn latest(&self, _kind: DictionaryKind) -> Result<Option<DictionaryRelease>, ServiceError> {
            Err(ServiceError::Unavailable)
        }
        fn download(&self, _release: &DictionaryRelease) -> Result<Vec<u8>, ServiceError> {
            Err(ServiceError::Unavailable)
        }
    }

    impl AccountService for Nothing {
        fn sign_in(&self, _credentials: &Credentials) -> Result<Session, ServiceError> {
            Err(ServiceError::Unavailable)
        }
        fn sign_out(&self, _session: &Session) -> Result<(), ServiceError> {
            Err(ServiceError::Unavailable)
        }
        fn who(&self, _session: &Session) -> Result<String, ServiceError> {
            Err(ServiceError::Unavailable)
        }
    }

    /// The three services can be used as objects: the code that needs them does not have to know how they are made.
    #[test]
    fn the_services_can_be_used_as_objects() {
        let curator: Box<dyn DictionaryCurator> = Box::new(Nothing);
        let updater: Box<dyn DictionaryUpdater> = Box::new(Nothing);
        let account: Box<dyn AccountService> = Box::new(Nothing);
        let request = DictionaryRequest {
            kind: DictionaryKind::Pinyin,
            missing: vec!["你好".to_string()],
            limit: 10,
        };
        assert_eq!(
            curator.curate(&request),
            Err(ServiceError::Unavailable),
            "a service that is not there says so"
        );
        assert_eq!(
            updater.latest(DictionaryKind::Pinyin),
            Err(ServiceError::Unavailable)
        );
        assert_eq!(
            account.sign_in(&Credentials {
                account: "someone".to_string(),
                secret: "secret".to_string(),
            }),
            Err(ServiceError::Unavailable)
        );
    }

    /// The dictionaries are the three the product has, and their names are the names of their files.
    #[test]
    fn the_three_dictionaries_are_the_three_of_the_product() {
        assert_eq!(DictionaryKind::Pinyin.file_name(), "pinyin");
        assert_eq!(DictionaryKind::Kana.file_name(), "kana");
        assert_eq!(DictionaryKind::English.file_name(), "english");
    }
}
