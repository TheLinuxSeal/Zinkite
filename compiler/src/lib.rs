//! The Zinkite compiler. So far: the lexer and the parser.

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;
pub mod sexpr;
pub mod source;
pub mod token;

use diag::Diagnostic;
use source::SourceFile;

/// Lexes and parses a file. Diagnostics come back in source order.
pub fn parse(file: &SourceFile) -> (ast::File, Vec<Diagnostic>) {
    let (tokens, mut diags) = lexer::lex(&file.text);
    let (tree, parse_diags) = parser::parse_file(tokens);
    diags.extend(parse_diags);
    diags.sort_by_key(|d| d.span.start);
    (tree, diags)
}
