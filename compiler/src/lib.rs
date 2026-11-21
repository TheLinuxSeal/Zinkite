//! The Zinkite compiler. So far: the lexer, the parser and the type checker.

pub mod ast;
pub mod builtins;
pub mod check;
pub mod diag;
pub mod flow;
pub mod lexer;
pub mod lifetimes;
pub mod parser;
pub mod program;
pub mod sexpr;
pub mod source;
pub mod token;
pub mod types;

use diag::{Diagnostic, Severity};
use source::SourceFile;

/// Lexes and parses a file. Diagnostics come back in source order.
pub fn parse(file: &SourceFile) -> (ast::File, Vec<Diagnostic>) {
    let (tokens, mut diags) = lexer::lex(&file.text);
    let (tree, parse_diags) = parser::parse_file(tokens);
    diags.extend(parse_diags);
    diags.sort_by_key(|d| d.span.start);
    (tree, diags)
}

/// Parses and type-checks the files of one program. Returns each file's
/// diagnostics in source order. Type checking runs only when every file parses.
pub fn check(files: &[SourceFile]) -> Vec<Vec<Diagnostic>> {
    let mut per_file = Vec::new();
    let mut asts = Vec::new();
    let mut parsed = true;
    for f in files {
        let (tree, diags) = parse(f);
        parsed &= !diags.iter().any(|d| d.severity == Severity::Error);
        per_file.push(diags);
        asts.push(tree);
    }
    if parsed {
        for (fi, d) in check::check(&asts, files) {
            per_file[fi].push(d);
        }
    }
    for v in &mut per_file {
        v.sort_by_key(|d| d.span.start);
    }
    per_file
}
