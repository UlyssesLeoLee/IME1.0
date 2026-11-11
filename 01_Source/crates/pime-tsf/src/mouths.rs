//! The mouths of light: one per source of input.
//!
//! The user asked for the collaboration of several agents to be visible: "给它们开颜色区分比较明显的随机rgb激光口，
//! 不同agent或者用户本人输入的时候，散落在屏幕上方左侧右侧的激光口动态发射属于它们颜色的激光" (give them laser mouths
//! with clearly different random RGB colours; when an agent or the user writes, the mouths scattered over the top, the
//! left and the right of the screen fire a laser **in their own colour**).
//!
//! This module is the pure part of that: the **registry** of the mouths. Every source of input (the human at the
//! keyboard and every agent) gets a mouth of its own, and the registry decides three things:
//!
//! * **the colour** — the human keeps the pale blue the user asked for ("一小截淡蓝色半透明激光"), every agent gets
//!   a colour of its own that is far away from the colours that are already taken;
//! * **the place** — the human's mouth stays where the user dragged it (the setting `mouth_place`), the mouths of the
//!   agents are spread over the top, the right and the left edge of the screen;
//! * **the stability** — the same key always gets the same colour and the same place, so that the user learns to
//!   recognise "the red one on the top edge is Claude" instead of seeing a new colour every time.
//!
//! Two sub-agents of the **same** provider are two different sources: the key of a source carries the provider, the
//! name of the agent and the instance of the client process, so the key of two sub-agents is never the same.
//!
//! Nothing here talks to Windows: the geometry and the colours are pure, so they can be tested without a screen.

use crate::laser::{Edge, Mouth, MouthPlace, floating_mouth, mouth_at};
use crate::layout::Rect;
use pime_core::style::{LaserTheme, Rgb, mix};

/// The key of the human at the keyboard.
pub const USER_KEY: &str = "user";

/// The name the human is shown under.
pub const USER_LABEL: &str = "user";

/// How many places of hue the circle of the colours is cut into (every 15 degrees).
pub const HUE_SLOTS: usize = 24;

/// The hue of one place of the circle (in degrees).
pub const HUE_STEP: f64 = 360.0 / HUE_SLOTS as f64;

/// The saturation of the colour of an agent: strong enough to be seen at a glance.
pub const SATURATION_MIN: f64 = 0.80;

/// The biggest saturation of the colour of an agent.
pub const SATURATION_MAX: f64 = 0.95;

/// The smallest value (the brightness) of the colour of an agent: the light of a laser is never dark.
pub const VALUE_MIN: f64 = 0.88;

/// The biggest value (the brightness) of the colour of an agent.
pub const VALUE_MAX: f64 = 1.00;

/// The edges the mouths of the agents take, in the order they take them: the first agent is on the top edge, the
/// second one on the right edge, the third one on the left edge, the fourth one on the top edge again.
pub const AGENT_EDGES: [Edge; 3] = [Edge::Top, Edge::Right, Edge::Left];

/// How many mouths fit on one edge.
///
/// The number is fixed and not the number of the mouths that happen to be on the edge: a mouth that was put there
/// keeps its place for the whole session, so the places cannot be counted again every time a new agent shows up.
pub const MOUTHS_PER_EDGE: usize = 6;

/// The radius of the lens of a mouth.
pub const MOUTH_RADIUS: i32 = 12;

/// One source of light: a mouth on the edge of the screen that belongs to one speaker.
#[derive(Clone, Debug, PartialEq)]
pub struct MouthSource {
    /// The key of the source (the identity of the speaker).
    pub key: String,
    /// The name that is shown to the user.
    pub label: String,
    /// The colour of the slit of light.
    pub colour: Rgb,
    /// The colour of the halo around it.
    pub halo: Rgb,
    /// Where the mouth is.
    pub mouth: Mouth,
    /// Whether this is the human at the keyboard.
    pub is_user: bool,
}

