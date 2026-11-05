//! The transports of the Hub: a byte stream with a deadline, and a peer.
//!
//! The Hub speaks over three kinds of stream, and they all look the same from here:
//!
//! * the **named pipe** of a logon session on Windows ([`WindowsPipe`]), which is the real one;
//! * the **memory pipe** ([`MemoryPipe`]), which is what the tests and the tools use;
//! * the **Unix domain socket** of a later version, which is not written yet (BD §16.2 names it for Linux).
//!
//! The named pipe is not written here: the pipe, its DACL, its `PIPE_REJECT_REMOTE_CLIENTS` and its
//! deadline-bound overlapped I/O come from `pime-ipc` (the IPC crate of the repository), and this module only
//! adapts them to the trait of TASE. Writing a second transport would mean a second set of security bugs.
//!
//! Two rules the transports keep:
//! * **the buffer is bounded.** A peer that does not read must not make the sender grow its memory: the memory
//!   pipe refuses the frame ([`FrameError::WouldBlock`]), the named pipe runs into its write deadline
//!   ([`FrameError::Timeout`]). Neither of them buffers without a limit (DD §17.5: every queue has its limit).
//! * **a deadline is a deadline.** `recv_until` gives up at the deadline and says so; it never waits for a peer
//!   that will not answer.

use crate::framing::FrameError;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a `send` or a `recv` without an explicit deadline waits.
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// A byte stream with a peer and a deadline.
///
/// The four methods are what a caller needs; `recv_until` and `send_until` are the same with a deadline and may
/// be overridden by a transport that can honour one. A transport that cannot time out says so in its
/// documentation: the default implementation of the two methods waits for the peer.
pub trait Transport: Send {
    /// Writes the bytes; the answer is the number of bytes that were written.
    ///
    /// A transport of this crate writes everything or fails: half a frame would desynchronise the stream.
    fn send(&mut self, bytes: &[u8]) -> Result<usize, FrameError>;

    /// Reads into `buf`; the answer is the number of bytes that were read.
    ///
    /// `Ok(0)` is not an answer: a transport that has nothing to say must wait or fail.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, FrameError>;

    /// The name of the peer, when the transport knows it.
    fn peer(&self) -> Option<String>;

    /// Closes the stream: the peer sees the end of it.
    fn close(&mut self);

