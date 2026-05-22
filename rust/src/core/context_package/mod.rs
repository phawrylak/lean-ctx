pub mod auto_load;
pub mod builder;
pub mod composition;
pub mod content;
pub mod graph_model;
pub mod loader;
pub mod manifest;
pub mod registry;
pub mod signing;

pub use auto_load::auto_load_packages;
pub use builder::PackageBuilder;
pub use composition::{merge_graphs, MergeReport};
pub use content::PackageContent;
pub use graph_model::{ContextEdge, ContextGraph, ContextNode};
pub use loader::{load_package, LoadReport};
pub use manifest::{PackageLayer, PackageManifest};
pub use registry::LocalRegistry;
pub use signing::{sign_package, verify_signature};