/// The registry of the mouths: every source gets its own colour and its own place.
///
/// The registry is the memory of the laser interface: it knows every source that has shown up, the colour it was
/// given and where its mouth is. The same key always gets the same colour and the same place.
pub struct MouthRegistry {
    /// Every source that has a mouth, in the order they showed up.
    sources: Vec<MouthSource>,
    /// The hues that are already taken. The pale blue of the human is in it from the start, so that no agent gets a
    /// colour that looks like the colour of the human.
    used_hues: Vec<f64>,
    /// How many agents have a mouth (the number of the agent that comes next).
    agents: usize,
}

impl Default for MouthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MouthRegistry {
    /// A registry with nothing in it but the hue of the human, which is taken from the start.
    pub fn new() -> Self {
        MouthRegistry {
            sources: Vec::new(),
            // The pale blue of the human is the first colour of the screen: an agent that looks like it would be
            // mistaken for the keyboard.
            used_hues: vec![hue_of(LaserTheme::MOUTH_SLIT)],
            agents: 0,
        }
    }

    /// The mouth of a source; it is made the first time the source is seen.
    ///
    /// The same key always gives the same colour and the same place. The mouth of the **human** follows
    /// `user_place` (the place the user dragged it to), the mouths of the agents never move.
    pub fn obtain(&mut self, key: &str, label: &str, work: Rect, user_place: MouthPlace) -> MouthSource {
        if let Some(index) = self.sources.iter().position(|source| source.key == key) {
            let source = &mut self.sources[index];
            if source.is_user {
                // The human's mouth is where the human put it.
                source.mouth = mouth_at(work, user_place, MOUTH_RADIUS);
            }
            return self.sources[index].clone();
        }
        self.make(key, label, work, user_place, None)
    }

    /// The mouth of a source at a given point of the screen.
    ///
    /// The mouths of the agents sit on the edges of the screen; the demonstration puts them in the middle
    /// of it, so that the beams fly across the screen instead of along its border. The point is kept inside
    /// the work area and away from its border, so that the lens is never cut off. A source that is already
    /// known moves to the new point: the demonstration puts its agents somewhere else every time it runs.
    pub fn obtain_at(
        &mut self,
        key: &str,
        label: &str,
        work: Rect,
        user_place: MouthPlace,
        x: i32,
        y: i32,
    ) -> MouthSource {
        if key == USER_KEY {
            // The human's mouth is where the human put it: it never follows the demonstration.
            return self.obtain(key, label, work, user_place);
        }
        let mouth = floating_mouth(work, x, y, MOUTH_RADIUS);
        if let Some(index) = self.sources.iter().position(|source| source.key == key) {
            self.sources[index].mouth = mouth;
            return self.sources[index].clone();
        }
        self.make(key, label, work, user_place, Some(mouth))
    }

    /// Make the source of a key that is not in the registry yet.
    ///
    /// The colour of an agent is the one of its key, and its place is the place of the edge it takes, unless
    /// the caller gave one (the demonstration puts its agents in the middle of the screen).
    fn make(
        &mut self,
        key: &str,
        label: &str,
        work: Rect,
        user_place: MouthPlace,
        at: Option<Mouth>,
    ) -> MouthSource {
        let is_user = key == USER_KEY;
        let (colour, halo, mouth) = if is_user {
            (
                LaserTheme::MOUTH_SLIT,
                LaserTheme::MOUTH_HALO,
                mouth_at(work, user_place, MOUTH_RADIUS),
            )
        } else {
            let (colour, hue) = agent_colour(key, &self.used_hues);
            self.used_hues.push(hue);
            let halo = mix(colour, LaserTheme::PAGE, 0.35);
            let mouth = match at {
                Some(mouth) => mouth,
                None => {
                    let place = agent_place(self.agents, work);
                    self.agents += 1;
                    mouth_at(work, place, MOUTH_RADIUS)
                }
            };
            (colour, halo, mouth)
        };
        let source = MouthSource {
            key: key.to_string(),
            label: label.to_string(),
            colour,
            halo,
            mouth,
            is_user,
        };
        self.sources.push(source.clone());
        source
    }

