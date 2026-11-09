//! The six dynamic programming algorithms of the catalogue (the assets
//! `algorithm.dp.*`).
//!
//! Every public function of this module names its asset in its doc comment with the
//! two marker lines `* asset:` and `* card:`; the registration script reads those
//! markers to wire the code to the catalogue.
//!
//! # The shape of the answers
//!
//! Three of the six algorithms answer with more than a number, because the plan that
//! reaches the optimum is worth more than the optimum itself:
//!
//! * [`longest_common_subsequence`] returns the common sequence and not only its
//!   length.
//! * [`interval_dp`] returns the merges of the optimal plan.
//! * [`bitmask_dp`] returns the tour that reaches the optimum.
//!
//! [`knapsack_01`] returns the selected items, [`coin_change`] returns the coins of
//! the solution and [`longest_increasing_subsequence`] returns the indices of the
//! subsequence, so that every algorithm of this module can be pinned on a plan and
//! not only on a value.
//!
//! # The errors
//!
//! A valid input never panics. An input the algorithm cannot handle - a negative
//! capacity, a negative denomination, a table that would not fit in memory - is
//! reported as an [`Err`] that says why.

use std::fmt;

/// The error every algorithm of this module reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpError {
    /// The capacity of the knapsack was negative.
    NegativeCapacity {
        /// The capacity that was given.
        capacity: i64,
    },
    /// An item had a negative weight.
    NegativeWeight {
        /// The index of the item in the input.
        index: usize,
        /// The weight of the item.
        weight: i64,
    },
    /// The amount to reach was negative.
    NegativeAmount {
        /// The amount that was given.
        amount: i64,
    },
    /// A denomination was negative.
    NegativeDenomination {
        /// The denomination that was given.
        denomination: i64,
    },
    /// The amount cannot be reached with the given denominations.
    UnreachableAmount {
        /// The amount that was given.
        amount: i64,
    },
    /// No tour visits every node of the distance matrix.
    NoTour {
        /// The number of nodes of the matrix.
        nodes: usize,
    },
    /// The input was empty where the algorithm needs at least one element.
    EmptyInput,
    /// The distance matrix is not square.
    InvalidDimensions {
        /// The number of rows the matrix has.
        expected: usize,
        /// The number of columns a row has.
        found: usize,
    },
    /// The table of the algorithm would not fit in memory.
    ProblemTooLarge {
        /// The number of cells the table would have.
        cells: usize,
        /// The largest number of cells this implementation accepts.
        limit: usize,
    },
    /// The set of elements is too large for a table of `2^n` entries.
    TooManyElements {
        /// The number of elements that were given.
        count: usize,
        /// The largest number of elements this implementation accepts.
        limit: usize,
    },
    /// A cost did not fit into an `i64`.
    ArithmeticOverflow,
}

impl fmt::Display for DpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeCapacity { capacity } => {
                write!(f, "the capacity {capacity} is negative")
            }
            Self::NegativeWeight { index, weight } => {
                write!(f, "the item {index} has the negative weight {weight}")
            }
            Self::NegativeAmount { amount } => {
                write!(f, "the amount {amount} is negative")
            }
            Self::NegativeDenomination { denomination } => {
                write!(f, "the denomination {denomination} is negative")
            }
            Self::UnreachableAmount { amount } => {
                write!(f, "the amount {amount} cannot be reached with these denominations")
            }
            Self::NoTour { nodes } => write!(
                f,
                "no tour visits every one of the {nodes} nodes of the distance matrix"
            ),
            Self::EmptyInput => write!(f, "the input is empty"),
            Self::InvalidDimensions { expected, found } => write!(
                f,
                "the distance matrix is not square: {expected} rows and {found} columns"
            ),
            Self::ProblemTooLarge { cells, limit } => write!(
                f,
                "the table would have {cells} cells, and this implementation accepts at most {limit}"
            ),
            Self::TooManyElements { count, limit } => write!(
                f,
                "there are {count} elements, and this implementation accepts at most {limit}"
            ),
            Self::ArithmeticOverflow => write!(f, "the cost did not fit into an i64"),
        }
    }
}

impl std::error::Error for DpError {}

/// An item of the 0-1 knapsack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    /// The weight of the item.
    pub weight: i64,
    /// The value of the item.
    pub value: i64,
}

/// The answer of the 0-1 knapsack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnapsackSolution {
    /// The total value of the selected items.
    pub value: i64,
    /// The total weight of the selected items.
    pub weight: i64,
    /// The indices of the selected items in the input, in ascending order.
    pub items: Vec<usize>,
}

/// The answer of the longest increasing subsequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LisResult {
    /// The length of the longest strictly increasing subsequence.
    pub length: usize,
    /// The indices of the elements of that subsequence, in the order they appear in
    /// the input.
    pub indices: Vec<usize>,
}

/// The answer of the longest common subsequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LcsResult<T> {
    /// The length of the longest common subsequence.
    pub length: usize,
    /// The common subsequence itself.
    pub sequence: Vec<T>,
}

/// The answer of the coin change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinChangeResult {
    /// The smallest number of coins that adds up to the amount.
    pub count: usize,
    /// The coins of that solution, in the order in which they complete the amount,
    /// from the smallest partial amount to the target.
    pub coins: Vec<i64>,
}

