//! The queues of the scheduler: the five layered work classes of the Hub and the deficit round robin over
//! the agents (detailed design §17.5).
//!
//! The design book gives the Hub five queues — `interactive_candidate`, `preview_read`, `prepare`, `verify`
//! and `commit` — and one rule: "全局对每 Agent 使用加权轮转/ deficit round robin，避免大量低成本读请求淹没验证工作"
//! (a global weighted round robin over the agents, so that a flood of cheap reads does not drown the
//! verification work).
//!
//! The two levels of the queue are implemented here:
//!
//! * **the class** decides how soon a task runs. The class of the design book has a base priority, and the
//!   aging ([`Aging`]) gives a step of priority to a class that has been waiting long enough, so that a
//!   stream of fresh cheap work cannot postpone a `commit` for ever;
//! * **the agent** decides who runs inside a class. Every visit of an agent grants it `weight × quantum`
//!   credits and the agent spends what it can: while every queue stays full, the share of the agents
//!   follows their weights, and no agent is starved.
//!
//! The crate has no clock of its own: the caller passes the instant of every decision, so a test can move
//! the clock by exactly the duration it needs.

use crate::Timestamp;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::Duration;
use tase_contract::{AgentId, JobId, TaseError};

/// The five layered queues of the Hub (detailed design §17.5).
///
/// The design book names them `interactive_candidate`, `preview_read`, `prepare`, `verify` and `commit`. The
/// order of the enum is the order of the priority: the candidate of the keyboard comes first, the commit
/// comes last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkClass {
    /// The candidate of the keyboard: the answer the user is waiting for.
    InteractiveCandidate,
    /// A read of a preview.
    PreviewRead,
    /// The preparation of a change.
    Prepare,
    /// The verification of a change.
    Verify,
    /// The commit of a change.
    Commit,
}

impl WorkClass {
    /// Every class, in the order of the priority.
    pub const ALL: [WorkClass; 5] = [
        WorkClass::InteractiveCandidate,
        WorkClass::PreviewRead,
        WorkClass::Prepare,
        WorkClass::Verify,
        WorkClass::Commit,
    ];

    /// The name of the class in the design book.
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkClass::InteractiveCandidate => "interactive_candidate",
            WorkClass::PreviewRead => "preview_read",
            WorkClass::Prepare => "prepare",
            WorkClass::Verify => "verify",
            WorkClass::Commit => "commit",
        }
    }

    /// The priority of the class when nothing is waiting: the smaller the number, the sooner the class runs.
    pub const fn base_priority(self) -> i32 {
        match self {
            WorkClass::InteractiveCandidate => 0,
            WorkClass::PreviewRead => 1,
            WorkClass::Prepare => 2,
            WorkClass::Verify => 3,
            WorkClass::Commit => 4,
        }
    }
}

impl fmt::Display for WorkClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A unit of work that waits in a queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// The job the task runs.
    pub job_id: JobId,
    /// The agent that asked for the task.
    pub agent_id: AgentId,
    /// The class of the task.
    pub class: WorkClass,
    /// The cost of the task in credit units: a cheap read costs one, a full compile costs many.
    pub cost: u32,
    /// The instant the task was queued.
    pub enqueued_at: Timestamp,
}

impl Task {
    /// A task of one credit unit, queued at `now`.
    pub fn new(job_id: JobId, agent_id: AgentId, class: WorkClass, now: Timestamp) -> Self {
        Task {
            job_id,
            agent_id,
            class,
            cost: 1,
            enqueued_at: now,
        }
    }

    /// The same task with another cost; a cost of zero becomes one.
    pub fn with_cost(mut self, cost: u32) -> Self {
        self.cost = cost.max(1);
        self
    }
}

/// The aging of the classes: a class that has been waiting long enough is promoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aging {
    /// How long the oldest task of a class waits before the class gains one step of priority.
    pub interval: Duration,
    /// The biggest number of steps the aging may give.
    pub max_boost: u32,
}

impl Default for Aging {
    fn default() -> Self {
        Aging {
            interval: Duration::from_secs(1),
            max_boost: 8,
        }
    }
}

/// The queue of one agent inside one class.
#[derive(Clone, Debug)]
struct AgentQueue {
    agent_id: AgentId,
    weight: u32,
    deficit: u64,
    tasks: VecDeque<Task>,
}

