//! The seven sorting algorithms of the catalogue (the assets `algorithm.sorting.*`).
//!
//! Every public function of this module names its asset in its doc comment:
//! `* asset: `algorithm.\<category\>.\<name\>``.
//!
//! | function | asset | what it does |
//! |---|---|---|
//! | [`merge_sort`] | `algorithm.sorting.merge_sort` | stable, worst case `O(n log n)`, `O(n)` extra space |
//! | [`heap_sort`] | `algorithm.sorting.heap_sort` | in place, worst case `O(n log n)`, `O(1)` extra space, unstable |
//! | [`quick_sort`] | `algorithm.sorting.quick_sort` | in place, average `O(n log n)`, a median-of-three pivot |
//! | [`quickselect`] | `algorithm.sorting.quickselect` | the `k`-th smallest element, average `O(n)` |
//! | [`counting_sort`] | `algorithm.sorting.counting_sort` | a non-comparison sort over a small integer key |
//! | [`radix_sort`] | `algorithm.sorting.radix_sort` | an LSD radix sort over fixed-width integer keys |
//! | [`top_k`] | `algorithm.sorting.top_k_heap` | the `k` largest elements of a stream, `O(n log k)` |
//!
//! # The rules of this module
//!
//! * **Deterministic.** The same input always gives the same output. Where the algorithm leaves a choice open
//!   (the pivot, the order of equal keys) the choice is fixed and written down in the doc comment of the function,
//!   so that a test can pin it.
//! * **No panic on a valid input.** An empty slice is a valid input and gives an empty result. An input the
//!   algorithm cannot handle (an index outside the slice, a key domain that is too wide) is reported as an `Err`
//!   with a readable reason.
//! * **The standard library only.** The crate has no dependency at all, and every dependency would need a licence
//!   review (design §1.2).

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::error::Error;
use std::fmt;

/// The largest key range [`counting_sort`] accepts.
///
/// Counting sort allocates one counter per key, so the key domain has to fit in memory: one `usize` per key.
/// The limit below is 1 048 576 keys (8 MiB of counters). A wider domain is reported as
/// [`SortingError::KeyRangeTooLarge`]; the card then points at [`radix_sort`] or at a comparison sort.
pub const MAX_COUNTING_KEY_RANGE: usize = 1 << 20;

/// The base of the digits [`radix_sort`] works with.
///
/// One byte per digit, so a `u64` key needs at most eight passes.
const RADIX_BASE: u64 = 256;

/// The failures the sorting assets of this module can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortingError {
    /// The index asked for is not inside the slice.
    IndexOutOfRange {
        /// The index that was asked for.
        index: usize,
        /// The length of the slice.
        len: usize,
    },
    /// The keys of the items span a domain that counting sort cannot hold in memory.
    KeyRangeTooLarge {
        /// The largest key that was found in the input.
        max_key: usize,
        /// The largest key range counting sort accepts.
        max_allowed: usize,
    },
}

impl fmt::Display for SortingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SortingError::IndexOutOfRange { index, len } => {
                write!(f, "the index {index} is outside a slice of length {len}")
            }
            SortingError::KeyRangeTooLarge { max_key, max_allowed } => write!(
                f,
                "the key range is too wide for counting sort: the largest key is {max_key} and the limit is \
                 {max_allowed}; use radix sort or a comparison sort instead"
            ),
        }
    }
}

impl Error for SortingError {}

/// Merge sort: a stable, worst-case `O(n log n)` divide-and-conquer sort.
///
/// The slice is cut in half, both halves are sorted and the two ordered halves are merged. The merge takes the
/// element of the left half when the two are equal (`<=`), which is what makes the sort stable.
///
/// * asset: `algorithm.sorting.merge_sort`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.merge_sort/card.md`
///
/// # Input preconditions (from the card)
///
/// * `data: in_memory_comparable` — the whole sequence must fit in memory and the elements must be comparable,
///   which the signature `T: Ord + Clone` enforces.
/// * `stability: stable` — elements that compare equal keep their relative order. This implementation guarantees
///   it, so it can sort by one key while a second key is carried along in the element.
///
/// # Complexity (from the card)
///
/// * Time `O(n log n)` in the worst, the average and the best case.
/// * Extra space `O(n)`: the auxiliary buffer that holds the merged halves.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *A small input that only has to be fast*: "数据量小且追求常数性能时，插入排序在 n < 32 左右更快" — the
///   result is still correct, only slower than an insertion sort would be.
/// * *Nearly sorted data*: "数据已是近乎有序时，归并的拷贝开销高于原地排序" — the copy of the merge costs more
///   than an in-place sort would.
/// * *A huge input and a tight memory*: use [`heap_sort`], which needs `O(1)` extra space.
///
/// # Determinism
///
/// The result depends only on the input. Equal elements keep the order they had, so the output is unique.
pub fn merge_sort<T: Ord + Clone>(items: &mut [T]) {
    let len = items.len();
    if len < 2 {
        return;
    }
    // The auxiliary buffer: one slot per element, which is the O(n) extra space of the card.
    let mut buffer = items.to_vec();
    merge_sort_range(items, &mut buffer, 0, len);
}

