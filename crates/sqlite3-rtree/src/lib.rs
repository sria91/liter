//! R*Tree extension for SQLite3-rs.
//!
//! Mirrors `rtree.c`. Provides a virtual table for spatial indexing using an
//! R*-tree backed by auxiliary B-tree pages.
//!
//! ## Status
//! Phase 3 — not yet implemented.

/// An R*Tree Virtual Table.
///
/// In a full implementation, this table manages auxiliary B-Tree pages containing
/// spatial node indices (min/max bounds for multiple dimensions).
pub struct RTreeTable {
    pub name: String,
    pub dimensions: usize,
}

impl RTreeTable {
    pub fn new(name: &str, dimensions: usize) -> Self {
        Self {
            name: name.to_string(),
            dimensions,
        }
    }
}

/// A 1D interval for a spatial bounding box.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundingBox {
    pub min: f64,
    pub max: f64,
}

/// A spatial entry in the R-Tree (could map to a rowid or child node).
#[derive(Debug, Clone, PartialEq)]
pub struct RTreeEntry {
    pub id: i64,
    pub bounds: Vec<BoundingBox>,
}

impl RTreeEntry {
    pub fn new(id: i64, bounds: Vec<BoundingBox>) -> Self {
        Self { id, bounds }
    }

    /// Calculates the N-dimensional volume (area) of this entry's bounding box.
    pub fn volume(&self) -> f64 {
        self.bounds.iter().fold(1.0, |vol, b| vol * (b.max - b.min).max(0.0))
    }

    /// Checks if this entry is fully contained within another set of bounds.
    pub fn is_contained_in(&self, other_bounds: &[BoundingBox]) -> bool {
        if self.bounds.len() != other_bounds.len() {
            return false;
        }
        for (i, b) in self.bounds.iter().enumerate() {
            if b.min < other_bounds[i].min || b.max > other_bounds[i].max {
                return false;
            }
        }
        true
    }
}

/// A node in the R-Tree (could be a leaf or internal node).
pub struct RTreeNode {
    pub is_leaf: bool,
    pub entries: Vec<RTreeEntry>,
}

impl RTreeNode {
    /// A simple linear split algorithm for R-Tree nodes.
    /// Returns two new nodes that partition the original entries.
    pub fn linear_split(&mut self) -> (RTreeNode, RTreeNode) {
        if self.entries.is_empty() {
            return (RTreeNode { is_leaf: self.is_leaf, entries: vec![] }, RTreeNode { is_leaf: self.is_leaf, entries: vec![] });
        }

        // Find the two seeds: the entries that are furthest apart in the first dimension.
        let mut min_idx = 0;
        let mut max_idx = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.bounds[0].min < self.entries[min_idx].bounds[0].min {
                min_idx = i;
            }
            if entry.bounds[0].max > self.entries[max_idx].bounds[0].max {
                max_idx = i;
            }
        }
        
        if min_idx == max_idx && self.entries.len() > 1 {
            max_idx = 1; // Fallback if perfectly overlapping
        }

        let mut node1 = RTreeNode { is_leaf: self.is_leaf, entries: Vec::new() };
        let mut node2 = RTreeNode { is_leaf: self.is_leaf, entries: Vec::new() };
        
        let idx2 = std::cmp::max(min_idx, max_idx);
        let idx1 = std::cmp::min(min_idx, max_idx);
        
        if self.entries.len() > idx2 {
            node2.entries.push(self.entries.remove(idx2));
        }
        if self.entries.len() > idx1 {
            node1.entries.push(self.entries.remove(idx1));
        }

        // Distribute the remaining entries evenly
        while let Some(entry) = self.entries.pop() {
            if node1.entries.len() < node2.entries.len() {
                node1.entries.push(entry);
            } else {
                node2.entries.push(entry);
            }
        }
        
        (node1, node2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_containment() {
        let e = RTreeEntry::new(1, vec![BoundingBox { min: 2.0, max: 4.0 }, BoundingBox { min: 2.0, max: 4.0 }]);
        let bounds = vec![BoundingBox { min: 0.0, max: 10.0 }, BoundingBox { min: 0.0, max: 10.0 }];
        assert!(e.is_contained_in(&bounds));

        let bounds2 = vec![BoundingBox { min: 3.0, max: 10.0 }, BoundingBox { min: 0.0, max: 10.0 }];
        assert!(!e.is_contained_in(&bounds2));
    }

    #[test]
    fn test_linear_split() {
        let mut node = RTreeNode {
            is_leaf: true,
            entries: vec![
                RTreeEntry::new(1, vec![BoundingBox { min: 0.0, max: 1.0 }]),
                RTreeEntry::new(2, vec![BoundingBox { min: 10.0, max: 11.0 }]),
                RTreeEntry::new(3, vec![BoundingBox { min: 5.0, max: 6.0 }]),
            ]
        };
        let (n1, n2) = node.linear_split();
        assert_eq!(n1.entries.len() + n2.entries.len(), 3);
    }
}
