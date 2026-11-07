//! The start of the Hub: the namespace of the logon session, the single instance and the pipe.
//!
//! The Hub is **one process per login session** (detailed design §17.2, basic design §16.2). The
//! namespace of that session is its **logon SID**: the name of the pipe and the name of the mutex are
//! computed out of it, so that two sessions of the same user never meet and a process of another
//! session cannot even open the pipe.
//!
//! The rules of the module:
//!
//! * **one instance.** [`start_or_connect`] tries to become the owner of the pipe of the namespace.
//!   When the name is already taken, another Hub is running: the caller connects to it instead of
//!   starting a second process that would write into the same files. The mutex of the namespace is
//!   computed and logged; the ownership of the pipe is what enforces it, because the first instance of
//!   a pipe name belongs to one process only.
//! * **the identity is injected.** The logon SID of the process is read by the Windows part
//!   ([`platform_identity`]), but the rule of the namespace is a function of the identity, so a caller
//!   may state one ([`StaticSessionIdentity`]) and a test may drive it.
//! * **the transport is a seam.** The pipe itself comes from `tase-local-ipc`; a test replaces it with
//!   [`PipeFactory`], so the rules of the singleton can be checked without a pipe.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tase_contract::TaseError;
use tase_local_ipc::{FrameError, PeerIdentity, Transport};

/// The prefix of the pipe of the Hub, as the IPC crate of TASE writes it.
pub const PIPE_PREFIX: &str = tase_local_ipc::acl::PIPE_PREFIX;

/// The name of the file of the database of the Hub, inside the data directory.
pub const STORE_FILE_NAME: &str = "tase-hub.db";

/// The configuration of the Hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubConfig {
    /// The prefix of the pipe of the Hub.
    ///
    /// The name of the pipe is owned by `tase-local-ipc` (the transport of the Hub is the one of that
    /// crate), so the prefix of the configuration must be the one that crate uses; another prefix is
    /// refused at the door by [`HubConfig::validate`] rather than silently ignored.
    pub pipe_prefix: String,
    /// The directory where the Hub keeps its data (the database, the logs).
    pub data_dir: PathBuf,
    /// The most connections at the same time (detailed design §17.5: 64).
    pub max_connections: u32,
    /// The most jobs that are not finished at the same time (detailed design §17.5: 16).
    pub max_concurrent_jobs: u32,
}

impl Default for HubConfig {
    fn default() -> HubConfig {
        HubConfig {
            pipe_prefix: PIPE_PREFIX.to_string(),
            data_dir: default_data_dir(),
            max_connections: crate::defaults::MAX_CONNECTIONS,
            max_concurrent_jobs: crate::defaults::MAX_CONCURRENT_JOBS,
        }
    }
}

impl HubConfig {
    /// The configuration of the Hub, with the data directory of the caller.
    #[must_use]
    pub fn with_data_dir(data_dir: impl Into<PathBuf>) -> HubConfig {
        HubConfig {
            data_dir: data_dir.into(),
            ..HubConfig::default()
        }
    }

    /// Checks the configuration.
    pub fn validate(&self) -> Result<(), TaseError> {
        if self.pipe_prefix != PIPE_PREFIX {
            return Err(TaseError::invalid_argument(format!(
                "the prefix of the pipe of the hub is {:?}, but the transport of the hub uses {:?}",
                self.pipe_prefix, PIPE_PREFIX
            )));
        }
        if self.max_connections == 0 {
            return Err(TaseError::invalid_argument("the hub takes no connection"));
        }
        if self.max_concurrent_jobs == 0 {
            return Err(TaseError::invalid_argument("the hub runs no job"));
        }
        Ok(())
    }

    /// The name of the pipe of the namespace of a logon session.
    pub fn pipe_name(&self, logon_sid: &str) -> Result<String, TaseError> {
        self.validate()?;
        Ok(tase_local_ipc::pipe_name(logon_sid))
    }

    /// The name of the mutex of the namespace of a logon session.
    ///
    /// The mutex is the name the design book gives to the single instance (detailed design §17.2). The
    /// pipe itself is what enforces it: the first instance of a pipe name belongs to one process, and a
    /// second process that tries to create it fails and connects instead.
    #[must_use]
    pub fn mutex_name(&self, logon_sid: &str) -> String {
        format!("Local\\TASE-HUB-SINGLETON-{}", safe_sid(logon_sid))
    }

