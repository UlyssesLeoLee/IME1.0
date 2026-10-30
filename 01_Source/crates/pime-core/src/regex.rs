//! A small backtracking regular-expression matcher, only for the fixed patterns of the symbol index
//! (`symbols.rs`). The patterns are the JavaScript sources of the reference engine, run verbatim, so the matching
//! (leftmost match, greedy/lazy priority, backtracking into groups, `\s` / `\w` / `\b` meaning) is the one of
//! JavaScript without the `u` flag.
//!
//! Supported syntax: literals, `.`, classes `[a-z_]` / `[^)]` with `\w \s \d` inside, `\s \S \w \W \d \D \b \B`,
//! groups `( )`, `(?: )`, negative lookahead `(?! )`, alternation, `* + ?` and their lazy forms, `^`, `$`.
//! There is no `{n,m}` and no back-reference: the patterns do not use them.
//!
//! Matching is bounded by a step budget so that a hostile line cannot make a pattern run away (the reference
//! engine has no such bound); an exhausted budget means "no match".

use crate::util::{is_js_space, is_word};

#[derive(Clone, Debug)]
enum Item {
    Ch(char),
    Range(char, char),
    Word(bool),
    Space(bool),
    Digit(bool),
}

#[derive(Clone, Debug)]
enum Node {
    Ch(char),
    Any,
    Class(Vec<Item>, bool),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Group(Box<Node>, Option<usize>),
    Repeat {
        node: Box<Node>,
        min: usize,
        max: usize,
        greedy: bool,
    },
    Bol,
    Eol,
    WordBoundary(bool),
    NegLookahead(Box<Node>),
}

#[derive(Debug)]
pub struct Regex {
    root: Node,
    groups: usize,
    icase: bool,
}

/// Capture spans (char indices); index 0 is the whole match.
pub type Captures = Vec<Option<(usize, usize)>>;

const STEP_BUDGET: u32 = 400_000;

struct State {
    caps: Captures,
    fuel: u32,
}

impl State {
    fn spend(&mut self) -> bool {
        if self.fuel == 0 {
            return false;
        }
        self.fuel -= 1;
        true
    }
}

struct Parser {
    s: Vec<char>,
    i: usize,
    groups: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn alt(&mut self) -> Node {
        let mut branches = vec![self.seq()];
        while self.eat('|') {
            branches.push(self.seq());
        }
        if branches.len() == 1 {
            branches.remove(0)
        } else {
            Node::Alt(branches)
        }
    }

    fn seq(&mut self) -> Node {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom();
            items.push(self.quantified(atom));
        }
        Node::Seq(items)
    }

    fn quantified(&mut self, atom: Node) -> Node {
        let (min, max) = match self.peek() {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            _ => return atom,
        };
        self.i += 1;
        let greedy = !self.eat('?');
        Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        }
    }

    fn atom(&mut self) -> Node {
        let c = self.s[self.i];
        self.i += 1;
        match c {
            '(' => {
                let node = if self.eat('?') {
                    if self.eat(':') {
                        Node::Group(Box::new(self.alt()), None)
                    } else if self.eat('!') {
                        Node::NegLookahead(Box::new(self.alt()))
                    } else {
                        panic!("unsupported group syntax in regex");
                    }
                } else {
                    self.groups += 1;
                    let index = self.groups;
                    Node::Group(Box::new(self.alt()), Some(index))
                };
                assert!(self.eat(')'), "unbalanced group in regex");
                node
            }
            '[' => self.class(),
            '.' => Node::Any,
            '^' => Node::Bol,
            '$' => Node::Eol,
            '\\' => self.escape(),
            c => Node::Ch(c),
        }
    }

    fn escape(&mut self) -> Node {
        let c = self.s[self.i];
        self.i += 1;
        match c {
            's' => Node::Class(vec![Item::Space(true)], false),
            'S' => Node::Class(vec![Item::Space(false)], false),
            'w' => Node::Class(vec![Item::Word(true)], false),
            'W' => Node::Class(vec![Item::Word(false)], false),
            'd' => Node::Class(vec![Item::Digit(true)], false),
            'D' => Node::Class(vec![Item::Digit(false)], false),
            'b' => Node::WordBoundary(true),
            'B' => Node::WordBoundary(false),
            c => Node::Ch(c),
        }
    }

    fn class(&mut self) -> Node {
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            let c = self.s[self.i];
            self.i += 1;
            if c == ']' {
                break;
            }
            let item = if c == '\\' {
                let e = self.s[self.i];
                self.i += 1;
                match e {
                    's' => Item::Space(true),
                    'S' => Item::Space(false),
                    'w' => Item::Word(true),
                    'W' => Item::Word(false),
                    'd' => Item::Digit(true),
                    'D' => Item::Digit(false),
                    e => Item::Ch(e),
                }
            } else {
                Item::Ch(c)
            };
            // a range `a-z` (a trailing `-` is a literal)
            if let Item::Ch(lo) = item
                && self.peek() == Some('-')
                && self.s.get(self.i + 1).is_some_and(|&n| n != ']')
            {
                self.i += 1;
                let mut hi = self.s[self.i];
                self.i += 1;
                if hi == '\\' {
                    hi = self.s[self.i];
                    self.i += 1;
                }
                items.push(Item::Range(lo, hi));
            } else {
                items.push(item);
            }
        }
        Node::Class(items, negated)
    }
}