/// The answer of the interval dynamic programming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalSolution {
    /// The optimal cost of the whole interval.
    pub cost: i64,
    /// The merges of the optimal plan, in postorder: the children are merged before
    /// their parent. The total cost is the sum of the costs of the merged intervals.
    pub merges: Vec<(usize, usize)>,
}

/// The answer of the bitmask dynamic programming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitmaskSolution {
    /// The length of the shortest tour.
    pub cost: i64,
    /// The tour: the nodes in the order they are visited, the node `0` first and
    /// last.
    pub tour: Vec<usize>,
}

/// The largest number of cells the table of the 0-1 knapsack may have.
const MAX_KNAPSACK_CELLS: usize = 16 * 1024 * 1024;

/// The largest number of cells the table of the longest common subsequence may have.
const MAX_LCS_CELLS: usize = 8 * 1024 * 1024;

/// The largest amount the table of the coin change may have.
const MAX_COIN_AMOUNT: usize = 2_000_000;

/// The largest number of nodes the table of the bitmask dynamic programming may
/// have: the table has `2^n` entries, so `n = 20` is already a million of them.
const MAX_BITMASK_NODES: usize = 20;

/// Selects the items that fit in the capacity and maximize the total value.
///
/// The dynamic program keeps the best value for every capacity and walks the
/// capacity downwards, so that every item is used at most once.
///
/// # Constraints (from the card)
///
/// * Every item may be used **at most once**: an item that could be taken again is
///   the unbounded variant, that is the complete knapsack.
/// * The capacity has to be a **not negative integer**: a negative capacity is
///   refused with [`DpError::NegativeCapacity`].
/// * The weights have to be **not negative**: a negative weight is refused with
///   [`DpError::NegativeWeight`]. A zero weight is allowed and the item is then
///   taken for free.
/// * The capacity may not be huge: the algorithm is pseudo-polynomial, it costs
///   `O(n * W)`; a table that would not fit in memory is refused with
///   [`DpError::ProblemTooLarge`].
///
/// # Complexity (from the card)
///
/// Time `O(n * W)`, extra space `O(W)` for the values. The table of the decisions
/// that reports the selected items costs `O(n * W)` bytes on top of it, which is the
/// price of the plan: the rolling array of the card cannot report the items.
///
/// # When it does not apply (from the card)
///
/// * Every item may be taken again - the complete knapsack is the right algorithm.
/// * The capacity is not an integer or is very large - the pseudo-polynomial cost is
///   not acceptable.
/// * An approximation is enough - the greedy of the fractional knapsack is cheaper.
/// * There are few items and many other constraints - the problem has to be modelled
///   again.
///
/// # Determinism
///
/// When two selections have the same value, the one that keeps the earlier items is
/// returned: an item is only taken when it improves the value strictly.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::dp::{knapsack_01, Item};
///
/// let items = [
///     Item { weight: 2, value: 3 },
///     Item { weight: 3, value: 4 },
///     Item { weight: 4, value: 5 },
/// ];
///
/// let solution = knapsack_01(&items, 5).unwrap();
/// assert_eq!(solution.value, 7);
/// assert_eq!(solution.weight, 5);
/// assert_eq!(solution.items, vec![0, 1]);
/// ```
///
/// * asset: `algorithm.dp.knapsack_01`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.dp.knapsack_01/card.md`
pub fn knapsack_01(items: &[Item], capacity: i64) -> Result<KnapsackSolution, DpError> {
    if capacity < 0 {
        return Err(DpError::NegativeCapacity { capacity });
    }
    for (index, item) in items.iter().enumerate() {
        if item.weight < 0 {
            return Err(DpError::NegativeWeight {
                index,
                weight: item.weight,
            });
        }
    }

    let capacity = usize::try_from(capacity)
        .map_err(|_| DpError::ProblemTooLarge { cells: 0, limit: MAX_KNAPSACK_CELLS })?;
    let row_width = capacity + 1;
    let cells = items.len().max(1).saturating_mul(row_width);
    if cells > MAX_KNAPSACK_CELLS {
        return Err(DpError::ProblemTooLarge {
            cells,
            limit: MAX_KNAPSACK_CELLS,
        });
    }

    // The best value that the items seen so far give for every capacity.
    let mut best = vec![0i64; row_width];
    // Whether the item `index` is part of an optimal solution of the items
    // `0..=index` for the capacity of the column.
    let mut taken = vec![false; cells];

    for (index, item) in items.iter().enumerate() {
        let weight = item.weight as usize;
        let row = index * row_width;
        // The capacity is walked downwards, so that an item is used at most once.
        for current in (weight..row_width).rev() {
            let without = best[current];
            let with = best[current - weight]
                .checked_add(item.value)
                .ok_or(DpError::ArithmeticOverflow)?;
            if with > without {
                best[current] = with;
                taken[row + current] = true;
            }
        }
    }

    let mut selected = Vec::new();
    let mut remaining = capacity;
    let mut value = 0i64;
    let mut weight = 0i64;
    for index in (0..items.len()).rev() {
        if taken[index * row_width + remaining] {
            selected.push(index);
            remaining -= items[index].weight as usize;
            value = value
                .checked_add(items[index].value)
                .ok_or(DpError::ArithmeticOverflow)?;
            weight = weight
                .checked_add(items[index].weight)
                .ok_or(DpError::ArithmeticOverflow)?;
        }
    }
    selected.reverse();

    Ok(KnapsackSolution {
        value,
        weight,
        items: selected,
    })
}

