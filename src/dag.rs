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

    #[test]
    fn test_dag_self_loop() {
        let edges = vec![("a", "a")];
        let err = check_dag(&edges).unwrap_err();
        assert!(err.contains(&"a"));
    }

    #[test]
    fn test_dag_diamond() {
        //   a
        //  / \
        // b   c
        //  \ /
        //   d
        let edges = vec![("a", "b"), ("a", "c"), ("b", "d"), ("c", "d")];
        let sorted = check_dag(&edges).unwrap();
        // a must come before b and c; b and c must come before d
        let pos = |n: &&str| sorted.iter().position(|x| x == n).unwrap();
        assert!(pos(&"a") < pos(&"b"));
        assert!(pos(&"a") < pos(&"c"));
        assert!(pos(&"b") < pos(&"d"));
        assert!(pos(&"c") < pos(&"d"));
    }

    #[test]
    fn test_dag_disconnected_components() {
        let edges = vec![("a", "b"), ("c", "d")];
        let sorted = check_dag(&edges).unwrap();
        assert_eq!(sorted.len(), 4);
    }

    #[test]
    fn test_dag_partial_cycle() {
        // a -> b -> c -> b (cycle), but d -> a is fine
        let edges = vec![("d", "a"), ("a", "b"), ("b", "c"), ("c", "b")];
        let err = check_dag(&edges).unwrap_err();
        // b and c are in the cycle
        assert!(err.contains(&"b"));
        assert!(err.contains(&"c"));
        // d and a are not in the cycle
        assert!(!err.contains(&"d"));
        assert!(!err.contains(&"a"));
    }

    #[test]
    fn test_dag_linear_chain() {
        let edges = vec![("a", "b"), ("b", "c"), ("c", "d"), ("d", "e")];
        let sorted = check_dag(&edges).unwrap();
        assert_eq!(sorted, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_dag_integer_nodes() {
        let edges = vec![(1, 2), (2, 3), (1, 3)];
        let sorted = check_dag(&edges).unwrap();
        let pos = |n: &i32| sorted.iter().position(|x| x == n).unwrap();
        assert!(pos(&1) < pos(&2));
        assert!(pos(&2) < pos(&3));
    }

    #[test]
    fn test_dag_multiple_cycles() {
        // Two independent cycles: a->b->a and c->d->c
        let edges = vec![("a", "b"), ("b", "a"), ("c", "d"), ("d", "c")];
        let err = check_dag(&edges).unwrap_err();
        assert_eq!(err.len(), 4);
    }
}
