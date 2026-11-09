//! The six string algorithms of the catalogue (the assets `algorithm.string.*`).
//!
//! Every public function of this module names its asset in its doc comment:
//! `* asset: `algorithm.\<category\>.\<name>``.
//!
//! The rules the six algorithms keep:
//!
//! * **The standard library only.** No third-party crate: the crate has to stay auditable line by line.
//! * **Exact matching.** Every algorithm of this module answers the question "where does this literal
//!   occur". None of them answers "how similar are these two texts": that is the job of
//!   `algorithm.string.edit_distance`.
//! * **The unit of the comparison is the element, not the byte.** The functions that work on a slice of
//!   `char` see one character as one unit. The ones that work on a slice of bytes see one byte as one
//!   unit, and a match can therefore fall inside a UTF-8 sequence. A caller that matches text should
//!   pass the characters, not the bytes.
//! * **Deterministic.** The same input gives the same output, always. The trie fixes the order of its
//!   prefix queries (lexicographic), the automaton fixes the order of its matches (by position).
//! * **No panic on a valid input.** An empty input gives an empty result. An input that cannot be
//!   handled (an empty pattern) is reported as an `Err` with a readable reason.

use std::collections::HashMap;
use std::collections::VecDeque;

// ---- algorithm.string.kmp ---------------------------------------------------------------------------------------

/// The prefix table of a pattern: `table[i]` is the length of the longest proper prefix of
/// `pattern[0..=i]` that is also a suffix of it.
///
/// The table is the memory of the algorithm: it is what lets the cursor of the text move forward only.
fn prefix_table<T: PartialEq>(pattern: &[T]) -> Vec<usize> {
    let mut table = vec![0usize; pattern.len()];
    let mut matched = 0usize;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = table[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        table[index] = matched;
    }
    table
}

/// Finds every occurrence of `pattern` in `text` with the Knuth-Morris-Pratt algorithm.
///
/// The algorithm remembers how much of the pattern has already matched, so the cursor of the text never
/// moves back: the whole search costs `O(n + m)` instead of the `O(n * m)` of the naive search, where `n`
/// is the length of the text and `m` the length of the pattern.
///
/// # Input
///
/// - `text` and `pattern` are slices of the same element type. The element is the unit of the
///   comparison: pass the characters of a text to match whole characters, pass the bytes to match
///   bytes. A byte match can fall inside a UTF-8 sequence, which is why the card warns about it.
/// - `pattern` must not be empty. An empty pattern occurs at every position of every text; the card
///   rejects it and this function reports it as an `Err` instead of answering.
/// - An empty `text` is legal: the result is empty.
///
/// # Complexity
///
/// - Time: `O(n + m)` - the prefix table is built once and the text is scanned once.
/// - Extra space: `O(m)` - the prefix table.
///
/// # When it does not apply
///
/// - The card rejects it when only one occurrence of a very short pattern is needed: the naive search
///   has the smaller constant.
/// - The card rejects it when many patterns have to be matched at once: one search per pattern costs
///   `O(k * (n + m))`, and `algorithm.string.aho_corasick` scans the text once.
/// - The card rejects it when the pattern changes at every call: the prefix table cannot be reused, so
///   the preprocessing is not amortised. This function rebuilds the table on every call.
/// - The card rejects it for a semantic (inexact) match: KMP is an exact match.
///
/// * asset: `algorithm.string.kmp`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.string.kmp/card.md`
pub fn kmp<T: PartialEq>(text: &[T], pattern: &[T]) -> Result<Vec<usize>, String> {
    if pattern.is_empty() {
        return Err(
            "kmp: the pattern is empty; an empty pattern matches at every position, so it is rejected"
                .to_owned(),
        );
    }
    if text.len() < pattern.len() {
        return Ok(Vec::new());
    }

    let table = prefix_table(pattern);
    let mut matches = Vec::new();
    let mut matched = 0usize;
    for (index, item) in text.iter().enumerate() {
        while matched > 0 && *item != pattern[matched] {
            matched = table[matched - 1];
        }
        if *item == pattern[matched] {
            matched += 1;
        }
        if matched == pattern.len() {
            matches.push(index + 1 - pattern.len());
            // the overlap: the pattern may start again inside the one that just matched
            matched = table[matched - 1];
        }
    }
    Ok(matches)
}

// ---- algorithm.string.z_algorithm -------------------------------------------------------------------------------