/// The queue of one class: the agents of that class and the deficit round robin over them.
///
/// The quantum is the unit of the credit: an agent of a weight of `w` gains `w × quantum` credits per
/// visit. The credit an agent cannot spend stays with it, so an expensive task is not lost behind the cheap
/// ones; the credit of an agent that has nothing to run is dropped, so an idle agent does not come back with
/// a pile of credit and take the head of the class.
#[derive(Clone, Debug)]
pub struct ClassQueue {
    class: WorkClass,
    quantum: u32,
    agents: Vec<AgentQueue>,
    cursor: usize,
    ready: VecDeque<Task>,
}

impl ClassQueue {
    /// An empty queue of a class.
    pub fn new(class: WorkClass) -> Self {
        ClassQueue {
            class,
            quantum: 1,
            agents: Vec::new(),
            cursor: 0,
            ready: VecDeque::new(),
        }
    }

    /// The class of the queue.
    pub fn class(&self) -> WorkClass {
        self.class
    }

    /// The number of the tasks that wait in the queue.
    ///
    /// The tasks that were taken out of the queue of an agent and that wait in the hand-over still count:
    /// they are not finished yet.
    pub fn len(&self) -> usize {
        let waiting: usize = self.agents.iter().map(|queue| queue.tasks.len()).sum();
        waiting + self.ready.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of the tasks of an agent that wait in the queue.
    pub fn len_of_agent(&self, agent_id: &AgentId) -> usize {
        self.agents
            .iter()
            .find(|queue| &queue.agent_id == agent_id)
            .map_or(0, |queue| queue.tasks.len())
    }

    /// The instant the oldest task of the queue was queued.
    pub fn oldest(&self) -> Option<Timestamp> {
        self.agents
            .iter()
            .filter_map(|queue| queue.tasks.front().map(|task| task.enqueued_at))
            .min()
    }

    /// Queues a task for an agent; the queue of the agent is created when it is the first task.
    pub fn push(&mut self, task: Task, weight: u32) {
        match self
            .agents
            .iter_mut()
            .find(|queue| queue.agent_id == task.agent_id)
        {
            Some(queue) => queue.tasks.push_back(task),
            None => {
                let mut queue = AgentQueue {
                    agent_id: task.agent_id.clone(),
                    weight,
                    deficit: 0,
                    tasks: VecDeque::new(),
                };
                queue.tasks.push_back(task);
                self.agents.push(queue);
            }
        }
    }

    /// Changes the weight of an agent of this queue.
    pub fn set_weight(&mut self, agent_id: &AgentId, weight: u32) {
        if let Some(queue) = self.agents.iter_mut().find(|queue| &queue.agent_id == agent_id) {
            queue.weight = weight;
        }
    }

    /// The next task of the queue, or `None` when the queue is empty.
    ///
    /// The task is the one of the agent that the round robin visits next; the agents are visited in the
    /// order of the cursor, and every visit grants the credit of the weight of the agent.
    pub fn next_task(&mut self) -> Option<Task> {
        if let Some(task) = self.ready.pop_front() {
            return Some(task);
        }
        let count = self.agents.len();
        if count == 0 {
            return None;
        }
        let start = self.cursor;
        for step in 0..count {
            let index = (start + step) % count;
            let served = self.visit(index);
            self.cursor = (index + 1) % count;
            if !served.is_empty() {
                self.ready.extend(served);
                return self.ready.pop_front();
            }
        }
        None
    }

    /// Grants the credit of one visit to an agent and takes the tasks that the credit pays for.
    fn visit(&mut self, index: usize) -> Vec<Task> {
        let quantum = u64::from(self.quantum);
        let queue = &mut self.agents[index];
        if queue.tasks.is_empty() {
            queue.deficit = 0;
            return Vec::new();
        }
        queue.deficit = queue.deficit.saturating_add(u64::from(queue.weight) * quantum);
        let mut served = Vec::new();
        while let Some(cost) = queue.tasks.front().map(|task| u64::from(task.cost)) {
            if cost > queue.deficit {
                break;
            }
            queue.deficit -= cost;
            if let Some(task) = queue.tasks.pop_front() {
                served.push(task);
            }
        }
        served
    }
}

/// The scheduler of the Hub: the five classes of the design book, the weight of the agents and the aging.
///
/// The scheduler is a pure in-memory structure: it holds the tasks that wait, it decides which one runs
/// next and it forgets everything else. The caller passes the instant of every decision.
#[derive(Clone, Debug)]
pub struct Scheduler {
    classes: Vec<ClassQueue>,
    agents: BTreeMap<AgentId, u32>,
    aging: Aging,
}

impl Scheduler {
    /// A scheduler with the five classes of the design book and the default aging.
    pub fn new() -> Self {
        Scheduler {
            classes: WorkClass::ALL
                .iter()
                .map(|class| ClassQueue::new(*class))
                .collect(),
            agents: BTreeMap::new(),
            aging: Aging::default(),
        }
    }

