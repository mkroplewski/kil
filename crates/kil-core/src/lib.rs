pub mod diagnostic;
pub mod kicad;
pub mod library;
pub mod model;
pub mod pipeline;
pub mod routing;
pub mod source_map;
pub mod validate;

pub use diagnostic::{Diagnostic, DiagnosticFormat, Severity, SourceSpan};
pub use model::KilProject;
pub use pipeline::{
    BuildOptions, BuildOutcome, ExitClass, RouteOptions, RouteOutcome, build, check, load_project,
    route, schema,
};
