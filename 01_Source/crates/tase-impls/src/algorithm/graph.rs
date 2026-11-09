//! The nine graph algorithms of the catalogue (the assets `algorithm.graph.*`).
//!
//! Every public function of this module names its asset in its doc comment with the
//! two marker lines `* asset:` and `* card:`; the registration script reads those
//! markers to wire the code to the catalogue.
//!
//! # The graph type
//!
//! All the algorithms of this module work on the same small type, [`Graph`]: a node
//! count, a directedness flag, the list of the edges and the adjacency lists. A node
//! is a `usize` index in `0..node_count()`. The adjacency list of a node is kept
//! sorted by `(target, weight)`, so the order in which an algorithm walks the
//! neighbours is a function of the *set* of edges and not of the order the edges were
//! added in.
//!
//! * [`Graph::add_edge`] adds an edge of weight `1`: the unweighted form, used by the
//!   algorithms that count edges ([`bfs`], [`dfs`], [`topological_sort`]).
//! * [`Graph::add_weighted_edge`] adds an edge with an explicit weight: the form used
//!   by the algorithms that minimize a cost ([`dijkstra`], [`bellman_ford`],
//!   [`floyd_warshall`], [`kruskal`], [`prim`]).
//!
//! # The errors
//!
//! A valid input never panics. An input the algorithm cannot handle - a negative
//! weight where the algorithm forbids it, a cycle in a graph that has to be acyclic, a
//! directed graph where a spanning tree is asked for - is reported as an [`Err`] that
//! says why.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::fmt;

/// The error every algorithm of this module reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// A node index was outside `0..node_count`.
    NodeOutOfRange {
        /// The offending node index.
        node: usize,
        /// The number of nodes of the graph.
        node_count: usize,
    },
    /// An edge had a negative weight and the algorithm forbids negative weights.
    NegativeWeight {
        /// The node the edge starts at.
        from: usize,
        /// The node the edge ends at.
        to: usize,
        /// The weight of the edge.
        weight: i64,
    },
    /// An edge had a weight other than `1` and the algorithm only works on an
    /// unweighted graph.
    NonUnitWeight {
        /// The node the edge starts at.
        from: usize,
        /// The node the edge ends at.
        to: usize,
        /// The weight of the edge.
        weight: i64,
    },
    /// The graph is directed and the algorithm only works on an undirected graph.
    DirectedGraphUnsupported {
        /// The name of the algorithm that refused the graph.
        algorithm: &'static str,
    },
    /// The graph is undirected and the algorithm only works on a directed graph.
    UndirectedGraphUnsupported {
        /// The name of the algorithm that refused the graph.
        algorithm: &'static str,
    },
    /// The graph has a cycle and the algorithm needs a directed acyclic graph.
    CycleDetected {
        /// The number of nodes that are not part of the order, that is the nodes that
        /// are on or behind a cycle.
        remaining_nodes: usize,
    },
    /// The graph has a cycle whose total weight is negative.
    NegativeCycleDetected,
    /// The graph is not connected and the algorithm needs a spanning tree.
    DisconnectedGraph {
        /// The number of connected components of the graph.
        components: usize,
    },
    /// A cost did not fit into an `i64`.
    ArithmeticOverflow,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeOutOfRange { node, node_count } => {
                write!(f, "the node {node} is outside the graph, which has {node_count} nodes")
            }
            Self::NegativeWeight { from, to, weight } => {
                write!(f, "the edge {from} -> {to} has the negative weight {weight}")
            }
            Self::NonUnitWeight { from, to, weight } => write!(
                f,
                "the edge {from} -> {to} has the weight {weight}, and this algorithm only works on an unweighted graph"
            ),
            Self::DirectedGraphUnsupported { algorithm } => {
                write!(f, "{algorithm} only works on an undirected graph")
            }
            Self::UndirectedGraphUnsupported { algorithm } => {
                write!(f, "{algorithm} only works on a directed graph")
            }
            Self::CycleDetected { remaining_nodes } => write!(
                f,
                "the graph has a cycle, {remaining_nodes} nodes cannot be ordered"
            ),
            Self::NegativeCycleDetected => {
                write!(f, "the graph has a cycle of negative total weight, so the shortest paths are not defined")
            }
            Self::DisconnectedGraph { components } => write!(
                f,
                "the graph is not connected, it has {components} components, so it has no spanning tree"
            ),
            Self::ArithmeticOverflow => write!(f, "the cost did not fit into an i64"),
        }
    }
}

impl std::error::Error for GraphError {}

/// An edge of a [`Graph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    /// The node the edge starts at.
    pub from: usize,
    /// The node the edge ends at.
    pub to: usize,
    /// The weight of the edge.
    pub weight: i64,
}

/// A small directed or undirected graph with weighted edges.
///
/// The graph is an adjacency list: `node_count()` nodes numbered from `0`, a list of
/// the edges in the order they were added, and the adjacency list of every node kept
/// sorted by `(target, weight)`. The sorted adjacency list is what makes every
/// traversal of this module deterministic: the same set of edges always produces the
/// same visiting order, whatever order the edges were added in.
#[derive(Debug, Clone)]
pub struct Graph {
    node_count: usize,
    directed: bool,
    edges: Vec<Edge>,
    adjacency: Vec<Vec<(usize, i64)>>,
}

impl Graph {
    /// Creates a graph of `node_count` nodes and no edge.
    ///
    /// The nodes are the indices `0..node_count`; an edge may only use those.
    ///
    /// # Example
    ///
    /// ```
    /// use tase_impls::algorithm::graph::Graph;
    ///
    /// let mut graph = Graph::new(3, false);
    /// graph.add_edge(0, 1).unwrap();
    /// graph.add_edge(1, 2).unwrap();
    /// assert_eq!(graph.node_count(), 3);
    /// assert_eq!(graph.edge_count(), 2);
    /// ```
    pub fn new(node_count: usize, directed: bool) -> Self {
        Self {
            node_count,
            directed,
            edges: Vec::new(),
            adjacency: vec![Vec::new(); node_count],
        }
    }

    /// The number of nodes of the graph.
    pub fn node_count(&self) -> usize {
        self.node_count
    }

    /// Whether the edges of the graph are directed.
    ///
    /// The algorithms that need a directed graph ([`topological_sort`]) refuse an
    /// undirected one, and the algorithms that need an undirected graph
    /// ([`kruskal`], [`prim`]) refuse a directed one: a spanning tree of a directed
    /// graph is not defined.
    pub fn is_directed(&self) -> bool {
        self.directed
    }

    /// The edges of the graph, in the order they were added.
    ///
    /// An undirected edge appears once, as it was added; the adjacency list of a node
    /// holds both directions.
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// The number of edges of the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// The neighbours of a node, sorted by `(target, weight)`.
    ///
    /// The weight of a neighbour is the weight of the edge that leads to it.
    pub fn neighbors(&self, node: usize) -> Result<&[(usize, i64)], GraphError> {
        if node >= self.node_count {
            return Err(GraphError::NodeOutOfRange {
                node,
                node_count: self.node_count,
            });
        }
        Ok(&self.adjacency[node])
    }

    /// Adds an edge of the weight `1`, that is an unweighted edge.
    ///
    /// The edge is directed when the graph is directed and it is symmetric when the
    /// graph is undirected.
    ///
    /// # Errors
    ///
    /// [`GraphError::NodeOutOfRange`] when one of the two nodes is outside the graph.
    pub fn add_edge(&mut self, from: usize, to: usize) -> Result<(), GraphError> {
        self.add_weighted_edge(from, to, 1)
    }