/// Computes the Z array of a sequence: for every position, the length of the longest common prefix of
/// the sequence and of the suffix that starts there.
///
/// The Z array answers "how much of the beginning of this string is repeated at this position", which
/// is the raw material of a period analysis and of the "pattern + separator + text" construction.
///
/// # Input
///
/// - `s` is the whole sequence. The Z array is a property of the whole sequence, so the function
///   cannot answer a question about a prefix of a stream that is still arriving.
/// - An empty sequence is legal: the result is an empty array.
/// - The element is the unit of the comparison: pass the characters of a text to compare whole
///   characters.
///
/// # Complexity
///
/// - Time: `O(n)` - every position is compared a constant number of times.
/// - Extra space: `O(n)` - the array itself.
///
/// # When it does not apply
///
/// - The card rejects it when only the position of one pattern is needed: KMP is simpler and carries
///   the smaller mental load.
/// - The card rejects it for a very short string: a naive comparison is enough.
/// - The card rejects it when the input arrives as a stream that cannot be read back: the Z function
///   needs the whole string in memory before it can answer anything.
/// - The card rejects it when many patterns have to be matched: `algorithm.string.aho_corasick` scans
///   the text once for all of them.
///
/// * asset: `algorithm.string.z_algorithm`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.string.z_algorithm/card.md`
pub fn z_algorithm<T: PartialEq>(s: &[T]) -> Vec<usize> {
    let length = s.len();
    if length == 0 {
        return Vec::new();
    }
    // the window [left, right) is the rightmost region that is known to match the start of the
    // sequence: it is what keeps the whole computation linear
    let mut z = vec![0usize; length];
    z[0] = length;
    let mut left = 0usize;
    let mut right = 0usize;
    for index in 1..length {
        if index < right {
            z[index] = (right - index).min(z[index - left]);
        }
        while index + z[index] < length && s[z[index]] == s[index + z[index]] {
            z[index] += 1;
        }
        if index + z[index] > right {
            left = index;
            right = index + z[index];
        }
    }
    z
}

// ---- algorithm.string.trie --------------------------------------------------------------------------------------

/// A key and the value the trie carries for it.
pub type TrieEntry<'a> = (&'a str, &'a str);

/// A node of the trie.
#[derive(Debug, Default)]
struct TrieNode {
    /// The children of the node, one per character that follows the prefix of the node.
    children: HashMap<char, TrieNode>,
    /// The entry that ends at this node, when a key ends here.
    entry: Option<(String, String)>,
}

/// A prefix tree: the keys share the path of their common prefix.
///
/// The trie answers the question "which keys start with this prefix" in the time of the length of the
/// prefix, and it answers it without comparing the whole key with every key of the set.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::string::trie;
///
/// let tree = trie(&[("apple", "1"), ("apply", "2"), ("banana", "3")]);
/// assert_eq!(tree.prefix_search("app"), vec!["apple", "apply"]);
/// assert_eq!(tree.get("banana"), Some("3"));
/// ```
#[derive(Debug, Default)]
pub struct Trie {
    root: TrieNode,
    size: usize,
}

impl Trie {
    /// An empty trie.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a key and the value it carries. A key that is already there keeps its place and takes
    /// the new value.
    pub fn insert(&mut self, key: &str, value: &str) {
        let mut node = &mut self.root;
        for character in key.chars() {
            node = node.children.entry(character).or_default();
        }
        let is_new = node.entry.is_none();
        node.entry = Some((key.to_owned(), value.to_owned()));
        if is_new {
            self.size += 1;
        }
    }

    /// The node of the prefix, when the prefix is in the trie.
    fn node_at(&self, prefix: &str) -> Option<&TrieNode> {
        let mut node = &self.root;
        for character in prefix.chars() {
            node = node.children.get(&character)?;
        }
        Some(node)
    }

    /// The value of the key, when the key is in the trie.
    pub fn get(&self, key: &str) -> Option<&str> {
        let node = self.node_at(key)?;
        node.entry.as_ref().map(|(_, value)| value.as_str())
    }

    /// Whether the key is in the trie.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Every key that starts with the prefix, in lexicographic order.
    ///
    /// The order is fixed on purpose: the walk of the trie is an ordered walk, so the result of the
    /// same trie is always the same list, whatever the order the keys were inserted in.
    pub fn prefix_search(&self, prefix: &str) -> Vec<&str> {
        let Some(node) = self.node_at(prefix) else {
            return Vec::new();
        };
        let mut keys: Vec<&str> = Vec::new();
        let mut stack: Vec<&TrieNode> = vec![node];
        while let Some(current) = stack.pop() {
            if let Some((key, _)) = &current.entry {
                keys.push(key.as_str());
            }
            for child in current.children.values() {
                stack.push(child);
            }
        }
        keys.sort_unstable();
        keys
    }

    /// The number of keys in the trie.
    pub fn len(&self) -> usize {
        self.size
    }

