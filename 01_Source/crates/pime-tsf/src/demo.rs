//! The demonstration of the laser interface: ten agents write into the document, one after the other.
//!
//! The user asked for it: the panel of the light has a row of its own, "多 Agent 演示", and when that row
//! is clicked ten agents each write twenty characters into the document the user is typing into. Every
//! agent writes in **its own domain** of a .NEMA document (the rules live in [`pime_core::nema`]), and
//! every agent has a mouth of light of its own in the middle of the screen, so that the beams fly across
//! the screen instead of along its border.
//!
//! Nothing here talks to Windows: the plan of the demonstration, the places of the mouths and the run
//! itself are pure, so that they can be tested without a screen and without a document. The text service
//! (in `tip`) is the only place that talks to the host.

use crate::layout::Rect;
use crate::ui::Anchor;
use pime_core::nema;

/// How many agents take part in the demonstration.
pub const AGENT_COUNT: usize = 10;

/// How many characters every agent writes.
pub const CHARS_PER_AGENT: usize = 20;

/// The note that is written in front of the domains when the text has no header of its own.
///
/// It is the address of the .NEMA interpreter, where the syntax rules live. It is a note for the reader,
/// not a header: it does not begin with a tab, so it opens no domain of its own.
pub const NOTE: &str = nema::FALLBACK_URL;

/// The beginning of the key of the agents of the demonstration ("agent:demo:01" ...).
pub const KEY_PREFIX: &str = "agent:demo:";

/// The revision of the domains of the demonstration: the first one.
pub const REVISION: u32 = 1;

/// How many columns the mouths of the demonstration are spread over.
pub const COLUMNS: usize = 5;

/// How many rows the mouths of the demonstration are spread over.
pub const ROWS: usize = 2;

/// How far the mouths of the demonstration stay away from the border of the screen.
pub const SCREEN_MARGIN: i32 = 48;

/// The size of the screen the demonstration falls back on when the interface does not know it.
pub const DEFAULT_SCREEN: (i32, i32) = (1920, 1040);

/// The one-line hint of the panel when the field is a password field.
pub const STATUS_SECURE: &str = "密码框：一个字符也没有写入";

/// The one-line hint of the panel when the host refused the write.
pub const STATUS_REFUSED: &str = "写入被宿主拒绝";

/// One agent of the demonstration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoAgent {
    /// The identity of the agent ("agent:demo:01"): the key of its mouth and of its colour.
    pub key: String,
    /// The name that is shown to the user ("demo-01").
    pub label: String,
    /// The id of the domain of the agent ("A01").
    pub domain_id: String,
    /// The revision of the domain of the agent.
    pub revision: u32,
    /// The twenty characters the agent writes inside its own domain.
    pub body: String,
}

impl DemoAgent {
    /// The header of the domain of the agent ("\tA01 1").
    ///
    /// A real tab, the id of the domain, one space and the revision: the shape of the rules of .NEMA.
    pub fn header(&self) -> String {
        format!("\t{} {}", self.domain_id, self.revision)
    }
}

/// The ten agents of the demonstration.
pub fn agents() -> Vec<DemoAgent> {
    (1..=AGENT_COUNT)
        .map(|number| {
            let body = format!("agent{number:02}_runs_nema_ok");
            debug_assert_eq!(
                body.chars().count(),
                CHARS_PER_AGENT,
                "the text of an agent is exactly {CHARS_PER_AGENT} characters"
            );
            DemoAgent {
                key: format!("{KEY_PREFIX}{number:02}"),
                label: format!("demo-{number:02}"),
                domain_id: format!("A{number:02}"),
                revision: REVISION,
                body,
            }
        })
        .collect()
}

/// The block of one agent: the header of its domain and the text that is written inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoBlock {
    /// The header of the domain, with the line break that ends it.
    pub header: String,
    /// The twenty characters of the agent, with the line break that ends them.
    pub body: String,
}

/// What the demonstration writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoPlan {
    /// Whether the note is written in front of the domains (the text has no header of its own).
    pub add_note: bool,
    /// The note, with the line break that ends it.
    pub note: String,
    /// The ten agents, with the revision of their domain in this text.
    pub agents: Vec<DemoAgent>,
    /// The block of every agent, in the order the agents write.
    pub blocks: Vec<DemoBlock>,
}