    /// Reads into `buf`, giving up at `deadline`.
    ///
    /// The default implementation waits for the peer and ignores the deadline: a transport that cannot time out
    /// must say so, because a caller that waits for an answer that never comes would wait for ever.
    fn recv_until(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, FrameError> {
        let _ = deadline;
        self.recv(buf)
    }

    /// Writes the bytes, giving up at `deadline`.
    ///
    /// The default implementation waits for the peer and ignores the deadline.
    fn send_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<usize, FrameError> {
        let _ = deadline;
        self.send(bytes)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the memory pipe

/// One direction of a memory pipe: the bytes that were written and are not read yet.
struct Channel {
    buffer: VecDeque<u8>,
    /// Whether the writer of this direction is still open.
    open: bool,
    /// The most bytes that may wait in this direction.
    capacity: usize,
}

struct Shared {
    /// The channel 0 is written by the first end of the pair and read by the second one.
    channels: [Mutex<Channel>; 2],
    signal: Condvar,
}

impl Shared {
    fn new(capacity: usize) -> Shared {
        Shared {
            channels: [
                Mutex::new(Channel {
                    buffer: VecDeque::new(),
                    open: true,
                    capacity,
                }),
                Mutex::new(Channel {
                    buffer: VecDeque::new(),
                    open: true,
                    capacity,
                }),
            ],
            signal: Condvar::new(),
        }
    }
}

/// A pipe in the memory: two ends, one direction each.
///
/// It is what the tests of the crate speak over, and it is honest about the two things that matter on a real
/// pipe: a bounded buffer (a peer that does not read makes the sender fail, not grow) and a deadline.
pub struct MemoryPipe {
    shared: Arc<Shared>,
    /// The channel this end writes into; the other one is the channel it reads from.
    write_side: usize,
    /// The name of the peer, when the pair was named.
    peer_name: Option<String>,
}

impl MemoryPipe {
    /// The capacity of a pair that is created without one.
    pub const DEFAULT_CAPACITY: usize = 256 * 1024;

    /// Two ends of a pipe: what one writes, the other reads.
    pub fn pair() -> (MemoryPipe, MemoryPipe) {
        MemoryPipe::pair_with_capacity(MemoryPipe::DEFAULT_CAPACITY)
    }

    /// Two ends of a pipe whose buffer holds at most `capacity` bytes.
    ///
    /// The capacity is the backpressure of the pipe: the sender fails instead of queueing without a limit.
    pub fn pair_with_capacity(capacity: usize) -> (MemoryPipe, MemoryPipe) {
        let shared = Arc::new(Shared::new(capacity));
        (
            MemoryPipe {
                shared: Arc::clone(&shared),
                write_side: 0,
                peer_name: None,
            },
            MemoryPipe {
                shared,
                write_side: 1,
                peer_name: None,
            },
        )
    }

    /// Two ends of a pipe that know the names of each other; `peer()` answers with the name of the other end.
    pub fn pair_named(name_a: &str, name_b: &str) -> (MemoryPipe, MemoryPipe) {
        let shared = Arc::new(Shared::new(MemoryPipe::DEFAULT_CAPACITY));
        (
            MemoryPipe {
                shared: Arc::clone(&shared),
                write_side: 0,
                peer_name: Some(name_b.to_string()),
            },
            MemoryPipe {
                shared,
                write_side: 1,
                peer_name: Some(name_a.to_string()),
            },
        )
    }

    /// The most bytes that may wait for the peer.
    pub fn capacity(&self) -> usize {
        let guard = lock(&self.shared.channels[self.write_side]);
        guard.capacity
    }

    /// The bytes that were written to the peer and are not read yet.
    pub fn queued_bytes(&self) -> usize {
        let guard = lock(&self.shared.channels[self.write_side]);
        guard.buffer.len()
    }

    fn read_side(&self) -> usize {
        1 - self.write_side
    }

    fn take(&mut self, buf: &mut [u8], deadline: Option<Instant>) -> Result<usize, FrameError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut guard = lock(&self.shared.channels[self.read_side()]);
        loop {
            if !guard.buffer.is_empty() {
                break;
            }
            if !guard.open {
                // the peer closed and there is nothing left to read
                return Err(FrameError::Closed);
            }
            match deadline {
                None => {
                    guard = self
                        .shared
                        .signal
                        .wait(guard)
                        .unwrap_or_else(|e| e.into_inner());
                }
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(FrameError::Timeout);
                    }
                    let (guard_after, _) = self
                        .shared
                        .signal
                        .wait_timeout(guard, deadline - now)
                        .unwrap_or_else(|e| e.into_inner());
                    guard = guard_after;
                }
            }
        }
        let count = buf.len().min(guard.buffer.len());
        for slot in buf.iter_mut().take(count) {
            if let Some(byte) = guard.buffer.pop_front() {
                *slot = byte;
            }
        }
        Ok(count)
    }
}

/// A mutex that is never poisoned: a panic in another thread must not turn every later read into a panic.
fn lock(channel: &Mutex<Channel>) -> MutexGuard<'_, Channel> {
    channel.lock().unwrap_or_else(|e| e.into_inner())
}