    /// The same scheduler with another aging.
    pub fn with_aging(mut self, aging: Aging) -> Self {
        self.aging = aging;
        self
    }

    /// The same scheduler with another quantum: the unit of the credit of the round robin.
    pub fn with_quantum(mut self, quantum: u32) -> Self {
        for class in &mut self.classes {
            class.quantum = quantum.max(1);
        }
        self
    }

    /// The aging of the scheduler.
    pub fn aging(&self) -> Aging {
        self.aging
    }

    /// The five classes of the scheduler, in the order of the priority.
    pub fn classes(&self) -> &[ClassQueue] {
        &self.classes
    }

    /// The queue of a class, or `None` when the scheduler has no such class.
    pub fn class_queue(&self, class: WorkClass) -> Option<&ClassQueue> {
        self.classes
            .iter()
            .find(|queue| queue.class() == class)
    }

    /// Registers an agent with its weight; a weight is at least one.
    pub fn register_agent(&mut self, agent_id: AgentId, weight: u32) -> Result<(), TaseError> {
        if weight == 0 {
            return Err(TaseError::invalid_argument(
                "the weight of an agent must be at least one",
            ));
        }
        self.agents.insert(agent_id.clone(), weight);
        for class in &mut self.classes {
            class.set_weight(&agent_id, weight);
        }
        Ok(())
    }

    /// The weight of an agent, or `None` when the agent is not registered.
    pub fn weight_of(&self, agent_id: &AgentId) -> Option<u32> {
        self.agents.get(agent_id).copied()
    }

    /// Queues a task; the agent of the task must be registered.
    pub fn submit(&mut self, task: Task) -> Result<(), TaseError> {
        let weight = self.agents.get(&task.agent_id).copied().ok_or_else(|| {
            TaseError::invalid_argument(format!("the agent {} is not registered", task.agent_id))
        })?;
        let class = task.class;
        let index = WorkClass::ALL
            .iter()
            .position(|known| *known == class)
            .ok_or_else(|| TaseError::invalid_argument(format!("unknown class {class}")))?;
        self.classes[index].push(task, weight);
        Ok(())
    }

    /// The number of the tasks that wait in every class.
    pub fn len(&self) -> usize {
        self.classes.iter().map(|class| class.len()).sum()
    }

    /// Whether every class is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The next task to run, or `None` when every class is empty.
    ///
    /// The class with the smallest effective priority runs first; a class that has been waiting long enough
    /// has a smaller effective priority than a younger one. Inside the class, the deficit round robin
    /// chooses the agent.
    pub fn next_task(&mut self, now: Timestamp) -> Option<Task> {
        let mut best: Option<usize> = None;
        let mut best_priority = i32::MAX;
        for (index, class) in self.classes.iter().enumerate() {
            if class.is_empty() {
                continue;
            }
            let priority = self.effective_priority(class, now);
            if priority < best_priority {
                best_priority = priority;
                best = Some(index);
            }
        }
        let first = best?;
        for step in 0..self.classes.len() {
            let index = (first + step) % self.classes.len();
            if let Some(task) = self.classes[index].next_task() {
                return Some(task);
            }
        }
        None
    }

    /// The priority of a class at `now`, after the aging.
    fn effective_priority(&self, class: &ClassQueue, now: Timestamp) -> i32 {
        class.class().base_priority() - self.aging_boost(class, now)
    }