    /// Adds an edge of the given weight.
    ///
    /// The edge is directed when the graph is directed and it is symmetric when the
    /// graph is undirected. A self loop is accepted and is never part of a spanning
    /// tree.
    ///
    /// # Errors
    ///
    /// [`GraphError::NodeOutOfRange`] when one of the two nodes is outside the graph.
    pub fn add_weighted_edge(
        &mut self,
        from: usize,
        to: usize,
        weight: i64,
    ) -> Result<(), GraphError> {
        if from >= self.node_count {
            return Err(GraphError::NodeOutOfRange {
                node: from,
                node_count: self.node_count,
            });
        }
        if to >= self.node_count {
            return Err(GraphError::NodeOutOfRange {
                node: to,
                node_count: self.node_count,
            });
        }
        self.edges.push(Edge { from, to, weight });
        self.insert_neighbor(from, to, weight);
        if !self.directed && from != to {
            self.insert_neighbor(to, from, weight);
        }
        Ok(())
    }

    /// Inserts a neighbour into the adjacency list of a node, keeping it sorted.
    fn insert_neighbor(&mut self, from: usize, to: usize, weight: i64) {
        let list = &mut self.adjacency[from];
        let position = list.partition_point(|&(target, existing)| (target, existing) < (to, weight));
        list.insert(position, (to, weight));
    }
}

/// The result of a breadth-first search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BfsResult {
    /// The nodes in the order the search reached them.
    pub order: Vec<usize>,
    /// The number of edges of a shortest path, in edge count, from the start to the
    /// node; `None` when the node is not reachable.
    pub distance: Vec<Option<usize>>,
    /// The predecessor of a node on that path; `None` for the start and for the nodes
    /// that are not reachable.
    pub parent: Vec<Option<usize>>,
}

/// Searches the graph level by level from `start` and returns the shortest path tree
/// of an unweighted graph: the number of edges of a shortest path to every reachable
/// node and the predecessor that walks the path back.
///
/// The search returns `Err` and never panics.
///
/// # Constraints (from the card)
///
/// * The graph has to be **unweighted**: every edge of the graph has to carry the
///   weight `1`. A graph with any other weight is refused with
///   [`GraphError::NonUnitWeight`], because the "fewest edges" of a breadth-first
///   search is not the "shortest distance" of a weighted graph.
/// * The start node has to exist: `start < graph.node_count()`.
/// * The search covers the connected component of the start node only; the other nodes
///   keep the distance `None`.
///
/// # Complexity (from the card)
///
/// Time `O(V + E)`, extra space `O(V)` for the distances, the parents and the queue.
///
/// # When it does not apply (from the card)
///
/// * The edges have weights and the weights differ - the fewest edges are not the
///   shortest path; use [`dijkstra`] instead.
/// * The graph is huge and only the connectivity matters - [`union_find`] costs less
///   memory.
/// * Every path is needed, not only the shortest one - [`dfs`] is more direct.
/// * The graph is a DAG and only the order matters - [`topological_sort`] fits better.
///
/// # Determinism
///
/// The neighbours of a node are visited in the order of the adjacency list, which is
/// sorted by `(target, weight)`; the same graph therefore always gives the same
/// `order`.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{bfs, Graph};
///
/// let mut graph = Graph::new(3, false);
/// graph.add_edge(0, 1).unwrap();
/// graph.add_edge(1, 2).unwrap();
///
/// let result = bfs(&graph, 0).unwrap();
/// assert_eq!(result.distance[2], Some(2));
/// assert_eq!(result.parent[2], Some(1));
/// ```
///
/// * asset: `algorithm.graph.bfs`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.bfs/card.md`
pub fn bfs(graph: &Graph, start: usize) -> Result<BfsResult, GraphError> {
    if start >= graph.node_count() {
        return Err(GraphError::NodeOutOfRange {
            node: start,
            node_count: graph.node_count(),
        });
    }
    for edge in graph.edges() {
        if edge.weight != 1 {
            return Err(GraphError::NonUnitWeight {
                from: edge.from,
                to: edge.to,
                weight: edge.weight,
            });
        }
    }

    let mut distance: Vec<Option<usize>> = vec![None; graph.node_count()];
    let mut parent: Vec<Option<usize>> = vec![None; graph.node_count()];
    let mut order = Vec::new();
    let mut queue: VecDeque<usize> = VecDeque::new();

    distance[start] = Some(0);
    queue.push_back(start);

    while let Some(node) = queue.pop_front() {
        order.push(node);
        let Some(level) = distance[node] else {
            continue;
        };
        for &(neighbor, _) in &graph.adjacency[node] {
            if distance[neighbor].is_some() {
                continue;
            }
            distance[neighbor] = Some(level + 1);
            parent[neighbor] = Some(node);
            queue.push_back(neighbor);
        }
    }

    Ok(BfsResult {
        order,
        distance,
        parent,
    })
}

/// The result of a depth-first search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DfsResult {
    /// The nodes in the order the search reached them.
    pub order: Vec<usize>,
    /// The predecessor of a node in the search tree; `None` for the start and for the
    /// nodes that are not reachable.
    pub parent: Vec<Option<usize>>,
    /// The depth of a node in the search tree; `None` for the nodes that are not
    /// reachable.
    pub depth: Vec<Option<usize>>,
}

/// Walks the graph depth first from `start` and returns the order the nodes were
/// reached in, together with the search tree.
///
/// The search is iterative: it uses an explicit stack, so a graph deeper than the
/// call stack of the thread does not overflow it.
///
/// # Constraints (from the card)
///
/// * The graph may be directed or undirected and may carry any weight: a depth-first
///   search does not minimize a cost, it only walks the reachable part of the graph.
/// * The start node has to exist: `start < graph.node_count()`.
/// * The search covers the connected component of the start node only.
///
/// # Complexity (from the card)
///
/// Time `O(V + E)`, extra space `O(V)` for the visited marks and the explicit stack.
///
/// # When it does not apply (from the card)
///
/// * The shortest path of an unweighted graph is needed - a depth-first search does
///   not guarantee the shortest path; use [`bfs`] instead.
/// * The graph is very deep - a recursive implementation would overflow the stack;
///   this implementation uses an explicit stack, so it does not.
/// * Every path has to be enumerated - the walk of a depth-first search is linear,
///   the enumeration of the paths is exponential.
///
/// # Determinism
///
/// The neighbours of a node are considered in the order of the adjacency list, which is
/// sorted by `(target, weight)`; the same graph therefore always gives the same
/// `order`.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{dfs, Graph};
///
/// let mut graph = Graph::new(3, true);
/// graph.add_edge(0, 1).unwrap();
/// graph.add_edge(1, 2).unwrap();
///
/// let result = dfs(&graph, 0).unwrap();
/// assert_eq!(result.order, vec![0, 1, 2]);
/// assert_eq!(result.depth[2], Some(2));
/// ```
///
/// * asset: `algorithm.graph.dfs`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.dfs/card.md`
pub fn dfs(graph: &Graph, start: usize) -> Result<DfsResult, GraphError> {
    if start >= graph.node_count() {
        return Err(GraphError::NodeOutOfRange {
            node: start,
            node_count: graph.node_count(),
        });
    }

    let mut visited = vec![false; graph.node_count()];
    let mut order = Vec::new();
    let mut parent: Vec<Option<usize>> = vec![None; graph.node_count()];
    let mut depth: Vec<Option<usize>> = vec![None; graph.node_count()];

    // The stack holds the nodes to reach and the node that reached them.
    let mut stack: Vec<(usize, Option<usize>)> = vec![(start, None)];
    while let Some((node, from)) = stack.pop() {
        if visited[node] {
            continue;
        }
        visited[node] = true;
        parent[node] = from;
        depth[node] = Some(match depth[from.unwrap_or(node)] {
            Some(value) => value + 1,
            None => 0,
        });
        order.push(node);

        // The neighbours are pushed in the reverse order so that they are popped in
        // the order of the adjacency list.
        let mut neighbors: Vec<usize> = graph.adjacency[node]
            .iter()
            .map(|&(target, _)| target)
            .filter(|&target| !visited[target])
            .collect();
        neighbors.sort_unstable();
        neighbors.dedup();
        for target in neighbors.into_iter().rev() {
            stack.push((target, Some(node)));
        }
    }

    Ok(DfsResult {
        order,
        parent,
        depth,
    })
}