impl Transport for MemoryPipe {
    fn send(&mut self, bytes: &[u8]) -> Result<usize, FrameError> {
        if bytes.is_empty() {
            return Ok(0);
        }
        {
            let mut guard = lock(&self.shared.channels[self.write_side]);
            if !guard.open {
                return Err(FrameError::Closed);
            }
            if guard.buffer.len() + bytes.len() > guard.capacity {
                // the peer does not read: the frame is dropped, the buffer does not grow
                return Err(FrameError::WouldBlock);
            }
            guard.buffer.extend(bytes);
        }
        self.shared.signal.notify_all();
        Ok(bytes.len())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, FrameError> {
        self.take(buf, None)
    }

    fn recv_until(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, FrameError> {
        self.take(buf, Some(deadline))
    }

    fn peer(&self) -> Option<String> {
        self.peer_name.clone()
    }

    fn close(&mut self) {
        {
            let mut guard = lock(&self.shared.channels[self.write_side]);
            guard.open = false;
        }
        self.shared.signal.notify_all();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the named pipe

/// The named pipe of a logon session on Windows.
///
/// The pipe, its DACL (the logon SID only), its `PIPE_REJECT_REMOTE_CLIENTS` and its overlapped I/O with
/// deadlines come from `pime-ipc`; this type only carries them and speaks the trait of TASE.
#[cfg(windows)]
pub struct WindowsPipe {
    /// `None` after the pipe was closed: the handle is closed with it and the peer sees the end of the stream.
    pipe: Option<pime_ipc::pipe::Pipe>,
    /// The name of the pipe, which is also the namespace of the logon session.
    peer: Option<String>,
}

#[cfg(windows)]
impl WindowsPipe {
    /// Whether the named pipe of the Hub exists on this platform.
    pub const fn is_available() -> bool {
        true
    }

    /// The name of the pipe of a logon session.
    pub fn pipe_name(logon_sid: &str) -> Result<String, FrameError> {
        let name = pime_ipc::auth::PipeName::for_sid(logon_sid).map_err(map_ipc_error)?;
        Ok(name.as_str().to_string())
    }

    /// Creates the first instance of the pipe of a logon session.
    ///
    /// The instance is created with the DACL of the logon SID and with `PIPE_REJECT_REMOTE_CLIENTS`; it fails
    /// when another process already owns the name, which is how a squatter is caught.
    pub fn listen(logon_sid: &str, max_instances: u32) -> Result<WindowsPipe, FrameError> {
        let name = pime_ipc::auth::PipeName::for_sid(logon_sid).map_err(map_ipc_error)?;
        let sddl = pime_ipc::auth::sddl_for_sid(logon_sid).map_err(map_ipc_error)?;
        let descriptor = pime_ipc::winsec::SecurityDescriptor::from_sddl(&sddl).map_err(map_ipc_error)?;
        let pipe = pime_ipc::pipe::Pipe::create_server_instance(&name, true, &descriptor, max_instances)
            .map_err(map_ipc_error)?;
        Ok(WindowsPipe {
            pipe: Some(pipe),
            peer: Some(name.as_str().to_string()),
        })
    }

    /// Opens the pipe of a logon session as a client.
    ///
    /// The answer is at once when no Hub is listening (`Unavailable`): a client does not wait for a Hub that is
    /// not there.
    pub fn connect(logon_sid: &str, timeout: Duration) -> Result<WindowsPipe, FrameError> {
        let name = pime_ipc::auth::PipeName::for_sid(logon_sid).map_err(map_ipc_error)?;
        let pipe = pime_ipc::pipe::Pipe::connect_client(
            &name,
            pime_ipc::frame::deadline_after(timeout),
        )
        .map_err(map_ipc_error)?;
        Ok(WindowsPipe {
            pipe: Some(pipe),
            peer: Some(name.as_str().to_string()),
        })
    }

    /// Waits for a client to connect to this instance.
    pub fn wait_for_client(&mut self, timeout: Duration) -> Result<(), FrameError> {
        let pipe = self.pipe.as_mut().ok_or(FrameError::Closed)?;
        pipe.wait_for_client(pime_ipc::frame::deadline_after(timeout), None)
            .map_err(map_ipc_error)
    }

    /// The terminal-services session of the client that is connected.
    pub fn client_session_id(&self) -> Result<u32, FrameError> {
        let pipe = self.pipe.as_ref().ok_or(FrameError::Closed)?;
        pipe.client_session_id().map_err(map_ipc_error)
    }
}

#[cfg(windows)]
impl Transport for WindowsPipe {
    fn send(&mut self, bytes: &[u8]) -> Result<usize, FrameError> {
        self.send_until(bytes, Instant::now() + DEFAULT_IO_TIMEOUT)
    }

    fn send_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<usize, FrameError> {
        let pipe = self.pipe.as_mut().ok_or(FrameError::Closed)?;
        pime_ipc::frame::Transport::write_all(pipe, bytes, deadline).map_err(map_ipc_error)?;
        Ok(bytes.len())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, FrameError> {
        self.recv_until(buf, Instant::now() + DEFAULT_IO_TIMEOUT)
    }

    fn recv_until(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, FrameError> {
        let pipe = self.pipe.as_mut().ok_or(FrameError::Closed)?;
        pime_ipc::frame::Transport::read_some(pipe, buf, deadline).map_err(map_ipc_error)
    }

    fn peer(&self) -> Option<String> {
        self.peer.clone()
    }

    fn close(&mut self) {
        // dropping the pipe closes the handle: the peer sees the end of the stream
        self.pipe = None;
    }
}

/// The failure of `pime-ipc` in the vocabulary of TASE.
#[cfg(windows)]
fn map_ipc_error(error: pime_ipc::error::IpcError) -> FrameError {
    use pime_ipc::error::IpcError as E;
    match error {
        E::Timeout => FrameError::Timeout,
        E::Closed | E::Truncated | E::Broken | E::Stopped => FrameError::Closed,
        E::PayloadTooLarge { .. } => FrameError::TooLarge,
        E::Malformed(_) => FrameError::Malformed,
        E::Os { op, code } => FrameError::Io(format!("{op} failed with Win32 error {code}")),
        other => FrameError::Io(other.to_string()),
    }
}

/// The named pipe of a logon session, on a platform that has none.
///
/// The Hub of TASE lives on Windows (the pipe is the namespace of a logon session). On the other platforms the
/// type exists so that the code that names it still compiles, and every call fails with a clear reason.
#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsPipe;

#[cfg(not(windows))]
impl WindowsPipe {
    /// Whether the named pipe of the Hub exists on this platform.
    pub const fn is_available() -> bool {
        false
    }

    /// The name of the pipe of a logon session: a Windows thing.
    pub fn pipe_name(_logon_sid: &str) -> Result<String, FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }

    /// Creates the first instance of the pipe of a logon session: a Windows thing.
    pub fn listen(_logon_sid: &str, _max_instances: u32) -> Result<WindowsPipe, FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }

    /// Opens the pipe of a logon session as a client: a Windows thing.
    pub fn connect(_logon_sid: &str, _timeout: Duration) -> Result<WindowsPipe, FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }

    /// Waits for a client: a Windows thing.
    pub fn wait_for_client(&mut self, _timeout: Duration) -> Result<(), FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }

    /// The session of the client: a Windows thing.
    pub fn client_session_id(&self) -> Result<u32, FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }
}

#[cfg(not(windows))]
impl Transport for WindowsPipe {
    fn send(&mut self, _bytes: &[u8]) -> Result<usize, FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }

    fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, FrameError> {
        Err(FrameError::Io(
            "the named pipe of the Hub is a Windows thing".into(),
        ))
    }

    fn peer(&self) -> Option<String> {
        None
    }

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn a_pair_carries_the_bytes_from_one_end_to_the_other() {
        let (mut a, mut b) = MemoryPipe::pair();
        assert_eq!(a.send(b"hello").expect("the bytes are written"), 5);
        let mut buf = [0u8; 16];
        let n = b.recv(&mut buf).expect("the bytes are read");
        assert_eq!(&buf[..n], b"hello");
        assert_eq!(b.peer(), None);
    }

    #[test]
    fn a_named_pair_knows_the_name_of_the_peer() {
        let (a, b) = MemoryPipe::pair_named("hub", "agent");
        assert_eq!(a.peer().as_deref(), Some("agent"));
        assert_eq!(b.peer().as_deref(), Some("hub"));
    }

    /// The buffer of the pipe is bounded: a peer that does not read makes the sender fail, not grow.
    #[test]
    fn the_buffer_is_bounded_and_the_sender_fails_instead_of_growing() {
        let (mut a, _b) = MemoryPipe::pair_with_capacity(64);
        assert_eq!(a.capacity(), 64);
        assert_eq!(a.send(&[0u8; 64]).expect("the buffer is filled"), 64);
        assert_eq!(a.queued_bytes(), 64);
        // the peer does not read: the next frame is refused, the buffer does not grow
        assert_eq!(a.send(b"more"), Err(FrameError::WouldBlock));
        assert_eq!(a.queued_bytes(), 64);
        assert!(a.send(&[0u8; 65]).is_err());
        assert_eq!(a.queued_bytes(), 64);
    }

