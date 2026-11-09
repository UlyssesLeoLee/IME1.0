//! The six data structures of the catalogue (the assets `algorithm.datastructure.*`).
//!
//! Every public function of this module names its asset in its doc comment:
//! `* asset: `algorithm.\<category\>.\<name>``.
//!
//! The rules the six structures keep:
//!
//! * **The standard library only.** No third-party crate: the crate has to stay auditable line by line.
//!   The card of the hash table and the card of the binary heap both point at the structures of the
//!   standard library; they are written here anyway, because the point of this crate is to make the
//!   mechanics of the asset visible and testable. A production code should still prefer
//!   `std::collections::HashMap` and `std::collections::BinaryHeap`.
//! * **Deterministic.** The same input gives the same output, always. The hash table fixes the order
//!   of its buckets, the cache fixes the order of its eviction, the heap fixes the order of its ties.
//! * **No panic on a valid input.** An empty input gives an empty structure. An input that cannot be
//!   handled (a capacity of zero) is reported as an `Err` with a readable reason.
//! * **The aggregates saturate.** The sums are computed with saturating arithmetic, so a value that
//!   does not fit in an `i64` stops at the bound instead of wrapping around (or panicking in a debug
//!   build).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

// ---- algorithm.datastructure.binary_heap -------------------------------------------------------------------------

/// A binary heap: a complete binary tree kept in an array, where a parent is never smaller than its
/// children.
///
/// The heap is the answer to "give me the largest thing again and again". It does not keep the
/// elements in order: it only guarantees that the root is the largest of them.
#[derive(Debug)]
pub struct BinaryHeap<T> {
    items: Vec<T>,
}

impl<T> Default for BinaryHeap<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T: Ord + Clone> BinaryHeap<T> {
    /// Builds a heap of the values, in `O(n)`.
    ///
    /// The heap is built from the bottom up: every leaf is already a heap, and the internal nodes are
    /// sifted down one after the other.
    pub fn from_slice(values: &[T]) -> Self {
        let mut heap = Self {
            items: values.to_vec(),
        };
        heap.heapify();
        heap
    }
}

impl<T: Ord> BinaryHeap<T> {
    /// An empty heap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the heap from the bottom up.
    fn heapify(&mut self) {
        if self.items.len() < 2 {
            return;
        }
        for index in (0..self.items.len() / 2).rev() {
            self.sift_down(index);
        }
    }

    /// Moves the value at the index up to its place, while it is larger than its parent.
    fn sift_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if self.items[index] > self.items[parent] {
                self.items.swap(index, parent);
                index = parent;
            } else {
                break;
            }
        }
    }

    /// Moves the value at the index down to its place, while it is smaller than one of its children.
    fn sift_down(&mut self, mut index: usize) {
        let length = self.items.len();
        loop {
            let left = 2 * index + 1;
            if left >= length {
                break;
            }
            let right = left + 1;
            let mut largest = left;
            if right < length && self.items[right] > self.items[left] {
                largest = right;
            }
            if self.items[index] >= self.items[largest] {
                break;
            }
            self.items.swap(index, largest);
            index = largest;
        }
    }

    /// Puts a value in the heap.
    pub fn push(&mut self, value: T) {
        self.items.push(value);
        let last = self.items.len() - 1;
        self.sift_up(last);
    }

    /// Removes the largest value of the heap, when there is one.
    pub fn pop(&mut self) -> Option<T> {
        if self.items.is_empty() {
            return None;
        }
        let last = self.items.len() - 1;
        self.items.swap(0, last);
        let value = self.items.pop();
        if !self.items.is_empty() {
            self.sift_down(0);
        }
        value
    }

    /// The largest value of the heap, when there is one.
    pub fn peek(&self) -> Option<&T> {
        self.items.first()
    }

    /// The number of values in the heap.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the heap holds no value.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The values of the heap in the descending order, the largest one first.
    pub fn into_sorted_vec(mut self) -> Vec<T> {
        let mut sorted = Vec::with_capacity(self.items.len());
        while let Some(value) = self.pop() {
            sorted.push(value);
        }
        sorted
    }

    /// The values of the heap as they are laid out in the array.
    ///
    /// The array is not sorted: only the root is the largest of the values.
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }
}

/// Builds a binary heap of the values.
///
/// The heap is the structure of the priority queue: the largest value is always at the root, and
/// taking it out and putting a new one in both cost `O(log n)`.
///
/// # Input
///
/// - `values` are the values of the heap. They are compared with `Ord`, so the order has to be a
///   total order. A floating point number that holds a `NaN` breaks that order and breaks the heap
///   with it: the card says so.
/// - An empty list is legal: the result is an empty heap.
///
/// # Complexity
///
/// - Build: `O(n)` - the heap is built from the bottom up.
/// - Insert and pop: `O(log n)`. Take the top: `O(1)`.
/// - Extra space: `O(n)`.
///
/// # When it does not apply
///
/// - The card rejects it when all the elements have to be sorted: the heap only guarantees the order
///   of its root, and a sort is the answer.
/// - The card rejects it when a single maximum is needed and the data does not change: one scan is
///   simpler.
/// - The card rejects it when a query is by range or by prefix: the heap does not answer it.
/// - The card rejects it when the data is small: a vector and a sort cost less.
///
/// * asset: `algorithm.datastructure.binary_heap`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.datastructure.binary_heap/card.md`
pub fn binary_heap<T: Ord + Clone>(values: &[T]) -> BinaryHeap<T> {
    BinaryHeap::from_slice(values)
}