    /// The path of the database of the Hub.
    #[must_use]
    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join(STORE_FILE_NAME)
    }
}

/// The directory where the Hub keeps its data, when nobody says otherwise.
///
/// On Windows it is the local application data of the user (`%LOCALAPPDATA%/IDE1.0/tase`), which is
/// the directory the design book asks for: a directory of the user, on the local disk, and never on a
/// shared or network drive (detailed design §17.2).
#[must_use]
pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA")
            && !local.trim().is_empty()
        {
            return PathBuf::from(local).join("IDE1.0").join("tase");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
            && !xdg.trim().is_empty()
        {
            return PathBuf::from(xdg).join("ide1.0").join("tase");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home).join(".local").join("share").join("ide1.0").join("tase");
        }
    }
    PathBuf::from("tase-hub-data")
}

/// A SID, made safe for a name.
fn safe_sid(sid: &str) -> String {
    let mut out = String::new();
    for character in sid.trim().chars().take(64) {
        if character.is_ascii_alphanumeric() || character == '-' {
            out.push(character);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        out.push_str("unknown");
    }
    out
}

/// The identity of the logon session the Hub belongs to.
///
/// The logon SID is the namespace of the Hub (detailed design §17.2), the user SID is the rule of the
/// access control, and the session id is the rule of the terminal services.
pub trait SessionIdentity: Send {
    /// The SID of the logon session.
    fn logon_sid(&self) -> String;
    /// The SID of the user.
    fn user_sid(&self) -> String;
    /// The number of the terminal-services session.
    fn session_id(&self) -> u32;
}

/// An identity that the caller states.
///
/// It is what a test uses, and what a caller uses when the identity is known by other means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticSessionIdentity {
    /// The SID of the logon session.
    pub logon_sid: String,
    /// The SID of the user.
    pub user_sid: String,
    /// The number of the terminal-services session.
    pub session_id: u32,
}

impl StaticSessionIdentity {
    /// The identity the caller states.
    #[must_use]
    pub fn new(logon_sid: &str, user_sid: &str, session_id: u32) -> StaticSessionIdentity {
        StaticSessionIdentity {
            logon_sid: logon_sid.to_string(),
            user_sid: user_sid.to_string(),
            session_id,
        }
    }
}

impl SessionIdentity for StaticSessionIdentity {
    fn logon_sid(&self) -> String {
        self.logon_sid.clone()
    }

    fn user_sid(&self) -> String {
        self.user_sid.clone()
    }

    fn session_id(&self) -> u32 {
        self.session_id
    }
}

/// The identity of the process of the Hub, as the machine tells it.
#[cfg(windows)]
pub struct PlatformSessionIdentity {
    identity: Option<StaticSessionIdentity>,
}

#[cfg(windows)]
impl PlatformSessionIdentity {
    /// The identity of the process; it is read once and kept.
    #[must_use]
    pub fn new() -> PlatformSessionIdentity {
        PlatformSessionIdentity {
            identity: read_platform_identity().ok(),
        }
    }

    /// The identity of the process, and the reason when it cannot be read.
    pub fn read() -> Result<StaticSessionIdentity, TaseError> {
        read_platform_identity()
    }
}

#[cfg(windows)]
impl Default for PlatformSessionIdentity {
    fn default() -> PlatformSessionIdentity {
        PlatformSessionIdentity::new()
    }
}

#[cfg(windows)]
impl SessionIdentity for PlatformSessionIdentity {
    fn logon_sid(&self) -> String {
        self.identity
            .as_ref()
            .map(|identity| identity.logon_sid.clone())
            .unwrap_or_default()
    }

    fn user_sid(&self) -> String {
        self.identity
            .as_ref()
            .map(|identity| identity.user_sid.clone())
            .unwrap_or_default()
    }

    fn session_id(&self) -> u32 {
        self.identity
            .as_ref()
            .map(|identity| identity.session_id)
            .unwrap_or(0)
    }
}

/// The identity of the process on a platform that has no token to read.
///
/// The Hub of TASE lives on Windows (the pipe is the namespace of a logon session). The other
/// platforms get a fixed identity, so that the code that names the namespace still compiles and says
/// that it is not the real thing.
#[cfg(not(windows))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformSessionIdentity {
    identity: StaticSessionIdentity,
}

