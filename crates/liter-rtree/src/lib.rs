//! R*Tree extension for Liter-rs.
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
        self.bounds
            .iter()
            .fold(1.0, |vol, b| vol * (b.max - b.min).max(0.0))
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
            return (
                RTreeNode {
                    is_leaf: self.is_leaf,
                    entries: vec![],
                },
                RTreeNode {
                    is_leaf: self.is_leaf,
                    entries: vec![],
                },
            );
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

        let mut node1 = RTreeNode {
            is_leaf: self.is_leaf,
            entries: Vec::new(),
        };
        let mut node2 = RTreeNode {
            is_leaf: self.is_leaf,
            entries: Vec::new(),
        };

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
    fn test_rtree_table_new() {
        let table = RTreeTable::new("spatial_idx", 2);
        assert_eq!(table.name, "spatial_idx");
        assert_eq!(table.dimensions, 2);
    }

    #[test]
    fn test_volume() {
        let e = RTreeEntry::new(
            1,
            vec![
                BoundingBox { min: 0.0, max: 2.0 },
                BoundingBox { min: 0.0, max: 3.0 },
            ],
        );
        assert_eq!(e.volume(), 6.0);
    }

    #[test]
    fn test_containment_dimension_mismatch() {
        let e = RTreeEntry::new(1, vec![BoundingBox { min: 0.0, max: 1.0 }]);
        let bounds = vec![
            BoundingBox { min: 0.0, max: 1.0 },
            BoundingBox { min: 0.0, max: 1.0 },
        ];
        assert!(!e.is_contained_in(&bounds));
    }

    #[test]
    fn test_linear_split_empty_entries() {
        let mut node = RTreeNode {
            is_leaf: true,
            entries: vec![],
        };
        let (n1, n2) = node.linear_split();
        assert!(n1.entries.is_empty());
        assert!(n2.entries.is_empty());
    }

    #[test]
    fn test_linear_split_min_idx_updates() {
        // Second entry has the smallest min, exercising the min_idx update branch.
        let mut node = RTreeNode {
            is_leaf: true,
            entries: vec![
                RTreeEntry::new(1, vec![BoundingBox { min: 5.0, max: 6.0 }]),
                RTreeEntry::new(
                    2,
                    vec![BoundingBox {
                        min: -10.0,
                        max: -9.0,
                    }],
                ),
                RTreeEntry::new(
                    3,
                    vec![BoundingBox {
                        min: 20.0,
                        max: 21.0,
                    }],
                ),
            ],
        };
        let (n1, n2) = node.linear_split();
        assert_eq!(n1.entries.len() + n2.entries.len(), 3);
    }

    #[test]
    fn test_linear_split_overlapping_seeds_fallback() {
        // All entries share identical bounds so min_idx == max_idx, exercising the fallback.
        let mut node = RTreeNode {
            is_leaf: true,
            entries: vec![
                RTreeEntry::new(1, vec![BoundingBox { min: 1.0, max: 1.0 }]),
                RTreeEntry::new(2, vec![BoundingBox { min: 1.0, max: 1.0 }]),
                RTreeEntry::new(3, vec![BoundingBox { min: 1.0, max: 1.0 }]),
            ],
        };
        let (n1, n2) = node.linear_split();
        assert_eq!(n1.entries.len() + n2.entries.len(), 3);
    }

    #[test]
    fn test_linear_split_distributes_to_smaller_node() {
        // With 4 entries, after seeding node1/node2 with 1 each, the two
        // remaining entries are distributed one to each side, exercising the
        // branch that pushes onto the currently-smaller node (node1).
        let mut node = RTreeNode {
            is_leaf: true,
            entries: vec![
                RTreeEntry::new(1, vec![BoundingBox { min: 0.0, max: 1.0 }]),
                RTreeEntry::new(
                    2,
                    vec![BoundingBox {
                        min: 10.0,
                        max: 11.0,
                    }],
                ),
                RTreeEntry::new(3, vec![BoundingBox { min: 5.0, max: 6.0 }]),
                RTreeEntry::new(4, vec![BoundingBox { min: 7.0, max: 8.0 }]),
            ],
        };
        let (n1, n2) = node.linear_split();
        assert_eq!(n1.entries.len(), 2);
        assert_eq!(n2.entries.len(), 2);
    }

    #[test]
    fn test_containment() {
        let e = RTreeEntry::new(
            1,
            vec![
                BoundingBox { min: 2.0, max: 4.0 },
                BoundingBox { min: 2.0, max: 4.0 },
            ],
        );
        let bounds = vec![
            BoundingBox {
                min: 0.0,
                max: 10.0,
            },
            BoundingBox {
                min: 0.0,
                max: 10.0,
            },
        ];
        assert!(e.is_contained_in(&bounds));

        let bounds2 = vec![
            BoundingBox {
                min: 3.0,
                max: 10.0,
            },
            BoundingBox {
                min: 0.0,
                max: 10.0,
            },
        ];
        assert!(!e.is_contained_in(&bounds2));
    }

    #[test]
    fn test_linear_split() {
        let mut node = RTreeNode {
            is_leaf: true,
            entries: vec![
                RTreeEntry::new(1, vec![BoundingBox { min: 0.0, max: 1.0 }]),
                RTreeEntry::new(
                    2,
                    vec![BoundingBox {
                        min: 10.0,
                        max: 11.0,
                    }],
                ),
                RTreeEntry::new(3, vec![BoundingBox { min: 5.0, max: 6.0 }]),
            ],
        };
        let (n1, n2) = node.linear_split();
        assert_eq!(n1.entries.len() + n2.entries.len(), 3);
    }
}
