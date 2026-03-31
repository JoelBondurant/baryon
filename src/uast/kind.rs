/// ==========================================
/// SEMANTIC COMPONENTS
/// ==========================================
/// The logical meaning of the node. Unifies Relational (CSV) and Logical (Rust/SQL).
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum SemanticKind {
	RelationalTable,
	RelationalRow,
	Token,
}