/// Sorts the nodes of a directed acyclic graph so that every edge goes from an
/// earlier node to a later one.
///
/// The search is Kahn's algorithm: the nodes whose in-degree reached zero are taken
/// in the order of their index, so the order is a function of the graph and not of the
/// order the edges were added in.
///
/// # Constraints (from the card)
///
/// * The graph has to be **directed**: an undirected graph is refused with
///   [`GraphError::UndirectedGraphUnsupported`], because a dependency order is a
///   relation between ordered things.
/// * The graph has to be **acyclic**: a graph with a cycle is refused with
///   [`GraphError::CycleDetected`]; the order is never silently incomplete.
/// * The graph is the dependency graph of the problem: an edge `u -> v` means that
///   `u` has to run before `v`.
///
/// # Complexity (from the card)
///
/// Time `O(V + E)`, extra space `O(V)` for the in-degrees, the ready queue and the
/// result.
///
/// # When it does not apply (from the card)
///
/// * The graph has a cycle - no topological order exists, the cycle has to be detected
///   and handled first.
/// * The dependencies are decided while the program runs - a static topological order
///   does not apply.
/// * The lexicographically smallest order is needed - the plain algorithm does not
///   give it; a variant with a priority queue does.
/// * Only any valid order is needed and the graph is tiny - the reverse postorder of
///   a depth-first search is shorter.
///
/// # Determinism
///
/// Among the nodes whose in-degree reached zero, the node with the smallest index is
/// taken first. The order is therefore stable and reproducible, but it is not the
/// only valid topological order.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{topological_sort, Graph};
///
/// let mut graph = Graph::new(3, true);
/// graph.add_edge(0, 2).unwrap();
/// graph.add_edge(1, 2).unwrap();
///
/// let order = topological_sort(&graph).unwrap();
/// assert_eq!(order, vec![0, 1, 2]);
/// ```
///
/// * asset: `algorithm.graph.topological_sort`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.topological_sort/card.md`
pub fn topological_sort(graph: &Graph) -> Result<Vec<usize>, GraphError> {
    if !graph.is_directed() {
        return Err(GraphError::UndirectedGraphUnsupported {
            algorithm: "topological_sort",
        });
    }

    let node_count = graph.node_count();
    let mut in_degree = vec![0usize; node_count];
    for edge in graph.edges() {
        in_degree[edge.to] += 1;
    }

    let mut ready: BinaryHeap<Reverse<usize>> = BinaryHeap::new();
    for (node, degree) in in_degree.iter().enumerate() {
        if *degree == 0 {
            ready.push(Reverse(node));
        }
    }

    let mut order = Vec::with_capacity(node_count);
    while let Some(Reverse(node)) = ready.pop() {
        order.push(node);
        for &(neighbor, _) in &graph.adjacency[node] {
            if in_degree[neighbor] == 0 {
                continue;
            }
            in_degree[neighbor] -= 1;
            if in_degree[neighbor] == 0 {
                ready.push(Reverse(neighbor));
            }
        }
    }

    if order.len() != node_count {
        return Err(GraphError::CycleDetected {
            remaining_nodes: node_count - order.len(),
        });
    }
    Ok(order)
}

/// The distances and the predecessors of a single source shortest path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortestPaths {
    /// The distance from the source to every node; `None` when the node is not
    /// reachable.
    pub distance: Vec<Option<i64>>,
    /// The predecessor of a node on a shortest path from the source; `None` for the
    /// source and for the nodes that are not reachable.
    pub predecessor: Vec<Option<usize>>,
}

impl ShortestPaths {
    /// The nodes of a shortest path from the source to `target`, the source first.
    ///
    /// `None` when the target is not reachable or when the target is outside the
    /// graph.
    pub fn path_to(&self, target: usize) -> Option<Vec<usize>> {
        if target >= self.distance.len() {
            return None;
        }
        // The target has to be reachable.
        self.distance[target]?;
        let mut path = vec![target];
        let mut current = target;
        while let Some(parent) = self.predecessor[current] {
            path.push(parent);
            current = parent;
            if path.len() > self.distance.len() {
                return None;
            }
        }
        path.reverse();
        Some(path)
    }
}

/// Computes the shortest path from `source` to every node of a graph whose edge
/// weights are not negative.
///
/// The search is Dijkstra's algorithm with a binary heap; the distances and the
/// predecessors of the paths are returned.
///
/// # Constraints (from the card)
///
/// * The edge weights have to be **not negative**: a graph with a negative edge is
///   refused with [`GraphError::NegativeWeight`], because the greedy assumption of
///   the algorithm fails there and the answer would silently be wrong.
/// * The source has to exist: `source < graph.node_count()`.
/// * The graph may be directed or undirected.
/// * The weights are integers; the distances are `i64`.
///
/// # Complexity (from the card)
///
/// Time `O((V + E) log V)` with a binary heap and an adjacency list, extra space
/// `O(V + E)`; the heap may hold `O(E)` entries when there are stale ones.
///
/// # When it does not apply (from the card)
///
/// * An edge has a negative weight - use [`bellman_ford`] instead.
/// * The distances of all the pairs are needed - use [`floyd_warshall`] or run this
///   function once per source.
/// * The graph is tiny and the weights are simple - the naive `O(V^2)` variant is
///   simpler and just as fast.
/// * The weights are floating point and an exact comparison is needed - the integer
///   weights of this implementation avoid that question.
///
/// # Determinism
///
/// The heap takes the smallest distance first and, on equal distances, the smallest
/// node index. The same graph therefore always gives the same predecessors.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{dijkstra, Graph};
///
/// let mut graph = Graph::new(3, true);
/// graph.add_weighted_edge(0, 1, 4).unwrap();
/// graph.add_weighted_edge(1, 2, 3).unwrap();
/// graph.add_weighted_edge(0, 2, 9).unwrap();
///
/// let paths = dijkstra(&graph, 0).unwrap();
/// assert_eq!(paths.distance[2], Some(7));
/// assert_eq!(paths.path_to(2), Some(vec![0, 1, 2]));
/// ```
///
/// * asset: `algorithm.graph.dijkstra`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.dijkstra/card.md`
pub fn dijkstra(graph: &Graph, source: usize) -> Result<ShortestPaths, GraphError> {
    if source >= graph.node_count() {
        return Err(GraphError::NodeOutOfRange {
            node: source,
            node_count: graph.node_count(),
        });
    }
    for edge in graph.edges() {
        if edge.weight < 0 {
            return Err(GraphError::NegativeWeight {
                from: edge.from,
                to: edge.to,
                weight: edge.weight,
            });
        }
    }

    let mut distance: Vec<Option<i64>> = vec![None; graph.node_count()];
    let mut predecessor: Vec<Option<usize>> = vec![None; graph.node_count()];
    let mut heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();

    distance[source] = Some(0);
    heap.push(Reverse((0, source)));

    while let Some(Reverse((current_distance, node))) = heap.pop() {
        let Some(known) = distance[node] else {
            continue;
        };
        if known != current_distance {
            // A stale entry: the distance of the node was improved after the entry
            // was pushed.
            continue;
        }
        for &(neighbor, weight) in &graph.adjacency[node] {
            let candidate = current_distance
                .checked_add(weight)
                .ok_or(GraphError::ArithmeticOverflow)?;
            let is_shorter = match distance[neighbor] {
                Some(previous) => candidate < previous,
                None => true,
            };
            if is_shorter {
                distance[neighbor] = Some(candidate);
                predecessor[neighbor] = Some(node);
                heap.push(Reverse((candidate, neighbor)));
            }
        }
    }

    Ok(ShortestPaths {
        distance,
        predecessor,
    })
}

