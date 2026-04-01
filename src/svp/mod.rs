pub mod highlight;
pub mod ingest;
pub mod parse;
pub mod pipeline;
#[allow(unused_imports)]
pub mod pointer;
pub mod projector;
pub mod resolver;
pub mod semantic;
pub mod sync;

#[cfg(test)]
mod tests;

pub use ingest::ingest_svp_file;
#[allow(unused_imports)]
pub use pointer::SvpPointer;
pub use resolver::{RequestPriority, SvpResolver};