fn item_matches(item: &Item, c: char) -> bool {
    match item {
        Item::Ch(x) => *x == c,
        Item::Range(a, b) => *a <= c && c <= *b,
        Item::Word(yes) => is_word(c) == *yes,
        Item::Space(yes) => is_js_space(c) == *yes,
        Item::Digit(yes) => c.is_ascii_digit() == *yes,
    }
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

impl Regex {
    /// Compiles a pattern; the patterns are constants of this crate, so a syntax error is a programming error.
    pub fn new(pattern: &str, icase: bool) -> Regex {
        let mut p = Parser {
            s: pattern.chars().collect(),
            i: 0,
            groups: 0,
        };
        let root = p.alt();
        assert!(p.i == p.s.len(), "unbalanced regex {pattern}");
        Regex {
            root,
            groups: p.groups,
            icase,
        }
    }

    fn char_eq(&self, a: char, b: char) -> bool {
        a == b || (self.icase && a.is_ascii() && b.is_ascii() && a.eq_ignore_ascii_case(&b))
    }

    fn class_matches(&self, items: &[Item], negated: bool, c: char) -> bool {
        let mut hit = items.iter().any(|it| item_matches(it, c));
        if !hit && self.icase && c.is_ascii_alphabetic() {
            let other = if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            hit = items.iter().any(|it| item_matches(it, other));
        }
        hit != negated
    }

    fn m(
        &self,
        node: &Node,
        t: &[char],
        i: usize,
        st: &mut State,
        k: &mut dyn FnMut(usize, &mut State) -> bool,
    ) -> bool {
        if !st.spend() {
            return false;
        }
        match node {
            Node::Ch(c) => t.get(i).is_some_and(|&x| self.char_eq(*c, x)) && k(i + 1, st),
            Node::Any => t.get(i).is_some_and(|&x| !is_line_terminator(x)) && k(i + 1, st),
            Node::Class(items, negated) => {
                t.get(i)
                    .is_some_and(|&x| self.class_matches(items, *negated, x))
                    && k(i + 1, st)
            }
            Node::Seq(nodes) => self.seq(nodes, t, i, st, k),
            Node::Alt(branches) => {
                for b in branches {
                    if self.m(b, t, i, st, k) {
                        return true;
                    }
                }
                false
            }
            Node::Group(inner, capture) => match capture {
                None => self.m(inner, t, i, st, k),
                Some(index) => {
                    let index = *index;
                    let saved = st.caps[index];
                    let ok = self.m(inner, t, i, st, &mut |j, st| {
                        let before = st.caps[index];
                        st.caps[index] = Some((i, j));
                        if k(j, st) {
                            return true;
                        }
                        st.caps[index] = before;
                        false
                    });
                    if !ok {
                        st.caps[index] = saved;
                    }
                    ok
                }
            },
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.rep(node, *min, *max, *greedy, 0, t, i, st, k),
            Node::Bol => i == 0 && k(i, st),
            Node::Eol => i == t.len() && k(i, st),
            Node::WordBoundary(want) => {
                let before = i > 0 && is_word(t[i - 1]);
                let after = i < t.len() && is_word(t[i]);
                ((before != after) == *want) && k(i, st)
            }
            Node::NegLookahead(inner) => {
                let mut scratch = State {
                    caps: st.caps.clone(),
                    fuel: st.fuel,
                };
                let matched = self.m(inner, t, i, &mut scratch, &mut |_, _| true);
                st.fuel = scratch.fuel;
                !matched && k(i, st)
            }
        }
    }

    fn seq(
        &self,
        nodes: &[Node],
        t: &[char],
        i: usize,
        st: &mut State,
        k: &mut dyn FnMut(usize, &mut State) -> bool,
    ) -> bool {
        match nodes.split_first() {
            None => k(i, st),
            Some((first, rest)) => {
                self.m(first, t, i, st, &mut |j, st| self.seq(rest, t, j, st, k))
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // a recursive matcher: every argument is part of the state
    fn rep(
        &self,
        node: &Node,
        min: usize,
        max: usize,
        greedy: bool,
        count: usize,
        t: &[char],
        i: usize,
        st: &mut State,
        k: &mut dyn FnMut(usize, &mut State) -> bool,
    ) -> bool {
        if !st.spend() {
            return false;
        }
        if greedy {
            if self.one_more(node, min, max, greedy, count, t, i, st, k) {
                return true;
            }
            count >= min && k(i, st)
        } else {
            if count >= min && k(i, st) {
                return true;
            }
            self.one_more(node, min, max, greedy, count, t, i, st, k)
        }
    }

    /// One more iteration of a repeat, then the rest of the repeat and the continuation.
    #[allow(clippy::too_many_arguments)] // see `rep`
    fn one_more(
        &self,
        node: &Node,
        min: usize,
        max: usize,
        greedy: bool,
        count: usize,
        t: &[char],
        i: usize,
        st: &mut State,
        k: &mut dyn FnMut(usize, &mut State) -> bool,
    ) -> bool {
        count < max
            && self.m(node, t, i, st, &mut |j, st| {
                // an empty iteration beyond the minimum can never help (and would loop forever)
                if j == i && count >= min {
                    return false;
                }
                self.rep(node, min, max, greedy, count + 1, t, j, st, k)
            })
    }

    /// First match anywhere in `text` (like `re.exec`): the capture spans, or `None`.
    pub fn exec(&self, text: &[char]) -> Option<Captures> {
        for start in 0..=text.len() {
            let mut st = State {
                caps: vec![None; self.groups + 1],
                fuel: STEP_BUDGET,
            };
            let mut end = None;
            let matched = self.m(&self.root, text, start, &mut st, &mut |j, _| {
                end = Some(j);
                true
            });
            if matched && let Some(j) = end {
                st.caps[0] = Some((start, j));
                return Some(st.caps);
            }
            if st.fuel == 0 {
                return None;
            }
        }
        None
    }

    pub fn is_match(&self, text: &[char]) -> bool {
        self.exec(text).is_some()
    }
}

/// The text of capture `n`.
pub fn group(text: &[char], caps: &Captures, n: usize) -> Option<String> {
    caps.get(n)
        .copied()
        .flatten()
        .map(|(a, b)| text[a..b].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pattern: &str, icase: bool, text: &str) -> Option<Vec<Option<String>>> {
        let re = Regex::new(pattern, icase);
        let t: Vec<char> = text.chars().collect();
        re.exec(&t)
            .map(|caps| (0..caps.len()).map(|n| group(&t, &caps, n)).collect())
    }

    fn g(text: &str, n: usize, pattern: &str) -> Option<String> {
        run(pattern, false, text).and_then(|v| v[n].clone())
    }

    #[test]
    fn literals_classes_and_anchors() {
        assert_eq!(g("fn main()", 1, r"^(fn)\s+"), Some("fn".into()));
        assert_eq!(g("  fn x", 1, r"^\s*(fn|struct)\s+"), Some("fn".into()));
        assert!(run(r"^abc$", false, "abc").is_some());
        assert!(run(r"^abc$", false, "abcd").is_none());
        assert_eq!(g("a-b_c", 1, r"([\w-]+)"), Some("a-b_c".into()));
        assert_eq!(g("x=1", 1, r"([^=]+)="), Some("x".into()));
    }

    #[test]
    fn greedy_and_lazy_quantifiers_backtrack_like_javascript() {
        // lazy prefix: the shortest run before the name
        assert_eq!(
            g(
                "int add(int a)",
                1,
                r"^[A-Za-z_][\w\s\*]*?[\s\*]([A-Za-z_]\w*)\s*\("
            ),
            Some("add".into())
        );
        // greedy `\s*` gives back what the next token needs
        assert_eq!(g("fn   x", 1, r"^fn\s*(\w)"), Some("x".into()));
        assert_eq!(g("aaa", 1, r"^(a*)a$"), Some("aa".into()));
        assert_eq!(g("aaa", 1, r"^(a*?)a*$"), Some("".into()));
    }

    #[test]
    fn optional_groups_alternation_and_negative_lookahead() {
        assert_eq!(
            g(
                "pub(crate) fn run",
                2,
                r"^(?:pub(?:\([^)]*\))?\s+)?(fn|struct)\s+(\w+)"
            ),
            Some("run".into())
        );
        assert!(run(r"^(?!\s*(?:if|for)\b)\w+", false, "if x").is_none());
        assert!(run(r"^(?!\s*(?:if|for)\b)\w+", false, "iffy x").is_some());
        assert!(run(r"^(?:a|ab)c", false, "abc").is_some());
    }

    #[test]
    fn case_insensitive_matches_ascii_letters() {
        assert!(g("FUNCTION Get-X", 1, r"^\s*(?:filter|function)\s+([\w-]+)").is_none());
        let hit = run(
            r"^\s*(?:filter|function)\s+([\w-]+)",
            true,
            "FUNCTION Get-X",
        )
        .unwrap();
        assert_eq!(hit[1].as_deref(), Some("Get-X"));
        assert!(run(r"^(?:MERGE|MATCH)\b", true, "merge (n)").is_some());
    }

    #[test]
    fn a_hostile_line_exhausts_the_budget_instead_of_running_away() {
        let re = Regex::new(r"^(a*)*b", false);
        let t: Vec<char> = "a".repeat(40).chars().collect();
        assert!(re.exec(&t).is_none());
    }
}