/// Sorts `items[lo..hi]` into `items`, using `buffer` as the auxiliary space.
fn merge_sort_range<T: Ord + Clone>(items: &mut [T], buffer: &mut [T], lo: usize, hi: usize) {
    if hi - lo < 2 {
        return;
    }
    let mid = lo + (hi - lo) / 2;
    merge_sort_range(items, buffer, lo, mid);
    merge_sort_range(items, buffer, mid, hi);
    // Merge the two ordered halves. The left half wins a tie, which keeps the sort stable.
    let mut left = lo;
    let mut right = mid;
    let mut out = lo;
    while left < mid && right < hi {
        if items[left] <= items[right] {
            buffer[out] = items[left].clone();
            left += 1;
        } else {
            buffer[out] = items[right].clone();
            right += 1;
        }
        out += 1;
    }
    while left < mid {
        buffer[out] = items[left].clone();
        left += 1;
        out += 1;
    }
    while right < hi {
        buffer[out] = items[right].clone();
        right += 1;
        out += 1;
    }
    items[lo..hi].clone_from_slice(&buffer[lo..hi]);
}

/// Heap sort: an in-place sort with a worst-case `O(n log n)` guarantee and `O(1)` extra space.
///
/// The slice is first turned into a max heap, then the root (the largest element) is swapped to the end of the
/// heap area and the heap is restored. The sort is **not** stable.
///
/// * asset: `algorithm.sorting.heap_sort`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.heap_sort/card.md`
///
/// # Input preconditions (from the card)
///
/// * `data: in_memory_comparable` — the elements must be comparable (`T: Ord`).
/// * `stability: unstable` — the caller must not rely on the order of equal elements.
/// * The slice is modified in place; the caller owns it exclusively (the sort is not a concurrent operation on
///   shared data).
///
/// # Complexity (from the card)
///
/// * Time `O(n log n)`: `O(n)` for the bottom-up build of the heap plus `n` sift-downs of `O(log n)`.
/// * Extra space `O(1)`: the sort works in place.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *A stable sort is needed*: "需要稳定排序时不能使用堆排序" — use [`merge_sort`].
/// * *A small input, or an input that is already nearly ordered*: "数据量小或已接近有序时，堆排序的常数劣于插入
///   排序与快速排序" — the result is still correct, only slower.
/// * *The average speed matters more than the worst case*: [`quick_sort`] is usually faster.
///
/// # Determinism
///
/// The result is a function of the input only. The order of equal elements is an artefact of the heap layout: it
/// is stable for a given input and this implementation, but it is not a contract and must not be relied on.
pub fn heap_sort<T: Ord>(items: &mut [T]) {
    let len = items.len();
    if len < 2 {
        return;
    }
    build_max_heap(items);
    for end in (1..len).rev() {
        items.swap(0, end);
        // `end` is the new length of the heap area: the element at `end` is already in its final place.
        sift_down(items, 0, end);
    }
}

/// Turns `items` into a max heap, bottom up, in `O(n)`.
fn build_max_heap<T: Ord>(items: &mut [T]) {
    let len = items.len();
    if len < 2 {
        return;
    }
    // The last parent of a leaf is at len / 2 - 1; the leaves need no work.
    for start in (0..len / 2).rev() {
        sift_down(items, start, len);
    }
}

/// Moves the element at `root` down until the max-heap property holds in `items[..len]`.
fn sift_down<T: Ord>(items: &mut [T], mut root: usize, len: usize) {
    loop {
        // `root < len / 2` is the "the root has a left child" test. It keeps `root * 2 + 1` from overflowing.
        if root >= len / 2 {
            return;
        }
        let child = root * 2 + 1;
        let right = child + 1;
        let mut largest = root;
        if items[child] > items[largest] {
            largest = child;
        }
        if right < len && items[right] > items[largest] {
            largest = right;
        }
        if largest == root {
            return;
        }
        items.swap(root, largest);
        root = largest;
    }
}