/// Computes the shortest path from `source` to every node of a graph whose edge
/// weights may be negative, and detects the cycles of negative total weight.
///
/// The search relaxes every edge at most `V - 1` times and stops earlier when a
/// round changes nothing.
///
/// # Constraints (from the card)
///
/// * The edge weights may be **negative**; a negative cycle is detected and reported
///   with [`GraphError::NegativeCycleDetected`], because the shortest paths are not
///   defined there.
/// * The source has to exist: `source < graph.node_count()`.
/// * The graph may be directed or undirected. An undirected graph with a negative edge
///   always has a negative cycle, since the edge can be walked in both directions.
///
/// # Complexity (from the card)
///
/// Time `O(V * E)`, less when the relaxation converges earlier; extra space `O(V)`
/// for the distances and the predecessors.
///
/// # When it does not apply (from the card)
///
/// * Every weight is not negative - [`dijkstra`] is faster and simpler.
/// * The distances of all the pairs are needed on a small graph -
///   [`floyd_warshall`] is more direct.
/// * The graph is big - the `O(V * E)` cost is usually not acceptable.
///
/// # Determinism
///
/// The edges are relaxed in the order they were added and a predecessor is only
/// replaced when the new distance is strictly shorter; the same graph therefore
/// always gives the same predecessors.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{bellman_ford, Graph};
///
/// let mut graph = Graph::new(3, true);
/// graph.add_weighted_edge(0, 1, 4).unwrap();
/// graph.add_weighted_edge(1, 2, -2).unwrap();
/// graph.add_weighted_edge(0, 2, 5).unwrap();
///
/// let paths = bellman_ford(&graph, 0).unwrap();
/// assert_eq!(paths.distance[2], Some(2));
/// ```
///
/// * asset: `algorithm.graph.bellman_ford`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.bellman_ford/card.md`
pub fn bellman_ford(graph: &Graph, source: usize) -> Result<ShortestPaths, GraphError> {
    if source >= graph.node_count() {
        return Err(GraphError::NodeOutOfRange {
            node: source,
            node_count: graph.node_count(),
        });
    }

    let node_count = graph.node_count();
    let mut distance: Vec<Option<i64>> = vec![None; node_count];
    let mut predecessor: Vec<Option<usize>> = vec![None; node_count];
    distance[source] = Some(0);

    for _ in 0..node_count.saturating_sub(1) {
        let mut changed = false;
        for edge in graph.edges() {
            let Some(from_distance) = distance[edge.from] else {
                continue;
            };
            let candidate = from_distance
                .checked_add(edge.weight)
                .ok_or(GraphError::ArithmeticOverflow)?;
            let is_shorter = match distance[edge.to] {
                Some(previous) => candidate < previous,
                None => true,
            };
            if is_shorter {
                distance[edge.to] = Some(candidate);
                predecessor[edge.to] = Some(edge.from);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // One more round: a distance that can still be improved means that a negative
    // cycle is reachable from the source.
    for edge in graph.edges() {
        let Some(from_distance) = distance[edge.from] else {
            continue;
        };
        let candidate = from_distance
            .checked_add(edge.weight)
            .ok_or(GraphError::ArithmeticOverflow)?;
        let is_shorter = match distance[edge.to] {
            Some(previous) => candidate < previous,
            None => true,
        };
        if is_shorter {
            return Err(GraphError::NegativeCycleDetected);
        }
    }

    Ok(ShortestPaths {
        distance,
        predecessor,
    })
}

/// The distances of all the pairs of nodes of a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllPairsShortestPaths {
    /// The distance of every pair of nodes; `None` when a pair cannot be reached.
    pub distance: Vec<Vec<Option<i64>>>,
    /// The next node on a shortest path, the first step of the path.
    pub next: Vec<Vec<Option<usize>>>,
}

impl AllPairsShortestPaths {
    /// The nodes of a shortest path from `from` to `to`, `from` first.
    ///
    /// `None` when the two nodes cannot be reached from each other or when one of
    /// them is outside the graph.
    pub fn path(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        if from >= self.distance.len() || to >= self.distance.len() {
            return None;
        }
        if from == to {
            return Some(vec![from]);
        }
        let mut path = vec![from];
        let mut current = from;
        while current != to {
            let step = self.next[current][to]?;
            if step == current {
                return None;
            }
            current = step;
            path.push(current);
            if path.len() > self.distance.len() {
                return None;
            }
        }
        Some(path)
    }
}

/// Computes the shortest distance of every pair of nodes of a graph.
///
/// The computation is the dynamic program of Floyd and Warshall: every node is taken
/// once as a middle node, and the direct distances are upgraded to the distances that
/// go through it.
///
/// # Constraints (from the card)
///
/// * The graph has to be **small**: the algorithm costs `O(V^3)` time and `O(V^2)`
///   space, so it is meant for the few hundred nodes at most.
/// * The edge weights may be negative as long as the graph has **no negative cycle**;
///   a negative cycle is detected and reported with
///   [`GraphError::NegativeCycleDetected`], because the distances are not defined
///   there.
/// * The graph may be directed or undirected.
///
/// # Complexity (from the card)
///
/// Time `O(V^3)`, extra space `O(V^2)` for the distance matrix and the matrix of the
/// next steps used to rebuild the paths.
///
/// # When it does not apply (from the card)
///
/// * `V` is in the thousands - the `O(V^3)` cost is not acceptable; run a single
///   source algorithm instead.
/// * Only some pairs are needed - [`dijkstra`] per source is cheaper.
/// * Only the distances are needed - the matrix of the next steps doubles the memory
///   and can be dropped when the paths are not rebuilt.
///
/// # Determinism
///
/// The middle nodes are taken in the order of their index and a distance is only
/// replaced when the new one is strictly shorter; the same graph therefore always
/// gives the same distances and the same paths.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{floyd_warshall, Graph};
///
/// let mut graph = Graph::new(3, true);
/// graph.add_weighted_edge(0, 1, 4).unwrap();
/// graph.add_weighted_edge(1, 2, 3).unwrap();
///
/// let paths = floyd_warshall(&graph).unwrap();
/// assert_eq!(paths.distance[0][2], Some(7));
/// assert_eq!(paths.path(0, 2), Some(vec![0, 1, 2]));
/// ```
///
/// * asset: `algorithm.graph.floyd_warshall`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.floyd_warshall/card.md`
pub fn floyd_warshall(graph: &Graph) -> Result<AllPairsShortestPaths, GraphError> {
    let node_count = graph.node_count();
    let mut distance: Vec<Vec<Option<i64>>> = vec![vec![None; node_count]; node_count];
    let mut next: Vec<Vec<Option<usize>>> = vec![vec![None; node_count]; node_count];

    for (node, (row, next_row)) in distance
        .iter_mut()
        .zip(next.iter_mut())
        .enumerate()
    {
        row[node] = Some(0);
        next_row[node] = Some(node);
    }

    // The edges are read from the adjacency lists, because an undirected edge is
    // part of the adjacency of both of its nodes and a directed one only of the
    // node it starts at.
    for from in 0..node_count {
        for &(to, weight) in &graph.adjacency[from] {
            let is_shorter = match distance[from][to] {
                Some(previous) => weight < previous,
                None => true,
            };
            if is_shorter {
                distance[from][to] = Some(weight);
                next[from][to] = Some(to);
            }
        }
    }

    for middle in 0..node_count {
        // The row and the column of the middle node are read once, so that the
        // updates of this round cannot change them.
        let row_of_middle = distance[middle].clone();
        let column_of_middle: Vec<Option<i64>> = (0..node_count)
            .map(|node| distance[node][middle])
            .collect();
        let next_column_of_middle: Vec<Option<usize>> = (0..node_count)
            .map(|node| next[node][middle])
            .collect();

        for from in 0..node_count {
            let Some(to_middle) = column_of_middle[from] else {
                continue;
            };
            for to in 0..node_count {
                let Some(from_middle) = row_of_middle[to] else {
                    continue;
                };
                let candidate = to_middle
                    .checked_add(from_middle)
                    .ok_or(GraphError::ArithmeticOverflow)?;
                let is_shorter = match distance[from][to] {
                    Some(previous) => candidate < previous,
                    None => true,
                };
                if is_shorter {
                    distance[from][to] = Some(candidate);
                    if let Some(step) = next_column_of_middle[from] {
                        next[from][to] = Some(step);
                    }
                }
            }
        }
    }

    for (node, row) in distance.iter().enumerate() {
        if row[node].is_some_and(|distance_to_self| distance_to_self < 0) {
            return Err(GraphError::NegativeCycleDetected);
        }
    }

    Ok(AllPairsShortestPaths { distance, next })
}

/// A minimum spanning tree of a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimumSpanningTree {
    /// The edges of the tree, in the order the algorithm took them.
    pub edges: Vec<Edge>,
    /// The sum of the weights of the edges of the tree.
    pub total_weight: i128,
}

/// Sorts the edges by weight and takes the cheapest ones that do not close a cycle,
/// which gives the minimum spanning tree of an undirected graph.
///
/// The cycle test is the union-find of this module, [`union_find`].
///
/// # Constraints (from the card)
///
/// * The graph has to be **undirected**: a directed graph is refused with
///   [`GraphError::DirectedGraphUnsupported`], because a spanning tree of a directed
///   graph is not defined.
/// * The graph has to be **connected**: a graph that is not connected is refused with
///   [`GraphError::DisconnectedGraph`]; the algorithm would only give a spanning
///   forest and the requirement has to say so.
/// * The weights may be negative; a minimum spanning tree does not need the weights
///   to be not negative.
/// * The tree has `V - 1` edges and covers every node.
///
/// # Complexity (from the card)
///
/// Time `O(E log E)`, the sort of the edges dominates; extra space `O(V + E)` for
/// the sorted edges and the union-find.
///
/// # When it does not apply (from the card)
///
/// * The graph is directed - a minimum spanning tree is not defined.
/// * The graph is dense - the matrix variant of [`prim`] may use less memory.
/// * The graph is not connected - the result would be a spanning forest, and the
///   requirement has to say so.
/// * A maximum spanning tree is needed - the weights have to be negated, and the
///   precision of floating point weights has to be checked.
///
/// # Determinism
///
/// The edges are sorted by `(weight, from, to)` and the first edge that joins two
/// components is taken, so the same graph always gives the same tree. A minimum
/// spanning tree is not unique; the tree of the smallest edges in that order is the
/// one this function returns.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{kruskal, Graph};
///
/// let mut graph = Graph::new(3, false);
/// graph.add_weighted_edge(0, 1, 1).unwrap();
/// graph.add_weighted_edge(1, 2, 2).unwrap();
/// graph.add_weighted_edge(0, 2, 3).unwrap();
///
/// let tree = kruskal(&graph).unwrap();
/// assert_eq!(tree.edges.len(), 2);
/// assert_eq!(tree.total_weight, 3);
/// ```
///
/// * asset: `algorithm.graph.kruskal`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.kruskal/card.md`
pub fn kruskal(graph: &Graph) -> Result<MinimumSpanningTree, GraphError> {
    if graph.is_directed() {
        return Err(GraphError::DirectedGraphUnsupported {
            algorithm: "kruskal",
        });
    }

    let node_count = graph.node_count();
    if node_count == 0 {
        return Ok(MinimumSpanningTree {
            edges: Vec::new(),
            total_weight: 0,
        });
    }

    let mut sorted = graph.edges().to_vec();
    sorted.sort_by(|left, right| {
        left.weight
            .cmp(&right.weight)
            .then(left.from.cmp(&right.from))
            .then(left.to.cmp(&right.to))
    });

    let mut components = union_find(node_count);
    let mut tree: Vec<Edge> = Vec::with_capacity(node_count.saturating_sub(1));
    let mut total_weight: i128 = 0;

    for edge in sorted {
        if edge.from == edge.to {
            // A self loop can never be part of a spanning tree.
            continue;
        }
        if components.union(edge.from, edge.to)? {
            tree.push(edge);
            total_weight += i128::from(edge.weight);
            if tree.len() == node_count - 1 {
                break;
            }
        }
    }

    if tree.len() != node_count - 1 {
        return Err(GraphError::DisconnectedGraph {
            components: components.component_count(),
        });
    }

    Ok(MinimumSpanningTree {
        edges: tree,
        total_weight,
    })
}

/// Grows a tree from one node and takes the cheapest edge that joins the tree to the
/// rest of the graph, which gives the minimum spanning tree of an undirected graph.
///
/// The search starts at the node `0`.
///
/// # Constraints (from the card)
///
/// * The graph has to be **undirected**: a directed graph is refused with
///   [`GraphError::DirectedGraphUnsupported`], because a spanning tree of a directed
///   graph is not defined.
/// * The graph has to be **connected**: a graph that is not connected is refused with
///   [`GraphError::DisconnectedGraph`]; the algorithm would only reach the component
///   of the node `0` and the requirement has to say so.
/// * The weights may be negative; a minimum spanning tree does not need the weights
///   to be not negative, unlike [`dijkstra`].
/// * The tree has `V - 1` edges and covers every node.
///
/// # Complexity (from the card)
///
/// Time `O(E log V)` with a binary heap, extra space `O(V)` for the heap and the
/// marks of the tree. The matrix variant costs `O(V^2)` time and `O(V^2)` memory.
///
/// # When it does not apply (from the card)
///
/// * The graph is very sparse - [`kruskal`] sorts fewer edges and is simpler.
/// * The graph is directed - a minimum spanning tree is not defined.
/// * Only a spanning forest is needed and the graph is not connected - every
///   component has to be handled.
///
/// # Determinism
///
/// The heap takes the smallest weight first and, on equal weights, the smallest
/// source node and then the smallest target node. The same graph therefore always
/// gives the same tree. A minimum spanning tree is not unique; the tree of this
/// function and the tree of [`kruskal`] have the same total weight.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::{prim, Graph};
///
/// let mut graph = Graph::new(3, false);
/// graph.add_weighted_edge(0, 1, 1).unwrap();
/// graph.add_weighted_edge(1, 2, 2).unwrap();
/// graph.add_weighted_edge(0, 2, 3).unwrap();
///
/// let tree = prim(&graph).unwrap();
/// assert_eq!(tree.edges.len(), 2);
/// assert_eq!(tree.total_weight, 3);
/// ```
///
/// * asset: `algorithm.graph.prim`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.prim/card.md`
pub fn prim(graph: &Graph) -> Result<MinimumSpanningTree, GraphError> {
    if graph.is_directed() {
        return Err(GraphError::DirectedGraphUnsupported {
            algorithm: "prim",
        });
    }

    let node_count = graph.node_count();
    if node_count == 0 {
        return Ok(MinimumSpanningTree {
            edges: Vec::new(),
            total_weight: 0,
        });
    }

    let mut in_tree = vec![false; node_count];
    let mut heap: BinaryHeap<Reverse<(i64, usize, usize)>> = BinaryHeap::new();
    let mut tree: Vec<Edge> = Vec::with_capacity(node_count.saturating_sub(1));
    let mut total_weight: i128 = 0;

    // The first entry is the start node itself; it carries no edge.
    heap.push(Reverse((0, 0, 0)));

    while let Some(Reverse((weight, from, to))) = heap.pop() {
        if in_tree[to] {
            continue;
        }
        in_tree[to] = true;
        if to != 0 {
            tree.push(Edge { from, to, weight });
            total_weight += i128::from(weight);
        }
        for &(neighbor, edge_weight) in &graph.adjacency[to] {
            if !in_tree[neighbor] {
                heap.push(Reverse((edge_weight, to, neighbor)));
            }
        }
    }

    if tree.len() != node_count - 1 {
        return Err(GraphError::DisconnectedGraph {
            components: component_count(graph),
        });
    }

    Ok(MinimumSpanningTree {
        edges: tree,
        total_weight,
    })
}

/// The number of connected components of an undirected graph.
///
/// The search starts from every node that was not reached yet, so every component is
/// counted once.
fn component_count(graph: &Graph) -> usize {
    let mut visited = vec![false; graph.node_count()];
    let mut components = 0;
    for start in 0..graph.node_count() {
        if visited[start] {
            continue;
        }
        components += 1;
        let mut stack = vec![start];
        visited[start] = true;
        while let Some(node) = stack.pop() {
            for &(neighbor, _) in &graph.adjacency[node] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    stack.push(neighbor);
                }
            }
        }
    }
    components
}

