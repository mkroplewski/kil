use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedDiagnostic {
    pub message: String,
    pub path: Option<String>,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub file: String,
    pub span: Option<SourceSpan>,
    pub path: Option<String>,
    pub related: Vec<RelatedDiagnostic>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>, file: &Path) -> Self {
        Self {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            file: file.display().to_string(),
            span: None,
            path: None,
            related: Vec::new(),
            help: None,
        }
    }

    pub fn warning(code: impl Into<String>, message: impl Into<String>, file: &Path) -> Self {
        let mut d = Self::error(code, message, file);
        d.severity = Severity::Warning;
        d
    }

    pub fn at_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_span(mut self, span: Option<SourceSpan>) -> Self {
        self.span = span;
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticFormat {
    Text,
    Json,
}

pub fn render_text(diags: &[Diagnostic], source: Option<&str>) -> String {
    let mut out = String::new();
    for diag in diags {
        let level = match diag.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        let _ = writeln!(out, "{level}[{}]: {}", diag.code, diag.message);
        if let Some(span) = diag.span {
            let _ = writeln!(out, "  --> {}:{}:{}", diag.file, span.line, span.column);
            if let Some(src) = source
                && let Some(line) = src.lines().nth(span.line.saturating_sub(1))
            {
                let _ = writeln!(out, "   | {line}");
                let caret = " ".repeat(span.column.saturating_sub(1));
                let _ = writeln!(out, "   | {caret}^");
            }
        } else {
            let _ = writeln!(out, "  --> {}", diag.file);
        }
        if let Some(path) = &diag.path {
            let _ = writeln!(out, "   = path: {path}");
        }
        if let Some(help) = &diag.help {
            let _ = writeln!(out, "   = help: {help}");
        }
    }
    out
}