#[cfg(not(windows))]
impl PlatformSessionIdentity {
    /// The fake identity of the platform.
    #[must_use]
    pub fn new() -> PlatformSessionIdentity {
        PlatformSessionIdentity {
            identity: StaticSessionIdentity::new(
                "S-1-5-5-0-0-1000",
                "S-1-5-21-0-0-0-1000",
                0,
            ),
        }
    }
}

#[cfg(not(windows))]
impl Default for PlatformSessionIdentity {
    fn default() -> PlatformSessionIdentity {
        PlatformSessionIdentity::new()
    }
}

#[cfg(not(windows))]
impl SessionIdentity for PlatformSessionIdentity {
    fn logon_sid(&self) -> String {
        self.identity.logon_sid()
    }

    fn user_sid(&self) -> String {
        self.identity.user_sid()
    }

    fn session_id(&self) -> u32 {
        self.identity.session_id()
    }
}

/// The identity of the process of the Hub.
#[must_use]
pub fn platform_identity() -> Box<dyn SessionIdentity> {
    Box::new(PlatformSessionIdentity::new())
}

/// Reads the token of the process: the logon SID, the user SID and the terminal-services session.
///
/// The logon SID is the namespace of the pipe (detailed design §17.2); it is the SID of the logon
/// session of the token of the process, and not the SID of the user: a user that is logged on twice has
/// two logon SIDs and therefore two Hubs.
#[cfg(windows)]
fn read_platform_identity() -> Result<StaticSessionIdentity, TaseError> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{PSID, TokenLogonSid, TokenUser, TOKEN_QUERY};
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};

    // SAFETY: the calls of the Win32 API that are used here are the documented ones of a token of the
    // process of the caller; the buffers are sized after the size the API itself asks for and the
    // token handle is closed at the end.
    unsafe {
        let mut token: HANDLE = HANDLE(std::ptr::null_mut());
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|error| {
            TaseError::internal(format!("the token of the process cannot be opened: {error}"))
        })?;
        let result = (|| {
            // the logon SID of the token: it is the namespace of the pipe
            let logon_buffer = token_buffer(token, TokenLogonSid)?;
            let logon = format_sid(logon_buffer.as_ptr(), logon_buffer.len()).ok_or_else(|| {
                TaseError::internal("the logon session of the process has no SID".to_string())
            })?;

            // the user of the token: it is the rule of the access control
            let user_buffer = token_buffer(token, TokenUser)?;
            if user_buffer.len() < std::mem::size_of::<PSID>() {
                return Err(TaseError::internal(
                    "the user of the process cannot be read".to_string(),
                ));
            }
            let user_pointer = u64::from_ne_bytes(
                user_buffer[..std::mem::size_of::<PSID>()]
                    .try_into()
                    .expect("the buffer holds the pointer"),
            );
            let user = format_sid(user_pointer as *const u8, MAX_SID_BYTES).ok_or_else(|| {
                TaseError::internal("the user of the process has no SID".to_string())
            })?;

            let mut session_id: u32 = 0;
            ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id).map_err(|error| {
                TaseError::internal(format!("the session of the process cannot be read: {error}"))
            })?;
            Ok(StaticSessionIdentity::new(&logon, &user, session_id))
        })();
        let _ = CloseHandle(token);
        result
    }
}

/// The biggest SID of the Win32 API: eight bytes of header and fifteen sub authorities.
#[cfg(windows)]
const MAX_SID_BYTES: usize = 8 + 4 * 15;

/// Reads a piece of the token of the process, with the size the API itself asks for.
///
/// The API answers a first call with the size it needs; the buffer is then that size, and not one
/// byte more.
#[cfg(windows)]
unsafe fn token_buffer(
    token: windows::Win32::Foundation::HANDLE,
    class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
) -> Result<Vec<u8>, TaseError> {
    use std::ffi::c_void;
    use windows::Win32::Security::GetTokenInformation;

    // SAFETY: the buffer is sized after the size the API itself asks for, and the API is called with
    // the token of the process of the caller.
    unsafe {
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(token, class, None, 0, &mut needed);
        if needed == 0 {
            return Err(TaseError::internal(
                "the token of the process gives no size".to_string(),
            ));
        }
        let mut buffer = vec![0u8; needed as usize];
        GetTokenInformation(
            token,
            class,
            Some(buffer.as_mut_ptr() as *mut c_void),
            buffer.len() as u32,
            &mut needed,
        )
        .map_err(|error| TaseError::internal(format!("the token cannot be read: {error}")))?;
        Ok(buffer)
    }
}