    /// The number of the steps of priority the aging gives to a class at `now`.
    fn aging_boost(&self, class: &ClassQueue, now: Timestamp) -> i32 {
        let interval = u64::try_from(self.aging.interval.as_millis()).unwrap_or(u64::MAX);
        if interval == 0 {
            return 0;
        }
        let Some(oldest) = class.oldest() else {
            return 0;
        };
        let waited = now.saturating_duration_since(oldest).as_millis();
        let steps = waited / u128::from(interval);
        let steps = u32::try_from(steps).unwrap_or(u32::MAX);
        i32::try_from(steps.min(self.aging.max_boost)).unwrap_or(i32::MAX)
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogicalClock;
    use std::time::Duration;

    /// An agent of the tests.
    fn agent(name: &str) -> AgentId {
        AgentId::parse(name).expect("a legal agent")
    }

    /// A job of the tests.
    fn job(name: &str) -> JobId {
        JobId::parse(name).expect("a legal job")
    }

    #[test]
    fn the_five_classes_of_the_design_book_are_here() {
        assert_eq!(WorkClass::ALL.len(), 5);
        let names: Vec<&str> = WorkClass::ALL.iter().map(|class| class.as_str()).collect();
        assert_eq!(
            names,
            [
                "interactive_candidate",
                "preview_read",
                "prepare",
                "verify",
                "commit"
            ]
        );
    }

    #[test]
    fn the_priority_of_the_classes_follows_the_order_of_the_design_book() {
        let priorities: Vec<i32> = WorkClass::ALL.iter().map(|class| class.base_priority()).collect();
        assert_eq!(priorities, [0, 1, 2, 3, 4]);
        // the commit is the last one and the candidate of the keyboard is the first one
        assert!(WorkClass::InteractiveCandidate < WorkClass::Commit);
    }

    #[test]
    fn a_scheduler_has_the_five_classes_and_nothing_else() {
        let scheduler = Scheduler::new();
        assert_eq!(scheduler.classes().len(), 5);
        assert!(scheduler.is_empty());
        assert_eq!(scheduler.len(), 0);
        assert_eq!(scheduler.aging(), Aging::default());
    }

    #[test]
    fn a_task_costs_one_credit_by_default() {
        let task = Task::new(
            job("job_001"),
            agent("agent_01"),
            WorkClass::Prepare,
            Timestamp::from_millis(10),
        );
        assert_eq!(task.cost, 1);
        assert_eq!(task.enqueued_at, Timestamp::from_millis(10));
        assert_eq!(task.clone().with_cost(9).cost, 9);
        assert_eq!(task.with_cost(0).cost, 1);
    }

    #[test]
    fn an_agent_without_a_weight_is_refused() {
        let mut scheduler = Scheduler::new();
        let error = scheduler
            .submit(Task::new(
                job("job_001"),
                agent("agent_01"),
                WorkClass::Prepare,
                Timestamp::zero(),
            ))
            .expect_err("the agent is not registered");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("agent_01"));
    }