/// A disjoint set union, also called a union-find.
///
/// The structure answers the question "are these two elements of the same set?" and
/// merges two sets, both in a nearly constant amortized time. The elements are the
/// indices `0..element_count`.
///
/// # Constraints (from the card)
///
/// * The elements have to be **indexable integers**: the structure works on the
///   indices `0..element_count`; another kind of element has to be mapped to an index
///   first.
/// * The structure only **merges**: there is no way to delete an element or to split
///   a set again, and no way to walk the members of a set without the extra work of
///   [`UnionFind::groups`].
/// * The structure is **mutable and not thread safe**: the path compression writes the
///   parent pointers, so a concurrent use has to be synchronized or to use one
///   structure per thread.
///
/// # Complexity (from the card)
///
/// Time `O(alpha(n))` amortized with the path compression and the union by rank,
/// `alpha` being the inverse of the Ackermann function, which is a constant for any
/// practical size; extra space `O(n)` for the parents and the ranks.
///
/// # When it does not apply (from the card)
///
/// * An element has to be deleted or a set has to be split - the union-find does not
///   support that; a balanced tree or a list of lists is needed.
/// * The members of a set have to be walked - the structure does not keep them; use
///   [`UnionFind::groups`], which costs `O(n)` on top of the queries.
/// * The elements are not indexable integers - a mapping to the indices is needed
///   first.
/// * There are very few queries - a plain array or a hash map is simpler.
///
/// # Example
///
/// ```
/// use tase_impls::algorithm::graph::union_find;
///
/// let mut sets = union_find(4);
/// assert!(sets.union(0, 1).unwrap());
/// assert!(!sets.union(0, 1).unwrap());
/// assert!(sets.connected(0, 1).unwrap());
/// assert!(!sets.connected(1, 2).unwrap());
/// assert_eq!(sets.component_count(), 3);
/// ```
///
/// * asset: `algorithm.graph.union_find`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.graph.union_find/card.md`
pub fn union_find(element_count: usize) -> UnionFind {
    UnionFind {
        parent: (0..element_count).collect(),
        rank: vec![0; element_count],
        components: element_count,
    }
}

