//! The five concurrency algorithms of the catalogue (the assets `algorithm.concurrency.*`).
//!
//! Every public function of this module names its asset in its doc comment:
//! `* asset: `algorithm.\<category\>.\<name>``.
//!
//! The rules the five algorithms keep:
//!
//! * **The standard library only.** No third-party crate: `std::thread`, `std::sync` and their
//!   condition variables are enough, and the crate has to stay auditable line by line.
//! * **No `unsafe`.** `#![forbid(unsafe_code)]` at the crate root.
//! * **The waiting is always a `while` loop.** A condition variable can wake a thread up for
//!   nothing: the condition is checked again, in a loop, before the thread goes on.
//! * **The tests never rely on the luck of the scheduler.** The threads are synchronized with
//!   barriers, channels and condition variables, the assertions count the items instead of ordering
//!   them, and every wait has a timeout written down, so a broken implementation fails instead of
//!   hanging.
//! * **The clocks are injected.** The backoff and the token bucket take their time as a parameter, so
//!   the tests do not wait a real second to see that the rate is right.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

// ---- algorithm.concurrency.bounded_queue ------------------------------------------------------------------------

/// What the queue does when it is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backpressure {
    /// The producer waits until there is room: the back pressure reaches the producer.
    Block,
    /// The producer is refused at once: the queue fails fast and loses nothing.
    Reject,
}

/// Why the queue refused an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// The queue is full and the policy refuses the entry.
    Full,
    /// The queue is closed: it takes no new entry.
    Closed,
    /// The lock of the queue was poisoned by the panic of another thread.
    Poisoned,
}

/// The state the queue protects with its lock.
#[derive(Debug)]
struct QueueState<T> {
    items: VecDeque<T>,
}

impl<T> Default for QueueState<T> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
        }
    }
}

/// The inner part of the queue, shared by the clones.
#[derive(Debug)]
struct QueueInner<T> {
    state: Mutex<QueueState<T>>,
    not_empty: Condvar,
    not_full: Condvar,
    closed: AtomicBool,
}

/// A bounded queue: a first in first out buffer of a fixed capacity.
///
/// The queue is the buffer that turns a fast producer into a slow consumer: when it is full, the
/// producer waits (the back pressure) or is refused (the fast failure). The two policies are chosen
/// explicitly, because the card says that they have to be.
#[derive(Debug)]
pub struct BoundedQueue<T> {
    capacity: usize,
    policy: Backpressure,
    inner: Arc<QueueInner<T>>,
}

