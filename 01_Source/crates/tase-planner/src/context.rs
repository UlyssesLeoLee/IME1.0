//! The trimming of the context of the model (detailed design §9.1).
//!
//! The context of a model call is the list of the files and of the symbols the model may read. The list is long and
//! the budget is short, so the planner has to choose what goes into the call and has to say why the rest was left
//! out.
//!
//! # The rules
//!
//! * the items are sorted by **priority, descending**; the items of the same priority are sorted by **path,
//!   ascending**, and by symbol when the path is the same. The order is therefore total and the selection is
//!   deterministic: the same list and the same budget always give the same selection;
//! * an item is kept when the item budget is not full and when its bytes still fit in the byte budget;
//! * an item that does not fit is **dropped and the walk continues**: a small item of a lower priority may still fit,
//!   so the selection is a packing and not a prefix of the sorted list. The kept items keep the sorted order;
//! * the report of the selection says how many items were kept and dropped and why the first one was dropped.
//!
//! # What this layer does not do
//!
//! This layer never reads a file and never holds the text of a file: an item is a path, an optional symbol, a size in
//! bytes and a priority. The body of a file is collected by the caller (the semantic layer), never by the planner.

/// One item of the context of a model call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextItem {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// The symbol the item carries, when it carries one.
    pub symbol: Option<String>,
    /// How many bytes the item weighs.
    pub bytes: usize,
    /// The priority of the item; the bigger the number, the earlier the item is taken.
    pub priority: u32,
}

/// The budget of the context of a model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBudget {
    /// How many bytes the context may weigh.
    pub max_bytes: usize,
    /// How many items the context may hold.
    pub max_items: usize,
}

impl Default for ContextBudget {
    /// The default of the design book: `planner.max_context_chars` is 24000 characters; the budget of the planner
    /// counts bytes, so the default is 24000 bytes.
    fn default() -> Self {
        ContextBudget {
            max_bytes: 24_000,
            max_items: 128,
        }
    }
}

/// What the selection dropped and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trim {
    /// How many items were kept.
    pub kept: usize,
    /// How many items were dropped.
    pub dropped: usize,
    /// How many bytes the kept items weigh.
    pub bytes_kept: usize,
    /// Why the items were dropped, in one readable line.
    pub reason: String,
}

/// A selection and its report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The items that were kept, in the sorted order.
    pub items: Vec<ContextItem>,
    /// The report of the selection.
    pub trim: Trim,
}

/// The items that fit into the budget.
///
/// The order of the result is the sorted order (priority descending, path ascending); the report of the selection is
/// available with [`select_with_report`].
pub fn select(items: &[ContextItem], budget: &ContextBudget) -> Vec<ContextItem> {
    select_with_report(items, budget).items
}

/// The items that fit into the budget, with the report of what was dropped.
pub fn select_with_report(items: &[ContextItem], budget: &ContextBudget) -> Selection {
    let mut ordered: Vec<&ContextItem> = items.iter().collect();
    ordered.sort_by(|left, right| compare(left, right));

    let mut kept: Vec<ContextItem> = Vec::new();
    let mut bytes_kept: usize = 0;
    let mut dropped: usize = 0;
    let mut first_reason: Option<String> = None;

    for item in ordered {
        if kept.len() >= budget.max_items {
            dropped += 1;
            if first_reason.is_none() {
                first_reason = Some(format!(
                    "the item limit of {} was reached: {} was dropped",
                    budget.max_items, item.path
                ));
            }
            continue;
        }
        if bytes_kept.saturating_add(item.bytes) > budget.max_bytes {
            dropped += 1;
            if first_reason.is_none() {
                first_reason = Some(format!(
                    "the byte budget of {} does not hold the {} bytes of {}",
                    budget.max_bytes, item.bytes, item.path
                ));
            }
            continue;
        }
        bytes_kept = bytes_kept.saturating_add(item.bytes);
        kept.push(item.clone());
    }

    let reason = match (dropped, first_reason) {
        (0, _) => format!(
            "every item fits: {} items and {} bytes were kept",
            kept.len(),
            bytes_kept
        ),
        (_, Some(reason)) => format!(
            "{} of {} items were dropped: {}",
            dropped,
            items.len(),
            reason
        ),
        (_, None) => format!("{} of {} items were dropped", dropped, items.len()),
    };

    let kept_count = kept.len();
    Selection {
        items: kept,
        trim: Trim {
            kept: kept_count,
            dropped,
            bytes_kept,
            reason,
        },
    }
}

