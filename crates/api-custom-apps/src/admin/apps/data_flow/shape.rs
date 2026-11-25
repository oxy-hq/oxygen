//! From the whole declared graph to what one answer carries: functions folded
//! into their apps, the focus's neighbourhood, and the node cap. Pure.

use std::collections::{HashMap, HashSet, VecDeque};

use super::assemble::Graph;
use super::{FlowEdge, FlowNode, NodeKind};

#[derive(Debug)]
pub(crate) struct Shaped {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    pub truncated: bool,
}

/// `graph` folded, focused and capped. `None` when `focus` names no node.
///
/// Functions stay nodes only for the focused app (a focus on one of its
/// functions counts); everywhere else they fold into their app, each edge
/// keeping which functions it came from. With a focus, the nodes within
/// `depth` hops of it, either direction, nearest first; without, the whole
/// graph, ordered outward from the apps so a cut keeps what reaches an app.
/// Past `max_nodes` the farthest go and `truncated` is set; an edge stays only
/// when both its ends do.
pub(crate) fn shape(
    mut graph: Graph,
    focus: Option<&str>,
    depth: u32,
    max_nodes: usize,
) -> Option<Shaped> {
    let expanded = focus.and_then(|f| expanded_app(&graph, f));
    fold_functions(&mut graph, expanded.as_deref());
    let order = match focus {
        Some(focus) => {
            if !graph.nodes.contains_key(focus) {
                return None;
            }
            walk(&graph, &[focus.to_string()], Some(depth))
        }
        None => whole(&graph),
    };
    let truncated = order.len() > max_nodes;
    let keep: HashSet<String> = order.into_iter().take(max_nodes).collect();
    let nodes = graph
        .nodes
        .into_values()
        .filter(|n| keep.contains(&n.id))
        .collect();
    let edges = graph
        .edges
        .into_values()
        .filter(|e| keep.contains(&e.from) && keep.contains(&e.to))
        .collect();
    Some(Shaped {
        nodes,
        edges,
        truncated,
    })
}

/// The app whose functions a focus unfolds: the app itself, or a function's.
fn expanded_app(graph: &Graph, focus: &str) -> Option<String> {
    let node = graph.nodes.get(focus)?;
    match node.kind {
        NodeKind::App => Some(node.id.clone()),
        NodeKind::Function => node.parent.clone(),
        _ => None,
    }
}

/// Every function not of `expanded` replaced by its app: its node dropped,
/// its edges moved onto the app and merged with the app's own, each
/// edge keeping which end's functions it came from.
fn fold_functions(graph: &mut Graph, expanded: Option<&str>) {
    let folded: HashMap<String, (String, String)> = graph
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Function && n.parent.as_deref() != expanded)
        .filter_map(|n| Some((n.id.clone(), (n.parent.clone()?, n.label.clone()))))
        .collect();
    if folded.is_empty() {
        return;
    }
    for id in folded.keys() {
        graph.nodes.remove(id);
    }
    for (_, mut edge) in std::mem::take(&mut graph.edges) {
        if let Some((app, function)) = folded.get(edge.from.as_str()) {
            edge.from_functions.insert(function.clone());
            edge.from = app.clone();
        }
        if let Some((app, function)) = folded.get(edge.to.as_str()) {
            edge.to_functions.insert(function.clone());
            edge.to = app.clone();
        }
        merge(graph, edge);
    }
}

/// Add `edge`, or fold it into the edge already there with its ends and kind.
fn merge(graph: &mut Graph, edge: FlowEdge) {
    let key = (edge.from.clone(), edge.to.clone(), edge.kind);
    match graph.edges.get_mut(&key) {
        None => {
            graph.edges.insert(key, edge);
        }
        Some(seen) => {
            seen.via.extend(edge.via);
            seen.tables.extend(edge.tables);
            seen.from_functions.extend(edge.from_functions);
            seen.to_functions.extend(edge.to_functions);
            seen.events.extend(edge.events);
            seen.problems.extend(edge.problems);
            // Approved only when every folded upload is.
            seen.approved = match (seen.approved, edge.approved) {
                (Some(a), Some(b)) => Some(a && b),
                (a, b) => a.or(b),
            };
        }
    }
}

/// Neighbours either way along an edge, and a function's app (and back), in
/// the graph's own order.
fn adjacency(graph: &Graph) -> HashMap<&str, Vec<&str>> {
    let mut pairs: Vec<(&str, &str)> = graph
        .edges
        .values()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    pairs.extend(graph.nodes.values().filter_map(|n| {
        let parent = n.parent.as_deref()?;
        graph
            .nodes
            .contains_key(parent)
            .then_some((n.id.as_str(), parent))
    }));
    let mut adjacent: HashMap<&str, Vec<&str>> = HashMap::new();
    for (a, b) in pairs {
        adjacent.entry(a).or_default().push(b);
        adjacent.entry(b).or_default().push(a);
    }
    adjacent
}

/// Breadth first from `starts`, at most `depth` hops when bounded: the nodes
/// reached, nearest first.
fn walk(graph: &Graph, starts: &[String], depth: Option<u32>) -> Vec<String> {
    let adjacent = adjacency(graph);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut frontier: VecDeque<(&str, u32)> = VecDeque::new();
    for start in starts {
        if seen.insert(start.as_str()) {
            order.push(start.clone());
            frontier.push_back((start.as_str(), 0));
        }
    }
    while let Some((id, hops)) = frontier.pop_front() {
        if depth.is_some_and(|max| hops >= max) {
            continue;
        }
        for &next in adjacent.get(id).into_iter().flatten() {
            if seen.insert(next) {
                order.push(next.to_string());
                frontier.push_back((next, hops + 1));
            }
        }
    }
    order
}

/// Every node: outward from the apps first, then whatever reaches no app, in
/// id order.
fn whole(graph: &Graph) -> Vec<String> {
    let apps: Vec<String> = graph
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::App)
        .map(|n| n.id.clone())
        .collect();
    let mut order = walk(graph, &apps, None);
    let reached: HashSet<String> = order.iter().cloned().collect();
    order.extend(
        graph
            .nodes
            .keys()
            .filter(|id| !reached.contains(*id))
            .cloned(),
    );
    order
}