impl<T> Clone for BoundedQueue<T> {
    fn clone(&self) -> Self {
        Self {
            capacity: self.capacity,
            policy: self.policy,
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> BoundedQueue<T> {
    /// Builds a queue of the given capacity.
    ///
    /// - The capacity must be at least 1. The card notes that a zero capacity channel is a legal
    ///   strict handshake (`sync_channel(0)`); this implementation does not provide that handshake
    ///   and refuses the capacity of zero instead of building a queue that cannot be used.
    /// - The policy is the answer to the question of the card: block, refuse, or drop. The drop
    ///   policy is not offered here: it loses data, and a caller that wants it has to say so.
    pub fn new(capacity: usize, policy: Backpressure) -> Result<Self, String> {
        if capacity == 0 {
            return Err(
                "bounded_queue: the capacity must be at least 1; the queue of capacity 0 is a strict \
                 handshake that this implementation does not provide"
                    .to_owned(),
            );
        }
        Ok(Self {
            capacity,
            policy,
            inner: Arc::new(QueueInner {
                state: Mutex::new(QueueState::default()),
                not_empty: Condvar::new(),
                not_full: Condvar::new(),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// The capacity of the queue.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The policy of the queue when it is full.
    pub fn policy(&self) -> Backpressure {
        self.policy
    }

    /// The number of items in the queue.
    pub fn len(&self) -> usize {
        match self.inner.state.lock() {
            Ok(state) => state.items.len(),
            Err(_) => 0,
        }
    }

    /// Whether the queue holds no item.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the queue is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Puts an item in the queue and waits for the room when the policy is `Block`.
    ///
    /// The item is refused with `QueueError::Full` when the policy is `Reject` and the queue is
    /// full, and with `QueueError::Closed` when the queue is closed.
    pub fn push(&self, item: T) -> Result<(), QueueError> {
        let mut state = self.inner.state.lock().map_err(|_| QueueError::Poisoned)?;
        loop {
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(QueueError::Closed);
            }
            if state.items.len() < self.capacity {
                state.items.push_back(item);
                drop(state);
                self.inner.not_empty.notify_one();
                return Ok(());
            }
            if self.policy == Backpressure::Reject {
                return Err(QueueError::Full);
            }
            // the condition is checked again in the loop: a wake up can come for nothing
            state = self
                .inner
                .not_full
                .wait(state)
                .map_err(|_| QueueError::Poisoned)?;
        }
    }

    /// Puts an item in the queue without waiting.
    ///
    /// The item is refused with `QueueError::Full` when the queue is full.
    pub fn try_push(&self, item: T) -> Result<(), QueueError> {
        let mut state = self.inner.state.lock().map_err(|_| QueueError::Poisoned)?;
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(QueueError::Closed);
        }
        if state.items.len() >= self.capacity {
            return Err(QueueError::Full);
        }
        state.items.push_back(item);
        drop(state);
        self.inner.not_empty.notify_one();
        Ok(())
    }

    /// Takes an item out of the queue and waits for one when the queue is empty.
    ///
    /// The answer is `None` when the queue is closed and empty: that is the end of the stream.
    pub fn pop(&self) -> Result<Option<T>, QueueError> {
        let mut state = self.inner.state.lock().map_err(|_| QueueError::Poisoned)?;
        loop {
            if let Some(item) = state.items.pop_front() {
                drop(state);
                self.inner.not_full.notify_one();
                return Ok(Some(item));
            }
            if self.inner.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            state = self
                .inner
                .not_empty
                .wait(state)
                .map_err(|_| QueueError::Poisoned)?;
        }
    }

    /// Takes an item out of the queue without waiting.
    ///
    /// The answer is `None` when the queue is empty.
    pub fn try_pop(&self) -> Result<Option<T>, QueueError> {
        let mut state = self.inner.state.lock().map_err(|_| QueueError::Poisoned)?;
        let item = state.items.pop_front();
        if item.is_some() {
            drop(state);
            self.inner.not_full.notify_one();
        }
        Ok(item)
    }

    /// Closes the queue: it takes no new item and the consumers see the end of the stream once the
    /// items that are left are taken.
    ///
    /// The lock is taken before the flag is raised, so a thread that is waiting cannot miss the
    /// wake up.
    pub fn close(&self) {
        let _guard = self.inner.state.lock();
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.not_empty.notify_all();
        self.inner.not_full.notify_all();
    }
}

/// Builds a bounded queue of the given capacity.
///
/// The queue is the buffer of a fixed size between a producer and a consumer: when it is full, the
/// producer waits (the back pressure) or is refused (the fast failure), and the choice is made
/// explicitly.
///
/// # Input
///
/// - The capacity must be at least 1: the capacity of zero is refused (see
///   [`BoundedQueue::new`]).
/// - The policy is `Block` or `Reject`. The card asks for the policy to be chosen on purpose, and it
///   names a third one, the drop, that this implementation does not offer because it loses data.
///
/// # Complexity
///
/// - Push and pop: `O(1)`.
/// - Extra space: `O(capacity)` - the memory of the queue does not grow with the traffic.
///
/// # When it does not apply
///
/// - The card rejects it when the production and the consumption are matched in rate: an unbounded
///   queue is simpler.
/// - The card rejects it when the backlog does not fit in the memory: a queue on the disk is the
///   answer.
/// - The card rejects it when dropping is better than waiting (the sampling of the telemetry): a
///   queue that drops on purpose has to be written on purpose.
/// - The card rejects it when the capacity has to change while the queue runs: the fixed capacity is
///   the premise of the design.
///
/// * asset: `algorithm.concurrency.bounded_queue`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.concurrency.bounded_queue/card.md`
pub fn bounded_queue<T>(capacity: usize, policy: Backpressure) -> Result<BoundedQueue<T>, String> {
    BoundedQueue::new(capacity, policy)
}

// ---- algorithm.concurrency.producer_consumer ---------------------------------------------------------------------

/// Why the buffer refused an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProducerConsumerError {
    /// The buffer is full and the producer was refused.
    Full,
    /// The buffer is closed: it takes no new item.
    Closed,
    /// The lock of the buffer was poisoned by the panic of another thread. The buffer refuses to go
    /// on: the state of the shared buffer is unknown after a panic.
    Poisoned,
}

/// The inner part of the buffer, shared by the clones.
#[derive(Debug)]
struct BufferInner<T> {
    items: Mutex<VecDeque<T>>,
    not_empty: Condvar,
    not_full: Condvar,
    closed: AtomicBool,
}

/// A shared bounded buffer: the producer writes in it and the consumer reads from it, and the two
/// are slowed down by each other.
///
/// The buffer is the carrier of the producer and consumer pattern: several producers and several
/// consumers share it, and the condition variables make the two sides wait for each other instead of
/// polling.
#[derive(Debug)]
pub struct ProducerConsumer<T> {
    capacity: usize,
    inner: Arc<BufferInner<T>>,
}

impl<T> Clone for ProducerConsumer<T> {
    fn clone(&self) -> Self {
        Self {
            capacity: self.capacity,
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> ProducerConsumer<T> {
    /// Builds a buffer of the given capacity.
    ///
    /// - The capacity must be at least 1: a buffer of capacity 0 holds nothing.
    pub fn new(capacity: usize) -> Result<Self, String> {
        if capacity == 0 {
            return Err(
                "producer_consumer: the capacity must be at least 1; the buffer of capacity 0 holds \
                 nothing"
                    .to_owned(),
            );
        }
        Ok(Self {
            capacity,
            inner: Arc::new(BufferInner {
                items: Mutex::new(VecDeque::new()),
                not_empty: Condvar::new(),
                not_full: Condvar::new(),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// The capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The number of items in the buffer.
    pub fn len(&self) -> usize {
        match self.inner.items.lock() {
            Ok(items) => items.len(),
            Err(_) => 0,
        }
    }

    /// Whether the buffer holds no item.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the buffer is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Puts an item in the buffer and waits for the room when the buffer is full.
    pub fn produce(&self, item: T) -> Result<(), ProducerConsumerError> {
        let mut items = self.inner.items.lock().map_err(|_| ProducerConsumerError::Poisoned)?;
        loop {
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(ProducerConsumerError::Closed);
            }
            if items.len() < self.capacity {
                items.push_back(item);
                drop(items);
                self.inner.not_empty.notify_one();
                return Ok(());
            }
            // the condition is checked again in the loop: a wake up can come for nothing
            items = self
                .inner
                .not_full
                .wait(items)
                .map_err(|_| ProducerConsumerError::Poisoned)?;
        }
    }

    /// Puts an item in the buffer without waiting.
    ///
    /// The item is refused with `ProducerConsumerError::Full` when the buffer is full.
    pub fn try_produce(&self, item: T) -> Result<(), ProducerConsumerError> {
        let mut items = self.inner.items.lock().map_err(|_| ProducerConsumerError::Poisoned)?;
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(ProducerConsumerError::Closed);
        }
        if items.len() >= self.capacity {
            return Err(ProducerConsumerError::Full);
        }
        items.push_back(item);
        drop(items);
        self.inner.not_empty.notify_one();
        Ok(())
    }

    /// Takes an item out of the buffer and waits for one when the buffer is empty.
    ///
    /// The answer is `None` when the buffer is closed and empty: that is the end of the stream.
    pub fn consume(&self) -> Result<Option<T>, ProducerConsumerError> {
        let mut items = self.inner.items.lock().map_err(|_| ProducerConsumerError::Poisoned)?;
        loop {
            if let Some(item) = items.pop_front() {
                drop(items);
                self.inner.not_full.notify_one();
                return Ok(Some(item));
            }
            if self.inner.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            // the condition is checked again in the loop: a wake up can come for nothing
            items = self
                .inner
                .not_empty
                .wait(items)
                .map_err(|_| ProducerConsumerError::Poisoned)?;
        }
    }

    /// Takes an item out of the buffer without waiting.
    ///
    /// The answer is `None` when the buffer is empty.
    pub fn try_consume(&self) -> Result<Option<T>, ProducerConsumerError> {
        let mut items = self.inner.items.lock().map_err(|_| ProducerConsumerError::Poisoned)?;
        let item = items.pop_front();
        if item.is_some() {
            drop(items);
            self.inner.not_full.notify_one();
        }
        Ok(item)
    }

    /// Closes the buffer: it takes no new item and the consumers see the end of the stream once the
    /// items that are left are taken.
    ///
    /// The lock is taken before the flag is raised, so a thread that is waiting cannot miss the
    /// wake up.
    pub fn close(&self) {
        let _guard = self.inner.items.lock();
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.not_empty.notify_all();
        self.inner.not_full.notify_all();
    }
}

/// Builds the shared buffer of a producer and consumer pattern.
///
/// The producer writes in the buffer and the consumer reads from it; the buffer of a fixed capacity
/// slows down the faster side, and the condition variables make the two sides wait for each other.
///
/// # Input
///
/// - The capacity must be at least 1: a buffer of capacity 0 holds nothing and is refused.
/// - The items are of any type: the buffer only moves them around.
///
/// # Complexity
///
/// - Every hand over: `O(1)`.
/// - Extra space: `O(capacity)` - the memory of the buffer does not grow with the traffic.
///
/// # When it does not apply
///
/// - The card rejects it when the production and the consumption are one and the same: a call chain
///   is enough.
/// - The card rejects it when the order has to be strict and the throughput does not matter: one
///   thread is simpler.
/// - The card rejects it across the processes or the machines: a message broker is the answer.
/// - The card rejects it when the consumer is faster than the producer and the data can be thrown
///   away: an unbounded queue or a plain drop is enough.
///
/// * asset: `algorithm.concurrency.producer_consumer`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.concurrency.producer_consumer/card.md`
pub fn producer_consumer<T>(capacity: usize) -> Result<ProducerConsumer<T>, String> {
    ProducerConsumer::new(capacity)
}

// ---- algorithm.concurrency.work_stealing --------------------------------------------------------------------------

/// The deque of a worker: its own tasks.
#[derive(Debug)]
struct WorkerDeque<T> {
    tasks: Mutex<VecDeque<T>>,
}

/// What the workers share: the number of tasks that are not finished, and the wake up of the
/// workers that have nothing to do.
#[derive(Debug)]
struct SchedulerState {
    remaining: usize,
}

/// The inner part of the pool, shared by the workers.
#[derive(Debug)]
struct Scheduler {
    state: Mutex<SchedulerState>,
    wake: Condvar,
}

/// The pool: one deque per worker, and the workers steal from each other.
#[derive(Debug)]
struct Pool<T> {
    workers: Vec<WorkerDeque<T>>,
    scheduler: Arc<Scheduler>,
}

impl<T> Pool<T> {
    /// Takes a task: the own deque first (the head, the most recent task), then the deque of the
    /// other workers (the tail, the oldest task).
    fn find_task(&self, index: usize) -> Option<T> {
        {
            let mut tasks = self.workers[index].tasks.lock().ok()?;
            if let Some(task) = tasks.pop_front() {
                return Some(task);
            }
        }
        let count = self.workers.len();
        for offset in 1..count {
            let victim = (index + offset) % count;
            let mut tasks = self.workers[victim].tasks.lock().ok()?;
            if let Some(task) = tasks.pop_back() {
                return Some(task);
            }
        }
        None
    }
}

/// Runs the tasks on the workers of a pool, and lets the workers steal from each other.
///
/// Every worker has its own deque: it takes its own tasks from the head (the most recent one, the
/// one that is the friendliest for the cache) and steals the tasks of the other workers from the
/// tail (the oldest one). The two ends are two different ends of the same deque, so the worker and
/// the thief hardly ever fight for the same lock.
///
/// # Input
///
/// - `worker_count` must be at least 1: a pool without a worker runs nothing.
/// - `tasks` are the tasks to run. They are spread over the workers in turn.
/// - `work` is what runs a task. It receives the index of the worker and the task. It must be
///   callable from several threads at the same time.
///
/// # Complexity
///
/// - Push, pop and steal: `O(1)`.
/// - Extra space: `O(number of tasks)` - the deques of the workers.
///
/// # What is fixed and what is not
///
/// - The set of the tasks that run, and the fact that each of them runs exactly once, are fixed.
/// - The worker that runs a task is not fixed: it depends on the order the threads are scheduled
///   in. The victim of a steal is not fixed either: the workers try the others in a fixed rotation
///   (`index + 1`, `index + 2`, ...), and not in a random order, so that two runs of the same
///   program behave the same way. The card asks for a random victim to avoid the pile up of the
///   thieves on the same queue; the fixed rotation spreads them over the queues instead.
/// - A work function that panics does not stop the pool: the panic is caught, the task is counted
///   as run and the pool goes on, so that the other workers always end.
///
/// # When it does not apply
///
/// - The card rejects it when there are few tasks (fewer than the workers): a static distribution
///   is enough.
/// - The card rejects it when the tasks are very small (a few nanoseconds): the cost of the
///   scheduling eats the benefit.
/// - The card rejects it when the tasks have a strict order or a strong dependency: a scheduler that
///   knows the dependencies is needed.
/// - The card rejects it when the number of tasks is known in advance: a distribution by blocks is
///   simpler.
///
/// * asset: `algorithm.concurrency.work_stealing`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.concurrency.work_stealing/card.md`
pub fn work_stealing<T: Send + 'static, F: Fn(usize, T) + Send + Sync + 'static>(
    worker_count: usize,
    tasks: Vec<T>,
    work: F,
) -> Result<usize, String> {
    if worker_count == 0 {
        return Err("work_stealing: the pool must have at least one worker".to_owned());
    }

    let total = tasks.len();
    let mut workers: Vec<WorkerDeque<T>> = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        workers.push(WorkerDeque {
            tasks: Mutex::new(VecDeque::new()),
        });
    }
    // the tasks are spread over the workers in turn
    for (index, task) in tasks.into_iter().enumerate() {
        let worker = index % worker_count;
        if let Ok(mut deque) = workers[worker].tasks.lock() {
            deque.push_back(task);
        }
    }

    let scheduler = Arc::new(Scheduler {
        state: Mutex::new(SchedulerState { remaining: total }),
        wake: Condvar::new(),
    });
    let pool = Arc::new(Pool { workers, scheduler });
    let work = Arc::new(work);
    let executed = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(worker_count);
    for index in 0..worker_count {
        let pool = Arc::clone(&pool);
        let work = Arc::clone(&work);
        let executed = Arc::clone(&executed);
        handles.push(std::thread::spawn(move || {
            loop {
                match pool.find_task(index) {
                    Some(task) => {
                        // a panic of the work must not stop the pool: the task is counted as run
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            work(index, task);
                        }));
                        executed.fetch_add(1, Ordering::SeqCst);
                        let finished = {
                            let mut state = match pool.scheduler.state.lock() {
                                Ok(state) => state,
                                Err(_) => return,
                            };
                            state.remaining -= 1;
                            state.remaining == 0
                        };
                        pool.scheduler.wake.notify_all();
                        if finished {
                            break;
                        }
                    }
                    None => {
                        let mut state = match pool.scheduler.state.lock() {
                            Ok(state) => state,
                            Err(_) => return,
                        };
                        if state.remaining == 0 {
                            break;
                        }
                        // the worker waits until a task is finished or another task appears
                        state = match pool.scheduler.wake.wait(state) {
                            Ok(state) => state,
                            Err(_) => return,
                        };
                        if state.remaining == 0 {
                            break;
                        }
                    }
                }
            }
        }));
    }

    for handle in handles {
        let _ = handle.join();
    }
    Ok(executed.load(Ordering::SeqCst))
}

// ---- algorithm.concurrency.exponential_backoff_jitter -------------------------------------------------------------

/// The ceiling of the backoff of an attempt: `base * 2^attempt`, capped.
fn backoff_ceiling(base_ms: u64, cap_ms: u64, attempt: u32) -> u64 {
    let exponent = 1u64 << attempt.min(63);
    base_ms.saturating_mul(exponent).min(cap_ms)
}

/// The wait before a retry, with the jitter that keeps the clients from waking up together.
///
/// The wait grows like `base * 2^attempt`, it never passes the cap, and the jitter spreads it over
/// the half of the ceiling: the wait is in `[ceiling / 2, ceiling]`.
///
/// The function is pure: it reads no clock and it sleeps for nothing. The caller owns the random
/// source and the sleeping, and that is what makes the behaviour testable.
///
/// # Input
///
/// - `base_ms` is the delay of the first retry. It must be at least 1 millisecond.
/// - `cap_ms` is the ceiling of the delay. It is required: a backoff without a ceiling never ends.
///   It must not be below `base_ms`, otherwise the ceiling would swallow the base from the start.
/// - `attempt` is the number of the retry, counted from zero. The attempt is a `u32`, so a negative
///   number of retries cannot be written down.
/// - `random` is the value the random source gave. The same value always gives the same wait.
///
/// # Complexity
///
/// - Time: `O(1)`. Space: `O(1)`.
///
/// # When it does not apply
///
/// - The card rejects it when the error is certain (an invalid argument): a retry only wastes time.
/// - The card rejects it when the operation is not reversible and has no idempotency key: the retry
///   makes the effect twice.
/// - The card rejects it when the user is waiting for the answer: a long backoff makes the answer
///   late, and a fast failure is better.
/// - The card rejects it when the service is already open-circuited: the retry goes around the
///   protection.
/// - The card rejects it for the decision itself ("should this be retried?"): this function only
///   computes the waiting time.
///
/// * asset: `algorithm.concurrency.exponential_backoff_jitter`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.concurrency.exponential_backoff_jitter/card.md`
pub fn exponential_backoff_jitter(
    base_ms: u64,
    cap_ms: u64,
    attempt: u32,
    random: u64,
) -> Result<u64, String> {
    if base_ms == 0 {
        return Err(
            "exponential_backoff_jitter: the base delay must be at least 1 millisecond".to_owned(),
        );
    }
    if cap_ms == 0 {
        return Err(
            "exponential_backoff_jitter: a ceiling is required; a backoff without a ceiling never \
             ends"
                .to_owned(),
        );
    }
    if cap_ms < base_ms {
        return Err(format!(
            "exponential_backoff_jitter: the ceiling ({cap_ms} ms) must not be below the base delay \
             ({base_ms} ms)"
        ));
    }
    let ceiling = backoff_ceiling(base_ms, cap_ms, attempt);
    let floor = ceiling / 2;
    // the span holds the jitter: from the floor to the ceiling, both included
    let span = ceiling - floor + 1;
    Ok(floor + random % span)
}

// ---- algorithm.concurrency.token_bucket ---------------------------------------------------------------------------

/// The state the bucket protects with its lock.
#[derive(Debug)]
struct BucketState {
    tokens: u64,
    last_ms: u64,
}

/// A token bucket: the tokens are added at a fixed rate and the requests take one.
///
/// The bucket limits the average rate and lets a burst through: it holds up to `capacity` tokens,
/// and it fills up again at the rate of `refill_per_second` tokens per second.
///
/// The clock is a parameter: the caller passes the current time in milliseconds, and the time it
/// passes must not go backwards. A caller that has a monotonic clock passes the time it reads from
/// it, and the bucket behaves the same whether the second is real or imagined.
#[derive(Debug)]
pub struct TokenBucket {
    capacity: u64,
    refill_per_second: u64,
    state: Mutex<BucketState>,
}

impl TokenBucket {
    /// Builds a bucket that is full at the given time.
    ///
    /// - The capacity must be at least 1: a bucket of capacity 0 never grants a token, so it is
    ///   refused.
    /// - The rate may be zero: the bucket then grants the tokens of its first burst and nothing
    ///   more.
    /// - `now_ms` is the current time, in milliseconds. The time the caller passes later must not be
    ///   smaller.
    pub fn new(capacity: u64, refill_per_second: u64, now_ms: u64) -> Result<Self, String> {
        if capacity == 0 {
            return Err(
                "token_bucket: the capacity must be at least 1; a bucket of capacity 0 never grants \
                 a token"
                    .to_owned(),
            );
        }
        Ok(Self {
            capacity,
            refill_per_second,
            state: Mutex::new(BucketState {
                tokens: capacity,
                last_ms: now_ms,
            }),
        })
    }

    /// The capacity of the bucket.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// The number of tokens the bucket adds per second.
    pub fn refill_per_second(&self) -> u64 {
        self.refill_per_second
    }

    /// Adds the tokens of the time that passed and checks that the clock did not go backwards.
    fn refill(&self, state: &mut BucketState, now_ms: u64) -> Result<(), String> {
        if now_ms < state.last_ms {
            return Err(format!(
                "token_bucket: the clock went backwards ({now_ms} ms after {} ms); the bucket needs \
                 a monotonic clock",
                state.last_ms
            ));
        }
        let elapsed = now_ms - state.last_ms;
        let added = (u128::from(elapsed) * u128::from(self.refill_per_second)) / 1000;
        let added = u64::try_from(added).unwrap_or(u64::MAX);
        state.tokens = state.tokens.saturating_add(added).min(self.capacity);
        state.last_ms = now_ms;
        Ok(())
    }

    /// The number of tokens the bucket holds at the given time.
    pub fn available_tokens(&self, now_ms: u64) -> Result<u64, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "token_bucket: the lock of the bucket was poisoned by another thread")?;
        self.refill(&mut state, now_ms)?;
        Ok(state.tokens)
    }

    /// Takes the tokens, when the bucket holds them.
    ///
    /// The answer is `true` when the request goes through, and the tokens are taken. The answer is
    /// `false` when the bucket does not hold enough tokens, and nothing is taken.
    pub fn try_acquire(&self, now_ms: u64, tokens: u64) -> Result<bool, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "token_bucket: the lock of the bucket was poisoned by another thread")?;
        // the addition, the check and the take are in one critical section: that is what keeps the
        // bucket from granting more tokens than it holds
        self.refill(&mut state, now_ms)?;
        if tokens == 0 {
            return Ok(true);
        }
        if tokens > self.capacity {
            return Ok(false);
        }
        if state.tokens >= tokens {
            state.tokens -= tokens;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Builds a token bucket that is full at the given time.
///
/// The bucket adds `refill_per_second` tokens per second, up to `capacity` tokens, and a request
/// goes through only when it takes the tokens it asks for.
///
/// # Input
///
/// - The capacity must be at least 1: a bucket of capacity 0 never grants a token and is refused.
/// - The rate may be zero: the bucket then grants its first burst and nothing more.
/// - `now_ms` is the current time in milliseconds. The time passed later must not go backwards.
///
/// # Complexity
///
/// - Every decision: `O(1)`.
/// - Extra space: `O(1)` - a few numbers.
///
/// # When it does not apply
///
/// - The card rejects it when no burst is allowed and the output has to be smooth: a leaky bucket
///   is the answer.
/// - The card rejects it when the number of the simultaneous requests is what has to be limited: a
///   semaphore is more direct.
/// - The card rejects it when the calls are internal and trusted and cannot overload anything: the
///   limiter only adds a delay.
/// - The card rejects it when the requests have different weights and a caller could go around it:
///   the implementation has to be extended to hold the weights.
///
/// * asset: `algorithm.concurrency.token_bucket`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.concurrency.token_bucket/card.md`
pub fn token_bucket(
    capacity: u64,
    refill_per_second: u64,
    now_ms: u64,
) -> Result<TokenBucket, String> {
    TokenBucket::new(capacity, refill_per_second, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
    use std::sync::Barrier;
    use std::time::Duration;

    /// The time the tests wait for before they give up.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// The time the tests wait to see that nothing happens.
    const QUIET: Duration = Duration::from_millis(200);

    // ---- algorithm.concurrency.bounded_queue

    #[test]
    fn bounded_queue_keeps_the_fifo_order_and_refuses_when_it_is_full() {
        let queue: BoundedQueue<usize> = bounded_queue(2, Backpressure::Reject).unwrap();
        assert_eq!(queue.capacity(), 2);
        assert_eq!(queue.policy(), Backpressure::Reject);
        assert!(queue.is_empty());
        assert_eq!(queue.push(1), Ok(()));
        assert_eq!(queue.push(2), Ok(()));
        // the queue is full: the policy refuses the third item and keeps the two first ones
        assert_eq!(queue.push(3), Err(QueueError::Full));
        assert_eq!(queue.len(), 2);
        // the order is the order of the entries
        assert_eq!(queue.try_pop(), Ok(Some(1)));
        assert_eq!(queue.try_pop(), Ok(Some(2)));
        assert_eq!(queue.try_pop(), Ok(None));
        // the closed queue takes no item and ends the stream
        queue.close();
        assert!(queue.is_closed());
        assert_eq!(queue.push(4), Err(QueueError::Closed));
        assert_eq!(queue.pop(), Ok(None));
    }

    #[test]
    fn bounded_queue_blocks_the_producer_when_it_is_full() {
        let queue: BoundedQueue<usize> = bounded_queue(1, Backpressure::Block).unwrap();
        assert_eq!(queue.push(1), Ok(()));
        // the queue is full: the producer of the second item has to wait
        let (sender, receiver): (Sender<()>, Receiver<()>) = channel();
        let producer = {
            let queue = queue.clone();
            std::thread::spawn(move || {
                queue.push(2).unwrap();
                sender.send(()).unwrap();
            })
        };
        // the wait of the producer really is a wait: nothing comes out of it
        assert!(matches!(
            receiver.recv_timeout(QUIET),
            Err(RecvTimeoutError::Timeout)
        ));
        // taking an item out frees the room and the producer goes on
        assert_eq!(queue.pop(), Ok(Some(1)));
        assert!(receiver.recv_timeout(TIMEOUT).is_ok());
        producer.join().unwrap();
        assert_eq!(queue.pop(), Ok(Some(2)));
    }

    #[test]
    fn bounded_queue_keeps_every_item_with_many_producers_and_consumers() {
        let queue: BoundedQueue<usize> = bounded_queue(8, Backpressure::Block).unwrap();
        let producers = 4usize;
        let per_producer = 25usize;
        let barrier = Arc::new(Barrier::new(producers));
        let (sender, receiver): (Sender<()>, Receiver<()>) = channel();

        let mut handles = Vec::new();
        for producer in 0..producers {
            let queue = queue.clone();
            let barrier = Arc::clone(&barrier);
            let sender = sender.clone();
            handles.push(std::thread::spawn(move || {
                // all the producers start at the same time
                barrier.wait();
                for item in 0..per_producer {
                    queue.push(producer * per_producer + item).unwrap();
                }
                sender.send(()).unwrap();
            }));
        }
        drop(sender);

        let consumed: Arc<Mutex<HashMap<usize, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut consumers = Vec::new();
        for _ in 0..2 {
            let queue = queue.clone();
            let consumed = Arc::clone(&consumed);
            consumers.push(std::thread::spawn(move || {
                while let Ok(Some(item)) = queue.pop() {
                    *consumed.lock().unwrap().entry(item).or_insert(0) += 1;
                }
            }));
        }

        // the producers have to finish before the queue is closed
        for _ in 0..producers {
            receiver.recv_timeout(TIMEOUT).unwrap();
        }
        queue.close();
        for consumer in consumers {
            consumer.join().unwrap();
        }
        for handle in handles {
            handle.join().unwrap();
        }

        // every item is there once, and only once
        let consumed = consumed.lock().unwrap();
        let total: usize = consumed.values().sum();
        assert_eq!(total, producers * per_producer);
        assert_eq!(consumed.len(), producers * per_producer);
        for item in 0..producers * per_producer {
            assert_eq!(consumed.get(&item), Some(&1), "the item {item} is not there once");
        }
    }

    #[test]
    fn bounded_queue_rejects_a_capacity_of_zero() {
        // the card lists the capacity of zero as an extreme input: this implementation refuses it
        // instead of building a queue that cannot carry anything
        let error = bounded_queue::<usize>(0, Backpressure::Block).unwrap_err();
        assert!(error.contains("capacity"), "the reason must be readable: {error}");
    }

    // ---- algorithm.concurrency.producer_consumer

    #[test]
    fn producer_consumer_hands_every_item_over_without_loss_or_duplication() {
        let buffer: ProducerConsumer<usize> = producer_consumer(4).unwrap();
        let producers = 3usize;
        let per_producer = 20usize;
        let barrier = Arc::new(Barrier::new(producers));
        let (sender, receiver): (Sender<()>, Receiver<()>) = channel();

        let mut handles = Vec::new();
        for producer in 0..producers {
            let buffer = buffer.clone();
            let barrier = Arc::clone(&barrier);
            let sender = sender.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for item in 0..per_producer {
                    buffer.produce(producer * per_producer + item).unwrap();
                }
                sender.send(()).unwrap();
            }));
        }
        drop(sender);

        let consumed: Arc<Mutex<HashMap<usize, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut consumers = Vec::new();
        for _ in 0..2 {
            let buffer = buffer.clone();
            let consumed = Arc::clone(&consumed);
            consumers.push(std::thread::spawn(move || {
                while let Ok(Some(item)) = buffer.consume() {
                    *consumed.lock().unwrap().entry(item).or_insert(0) += 1;
                }
            }));
        }

        for _ in 0..producers {
            receiver.recv_timeout(TIMEOUT).unwrap();
        }
        buffer.close();
        for consumer in consumers {
            consumer.join().unwrap();
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let consumed = consumed.lock().unwrap();
        let total: usize = consumed.values().sum();
        assert_eq!(total, producers * per_producer);
        assert_eq!(consumed.len(), producers * per_producer);
        for item in 0..producers * per_producer {
            assert_eq!(consumed.get(&item), Some(&1), "the item {item} is not there once");
        }
    }

    #[test]
    fn producer_consumer_ends_the_consumers_when_it_is_closed() {
        let buffer: ProducerConsumer<usize> = producer_consumer(2).unwrap();
        // the capacity is really a capacity
        assert_eq!(buffer.produce(1), Ok(()));
        assert_eq!(buffer.produce(2), Ok(()));
        assert_eq!(buffer.try_produce(3), Err(ProducerConsumerError::Full));
        assert_eq!(buffer.len(), 2);
        // the closed buffer ends the stream for the consumer
        buffer.close();
        assert!(buffer.is_closed());
        assert_eq!(buffer.consume(), Ok(Some(1)));
        assert_eq!(buffer.consume(), Ok(Some(2)));
        assert_eq!(buffer.consume(), Ok(None));
        assert_eq!(buffer.produce(4), Err(ProducerConsumerError::Closed));
        // the capacity of zero is refused
        let error = producer_consumer::<usize>(0).unwrap_err();
        assert!(error.contains("capacity"), "the reason must be readable: {error}");
    }

    // ---- algorithm.concurrency.work_stealing

    #[test]
    fn work_stealing_runs_every_task_exactly_once() {
        let workers = 4usize;
        let tasks: Vec<usize> = (0..64usize).collect();
        let executed: Arc<Mutex<HashMap<usize, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let counter = Arc::clone(&executed);
        let count = work_stealing(workers, tasks, move |_worker: usize, task: usize| {
            *counter.lock().unwrap().entry(task).or_insert(0) += 1;
        })
        .unwrap();
        assert_eq!(count, 64);
        let executed = executed.lock().unwrap();
        assert_eq!(executed.len(), 64);
        for task in 0..64usize {
            assert_eq!(executed.get(&task), Some(&1), "the task {task} did not run once");
        }
        // the pool without a worker is refused
        let error = work_stealing(0usize, Vec::<usize>::new(), |_worker, _task: usize| {}).unwrap_err();
        assert!(error.contains("worker"), "the reason must be readable: {error}");
    }

    #[test]
    fn work_stealing_steals_a_task_from_another_worker() {
        let workers = 3usize;
        let tasks: Vec<usize> = (0..7usize).collect();
        // the owner of a task is the worker it was given to: the tasks are spread in turn
        let owner = move |task: usize| task % workers;
        // the blocker is the last task of the worker 0: it waits until a task is really stolen
        let blocker = 6usize;
        let stolen = Arc::new(AtomicBool::new(false));
        let flag = stolen.clone();
        let lock = Arc::new(Mutex::new(()));
        let wait_lock = Arc::clone(&lock);
        let condition = Arc::new(Condvar::new());
        let wait_condition = Arc::clone(&condition);
        let executed: Arc<Mutex<Vec<(usize, usize)>>> = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&executed);

        let count = work_stealing(workers, tasks, move |worker: usize, task: usize| {
            if worker != owner(task) {
                // a real steal: the flag is raised under the lock, so the blocker cannot miss it
                let _guard = wait_lock.lock().unwrap();
                flag.store(true, Ordering::SeqCst);
                wait_condition.notify_all();
            }
            if task == blocker {
                // the blocker waits for the steal, with a timeout so that a broken pool fails the
                // test instead of hanging it
                let mut guard = wait_lock.lock().unwrap();
                if !flag.load(Ordering::SeqCst) {
                    let (next, _) = wait_condition.wait_timeout(guard, TIMEOUT).unwrap();
                    guard = next;
                    drop(guard);
                }
            }
            record.lock().unwrap().push((worker, task));
        })
        .unwrap();

        assert_eq!(count, 7);
        assert!(
            stolen.load(Ordering::SeqCst),
            "no task was stolen from another worker"
        );
        let executed = executed.lock().unwrap();
        assert_eq!(executed.len(), 7);
        for task in 0..7usize {
            let times = executed
                .iter()
                .filter(|(_, ran)| *ran == task)
                .count();
            assert_eq!(times, 1, "the task {task} did not run once");
        }
    }

    // ---- algorithm.concurrency.exponential_backoff_jitter

    #[test]
    fn exponential_backoff_jitter_grows_exponentially_and_stays_under_the_cap() {
        let base = 100u64;
        let cap = 10_000u64;
        for attempt in 0..=6u32 {
            let ceiling = (base << attempt).min(cap);
            for random in [0u64, 1, 2, 7, 12345, u64::MAX] {
                let wait = exponential_backoff_jitter(base, cap, attempt, random).unwrap();
                assert!(
                    (ceiling / 2..=ceiling).contains(&wait),
                    "the wait {wait} is not in [{}, {ceiling}]",
                    ceiling / 2
                );
            }
        }
        // the ceiling holds: the wait never passes it, whatever the attempt
        for attempt in 0..64u32 {
            let wait = exponential_backoff_jitter(base, cap, attempt, u64::MAX).unwrap();
            assert!(wait <= cap, "the wait {wait} passes the ceiling {cap}");
        }
        // the same inputs always give the same wait
        assert_eq!(
            exponential_backoff_jitter(base, cap, 3, 42).unwrap(),
            exponential_backoff_jitter(base, cap, 3, 42).unwrap()
        );
        // the wait grows with the attempt
        let first = exponential_backoff_jitter(base, cap, 0, 1).unwrap();
        let later = exponential_backoff_jitter(base, cap, 3, 1).unwrap();
        assert!(later > first, "the wait did not grow: {first} then {later}");
    }

    #[test]
    fn exponential_backoff_jitter_rejects_the_configuration_that_cannot_back_off() {
        // a base of zero is not a delay
        assert!(exponential_backoff_jitter(0, 100, 0, 1).is_err());
        // a backoff without a ceiling never ends
        assert!(exponential_backoff_jitter(100, 0, 0, 1).is_err());
        // a ceiling below the base swallows the base from the start
        assert!(exponential_backoff_jitter(100, 50, 0, 1).is_err());
        // the reason is readable
        let error = exponential_backoff_jitter(0, 100, 0, 1).unwrap_err();
        assert!(error.contains("base"), "the reason must be readable: {error}");
    }

    // ---- algorithm.concurrency.token_bucket

    #[test]
    fn token_bucket_grants_the_burst_and_then_refuses() {
        let bucket: TokenBucket = token_bucket(5, 10, 0).unwrap();
        assert_eq!(bucket.capacity(), 5);
        assert_eq!(bucket.refill_per_second(), 10);
        // the bucket starts full: five tokens and nothing more
        for _ in 0..5 {
            assert_eq!(bucket.try_acquire(0, 1), Ok(true));
        }
        assert_eq!(bucket.try_acquire(0, 1), Ok(false));
        assert_eq!(bucket.available_tokens(0), Ok(0));
        // half a second later the bucket holds five tokens again
        for _ in 0..5 {
            assert_eq!(bucket.try_acquire(500, 1), Ok(true));
        }
        assert_eq!(bucket.try_acquire(500, 1), Ok(false));
        // the capacity holds: the tokens of a long wait do not pile up
        assert_eq!(bucket.available_tokens(10_000), Ok(5));
        // a request of more tokens than the capacity can ever hold is refused
        assert_eq!(bucket.try_acquire(10_000, 6), Ok(false));
        // a request of no token at all goes through and takes nothing
        assert_eq!(bucket.try_acquire(10_000, 0), Ok(true));
        // the clock has to go forward
        let error = bucket.try_acquire(9_999, 1).unwrap_err();
        assert!(error.contains("backwards"), "the reason must be readable: {error}");
    }

    #[test]
    fn token_bucket_never_grants_more_than_the_capacity_under_concurrency() {
        let bucket = Arc::new(token_bucket(5, 10, 0).unwrap());
        let threads = 8usize;
        let attempts = 50usize;
        let barrier = Arc::new(Barrier::new(threads));
        let (sender, receiver): (Sender<usize>, Receiver<usize>) = channel();
        let mut handles = Vec::new();
        for _ in 0..threads {
            let bucket = Arc::clone(&bucket);
            let barrier = Arc::clone(&barrier);
            let sender = sender.clone();
            handles.push(std::thread::spawn(move || {
                let mut granted = 0usize;
                barrier.wait();
                for _ in 0..attempts {
                    // the clock does not move: the bucket cannot be refilled during the burst
                    if bucket.try_acquire(0, 1) == Ok(true) {
                        granted += 1;
                    }
                }
                sender.send(granted).unwrap();
            }));
        }
        drop(sender);
        let mut total = 0usize;
        for _ in 0..threads {
            total += receiver.recv_timeout(TIMEOUT).unwrap();
        }
        for handle in handles {
            handle.join().unwrap();
        }
        // the capacity is the ceiling of the burst: no thread can be given a token that the bucket
        // does not hold
        assert_eq!(total, 5);
        assert_eq!(bucket.available_tokens(0), Ok(0));
    }

    #[test]
    fn token_bucket_rejects_a_bucket_without_capacity() {
        // the card lists the capacity of zero as an extreme input: a bucket that can hold nothing
        // never grants a token, so it is refused
        let error = token_bucket(0, 10, 0).unwrap_err();
        assert!(error.contains("capacity"), "the reason must be readable: {error}");
        // a rate of zero is legal: the bucket grants its first burst and nothing more
        let bucket: TokenBucket = token_bucket(2, 0, 0).unwrap();
        assert_eq!(bucket.try_acquire(0, 1), Ok(true));
        assert_eq!(bucket.try_acquire(0, 1), Ok(true));
        assert_eq!(bucket.try_acquire(0, 1), Ok(false));
        assert_eq!(bucket.try_acquire(60_000, 1), Ok(false));
    }
}
