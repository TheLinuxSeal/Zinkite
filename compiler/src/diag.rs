//! Diagnostics: errors and warnings with a span, rendered with the source line.

use crate::source::{SourceFile, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            span,
            message: message.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help = Some(help.into());
        self
    }
}

/// `file:line:col: error: message`, then the line and a caret under the span.
pub fn render(file: &SourceFile, d: &Diagnostic) -> String {
    let (line, col) = file.line_col(d.span.start as usize);
    let kind = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let text = file.line_text(line);
    let (end_line, end_col) = file.line_col(d.span.end as usize);
    let width = if end_line == line && end_col > col {
        end_col - col
    } else {
        1
    };
    let pad: String = text
        .chars()
        .take(col - 1)
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    let mut out = format!("{}:{}:{}: {}: {}\n", file.name, line, col, kind, d.message);
    out.push_str(&format!("    {}\n    {}{}\n", text, pad, "^".repeat(width)));
    if let Some(h) = &d.help {
        out.push_str(&format!("    help: {}\n", h));
    }
    out
}
