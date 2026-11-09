//! The five searching algorithms of the catalogue (the assets `algorithm.searching.*`).
//!
//! Every public function of this module names its asset in its doc comment:
//! `* asset: `algorithm.\<category\>.\<name\>``.
//!
//! | function | asset | what it does |
//! |---|---|---|
//! | [`binary_search`] | `algorithm.searching.binary_search` | the position of a key in an ordered slice, `O(log n)` |
//! | [`lower_bound`] | `algorithm.searching.lower_bound` | the first position that is not smaller than a key |
//! | [`upper_bound`] | `algorithm.searching.upper_bound` | the first position that is greater than a key |
//! | [`two_pointers`] | `algorithm.searching.two_pointers` | a pair that adds up to a target, one linear scan |
//! | [`sliding_window`] | `algorithm.searching.sliding_window` | the longest window whose state is valid |
//!
//! # The rules of this module
//!
//! * **The preconditions are checked.** The card of every function here says that the input has to be ordered.
//!   A call on an unordered slice would answer with an index that means nothing, so the implementations check the
//!   order first and report [`SearchingError::NotSorted`] instead of quietly answering wrongly.
//! * **Deterministic.** The same input always gives the same output. Where the algorithm allows several correct
//!   answers (which of several equal elements is reported) the choice is fixed and written down in the doc comment
//!   of the function, so that a test can pin it.
//! * **No panic on a valid input.** An empty slice is a valid input and gives an empty result. An input the
//!   algorithm cannot handle (an unordered slice, a window size of zero, a sum that overflows) is reported as an
//!   `Err` with a readable reason.
//! * **The standard library only.** The crate has no dependency at all, and every dependency would need a licence
//!   review (design §1.2).

use std::error::Error;
use std::fmt;

/// The failures the searching assets of this module can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchingError {
    /// The slice is not ordered, so the result of the search would mean nothing.
    NotSorted {
        /// The position of the first element that breaks the order.
        index: usize,
    },
    /// The window size is zero, so no window can be built.
    InvalidWindowSize {
        /// The window size that was asked for.
        size: usize,
    },
    /// The sum of two elements does not fit into the type of the elements.
    SumOverflow,
}

impl fmt::Display for SearchingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchingError::NotSorted { index } => write!(
                f,
                "the slice is not ordered: the element at the position {index} is smaller than the one before it"
            ),
            SearchingError::InvalidWindowSize { size } => {
                write!(f, "the window size {size} is not usable: a window has to hold at least one element")
            }
            SearchingError::SumOverflow => write!(f, "the sum of two elements does not fit into i64"),
        }
    }
}

impl Error for SearchingError {}

/// Checks that the slice is ordered and reports the first position that breaks the order.
///
/// The check costs `O(n)`. It is the price of not answering a question about an unordered slice with an index
/// that means nothing.
fn ensure_sorted<T: Ord>(items: &[T]) -> Result<(), SearchingError> {
    for index in 1..items.len() {
        if items[index - 1] > items[index] {
            return Err(SearchingError::NotSorted { index });
        }
    }
    Ok(())
}

/// Binary search: the position of a key in an ordered slice.
///
/// The candidate range is halved at every step, so the key is found in `O(log n)` comparisons. The function
/// reports the **leftmost** position of an element that is equal to the key.
///
/// * asset: `algorithm.searching.binary_search`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.searching.binary_search/card.md`
///
/// # Input preconditions (from the card)
///
/// * `precondition: sorted_input` — the slice has to be ordered. The function checks it and reports
///   [`SearchingError::NotSorted`] when it is not, instead of answering with an index that means nothing.
/// * `result: index_or_presence` — the result says whether the key is there and where it is.
/// * The elements must be comparable (`T: Ord`).
///
/// # Complexity (from the card)
///
/// * Time `O(log n)` for the search itself, `O(n)` for the check of the order that precedes it, so a single call
///   costs `O(n)` in total.
/// * Extra space `O(1)`: the search keeps no buffer.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The input is unordered*: "输入无序时不能直接二分——除非先排序（但排序代价更高）" — the function reports
///   [`SearchingError::NotSorted`]; sorting first is usually not worth it.
/// * *`O(1)` lookups are needed and the data fits in memory*: "需要 O(1) 查找且内存够——哈希表更合适".
/// * *The data changes often*: "数据频繁变动——每次变动都要维护有序结构".
/// * *Only an approximate answer is needed*: "只需要小范围近似查找——布隆过滤器/位图更省".
///
/// # Determinism
///
/// The answer does not depend on anything but the input. Between several equal elements the leftmost one is
/// reported; use [`lower_bound`] and [`upper_bound`] to get the whole range of the equal elements.
pub fn binary_search<T: Ord>(items: &[T], target: &T) -> Result<Option<usize>, SearchingError> {
    ensure_sorted(items)?;
    // The first element that is not smaller than the target: if it is equal, it is the leftmost match.
    let index = lower_bound_of(items, target);
    if index < items.len() && items[index] == *target {
        Ok(Some(index))
    } else {
        Ok(None)
    }
}