// ---- algorithm.datastructure.segment_tree ------------------------------------------------------------------------

/// A segment tree: the array is cut in halves until every piece holds one element, and every node
/// holds the sum of its own piece.
///
/// The tree answers a range query by merging `O(log n)` nodes, and it takes the same time to change
/// one element or one range.
#[derive(Debug, Default)]
pub struct SegmentTree {
    tree: Vec<i64>,
    lazy: Vec<i64>,
    length: usize,
}

impl SegmentTree {
    /// Builds the tree of the values, in `O(n)`.
    pub fn new(values: &[i64]) -> Self {
        let length = values.len();
        let mut segment = Self {
            tree: vec![0; 4 * length + 4],
            lazy: vec![0; 4 * length + 4],
            length,
        };
        if length > 0 {
            segment.build(1, 0, length, values);
        }
        segment
    }

    /// Fills the tree from the leaves up.
    fn build(&mut self, node: usize, left: usize, right: usize, values: &[i64]) {
        if right - left == 1 {
            self.tree[node] = values[left];
            return;
        }
        let middle = left + (right - left) / 2;
        self.build(node * 2, left, middle, values);
        self.build(node * 2 + 1, middle, right, values);
        self.tree[node] = self.tree[node * 2].saturating_add(self.tree[node * 2 + 1]);
    }

    /// Adds the pending change of the node to the node itself and to its lazy mark.
    fn apply(&mut self, node: usize, length: usize, delta: i64) {
        let total = delta.saturating_mul(length as i64);
        self.tree[node] = self.tree[node].saturating_add(total);
        self.lazy[node] = self.lazy[node].saturating_add(delta);
    }

    /// Pushes the lazy mark of the node down to its children.
    fn push(&mut self, node: usize, left: usize, right: usize) {
        let delta = self.lazy[node];
        if delta == 0 || right - left == 1 {
            return;
        }
        let middle = left + (right - left) / 2;
        self.apply(node * 2, middle - left, delta);
        self.apply(node * 2 + 1, right - middle, delta);
        self.lazy[node] = 0;
    }

    /// The sum of the range, with the lazy marks of the ancestors carried along.
    fn query_range(
        &self,
        node: usize,
        left: usize,
        right: usize,
        query_left: usize,
        query_right: usize,
        carry: i64,
    ) -> i64 {
        if query_right <= left || right <= query_left {
            return 0;
        }
        if query_left <= left && right <= query_right {
            let pending = carry.saturating_mul((right - left) as i64);
            return self.tree[node].saturating_add(pending);
        }
        let carry = carry.saturating_add(self.lazy[node]);
        let middle = left + (right - left) / 2;
        let left_sum =
            self.query_range(node * 2, left, middle, query_left, query_right, carry);
        let right_sum =
            self.query_range(node * 2 + 1, middle, right, query_left, query_right, carry);
        left_sum.saturating_add(right_sum)
    }

    /// Adds the delta to the range.
    fn range_add(
        &mut self,
        node: usize,
        left: usize,
        right: usize,
        query_left: usize,
        query_right: usize,
        delta: i64,
    ) {
        if query_right <= left || right <= query_left {
            return;
        }
        if query_left <= left && right <= query_right {
            self.apply(node, right - left, delta);
            return;
        }
        self.push(node, left, right);
        let middle = left + (right - left) / 2;
        self.range_add(node * 2, left, middle, query_left, query_right, delta);
        self.range_add(node * 2 + 1, middle, right, query_left, query_right, delta);
        self.tree[node] = self.tree[node * 2].saturating_add(self.tree[node * 2 + 1]);
    }

    /// Writes the value at the index.
    fn set_point(&mut self, node: usize, left: usize, right: usize, index: usize, value: i64) {
        if right - left == 1 {
            self.tree[node] = value;
            self.lazy[node] = 0;
            return;
        }
        self.push(node, left, right);
        let middle = left + (right - left) / 2;
        if index < middle {
            self.set_point(node * 2, left, middle, index, value);
        } else {
            self.set_point(node * 2 + 1, middle, right, index, value);
        }
        self.tree[node] = self.tree[node * 2].saturating_add(self.tree[node * 2 + 1]);
    }

    /// The number of elements of the tree.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether the tree holds no element.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// The sum of the range `[left, right)`.
    ///
    /// - The range must be inside the array: a range that goes past the end is an `Err`.
    /// - An empty range (`left == right`) is legal and its sum is zero.
    pub fn query(&self, left: usize, right: usize) -> Result<i64, String> {
        if left > right {
            return Err(format!(
                "segment_tree: the range [{left}, {right}) is not a range: the end is before the start"
            ));
        }
        if right > self.length {
            return Err(format!(
                "segment_tree: the range [{left}, {right}) goes past the end of the array ({} elements)",
                self.length
            ));
        }
        if self.length == 0 || left == right {
            return Ok(0);
        }
        Ok(self.query_range(1, 0, self.length, left, right, 0))
    }

    /// Writes the value at the index.
    ///
    /// - The index must be inside the array: an index that is past the end is an `Err`.
    pub fn set(&mut self, index: usize, value: i64) -> Result<(), String> {
        if index >= self.length {
            return Err(format!(
                "segment_tree: the index {index} is past the end of the array ({} elements)",
                self.length
            ));
        }
        self.set_point(1, 0, self.length, index, value);
        Ok(())
    }

