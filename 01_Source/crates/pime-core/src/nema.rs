//! The rules of .NEMA v1.2: the headers of the domains, the domains they open, and the address of the
//! interpreter that reads them.
//!
//! A .NEMA document is a sequence of **domains**. Every domain begins with a **header**, and the header is a
//! line of its own:
//!
//! ```text
//! \tTtext1 1
//! What the text says.
//! \t> Gauth1 1
//! What the graph says.
//! ```
//!
//! The header is a **real tab** (U+0009), the id of the domain, **one space** and the number of the revision
//! (a positive integer). A graph domain puts a `>` between the tab and the id. Nothing may follow the
//! revision: a line that looks like a header **is** a header, so that a boundary is never lost when the
//! document has no empty line between two domains.
//!
//! The **scope** of a domain runs from the line after its header to the line before the header of the next
//! domain (to the end of the text when there is no next one). A writer that stays inside the scope of its
//! own domain cannot step on the text of another one.
//!
//! The rules are pure: the client, the server and the tests read a document the same way.

/// The address of the .NEMA interpreter, where the syntax rules live.
///
/// The demonstration writes this line in front of its domains when the text it writes into has no domain
/// header of its own: it tells the reader where the rules are. It is a note for the reader, not a header.
pub const FALLBACK_URL: &str = "https://nema-thread.vercel.app/website/download.html";

/// The character that opens a domain header: a real tab (U+0009), never a space.
pub const HEADER_TAB: char = '\t';

/// One domain of a .NEMA document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Domain {
    /// The id of the domain (the part between the tab and the space).
    pub id: String,
    /// The number of the revision of the domain (a positive integer).
    pub revision: u32,
    /// The offset of the first byte **after** the header of the domain.
    pub start: usize,
    /// The offset of the first byte of the header of the next domain (the end of the text when the domain is
    /// the last one).
    pub end: usize,
}

impl Domain {
    /// Whether the offset is inside the scope of the domain.
    ///
    /// The end of the scope belongs to it as well: the caret that waits at the end of the text is in the
    /// domain that is open there, and what is written there is written in that domain.
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset <= self.end
    }
}

/// The id and the revision of a domain header, if the line is one.
fn parse_header(line: &str) -> Option<(String, u32)> {
    // exactly one tab at the start of the line: a second one is not a header any more
    let rest = line.strip_prefix(HEADER_TAB)?;
    // a graph domain puts a `>` between the tab and the id
    let rest = rest.strip_prefix('>').unwrap_or(rest);
    // the id, one space, and the revision: nothing else may follow
    let (id, revision) = rest.split_once(' ')?;
    if id.is_empty() || !id.bytes().all(is_id_byte) {
        return None;
    }
    if revision.is_empty() || !revision.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // the revision is a positive integer
    let revision: u32 = revision.parse().ok()?;
    if revision == 0 {
        return None;
    }
    Some((id.to_string(), revision))
}

/// Whether the byte may be part of the id of a domain.
fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
}

/// Whether the line is the header of a domain.
///
/// The line is a header when it is a tab, an optional `>`, the id of the domain, one space and the number
/// of the revision, and nothing else.
pub fn is_domain_header(line: &str) -> bool {
    parse_header(line).is_some()
}

/// Whether the text holds a domain header.
///
/// The text of a document that is not a .NEMA document (or the empty text) holds none.
pub fn has_domain_header(text: &str) -> bool {
    text.lines().any(is_domain_header)
}

/// Every domain of the text, in the order they come.
///
/// The scope of a domain runs from the line after its header to the line before the header of the next one.
pub fn domains(text: &str) -> Vec<Domain> {
    // the headers of the text: where the line of the header begins, and where the domain begins
    let mut headers: Vec<(usize, String, u32, usize)> = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        // a document of .NEMA has no CR, but a text that comes from a host may have one
        let content = content.strip_suffix('\r').unwrap_or(content);
        let line_start = offset;
        offset += line.len();
        if let Some((id, revision)) = parse_header(content) {
            headers.push((line_start, id, revision, offset));
        }
    }
    let mut out = Vec::with_capacity(headers.len());
    for (index, (_, id, revision, start)) in headers.iter().enumerate() {
        // the scope of a domain ends where the header of the next one begins
        let end = headers
            .get(index + 1)
            .map(|(line_start, ..)| *line_start)
            .unwrap_or(text.len());
        out.push(Domain {
            id: id.clone(),
            revision: *revision,
            start: *start,
            end,
        });
    }
    out
}