/// The SID of a structure of the Win32 API, as the string the design book writes.
///
/// The layout of a SID is fixed: the revision, the number of the sub authorities, the authority (six
/// bytes, big endian) and the sub authorities (four bytes each, little endian). The header is
/// checked before anything is read, so that a buffer that is not a SID is refused instead of being
/// read as one.
#[cfg(windows)]
fn format_sid(sid: *const u8, available: usize) -> Option<String> {
    if sid.is_null() || available < 8 {
        return None;
    }
    // SAFETY: the pointer is the SID of a token of the caller; the header is checked before the
    // sub authorities are read, and only the bytes of the sub authorities are read.
    unsafe {
        let revision = *sid;
        let count = *sid.add(1);
        if revision != 1 || count > 15 {
            return None;
        }
        if available < 8 + 4 * count as usize {
            return None;
        }
        let mut authority: u64 = 0;
        for index in 0..6 {
            authority = (authority << 8) | u64::from(*sid.add(2 + index));
        }
        let mut out = format!("S-{revision}-{authority}");
        for index in 0..count as usize {
            let sub = std::ptr::read_unaligned(sid.add(8 + index * 4) as *const u32);
            out.push('-');
            out.push_str(&sub.to_string());
        }
        Some(out)
    }
}

/// A transport whose type is not known at compile time.
///
/// The Hub speaks over the pipe of `tase-local-ipc`, but a test speaks over the memory pipe of the
/// same crate: the two have to look the same to the Hub.
pub struct BoxedTransport(Box<dyn Transport>);

impl BoxedTransport {
    /// Wraps a transport.
    #[must_use]
    pub fn new(transport: Box<dyn Transport>) -> BoxedTransport {
        BoxedTransport(transport)
    }

    /// The name of the peer, when the transport knows it.
    #[must_use]
    pub fn peer(&self) -> Option<String> {
        self.0.peer()
    }
}

impl Transport for BoxedTransport {
    fn send(&mut self, bytes: &[u8]) -> Result<usize, FrameError> {
        self.0.send(bytes)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, FrameError> {
        self.0.recv(buf)
    }

    fn peer(&self) -> Option<String> {
        self.0.peer()
    }

    fn close(&mut self) {
        self.0.close();
    }

    fn recv_until(&mut self, buf: &mut [u8], deadline: std::time::Instant) -> Result<usize, FrameError> {
        self.0.recv_until(buf, deadline)
    }

    fn send_until(&mut self, bytes: &[u8], deadline: std::time::Instant) -> Result<usize, FrameError> {
        self.0.send_until(bytes, deadline)
    }
}

/// A connection that was accepted, with the identity of the peer.
pub struct AcceptedConnection {
    /// The transport of the connection.
    pub transport: BoxedTransport,
    /// The identity of the peer, when the pipe could tell it.
    ///
    /// A peer whose identity is unknown is refused by the Hub: a peer that cannot be named is not
    /// taken for a friend.
    pub peer: Option<PeerIdentity>,
}

/// The listener of the pipe of the Hub.
pub trait PipeListener: Send {
    /// Waits for a client; `None` means that nobody came in the time that was given.
    fn accept(&mut self, timeout: Duration) -> Result<Option<AcceptedConnection>, TaseError>;

    /// The name of the pipe.
    fn pipe_name(&self) -> &str;
}

/// The factory of the pipe: the seam that lets a test replace the pipe of Windows.
pub trait PipeFactory: Send {
    /// Tries to become the owner of the pipe of the namespace.
    ///
    /// The answer is an error when the name is already taken: that is how the Hub learns that another
    /// instance is running.
    fn listen(
        &mut self,
        logon_sid: &str,
        max_instances: u32,
    ) -> Result<Box<dyn PipeListener>, TaseError>;

    /// Tries to connect to the pipe of the namespace.
    fn connect(&mut self, logon_sid: &str, timeout: Duration) -> Result<BoxedTransport, TaseError>;
}

/// The role of the process in the namespace of the logon session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubRole {
    /// This process owns the pipe: it is the Hub.
    Owner,
    /// Another process is the Hub: this process is a client of it.
    Client,
}