    /// Adds the delta to the range `[left, right)`.
    ///
    /// - The range must be inside the array: a range that goes past the end is an `Err`.
    /// - An empty range (`left == right`) is legal and changes nothing.
    pub fn add(&mut self, left: usize, right: usize, delta: i64) -> Result<(), String> {
        if left > right {
            return Err(format!(
                "segment_tree: the range [{left}, {right}) is not a range: the end is before the start"
            ));
        }
        if right > self.length {
            return Err(format!(
                "segment_tree: the range [{left}, {right}) goes past the end of the array ({} elements)",
                self.length
            ));
        }
        if self.length == 0 || left == right {
            return Ok(());
        }
        self.range_add(1, 0, self.length, left, right, delta);
        Ok(())
    }
}

/// Builds a segment tree of the values.
///
/// The tree cuts the array in halves until every piece holds one element, and every node holds the
/// sum of its own piece. A range query merges `O(log n)` nodes, and a change of one element or of
/// one range costs the same.
///
/// # Input
///
/// - `values` are the elements of the array. The aggregate is the sum, and the sum is an
///   associative operation: that is the constraint the card puts on the aggregate.
/// - An empty array is legal: the result is an empty tree.
///
/// # Complexity
///
/// - Build: `O(n)`. Query and update: `O(log n)`.
/// - Extra space: `O(n)` - about four slots per element, the lazy marks included.
///
/// # When it does not apply
///
/// - The card rejects it when only the prefixes are queried: a Fenwick tree is smaller and simpler.
/// - The card rejects it when the data never changes: a sparse table answers the queries in `O(1)`
///   after an `O(n log n)` build.
/// - The card rejects it when only one element is read: the array itself is the answer.
/// - The card rejects it when the aggregate is not associative: the merge of the nodes is not defined
///   and the tree does not hold.
/// - The card rejects it for the non-commutative aggregates: the order of the merge matters and has
///   to be respected by the implementation.
///
/// * asset: `algorithm.datastructure.segment_tree`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.datastructure.segment_tree/card.md`
pub fn segment_tree(values: &[i64]) -> SegmentTree {
    SegmentTree::new(values)
}

// ---- algorithm.datastructure.fenwick_tree ------------------------------------------------------------------------

/// A Fenwick tree (binary indexed tree): the sums of the prefixes, stored by the lowest set bit.
///
/// The tree answers a prefix sum and takes a point update in `O(log n)`, with `n` slots of memory.
#[derive(Debug, Default)]
pub struct FenwickTree {
    tree: Vec<i64>,
    length: usize,
}

impl FenwickTree {
    /// Builds the tree of the values, in `O(n)`.
    pub fn new(values: &[i64]) -> Self {
        let length = values.len();
        let mut tree = vec![0i64; length + 1];
        for (index, value) in values.iter().enumerate() {
            tree[index + 1] = *value;
        }
        // the linear build: every slot gives its sum to the slot that covers it. The low bit is
        // `index & index.wrapping_neg()`, which `isolate_lowest_one` computes without ever
        // negating the zero (that is the overflow the card warns about).
        for index in 1..=length {
            let parent = index + index.isolate_lowest_one();
            if parent <= length {
                tree[parent] = tree[parent].saturating_add(tree[index]);
            }
        }
        Self { tree, length }
    }

    /// The number of elements of the tree.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether the tree holds no element.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Adds the delta to the element at the index.
    ///
    /// - The index must be inside the array: an index that is past the end is an `Err`.
    pub fn add(&mut self, index: usize, delta: i64) -> Result<(), String> {
        if index >= self.length {
            return Err(format!(
                "fenwick_tree: the index {index} is past the end of the array ({} elements)",
                self.length
            ));
        }
        let mut cursor = index + 1;
        while cursor <= self.length {
            self.tree[cursor] = self.tree[cursor].saturating_add(delta);
            cursor += cursor.isolate_lowest_one();
        }
        Ok(())
    }

    /// The sum of the elements before the index `end`, that is the sum of `[0, end)`.
    ///
    /// - The index must be inside the array: an index that is past the end is an `Err`.
    pub fn prefix_sum(&self, end: usize) -> Result<i64, String> {
        if end > self.length {
            return Err(format!(
                "fenwick_tree: the index {end} is past the end of the array ({} elements)",
                self.length
            ));
        }
        let mut cursor = end;
        let mut sum = 0i64;
        while cursor > 0 {
            sum = sum.saturating_add(self.tree[cursor]);
            cursor -= cursor.isolate_lowest_one();
        }
        Ok(sum)
    }

    /// The sum of the range `[left, right)`.
    ///
    /// - The range must be inside the array: a range that goes past the end is an `Err`.
    /// - An empty range (`left == right`) is legal and its sum is zero.
    pub fn range_sum(&self, left: usize, right: usize) -> Result<i64, String> {
        if left > right {
            return Err(format!(
                "fenwick_tree: the range [{left}, {right}) is not a range: the end is before the start"
            ));
        }
        if right > self.length {
            return Err(format!(
                "fenwick_tree: the range [{left}, {right}) goes past the end of the array ({} elements)",
                self.length
            ));
        }
        let head = self.prefix_sum(right)?;
        let tail = self.prefix_sum(left)?;
        Ok(head.saturating_sub(tail))
    }
}