    #[test]
    fn a_weight_of_zero_is_refused() {
        let mut scheduler = Scheduler::new();
        let error = scheduler
            .register_agent(agent("agent_01"), 0)
            .expect_err("a weight of zero is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_weight_of_an_agent_is_kept() {
        let mut scheduler = Scheduler::new();
        scheduler.register_agent(agent("agent_01"), 5).unwrap();
        assert_eq!(scheduler.weight_of(&agent("agent_01")), Some(5));
        assert_eq!(scheduler.weight_of(&agent("agent_02")), None);
    }

    #[test]
    fn the_share_of_the_agents_follows_their_weights() {
        let mut scheduler = Scheduler::new();
        let heavy = agent("agent_heavy");
        let light = agent("agent_light");
        scheduler.register_agent(heavy.clone(), 3).unwrap();
        scheduler.register_agent(light.clone(), 1).unwrap();

        let now = Timestamp::zero();
        for index in 0..10_000u32 {
            scheduler
                .submit(Task::new(
                    job(&format!("job_heavy_{index}")),
                    heavy.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
            scheduler
                .submit(Task::new(
                    job(&format!("job_light_{index}")),
                    light.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
        }
        assert_eq!(scheduler.len(), 20_000);

        // The share is a property of the window in which every queue stays full: once a queue is empty, the
        // others take the rest. The window is the one where both agents still have work to do.
        let mut heavy_count = 0usize;
        let mut light_count = 0usize;
        while scheduler
            .class_queue(WorkClass::Prepare)
            .map(|queue| queue.len_of_agent(&heavy) > 0 && queue.len_of_agent(&light) > 0)
            .unwrap_or(false)
        {
            let task = scheduler.next_task(now).expect("a task runs");
            if task.agent_id == heavy {
                heavy_count += 1;
            } else {
                light_count += 1;
            }
        }
        let total = heavy_count + light_count;
        assert!(total > 0);

        // The ideal share of the heavy agent is 3/4 of the tasks. The deviation of a deficit round robin
        // over a window is bounded by one round of the weights, so 1% is a wide margin.
        let ideal = total * 3 / 4;
        let deviation = heavy_count as i64 - ideal as i64;
        assert!(
            deviation.abs() <= (total as i64) / 100,
            "the share of the heavy agent is off by {deviation} over {total} tasks"
        );
        let ideal_light = total / 4;
        let deviation_light = light_count as i64 - ideal_light as i64;
        assert!(
            deviation_light.abs() <= (total as i64) / 100,
            "the share of the light agent is off by {deviation_light} over {total} tasks"
        );
    }

    #[test]
    fn no_agent_is_starved_by_a_heavier_one() {
        let mut scheduler = Scheduler::new();
        let heavy = agent("agent_heavy");
        let light = agent("agent_light");
        scheduler.register_agent(heavy.clone(), 1_000).unwrap();
        scheduler.register_agent(light.clone(), 1).unwrap();

        let now = Timestamp::zero();
        for index in 0..2_000u32 {
            scheduler
                .submit(Task::new(
                    job(&format!("job_heavy_{index}")),
                    heavy.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
        }
        for index in 0..2u32 {
            scheduler
                .submit(Task::new(
                    job(&format!("job_light_{index}")),
                    light.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
        }

        // The light agent waits for at most the sum of the weights: one round of the round robin.
        let bound = 1_000 + 1;
        let mut light_served = 0usize;
        let mut dispatched = 0usize;
        while dispatched < bound {
            let Some(task) = scheduler.next_task(now) else {
                break;
            };
            dispatched += 1;
            if task.agent_id == light {
                light_served += 1;
                break;
            }
        }
        assert_eq!(light_served, 1, "the light agent waits at most one round");
        assert!(dispatched <= bound, "the light agent waited {dispatched} dispatches");
    }

    #[test]
    fn the_tasks_of_an_agent_keep_their_order() {
        let mut scheduler = Scheduler::new();
        let first = agent("agent_01");
        let second = agent("agent_02");
        scheduler.register_agent(first.clone(), 1).unwrap();
        scheduler.register_agent(second.clone(), 1).unwrap();

        let now = Timestamp::zero();
        for index in 0..5u32 {
            scheduler
                .submit(Task::new(
                    job(&format!("job_a_{index}")),
                    first.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
            scheduler
                .submit(Task::new(
                    job(&format!("job_b_{index}")),
                    second.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
        }

        let mut order_first = Vec::new();
        let mut order_second = Vec::new();
        while let Some(task) = scheduler.next_task(now) {
            if task.agent_id == first {
                order_first.push(task.job_id.as_str().to_string());
            } else {
                order_second.push(task.job_id.as_str().to_string());
            }
        }
        assert_eq!(
            order_first,
            ["job_a_0", "job_a_1", "job_a_2", "job_a_3", "job_a_4"]
        );
        assert_eq!(
            order_second,
            ["job_b_0", "job_b_1", "job_b_2", "job_b_3", "job_b_4"]
        );
    }

    #[test]
    fn an_expensive_task_is_not_lost_behind_the_cheap_ones() {
        let mut scheduler = Scheduler::new();
        let big = agent("agent_big");
        let small = agent("agent_small");
        scheduler.register_agent(big.clone(), 1).unwrap();
        scheduler.register_agent(small.clone(), 1).unwrap();

        let now = Timestamp::zero();
        // The expensive task costs ten credits: the credit of its agent accumulates over the visits.
        scheduler
            .submit(
                Task::new(job("job_big"), big.clone(), WorkClass::Prepare, now).with_cost(10),
            )
            .unwrap();
        for index in 0..20u32 {
            scheduler
                .submit(Task::new(
                    job(&format!("job_small_{index}")),
                    small.clone(),
                    WorkClass::Prepare,
                    now,
                ))
                .unwrap();
        }

        let mut big_ran = false;
        let mut dispatched = 0usize;
        while let Some(task) = scheduler.next_task(now) {
            dispatched += 1;
            if task.agent_id == big {
                big_ran = true;
                break;
            }
        }
        assert!(big_ran, "the expensive task must run");
        assert!(
            dispatched <= 12,
            "the expensive task waited {dispatched} dispatches"
        );
    }

    #[test]
    fn the_class_of_the_keyboard_comes_first() {
        let mut scheduler = Scheduler::new();
        let who = agent("agent_01");
        scheduler.register_agent(who.clone(), 1).unwrap();

        let now = Timestamp::zero();
        scheduler
            .submit(Task::new(job("job_commit"), who.clone(), WorkClass::Commit, now))
            .unwrap();
        scheduler
            .submit(Task::new(
                job("job_prepare"),
                who.clone(),
                WorkClass::Prepare,
                now,
            ))
            .unwrap();
        scheduler
            .submit(Task::new(
                job("job_candidate"),
                who.clone(),
                WorkClass::InteractiveCandidate,
                now,
            ))
            .unwrap();
        scheduler
            .submit(Task::new(
                job("job_preview"),
                who.clone(),
                WorkClass::PreviewRead,
                now,
            ))
            .unwrap();
        scheduler
            .submit(Task::new(job("job_verify"), who.clone(), WorkClass::Verify, now))
            .unwrap();

        let mut order = Vec::new();
        while let Some(task) = scheduler.next_task(now) {
            order.push(task.job_id.as_str().to_string());
        }
        assert_eq!(
            order,
            [
                "job_candidate",
                "job_preview",
                "job_prepare",
                "job_verify",
                "job_commit"
            ]
        );
    }

    #[test]
    fn a_class_that_waits_long_enough_is_promoted() {
        let mut scheduler = Scheduler::new().with_aging(Aging {
            interval: Duration::from_secs(1),
            max_boost: 8,
        });
        let who = agent("agent_01");
        scheduler.register_agent(who.clone(), 1).unwrap();

        let start = Timestamp::from_millis(0);
        // The commit waits while the cheap reads keep coming.
        scheduler
            .submit(Task::new(job("job_commit"), who.clone(), WorkClass::Commit, start))
            .unwrap();

        let mut clock = LogicalClock::new(start);
        for round in 0..12u32 {
            clock.advance(Duration::from_secs(1));
            let now = clock.now();
            scheduler
                .submit(Task::new(
                    job(&format!("job_preview_{round}")),
                    who.clone(),
                    WorkClass::PreviewRead,
                    now,
                ))
                .unwrap();
            let task = scheduler.next_task(now).expect("a task runs");
            // The commit has waited `round + 1` seconds and the fresh read has waited nothing: the commit
            // wins as soon as its age is worth more steps than the distance between the two priorities.
            if round < 3 {
                assert_eq!(
                    task.class,
                    WorkClass::PreviewRead,
                    "the fresh read wins while the commit is young"
                );
            } else {
                assert_eq!(
                    task.class,
                    WorkClass::Commit,
                    "the commit is promoted once it has waited long enough"
                );
                break;
            }
        }
    }

    #[test]
    fn the_quantum_is_the_unit_of_the_credit() {
        let mut scheduler = Scheduler::new().with_quantum(4);
        let who = agent("agent_01");
        scheduler.register_agent(who.clone(), 1).unwrap();
        let now = Timestamp::zero();
        scheduler
            .submit(
                Task::new(job("job_01"), who.clone(), WorkClass::Prepare, now).with_cost(3),
            )
            .unwrap();
        // One visit grants 1 * 4 credits: the task of three credits is paid at once.
        let task = scheduler.next_task(now).expect("the task runs");
        assert_eq!(task.job_id.as_str(), "job_01");
    }

    #[test]
    fn the_queue_of_a_class_reports_what_it_holds() {
        let mut queue = ClassQueue::new(WorkClass::Prepare);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.oldest(), None);
        assert_eq!(queue.class(), WorkClass::Prepare);

        let who = agent("agent_01");
        let now = Timestamp::from_millis(100);
        queue.push(
            Task::new(job("job_01"), who.clone(), WorkClass::Prepare, now),
            2,
        );
        queue.push(
            Task::new(job("job_02"), who.clone(), WorkClass::Prepare, now),
            2,
        );
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.len_of_agent(&who), 2);
        assert_eq!(queue.len_of_agent(&agent("agent_02")), 0);
        assert_eq!(queue.oldest(), Some(now));
        assert!(queue.next_task().is_some());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn a_scheduler_without_a_task_returns_nothing() {
        let mut scheduler = Scheduler::new();
        assert_eq!(scheduler.next_task(Timestamp::zero()), None);
        assert!(scheduler.is_empty());
    }
}
