//! Test doubles for the pure layers: a scripted byte stream, an in-memory duplex pair and a deterministic entropy
//! source. Compiled for tests only.

use crate::auth::Entropy;
use crate::error::IpcError;
use crate::frame::Transport;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

pub fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

pub fn hex(s: &str) -> Vec<u8> {
    pime_core::sha256::from_hex(s).expect("test vector is hex")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Closed,
    Timeout,
}

/// Reads from a fixed buffer (optionally in small chunks); once it is exhausted the stream ends or times out.
pub struct Script {
    data: VecDeque<u8>,
    chunk: usize,
    end: End,
    pub written: Vec<u8>,
}

impl Script {
    pub fn new(bytes: &[u8]) -> Script {
        Script {
            data: bytes.iter().copied().collect(),
            chunk: usize::MAX,
            end: End::Closed,
            written: Vec::new(),
        }
    }

    pub fn chunked(mut self, chunk: usize) -> Script {
        self.chunk = chunk;
        self
    }

    pub fn then_timeout(mut self) -> Script {
        self.end = End::Timeout;
        self
    }
}

impl Transport for Script {
    fn read_some(&mut self, buf: &mut [u8], _deadline: Instant) -> Result<usize, IpcError> {
        if self.data.is_empty() {
            return Err(match self.end {
                End::Closed => IpcError::Closed,
                End::Timeout => IpcError::Timeout,
            });
        }
        let n = buf.len().min(self.chunk).min(self.data.len());
        for slot in &mut buf[..n] {
            *slot = self.data.pop_front().expect("length checked");
        }
        Ok(n)
    }

    fn write_all(&mut self, data: &[u8], _deadline: Instant) -> Result<(), IpcError> {
        self.written.extend_from_slice(data);
        Ok(())
    }
}

/// One end of an in-memory duplex stream (the other end is the peer's). Dropping an end closes it.
pub struct MemEnd {
    rx: Receiver<Vec<u8>>,
    tx: Sender<Vec<u8>>,
    pending: VecDeque<u8>,
}

pub fn mem_pair() -> (MemEnd, MemEnd) {
    let (atx, brx) = channel();
    let (btx, arx) = channel();
    (
        MemEnd {
            rx: arx,
            tx: atx,
            pending: VecDeque::new(),
        },
        MemEnd {
            rx: brx,
            tx: btx,
            pending: VecDeque::new(),
        },
    )
}

impl Transport for MemEnd {
    fn read_some(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, IpcError> {
        if self.pending.is_empty() {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(chunk) => self.pending.extend(chunk),
                Err(RecvTimeoutError::Timeout) => return Err(IpcError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(IpcError::Closed),
            }
        }
        let n = buf.len().min(self.pending.len());
        for slot in &mut buf[..n] {
            *slot = self.pending.pop_front().expect("length checked");
        }
        Ok(n)
    }

    fn write_all(&mut self, data: &[u8], _deadline: Instant) -> Result<(), IpcError> {
        self.tx.send(data.to_vec()).map_err(|_| IpcError::Closed)
    }
}

/// Deterministic "entropy": a counter run through SHA-256. Distinct per instance seed; never used outside tests.
pub struct CountingEntropy {
    seed: u8,
    counter: u64,
}

impl CountingEntropy {
    pub fn new(seed: u8) -> CountingEntropy {
        CountingEntropy { seed, counter: 0 }
    }
}

impl Entropy for CountingEntropy {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), IpcError> {
        let mut filled = 0;
        while filled < buf.len() {
            self.counter += 1;
            let mut input = vec![self.seed];
            input.extend_from_slice(&self.counter.to_le_bytes());
            let block = pime_core::sha256::sha256(&input);
            let n = (buf.len() - filled).min(32);
            buf[filled..filled + n].copy_from_slice(&block[..n]);
            filled += n;
        }
        Ok(())
    }
}