/// Finds the longest strictly increasing subsequence of a sequence and returns its
/// length and the indices of its elements.
///
/// The algorithm keeps the smallest possible tail of every length and finds the
/// place of a new value with a binary search, so it costs `O(n log n)` and not the
/// `O(n^2)` of the plain dynamic program.
///
/// # Constraints (from the card)
///
/// * The subsequence is **strictly increasing**: a value equal to the previous one
///   does not extend it. The non-decreasing variant would use an upper bound instead
///   of a lower bound; this implementation always uses the strict one.
/// * The subsequence is **not necessarily contiguous**: the elements are taken in the
///   order of the input, with gaps allowed.
/// * The elements are compared with their [`Ord`] implementation, so the function
///   works on any ordered type.
///
/// # Complexity (from the card)
///
/// Time `O(n log n)`, extra space `O(n)` for the tails and the predecessors.
///
/// # When it does not apply (from the card)
///
/// * The elements have to be contiguous - the longest increasing subarray is found
///   by a linear scan.
/// * The data is small - the `O(n^2)` dynamic program is more obvious.
/// * Every longest subsequence has to be enumerated - the tails do not enumerate them.
/// * The non-decreasing variant is needed - this implementation is strictly
///   increasing.
///
/// # Determinism
///
/// The first tail that is not smaller than the new value is replaced, so the same
/// sequence always gives the same subsequence.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::dp::longest_increasing_subsequence;
///
/// let result = longest_increasing_subsequence(&[3, 1, 2]);
/// assert_eq!(result.length, 2);
/// assert_eq!(result.indices, vec![1, 2]);
/// ```
///
/// * asset: `algorithm.dp.longest_increasing_subsequence`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.dp.longest_increasing_subsequence/card.md`
pub fn longest_increasing_subsequence<T: Ord>(sequence: &[T]) -> LisResult {
    // The smallest tail of an increasing subsequence of every length.
    let mut tails: Vec<&T> = Vec::with_capacity(sequence.len());
    let mut tail_indices: Vec<usize> = Vec::with_capacity(sequence.len());
    let mut predecessor: Vec<Option<usize>> = vec![None; sequence.len()];

    for (index, value) in sequence.iter().enumerate() {
        // The first tail that is not smaller than the value: a strictly increasing
        // subsequence does not take an equal value.
        let position = tails.partition_point(|tail| *tail < value);
        if position > 0 {
            predecessor[index] = Some(tail_indices[position - 1]);
        }
        if position == tails.len() {
            tails.push(value);
            tail_indices.push(index);
        } else {
            tails[position] = value;
            tail_indices[position] = index;
        }
    }

    let mut indices = Vec::new();
    let mut current = tail_indices.last().copied();
    while let Some(index) = current {
        indices.push(index);
        current = predecessor[index];
    }
    indices.reverse();

    LisResult {
        length: indices.len(),
        indices,
    }
}

/// Finds the longest common subsequence of two sequences and returns its length and
/// the subsequence itself.
///
/// The dynamic program keeps the length of the common subsequence of every prefix
/// pair; the table is then walked backwards to rebuild the subsequence.
///
/// # Constraints (from the card)
///
/// * The two sequences are compared element by element with [`PartialEq`]; the
///   alphabet may be any type.
/// * The subsequence is **not necessarily contiguous**.
/// * The two sequences may not be very long: the table has one cell per pair of
///   positions, so a table that would not fit in memory is refused with
///   [`DpError::ProblemTooLarge`].
///
/// # Complexity (from the card)
///
/// Time `O(n * m)`, extra space `O(n * m)` for the table that is needed to rebuild
/// the subsequence; the rolling array of the card would cost `O(min(n, m))` but
/// cannot report the subsequence.
///
/// # When it does not apply (from the card)
///
/// * Only the question "is this a subsequence of that one?" has to be answered - two
///   pointers answer it in `O(n + m)`.
/// * The sequences are very long - the `O(n * m)` cost is too high.
/// * The alphabet is large and the similarity is what matters - the edit distance
///   measures that better.
/// * Many pairs of sequences have to be compared - a cheap filter on the length or
///   on a hash comes first.
///
/// # Determinism
///
/// When the two options have the same length, the element of the first sequence is
/// skipped; the same pair of sequences therefore always gives the same subsequence.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::dp::longest_common_subsequence;
///
/// let first = ['a', 'b', 'c', 'd'];
/// let second = ['a', 'b', 'd'];
///
/// let result = longest_common_subsequence(&first, &second).unwrap();
/// assert_eq!(result.length, 3);
/// assert_eq!(result.sequence, vec!['a', 'b', 'd']);
/// ```
///
/// * asset: `algorithm.dp.longest_common_subsequence`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.dp.longest_common_subsequence/card.md`
pub fn longest_common_subsequence<T: Clone + PartialEq>(
    first: &[T],
    second: &[T],
) -> Result<LcsResult<T>, DpError> {
    let rows = first.len() + 1;
    let columns = second.len() + 1;
    let cells = rows.saturating_mul(columns);
    if cells > MAX_LCS_CELLS {
        return Err(DpError::ProblemTooLarge {
            cells,
            limit: MAX_LCS_CELLS,
        });
    }

    let mut table = vec![vec![0u32; columns]; rows];
    for (row, left) in first.iter().enumerate() {
        for (column, right) in second.iter().enumerate() {
            if left == right {
                table[row + 1][column + 1] = table[row][column] + 1;
            } else {
                table[row + 1][column + 1] = table[row][column + 1].max(table[row + 1][column]);
            }
        }
    }

    let mut sequence = Vec::new();
    let mut row = first.len();
    let mut column = second.len();
    while row > 0 && column > 0 {
        if first[row - 1] == second[column - 1] {
            sequence.push(first[row - 1].clone());
            row -= 1;
            column -= 1;
        } else if table[row - 1][column] >= table[row][column - 1] {
            // The two options have the same length: the element of the first
            // sequence is skipped.
            row -= 1;
        } else {
            column -= 1;
        }
    }
    sequence.reverse();

    Ok(LcsResult {
        length: table[first.len()][second.len()] as usize,
        sequence,
    })
}