    /// The mouth of a source, if it has one.
    pub fn get(&self, key: &str) -> Option<&MouthSource> {
        self.sources.iter().find(|source| source.key == key)
    }

    /// Every source that has a mouth.
    pub fn sources(&self) -> Vec<MouthSource> {
        self.sources.clone()
    }

    /// Forget a source (its mouth goes away).
    ///
    /// The colour it had is free again: the next agent of the same kind may take it.
    pub fn release(&mut self, key: &str) {
        let Some(index) = self.sources.iter().position(|source| source.key == key) else {
            return;
        };
        let source = self.sources.remove(index);
        if !source.is_user {
            self.agents = self.agents.saturating_sub(1);
            let hue = hue_of(source.colour);
            self.used_hues.retain(|used| *used != hue);
        }
    }

    /// Forget every source (all the mouths go away).
    pub fn clear(&mut self) {
        self.sources.clear();
        self.agents = 0;
        self.used_hues = vec![hue_of(LaserTheme::MOUTH_SLIT)];
    }
}

/// The place of the mouth of an agent.
///
/// The edges turn: the first agent is on the top edge, the second one on the right edge, the third one on the left
/// edge, the fourth one on the top edge again. The mouths of one edge are spread over it: the n-th one of an edge
/// sits at the fraction `(n+1)/(the number of places of the edge + 1)`, so that no two of them are on the same spot.
fn agent_place(index: usize, work: Rect) -> MouthPlace {
    let edge = AGENT_EDGES[index % AGENT_EDGES.len()];
    let nth = index / AGENT_EDGES.len();
    // The number of the places of the edge is fixed, so that a mouth never moves when another one shows up.
    let places = MOUTHS_PER_EDGE.max(nth + 1);
    let fraction = (nth + 1) as f64 / (places + 1) as f64;
    let offset = match edge {
        Edge::Top => work.left + (work.width() as f64 * fraction).round() as i32,
        Edge::Left | Edge::Right => work.top + (work.height() as f64 * fraction).round() as i32,
    };
    MouthPlace { edge, offset }
}

/// The colour of an agent, and the hue it was given.
///
/// The hue is the **farthest point** of the circle from every hue that is already taken (the greedy farthest point of
/// the circle): the colours of the agents are therefore as far from each other as the circle allows. When two places
/// are equally far away, the hash of the key picks one of them: it looks random, but the same key always gets the
/// same colour.
///
/// The saturation and the value come from the hash of the key as well: two agents whose hues are close still have a
/// different brightness, and the eye tells them apart.
fn agent_colour(key: &str, used: &[f64]) -> (Rgb, f64) {
    let hue = choose_hue(key, used);
    let digest = pime_core::sha256::sha256(key.as_bytes());
    let saturation = SATURATION_MIN + (digest[0] as f64 / 255.0) * (SATURATION_MAX - SATURATION_MIN);
    let value = VALUE_MIN + (digest[1] as f64 / 255.0) * (VALUE_MAX - VALUE_MIN);
    (Rgb::from_hsv(hue, saturation, value), hue)
}

/// The hue of the agent: the place of the circle that is the farthest from every hue that is taken.
fn choose_hue(key: &str, used: &[f64]) -> f64 {
    let mut best: Vec<f64> = Vec::new();
    let mut best_distance = f64::MIN;
    for slot in 0..HUE_SLOTS {
        let hue = slot as f64 * HUE_STEP;
        let distance = used
            .iter()
            .map(|taken| angle_distance(hue, *taken))
            .fold(f64::MAX, f64::min);
        if distance > best_distance + 1e-9 {
            best_distance = distance;
            best.clear();
            best.push(hue);
        } else if (distance - best_distance).abs() <= 1e-9 {
            best.push(hue);
        }
    }
    // The hash of the key picks among the places that are equally far away.
    let digest = pime_core::sha256::sha256(key.as_bytes());
    let pick = u32::from_be_bytes([digest[8], digest[9], digest[10], digest[11]]) as usize % best.len();
    best[pick]
}