/// Quick sort: an in-place sort with an average `O(n log n)` running time and a very small constant.
///
/// The range is split around a pivot and both sides are sorted. The pivot is the **median of the first, the middle
/// and the last element** of the range, so the choice needs no random number and the split stays balanced on an
/// ordered input. The sort is **not** stable.
///
/// * asset: `algorithm.sorting.quick_sort`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.quick_sort/card.md`
///
/// # Input preconditions (from the card)
///
/// * `data: in_memory_comparable` — the elements must be comparable (`T: Ord`).
/// * `stability: unstable` — the caller must not rely on the order of equal elements.
/// * The slice is modified in place; the caller owns it exclusively.
///
/// # Complexity (from the card)
///
/// * Time `O(n log n)` on average, `O(n^2)` in the worst case (a pivot that splits the range in the worst possible
///   way). The median-of-three pivot removes the classic "already sorted input" trap of the card.
/// * Extra space `O(log n)`: the work list of the ranges that are still to be sorted. The implementation is
///   iterative, so a deep recursion cannot overflow the stack (the card asks for that).
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *A stable sort is needed*: "需要稳定排序——快速排序不满足" — use [`merge_sort`].
/// * *A worst-case guarantee is needed and no random pivot may be used*: "需要最坏情况 O(n log n) 保证且不能用
///   随机枢轴时——改用归并或堆排序" — use [`merge_sort`] or [`heap_sort`].
/// * *The data is enormous and the memory is tight*: "数据量极大且内存紧张——堆排序更稳妥".
/// * *The data lives on a disk*: "外部排序（磁盘）——快排的随机访问代价高，应使用归并".
///
/// # Determinism
///
/// The pivot is the median of three elements, not a random element, so the same input always gives the same
/// output. The order of equal elements is an artefact of the partition and is not a contract.
pub fn quick_sort<T: Ord>(items: &mut [T]) {
    let _ = quick_sort_peak_depth(items);
}

/// Sorts the slice and returns the largest number of ranges that were waiting in the work list.
///
/// The work list is the extra space of the sort: the smaller side of a split is handled right away and the
/// larger one is pushed, so the range that is handled right away at least halves with every step and the work
/// list never holds more than `O(log n)` ranges.
fn quick_sort_peak_depth<T: Ord>(items: &mut [T]) -> usize {
    let len = items.len();
    if len < 2 {
        return 0;
    }
    let mut pending: Vec<(usize, usize)> = Vec::new();
    let mut peak = 0usize;
    let mut current = Some((0usize, len - 1));
    while let Some((mut lo, mut hi)) = current {
        while lo < hi {
            let pivot = partition(items, lo, hi);
            let left_len = pivot - lo;
            let right_len = hi - pivot;
            if left_len > right_len {
                // The right side is the smaller one: it is handled right away, the left side is pushed.
                if left_len > 0 {
                    pending.push((lo, pivot - 1));
                }
                lo = pivot + 1;
            } else {
                // The left side is the smaller one: it is handled right away, the right side is pushed.
                if right_len > 0 {
                    pending.push((pivot + 1, hi));
                }
                hi = pivot.saturating_sub(1);
            }
            peak = peak.max(pending.len());
        }
        current = pending.pop();
    }
    peak
}

/// Partitions `items[lo..=hi]` around a pivot and returns the final position of the pivot.
///
/// The pivot is the median of the first, the middle and the last element of the range. It is moved to `hi - 1`,
/// the elements below it are moved to the front and the pivot is put in the middle of the range.
fn partition<T: Ord>(items: &mut [T], lo: usize, hi: usize) -> usize {
    debug_assert!(lo < hi);
    let mid = lo + (hi - lo) / 2;
    // Sort the three sampled values: afterwards items[lo] <= items[mid] <= items[hi].
    if items[lo] > items[mid] {
        items.swap(lo, mid);
    }
    if items[lo] > items[hi] {
        items.swap(lo, hi);
    }
    if items[mid] > items[hi] {
        items.swap(mid, hi);
    }
    // The median is now at `mid`; the largest of the three stays at `hi` and is not part of the scan.
    let pivot = hi - 1;
    items.swap(mid, pivot);
    // Lomuto partition: everything below the pivot moves to the front of the range.
    let mut store = lo;
    for k in lo..pivot {
        if items[k] < items[pivot] {
            items.swap(k, store);
            store += 1;
        }
    }
    items.swap(store, pivot);
    store
}