/// Finds the smallest number of coins that adds up to an amount, and the coins of
/// that solution.
///
/// The dynamic program walks the amount from the small one to the large one and
/// tries every denomination at every amount.
///
/// # Constraints (from the card)
///
/// * Every denomination may be used **as often as wanted**: a denomination that may
///   be used once only is the 0-1 knapsack.
/// * The amount has to be a **not negative integer**: a negative amount is refused
///   with [`DpError::NegativeAmount`].
/// * The denominations have to be **not negative**: a negative denomination is
///   refused with [`DpError::NegativeDenomination`]. A denomination of zero is
///   dropped, because it would add nothing and the loop would never end.
/// * The amount may not be huge: the algorithm costs `O(amount * coins)` and keeps a
///   table of `O(amount)`; an amount that is too large is refused with
///   [`DpError::ProblemTooLarge`].
///
/// # Complexity (from the card)
///
/// Time `O(amount * coins)`, extra space `O(amount)`.
///
/// # When it does not apply (from the card)
///
/// * Every denomination may be used once only - the 0-1 knapsack is the right
///   algorithm.
/// * The amount is very large - the table does not fit in memory; a number theoretic
///   method or a scaling by the greatest common divisor is needed.
/// * The denominations are real numbers - the integer dynamic program does not
///   apply.
///
/// # Determinism
///
/// The denominations are sorted and de-duplicated and the smallest denomination that
/// reaches the optimum is preferred, so the same input always gives the same coins.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::dp::coin_change;
///
/// let result = coin_change(&[1, 3, 4], 6).unwrap();
/// assert_eq!(result.count, 2);
/// assert_eq!(result.coins, vec![3, 3]);
/// ```
///
/// * asset: `algorithm.dp.coin_change`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.dp.coin_change/card.md`
pub fn coin_change(coins: &[i64], amount: i64) -> Result<CoinChangeResult, DpError> {
    if amount < 0 {
        return Err(DpError::NegativeAmount { amount });
    }

    let mut denominations = Vec::with_capacity(coins.len());
    for &coin in coins {
        if coin < 0 {
            return Err(DpError::NegativeDenomination { denomination: coin });
        }
        if coin > 0 {
            denominations.push(coin);
        }
    }
    denominations.sort_unstable();
    denominations.dedup();

    let target = usize::try_from(amount).map_err(|_| DpError::ProblemTooLarge {
        cells: 0,
        limit: MAX_COIN_AMOUNT,
    })?;
    if target > MAX_COIN_AMOUNT {
        return Err(DpError::ProblemTooLarge {
            cells: target,
            limit: MAX_COIN_AMOUNT,
        });
    }

    let unreachable = u32::MAX;
    let mut best = vec![unreachable; target + 1];
    let mut chosen = vec![u32::MAX; target + 1];
    best[0] = 0;

    for value in 1..=target {
        for (index, &coin) in denominations.iter().enumerate() {
            let coin = coin as usize;
            if coin > value {
                continue;
            }
            let previous = best[value - coin];
            if previous == unreachable {
                continue;
            }
            if previous + 1 < best[value] {
                best[value] = previous + 1;
                chosen[value] = index as u32;
            }
        }
    }

    if best[target] == unreachable {
        return Err(DpError::UnreachableAmount { amount });
    }

    let mut used = Vec::new();
    let mut remaining = target;
    while remaining > 0 {
        let index = chosen[remaining];
        if index == u32::MAX {
            return Err(DpError::UnreachableAmount { amount });
        }
        let Some(&coin) = denominations.get(index as usize) else {
            return Err(DpError::UnreachableAmount { amount });
        };
        let coin_size = coin as usize;
        if coin_size == 0 || coin_size > remaining {
            return Err(DpError::UnreachableAmount { amount });
        }
        used.push(coin);
        remaining -= coin_size;
    }
    used.reverse();

    Ok(CoinChangeResult {
        count: best[target] as usize,
        coins: used,
    })
}