/// Lower bound: the first position of the slice whose element is not smaller than the key.
///
/// The position is also the place where the key would be inserted without breaking the order. When every element
/// is smaller than the key the result is the length of the slice.
///
/// * asset: `algorithm.searching.lower_bound`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.searching.lower_bound/card.md`
///
/// # Input preconditions (from the card)
///
/// * `precondition: sorted_input` — the slice has to be ordered. The function checks it and reports
///   [`SearchingError::NotSorted`] when it is not.
/// * `result: insertion_index` — the result is a position, and it is always inside `0..=items.len()`.
/// * The elements must be comparable (`T: Ord`).
///
/// # Complexity (from the card)
///
/// * Time `O(log n)` for the search itself, `O(n)` for the check of the order that precedes it, so a single call
///   costs `O(n)` in total.
/// * Extra space `O(1)`.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *Only the presence of the key matters*: "只需要判断存在与否时——直接用 binary_search 更省事".
/// * *The last element that is not greater than the key is needed*: "需要的是"最后一个不大于"时——应改用
///   upper_bound 配合减一".
/// * *The slice is unordered*: "序列无序——前提不满足" — the function reports [`SearchingError::NotSorted`].
/// * *The predicate is not monotone*: "谓词非单调（例如"偶数位置"）——二分的前提失效".
///
/// # Determinism
///
/// The leftmost position is reported, so the result of two calls with the same input is the same.
pub fn lower_bound<T: Ord>(items: &[T], target: &T) -> Result<usize, SearchingError> {
    ensure_sorted(items)?;
    Ok(lower_bound_of(items, target))
}