/// Builds a Fenwick tree of the values.
///
/// The tree stores the sums of the prefixes by the lowest set bit, so a prefix sum and a point update
/// both walk `O(log n)` steps, and the memory is `n` slots.
///
/// # Input
///
/// - `values` are the elements of the array. The aggregate is the sum, and the sum can be taken back
///   from the prefixes: that is the constraint the card puts on the aggregate.
/// - An empty array is legal: the result is an empty tree.
///
/// # Complexity
///
/// - Build: `O(n)`. Query and update: `O(log n)`.
/// - Extra space: `O(n)`.
///
/// # When it does not apply
///
/// - The card rejects it when the aggregate of a range is not a difference of two prefixes, the
///   maximum of a range for instance: the tree cannot answer it.
/// - The card rejects it when the ranges themselves have to be updated: it takes two trees (the
///   difference trick) or a segment tree.
/// - The card rejects it for a two dimensional range query: the tree of trees is hard to hold.
/// - The card rejects it when the array is rebuilt often: a plain prefix sum array is simpler.
///
/// * asset: `algorithm.datastructure.fenwick_tree`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.datastructure.fenwick_tree/card.md`
pub fn fenwick_tree(values: &[i64]) -> FenwickTree {
    FenwickTree::new(values)
}

// ---- algorithm.datastructure.lru_cache ---------------------------------------------------------------------------

/// A node of the cache: the key, the value and the two links of the list of the use order.
#[derive(Debug)]
struct LruNode<K, V> {
    key: K,
    value: V,
    previous: Option<usize>,
    next: Option<usize>,
}

/// A least recently used cache: a hash table finds the entry, a doubly linked list keeps the order
/// of the uses, and the two together make every read and every write `O(1)`.
///
/// The list is kept in an arena of slots and the links are the indexes of the slots, so the cache
/// needs no raw pointer and no unsafe code.
#[derive(Debug)]
pub struct LruCache<K: Hash + Eq + Clone, V> {
    capacity: usize,
    index: HashMap<K, usize>,
    slots: Vec<Option<LruNode<K, V>>>,
    free: Vec<usize>,
    head: Option<usize>,
    tail: Option<usize>,
}

impl<K: Hash + Eq + Clone, V> LruCache<K, V> {
    /// Builds a cache of the given capacity.
    ///
    /// - The capacity must be at least 1: a cache that holds nothing cannot answer anything, so the
    ///   capacity of zero is reported as an `Err`.
    pub fn new(capacity: usize) -> Result<Self, String> {
        if capacity == 0 {
            return Err(
                "lru_cache: the capacity must be at least 1; a cache of capacity 0 holds nothing"
                    .to_owned(),
            );
        }
        Ok(Self {
            capacity,
            index: HashMap::new(),
            slots: Vec::new(),
            free: Vec::new(),
            head: None,
            tail: None,
        })
    }

    /// Takes the node out of the list of the use order.
    fn unlink(&mut self, slot: usize) {
        let (previous, next) = match &self.slots[slot] {
            Some(node) => (node.previous, node.next),
            None => return,
        };
        match previous {
            Some(previous) => {
                if let Some(node) = self.slots[previous].as_mut() {
                    node.next = next;
                }
            }
            None => self.head = next,
        }
        match next {
            Some(next) => {
                if let Some(node) = self.slots[next].as_mut() {
                    node.previous = previous;
                }
            }
            None => self.tail = previous,
        }
        if let Some(node) = self.slots[slot].as_mut() {
            node.previous = None;
            node.next = None;
        }
    }

    /// Puts the node at the head of the list of the use order.
    fn push_front(&mut self, slot: usize) {
        let head = self.head;
        if let Some(node) = self.slots[slot].as_mut() {
            node.previous = None;
            node.next = head;
        }
        if let Some(head) = head
            && let Some(node) = self.slots[head].as_mut()
        {
            node.previous = Some(slot);
        }
        self.head = Some(slot);
        if self.tail.is_none() {
            self.tail = Some(slot);
        }
    }

    /// The value of the key, and the key becomes the most recently used one.
    ///
    /// A read of the cache is a write too: that is what the card points at, and it is why a cache
    /// that is shared between threads needs a lock.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let slot = *self.index.get(key)?;
        self.unlink(slot);
        self.push_front(slot);
        self.slots[slot].as_ref().map(|node| &node.value)
    }

    /// The value of the key, without touching the order of the uses.
    pub fn peek(&self, key: &K) -> Option<&V> {
        let slot = *self.index.get(key)?;
        self.slots[slot].as_ref().map(|node| &node.value)
    }

    /// Puts a value in the cache and returns the value it replaces, when there was one.
    ///
    /// The least recently used entry is evicted when the cache is full.
    pub fn put(&mut self, key: K, value: V) -> Option<V> {
        if let Some(&slot) = self.index.get(&key) {
            let mut node = self.slots[slot].take()?;
            let old = std::mem::replace(&mut node.value, value);
            self.slots[slot] = Some(node);
            self.unlink(slot);
            self.push_front(slot);
            return Some(old);
        }

        let node = LruNode {
            key: key.clone(),
            value,
            previous: None,
            next: None,
        };
        let slot = match self.free.pop() {
            Some(free) => {
                self.slots[free] = Some(node);
                free
            }
            None => {
                self.slots.push(Some(node));
                self.slots.len() - 1
            }
        };
        self.index.insert(key, slot);
        self.push_front(slot);

        if self.index.len() > self.capacity {
            self.evict();
        }
        None
    }

    /// Removes the entry of the key and returns its value, when there was one.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let slot = *self.index.get(key)?;
        self.unlink(slot);
        let node = self.slots[slot].take();
        self.index.remove(key);
        self.free.push(slot);
        node.map(|node| node.value)
    }

    /// Evicts the least recently used entry.
    fn evict(&mut self) {
        let Some(tail) = self.tail else {
            return;
        };
        self.unlink(tail);
        let node = self.slots[tail].take();
        if let Some(node) = node {
            self.index.remove(&node.key);
        }
        self.free.push(tail);
    }

    /// The capacity of the cache.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The number of entries of the cache.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the cache holds no entry.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The keys of the cache, the most recently used one first.
    pub fn keys_by_recency(&self) -> Vec<&K> {
        let mut keys = Vec::with_capacity(self.index.len());
        let mut cursor = self.head;
        while let Some(slot) = cursor {
            if let Some(node) = self.slots[slot].as_ref() {
                keys.push(&node.key);
            }
            cursor = self.slots[slot].as_ref().and_then(|node| node.next);
        }
        keys
    }
}