/// Splits an interval into pieces as cheaply as possible and returns the cost and
/// the merges of the optimal plan.
///
/// The cost of an interval is given by the caller: `cost(start, end)` is the price
/// of merging the interval `start..=end` into one piece. The dynamic program walks
/// the intervals by their length, from the short ones to the long ones, and tries
/// every split point of every interval.
///
/// # Constraints (from the card)
///
/// * The problem has to have an **interval structure**: the cost of an interval is
///   what is paid when it is merged, and the last merge of an interval splits it in
///   two.
/// * The algorithm makes **no assumption on the cost**: the cost may be negative and
///   it does not have to satisfy the triangle inequality.
/// * The number of pieces has to be small: the algorithm costs `O(n^3)` time and
///   `O(n^2)` space, so it is meant for the few hundred pieces at most.
///
/// # Complexity (from the card)
///
/// Time `O(n^3)`, extra space `O(n^2)` for the costs and the split points.
///
/// # When it does not apply (from the card)
///
/// * The problem has no interval structure - the interval dynamic programming does
///   not apply.
/// * A greedy or a linear solution exists - it is simpler and cheaper.
/// * There are many pieces - the `O(n^3)` cost is not acceptable.
/// * The way of splitting is not unique and all the splittings have to be listed -
///   the state would explode.
///
/// # Determinism
///
/// The split points are tried in the order of their index and a split is only
/// replaced when the new cost is strictly smaller, so the same input always gives
/// the same plan.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::dp::interval_dp;
///
/// let dimensions = [10i64, 30, 5, 60];
/// let cost = |start: usize, end: usize| dimensions[start] * dimensions[end + 1];
///
/// let solution = interval_dp(3, cost).unwrap();
/// assert_eq!(solution.cost, 650);
/// assert_eq!(solution.merges, vec![(0, 1), (0, 2)]);
/// ```
///
/// * asset: `algorithm.dp.interval_dp`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.dp.interval_dp/card.md`
pub fn interval_dp<F>(length: usize, cost: F) -> Result<IntervalSolution, DpError>
where
    F: Fn(usize, usize) -> i64,
{
    if length == 0 {
        return Ok(IntervalSolution {
            cost: 0,
            merges: Vec::new(),
        });
    }

    let mut best = vec![vec![0i64; length]; length];
    let mut split = vec![vec![0usize; length]; length];

    for span in 2..=length {
        for start in 0..=(length - span) {
            let end = start + span - 1;
            let span_cost = cost(start, end);
            let mut best_cost = i64::MAX;
            let mut best_split = start;
            let mut found = false;
            for middle in start..end {
                let candidate = best[start][middle]
                    .checked_add(best[middle + 1][end])
                    .and_then(|value| value.checked_add(span_cost))
                    .ok_or(DpError::ArithmeticOverflow)?;
                if !found || candidate < best_cost {
                    best_cost = candidate;
                    best_split = middle;
                    found = true;
                }
            }
            best[start][end] = best_cost;
            split[start][end] = best_split;
        }
    }

    // The plan is walked back, the children before their parent.
    enum Step {
        Visit(usize, usize),
        Emit(usize, usize),
    }
    let mut merges = Vec::new();
    let mut stack = vec![Step::Visit(0, length - 1)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Visit(start, end) => {
                if start == end {
                    continue;
                }
                let middle = split[start][end];
                stack.push(Step::Emit(start, end));
                stack.push(Step::Visit(middle + 1, end));
                stack.push(Step::Visit(start, middle));
            }
            Step::Emit(start, end) => merges.push((start, end)),
        }
    }

    Ok(IntervalSolution {
        cost: best[0][length - 1],
        merges,
    })
}