/// A summary of the context for the model.
///
/// The summary holds the path, the optional symbol, the size and the priority of every item; it never holds the text
/// of a file, because this layer never collects it. The items are sorted the way the selection sorts them, so the
/// summary of the same list is always the same string.
pub fn summarise(items: &[ContextItem]) -> String {
    let mut ordered: Vec<&ContextItem> = items.iter().collect();
    ordered.sort_by(|left, right| compare(left, right));

    let bytes: usize = ordered.iter().map(|item| item.bytes).sum();
    let mut out = String::new();
    out.push_str(&format!(
        "context: {} item(s), {} byte(s)\n",
        ordered.len(),
        bytes
    ));
    for item in ordered {
        out.push_str(&format!("- {}", item.path));
        if let Some(symbol) = &item.symbol {
            out.push_str(&format!(" [symbol: {symbol}]"));
        }
        out.push_str(&format!(
            ": {} byte(s), priority {}\n",
            item.bytes, item.priority
        ));
    }
    out
}

/// The order of the items: the priority descending, then the path ascending, then the symbol ascending.
///
/// The symbol of an item that carries none is the empty string, so it comes first.
fn compare(left: &ContextItem, right: &ContextItem) -> std::cmp::Ordering {
    right
        .priority
        .cmp(&left.priority)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| {
            left.symbol
                .as_deref()
                .unwrap_or("")
                .cmp(right.symbol.as_deref().unwrap_or(""))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An item with a path, a size and a priority.
    fn item(path: &str, bytes: usize, priority: u32) -> ContextItem {
        ContextItem {
            path: path.to_string(),
            symbol: None,
            bytes,
            priority,
        }
    }

    /// A budget of 100 bytes and 10 items.
    fn budget() -> ContextBudget {
        ContextBudget {
            max_bytes: 100,
            max_items: 10,
        }
    }

    #[test]
    fn the_items_are_taken_by_priority_descending() {
        let items = vec![
            item("a.rs", 10, 1),
            item("b.rs", 10, 9),
            item("c.rs", 10, 5),
        ];
        let kept = select(&items, &budget());
        let paths: Vec<&str> = kept.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["b.rs", "c.rs", "a.rs"]);
    }

    #[test]
    fn the_items_of_the_same_priority_are_taken_by_path_ascending() {
        let items = vec![
            item("z.rs", 10, 7),
            item("a.rs", 10, 7),
            item("m.rs", 10, 7),
        ];
        let kept = select(&items, &budget());
        let paths: Vec<&str> = kept.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["a.rs", "m.rs", "z.rs"]);
    }

    #[test]
    fn the_selection_is_the_same_when_the_input_is_shuffled() {
        let items = vec![
            item("a.rs", 10, 3),
            item("b.rs", 10, 8),
            item("c.rs", 10, 5),
            item("d.rs", 10, 8),
        ];
        let first = select(&items, &budget());
        let mut shuffled = items.clone();
        shuffled.reverse();
        let second = select(&shuffled, &budget());
        assert_eq!(first, second);
    }

    #[test]
    fn the_byte_budget_is_never_crossed() {
        let items = vec![item("a.rs", 60, 3), item("b.rs", 60, 2)];
        let kept = select(&items, &budget());
        let bytes: usize = kept.iter().map(|i| i.bytes).sum();
        assert!(bytes <= 100, "the budget must hold: {bytes}");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn an_item_that_does_not_fit_is_dropped_and_the_walk_continues() {
        // 90 bytes are kept, the 20 bytes of the second item do not fit in the 10 that are left.
        let items = vec![item("big.rs", 90, 9), item("small.rs", 20, 1)];
        let selection = select_with_report(&items, &budget());
        let paths: Vec<&str> = selection.items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            paths,
            ["big.rs"],
            "the second item does not fit in the rest"
        );
        assert_eq!(selection.trim.kept, 1);
        assert_eq!(selection.trim.dropped, 1);
        assert_eq!(selection.trim.bytes_kept, 90);
    }

    #[test]
    fn an_item_that_fills_the_budget_exactly_is_kept() {
        let items = vec![item("big.rs", 90, 9), item("small.rs", 10, 1)];
        let selection = select_with_report(&items, &budget());
        let paths: Vec<&str> = selection.items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["big.rs", "small.rs"]);
        assert_eq!(selection.trim.bytes_kept, 100);
        assert_eq!(selection.trim.dropped, 0);
    }

    #[test]
    fn a_small_item_of_a_lower_priority_is_kept_when_it_fits() {
        let items = vec![item("big.rs", 90, 9), item("small.rs", 5, 1)];
        let selection = select_with_report(&items, &budget());
        let paths: Vec<&str> = selection.items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["big.rs", "small.rs"]);
        assert_eq!(selection.trim.kept, 2);
        assert_eq!(selection.trim.dropped, 0);
        assert_eq!(selection.trim.bytes_kept, 95);
    }

    #[test]
    fn the_item_budget_is_never_crossed() {
        let items = vec![item("a.rs", 1, 9), item("b.rs", 1, 8), item("c.rs", 1, 7)];
        let small = ContextBudget {
            max_bytes: 100,
            max_items: 2,
        };
        let selection = select_with_report(&items, &small);
        assert_eq!(selection.items.len(), 2);
        assert_eq!(selection.trim.kept, 2);
        assert_eq!(selection.trim.dropped, 1);
        assert!(
            selection.trim.reason.contains("item limit"),
            "{}",
            selection.trim.reason
        );
    }

    #[test]
    fn the_reason_says_what_was_dropped_and_why() {
        let items = vec![item("a.rs", 200, 9)];
        let selection = select_with_report(&items, &budget());
        assert_eq!(selection.trim.kept, 0);
        assert_eq!(selection.trim.dropped, 1);
        assert_eq!(selection.trim.bytes_kept, 0);
        assert!(
            selection.trim.reason.contains("byte budget"),
            "{}",
            selection.trim.reason
        );
    }

    #[test]
    fn a_selection_that_drops_nothing_says_so() {
        let items = vec![item("a.rs", 10, 9)];
        let selection = select_with_report(&items, &budget());
        assert_eq!(selection.trim.dropped, 0);
        assert!(
            selection.trim.reason.contains("every item fits"),
            "{}",
            selection.trim.reason
        );
    }

    #[test]
    fn an_empty_list_gives_an_empty_selection() {
        let selection = select_with_report(&[], &budget());
        assert!(selection.items.is_empty());
        assert_eq!(selection.trim.kept, 0);
        assert_eq!(selection.trim.dropped, 0);
        assert_eq!(selection.trim.bytes_kept, 0);
    }

    #[test]
    fn a_budget_of_zero_keeps_nothing() {
        let items = vec![item("a.rs", 10, 9)];
        let empty = ContextBudget {
            max_bytes: 0,
            max_items: 10,
        };
        let selection = select_with_report(&items, &empty);
        assert!(selection.items.is_empty());
        assert_eq!(selection.trim.dropped, 1);
    }

    #[test]
    fn the_summary_holds_the_path_the_symbol_the_size_and_the_priority() {
        let items = vec![ContextItem {
            path: "src/api/user.rs".to_string(),
            symbol: Some("create_user".to_string()),
            bytes: 400,
            priority: 10,
        }];
        let summary = summarise(&items);
        assert!(summary.contains("src/api/user.rs"), "{summary}");
        assert!(summary.contains("create_user"), "{summary}");
        assert!(summary.contains("400"), "{summary}");
        assert!(summary.contains("10"), "{summary}");
        assert!(summary.contains("1 item(s)"), "{summary}");
    }

    #[test]
    fn the_summary_of_an_item_without_a_symbol_has_no_bracket() {
        let summary = summarise(&[item("src/main.rs", 10, 1)]);
        assert!(!summary.contains("symbol"), "{summary}");
    }

    #[test]
    fn the_summary_is_the_same_when_the_input_is_shuffled() {
        let items = vec![item("a.rs", 10, 3), item("b.rs", 20, 8)];
        let mut shuffled = items.clone();
        shuffled.reverse();
        assert_eq!(summarise(&items), summarise(&shuffled));
    }

    #[test]
    fn the_summary_of_an_empty_list_says_so() {
        let summary = summarise(&[]);
        assert!(summary.contains("0 item(s)"), "{summary}");
        assert!(summary.contains("0 byte(s)"), "{summary}");
    }
}