    /// Whether the trie holds no key.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

/// Builds a trie that holds the given keys and the values they carry.
///
/// The keys share the path of their common prefix, so a prefix query walks the trie once and collects
/// the keys that hang under the prefix.
///
/// # Input
///
/// - `entries` is the list of the keys and of the values they carry. The same key twice keeps the
///   value of the last occurrence.
/// - The empty list is legal: the result is an empty trie.
/// - The empty key is legal: it is the root of the trie and it is found by a query of the empty prefix.
///
/// # Complexity
///
/// - Time: `O(len)` per key - one step of the walk per character.
/// - Extra space: `O(total characters)` - one node per character, plus the keys and the values.
///
/// # When it does not apply
///
/// - The card rejects it when only an exact lookup is needed: a hash table costs less memory.
/// - The card rejects it when there are few keys: a linear scan or an ordered table is simpler.
/// - The card rejects it when the keys are long and share little: one node per character wastes memory.
/// - The card rejects it for a fuzzy match (an allowed edit distance): the trie only matches prefixes.
/// - The card rejects it for a query that is neither a lookup nor a prefix query: the structure does
///   not answer them.
///
/// * asset: `algorithm.string.trie`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.string.trie/card.md`
pub fn trie(entries: &[TrieEntry<'_>]) -> Trie {
    let mut tree = Trie::new();
    for (key, value) in entries {
        tree.insert(key, value);
    }
    tree
}

// ---- algorithm.string.aho_corasick ------------------------------------------------------------------------------

/// A match of the automaton: the pattern that was found, and where it was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AhoMatch {
    /// The byte offset of the beginning of the match.
    pub start: usize,
    /// The byte offset of the end of the match (excluded).
    pub end: usize,
    /// The index of the pattern in the list the automaton was built from.
    pub pattern: usize,
}

/// A node of the automaton: the transitions, the failure link and the patterns that end here.
#[derive(Debug, Default)]
struct AhoNode {
    children: HashMap<char, usize>,
    failure: usize,
    output: Vec<usize>,
}

/// The automaton of Aho-Corasick: many patterns are woven into one trie with failure links, so a
/// single scan of the text tells which patterns occur and where.
///
/// The automaton is read-only once it is built, so it can be shared between threads (`Send` and
/// `Sync`) and every scanning thread keeps its own state, which is a single integer.
#[derive(Debug)]
pub struct AhoCorasick {
    nodes: Vec<AhoNode>,
    patterns: Vec<String>,
    lengths: Vec<usize>,
    max_matches: usize,
}

/// The default number of matches a single scan may report.
///
/// The card asks for a guard: the number of matches of a scan can be larger than the length of the
/// text (a pattern of one character against a text of one character repeated), and a caller that does
/// not want that has to be able to stop the scan.
pub const DEFAULT_MAX_MATCHES: usize = 1_000_000;

impl AhoCorasick {
    /// Builds the automaton of the patterns.
    ///
    /// - `patterns` is the list of the patterns. The index of a pattern in this list is the index the
    ///   matches report.
    /// - An empty pattern is rejected: it occurs at every position of every text.
    /// - An empty list of patterns is legal: the automaton then reports nothing.
    pub fn new(patterns: &[&str]) -> Result<Self, String> {
        Self::with_max_matches(patterns, DEFAULT_MAX_MATCHES)
    }

    /// Builds the automaton of the patterns and caps the number of matches a single scan may report.
    ///
    /// The cap is the guard the card asks for: a scan that would report more matches than the cap
    /// fails with an `Err` instead of filling the memory of the caller.
    pub fn with_max_matches(patterns: &[&str], max_matches: usize) -> Result<Self, String> {
        if max_matches == 0 {
            return Err(
                "aho_corasick: the number of matches a scan may report must be at least 1".to_owned(),
            );
        }

        let mut nodes = vec![AhoNode::default()];
        let mut lengths = Vec::with_capacity(patterns.len());
        for (index, pattern) in patterns.iter().enumerate() {
            if pattern.is_empty() {
                return Err(format!(
                    "aho_corasick: the pattern at index {index} is empty; an empty pattern matches at \
                     every position, so it is rejected"
                ));
            }
            lengths.push(pattern.chars().count());
            let mut current = 0usize;
            for character in pattern.chars() {
                let next = match nodes[current].children.get(&character) {
                    Some(&next) => next,
                    None => {
                        let next = nodes.len();
                        nodes.push(AhoNode::default());
                        nodes[current].children.insert(character, next);
                        next
                    }
                };
                current = next;
            }
            nodes[current].output.push(index);
        }

        // the failure links, in the breadth-first order: the link of a node is the longest proper
        // suffix of its path that is also a path of the trie
        let mut queue: VecDeque<usize> = VecDeque::new();
        let roots: Vec<usize> = nodes[0].children.values().copied().collect();
        for child in roots {
            nodes[child].failure = 0;
            queue.push_back(child);
        }
        while let Some(node) = queue.pop_front() {
            let children: Vec<(char, usize)> =
                nodes[node].children.iter().map(|(&c, &n)| (c, n)).collect();
            for (character, child) in children {
                let mut fallback = nodes[node].failure;
                while fallback != 0 && !nodes[fallback].children.contains_key(&character) {
                    fallback = nodes[fallback].failure;
                }
                nodes[child].failure = match nodes[fallback].children.get(&character) {
                    Some(&target) => target,
                    None => 0,
                };
                let inherited = nodes[nodes[child].failure].output.clone();
                nodes[child].output.extend(inherited);
                queue.push_back(child);
            }
        }

        Ok(Self {
            nodes,
            patterns: patterns.iter().map(|pattern| (*pattern).to_owned()).collect(),
            lengths,
            max_matches,
        })
    }