/// Builds a least recently used cache of the given capacity.
///
/// The cache finds its entries with a hash table and keeps the order of the uses with a doubly
/// linked list, so a read, a write and an eviction all cost `O(1)`.
///
/// # Input
///
/// - The capacity must be at least 1: a cache of capacity 0 holds nothing and answers nothing, so it
///   is reported as an `Err`.
/// - The keys must be hashable and comparable, and the two must agree: a key whose hash and whose
///   equality disagree is never found.
///
/// # Complexity
///
/// - Read, write and eviction: `O(1)`.
/// - Extra space: `O(capacity)` - the entries and their links.
///
/// # When it does not apply
///
/// - The card rejects it when the hit rate does not matter: not caching at all, or a simple FIFO,
///   is enough.
/// - The card rejects it when the entries have to be evicted by their frequency: a LFU or an ARC is
///   the answer.
/// - The card rejects it when the entries are big and their number is not known: only the indexes
///   should be cached, or a disk layer should be added.
/// - The card rejects it when the entries expire: the LRU has no notion of time, the expiry has to be
///   kept aside.
///
/// * asset: `algorithm.datastructure.lru_cache`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.datastructure.lru_cache/card.md`
pub fn lru_cache<K: Hash + Eq + Clone, V>(capacity: usize) -> Result<LruCache<K, V>, String> {
    LruCache::new(capacity)
}

// ---- algorithm.datastructure.hash_table --------------------------------------------------------------------------

/// A key and the value it carries in the table.
pub type HashTableEntry<K, V> = (K, V);

/// The hash of the keys: FNV-1a over the bytes the `Hash` implementation writes.
///
/// The seed is fixed, so the hash of a key is the same in every run and the order of the buckets is
/// the same too. The card asks for it: the hash of a key must be constant.
struct FixedHasher(u64);

impl Hasher for FixedHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// The hash of a key, with the fixed seed.
fn fixed_hash<K: Hash>(key: &K) -> u64 {
    let mut hasher = FixedHasher(0xcbf2_9ce4_8422_2325);
    key.hash(&mut hasher);
    hasher.finish()
}

/// A hash table: the keys are hashed into buckets, and a bucket holds the entries whose keys
/// collide.
///
/// The table grows when the load factor passes three quarters, so the chains stay short. The order of
/// the buckets is fixed by the hash and the order inside a bucket is the order of the insertions, so
/// the walk of the table is always the same.
#[derive(Debug)]
pub struct HashTable<K, V> {
    buckets: Vec<Vec<(K, V)>>,
    length: usize,
}

impl<K, V> Default for HashTable<K, V> {
    fn default() -> Self {
        Self {
            buckets: vec![Vec::new()],
            length: 0,
        }
    }
}

impl<K: Hash + Eq, V> HashTable<K, V> {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty table with the given number of buckets.
    pub fn with_capacity(buckets: usize) -> Self {
        Self {
            buckets: (0..buckets.max(1)).map(|_| Vec::new()).collect(),
            length: 0,
        }
    }

    /// The bucket of the key.
    fn bucket_of(&self, key: &K) -> usize {
        (fixed_hash(key) as usize) % self.buckets.len()
    }

    /// Doubles the number of buckets and puts the entries back, following the new hash.
    fn grow(&mut self) {
        let count = self.buckets.len() * 2;
        let mut buckets: Vec<Vec<(K, V)>> = (0..count).map(|_| Vec::new()).collect();
        for bucket in std::mem::take(&mut self.buckets) {
            for (key, value) in bucket {
                let index = (fixed_hash(&key) as usize) % count;
                buckets[index].push((key, value));
            }
        }
        self.buckets = buckets;
    }