/// Finds the shortest tour that visits every node of a distance matrix exactly once
/// and returns its length and the tour itself.
///
/// The dynamic program keeps, for every set of visited nodes, the cost of the
/// shortest path that visits exactly that set and ends on every node of the set; the
/// set is held in the bits of an integer, so the number of states is `2^n` and not
/// `n!`.
///
/// The distance `i64::MAX` means that there is no edge between the two nodes.
///
/// # Constraints (from the card)
///
/// * The number of nodes has to be **small**: the table has `2^n` entries, so this
///   implementation accepts up to 20 nodes and refuses more with
///   [`DpError::TooManyElements`].
/// * The tour starts and ends on the node `0`.
/// * The matrix has to be square; a matrix that is not is refused with
///   [`DpError::InvalidDimensions`].
///
/// # Complexity (from the card)
///
/// Time `O(2^n * n^2)`, extra space `O(2^n * n)` for the table of the paths; the
/// card counts the states and not the end point, which is the same order up to the
/// factor `n`.
///
/// # When it does not apply (from the card)
///
/// * There are many nodes - the table of `2^n` entries does not fit in memory.
/// * A greedy or a polynomial algorithm exists - the subset dynamic programming is
///   the heavy but sure exact method.
/// * The state is not a set - an ordinary dynamic program is enough.
/// * The same table has to answer many different starting sets - the table has to be
///   built again for each of them.
///
/// # Determinism
///
/// The masks are walked in the order of their index and a cost is only replaced when
/// the new one is strictly smaller, so the same matrix always gives the same tour.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::dp::bitmask_dp;
///
/// let distances = vec![
///     vec![0, 2, 100, 20],
///     vec![20, 0, 3, 100],
///     vec![1, 20, 0, 4],
///     vec![5, 1, 20, 0],
/// ];
///
/// let solution = bitmask_dp(&distances).unwrap();
/// assert_eq!(solution.cost, 14);
/// assert_eq!(solution.tour, vec![0, 1, 2, 3, 0]);
/// ```
///
/// * asset: `algorithm.dp.bitmask_dp`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.dp.bitmask_dp/card.md`
pub fn bitmask_dp(distances: &[Vec<i64>]) -> Result<BitmaskSolution, DpError> {
    let node_count = distances.len();
    if node_count == 0 {
        return Ok(BitmaskSolution {
            cost: 0,
            tour: Vec::new(),
        });
    }
    for row in distances {
        if row.len() != node_count {
            return Err(DpError::InvalidDimensions {
                expected: node_count,
                found: row.len(),
            });
        }
    }
    if node_count > MAX_BITMASK_NODES {
        return Err(DpError::TooManyElements {
            count: node_count,
            limit: MAX_BITMASK_NODES,
        });
    }
    if node_count == 1 {
        return Ok(BitmaskSolution {
            cost: 0,
            tour: vec![0],
        });
    }

    // Every reachable mask contains the node 0, so the masks are numbered by
    // `mask >> 1` and the table is half as large as the full one.
    let size = 1usize << node_count;
    let slots = (size >> 1) * node_count;
    let unreachable = i64::MAX;
    let mut best = vec![unreachable; slots];
    let mut predecessor = vec![u8::MAX; slots];
    // The path that starts on the node 0 and visits nothing else.
    best[0] = 0;

    for mask in 1..size {
        if mask & 1 == 0 {
            continue;
        }
        let base = (mask >> 1) * node_count;
        for last in 0..node_count {
            if mask & (1usize << last) == 0 {
                continue;
            }
            let current = best[base + last];
            if current == unreachable {
                continue;
            }
            for (next, &edge) in distances[last].iter().enumerate() {
                let bit = 1usize << next;
                if mask & bit != 0 {
                    continue;
                }
                if edge == unreachable {
                    continue;
                }
                let candidate = current
                    .checked_add(edge)
                    .ok_or(DpError::ArithmeticOverflow)?;
                let slot = ((mask | bit) >> 1) * node_count + next;
                if candidate < best[slot] {
                    best[slot] = candidate;
                    predecessor[slot] = last as u8;
                }
            }
        }
    }

    let full = size - 1;
    let base = (full >> 1) * node_count;
    let mut shortest = unreachable;
    let mut shortest_last = usize::MAX;
    for last in 1..node_count {
        let current = best[base + last];
        if current == unreachable {
            continue;
        }
        let closing = distances[last][0];
        if closing == unreachable {
            continue;
        }
        let total = current
            .checked_add(closing)
            .ok_or(DpError::ArithmeticOverflow)?;
        if total < shortest {
            shortest = total;
            shortest_last = last;
        }
    }
    if shortest_last == usize::MAX {
        return Err(DpError::NoTour { nodes: node_count });
    }

    let mut chain = Vec::new();
    let mut mask = full;
    let mut last = shortest_last;
    while last != 0 {
        chain.push(last);
        let previous = predecessor[(mask >> 1) * node_count + last];
        if previous == u8::MAX {
            return Err(DpError::NoTour { nodes: node_count });
        }
        mask ^= 1usize << last;
        last = previous as usize;
    }
    chain.reverse();

    let mut tour = Vec::with_capacity(node_count + 1);
    tour.push(0);
    tour.extend(chain);
    tour.push(0);

    Ok(BitmaskSolution {
        cost: shortest,
        tour,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple "is this a subsequence of that one" test.
    fn is_subsequence<T: PartialEq>(needle: &[T], haystack: &[T]) -> bool {
        let mut position = 0;
        for element in haystack {
            if position < needle.len() && needle[position] == *element {
                position += 1;
            }
        }
        position == needle.len()
    }

    #[test]
    fn knapsack_01_asset_picks_the_best_items() {
        let items = [
            Item { weight: 2, value: 3 },
            Item { weight: 3, value: 4 },
            Item { weight: 4, value: 5 },
            Item { weight: 5, value: 6 },
        ];
        let solution = knapsack_01(&items, 8).unwrap();
        assert_eq!(solution.value, 10);
        assert_eq!(solution.weight, 8);
        assert_eq!(solution.items, vec![1, 3]);
    }

    #[test]
    fn knapsack_01_asset_on_the_boundary_inputs() {
        // No item at all.
        let solution = knapsack_01(&[], 5).unwrap();
        assert_eq!(solution.value, 0);
        assert_eq!(solution.weight, 0);
        assert!(solution.items.is_empty());

        // A capacity of zero leaves room for the items of weight zero only.
        let items = [
            Item { weight: 0, value: 5 },
            Item { weight: 1, value: 7 },
        ];
        let solution = knapsack_01(&items, 0).unwrap();
        assert_eq!(solution.value, 5);
        assert_eq!(solution.items, vec![0]);

        // An item that does not fit is left out.
        let solution = knapsack_01(&[Item { weight: 9, value: 100 }], 3).unwrap();
        assert_eq!(solution.value, 0);
        assert!(solution.items.is_empty());
    }

    #[test]
    fn knapsack_01_asset_refuses_a_negative_capacity_and_a_negative_weight() {
        // The counterexample of the card: the capacity and the weights have to be
        // not negative integers.
        assert_eq!(
            knapsack_01(&[], -1),
            Err(DpError::NegativeCapacity { capacity: -1 })
        );
        assert_eq!(
            knapsack_01(&[Item { weight: -2, value: 3 }], 5),
            Err(DpError::NegativeWeight {
                index: 0,
                weight: -2
            })
        );
    }

    #[test]
    fn longest_increasing_subsequence_asset_finds_the_longest_run() {
        let sequence = [10, 9, 2, 5, 3, 7, 101, 18];
        let result = longest_increasing_subsequence(&sequence);
        assert_eq!(result.length, 4);
        assert_eq!(result.indices, vec![2, 4, 5, 7]);
        // The result really is a strictly increasing subsequence of the input.
        let values: Vec<i64> = result.indices.iter().map(|&i| sequence[i]).collect();
        assert_eq!(values, vec![2, 3, 7, 18]);
        for window in values.windows(2) {
            assert!(window[0] < window[1]);
        }
    }

    #[test]
    fn longest_increasing_subsequence_asset_on_the_boundary_inputs() {
        // An empty sequence.
        let result = longest_increasing_subsequence::<i64>(&[]);
        assert_eq!(result.length, 0);
        assert!(result.indices.is_empty());

        // A single element.
        let result = longest_increasing_subsequence(&[7]);
        assert_eq!(result.length, 1);
        assert_eq!(result.indices, vec![0]);

        // The same element twice: the subsequence is strictly increasing, so the
        // length is one and not two.
        let result = longest_increasing_subsequence(&[5, 5, 5]);
        assert_eq!(result.length, 1);
        assert_eq!(result.indices, vec![2]);
    }

    #[test]
    fn longest_increasing_subsequence_asset_is_strictly_increasing() {
        // The counterexample of the card: the non-decreasing variant would give
        // three, the strict one gives two.
        let result = longest_increasing_subsequence(&[1, 1, 2]);
        assert_eq!(result.length, 2);
        let sequence = [1, 1, 2];
        let values: Vec<i64> = result.indices.iter().map(|&i| sequence[i]).collect();
        assert_eq!(values, vec![1, 2]);
    }

    #[test]
    fn longest_common_subsequence_asset_finds_the_common_sequence() {
        let first = ['a', 'b', 'c', 'd'];
        let second = ['a', 'b', 'd'];
        let result = longest_common_subsequence(&first, &second).unwrap();
        assert_eq!(result.length, 3);
        assert_eq!(result.sequence, vec!['a', 'b', 'd']);
        assert!(is_subsequence(&result.sequence, &first));
        assert!(is_subsequence(&result.sequence, &second));
    }

    #[test]
    fn longest_common_subsequence_asset_on_the_boundary_inputs() {
        // An empty sequence has nothing in common with the other one.
        let empty: [char; 0] = [];
        let result = longest_common_subsequence(&empty, &['a', 'b', 'c']).unwrap();
        assert_eq!(result.length, 0);
        assert!(result.sequence.is_empty());

        // Two sequences that have nothing in common.
        let result = longest_common_subsequence(&['a', 'b', 'c'], &['x', 'y', 'z']).unwrap();
        assert_eq!(result.length, 0);
        assert!(result.sequence.is_empty());

        // The answer does not depend on the order of the two sequences.
        let first = ['a', 'b', 'c', 'b', 'd', 'a', 'b'];
        let second = ['b', 'd', 'c', 'a', 'b', 'a'];
        let one = longest_common_subsequence(&first, &second).unwrap();
        let other = longest_common_subsequence(&second, &first).unwrap();
        assert_eq!(one.length, 4);
        assert_eq!(one.length, other.length);
        assert!(is_subsequence(&one.sequence, &first));
        assert!(is_subsequence(&one.sequence, &second));
    }

    #[test]
    fn longest_common_subsequence_asset_refuses_a_table_that_does_not_fit() {
        // The counterexample of the card: the table of the dynamic program has one
        // cell per pair of positions, so a long pair of sequences does not fit in
        // memory.
        let long = vec!['a'; 5_000];
        assert_eq!(
            longest_common_subsequence(&long, &long),
            Err(DpError::ProblemTooLarge {
                cells: 25_010_001,
                limit: MAX_LCS_CELLS
            })
        );
    }

    #[test]
    fn coin_change_asset_finds_the_fewest_coins() {
        // The greedy takes 4 + 1 + 1 and needs three coins, the dynamic program
        // finds the two coins of three.
        let result = coin_change(&[1, 3, 4], 6).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.coins, vec![3, 3]);
    }

    #[test]
    fn coin_change_asset_on_the_boundary_inputs() {
        // An amount of zero needs no coin at all.
        let result = coin_change(&[1, 3, 4], 0).unwrap();
        assert_eq!(result.count, 0);
        assert!(result.coins.is_empty());

        // A denomination of zero is dropped and a duplicate is counted once.
        let result = coin_change(&[0, 1, 1], 2).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.coins, vec![1, 1]);

        // An amount that cannot be reached.
        assert_eq!(
            coin_change(&[2, 4], 3),
            Err(DpError::UnreachableAmount { amount: 3 })
        );
    }

    #[test]
    fn coin_change_asset_refuses_a_negative_amount_and_a_negative_denomination() {
        // The counterexample of the card: the amount and the denominations have to
        // be integers, a negative one has no meaning here.
        assert_eq!(
            coin_change(&[1], -1),
            Err(DpError::NegativeAmount { amount: -1 })
        );
        assert_eq!(
            coin_change(&[1, -2], 3),
            Err(DpError::NegativeDenomination { denomination: -2 })
        );
    }

    #[test]
    fn interval_dp_asset_finds_the_cheapest_plan() {
        let dimensions = [10i64, 30, 5, 60];
        let cost = |start: usize, end: usize| dimensions[start] * dimensions[end + 1];
        let solution = interval_dp(3, cost).unwrap();
        assert_eq!(solution.cost, 650);
        assert_eq!(solution.merges, vec![(0, 1), (0, 2)]);
        // The plan really costs what the answer says.
        let total: i64 = solution.merges.iter().map(|&(start, end)| cost(start, end)).sum();
        assert_eq!(total, solution.cost);
    }

    #[test]
    fn interval_dp_asset_on_the_boundary_inputs() {
        // A single piece is not split at all.
        let solution = interval_dp(1, |_, _| 100).unwrap();
        assert_eq!(solution.cost, 0);
        assert!(solution.merges.is_empty());

        // No piece at all.
        let solution = interval_dp(0, |_, _| 100).unwrap();
        assert_eq!(solution.cost, 0);
        assert!(solution.merges.is_empty());

        // Two pieces: there is only one plan.
        let solution = interval_dp(2, |_, _| 7).unwrap();
        assert_eq!(solution.cost, 7);
        assert_eq!(solution.merges, vec![(0, 1)]);
    }

    #[test]
    fn interval_dp_asset_makes_no_assumption_on_the_cost() {
        // The counterexample of the card: the cost does not have to be symmetric and
        // it does not have to satisfy the triangle inequality.
        let cost = |start: usize, end: usize| ((start + 1) * 10 + (end + 1)) as i64;
        let solution = interval_dp(3, cost).unwrap();
        assert_eq!(solution.cost, 25);
        assert_eq!(solution.merges, vec![(0, 1), (0, 2)]);
        let total: i64 = solution.merges.iter().map(|&(start, end)| cost(start, end)).sum();
        assert_eq!(total, solution.cost);
    }

    #[test]
    fn bitmask_dp_asset_finds_the_shortest_tour() {
        let distances = vec![
            vec![0, 2, 100, 20],
            vec![20, 0, 3, 100],
            vec![1, 20, 0, 4],
            vec![5, 1, 20, 0],
        ];
        let solution = bitmask_dp(&distances).unwrap();
        assert_eq!(solution.cost, 14);
        assert_eq!(solution.tour, vec![0, 1, 2, 3, 0]);

        // The tour visits every node exactly once and costs what the answer says.
        let interior = &solution.tour[1..solution.tour.len() - 1];
        let mut visited = interior.to_vec();
        visited.sort_unstable();
        assert_eq!(visited, vec![1, 2, 3]);
        let total: i64 = solution
            .tour
            .windows(2)
            .map(|pair| distances[pair[0]][pair[1]])
            .sum();
        assert_eq!(total, solution.cost);
    }

    #[test]
    fn bitmask_dp_asset_on_the_boundary_inputs() {
        // No node at all: there is no tour and nothing to pay.
        let solution = bitmask_dp(&[]).unwrap();
        assert_eq!(solution.cost, 0);
        assert!(solution.tour.is_empty());

        // A single node: the tour stays where it is.
        let solution = bitmask_dp(&[vec![0]]).unwrap();
        assert_eq!(solution.cost, 0);
        assert_eq!(solution.tour, vec![0]);
    }

    #[test]
    fn bitmask_dp_asset_refuses_a_matrix_that_gives_no_tour() {
        // The counterexample of the card: a graph that is not connected has states
        // that cannot be reached, and the tour does not exist.
        let distances = vec![
            vec![0, 1],
            vec![i64::MAX, 0],
        ];
        assert_eq!(bitmask_dp(&distances), Err(DpError::NoTour { nodes: 2 }));

        // A matrix that is not square.
        assert_eq!(
            bitmask_dp(&[vec![0, 1, 2]]),
            Err(DpError::InvalidDimensions {
                expected: 1,
                found: 3
            })
        );
    }
}