/// Quick select: the `k`-th smallest element of a slice, in average `O(n)`.
///
/// The slice is partitioned as in [`quick_sort`], but only the side that holds the wanted element is sorted
/// further. The slice is modified in place and is **not** fully ordered afterwards.
///
/// * asset: `algorithm.sorting.quickselect`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.quickselect/card.md`
///
/// # Input preconditions (from the card)
///
/// * `data: in_memory_comparable` — the elements must be comparable (`T: Ord + Clone`).
/// * `k` must be inside the slice: `k < items.len()`. Anything else is reported as
///   [`SortingError::IndexOutOfRange`], the card asks for an error rather than a panic.
/// * The slice is modified in place; the caller owns it exclusively.
///
/// # Complexity (from the card)
///
/// * Time `O(n)` on average, `O(n^2)` in the worst case.
/// * Extra space `O(1)`: the implementation is iterative.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The whole sequence has to be ordered*: "需要完整有序结果时——直接排序更快" — use [`quick_sort`].
/// * *Only the largest or the smallest element is needed*: "k 接近两端（最小/最大）时——单遍扫描求 min/max 即可".
/// * *The `k` largest elements are needed, all of them and in order*: "需要前 k 个全部有序——用堆或部分排序" —
///   use [`top_k`].
///
/// # Determinism
///
/// The pivot is the median of three elements, not a random element, so the same input always gives the same
/// answer. Only the `k`-th smallest *value* is returned; where equal values sit in the slice is not a contract.
pub fn quickselect<T: Ord + Clone>(items: &mut [T], k: usize) -> Result<T, SortingError> {
    let len = items.len();
    if k >= len {
        return Err(SortingError::IndexOutOfRange { index: k, len });
    }
    let mut lo = 0usize;
    let mut hi = len - 1;
    loop {
        if lo == hi {
            return Ok(items[lo].clone());
        }
        let pivot = partition(items, lo, hi);
        match k.cmp(&pivot) {
            Ordering::Less => hi = pivot.saturating_sub(1),
            Ordering::Equal => return Ok(items[pivot].clone()),
            Ordering::Greater => lo = pivot + 1,
        }
    }
}

/// Counting sort: a non-comparison sort over a small integer key domain.
///
/// The number of occurrences of every key is counted, the counts are turned into a prefix sum and the elements
/// are written back from the end, which keeps equal keys in their original order.
///
/// * asset: `algorithm.sorting.counting_sort`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.counting_sort/card.md`
///
/// # Input preconditions (from the card)
///
/// * `key_range: small_integer_domain` — the key of an item is a `usize` and the largest key of the input has to
///   fit into [`MAX_COUNTING_KEY_RANGE`]. A wider domain is reported as [`SortingError::KeyRangeTooLarge`], the
///   counterexample of the card ("键值范围很大（如 64 位哈希）时——O(k) 的额外空间不可接受").
/// * `data: in_memory_comparable` — the elements are moved as a whole, so `T: Clone` is needed.
/// * `stability: stable` — elements with an equal key keep their relative order.
///
/// # Complexity (from the card)
///
/// * Time `O(n + k)`: one pass to count, one to accumulate and one to write back, `k` being the number of keys.
/// * Extra space `O(k)`: the counter array.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The key range is wide*: "键值范围很大（如 64 位哈希）时——O(k) 的额外空间不可接受" — the function returns
///   [`SortingError::KeyRangeTooLarge`]; use [`radix_sort`] or a comparison sort.
/// * *The key is not an integer, or cannot be mapped to a small domain*: "键不是整数或无法映射到小范围时——不适用"
///   — map the key to a `usize` first, and keep the mapping order preserving.
/// * *The input is small*: "数据量很小——比较排序的常数更小".
///
/// # Determinism
///
/// The output is unique: equal keys keep their order, so the result depends only on the input.
pub fn counting_sort<T: Clone>(items: &mut [T], key: impl Fn(&T) -> usize) -> Result<(), SortingError> {
    if items.len() < 2 {
        return Ok(());
    }
    // The width of the key domain decides the size of the counter array.
    let mut max_key = 0usize;
    for item in items.iter() {
        let k = key(item);
        if k > max_key {
            max_key = k;
        }
    }
    if max_key >= MAX_COUNTING_KEY_RANGE {
        return Err(SortingError::KeyRangeTooLarge {
            max_key,
            max_allowed: MAX_COUNTING_KEY_RANGE,
        });
    }
    let mut counts = vec![0usize; max_key + 1];
    for item in items.iter() {
        counts[key(item)] += 1;
    }
    // The counters become the end position of every key.
    let mut running = 0usize;
    for count in counts.iter_mut() {
        running += *count;
        *count = running;
    }
    // The elements are written back from the end, so an equal key keeps the order it had.
    let mut out: Vec<T> = items.to_vec();
    for item in items.iter().rev() {
        let k = key(item);
        counts[k] -= 1;
        out[counts[k]] = item.clone();
    }
    items.clone_from_slice(&out);
    Ok(())
}