impl DemoPlan {
    /// The whole text of the demonstration, as it is written into the document.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if self.add_note {
            text.push_str(&self.note);
        }
        for block in &self.blocks {
            text.push_str(&block.header);
            text.push_str(&block.body);
        }
        text
    }
}

/// The revision of the domain of an agent in a text: one more than the highest revision that is already
/// there for that domain, or one when the text does not hold that domain yet.
///
/// The same domain written twice in one document must not carry the same revision twice: the revision is
/// what tells the two writings apart. The text that is looked at is the piece in front of the caret, so a
/// domain that lies further up in a long document is not seen (the revision then starts at one again).
pub fn next_revision(text: &str, domain_id: &str) -> u32 {
    nema::domains(text)
        .iter()
        .filter(|domain| domain.id == domain_id)
        .map(|domain| domain.revision)
        .max()
        .map_or(1, |highest| highest.saturating_add(1))
}

/// The plan of the demonstration for a text that is already in the document.
///
/// The note (the address of the interpreter of the rules) is written only when the text has no domain
/// header of its own: when it has one, the agents simply write in their own domains.
pub fn plan(text: &str) -> DemoPlan {
    // every agent writes into its own domain; a domain that is already there is written again as the next
    // revision of it, never as a second domain with the same revision
    let agents: Vec<DemoAgent> = agents()
        .into_iter()
        .map(|mut agent| {
            agent.revision = next_revision(text, &agent.domain_id);
            agent
        })
        .collect();
    let blocks = agents
        .iter()
        .map(|agent| DemoBlock {
            header: format!("{}\n", agent.header()),
            body: format!("{}\n", agent.body),
        })
        .collect();
    DemoPlan {
        add_note: !nema::has_domain_header(text),
        note: format!("{NOTE}\n"),
        agents,
        blocks,
    }
}

/// One mouth of the demonstration: the point of the screen the light of an agent comes out of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoMouth {
    /// The identity of the agent the mouth belongs to.
    pub key: String,
    /// The name of the agent.
    pub label: String,
    /// The x of the mouth on the screen.
    pub x: i32,
    /// The y of the mouth on the screen.
    pub y: i32,
}

/// The mouths of the agents of the demonstration: ten points in the middle of the screen.
///
/// The mouths of the agents sit on the edges of the screen; the demonstration puts them in the middle of
/// it, so that the beams fly across the screen. The seed decides where they are: the same seed always
/// gives the same picture, and the seed comes from the caller (the number of the run, for instance), never
/// from the clock of the machine.
pub fn mouths(work: Rect, seed: u64) -> Vec<DemoMouth> {
    // The interface knows the screen it is on; when it does not, a screen of the usual size keeps the
    // mouths apart instead of piling them up on one spot.
    let work = if work.width() > 0 && work.height() > 0 {
        work
    } else {
        Rect::new(0, 0, DEFAULT_SCREEN.0, DEFAULT_SCREEN.1)
    };
    let area = inner_area(work);
    let cell_width = (area.width() / COLUMNS as i32).max(1);
    let cell_height = (area.height() / ROWS as i32).max(1);
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(AGENT_COUNT);
    for (index, agent) in agents().iter().enumerate() {
        // one mouth per cell of the grid: they never lie on the same spot
        let column = (index % COLUMNS) as i32;
        let row = (index / COLUMNS) as i32;
        let left = area.left + column * cell_width;
        let top = area.top + row * cell_height;
        out.push(DemoMouth {
            key: agent.key.clone(),
            label: agent.label.clone(),
            x: left + rng.below(cell_width as u32) as i32,
            y: top + rng.below(cell_height as u32) as i32,
        });
    }
    out
}

/// The part of the work area the mouths of the demonstration are put in.
fn inner_area(work: Rect) -> Rect {
    let margin = SCREEN_MARGIN.min(work.width() / 4).min(work.height() / 4);
    Rect::new(
        work.left + margin,
        work.top + margin,
        (work.right - margin).max(work.left + margin),
        (work.bottom - margin).max(work.top + margin),
    )
}

/// A small pseudo random generator (xorshift64*).
///
/// The demonstration must be repeatable: the same seed gives the same mouths, so that a picture of it can
/// be reproduced. The seed comes from the caller; the clock of the machine is never read here.
struct Rng(u64);

