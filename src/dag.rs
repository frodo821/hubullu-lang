//! Generic DAG (directed acyclic graph) cycle detector.

use std::collections::{HashMap, HashSet, VecDeque};

/// Check that a directed graph has no cycles using Kahn's algorithm.
/// Returns the cycle members if a cycle is detected.
pub fn check_dag<T: Clone + Eq + std::hash::Hash + std::fmt::Debug>(
    edges: &[(T, T)],
) -> Result<Vec<T>, Vec<T>> {
    let mut in_degree: HashMap<&T, usize> = HashMap::new();
    let mut adjacency: HashMap<&T, Vec<&T>> = HashMap::new();
    let mut nodes: HashSet<&T> = HashSet::new();

    for (from, to) in edges {
        nodes.insert(from);
        nodes.insert(to);
        adjacency.entry(from).or_default().push(to);
        *in_degree.entry(to).or_insert(0) += 1;
        in_degree.entry(from).or_insert(0);
    }

    let mut queue: VecDeque<&T> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&node, _)| node)
        .collect();

    let mut sorted = Vec::new();

    while let Some(node) = queue.pop_front() {
        sorted.push(node.clone());
        if let Some(neighbors) = adjacency.get(node) {
            for &neighbor in neighbors {
                let deg = in_degree.get_mut(neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    if sorted.len() == nodes.len() {
        Ok(sorted)
    } else {
        // Nodes not in sorted output are part of cycles
        let sorted_set: HashSet<_> = sorted.iter().collect();
        let cycle_nodes: Vec<T> = nodes
            .into_iter()
            .filter(|n| !sorted_set.contains(n))
            .cloned()
            .collect();
        Err(cycle_nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_ok() {
        let edges = vec![("a", "b"), ("b", "c"), ("a", "c")];
        assert!(check_dag(&edges).is_ok());
    }

    #[test]
    fn test_dag_cycle() {
        let edges = vec![("a", "b"), ("b", "c"), ("c", "a")];
        assert!(check_dag(&edges).is_err());
    }

    #[test]
    fn test_dag_empty() {
        let edges: Vec<(&str, &str)> = vec![];
        assert!(check_dag(&edges).is_ok());
    }
}
