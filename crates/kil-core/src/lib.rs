pub mod diagnostic;
pub mod kicad;
pub mod library;
pub mod model;
pub mod modules;
pub mod pipeline;
pub mod routing;
pub mod source;
pub mod source_map;
pub mod validate;

pub use diagnostic::{Diagnostic, DiagnosticFormat, Severity, SourceSpan};
pub use model::ResolvedProject;
pub use pipeline::{
    BuildOptions, BuildOutcome, ExitClass, InspectOptions, InspectOutcome, RouteOptions,
    RouteOutcome, build, check, inspect, load_project, module_schema, route, schema,
};