    /// The number of patterns of the automaton.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Scans the text and reports every match, the overlapping ones included.
    ///
    /// The matches are ordered by their position (the beginning, then the end, then the index of the
    /// pattern), so the same text always gives the same list.
    pub fn search(&self, text: &str) -> Result<Vec<AhoMatch>, String> {
        let mut matches: Vec<AhoMatch> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();
        let mut state = 0usize;
        for (offset, character) in text.char_indices() {
            offsets.push(offset);
            let end = offset + character.len_utf8();
            while state != 0 && !self.nodes[state].children.contains_key(&character) {
                state = self.nodes[state].failure;
            }
            state = match self.nodes[state].children.get(&character) {
                Some(&next) => next,
                None => 0,
            };
            for &pattern in &self.nodes[state].output {
                let length = self.lengths[pattern];
                let start = offsets[offsets.len() - length];
                matches.push(AhoMatch {
                    pattern,
                    start,
                    end,
                });
                if matches.len() > self.max_matches {
                    return Err(format!(
                        "aho_corasick: the scan found more than {} matches; raise the limit of the \
                         automaton to go on",
                        self.max_matches
                    ));
                }
            }
        }
        matches.sort_unstable();
        Ok(matches)
    }
}

/// Builds the automaton that matches many patterns in one scan of the text.
///
/// The patterns are woven into one trie with failure links, so the text is read once and every
/// pattern that occurs is reported, the overlapping ones included.
///
/// # Input
///
/// - `patterns` is the list of the patterns. The index of a pattern in this list is the index the
///   matches report.
/// - An empty pattern is rejected: it occurs at every position of every text.
/// - An empty list of patterns is legal: the scan then reports nothing.
/// - An empty text is legal: the scan then reports nothing.
///
/// # Complexity
///
/// - Build: `O(total length of the patterns)` - one step of the walk per character, plus one step per
///   node for the failure links.
/// - Scan: `O(length of the text + number of matches)`.
/// - Extra space: `O(total length of the patterns)` - one node per character.
///
/// # When it does not apply
///
/// - The card rejects it when there is only one pattern: KMP is simpler and its constant is smaller.
/// - The card rejects it when there are few patterns and the text is short: one search per pattern
///   may cost less memory.
/// - The card rejects it for a fuzzy or approximate match: the automaton is an exact match.
/// - The card rejects it when the patterns are very many and the alphabet is very large: the memory of
///   the automaton explodes.
///
/// * asset: `algorithm.string.aho_corasick`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.string.aho_corasick/card.md`
pub fn aho_corasick(patterns: &[&str]) -> Result<AhoCorasick, String> {
    AhoCorasick::new(patterns)
}

// ---- algorithm.string.rabin_karp --------------------------------------------------------------------------------

/// The hash of a byte sequence, with the rolling rule `hash = hash * base + byte`.
fn hash_bytes(bytes: &[u8], base: u128, modulus: u128) -> u128 {
    let mut hash = 0u128;
    for byte in bytes {
        hash = (hash * base + u128::from(*byte)) % modulus;
    }
    hash
}

/// Finds every occurrence of `pattern` in `text` with the Rabin-Karp algorithm.
///
/// The algorithm keeps a rolling hash of the window of the text that is under the pattern. The window
/// is compared with the pattern only when the two hashes are equal, and the comparison is always made
/// again on the bytes themselves, so a collision costs a comparison and never a wrong answer.
///
/// # Input
///
/// - `text` and `pattern` are byte sequences. The byte is the unit: a match can fall inside a UTF-8
///   sequence, so a caller that matches text should check the boundaries itself, or use
///   `algorithm.string.kmp` on the characters.
/// - `pattern` must not be empty: an empty pattern occurs at every position.
/// - `base` must be at least 2 and `modulus` at least 2: a smaller modulus makes the hash constant
///   and a smaller base makes it degenerate. A base that is a multiple of the modulus is allowed; it
///   only makes the collisions more frequent, and the second comparison keeps the answer right.
/// - An empty `text` is legal: the result is empty.
///
/// # Complexity
///
/// - Time: `O(n + m)` in expectation, `O(n * m)` in the worst case (when every window collides).
/// - Extra space: `O(1)` - the window hash, the pattern hash and the power of the base.
///
/// # When it does not apply
///
/// - The card rejects it when there is a single pattern and the text is long: KMP is deterministic and
///   costs `O(n + m)` in the worst case too.
/// - The card rejects it for a very short pattern and a very short text: a naive comparison has the
///   smaller constant.
/// - The card rejects it when the caller does not want to compare again after a hash match: the hash
///   alone reports false positives.
/// - The card rejects it when only the hashes are needed: the rolling hash on its own is the answer,
///   and the matching is only the excuse.
///
/// * asset: `algorithm.string.rabin_karp`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.string.rabin_karp/card.md`
pub fn rabin_karp(
    text: &[u8],
    pattern: &[u8],
    base: u64,
    modulus: u64,
) -> Result<Vec<usize>, String> {
    if pattern.is_empty() {
        return Err(
            "rabin_karp: the pattern is empty; an empty pattern matches at every position, so it is \
             rejected"
                .to_owned(),
        );
    }
    if modulus < 2 {
        return Err(format!(
            "rabin_karp: the modulus must be at least 2, {modulus} was given"
        ));
    }
    if base < 2 {
        return Err(format!(
            "rabin_karp: the base must be at least 2, {base} was given"
        ));
    }
    if text.len() < pattern.len() {
        return Ok(Vec::new());
    }

    let base = u128::from(base);
    let modulus = u128::from(modulus);
    let pattern_hash = hash_bytes(pattern, base, modulus);
    let mut window_hash = hash_bytes(&text[..pattern.len()], base, modulus);

    // base^(m-1) modulo the modulus: the weight of the byte that leaves the window
    let mut high = 1u128;
    for _ in 1..pattern.len() {
        high = (high * base) % modulus;
    }

    let mut matches = Vec::new();
    let mut index = 0usize;
    while index + pattern.len() <= text.len() {
        if window_hash == pattern_hash && &text[index..index + pattern.len()] == pattern {
            matches.push(index);
        }
        if index + pattern.len() < text.len() {
            let leaving = u128::from(text[index]);
            let entering = u128::from(text[index + pattern.len()]);
            let reduced = (window_hash + modulus - (leaving * high) % modulus) % modulus;
            window_hash = (reduced * base + entering) % modulus;
        }
        index += 1;
    }
    Ok(matches)
}

// ---- algorithm.string.edit_distance ------------------------------------------------------------------------------

/// The edit distance of two sequences of characters, with the rolling array of the card.
fn edit_distance_of(left: &[char], right: &[char]) -> usize {
    // the shorter sequence is the one that is kept in the row
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            let substitution = previous[j] + cost;
            current[j + 1] = deletion.min(insertion).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// The number of insertions, deletions and substitutions that turn `a` into `b`.
///
/// The distance is the Levenshtein distance: every operation costs one, and the three operations are
/// the only ones allowed.
///
/// # Input
///
/// - `a` and `b` are texts. The unit is the **character**, not the byte and not the grapheme: a
///   letter with a combining mark is two characters here, and the standard library has no segmenter
///   that would make it one. A caller that needs the grapheme as the unit has to segment the text
///   itself before calling.
/// - An empty text is legal: the distance is the length of the other text.
/// - The two texts may be of any length.
///
/// # Complexity
///
/// - Time: `O(n * m)`, where `n` and `m` are the numbers of characters.
/// - Extra space: `O(min(n, m))` - two rolling rows.
///
/// # When it does not apply
///
/// - The card rejects it when only the equality of the two texts is needed: a comparison is enough.
/// - The card rejects it for very long texts: `O(n * m)` is too much, and a bit-parallel or heuristic
///   method is the answer.
/// - The card rejects it when the similarity that is wanted is semantic: embeddings or a tokenizer
///   are the answer, not the number of edits.
/// - The card rejects it when the operations have different costs: the transitions of the matrix are
///   the same, but the costs have to be parameterised, and this function does not parameterise them.
/// - The card rejects it when the transpositions have to be counted: this is the Levenshtein distance,
///   not the Damerau-Levenshtein distance.
///
/// * asset: `algorithm.string.edit_distance`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.string.edit_distance/card.md`
pub fn edit_distance(a: &str, b: &str) -> usize {
    let left: Vec<char> = a.chars().collect();
    let right: Vec<char> = b.chars().collect();
    if left.len() < right.len() {
        return edit_distance_of(&right, &left);
    }
    edit_distance_of(&left, &right)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The naive search, used as the reference of the linear searches.
    fn naive_matches<T: PartialEq>(text: &[T], pattern: &[T]) -> Vec<usize> {
        if pattern.is_empty() || pattern.len() > text.len() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        for index in 0..=text.len() - pattern.len() {
            if &text[index..index + pattern.len()] == pattern {
                matches.push(index);
            }
        }
        matches
    }

    /// The Z array computed the slow way, used as the reference of the linear algorithm.
    fn naive_z<T: PartialEq>(s: &[T]) -> Vec<usize> {
        let mut z = vec![0usize; s.len()];
        for index in 0..s.len() {
            let mut length = 0usize;
            while index + length < s.len() && s[length] == s[index + length] {
                length += 1;
            }
            z[index] = length;
        }
        if !z.is_empty() {
            z[0] = s.len();
        }
        z
    }

    /// The edit distance computed by the recursion, used as the reference of the dynamic programming.
    fn naive_edit_distance(left: &[char], right: &[char]) -> usize {
        if left.is_empty() {
            return right.len();
        }
        if right.is_empty() {
            return left.len();
        }
        let cost = usize::from(left[0] != right[0]);
        let deletion = naive_edit_distance(&left[1..], right) + 1;
        let insertion = naive_edit_distance(left, &right[1..]) + 1;
        let substitution = naive_edit_distance(&left[1..], &right[1..]) + cost;
        deletion.min(insertion).min(substitution)
    }

    // ---- algorithm.string.kmp

    #[test]
    fn kmp_finds_the_pattern_at_every_offset() {
        // the overlapping matches of "abab" in "abababc" are at 0 and at 2
        assert_eq!(
            kmp(&"abababc".chars().collect::<Vec<_>>(), &"abab".chars().collect::<Vec<_>>())
                .unwrap(),
            vec![0, 2]
        );
        // a pattern of one character hits every occurrence
        assert_eq!(
            kmp(&"abababc".chars().collect::<Vec<_>>(), &"a".chars().collect::<Vec<_>>()).unwrap(),
            vec![0, 2, 4]
        );
        // the overlaps are found too
        assert_eq!(
            kmp(&"aaaa".chars().collect::<Vec<_>>(), &"aa".chars().collect::<Vec<_>>()).unwrap(),
            vec![0, 1, 2]
        );
        // the empty text and the too short pattern give an empty result
        assert!(kmp(&[], &"a".chars().collect::<Vec<_>>()).unwrap().is_empty());
        assert!(
            kmp(&"ab".chars().collect::<Vec<_>>(), &"abc".chars().collect::<Vec<_>>())
                .unwrap()
                .is_empty()
        );
        // the answer is the answer of the naive search
        let text: Vec<char> = "abracadabra abracadabra".chars().collect();
        for pattern in ["a", "ab", "abra", "cad", "abracadabra", "zz"] {
            let pattern: Vec<char> = pattern.chars().collect();
            assert_eq!(kmp(&text, &pattern).unwrap(), naive_matches(&text, &pattern));
        }
    }

    #[test]
    fn kmp_rejects_the_empty_pattern_that_the_card_excludes() {
        // the empty pattern would match at every position: the card rejects it and the function says so
        let error = kmp(&"abc".chars().collect::<Vec<_>>(), &[]).unwrap_err();
        assert!(error.contains("empty"), "the reason must be readable: {error}");
        // the same for the bytes
        assert!(kmp(b"abc", b"").is_err());
        // the single element pattern is legal
        assert_eq!(
            kmp(&"abc".chars().collect::<Vec<_>>(), &"c".chars().collect::<Vec<_>>()).unwrap(),
            vec![2]
        );
    }

    // ---- algorithm.string.z_algorithm

    #[test]
    fn z_algorithm_matches_the_brute_force_common_prefixes() {
        assert_eq!(z_algorithm(&"ababab".chars().collect::<Vec<_>>()), vec![6, 0, 4, 0, 2, 0]);
        assert_eq!(z_algorithm(&"aaaa".chars().collect::<Vec<_>>()), vec![4, 3, 2, 1]);
        assert_eq!(z_algorithm(&"abcd".chars().collect::<Vec<_>>()), vec![4, 0, 0, 0]);
        assert_eq!(z_algorithm(&"a".chars().collect::<Vec<_>>()), vec![1]);
        // the empty string gives the empty array
        assert!(z_algorithm::<char>(&[]).is_empty());
        // the answer is the answer of the slow computation
        for text in ["", "a", "ababab", "aaaa", "abcabcabc", "abbaabba", "the quick brown fox"] {
            let characters: Vec<char> = text.chars().collect();
            assert_eq!(z_algorithm(&characters), naive_z(&characters));
        }
    }

    #[test]
    fn z_algorithm_finds_the_pattern_through_the_separator_construction() {
        // the construction of the card: "pattern + separator + text"
        let pattern = "abab";
        let text = "xxababyy";
        let separator = '#';
        let mut combined: Vec<char> = pattern.chars().collect();
        combined.push(separator);
        combined.extend(text.chars());
        let z = z_algorithm(&combined);
        let offset = pattern.chars().count() + 1;
        let mut found = Vec::new();
        for (index, length) in z.iter().enumerate().skip(offset) {
            if *length >= pattern.chars().count() {
                found.push(index - offset);
            }
        }
        assert_eq!(found, vec![2]);
        // the separator must not appear in the text for the construction to hold
        assert!(!text.contains(separator));
    }

    // ---- algorithm.string.trie

    #[test]
    fn trie_finds_the_keys_by_prefix() {
        let tree = trie(&[
            ("apple", "1"),
            ("app", "2"),
            ("apply", "3"),
            ("banana", "4"),
        ]);
        assert_eq!(tree.get("app"), Some("2"));
        assert_eq!(tree.get("apple"), Some("1"));
        assert_eq!(tree.get("apply"), Some("3"));
        assert_eq!(tree.get("banana"), Some("4"));
        assert_eq!(tree.get("apricot"), None);
        // the prefix query gives exactly the keys that start with the prefix, in a fixed order
        assert_eq!(tree.prefix_search("app"), vec!["app", "apple", "apply"]);
        assert_eq!(tree.prefix_search("ban"), vec!["banana"]);
        assert_eq!(tree.prefix_search(""), vec!["app", "apple", "apply", "banana"]);
        assert!(tree.prefix_search("z").is_empty());
        assert_eq!(tree.len(), 4);
        assert!(!tree.is_empty());
    }

    #[test]
    fn trie_does_not_answer_a_fuzzy_question() {
        // the card rejects the trie for a fuzzy match: it matches prefixes, not approximations
        let tree = trie(&[("apple", "1"), ("apply", "2")]);
        // "appl" is a prefix of both keys but it is not a key
        assert_eq!(tree.get("appl"), None);
        assert!(!tree.contains_key("appl"));
        // the prefix query is the only fuzzy question the trie answers, and it answers it exactly
        assert_eq!(tree.prefix_search("appl"), vec!["apple", "apply"]);
        // the empty key is a key like any other
        let with_empty = trie(&[("", "root"), ("a", "1")]);
        assert_eq!(with_empty.get(""), Some("root"));
        assert_eq!(with_empty.prefix_search(""), vec!["", "a"]);
        // inserting the same key twice keeps one entry and takes the last value
        let mut tree = trie(&[("a", "1")]);
        tree.insert("a", "2");
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get("a"), Some("2"));
    }

    // ---- algorithm.string.aho_corasick

    #[test]
    fn aho_corasick_reports_every_overlapping_match() {
        let automaton = aho_corasick(&["he", "she", "his", "hers"]).unwrap();
        let matches = automaton.search("ushers").unwrap();
        assert_eq!(
            matches,
            vec![
                AhoMatch { pattern: 1, start: 1, end: 4 },
                AhoMatch { pattern: 0, start: 2, end: 4 },
                AhoMatch { pattern: 3, start: 2, end: 6 },
            ]
        );
        // the overlapping matches are reported too
        let automaton = aho_corasick(&["aa"]).unwrap();
        assert_eq!(
            automaton.search("aaa").unwrap(),
            vec![
                AhoMatch { pattern: 0, start: 0, end: 2 },
                AhoMatch { pattern: 0, start: 1, end: 3 },
            ]
        );
        // the answer is the answer of the naive search, pattern by pattern
        let patterns = ["ab", "b", "abc"];
        let automaton = aho_corasick(&patterns).unwrap();
        let text = "abcab";
        let mut expected: Vec<AhoMatch> = Vec::new();
        for (index, pattern) in patterns.iter().enumerate() {
            for start in naive_matches(text.as_bytes(), pattern.as_bytes()) {
                expected.push(AhoMatch {
                    pattern: index,
                    start,
                    end: start + pattern.len(),
                });
            }
        }
        expected.sort_unstable();
        assert_eq!(automaton.search(text).unwrap(), expected);
    }

    #[test]
    fn aho_corasick_rejects_the_empty_pattern_that_the_card_excludes() {
        // the empty pattern matches at every position: the card asks for it to be rejected
        let error = aho_corasick(&["ab", ""]).unwrap_err();
        assert!(error.contains("empty"), "the reason must be readable: {error}");
        // the empty set of patterns is legal and reports nothing
        let automaton = aho_corasick(&[]).unwrap();
        assert_eq!(automaton.pattern_count(), 0);
        assert!(automaton.search("abc").unwrap().is_empty());
        // the empty text is legal too
        let automaton = aho_corasick(&["ab"]).unwrap();
        assert!(automaton.search("").unwrap().is_empty());
        // the guard of the card: a scan that reports too many matches fails instead of going on
        let automaton = AhoCorasick::with_max_matches(&["a"], 2).unwrap();
        let error = automaton.search("aaaa").unwrap_err();
        assert!(error.contains("more than"), "the reason must be readable: {error}");
    }

    // ---- algorithm.string.rabin_karp

    #[test]
    fn rabin_karp_finds_the_same_matches_as_the_naive_search() {
        let text = b"abababc";
        assert_eq!(rabin_karp(text, b"abab", 256, 1_000_003).unwrap(), vec![0, 2]);
        assert_eq!(rabin_karp(text, b"a", 256, 1_000_003).unwrap(), vec![0, 2, 4]);
        assert!(rabin_karp(text, b"abcabc", 256, 1_000_003).unwrap().is_empty());
        assert!(rabin_karp(b"", b"a", 256, 1_000_003).unwrap().is_empty());
        // the answer is the answer of the naive search
        for (text, pattern) in [
            ("abracadabra", "abra"),
            ("aaaa", "aa"),
            ("the quick brown fox", "own"),
            ("abc", "abc"),
            ("abc", "d"),
        ] {
            assert_eq!(
                rabin_karp(text.as_bytes(), pattern.as_bytes(), 31, 1_000_003).unwrap(),
                naive_matches(text.as_bytes(), pattern.as_bytes())
            );
        }
    }

    #[test]
    fn rabin_karp_verifies_the_candidates_after_a_collision() {
        // the modulus 2 makes every hash of the same length collide: the second comparison is what
        // keeps the answer right
        let text = b"abcabcabc";
        assert_eq!(rabin_karp(text, b"abc", 2, 2).unwrap(), vec![0, 3, 6]);
        assert_eq!(rabin_karp(text, b"bca", 2, 2).unwrap(), vec![1, 4]);
        assert!(rabin_karp(text, b"xyz", 2, 2).unwrap().is_empty());
        // the configuration that cannot hash is rejected
        assert!(rabin_karp(text, b"abc", 2, 1).is_err());
        assert!(rabin_karp(text, b"abc", 1, 1_000_003).is_err());
        // the empty pattern is rejected too
        let error = rabin_karp(text, b"", 2, 2).unwrap_err();
        assert!(error.contains("empty"), "the reason must be readable: {error}");
    }

    // ---- algorithm.string.edit_distance

    #[test]
    fn edit_distance_counts_the_minimum_edits() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("abc", "ab"), 1);
        // the distance is symmetric
        assert_eq!(edit_distance("sitting", "kitten"), 3);
        // the answer is the answer of the recursion
        for left in ["", "a", "ab", "abc", "flaw"] {
            for right in ["", "a", "ba", "abc", "lawn"] {
                let left_chars: Vec<char> = left.chars().collect();
                let right_chars: Vec<char> = right.chars().collect();
                assert_eq!(
                    edit_distance(left, right),
                    naive_edit_distance(&left_chars, &right_chars)
                );
            }
        }
    }

    #[test]
    fn edit_distance_measures_characters_and_not_bytes() {
        // the unit is the character: one substitution, not two byte substitutions
        assert_eq!(edit_distance("\u{e9}", "e"), 1);
        // a letter with a combining mark is two characters here, and the standard library has no
        // segmenter that would make it one: "e" + the combining acute is two characters, the
        // precomposed letter is one
        assert_eq!(edit_distance("e\u{301}", "\u{e9}"), 2);
        assert_eq!(edit_distance("e\u{301}", "e\u{301}"), 0);
        // the same rule for the non-ASCII text
        assert_eq!(edit_distance("\u{65e5}\u{672c}\u{8a9e}", "\u{65e5}\u{672c}"), 1);
        // the counterexample of the card: when only the equality is needed, a comparison is enough
        assert_eq!(edit_distance("same", "same"), 0);
        assert_ne!(edit_distance("same", "sane"), 0);
    }
}