/// The angle between two hues, the short way round the circle.
pub fn angle_distance(a: f64, b: f64) -> f64 {
    let difference = (a - b).abs() % 360.0;
    if difference > 180.0 {
        360.0 - difference
    } else {
        difference
    }
}

/// The hue of a colour, if it has one (a grey has none).
fn hue_of(colour: Rgb) -> f64 {
    colour.hue().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::style::LaserTheme;

    /// The work area of a monitor.
    fn work() -> Rect {
        Rect::new(0, 0, 1920, 1040)
    }

    /// The place of the human when nothing was saved: the left edge, in the middle.
    fn user_place() -> MouthPlace {
        MouthPlace::default_in(work())
    }

    /// The hue of a colour, or -1 when it has none.
    fn hue(colour: Rgb) -> f64 {
        colour.hue().unwrap_or(-1.0)
    }

    /// Two agents never get the same colour: the user asked for colours that tell them apart at a glance.
    #[test]
    fn two_agents_get_different_colours() {
        let mut registry = MouthRegistry::new();
        let first = registry.obtain("agent:a:one:1", "one", work(), user_place());
        let second = registry.obtain("agent:a:two:1", "two", work(), user_place());
        assert_ne!(first.colour, second.colour, "the colours must be different");
        assert_ne!(first.halo, second.halo, "the halos must be different");
    }

    /// The third agent is far away from the first two: the colours are spread over the circle, not clustered.
    #[test]
    fn the_third_agent_is_far_from_the_first_two() {
        let mut registry = MouthRegistry::new();
        let first = registry.obtain("agent:a:one:1", "one", work(), user_place());
        let second = registry.obtain("agent:a:two:1", "two", work(), user_place());
        let third = registry.obtain("agent:a:three:1", "three", work(), user_place());
        let third_hue = hue(third.colour);
        assert!(
            angle_distance(third_hue, hue(first.colour)) >= 45.0,
            "the third colour is too close to the first one ({} degrees)",
            angle_distance(third_hue, hue(first.colour))
        );
        assert!(
            angle_distance(third_hue, hue(second.colour)) >= 45.0,
            "the third colour is too close to the second one ({} degrees)",
            angle_distance(third_hue, hue(second.colour))
        );
    }

    /// The colour of an agent is never the pale blue of the human: the human's hue is taken from the start.
    #[test]
    fn the_colour_of_an_agent_is_far_from_the_pale_blue_of_the_human() {
        let mut registry = MouthRegistry::new();
        for index in 0..6 {
            let source = registry.obtain(
                &format!("agent:provider:agent-{index}:instance"),
                "agent",
                work(),
                user_place(),
            );
            let distance = angle_distance(hue(source.colour), hue(LaserTheme::MOUTH_SLIT));
            assert!(
                distance >= 45.0,
                "the colour of the agent {index} is too close to the blue of the human ({distance} degrees)"
            );
        }
    }

    /// The human keeps the pale blue the user asked for; the agents are the ones that get a random colour.
    #[test]
    fn the_human_keeps_the_pale_blue_and_the_agents_do_not() {
        let mut registry = MouthRegistry::new();
        let user = registry.obtain(USER_KEY, USER_LABEL, work(), user_place());
        assert!(user.is_user);
        assert_eq!(user.colour, LaserTheme::MOUTH_SLIT);
        assert_eq!(user.halo, LaserTheme::MOUTH_HALO);
        let agent = registry.obtain("agent:a:one:1", "one", work(), user_place());
        assert!(!agent.is_user);
        assert_ne!(agent.colour, LaserTheme::MOUTH_SLIT);
    }

    /// The same key always gets the same colour and the same place; another key gets another colour and another
    /// place. This is what lets the user learn "the red one is Claude".
    #[test]
    fn the_same_key_always_gets_the_same_colour_and_the_same_place() {
        let mut first = MouthRegistry::new();
        let one = first.obtain("agent:a:one:1", "one", work(), user_place());
        let again = first.obtain("agent:a:one:1", "one", work(), user_place());
        assert_eq!(one.colour, again.colour, "the colour must not change");
        assert_eq!(one.mouth, again.mouth, "the place must not change");
        let other = first.obtain("agent:a:two:1", "two", work(), user_place());
        assert_ne!(one.colour, other.colour, "another key gets another colour");
        assert_ne!(one.mouth, other.mouth, "another key gets another place");
    }

    /// The place of a mouth does not change when another agent shows up: the mouth of an agent is its own.
    #[test]
    fn the_place_of_a_mouth_does_not_move_when_another_agent_shows_up() {
        let mut registry = MouthRegistry::new();
        let one = registry.obtain("agent:a:one:1", "one", work(), user_place());
        let two = registry.obtain("agent:a:two:1", "two", work(), user_place());
        let three = registry.obtain("agent:a:three:1", "three", work(), user_place());
        let one_again = registry.obtain("agent:a:one:1", "one", work(), user_place());
        assert_eq!(one.mouth, one_again.mouth, "the first mouth stayed where it was");
        assert_eq!(two.mouth, registry.obtain("agent:a:two:1", "two", work(), user_place()).mouth);
        assert_eq!(three.mouth, registry.obtain("agent:a:three:1", "three", work(), user_place()).mouth);
    }

    /// The edges turn: top, right, left, top again.
    #[test]
    fn the_edges_of_the_agents_turn() {
        let mut registry = MouthRegistry::new();
        let one = registry.obtain("agent:a:one:1", "one", work(), user_place());
        let two = registry.obtain("agent:a:two:1", "two", work(), user_place());
        let three = registry.obtain("agent:a:three:1", "three", work(), user_place());
        let four = registry.obtain("agent:a:four:1", "four", work(), user_place());
        assert_eq!(one.mouth.edge, Edge::Top);
        assert_eq!(two.mouth.edge, Edge::Right);
        assert_eq!(three.mouth.edge, Edge::Left);
        assert_eq!(four.mouth.edge, Edge::Top);
    }

    /// Two mouths of the same edge never lie on the same spot.
    #[test]
    fn two_mouths_of_the_same_edge_do_not_overlap() {
        let mut registry = MouthRegistry::new();
        let mut mouths = Vec::new();
        for index in 0..9 {
            let source = registry.obtain(
                &format!("agent:provider:agent-{index}:instance"),
                "agent",
                work(),
                user_place(),
            );
            mouths.push(source.mouth);
        }
        for (index, mouth) in mouths.iter().enumerate() {
            for (other_index, other) in mouths.iter().enumerate() {
                if index >= other_index {
                    continue;
                }
                if mouth.edge != other.edge {
                    continue; // the mouths of two edges are never on the same spot
                }
                let distance = (mouth.x - other.x).abs() + (mouth.y - other.y).abs();
                assert!(
                    distance > 26,
                    "the mouths {index} and {other_index} are on the same spot ({distance} pixels apart)"
                );
            }
        }
    }

    /// The mouths of one edge are spread over it: they are not all in the same corner.
    #[test]
    fn the_mouths_of_one_edge_are_spread_over_it() {
        let mut registry = MouthRegistry::new();
        // nine agents: the first of every round of three is on the top edge
        for index in 0..9 {
            registry.obtain(
                &format!("agent:provider:agent-{index}:instance"),
                "agent",
                work(),
                user_place(),
            );
        }
        let sources = registry.sources();
        let offsets: Vec<i32> = [0usize, 3, 6]
            .iter()
            .map(|index| sources[*index].mouth.x)
            .collect();
        for index in [0usize, 3, 6] {
            assert_eq!(sources[index].mouth.edge, Edge::Top, "the agent {index}");
        }
        assert!(offsets[0] < offsets[1], "the mouths are not in order: {offsets:?}");
        assert!(offsets[1] < offsets[2], "the mouths are not in order: {offsets:?}");
        // and they are not on the very edge of the screen either
        assert!(offsets[0] > 0 && offsets[2] < 1920, "{offsets:?}");
    }

    /// A mouth that is forgotten is really gone.
    #[test]
    fn a_released_mouth_is_gone() {
        let mut registry = MouthRegistry::new();
        registry.obtain("agent:a:one:1", "one", work(), user_place());
        registry.obtain("agent:a:two:1", "two", work(), user_place());
        assert_eq!(registry.sources().len(), 2);
        registry.release("agent:a:one:1");
        assert!(registry.get("agent:a:one:1").is_none(), "the mouth is gone");
        assert!(registry.get("agent:a:two:1").is_some(), "the other mouth is still there");
        assert_eq!(registry.sources().len(), 1);
    }

    /// Everything can be forgotten at once (the input method is deactivated).
    #[test]
    fn a_cleared_registry_is_empty() {
        let mut registry = MouthRegistry::new();
        registry.obtain(USER_KEY, USER_LABEL, work(), user_place());
        registry.obtain("agent:a:one:1", "one", work(), user_place());
        assert_eq!(registry.sources().len(), 2);
        registry.clear();
        assert!(registry.sources().is_empty());
        assert!(registry.get(USER_KEY).is_none());
        // and the human gets the pale blue again
        let user = registry.obtain(USER_KEY, USER_LABEL, work(), user_place());
        assert_eq!(user.colour, LaserTheme::MOUTH_SLIT);
    }

    /// The human's mouth is where the user put it, even when the registry already knows it.
    #[test]
    fn the_mouth_of_the_human_follows_the_place_of_the_user() {
        let mut registry = MouthRegistry::new();
        let place = MouthPlace {
            edge: Edge::Top,
            offset: 700,
        };
        let user = registry.obtain(USER_KEY, USER_LABEL, work(), place);
        assert_eq!(user.mouth.edge, Edge::Top);
        assert_eq!(user.mouth.x, 700);
        // the same source, the same place: the mouth does not move
        let again = registry.obtain(USER_KEY, USER_LABEL, work(), place);
        assert_eq!(user.mouth, again.mouth);
    }

    /// A mouth that is put at a point of the screen is in the middle of it, not on its border.
    #[test]
    fn a_mouth_that_is_put_at_a_point_is_not_on_the_border() {
        let mut registry = MouthRegistry::new();
        let source = registry.obtain_at("agent:demo:01", "demo-01", work(), user_place(), 900, 400);
        assert_eq!((source.mouth.x, source.mouth.y), (900, 400));
        assert!(source.mouth.x > work().left + source.mouth.radius);
        assert!(source.mouth.x < work().right - source.mouth.radius);
        assert!(source.mouth.y > work().top + source.mouth.radius);
        assert!(source.mouth.y < work().bottom - source.mouth.radius);
        // and it is not the human
        assert!(!source.is_user);
    }

    /// The same agent keeps its colour when it moves to another point of the screen.
    #[test]
    fn a_mouth_that_moves_keeps_its_colour() {
        let mut registry = MouthRegistry::new();
        let first = registry.obtain_at("agent:demo:01", "demo-01", work(), user_place(), 900, 400);
        let again = registry.obtain_at("agent:demo:01", "demo-01", work(), user_place(), 300, 700);
        assert_eq!(first.colour, again.colour, "the colour must not change");
        assert_eq!((again.mouth.x, again.mouth.y), (300, 700), "the mouth moved");
        assert_eq!(registry.sources().len(), 1);
    }

    /// The mouth of the human never follows the demonstration: it stays where the user put it.
    #[test]
    fn the_mouth_of_the_human_does_not_follow_the_demonstration() {
        let mut registry = MouthRegistry::new();
        let place = MouthPlace {
            edge: Edge::Top,
            offset: 700,
        };
        let user = registry.obtain_at(USER_KEY, USER_LABEL, work(), place, 900, 400);
        assert_eq!(user.mouth.x, 700, "the human's mouth stays where the user put it");
        assert_eq!(user.mouth.y, work().top + MOUTH_RADIUS);
    }
}