    /// Puts a value in the table and returns the value it replaces, when there was one.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let index = self.bucket_of(&key);
        for entry in self.buckets[index].iter_mut() {
            if entry.0 == key {
                return Some(std::mem::replace(&mut entry.1, value));
            }
        }
        self.buckets[index].push((key, value));
        self.length += 1;
        if self.length * 4 > self.buckets.len() * 3 {
            self.grow();
        }
        None
    }

    /// The value of the key, when the key is in the table.
    pub fn get(&self, key: &K) -> Option<&V> {
        let index = self.bucket_of(key);
        self.buckets[index]
            .iter()
            .find(|entry| &entry.0 == key)
            .map(|entry| &entry.1)
    }

    /// The value of the key, for a write in place.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let index = self.bucket_of(key);
        self.buckets[index]
            .iter_mut()
            .find(|entry| &entry.0 == key)
            .map(|entry| &mut entry.1)
    }

    /// Removes the entry of the key and returns its value, when there was one.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let index = self.bucket_of(key);
        let position = self.buckets[index].iter().position(|entry| &entry.0 == key)?;
        self.length -= 1;
        Some(self.buckets[index].remove(position).1)
    }

    /// Whether the key is in the table.
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// The number of entries of the table.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether the table holds no entry.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// The number of buckets of the table.
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// The number of entries per bucket.
    pub fn load_factor(&self) -> f64 {
        if self.buckets.is_empty() {
            return 0.0;
        }
        self.length as f64 / self.buckets.len() as f64
    }

    /// The entries of the table, in the order of the buckets and, inside a bucket, in the order of
    /// the insertions.
    ///
    /// The order is fixed on purpose: the hash has a fixed seed, so the same keys always give the
    /// same order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.buckets
            .iter()
            .flat_map(|bucket| bucket.iter().map(|(key, value)| (key, value)))
    }
}

/// Builds a hash table of the entries.
///
/// The keys are hashed into buckets and the collisions are chained, so a lookup is one hash and a
/// walk of the chain.
///
/// # Input
///
/// - `entries` are the keys and the values they carry. The same key twice keeps the value of the
///   last occurrence.
/// - The keys must be hashable and comparable, and the two must agree: a key whose hash and whose
///   equality disagree is never found. A key that is a floating point number does not satisfy that
///   contract (`NaN` is not equal to itself), so it cannot be a key of this table.
/// - An empty list of entries is legal: the result is an empty table.
///
/// # Complexity
///
/// - Every operation: `O(1)` in expectation, `O(n)` in the worst case (when every key lands in the
///   same bucket).
/// - Extra space: `O(n)`.
///
/// # When it does not apply
///
/// - The card rejects it when the entries have to be walked in the order of the keys: an ordered
///   structure (a B tree, a skip list) is the answer.
/// - The card rejects it when there are few keys: a linear scan or an ordered array is simpler.
/// - The card rejects it when the keys come from an attacker and the hash can be predicted: a salted
///   hash is the answer.
/// - The card rejects it when the hash of the keys is expensive: the cost of the hash can eat the
///   benefit of the table.
///
/// * asset: `algorithm.datastructure.hash_table`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.datastructure.hash_table/card.md`
pub fn hash_table<K: Hash + Eq + Clone, V: Clone>(
    entries: &[HashTableEntry<K, V>],
) -> HashTable<K, V> {
    let mut table = HashTable::with_capacity(entries.len());
    for (key, value) in entries {
        table.insert(key.clone(), value.clone());
    }
    table
}

// ---- algorithm.datastructure.monotonic_stack --------------------------------------------------------------------