impl std::fmt::Debug for HubHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubHandle")
            .field("role", &self.role)
            .field("pipe_name", &self.pipe_name)
            .field("mutex_name", &self.mutex_name)
            .field("logon_sid", &self.logon_sid)
            .field("session_id", &self.session_id)
            .finish()
    }
}

/// What [`start_or_connect`] gives back.
pub struct HubHandle {
    role: HubRole,
    pipe_name: String,
    mutex_name: String,
    logon_sid: String,
    user_sid: String,
    session_id: u32,
    data_dir: PathBuf,
    listener: Option<Box<dyn PipeListener>>,
    transport: Option<BoxedTransport>,
}

impl HubHandle {
    /// The role of the process.
    #[must_use]
    pub fn role(&self) -> HubRole {
        self.role
    }

    /// Whether this process is the Hub of the namespace.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        self.role == HubRole::Owner
    }

    /// The name of the pipe of the namespace.
    #[must_use]
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    /// The name of the mutex of the namespace.
    #[must_use]
    pub fn mutex_name(&self) -> &str {
        &self.mutex_name
    }

    /// The SID of the logon session.
    #[must_use]
    pub fn logon_sid(&self) -> &str {
        &self.logon_sid
    }

    /// The SID of the user.
    #[must_use]
    pub fn user_sid(&self) -> &str {
        &self.user_sid
    }

    /// The number of the terminal-services session.
    #[must_use]
    pub fn session_id(&self) -> u32 {
        self.session_id
    }

    /// The directory of the data of the Hub.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The listener of the pipe, when this process is the owner.
    pub fn take_listener(&mut self) -> Option<Box<dyn PipeListener>> {
        self.listener.take()
    }

    /// The connection to the Hub that is already running, when this process is a client.
    pub fn take_transport(&mut self) -> Option<BoxedTransport> {
        self.transport.take()
    }
}

/// Starts the Hub, or connects to the Hub that is already running.
///
/// The rule of the detailed design §17.2: "先以 Windows 登录 SID 作为 namespace 计算本用户管道名和单实例
/// 互斥体；若已存在已认证实例则连接，不再启动第二个写入协调进程".
///
/// The function tries to become the owner of the pipe of the namespace. When the name is taken, the
/// Hub of that namespace is already running, and the caller gets a handle of the role
/// [`HubRole::Client`] with the connection to it.
pub fn start_or_connect(config: &HubConfig) -> Result<HubHandle, TaseError> {
    let identity = platform_identity();
    let mut factory = platform_factory();
    start_or_connect_with(config, identity.as_ref(), factory.as_mut())
}

/// Starts the Hub, or connects to the Hub that is already running, with the identity and the pipe of
/// the caller.
///
/// The identity and the factory are the two seams of the module: the identity says which namespace the
/// Hub belongs to, and the factory says how the pipe of that namespace is opened.
pub fn start_or_connect_with(
    config: &HubConfig,
    identity: &dyn SessionIdentity,
    factory: &mut dyn PipeFactory,
) -> Result<HubHandle, TaseError> {
    config.validate()?;
    let logon_sid = identity.logon_sid();
    if logon_sid.trim().is_empty() {
        return Err(TaseError::internal(
            "the hub does not know the logon session it belongs to".to_string(),
        ));
    }
    let pipe_name = config.pipe_name(&logon_sid)?;
    let mutex_name = config.mutex_name(&logon_sid);
    match factory.listen(&logon_sid, config.max_connections) {
        Ok(listener) => Ok(HubHandle {
            role: HubRole::Owner,
            pipe_name,
            mutex_name,
            logon_sid,
            user_sid: identity.user_sid(),
            session_id: identity.session_id(),
            data_dir: config.data_dir.clone(),
            listener: Some(listener),
            transport: None,
        }),
        Err(listen_error) => {
            // the name is taken: another instance of the hub is running
            let transport = factory
                .connect(&logon_sid, Duration::from_millis(500))
                .map_err(|_| listen_error)?;
            Ok(HubHandle {
                role: HubRole::Client,
                pipe_name,
                mutex_name,
                logon_sid,
                user_sid: identity.user_sid(),
                session_id: identity.session_id(),
                data_dir: config.data_dir.clone(),
                listener: None,
                transport: Some(transport),
            })
        }
    }
}