    /// A closed pipe is an orderly end: the peer reads what is left and then sees the end.
    #[test]
    fn a_closed_pipe_ends_the_stream_in_an_orderly_way() {
        let (mut a, mut b) = MemoryPipe::pair();
        a.send(b"last").expect("the bytes are written");
        a.close();
        let mut buf = [0u8; 16];
        let n = b.recv(&mut buf).expect("what is left is read");
        assert_eq!(&buf[..n], b"last");
        assert_eq!(b.recv(&mut buf), Err(FrameError::Closed));
        // and nothing more may be written into a closed pipe
        assert_eq!(a.send(b"more"), Err(FrameError::Closed));
    }

    /// A deadline is a deadline: a peer that does not answer must not hold the caller for ever.
    #[test]
    fn a_read_gives_up_at_the_deadline() {
        let (_a, mut b) = MemoryPipe::pair();
        let mut buf = [0u8; 16];
        let start = Instant::now();
        let answer = b.recv_until(&mut buf, Instant::now() + Duration::from_millis(50));
        assert_eq!(answer, Err(FrameError::Timeout));
        assert!(start.elapsed() < Duration::from_secs(2), "the caller waits for the deadline only");
    }

    /// The bytes that arrive while the reader waits are not lost.
    #[test]
    fn the_reader_wakes_up_when_the_bytes_arrive() {
        let (mut a, mut b) = MemoryPipe::pair();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            a.send(b"wake up").expect("the bytes are written");
        });
        let mut buf = [0u8; 16];
        let n = b
            .recv_until(&mut buf, Instant::now() + Duration::from_secs(5))
            .expect("the bytes are read");
        assert_eq!(&buf[..n], b"wake up");
        writer.join().expect("the writer ends");
    }

    /// A transport that cannot time out must say so: the default of `recv_until` waits for the peer.
    #[test]
    fn the_default_deadline_waits_for_the_peer() {
        struct Deaf;
        impl Transport for Deaf {
            fn send(&mut self, _bytes: &[u8]) -> Result<usize, FrameError> {
                Ok(0)
            }
            fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, FrameError> {
                Err(FrameError::Closed)
            }
            fn peer(&self) -> Option<String> {
                None
            }
            fn close(&mut self) {}
        }
        let mut deaf = Deaf;
        let mut buf = [0u8; 4];
        assert_eq!(deaf.recv_until(&mut buf, Instant::now()), Err(FrameError::Closed));
    }

    /// The named pipe of the Hub is a Windows thing: the stub says so instead of pretending.
    #[test]
    fn the_named_pipe_says_whether_it_exists_here() {
        if WindowsPipe::is_available() {
            assert!(WindowsPipe::pipe_name("S-1-5-5-1000-1000").is_ok());
        } else {
            assert!(WindowsPipe::pipe_name("S-1-5-5-1000-1000").is_err());
            assert!(WindowsPipe::connect("S-1-5-5-1000-1000", Duration::from_millis(1)).is_err());
        }
    }
}