/// A disjoint set union, also called a union-find.
///
/// See [`union_find`], the function that builds one.
#[derive(Debug, Clone)]
pub struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
    components: usize,
}

impl UnionFind {
    /// The number of elements of the structure.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// Whether the structure has no element at all.
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// The number of sets the elements are currently split into.
    pub fn component_count(&self) -> usize {
        self.components
    }

    /// The representative of the set the element belongs to.
    ///
    /// The path of the parent pointers is compressed on the way, so the queries that
    /// follow are cheap.
    ///
    /// # Errors
    ///
    /// [`GraphError::NodeOutOfRange`] when the element is outside the structure.
    pub fn find(&mut self, element: usize) -> Result<usize, GraphError> {
        if element >= self.parent.len() {
            return Err(GraphError::NodeOutOfRange {
                node: element,
                node_count: self.parent.len(),
            });
        }
        let mut current = element;
        while self.parent[current] != current {
            let grandparent = self.parent[self.parent[current]];
            self.parent[current] = grandparent;
            current = grandparent;
        }
        Ok(current)
    }

    /// Merges the sets of the two elements.
    ///
    /// Returns `true` when the two elements were in two different sets and the sets
    /// were merged, and `false` when they already were in the same set and nothing
    /// changed.
    ///
    /// # Errors
    ///
    /// [`GraphError::NodeOutOfRange`] when one of the two elements is outside the
    /// structure.
    pub fn union(&mut self, first: usize, second: usize) -> Result<bool, GraphError> {
        let first_root = self.find(first)?;
        let second_root = self.find(second)?;
        if first_root == second_root {
            return Ok(false);
        }
        if self.rank[first_root] < self.rank[second_root] {
            self.parent[first_root] = second_root;
        } else if self.rank[first_root] > self.rank[second_root] {
            self.parent[second_root] = first_root;
        } else {
            self.parent[second_root] = first_root;
            self.rank[first_root] = self.rank[first_root].saturating_add(1);
        }
        self.components -= 1;
        Ok(true)
    }