/// Finds, for every element, the index of the first element on its right that is strictly greater.
///
/// The stack holds the indexes whose answer is not known yet, and the values of the stack are
/// decreasing: when a value arrives, it answers every index of the stack whose value is smaller.
///
/// # Input
///
/// - `values` are the elements. They are compared with `Ord`, and an element that holds a `NaN`
///   breaks the order and the stack with it: the card says so.
/// - An empty array is legal: the result is an empty array.
/// - The convention is fixed: the answer is the first **strictly** greater element. An element that
///   is equal to the one that is looked for is not an answer, and that is the convention the card
///   asks to be written down.
///
/// # Complexity
///
/// - Time: `O(n)` in total - every element goes in the stack once and comes out once.
/// - Extra space: `O(n)` - the stack, which is at its deepest when the values are sorted.
///
/// # When it does not apply
///
/// - The card rejects it when only the maximum of the array is needed: one scan is enough.
/// - The card rejects it when the history has to be changed or deleted: the rule of the stack becomes
///   complicated.
/// - The card rejects it when the values have to be compared and not the indexes: the convention of
///   the equality has to be written down, and it is here (strictly greater).
/// - The card rejects it when the data is small: the brute force is enough.
///
/// * asset: `algorithm.datastructure.monotonic_stack`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.datastructure.monotonic_stack/card.md`
pub fn monotonic_stack(values: &[i64]) -> Vec<Option<usize>> {
    let mut answers: Vec<Option<usize>> = vec![None; values.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (index, value) in values.iter().enumerate() {
        while let Some(&top) = stack.last() {
            if values[top] < *value {
                answers[top] = Some(index);
                stack.pop();
            } else {
                break;
            }
        }
        stack.push(index);
    }
    answers
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sum of a range, computed the slow way.
    fn slow_range_sum(values: &[i64], left: usize, right: usize) -> i64 {
        values[left..right].iter().sum()
    }

    // ---- algorithm.datastructure.binary_heap

    #[test]
    fn binary_heap_pops_the_values_in_descending_order() {
        let mut heap = binary_heap(&[3, 1, 4, 1, 5, 9, 2, 6]);
        assert_eq!(heap.len(), 8);
        assert_eq!(heap.peek(), Some(&9));
        let mut popped = Vec::new();
        while let Some(value) = heap.pop() {
            popped.push(value);
        }
        // the duplicates are kept, the order is the descending one
        assert_eq!(popped, vec![9, 6, 5, 4, 3, 2, 1, 1]);
        // the empty heap gives nothing and does not panic
        let mut empty: BinaryHeap<i32> = BinaryHeap::new();
        assert!(empty.is_empty());
        assert_eq!(empty.pop(), None);
        assert_eq!(empty.peek(), None);
        // a value that is pushed after the build takes its place
        heap.push(7);
        assert_eq!(heap.peek(), Some(&7));
        assert_eq!(heap.pop(), Some(7));
        // the single element
        let mut single = binary_heap(&[42]);
        assert_eq!(single.peek(), Some(&42));
        assert_eq!(single.pop(), Some(42));
        assert!(single.is_empty());
        // the maximum of the heap is the maximum of a scan
        let values = [3, 1, 4, 1, 5, 9, 2, 6];
        assert_eq!(
            binary_heap(&values).peek(),
            Some(values.iter().max().unwrap())
        );
    }

    #[test]
    fn binary_heap_keeps_only_the_root_in_order() {
        // the counterexample of the card: the heap does not give the elements in order, only the
        // root is the largest of them
        let heap = binary_heap(&[1, 2, 3, 4, 5]);
        assert!(!heap.as_slice().is_sorted());
        assert_eq!(heap.peek(), Some(&5));
        // the sorted vector is the descending one
        assert_eq!(heap.into_sorted_vec(), vec![5, 4, 3, 2, 1]);
    }

    // ---- algorithm.datastructure.segment_tree

    #[test]
    fn segment_tree_answers_the_range_sums_after_the_updates() {
        let mut values = vec![1, 2, 3, 4, 5];
        let mut tree = segment_tree(&values);
        assert_eq!(tree.query(0, 5).unwrap(), 15);
        assert_eq!(tree.query(1, 3).unwrap(), 5);
        assert_eq!(tree.query(0, 1).unwrap(), 1);
        // the empty range has the sum of nothing
        assert_eq!(tree.query(2, 2).unwrap(), 0);
        // the point update
        tree.set(2, 10).unwrap();
        values[2] = 10;
        assert_eq!(tree.query(0, 5).unwrap(), 22);
        // the range update, with the lazy mark
        tree.add(0, 5, 5).unwrap();
        for value in values.iter_mut() {
            *value += 5;
        }
        assert_eq!(tree.query(0, 5).unwrap(), 47);
        assert_eq!(tree.query(0, 2).unwrap(), 13);
        assert_eq!(tree.query(4, 5).unwrap(), 10);
        // every range agrees with the slow computation
        for left in 0..values.len() {
            for right in left..=values.len() {
                assert_eq!(
                    tree.query(left, right).unwrap(),
                    slow_range_sum(&values, left, right)
                );
            }
        }
    }

    #[test]
    fn segment_tree_rejects_the_ranges_that_are_out_of_the_array() {
        let mut tree = segment_tree(&[1, 2, 3]);
        assert!(tree.query(0, 4).is_err());
        assert!(tree.query(2, 1).is_err());
        assert!(tree.add(0, 4, 1).is_err());
        assert!(tree.set(3, 1).is_err());
        // the empty tree only answers the empty range
        let mut empty = segment_tree(&[]);
        assert!(empty.is_empty());
        assert_eq!(empty.query(0, 0).unwrap(), 0);
        assert!(empty.query(0, 1).is_err());
        assert!(empty.add(0, 1, 1).is_err());
    }

    // ---- algorithm.datastructure.fenwick_tree

    #[test]
    fn fenwick_tree_answers_the_prefix_and_range_sums() {
        let mut values = vec![1, 2, 3, 4, 5];
        let mut tree = fenwick_tree(&values);
        assert_eq!(tree.prefix_sum(5).unwrap(), 15);
        assert_eq!(tree.prefix_sum(0).unwrap(), 0);
        assert_eq!(tree.prefix_sum(3).unwrap(), 6);
        assert_eq!(tree.range_sum(1, 4).unwrap(), 9);
        assert_eq!(tree.range_sum(2, 2).unwrap(), 0);
        // the point update
        tree.add(2, 10).unwrap();
        values[2] += 10;
        assert_eq!(tree.prefix_sum(5).unwrap(), 25);
        assert_eq!(tree.prefix_sum(3).unwrap(), 16);
        // a negative delta
        tree.add(0, -1).unwrap();
        values[0] -= 1;
        assert_eq!(tree.prefix_sum(5).unwrap(), 24);
        // every prefix agrees with the slow computation
        for end in 0..=values.len() {
            assert_eq!(
                tree.prefix_sum(end).unwrap(),
                values[..end].iter().sum::<i64>()
            );
        }
    }

    #[test]
    fn fenwick_tree_rejects_the_indices_out_of_the_array() {
        let mut tree = fenwick_tree(&[1, 2, 3]);
        assert!(tree.add(3, 1).is_err());
        assert!(tree.prefix_sum(4).is_err());
        assert!(tree.range_sum(0, 4).is_err());
        assert!(tree.range_sum(2, 1).is_err());
        // the empty tree answers the empty prefix only
        let mut empty = fenwick_tree(&[]);
        assert!(empty.is_empty());
        assert_eq!(empty.prefix_sum(0).unwrap(), 0);
        assert!(empty.add(0, 1).is_err());
    }

    // ---- algorithm.datastructure.lru_cache

    #[test]
    fn lru_cache_evicts_the_least_recently_used_entry() {
        let mut cache: LruCache<&str, i32> = lru_cache(2).unwrap();
        assert_eq!(cache.capacity(), 2);
        assert!(cache.is_empty());
        assert_eq!(cache.put("a", 1), None);
        assert_eq!(cache.put("b", 2), None);
        assert_eq!(cache.len(), 2);
        // the read of "a" makes it the most recently used one, so "b" is the one that leaves
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.put("c", 3), None);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.get(&"c"), Some(&3));
        // the order of the uses is fixed
        assert_eq!(cache.keys_by_recency(), vec![&"c", &"a"]);
        // writing an existing key does not grow the cache
        assert_eq!(cache.put("c", 30), Some(3));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&"c"), Some(&30));
        // the peek does not touch the order: "b" was written last, so it is still the most
        // recently used one
        let mut cache: LruCache<&str, i32> = lru_cache(2).unwrap();
        cache.put("a", 1);
        cache.put("b", 2);
        assert_eq!(cache.peek(&"a"), Some(&1));
        assert_eq!(cache.keys_by_recency(), vec![&"b", &"a"]);
        // the removal takes the entry out
        assert_eq!(cache.remove(&"a"), Some(1));
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn lru_cache_rejects_a_capacity_of_zero() {
        // the card lists the capacity of zero as an extreme input: the cache would hold nothing, so
        // it is reported instead of being built
        let error = lru_cache::<&str, i32>(0).unwrap_err();
        assert!(error.contains("capacity"), "the reason must be readable: {error}");
        // the capacity of one is legal
        let mut cache: LruCache<&str, i32> = lru_cache(1).unwrap();
        cache.put("a", 1);
        cache.put("b", 2);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), Some(&2));
    }

    // ---- algorithm.datastructure.hash_table

    #[test]
    fn hash_table_inserts_finds_and_removes_the_keys() {
        let mut table: HashTable<String, u32> = hash_table(&[]);
        assert!(table.is_empty());
        // the many keys make the table grow several times
        for index in 0..1_000u32 {
            assert_eq!(table.insert(format!("key{index}"), index), None);
        }
        assert_eq!(table.len(), 1_000);
        assert!(table.bucket_count() > 1);
        assert!(table.load_factor() <= 0.75);
        for index in 0..1_000u32 {
            assert_eq!(table.get(&format!("key{index}")), Some(&index));
            assert!(table.contains_key(&format!("key{index}")));
        }
        // a key that is not there
        assert_eq!(table.get(&"absent".to_owned()), None);
        // the overwrite returns the old value
        assert_eq!(table.insert("key0".to_owned(), 42), Some(0));
        assert_eq!(table.get(&"key0".to_owned()), Some(&42));
        assert_eq!(table.len(), 1_000);
        // the removal
        assert_eq!(table.remove(&"key0".to_owned()), Some(42));
        assert_eq!(table.get(&"key0".to_owned()), None);
        assert_eq!(table.len(), 999);
    }

    #[test]
    fn hash_table_walks_the_entries_in_a_fixed_order() {
        let entries: Vec<HashTableEntry<&str, u32>> = vec![
            ("delta", 1),
            ("alpha", 2),
            ("charlie", 3),
            ("bravo", 4),
        ];
        let table = hash_table(&entries);
        let first: Vec<&str> = table.iter().map(|(key, _)| *key).collect();
        let second: Vec<&str> = table.iter().map(|(key, _)| *key).collect();
        // the walk is the same every time
        assert_eq!(first, second);
        // and it holds every key
        assert_eq!(first.len(), 4);
        for (key, _) in &entries {
            assert!(first.contains(key));
        }
        // the counterexample of the card: the table does not give the keys in order
        assert!(!first.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    // ---- algorithm.datastructure.monotonic_stack

    #[test]
    fn monotonic_stack_finds_the_next_strictly_greater_element() {
        assert_eq!(
            monotonic_stack(&[2, 1, 4, 3]),
            vec![Some(2), Some(2), None, None]
        );
        assert_eq!(monotonic_stack(&[5, 4, 3, 2, 1]), vec![None; 5]);
        assert_eq!(
            monotonic_stack(&[1, 2, 3]),
            vec![Some(1), Some(2), None]
        );
        // the empty array and the single element
        assert!(monotonic_stack(&[]).is_empty());
        assert_eq!(monotonic_stack(&[7]), vec![None]);
        // the answer is the answer of the brute force
        let values = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5];
        let answers = monotonic_stack(&values);
        for (index, value) in values.iter().enumerate() {
            let expected = values[index + 1..]
                .iter()
                .position(|other| other > value)
                .map(|offset| index + 1 + offset);
            assert_eq!(answers[index], expected);
        }
    }

    #[test]
    fn monotonic_stack_answers_nothing_for_the_equal_elements() {
        // the convention of the card: the answer is the first strictly greater element, so the equal
        // ones are not an answer
        assert_eq!(monotonic_stack(&[1, 1, 1]), vec![None; 3]);
        assert_eq!(
            monotonic_stack(&[2, 2, 3]),
            vec![Some(2), Some(2), None]
        );
        // the counterexample of the card: when only the maximum is needed, one scan is enough
        assert_eq!(monotonic_stack(&[5, 1, 9]).iter().copied().flatten().max(), Some(2));
    }
}