/// The first position whose element is not smaller than the target, on a slice that is known to be ordered.
fn lower_bound_of<T: Ord>(items: &[T], target: &T) -> usize {
    let mut lo = 0usize;
    let mut hi = items.len();
    while lo < hi {
        // The middle is computed as lo + (hi - lo) / 2 so that lo + hi cannot overflow.
        let mid = lo + (hi - lo) / 2;
        if items[mid] < *target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Upper bound: the first position of the slice whose element is greater than the key.
///
/// The position closes the range of the elements that are equal to the key: the range
/// `[lower_bound(key), upper_bound(key))` holds exactly the elements that are equal to the key.
///
/// * asset: `algorithm.searching.upper_bound`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.searching.upper_bound/card.md`
///
/// # Input preconditions (from the card)
///
/// * `precondition: sorted_input` — the slice has to be ordered. The function checks it and reports
///   [`SearchingError::NotSorted`] when it is not.
/// * `result: index_after_all_equal_elements` — the result is a position, and it is always inside
///   `0..=items.len()`.
/// * The elements must be comparable (`T: Ord`).
///
/// # Complexity (from the card)
///
/// * Time `O(log n)` for the search itself, `O(n)` for the check of the order that precedes it, so a single call
///   costs `O(n)` in total.
/// * Extra space `O(1)`.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The slice is unordered*: "数据无序时不能使用" — the function reports [`SearchingError::NotSorted`].
/// * *Only the last element that is equal to the key is needed*: "只需要"最后一个等于 x 的位置"时——用 upper_bound
///   再减一即可".
/// * *Only the presence of the key matters*: "只需要存在性——用 binary_search".
/// * *The slice is small*: "数据量小——线性扫描即可".
///
/// # Determinism
///
/// The rightmost position is reported, so the result of two calls with the same input is the same. The caller
/// has to handle the boundary where the result is the length of the slice.
pub fn upper_bound<T: Ord>(items: &[T], target: &T) -> Result<usize, SearchingError> {
    ensure_sorted(items)?;
    let mut lo = 0usize;
    let mut hi = items.len();
    while lo < hi {
        // The middle is computed as lo + (hi - lo) / 2 so that lo + hi cannot overflow.
        let mid = lo + (hi - lo) / 2;
        if items[mid] <= *target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

/// Two pointers: a pair of positions whose values add up to the target, found in one scan.
///
/// One pointer starts at the front of the slice and one at the back. A sum that is too small moves the front
/// pointer to the right, a sum that is too big moves the back pointer to the left, so each pointer walks the slice
/// at most once.
///
/// * asset: `algorithm.searching.two_pointers`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.searching.two_pointers/card.md`
///
/// # Input preconditions (from the card)
///
/// * `precondition: sorted_for_two_sum` — the slice has to be ordered. The function checks it and reports
///   [`SearchingError::NotSorted`] when it is not.
/// * `movement: monotonic` — both pointers only move towards each other, which is what makes the scan linear and
///   what makes it safe to skip a whole side of the range.
/// * The elements are `i64`, so the sum of two of them cannot overflow; a sum that does not fit is reported as
///   [`SearchingError::SumOverflow`].
///
/// # Complexity (from the card)
///
/// * Time `O(n)`: each pointer walks the slice at most once, plus `O(n)` for the check of the order.
/// * Extra space `O(1)`: no auxiliary structure.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The slice is unordered*: "数组无序时——对撞指针的正确性前提不成立" — the function reports
///   [`SearchingError::NotSorted`]; sorting it first would lose the original positions anyway.
/// * *Several pairs are needed and there are many of them*: "需要返回所有配对且数量多——哈希表或排序后枚举更方便".
/// * *The data arrives as a stream and cannot be looked at twice*: "数据流式到达且无法回看——双指针需要随机访问".
/// * *The movement is not monotone*: "指针移动不具单调性——移动策略必须能被证明不漏解".
///
/// # Determinism
///
/// The pair with the smallest first position is reported. The order of the two positions is always the smaller one
/// first.
pub fn two_pointers(items: &[i64], target: i64) -> Result<Option<(usize, usize)>, SearchingError> {
    ensure_sorted(items)?;
    if items.is_empty() {
        return Ok(None);
    }
    // `hi` is the last element of the range, so the two positions are always different ones.
    let mut lo = 0usize;
    let mut hi = items.len() - 1;
    while lo < hi {
        let sum = items[lo]
            .checked_add(items[hi])
            .ok_or(SearchingError::SumOverflow)?;
        match sum.cmp(&target) {
            std::cmp::Ordering::Less => lo += 1,
            std::cmp::Ordering::Greater => hi -= 1,
            std::cmp::Ordering::Equal => return Ok(Some((lo, hi))),
        }
    }
    Ok(None)
}

/// Sliding window: the longest window of consecutive elements whose state is valid.
///
/// A window is moved from left to right over the slice. An element that enters the window updates the state, an
/// element that leaves it updates the state back, so every element is added once and removed once. The state is
/// kept by the caller, which is what makes the scan linear: the caller decides what "valid" means and how the
/// state is maintained.
///
/// * asset: `algorithm.searching.sliding_window`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.searching.sliding_window/card.md`
///
/// # Input preconditions (from the card)
///
/// * `precondition: contiguous_interval` — the problem has to be about consecutive elements.
/// * `window_size: fixed_or_variable` — `max_window_size` is the largest window the caller accepts; the window
///   that is returned can be shorter.
/// * The state has to be maintainable by adding and removing elements: `enter` adds an element to the state and
///   `leave` takes it out again, and `is_valid` reads the state. The card asks for the state to be updated this
///   way ("窗口状态可以用"进入时加、离开时减"的方式增量维护").
///
/// # Complexity (from the card)
///
/// * Time `O(n)` amortised: every element enters the window once and leaves it once, and `is_valid` is `O(1)`.
/// * Extra space `O(k)`, the size of the window, held by the state of the caller.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The problem is about arbitrary subsets and not about consecutive elements*: "区间不连续（需要任意子集）时——窗口
///   模型不适用".
/// * *The state cannot be maintained incrementally*: "窗口状态无法增量维护（如区间内最值不可加减）——改用单调队列
///   或其它结构".
/// * *The length of the interval is bound by many conditions at once*: "区间长度受多种复杂约束——单调性难以证明".
/// * *There are few intervals and the input is small*: "数据量小且区间数少——暴力更简单".
///
/// # Determinism
///
/// The leftmost of the longest valid windows is reported, as the pair of its first and last position. The
/// callbacks are called in a fixed order: `enter` for every element that arrives, then `leave` for every element
/// that leaves, then `is_valid`.
pub fn sliding_window<T, S, E, L, P>(
    items: &[T],
    max_window_size: usize,
    state: &mut S,
    mut enter: E,
    mut leave: L,
    mut is_valid: P,
) -> Result<Option<(usize, usize)>, SearchingError>
where
    E: FnMut(&mut S, &T),
    L: FnMut(&mut S, &T),
    P: FnMut(&S) -> bool,
{
    if max_window_size == 0 {
        // A window of zero elements has no meaning: the card asks for the extreme cases to be safe.
        return Err(SearchingError::InvalidWindowSize { size: max_window_size });
    }
    if items.is_empty() {
        return Ok(None);
    }
    let mut best: Option<(usize, usize)> = None;
    let mut left = 0usize;
    for (right, item) in items.iter().enumerate() {
        enter(state, item);
        // The window shrinks from the left until it is small enough and valid again.
        while left <= right && (right - left + 1 > max_window_size || !is_valid(state)) {
            leave(state, &items[left]);
            left += 1;
        }
        if is_valid(state) {
            let length = right - left + 1;
            let better = match best {
                Some((best_left, best_right)) => length > best_right - best_left + 1,
                None => true,
            };
            if better {
                best = Some((left, right));
            }
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number of times every value of the slice appears, used as the state of the sliding window.
    #[derive(Default)]
    struct Counts {
        /// How often every value appears in the window.
        counts: std::collections::HashMap<i32, usize>,
    }

    impl Counts {
        /// The number of different values in the window.
        fn distinct(&self) -> usize {
            self.counts.len()
        }
    }

    /// Adds a value to the state of the window.
    fn enter(state: &mut Counts, value: &i32) {
        *state.counts.entry(*value).or_insert(0) += 1;
    }

    /// Takes a value out of the state of the window.
    fn leave(state: &mut Counts, value: &i32) {
        let count = state.counts.get_mut(value).expect("the value is in the window");
        *count -= 1;
        if *count == 0 {
            state.counts.remove(value);
        }
    }

    /// The window is valid when it holds at most two different values.
    fn at_most_two(state: &Counts) -> bool {
        state.distinct() <= 2
    }

    #[test]
    fn binary_search_finds_a_key_and_reports_a_missing_one() {
        let data = vec![1, 3, 3, 3, 5, 7, 9];

        // The card asks for: first element, last element, missing, below everything, above everything.
        assert_eq!(binary_search(&data, &1), Ok(Some(0)));
        assert_eq!(binary_search(&data, &9), Ok(Some(6)));
        assert_eq!(binary_search(&data, &3), Ok(Some(1)), "the leftmost match is reported");
        assert_eq!(binary_search(&data, &4), Ok(None));
        assert_eq!(binary_search(&data, &0), Ok(None));
        assert_eq!(binary_search(&data, &100), Ok(None));

        // The empty slice and the single element.
        let empty: Vec<i32> = Vec::new();
        assert_eq!(binary_search(&empty, &1), Ok(None));
        let single = vec![42];
        assert_eq!(binary_search(&single, &42), Ok(Some(0)));
        assert_eq!(binary_search(&single, &41), Ok(None));
    }

    #[test]
    fn binary_search_rejects_an_unordered_slice() {
        // The counterexample of the card: an unordered input cannot be searched with a binary search.
        let data = vec![5, 1, 9];
        assert_eq!(
            binary_search(&data, &1),
            Err(SearchingError::NotSorted { index: 1 })
        );
    }

    #[test]
    fn lower_bound_returns_the_insertion_point() {
        let data = vec![1, 3, 3, 3, 5, 7];

        // Below everything, above everything, between two elements, on an equal element.
        assert_eq!(lower_bound(&data, &0), Ok(0));
        assert_eq!(lower_bound(&data, &8), Ok(6));
        assert_eq!(lower_bound(&data, &4), Ok(4));
        assert_eq!(lower_bound(&data, &3), Ok(1), "the leftmost position of the equal elements");

        // The empty slice and the single element.
        let empty: Vec<i32> = Vec::new();
        assert_eq!(lower_bound(&empty, &1), Ok(0));
        let single = vec![42];
        assert_eq!(lower_bound(&single, &42), Ok(0));
        assert_eq!(lower_bound(&single, &43), Ok(1));
    }

    #[test]
    fn lower_bound_rejects_an_unordered_slice() {
        // The counterexample of the card: the sequence has to be ordered.
        let data = vec![5, 1, 9];
        assert_eq!(lower_bound(&data, &1), Err(SearchingError::NotSorted { index: 1 }));
    }

    #[test]
    fn upper_bound_closes_the_range_of_the_equal_elements() {
        let data = vec![1, 3, 3, 3, 5, 7];

        assert_eq!(upper_bound(&data, &0), Ok(0));
        assert_eq!(upper_bound(&data, &8), Ok(6));
        assert_eq!(upper_bound(&data, &3), Ok(4), "the right end of the equal elements");
        assert_eq!(upper_bound(&data, &7), Ok(6));

        // The empty slice and the slice where every element is equal.
        let empty: Vec<i32> = Vec::new();
        assert_eq!(upper_bound(&empty, &1), Ok(0));
        let equal = vec![2, 2, 2];
        assert_eq!(upper_bound(&equal, &2), Ok(3));

        // The card asks for the pair of the two bounds.
        let lower = lower_bound(&data, &3).expect("the slice is ordered");
        let upper = upper_bound(&data, &3).expect("the slice is ordered");
        assert!(data[lower..upper].iter().all(|v| *v == 3));
    }

    #[test]
    fn upper_bound_rejects_an_unordered_slice() {
        // The counterexample of the card: the data has to be ordered.
        let data = vec![5, 1, 9];
        assert_eq!(upper_bound(&data, &1), Err(SearchingError::NotSorted { index: 1 }));
    }

    #[test]
    fn two_pointers_finds_the_pair_that_adds_up_to_the_target() {
        let data = vec![-3, 1, 4, 6, 9, 12];

        // 1 + 9 and 9 + 12.
        assert_eq!(two_pointers(&data, 10), Ok(Some((1, 4))));
        assert_eq!(two_pointers(&data, 21), Ok(Some((4, 5))));
        // No pair at all.
        assert_eq!(two_pointers(&data, 100), Ok(None));
        assert_eq!(two_pointers(&data, 2), Ok(None));

        // Several pairs: the card asks for the first one of the convention.
        let several = vec![1, 2, 3, 4, 5];
        assert_eq!(two_pointers(&several, 6), Ok(Some((0, 4))));

        // The empty slice and the single element.
        let empty: Vec<i64> = Vec::new();
        assert_eq!(two_pointers(&empty, 0), Ok(None));
        let single = vec![5i64];
        assert_eq!(two_pointers(&single, 5), Ok(None));

        // The sum of two elements does not fit into an i64: the card asks for an error and not a panic.
        let huge = vec![1i64, i64::MAX];
        assert_eq!(two_pointers(&huge, i64::MAX), Err(SearchingError::SumOverflow));
    }

    #[test]
    fn two_pointers_rejects_an_unordered_slice() {
        // The counterexample of the card: the colliding pointers need an ordered slice.
        let data = vec![5, 1, 9];
        assert_eq!(two_pointers(&data, 6), Err(SearchingError::NotSorted { index: 1 }));
    }

    #[test]
    fn sliding_window_finds_the_longest_window_with_the_fewest_distinct_values() {
        let data = vec![1, 2, 1, 3, 2, 2, 4];
        let mut state = Counts::default();
        let found = sliding_window(&data, 4, &mut state, enter, leave, at_most_two)
            .expect("the window size is usable");
        // The longest window with at most two different values is [1, 2, 1] at the positions 0 to 2.
        assert_eq!(found, Some((0, 2)));

        // The empty slice gives no window at all.
        let empty: Vec<i32> = Vec::new();
        let mut state = Counts::default();
        let found = sliding_window(&empty, 3, &mut state, enter, leave, at_most_two)
            .expect("the window size is usable");
        assert_eq!(found, None);
    }

    #[test]
    fn sliding_window_rejects_a_window_size_of_zero() {
        // The extreme case of the card: the size has to be checked.
        let data = vec![1, 2, 3];
        let mut state = Counts::default();
        let result = sliding_window(&data, 0, &mut state, enter, leave, at_most_two);
        assert_eq!(result, Err(SearchingError::InvalidWindowSize { size: 0 }));
    }
}