    /// Whether the two elements are in the same set.
    ///
    /// # Errors
    ///
    /// [`GraphError::NodeOutOfRange`] when one of the two elements is outside the
    /// structure.
    pub fn connected(&mut self, first: usize, second: usize) -> Result<bool, GraphError> {
        Ok(self.find(first)? == self.find(second)?)
    }

    /// The elements of every set, grouped by set.
    ///
    /// The groups are ordered by their representative and the elements of a group are
    /// ordered by their index. The cost is `O(n)` on top of the queries.
    pub fn groups(&self) -> Vec<Vec<usize>> {
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); self.parent.len()];
        for element in 0..self.parent.len() {
            let mut root = element;
            while self.parent[root] != root {
                root = self.parent[root];
            }
            buckets[root].push(element);
        }
        buckets.into_iter().filter(|group| !group.is_empty()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small undirected graph with the weights `0-1: 1`, `1-2: 2`, `0-2: 3`,
    /// `2-3: 4` and the isolated node `4`.
    fn weighted_sample() -> Graph {
        let mut graph = Graph::new(5, false);
        graph.add_weighted_edge(0, 1, 1).unwrap();
        graph.add_weighted_edge(1, 2, 2).unwrap();
        graph.add_weighted_edge(0, 2, 3).unwrap();
        graph.add_weighted_edge(2, 3, 4).unwrap();
        graph
    }

    #[test]
    fn bfs_asset_walks_the_graph_level_by_level() {
        let mut graph = Graph::new(6, false);
        graph.add_edge(0, 1).unwrap();
        graph.add_edge(0, 2).unwrap();
        graph.add_edge(1, 3).unwrap();
        graph.add_edge(2, 3).unwrap();
        graph.add_edge(3, 4).unwrap();
        // The node 5 stays isolated.

        let result = bfs(&graph, 0).unwrap();
        assert_eq!(result.order, vec![0, 1, 2, 3, 4]);
        assert_eq!(result.distance[0], Some(0));
        assert_eq!(result.distance[3], Some(2));
        assert_eq!(result.distance[4], Some(3));
        assert_eq!(result.distance[5], None);
        assert_eq!(result.parent[4], Some(3));
        assert_eq!(result.parent[5], None);
    }

    #[test]
    fn bfs_asset_on_an_empty_graph_and_on_a_self_loop() {
        let empty = Graph::new(0, false);
        assert_eq!(bfs(&empty, 0), Err(GraphError::NodeOutOfRange { node: 0, node_count: 0 }));

        let mut graph = Graph::new(2, false);
        graph.add_edge(0, 0).unwrap();
        graph.add_edge(0, 1).unwrap();
        let result = bfs(&graph, 0).unwrap();
        assert_eq!(result.order, vec![0, 1]);
        assert_eq!(result.distance[1], Some(1));
    }

    #[test]
    fn bfs_asset_refuses_a_weighted_graph() {
        // The counterexample of the card: with weighted edges the fewest edges are
        // not the shortest path, so the graph is refused instead of answering
        // something that looks like a distance.
        let mut graph = Graph::new(2, false);
        graph.add_weighted_edge(0, 1, 5).unwrap();
        assert_eq!(
            bfs(&graph, 0),
            Err(GraphError::NonUnitWeight { from: 0, to: 1, weight: 5 })
        );
    }

    #[test]
    fn dfs_asset_visits_every_node_once() {
        let mut graph = Graph::new(5, true);
        graph.add_edge(0, 1).unwrap();
        graph.add_edge(0, 2).unwrap();
        graph.add_edge(1, 3).unwrap();
        graph.add_edge(2, 3).unwrap();
        graph.add_edge(3, 4).unwrap();

        let result = dfs(&graph, 0).unwrap();
        assert_eq!(result.order, vec![0, 1, 3, 4, 2]);
        assert_eq!(result.depth[4], Some(3));
        assert_eq!(result.depth[2], Some(1));
        assert_eq!(result.parent[3], Some(1));
        // Every node is visited exactly once.
        let mut sorted = result.order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn dfs_asset_does_not_overflow_the_stack_on_a_deep_graph() {
        // The counterexample of the card: a recursive implementation would overflow
        // the stack on a deep chain, the explicit stack does not.
        let depth = 50_000;
        let mut graph = Graph::new(depth, true);
        for node in 1..depth {
            graph.add_edge(node - 1, node).unwrap();
        }
        let result = dfs(&graph, 0).unwrap();
        assert_eq!(result.order.len(), depth);
        assert_eq!(result.depth[depth - 1], Some(depth - 1));
    }

    #[test]
    fn topological_sort_asset_orders_the_dependencies() {
        let mut graph = Graph::new(6, true);
        graph.add_edge(0, 2).unwrap();
        graph.add_edge(1, 2).unwrap();
        graph.add_edge(2, 3).unwrap();
        graph.add_edge(0, 4).unwrap();
        graph.add_edge(4, 5).unwrap();

        let order = topological_sort(&graph).unwrap();
        assert_eq!(order.len(), 6);
        // Every edge goes from an earlier node to a later one.
        for edge in graph.edges() {
            let from = order.iter().position(|&node| node == edge.from).unwrap();
            let to = order.iter().position(|&node| node == edge.to).unwrap();
            assert!(from < to, "the edge {edge:?} is not respected by {order:?}");
        }
        // The smallest node that is ready comes first.
        assert_eq!(order[0], 0);
        assert_eq!(order[1], 1);
    }

    #[test]
    fn topological_sort_asset_refuses_a_cycle_and_an_undirected_graph() {
        // The counterexample of the card: a graph with a cycle has no topological
        // order, the error says so instead of returning an incomplete order.
        let mut cyclic = Graph::new(3, true);
        cyclic.add_edge(0, 1).unwrap();
        cyclic.add_edge(1, 2).unwrap();
        cyclic.add_edge(2, 0).unwrap();
        assert_eq!(
            topological_sort(&cyclic),
            Err(GraphError::CycleDetected { remaining_nodes: 3 })
        );

        // A dependency order is a relation between ordered things.
        let mut undirected = Graph::new(2, false);
        undirected.add_edge(0, 1).unwrap();
        assert_eq!(
            topological_sort(&undirected),
            Err(GraphError::UndirectedGraphUnsupported {
                algorithm: "topological_sort"
            })
        );
    }

    #[test]
    fn dijkstra_asset_computes_the_shortest_distances() {
        let graph = weighted_sample();
        let paths = dijkstra(&graph, 0).unwrap();
        assert_eq!(paths.distance[0], Some(0));
        assert_eq!(paths.distance[1], Some(1));
        assert_eq!(paths.distance[2], Some(3));
        assert_eq!(paths.distance[3], Some(7));
        // The node 4 is isolated.
        assert_eq!(paths.distance[4], None);
        assert_eq!(paths.path_to(3), Some(vec![0, 2, 3]));
        assert_eq!(paths.path_to(4), None);
    }

    #[test]
    fn dijkstra_asset_refuses_a_negative_edge() {
        // The counterexample of the card: with a negative edge the greedy assumption
        // fails and the answer would silently be wrong.
        let mut graph = Graph::new(3, true);
        graph.add_weighted_edge(0, 1, 4).unwrap();
        graph.add_weighted_edge(1, 2, -2).unwrap();
        assert_eq!(
            dijkstra(&graph, 0),
            Err(GraphError::NegativeWeight { from: 1, to: 2, weight: -2 })
        );
    }

    #[test]
    fn bellman_ford_asset_computes_the_shortest_distances_with_negative_edges() {
        let mut graph = Graph::new(4, true);
        graph.add_weighted_edge(0, 1, 4).unwrap();
        graph.add_weighted_edge(1, 2, -2).unwrap();
        graph.add_weighted_edge(2, 3, 3).unwrap();
        graph.add_weighted_edge(0, 3, 5).unwrap();

        let paths = bellman_ford(&graph, 0).unwrap();
        assert_eq!(paths.distance[3], Some(5));
        assert_eq!(paths.path_to(3), Some(vec![0, 1, 2, 3]));
    }

    #[test]
    fn bellman_ford_asset_detects_a_negative_cycle() {
        // The counterexample of the card: with a negative cycle the shortest path is
        // not defined, the error says so instead of returning a wrong distance.
        let mut graph = Graph::new(3, true);
        graph.add_weighted_edge(0, 1, 1).unwrap();
        graph.add_weighted_edge(1, 2, -3).unwrap();
        graph.add_weighted_edge(2, 1, 1).unwrap();
        assert_eq!(bellman_ford(&graph, 0), Err(GraphError::NegativeCycleDetected));
    }

    #[test]
    fn floyd_warshall_asset_agrees_with_dijkstra() {
        let graph = weighted_sample();
        let all_pairs = floyd_warshall(&graph).unwrap();
        for source in 0..graph.node_count() {
            let single = dijkstra(&graph, source).unwrap();
            for target in 0..graph.node_count() {
                assert_eq!(
                    all_pairs.distance[source][target], single.distance[target],
                    "the distances of {source} to {target} differ"
                );
            }
        }
        assert_eq!(all_pairs.path(0, 3), Some(vec![0, 2, 3]));
        assert_eq!(all_pairs.path(0, 4), None);
    }

    #[test]
    fn floyd_warshall_asset_detects_a_negative_cycle() {
        // The counterexample of the card: a negative cycle leaves the distances
        // undefined.
        let mut graph = Graph::new(3, true);
        graph.add_weighted_edge(0, 1, 1).unwrap();
        graph.add_weighted_edge(1, 2, -3).unwrap();
        graph.add_weighted_edge(2, 0, 1).unwrap();
        assert_eq!(floyd_warshall(&graph), Err(GraphError::NegativeCycleDetected));
    }

    #[test]
    fn kruskal_asset_builds_the_cheapest_tree() {
        // The node 4 of the sample is isolated, so the graph is not connected and
        // the tree cannot cover it.
        let graph = weighted_sample();
        assert_eq!(
            kruskal(&graph),
            Err(GraphError::DisconnectedGraph { components: 2 })
        );

        let mut connected = Graph::new(4, false);
        connected.add_weighted_edge(0, 1, 1).unwrap();
        connected.add_weighted_edge(1, 2, 2).unwrap();
        connected.add_weighted_edge(0, 2, 3).unwrap();
        connected.add_weighted_edge(2, 3, 4).unwrap();
        let tree = kruskal(&connected).unwrap();
        assert_eq!(tree.edges.len(), 3);
        assert_eq!(tree.total_weight, 7);
        assert_eq!(tree.edges[0], Edge { from: 0, to: 1, weight: 1 });
        assert_eq!(tree.edges[1], Edge { from: 1, to: 2, weight: 2 });
        assert_eq!(tree.edges[2], Edge { from: 2, to: 3, weight: 4 });
    }

    #[test]
    fn kruskal_asset_refuses_a_directed_graph_and_a_disconnected_graph() {
        // The counterexample of the card: a minimum spanning tree of a directed graph
        // is not defined.
        let mut directed = Graph::new(3, true);
        directed.add_weighted_edge(0, 1, 1).unwrap();
        directed.add_weighted_edge(1, 2, 2).unwrap();
        assert_eq!(
            kruskal(&directed),
            Err(GraphError::DirectedGraphUnsupported { algorithm: "kruskal" })
        );

        // A graph that is not connected would only give a spanning forest.
        let mut disconnected = Graph::new(4, false);
        disconnected.add_weighted_edge(0, 1, 1).unwrap();
        disconnected.add_weighted_edge(2, 3, 1).unwrap();
        assert_eq!(
            kruskal(&disconnected),
            Err(GraphError::DisconnectedGraph { components: 2 })
        );
    }

    #[test]
    fn prim_asset_agrees_with_kruskal() {
        let mut connected = Graph::new(4, false);
        connected.add_weighted_edge(0, 1, 1).unwrap();
        connected.add_weighted_edge(1, 2, 2).unwrap();
        connected.add_weighted_edge(0, 2, 3).unwrap();
        connected.add_weighted_edge(2, 3, 4).unwrap();

        let from_prim = prim(&connected).unwrap();
        let from_kruskal = kruskal(&connected).unwrap();
        assert_eq!(from_prim.total_weight, from_kruskal.total_weight);
        assert_eq!(from_prim.total_weight, 7);
        assert_eq!(from_prim.edges.len(), 3);
    }

    #[test]
    fn prim_asset_refuses_a_directed_graph_and_a_disconnected_graph() {
        // The counterexample of the card: a minimum spanning tree of a directed graph
        // is not defined.
        let mut directed = Graph::new(3, true);
        directed.add_weighted_edge(0, 1, 1).unwrap();
        assert_eq!(
            prim(&directed),
            Err(GraphError::DirectedGraphUnsupported { algorithm: "prim" })
        );

        let mut disconnected = Graph::new(4, false);
        disconnected.add_weighted_edge(0, 1, 1).unwrap();
        disconnected.add_weighted_edge(2, 3, 1).unwrap();
        assert_eq!(
            prim(&disconnected),
            Err(GraphError::DisconnectedGraph { components: 2 })
        );
    }

    #[test]
    fn union_find_asset_merges_and_queries_the_sets() {
        let mut sets = union_find(6);
        assert_eq!(sets.component_count(), 6);
        assert_eq!(sets.groups(), vec![vec![0], vec![1], vec![2], vec![3], vec![4], vec![5]]);

        assert!(sets.union(0, 1).unwrap());
        assert!(sets.union(1, 2).unwrap());
        // Merging two elements of the same set changes nothing.
        assert!(!sets.union(0, 2).unwrap());
        assert_eq!(sets.component_count(), 4);

        assert!(sets.connected(0, 2).unwrap());
        assert!(!sets.connected(0, 3).unwrap());
        let root_of_two = sets.find(2).unwrap();
        let root_of_zero = sets.find(0).unwrap();
        assert_eq!(root_of_two, root_of_zero);
        assert_eq!(sets.groups()[0], vec![0, 1, 2]);
    }

    #[test]
    fn union_find_asset_refuses_an_element_outside_the_structure() {
        // The counterexample of the card: the elements have to be indexable integers,
        // an index outside the structure is an error and not a panic.
        let mut sets = union_find(2);
        assert_eq!(
            sets.find(2),
            Err(GraphError::NodeOutOfRange { node: 2, node_count: 2 })
        );
        assert_eq!(
            sets.union(0, 7),
            Err(GraphError::NodeOutOfRange { node: 7, node_count: 2 })
        );
    }
}