impl Rng {
    /// A generator for the seed. A state of zero would leave the generator still for ever.
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    /// The next number of the sequence.
    fn next(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below the bound (nothing when the bound is zero).
    fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        (self.next() % u64::from(bound)) as u32
    }
}

/// Why the demonstration did not write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DemoError {
    /// The field is a password field: not one character is written into it.
    SecureField,
    /// The host refused the write.
    Refused(String),
}

impl std::fmt::Display for DemoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DemoError::SecureField => write!(formatter, "the field is a password field"),
            DemoError::Refused(reason) => write!(formatter, "the host refused the write: {reason}"),
        }
    }
}

/// What one agent of the demonstration did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoAgentReport {
    /// The identity of the agent.
    pub key: String,
    /// The name of the agent.
    pub label: String,
    /// The id of the domain the agent wrote in.
    pub domain_id: String,
    /// The revision of the domain the agent wrote (one more than the one that was there before).
    pub revision: u32,
    /// The twenty characters the agent wrote.
    pub text: String,
    /// Where the mouth of the agent was on the screen.
    pub mouth: (i32, i32),
    /// Whether the beam was fired (there is no beam without a caret to fire it at).
    pub beam: bool,
}

/// What the demonstration did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoReport {
    /// The reason the demonstration stopped (nothing when it ran to the end).
    pub error: Option<DemoError>,
    /// Whether the note was written in front of the domains.
    pub note: bool,
    /// What every agent wrote, in the order they wrote.
    pub agents: Vec<DemoAgentReport>,
}

impl DemoReport {
    /// The one-line hint the panel shows after the demonstration.
    pub fn status(&self) -> String {
        match &self.error {
            Some(DemoError::SecureField) => STATUS_SECURE.to_string(),
            Some(DemoError::Refused(_)) => STATUS_REFUSED.to_string(),
            None => format!("{} 个 Agent 已写入各自的领域", self.agents.len()),
        }
    }
}

/// What the demonstration needs from the document it writes into.
pub trait DemoHost {
    /// Whether the field is a password field. Nothing is written into one: not one character.
    fn is_secure_field(&mut self) -> bool;
    /// The text that is already in the document at the caret (the piece the demonstration looks at).
    fn text_at_caret(&mut self) -> Option<String>;
    /// The caret in screen coordinates: the beams fly to it. `None` when the host cannot say.
    fn caret_anchor(&mut self) -> Option<Anchor>;
    /// Write the text at the caret and put the caret after it.
    fn write(&mut self, text: &str) -> Result<(), DemoError>;
    /// The light of an agent: the beam from its mouth to the character it wrote.
    fn fire(&mut self, key: &str, label: &str, anchor: Anchor, mouth: (i32, i32));
}