/// The factory of the pipe of the platform.
#[must_use]
pub fn platform_factory() -> Box<dyn PipeFactory> {
    Box::new(PlatformPipeFactory)
}

/// The factory of the pipe of Windows.
#[cfg(windows)]
pub struct PlatformPipeFactory;

#[cfg(windows)]
impl PipeFactory for PlatformPipeFactory {
    fn listen(
        &mut self,
        logon_sid: &str,
        max_instances: u32,
    ) -> Result<Box<dyn PipeListener>, TaseError> {
        let pipe = tase_local_ipc::WindowsPipe::listen(logon_sid, max_instances)
            .map_err(map_frame_error)?;
        Ok(Box::new(WindowsPipeListener {
            pipe: Some(pipe),
            logon_sid: logon_sid.to_string(),
            max_instances,
            name: tase_local_ipc::WindowsPipe::pipe_name(logon_sid).map_err(map_frame_error)?,
        }))
    }

    fn connect(&mut self, logon_sid: &str, timeout: Duration) -> Result<BoxedTransport, TaseError> {
        let pipe = tase_local_ipc::WindowsPipe::connect(logon_sid, timeout).map_err(map_frame_error)?;
        Ok(BoxedTransport::new(Box::new(pipe)))
    }
}

/// The factory of the pipe on a platform that has none.
#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformPipeFactory;

#[cfg(not(windows))]
impl PipeFactory for PlatformPipeFactory {
    fn listen(
        &mut self,
        _logon_sid: &str,
        _max_instances: u32,
    ) -> Result<Box<dyn PipeListener>, TaseError> {
        Err(TaseError::worker_unavailable(
            "the named pipe of the hub is a Windows thing".to_string(),
        ))
    }

    fn connect(&mut self, _logon_sid: &str, _timeout: Duration) -> Result<BoxedTransport, TaseError> {
        Err(TaseError::worker_unavailable(
            "the named pipe of the hub is a Windows thing".to_string(),
        ))
    }
}

/// The listener of the pipe of Windows.
///
/// A named pipe is not a socket: the server creates an instance of the name and waits on it. When the
/// client of that instance is there, the instance becomes the transport of the connection, and the next
/// instance of the name is created for the next client.
#[cfg(windows)]
pub struct WindowsPipeListener {
    pipe: Option<tase_local_ipc::WindowsPipe>,
    logon_sid: String,
    max_instances: u32,
    name: String,
}

#[cfg(windows)]
impl PipeListener for WindowsPipeListener {
    fn accept(&mut self, timeout: Duration) -> Result<Option<AcceptedConnection>, TaseError> {
        let pipe = self
            .pipe
            .as_mut()
            .ok_or_else(|| TaseError::internal("the pipe of the hub is closed".to_string()))?;
        match pipe.wait_for_client(timeout) {
            Ok(()) => {}
            Err(FrameError::Timeout) => return Ok(None),
            Err(error) => return Err(map_frame_error(error)),
        }
        // the identity of the peer: the DACL of the pipe is bound to the logon SID, so the peer is of
        // the same logon session; the terminal-services session is read from the pipe itself.
        let session_id = pipe.client_session_id().map_err(map_frame_error)?;
        let connected = self
            .pipe
            .take()
            .ok_or_else(|| TaseError::internal("the pipe of the hub is closed".to_string()))?;
        // the next instance of the name, for the next client
        self.pipe = Some(
            tase_local_ipc::WindowsPipe::listen(&self.logon_sid, self.max_instances)
                .map_err(map_frame_error)?,
        );
        Ok(Some(AcceptedConnection {
            transport: BoxedTransport::new(Box::new(connected)),
            peer: Some(PeerIdentity::new(&self.identity_user_sid(), &self.logon_sid, session_id, 0)),
        }))
    }

    fn pipe_name(&self) -> &str {
        &self.name
    }
}

#[cfg(windows)]
impl WindowsPipeListener {
    /// The SID of the user of the Hub.
    ///
    /// The DACL of the pipe is built from the logon SID, and the logon SID belongs to the user of the
    /// process: the user of the peer is the user of the Hub.
    fn identity_user_sid(&self) -> String {
        match PlatformSessionIdentity::read() {
            Ok(identity) => identity.user_sid,
            Err(_) => String::new(),
        }
    }
}