/// Radix sort: a least-significant-digit radix sort over fixed-width integer keys.
///
/// The key is taken apart into bytes, and every byte is sorted with a stable counting sort, from the least
/// significant byte to the most significant one. The result is stable because every pass is stable.
///
/// * asset: `algorithm.sorting.radix_sort`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.radix_sort/card.md`
///
/// # Input preconditions (from the card)
///
/// * `key_type: fixed_width_integer` — the key of an item is a `u64`. A key that is not an integer, or that does
///   not fit into 64 bits without loss, is not supported.
/// * The mapping from an element to its key has to **keep the order**: the sort is only correct when
///   `key(a) < key(b)` holds exactly when `a < b` holds. For a signed key the mapping `(x as u64) ^ (1 << 63)`
///   does that: it flips the sign bit, which turns the two's complement image of a signed value into an order
///   preserving unsigned one.
/// * `stability: stable` — elements with an equal key keep their relative order.
///
/// # Complexity (from the card)
///
/// * Time `O(d * (n + b))`, `d` being the number of bytes of the widest key and `b` the base (256 here).
/// * Extra space `O(n + b)`: the auxiliary buffer and the counter array.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The key is a float, a variable-length string or an object*: "键是浮点数、变长字符串或对象时——需要额外映射
///   且可能得不偿失" — a float needs the special handling of its IEEE 754 bit pattern, a string needs the MSD
///   variant; use a comparison sort otherwise.
/// * *The input is small*: "数据量小且比较成本低时——比较排序通常更快".
/// * *The keys use their whole width but stay sparse*: "键分布使有效位宽很大（如 64 位稀疏值）——趟数增加，优势
///   消失".
///
/// # Determinism
///
/// The output is unique: the passes are stable, so equal keys keep their order.
pub fn radix_sort<T: Clone>(items: &mut [T], key: impl Fn(&T) -> u64) {
    let len = items.len();
    if len < 2 {
        return;
    }
    // The number of bytes the widest key needs.
    let mut max_key = 0u64;
    for item in items.iter() {
        let k = key(item);
        if k > max_key {
            max_key = k;
        }
    }
    let mut passes = 1usize;
    let mut rest = max_key;
    while rest >= RADIX_BASE {
        rest /= RADIX_BASE;
        passes += 1;
    }
    let mut buffer: Vec<T> = items.to_vec();
    for pass in 0..passes {
        let shift = 8 * pass;
        let mut counts = [0usize; RADIX_BASE as usize];
        for item in items.iter() {
            counts[digit(key(item), shift)] += 1;
        }
        let mut running = 0usize;
        for count in counts.iter_mut() {
            running += *count;
            *count = running;
        }
        // The elements are written back from the end, so the pass is stable.
        for item in items.iter().rev() {
            let d = digit(key(item), shift);
            counts[d] -= 1;
            buffer[counts[d]] = item.clone();
        }
        items.clone_from_slice(&buffer);
    }
}

/// The byte of `key` at `shift` (a multiple of eight).
fn digit(key: u64, shift: usize) -> usize {
    ((key >> shift) & 0xff) as usize
}

/// An element in the heap of [`top_k`]: the value and the order in which it arrived.
struct HeapItem<T> {
    /// The value of the element.
    value: T,
    /// The position of the element in the input, counted from zero.
    arrival: usize,
}

impl<T: Ord> PartialEq for HeapItem<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.arrival == other.arrival
    }
}

impl<T: Ord> Eq for HeapItem<T> {}

impl<T: Ord> PartialOrd for HeapItem<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Ord> Ord for HeapItem<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // The value decides. Between two equal values the element that arrived earlier is the greater one, so
        // that the heap drops the later one when it has to make room for a better element.
        self.value
            .cmp(&other.value)
            .then_with(|| other.arrival.cmp(&self.arrival))
    }
}