/// The domain the offset is in.
///
/// The offset of a character of a domain is in that domain. The header of a domain is in no domain: it is
/// the boundary between the one before it and the one it opens.
pub fn domain_at(text: &str, offset: usize) -> Option<Domain> {
    let domains = domains(text);
    let offset = offset.min(text.len());
    for domain in &domains {
        if offset >= domain.start && offset < domain.end {
            return Some(domain.clone());
        }
    }
    // the caret after the last character of the text is in the domain that is open there
    if offset == text.len() {
        return domains.last().cloned();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the header of a domain: a tab, the id, a space and the revision.
    #[test]
    fn a_tab_an_id_a_space_and_a_revision_are_a_header() {
        assert!(is_domain_header("\tTtext1 1"));
        assert!(is_domain_header("\tA01 1"));
        assert!(is_domain_header("\tA01 42"));
    }

    /// The header of a graph domain puts a `>` between the tab and the id.
    ///
    /// The strict shape of the rules is `Tab + > + id + space + revision`: the `>` touches the id, and a
    /// space between them is not a header (the shape of the rules is `^\t>?ID REV$`).
    #[test]
    fn a_graph_domain_has_a_mark_after_the_tab() {
        assert!(is_domain_header("\t>Gauth1 1"));
        assert!(is_domain_header("\t>Gauth1 12"));
        assert!(!is_domain_header("\t> Gauth1 1"), "the mark touches the id");
    }

    /// The tab is a real tab: a space in its place is not a header.
    #[test]
    fn a_space_instead_of_the_tab_is_not_a_header() {
        assert!(!is_domain_header(" Ttext1 1"));
        assert!(!is_domain_header("  Ttext1 1"));
        assert!(!is_domain_header(""));
    }

    /// Exactly one tab opens the header: a second one belongs to the id, and the id has no tab in it.
    #[test]
    fn two_tabs_are_not_a_header() {
        assert!(!is_domain_header("\t\tTtext1 1"));
        assert!(!is_domain_header("\tA01\t1"));
    }

    /// The revision is a positive integer: no zero, no sign, no letter, no space.
    #[test]
    fn the_revision_is_a_positive_integer() {
        assert!(!is_domain_header("\tTtext1 0"));
        assert!(!is_domain_header("\tTtext1 -1"));
        assert!(!is_domain_header("\tTtext1 +1"));
        assert!(!is_domain_header("\tTtext1 x"));
        assert!(!is_domain_header("\tTtext1 1a"));
        assert!(!is_domain_header("\tTtext1 1 2"));
        assert!(!is_domain_header("\tTtext1 1 "));
        assert!(!is_domain_header("\tTtext1 "));
        assert!(!is_domain_header("\tTtext1"));
    }

    /// Nothing may follow the revision.
    #[test]
    fn nothing_may_follow_the_revision() {
        assert!(!is_domain_header("\tTtext1 1 extra"));
        assert!(!is_domain_header("\tTtext1 1\t"));
    }

    /// The text of a domain is at the left of the line: it is not a header, even when it looks like one.
    #[test]
    fn a_line_at_the_left_of_the_margin_is_not_a_header() {
        assert!(!is_domain_header("Ttext1 1"));
        assert!(!is_domain_header("agent01_runs_nema_ok"));
        assert!(!is_domain_header("What the text says."));
    }

    /// A text that holds a header is a .NEMA document; an empty text is not.
    #[test]
    fn a_text_holds_a_header_only_when_it_has_one() {
        assert!(has_domain_header("\tTtext1 1\nWhat the text says.\n"));
        assert!(has_domain_header("What the text says.\n\tTtext1 1\n"));
        assert!(!has_domain_header(""));
        assert!(!has_domain_header("What the text says.\n"));
    }

    /// Two domains follow one another without an empty line between them, and every one of them keeps its
    /// own text.
    #[test]
    fn two_domains_follow_one_another_without_an_empty_line() {
        let text = "\tTtext1 1\nfirst\n\t>Gauth1 1\nsecond\n";
        let domains = domains(text);
        assert_eq!(domains.len(), 2);
        assert_eq!(domains[0].id, "Ttext1");
        assert_eq!(domains[0].revision, 1);
        assert_eq!(domains[1].id, "Gauth1");
        assert_eq!(domains[1].revision, 1);
        // the scope of the first one ends where the header of the second one begins
        assert_eq!(domains[0].end, text.find("\t>Gauth1 1").unwrap());
        assert_eq!(domains[1].start, text.find("second").unwrap());
        assert_eq!(domains[1].end, text.len());
    }

    /// The scope of the last domain runs to the end of the text.
    #[test]
    fn the_last_domain_runs_to_the_end_of_the_text() {
        let text = "\tTtext1 1\nfirst\n";
        let domains = domains(text);
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].start, "\tTtext1 1\n".len());
        assert_eq!(domains[0].end, text.len());
    }

    /// The text of a domain is in the domain, and the header of a domain is in no domain at all.
    #[test]
    fn the_text_of_a_domain_is_in_that_domain() {
        let text = "\tTtext1 1\nfirst\n\t>Gauth1 1\nsecond\n";
        let first = domain_at(text, text.find("first").unwrap()).expect("the text of the first domain");
        assert_eq!(first.id, "Ttext1");
        let second = domain_at(text, text.find("second").unwrap()).expect("the text of the second domain");
        assert_eq!(second.id, "Gauth1");
        // the header of the second domain is the boundary: it belongs to no domain
        assert!(domain_at(text, text.find("\t>Gauth1 1").unwrap()).is_none());
    }

    /// The caret at the end of the text is in the domain that is open there.
    #[test]
    fn the_caret_at_the_end_of_the_text_is_in_the_last_domain() {
        let text = "\tTtext1 1\nfirst\n";
        let domain = domain_at(text, text.len()).expect("the caret waits at the end of the text");
        assert_eq!(domain.id, "Ttext1");
    }

    /// A text that is not a .NEMA document holds no domain.
    #[test]
    fn a_text_without_a_header_holds_no_domain() {
        assert!(domains("What the text says.\n").is_empty());
        assert!(domains("").is_empty());
        assert!(domain_at("What the text says.\n", 0).is_none());
    }
}