/// The failure of the IPC crate in the vocabulary of the contract.
pub fn map_frame_error(error: FrameError) -> TaseError {
    match error {
        FrameError::Timeout => TaseError::worker_unavailable("the pipe did not answer in time"),
        FrameError::Closed => TaseError::worker_unavailable("the pipe is closed"),
        FrameError::WouldBlock => TaseError::rate_limited("the peer does not read the pipe"),
        FrameError::TooLarge => TaseError::invalid_argument("the frame is too large"),
        FrameError::Malformed | FrameError::UnknownKind | FrameError::Truncated => {
            TaseError::invalid_argument("the frame is broken")
        }
        FrameError::ProtocolMismatch => {
            TaseError::invalid_argument("the peer speaks another version of the protocol")
        }
        FrameError::CapabilityDenied => TaseError::permission_denied("a capability was not granted"),
        FrameError::Duplicate => TaseError::internal("the peer answered the same request twice"),
        FrameError::Broken => TaseError::internal("the connection is out of sync"),
        FrameError::Io(why) => TaseError::internal(why),
        FrameError::Remote(why) => TaseError::internal(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const LOGON: &str = "S-1-5-5-1000-1000";
    const USER: &str = "S-1-5-21-1-2-3-1001";
    const OTHER_LOGON: &str = "S-1-5-5-2000-2000";

    /// A factory that remembers which names are taken; the rule of the singleton is checked with it.
    #[derive(Clone, Default)]
    struct FakeFactory {
        taken: Arc<Mutex<HashMap<String, usize>>>,
        listen_error: Option<String>,
    }

    impl FakeFactory {
        fn new() -> FakeFactory {
            FakeFactory::default()
        }

        fn taken(&self) -> usize {
            self.taken.lock().expect("the lock is not poisoned").len()
        }
    }

    impl PipeFactory for FakeFactory {
        fn listen(
            &mut self,
            logon_sid: &str,
            _max_instances: u32,
        ) -> Result<Box<dyn PipeListener>, TaseError> {
            if let Some(reason) = &self.listen_error {
                return Err(TaseError::worker_unavailable(reason.clone()));
            }
            let mut taken = self.taken.lock().expect("the lock is not poisoned");
            if taken.contains_key(logon_sid) {
                return Err(TaseError::worker_unavailable("the pipe is taken".to_string()));
            }
            taken.insert(logon_sid.to_string(), 1);
            Ok(Box::new(FakeListener {
                name: format!("fake://{logon_sid}"),
            }))
        }

        fn connect(&mut self, logon_sid: &str, _timeout: Duration) -> Result<BoxedTransport, TaseError> {
            let taken = self.taken.lock().expect("the lock is not poisoned");
            if !taken.contains_key(logon_sid) {
                return Err(TaseError::worker_unavailable("nobody listens".to_string()));
            }
            drop(taken);
            let (left, _right) = tase_local_ipc::MemoryPipe::pair();
            Ok(BoxedTransport::new(Box::new(left)))
        }
    }

    struct FakeListener {
        name: String,
    }

    impl PipeListener for FakeListener {
        fn accept(&mut self, _timeout: Duration) -> Result<Option<AcceptedConnection>, TaseError> {
            Ok(None)
        }

        fn pipe_name(&self) -> &str {
            &self.name
        }
    }

    fn identity() -> StaticSessionIdentity {
        StaticSessionIdentity::new(LOGON, USER, 3)
    }

    #[test]
    fn the_first_process_owns_the_pipe_of_the_namespace() {
        let mut factory = FakeFactory::new();
        let mut handle = start_or_connect_with(&HubConfig::default(), &identity(), &mut factory)
            .expect("the hub starts");
        assert_eq!(handle.role(), HubRole::Owner);
        assert!(handle.is_owner());
        assert_eq!(handle.logon_sid(), LOGON);
        assert_eq!(handle.user_sid(), USER);
        assert_eq!(handle.session_id(), 3);
        assert!(handle.take_listener().is_some());
    }

    #[test]
    fn the_second_process_connects_instead_of_starting_a_second_hub() {
        let mut factory = FakeFactory::new();
        let first = start_or_connect_with(&HubConfig::default(), &identity(), &mut factory)
            .expect("the hub starts");
        assert!(first.is_owner());
        let mut second = start_or_connect_with(&HubConfig::default(), &identity(), &mut factory)
            .expect("the second process connects");
        assert_eq!(second.role(), HubRole::Client);
        assert!(!second.is_owner());
        assert!(second.take_listener().is_none());
        assert!(second.take_transport().is_some());
        assert_eq!(factory.taken(), 1);
    }

    #[test]
    fn every_logon_session_has_its_own_pipe() {
        let mut factory = FakeFactory::new();
        let first = start_or_connect_with(&HubConfig::default(), &identity(), &mut factory)
            .expect("the hub starts");
        let other = StaticSessionIdentity::new(OTHER_LOGON, USER, 3);
        let second = start_or_connect_with(&HubConfig::default(), &other, &mut factory)
            .expect("the other session starts its own hub");
        assert!(second.is_owner());
        assert_ne!(first.pipe_name(), second.pipe_name());
        assert_ne!(first.mutex_name(), second.mutex_name());
        assert_eq!(factory.taken(), 2);
    }

    #[test]
    fn the_pipe_is_named_after_the_logon_session() {
        let config = HubConfig::default();
        let name = config.pipe_name(LOGON).expect("the name of the pipe");
        assert!(name.contains(LOGON));
        assert!(name.starts_with(r"\\.\pipe\"));
        assert_eq!(name, config.pipe_name(LOGON).expect("the name of the pipe"));
    }

    #[test]
    fn the_mutex_is_named_after_the_logon_session() {
        let config = HubConfig::default();
        let name = config.mutex_name(LOGON);
        assert!(name.contains(LOGON));
        assert!(name.starts_with("Local\\"));
        assert_eq!(name, config.mutex_name(LOGON));
    }

    #[test]
    fn a_prefix_that_the_transport_does_not_use_is_refused() {
        let mut config = HubConfig::default();
        config.pipe_prefix = r"\\.\pipe\ide1-tase-".to_string();
        let error = config.validate().expect_err("the prefix is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        let mut factory = FakeFactory::new();
        let error = start_or_connect_with(&config, &identity(), &mut factory)
            .expect_err("the hub does not start");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_hub_without_a_connection_or_a_job_is_refused() {
        let mut config = HubConfig::default();
        config.max_connections = 0;
        assert!(config.validate().is_err());
        let mut config = HubConfig::default();
        config.max_concurrent_jobs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_hub_that_does_not_know_its_logon_session_does_not_start() {
        let mut factory = FakeFactory::new();
        let empty = StaticSessionIdentity::new("", USER, 3);
        let error = start_or_connect_with(&HubConfig::default(), &empty, &mut factory)
            .expect_err("the hub does not start");
        assert_eq!(error.code, tase_contract::ErrorCode::InternalError);
    }

    #[test]
    fn the_error_of_the_pipe_is_in_the_vocabulary_of_the_contract() {
        let error = map_frame_error(FrameError::Timeout);
        assert_eq!(error.code, tase_contract::ErrorCode::WorkerUnavailable);
        let error = map_frame_error(FrameError::Io("the handle is closed".to_string()));
        assert_eq!(error.code, tase_contract::ErrorCode::InternalError);
        let error = map_frame_error(FrameError::Malformed);
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_database_lives_in_the_data_directory() {
        let config = HubConfig::with_data_dir("C:/tmp/tase");
        assert_eq!(config.store_path(), PathBuf::from("C:/tmp/tase").join(STORE_FILE_NAME));
    }

    #[test]
    fn the_identity_of_the_platform_is_never_empty_on_windows() {
        let identity = platform_identity();
        // the identity is read from the token of the process; a machine that cannot read it says so
        // with an empty SID, and the hub refuses to start rather than opening a pipe to everybody.
        if cfg!(windows) {
            let read = PlatformSessionIdentity::read().expect("the identity is read");
            assert!(read.logon_sid.starts_with("S-1-"), "{}", read.logon_sid);
            assert!(read.user_sid.starts_with("S-1-"), "{}", read.user_sid);
            assert!(!identity.logon_sid().is_empty(), "the logon SID is empty");
            assert!(!identity.user_sid().is_empty(), "the user SID is empty");
        }
    }
}