/// Top-k with a heap: the `k` largest elements of a stream, in `O(n log k)`.
///
/// A heap of `k` elements is kept while the stream is read. The smallest element of the heap is the current `k`-th
/// largest; an element that is not greater than it is dropped right away, so the memory does not grow with the
/// length of the stream.
///
/// * asset: `algorithm.sorting.top_k_heap`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.top_k_heap/card.md`
///
/// # Input preconditions (from the card)
///
/// * `streaming: true` — the input is any iterator, so the elements do not have to be in memory at once.
/// * `data: in_memory_comparable` — the elements must be comparable (`T: Ord`).
/// * `order: kth_element_or_full_set` — the result is the set of the `k` largest elements, not the whole order.
///
/// # Complexity (from the card)
///
/// * Time `O(n log k)`: one comparison of `O(log k)` per element of the stream.
/// * Extra space `O(k)`: the heap, and nothing that grows with `n`.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The input is small and the whole order is needed*: "数据量小且需要完整排序结果时——直接排序更简单" — use
///   [`quick_sort`] or [`merge_sort`].
/// * *`k` is as large as `n`*: "k 接近 n 时——堆的优势消失，排序反而更省事".
/// * *The ties have to be broken in a particular way*: "需要精确的前 k 名且有并列——并列元素的取舍规则要显式定义"
///   — the rule of this implementation is written down below.
/// * *The data is queried often and changes often*: "数据可多次查询且变化频繁——应考虑有序结构而不是每次重建堆".
///
/// # Determinism
///
/// The set of the `k` largest elements is unique. Two rules make the rest of the result fixed as well:
///
/// * Once the heap holds `k` elements, a new element is taken only when it is **strictly greater** than the
///   smallest element of the heap; an element that is equal to it is dropped.
/// * Between two elements with an equal value the one that **arrived earlier** is kept, and the result lists the
///   equal values in the order of their arrival.
///
/// The result is therefore in descending order, and the same input always gives the same output.
pub fn top_k<T: Ord>(items: impl IntoIterator<Item = T>, k: usize) -> Vec<T> {
    if k == 0 {
        // The card: an empty k gives an empty result and does not consume the input.
        return Vec::new();
    }
    // No `with_capacity`: an enormous k must not turn into a failed allocation.
    let mut heap: BinaryHeap<Reverse<HeapItem<T>>> = BinaryHeap::new();
    for (arrival, value) in items.into_iter().enumerate() {
        let item = HeapItem { value, arrival };
        if heap.len() < k {
            heap.push(Reverse(item));
            continue;
        }
        // The smallest element of the heap is the one that has to make room.
        let better = match heap.peek() {
            Some(Reverse(worst)) => item.value > worst.value,
            None => true,
        };
        if better {
            heap.pop();
            heap.push(Reverse(item));
        }
    }
    let mut kept: Vec<HeapItem<T>> = heap.into_iter().map(|Reverse(item)| item).collect();
    // Descending by value; the elements with an equal value keep the order of their arrival.
    kept.sort_by(|a, b| b.value.cmp(&a.value).then_with(|| a.arrival.cmp(&b.arrival)));
    kept.into_iter().map(|item| item.value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An element that remembers where it came from, so that a test can see whether a sort is stable.
    #[derive(Debug, Clone)]
    struct Keyed {
        /// The key the element is sorted by.
        key: i32,
        /// The position of the element in the input.
        index: usize,
    }

    // Two elements with an equal key are equal, so that the sort sees them as a tie.
    impl PartialEq for Keyed {
        fn eq(&self, other: &Self) -> bool {
            self.key == other.key
        }
    }

    impl Eq for Keyed {}

    impl PartialOrd for Keyed {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for Keyed {
        fn cmp(&self, other: &Self) -> Ordering {
            self.key.cmp(&other.key)
        }
    }

    /// Builds the keyed elements of `keys`, with the position of every element as its index.
    fn keyed(keys: &[i32]) -> Vec<Keyed> {
        keys.iter()
            .enumerate()
            .map(|(index, &key)| Keyed { key, index })
            .collect()
    }

    /// The arrival order of the keyed elements, as a vector of the original positions.
    fn arrival_order(items: &[Keyed]) -> Vec<usize> {
        items.iter().map(|item| item.index).collect()
    }

    #[test]
    fn merge_sort_sorts_and_keeps_the_card_order() {
        let mut empty: Vec<i32> = Vec::new();
        merge_sort(&mut empty);
        assert!(empty.is_empty());

        let mut single = vec![7];
        merge_sort(&mut single);
        assert_eq!(single, vec![7]);

        let mut data = vec![5, 3, 9, 1, 3, 5, -2, 0, 3];
        merge_sort(&mut data);
        assert_eq!(data, vec![-2, 0, 1, 3, 3, 3, 5, 5, 9]);

        // The card asks for the stability of the equal keys.
        let mut items = keyed(&[3, 1, 3, 2, 1, 3]);
        merge_sort(&mut items);
        assert_eq!(arrival_order(&items), vec![1, 4, 3, 0, 2, 5]);
    }

    #[test]
    fn merge_sort_is_not_the_fastest_choice_for_a_tiny_or_nearly_sorted_input() {
        // The counterexamples of the card: a small input and a nearly sorted input.
        let mut tiny = vec![2, 1];
        merge_sort(&mut tiny);
        assert_eq!(tiny, vec![1, 2]);

        let mut nearly_sorted = vec![1, 2, 4, 3, 5, 6, 7];
        merge_sort(&mut nearly_sorted);
        assert_eq!(nearly_sorted, vec![1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn heap_sort_sorts_in_place() {
        let mut empty: Vec<i32> = Vec::new();
        heap_sort(&mut empty);
        assert!(empty.is_empty());

        let mut single = vec![42];
        heap_sort(&mut single);
        assert_eq!(single, vec![42]);

        let mut data = vec![9, 4, 7, 1, 4, 0, 4];
        heap_sort(&mut data);
        assert_eq!(data, vec![0, 1, 4, 4, 4, 7, 9]);
    }

    #[test]
    fn heap_sort_builds_a_max_heap_and_does_not_preserve_the_order_of_equal_keys() {
        // The invariant of the card: after the build the slice satisfies the max-heap property.
        let mut data = vec![3, 9, 2, 1, 7, 8];
        build_max_heap(&mut data);
        for child in 1..data.len() {
            let parent = (child - 1) / 2;
            assert!(data[parent] >= data[child], "the max-heap property is broken at {child}");
        }
        // The counterexample of the card: heap sort is not stable. The order of the equal keys is pinned here,
        // it is deterministic but it is not a contract.
        let mut items = keyed(&[1, 2, 1, 2]);
        heap_sort(&mut items);
        assert_eq!(arrival_order(&items), vec![0, 2, 3, 1]);
    }

    #[test]
    fn quick_sort_sorts_and_uses_a_deterministic_pivot() {
        let mut empty: Vec<i32> = Vec::new();
        quick_sort(&mut empty);
        assert!(empty.is_empty());

        let mut single = vec![1];
        quick_sort(&mut single);
        assert_eq!(single, vec![1]);

        let mut data = vec![4, 2, 7, 1, 2, 9, 9, 0];
        quick_sort(&mut data);
        assert_eq!(data, vec![0, 1, 2, 2, 4, 7, 9, 9]);

        // The same input gives the same output: the pivot is not a random element.
        let mut again = vec![4, 2, 7, 1, 2, 9, 9, 0];
        quick_sort(&mut again);
        assert_eq!(data, again);
    }

    #[test]
    fn quick_sort_survives_the_sorted_input_that_breaks_a_first_element_pivot() {
        // The counterexample of the card: with a pivot that is always the first element an ordered input
        // degrades to O(n^2). The median-of-three pivot does not, and the explicit work list keeps the stack
        // shallow.
        let n = 20_000;
        let mut sorted: Vec<i32> = (0..n).collect();
        quick_sort(&mut sorted);
        assert!(sorted.windows(2).all(|w| w[0] <= w[1]));

        let mut reversed: Vec<i32> = (0..n).rev().collect();
        quick_sort(&mut reversed);
        assert!(reversed.windows(2).all(|w| w[0] <= w[1]));

        // The extra space of the card: the work list never holds more than O(log n) ranges.
        let mut sorted: Vec<i32> = (0..n).collect();
        let peak = quick_sort_peak_depth(&mut sorted);
        assert!(peak <= 16, "the work list held {peak} ranges, more than log2({n})");
    }

    #[test]
    fn quickselect_returns_the_kth_smallest_element() {
        let mut data = vec![5, 1, 9, 3, 3, 7];
        assert_eq!(quickselect(&mut data, 0), Ok(1));

        let mut data = vec![5, 1, 9, 3, 3, 7];
        assert_eq!(quickselect(&mut data, 2), Ok(3));

        let mut data = vec![5, 1, 9, 3, 3, 7];
        assert_eq!(quickselect(&mut data, 5), Ok(9));
    }

    #[test]
    fn quickselect_rejects_an_index_outside_the_slice() {
        // The card asks for an error and not a panic on an empty slice or on an index that is out of range.
        let mut empty: Vec<i32> = Vec::new();
        assert_eq!(
            quickselect(&mut empty, 0),
            Err(SortingError::IndexOutOfRange { index: 0, len: 0 })
        );

        let mut single = vec![1];
        assert_eq!(
            quickselect(&mut single, 1),
            Err(SortingError::IndexOutOfRange { index: 1, len: 1 })
        );
    }

    #[test]
    fn counting_sort_sorts_and_keeps_the_order_of_equal_keys() {
        let mut empty: Vec<u8> = Vec::new();
        counting_sort(&mut empty, |b| *b as usize).expect("an empty slice is a valid input");
        assert!(empty.is_empty());

        let mut single = vec![7u8];
        counting_sort(&mut single, |b| *b as usize).expect("a single element is a valid input");
        assert_eq!(single, vec![7]);

        let mut data = vec![3u8, 1, 3, 0, 1, 255, 0];
        counting_sort(&mut data, |b| *b as usize).expect("the key domain is small enough");
        assert_eq!(data, vec![0, 0, 1, 1, 3, 3, 255]);

        // The stability of the card: the equal keys keep the order of their arrival.
        let mut items = keyed(&[1, 0, 1, 0]);
        counting_sort(&mut items, |item| item.key as usize).expect("the key domain is small enough");
        assert_eq!(arrival_order(&items), vec![1, 3, 0, 2]);
    }

    #[test]
    fn counting_sort_rejects_a_key_range_that_does_not_fit_in_memory() {
        // The counterexample of the card: a wide key domain makes the counter array too big.
        let mut data = vec![0usize, usize::MAX / 2];
        let error = counting_sort(&mut data, |k| *k).expect_err("the key domain is too wide");
        assert_eq!(
            error,
            SortingError::KeyRangeTooLarge {
                max_key: usize::MAX / 2,
                max_allowed: MAX_COUNTING_KEY_RANGE,
            }
        );
    }

    #[test]
    fn radix_sort_sorts_fixed_width_integer_keys() {
        let mut empty: Vec<u32> = Vec::new();
        radix_sort(&mut empty, |v| *v as u64);
        assert!(empty.is_empty());

        let mut single = vec![9u32];
        radix_sort(&mut single, |v| *v as u64);
        assert_eq!(single, vec![9]);

        let mut data = vec![170u32, 45, 75, 90, 802, 24, 2, 66];
        radix_sort(&mut data, |v| *v as u64);
        assert_eq!(data, vec![2, 24, 45, 66, 75, 90, 170, 802]);

        // The whole width of the key has to work.
        let mut wide = vec![u64::MAX, 0, 1 << 40, u64::MAX / 3];
        radix_sort(&mut wide, |v| *v);
        assert_eq!(wide, vec![0, 1 << 40, u64::MAX / 3, u64::MAX]);
    }

    #[test]
    fn radix_sort_maps_a_signed_key_in_an_order_preserving_way() {
        // The card warns against a float or a string key. An order preserving mapping of a signed key is the
        // way to use the sort for them: flipping the sign bit turns the two's complement image of a signed
        // value into an order preserving unsigned one.
        let mut items = keyed(&[-5, 3, -1, 0, 3]);
        radix_sort(&mut items, |item| (item.key as i64 as u64) ^ (1u64 << 63));
        assert_eq!(arrival_order(&items), vec![0, 2, 3, 1, 4]);
    }

    #[test]
    fn top_k_returns_the_k_largest_elements_in_descending_order() {
        assert!(top_k(Vec::<i32>::new(), 3).is_empty());
        assert_eq!(top_k(vec![1, 2, 3], 0), Vec::<i32>::new());
        assert_eq!(top_k(vec![1, 2, 3], 1), vec![3]);
        assert_eq!(top_k(vec![1, 2, 3], 3), vec![3, 2, 1]);
        assert_eq!(top_k(vec![5, 1, 5, 9, 3], 2), vec![9, 5]);
    }

    #[test]
    fn top_k_keeps_the_earlier_of_two_equal_elements_and_does_not_consume_the_input_for_k_zero() {
        // The card asks for an explicit rule for the ties.
        let mut consumed = 0usize;
        let counted = (0..4).inspect(|_| {
            consumed += 1;
        });
        assert!(top_k(counted, 0).is_empty());
        assert_eq!(consumed, 0, "a k of zero must not consume the input");

        // Three equal values and a k of two: the two that arrived first are kept.
        let items = keyed(&[7, 7, 7]);
        let kept = top_k(items, 2);
        assert_eq!(kept.len(), 2);
        assert_eq!(arrival_order(&kept), vec![0, 1]);
    }
}