/// Run the demonstration: the note (when it is needed), then every agent writes its own domain.
///
/// The agents write one after the other and the caret follows them: the text of an agent is inside its own
/// domain, and the domain of the next agent begins where the last one ends. A password field gets
/// nothing at all.
pub fn run<H: DemoHost>(host: &mut H, seed: u64, work: Rect) -> DemoReport {
    let mut report = DemoReport {
        error: None,
        note: false,
        agents: Vec::new(),
    };
    // A password field gets nothing: not one character, and no pretending that something was written.
    if host.is_secure_field() {
        report.error = Some(DemoError::SecureField);
        return report;
    }
    // The text that is already there decides whether the note is needed.
    let current = host.text_at_caret().unwrap_or_default();
    let plan = plan(&current);
    if plan.add_note {
        if let Err(error) = host.write(&plan.note) {
            report.error = Some(error);
            return report;
        }
        report.note = true;
    }
    // Every agent writes its own domain: the header first, then the twenty characters inside it.
    let mouths = mouths(work, seed);
    for (index, block) in plan.blocks.iter().enumerate() {
        let agent = &plan.agents[index];
        let mouth = &mouths[index];
        // The anchor is the caret before the write: that is where the characters appear.
        let anchor = host.caret_anchor();
        if let Err(error) = host.write(&block.header) {
            report.error = Some(error);
            return report;
        }
        if let Err(error) = host.write(&block.body) {
            report.error = Some(error);
            return report;
        }
        // The light of the agent: the beam comes out of its mouth and has its colour.
        let beam = if let Some(anchor) = anchor {
            host.fire(&mouth.key, &mouth.label, anchor, (mouth.x, mouth.y));
            true
        } else {
            false
        };
        report.agents.push(DemoAgentReport {
            key: agent.key.clone(),
            label: agent.label.clone(),
            domain_id: agent.domain_id.clone(),
            revision: agent.revision,
            text: agent.body.clone(),
            mouth: (mouth.x, mouth.y),
            beam,
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The work area of a monitor.
    fn work() -> Rect {
        Rect::new(0, 0, 1920, 1040)
    }

    /// The demonstration has ten agents.
    #[test]
    fn the_demonstration_has_ten_agents() {
        let agents = agents();
        assert_eq!(agents.len(), AGENT_COUNT);
        assert_eq!(agents.len(), 10);
    }

    /// Every agent writes exactly twenty characters.
    #[test]
    fn every_agent_writes_twenty_characters() {
        for agent in agents() {
            assert_eq!(
                agent.body.chars().count(),
                CHARS_PER_AGENT,
                "the text of {} is not {CHARS_PER_AGENT} characters",
                agent.label
            );
        }
    }

    /// The identity and the name of the agents are the ones the user asked for.
    #[test]
    fn the_agents_are_the_ones_the_user_asked_for() {
        let agents = agents();
        assert_eq!(agents[0].key, "agent:demo:01");
        assert_eq!(agents[0].label, "demo-01");
        assert_eq!(agents[9].key, "agent:demo:10");
        assert_eq!(agents[9].label, "demo-10");
    }

    /// The header of the domain of an agent is a real tab, the id, a space and the revision.
    #[test]
    fn the_header_of_a_domain_is_a_tab_an_id_a_space_and_a_revision() {
        for agent in agents() {
            let header = agent.header();
            assert!(header.starts_with('\t'), "the header opens with a tab: {header:?}");
            assert_eq!(header, format!("\t{} 1", agent.domain_id));
            assert!(nema::is_domain_header(&header), "{header:?} is not a header");
        }
        assert_eq!(agents()[0].header(), "\tA01 1");
    }

    /// A text that has no domain header of its own gets the note in front of the domains.
    #[test]
    fn a_text_without_a_header_gets_the_note() {
        let plan = plan("What the user wrote.\n");
        assert!(plan.add_note, "the note is needed");
        assert_eq!(plan.note, format!("{NOTE}\n"));
        assert!(plan.text().starts_with(&plan.note), "the note comes first");
    }

    /// A text that has a domain header of its own gets no note.
    #[test]
    fn a_text_with_a_header_gets_no_note() {
        let plan = plan("\tTtext1 1\nWhat the user wrote.\n");
        assert!(!plan.add_note, "the note is not needed");
        assert!(!plan.text().contains(NOTE), "the note is not written");
    }

    /// The note is not a header: it does not begin with a tab, and it opens no domain.
    #[test]
    fn the_note_is_not_a_domain_header() {
        assert!(!nema::is_domain_header(NOTE));
        assert!(!nema::has_domain_header(&format!("{NOTE}\n")));
    }

    /// The text of every agent is inside the domain of that agent, and the domains follow one another
    /// without an empty line between them.
    #[test]
    fn every_agent_writes_inside_its_own_domain() {
        let text = plan("").text();
        let domains = nema::domains(&text);
        assert_eq!(domains.len(), AGENT_COUNT);
        for (index, agent) in agents().iter().enumerate() {
            let domain = &domains[index];
            assert_eq!(domain.id, agent.domain_id, "the domain of {}", agent.label);
            // the header of the agent opens the domain ...
            assert_eq!(domain.start, text.find(&agent.header()).unwrap() + agent.header().len() + 1);
            // ... and the text of the agent is inside it
            let start = text.find(&agent.body).expect("the text of the agent");
            let found = nema::domain_at(&text, start).expect("the text is in a domain");
            assert_eq!(found.id, agent.domain_id, "the text of {} is not in its domain", agent.label);
            assert!(found.contains(start + agent.body.len() - 1));
        }
        // and there is no empty line between two domains
        assert!(!text.contains("\n\n"), "the domains follow one another without an empty line");
    }

    /// The first run writes the revision one: the domains of the agents do not exist yet.
    #[test]
    fn the_first_run_writes_the_revision_one() {
        let plan = plan("");
        for agent in &plan.agents {
            assert_eq!(agent.revision, 1, "the domain of {} is new", agent.label);
        }
        assert!(plan.agents[0].header().starts_with("\tA01 1"));
    }

    /// A domain that is already in the text is written again as the next revision of it.
    #[test]
    fn a_domain_that_is_already_there_goes_up_one_revision() {
        let text = "\tA01 1\nagent01_runs_nema_ok\n";
        let plan = plan(text);
        assert_eq!(plan.agents[0].revision, 2, "the domain of agent 01 is written again");
        assert_eq!(plan.agents[0].header(), "\tA01 2");
        // the other agents are new: their revision stays one
        assert_eq!(plan.agents[1].revision, 1);
        assert_eq!(plan.agents[1].header(), "\tA02 1");
    }

    /// Two runs in a row never write the same domain with the same revision: the revision is what tells the
    /// two writings apart, so a document that holds both has no ambiguous domain in it.
    #[test]
    fn two_runs_never_write_the_same_revision_twice() {
        let first = plan("");
        let text = first.text();
        let second = plan(&text);
        assert_eq!(second.agents[0].revision, 2);
        let both = format!("{text}{}", second.text());
        // every domain of the document has its own revision
        let domains = nema::domains(&both);
        assert_eq!(domains.len(), AGENT_COUNT * 2);
        for (index, domain) in domains.iter().enumerate() {
            let expected = if index < AGENT_COUNT { 1 } else { 2 };
            assert_eq!(domain.revision, expected, "the revision of {} is wrong", domain.id);
        }
        // and the same domain id never carries the same revision twice
        for (index, domain) in domains.iter().enumerate() {
            for other in domains.iter().skip(index + 1) {
                assert!(
                    domain.id != other.id || domain.revision != other.revision,
                    "the domain {} carries the revision {} twice",
                    domain.id,
                    domain.revision
                );
            }
        }
    }

    /// A domain that is already at a high revision goes up from there.
    #[test]
    fn the_revision_goes_up_from_the_highest_one_that_is_there() {
        let text = "\tA01 4\nsomething\n";
        assert_eq!(next_revision(text, "A01"), 5);
        assert_eq!(next_revision(text, "A02"), 1, "a domain that is not there starts at one");
    }

    /// The mouths of the demonstration are in the middle of the screen, not on its border, and they never
    /// lie on the same spot.
    #[test]
    fn the_mouths_are_in_the_middle_of_the_screen() {
        let mouths = mouths(work(), 7);
        assert_eq!(mouths.len(), AGENT_COUNT);
        for mouth in &mouths {
            assert!(mouth.x > 0 && mouth.x < 1920, "the mouth is off the screen: {mouth:?}");
            assert!(mouth.y > 0 && mouth.y < 1040, "the mouth is off the screen: {mouth:?}");
        }
        for (index, mouth) in mouths.iter().enumerate() {
            for (other_index, other) in mouths.iter().enumerate() {
                if index >= other_index {
                    continue;
                }
                assert!(
                    mouth.x != other.x || mouth.y != other.y,
                    "the mouths {index} and {other_index} lie on the same spot"
                );
            }
        }
    }

    /// The same seed gives the same mouths: the demonstration can be repeated.
    #[test]
    fn the_same_seed_gives_the_same_mouths() {
        let first = mouths(work(), 3);
        let again = mouths(work(), 3);
        assert_eq!(first, again);
        let other = mouths(work(), 4);
        assert_ne!(first, other, "another seed gives another picture");
    }

    /// A screen that is not known does not pile the mouths up on one spot.
    #[test]
    fn an_unknown_screen_keeps_the_mouths_apart() {
        let mouths = mouths(Rect::new(0, 0, 0, 0), 5);
        assert_eq!(mouths.len(), AGENT_COUNT);
        for (index, mouth) in mouths.iter().enumerate() {
            for (other_index, other) in mouths.iter().enumerate() {
                if index >= other_index {
                    continue;
                }
                assert!(
                    mouth.x != other.x || mouth.y != other.y,
                    "the mouths {index} and {other_index} lie on the same spot"
                );
            }
        }
    }

    /// A host that writes nothing: the double of the tests.
    struct FakeHost {
        /// Whether the field is a password field.
        secure: bool,
        /// The text that is already in the document.
        text: String,
        /// Everything that was written, in order.
        written: Vec<String>,
        /// Whether the host refuses the writes.
        refuse: bool,
        /// The beams that were fired.
        beams: Vec<(String, i32, i32)>,
        /// Whether the host knows where the caret is.
        has_caret: bool,
    }

    impl FakeHost {
        fn new() -> FakeHost {
            FakeHost {
                secure: false,
                text: String::new(),
                written: Vec::new(),
                refuse: false,
                beams: Vec::new(),
                has_caret: true,
            }
        }
    }

    impl DemoHost for FakeHost {
        fn is_secure_field(&mut self) -> bool {
            self.secure
        }

        fn text_at_caret(&mut self) -> Option<String> {
            Some(self.text.clone())
        }

        fn caret_anchor(&mut self) -> Option<Anchor> {
            if self.has_caret {
                Some(Anchor {
                    x: 100,
                    y: 200,
                    line_height: 20,
                })
            } else {
                None
            }
        }

        fn write(&mut self, text: &str) -> Result<(), DemoError> {
            if self.refuse {
                return Err(DemoError::Refused("the host said no (test)".to_string()));
            }
            self.written.push(text.to_string());
            Ok(())
        }

        fn fire(&mut self, key: &str, _label: &str, _anchor: Anchor, mouth: (i32, i32)) {
            self.beams.push((key.to_string(), mouth.0, mouth.1));
        }
    }

    /// A password field gets nothing: not one character, and no pretending that something was written.
    #[test]
    fn a_password_field_is_not_written_into() {
        let mut host = FakeHost::new();
        host.secure = true;
        let report = run(&mut host, 1, work());
        assert_eq!(report.error, Some(DemoError::SecureField));
        assert!(host.written.is_empty(), "nothing was written: {:?}", host.written);
        assert!(host.beams.is_empty(), "no light was fired");
        assert!(report.agents.is_empty());
        assert_eq!(report.status(), STATUS_SECURE);
    }

    /// The demonstration writes the note and the ten domains, and fires one beam per agent.
    #[test]
    fn the_demonstration_writes_the_note_and_the_ten_domains() {
        let mut host = FakeHost::new();
        let report = run(&mut host, 1, work());
        assert_eq!(report.error, None);
        assert!(report.note, "the note was written");
        assert_eq!(report.agents.len(), AGENT_COUNT);
        assert_eq!(host.beams.len(), AGENT_COUNT);
        // the note comes first, then the header and the text of every agent
        assert_eq!(host.written[0], format!("{NOTE}\n"));
        assert_eq!(host.written[1], "\tA01 1\n");
        assert_eq!(host.written[2], "agent01_runs_nema_ok\n");
        assert_eq!(host.written.len(), 1 + AGENT_COUNT * 2);
        // every agent writes its own text
        for (index, agent) in agents().iter().enumerate() {
            assert_eq!(report.agents[index].text, agent.body);
            assert_eq!(report.agents[index].key, agent.key);
            assert!(report.agents[index].beam);
        }
    }

    /// A text that has a domain header of its own gets no note.
    #[test]
    fn a_text_with_a_header_is_not_given_a_note() {
        let mut host = FakeHost::new();
        host.text = "\tTtext1 1\n".to_string();
        let report = run(&mut host, 1, work());
        assert!(!report.note, "the note is not needed");
        assert_eq!(host.written[0], "\tA01 1\n");
    }

    /// A host that refuses the write stops the demonstration, and says so.
    #[test]
    fn a_refused_write_stops_the_demonstration() {
        let mut host = FakeHost::new();
        host.refuse = true;
        let report = run(&mut host, 1, work());
        assert!(matches!(report.error, Some(DemoError::Refused(_))));
        assert!(host.written.is_empty());
        assert_eq!(report.status(), STATUS_REFUSED);
    }

    /// A host that cannot say where the caret is still writes the text; only the light is missing.
    #[test]
    fn a_host_without_a_caret_still_writes_the_text() {
        let mut host = FakeHost::new();
        host.has_caret = false;
        let report = run(&mut host, 1, work());
        assert_eq!(report.error, None);
        assert_eq!(host.written.len(), 1 + AGENT_COUNT * 2);
        assert!(host.beams.is_empty(), "there is no beam without a caret");
        assert!(report.agents.iter().all(|agent| !agent.beam));
    }
}
