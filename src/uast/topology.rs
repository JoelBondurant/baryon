use crate::ecs::id::NodeId;

/// The Edges of the DAG/Tree.
/// Instead of `Vec<NodeId>` which causes heap fragmentation, we use a
/// Left-Child / Right-Sibling (LCRS) tree representation.
/// This guarantees every node's topological footprint is exactly 12 bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct TreeEdges {
	pub parent: Option<NodeId>,
	pub first_child: Option<NodeId>,
	pub next_sibling: Option<NodeId>,
}
