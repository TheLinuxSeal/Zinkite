//! The Zinkite compiler. So far: the lexer, the parser and the type checker.

pub mod ast;
pub mod builtins;
pub mod cheader;
pub mod check;
pub mod codegen;
pub mod diag;
pub mod driver;
pub mod flow;
pub mod lexer;
pub mod lifetimes;
pub mod parser;
pub mod program;
pub mod sexpr;
pub mod source;
pub mod stdlib;
pub mod token;
pub mod types;

use diag::{Diagnostic, Severity};
use source::SourceFile;

/// Lexes and parses a file. Diagnostics come back in source order.
pub fn parse(file: &SourceFile) -> (ast::File, Vec<Diagnostic>) {
    let (mut tokens, mut diags) = lexer::lex(&file.text);
    if file.base != 0 {
        shift(&mut tokens, file.base);
        for d in &mut diags {
            d.span = moved(d.span, file.base);
        }
    }
    let (tree, parse_diags) = parser::parse_file(tokens);
    diags.extend(parse_diags);
    diags.sort_by_key(|d| d.span.start);
    (tree, diags)
}

fn moved(s: source::Span, by: u32) -> source::Span {
    source::Span {
        start: s.start + by,
        end: s.end + by,
    }
}

/// Moves tokens into their file's range of spans, interpolations included.
fn shift(tokens: &mut [token::Token], by: u32) {
    for t in tokens {
        t.span = moved(t.span, by);
        if let token::Tok::Str(pieces) = &mut t.tok {
            for p in pieces {
                if let token::StrPiece::Interp(inner, span) = p {
                    *span = moved(*span, by);
                    shift(inner, by);
                }
            }
        }
    }
}

/// The program's files parsed, with the library files their imports need added after
/// them, transitively (DESIGN §8.4): every source, its tree and its diagnostics, and
/// whether everything parsed.
struct Parsed {
    files: Vec<SourceFile>,
    asts: Vec<ast::File>,
    diags: Vec<Vec<Diagnostic>>,
    ok: bool,
}

fn parse_with_library(user: &[SourceFile]) -> Parsed {
    let mut p = Parsed {
        files: user.to_vec(),
        asts: Vec::new(),
        diags: Vec::new(),
        ok: true,
    };
    let mut added: Vec<String> = Vec::new();
    let mut i = 0;
    while i < p.files.len() {
        // Each file's spans follow the previous file's.
        if i > 0 {
            let prev = &p.files[i - 1];
            p.files[i].base = prev.base + prev.text.len() as u32 + 1;
        }
        let (tree, mut diags) = parse(&p.files[i]);
        if i < user.len()
            && let Some(q) = &tree.package
            && stdlib::reserved(&q.text())
        {
            diags.push(Diagnostic::error(
                q.span,
                "`zink.*` is reserved for the standard library (DESIGN §8.4)",
            ));
        }
        p.ok &= !diags.iter().any(|d| d.severity == Severity::Error);
        for pkg in stdlib::imported_packages(&tree) {
            if stdlib::has_package(&pkg) && !added.contains(&pkg) {
                p.files.extend(stdlib::sources(&pkg));
                added.push(pkg);
            }
        }
        p.diags.push(diags);
        p.asts.push(tree);
        i += 1;
    }
    p
}

/// The user's files' diagnostics. An error in a library file is a bug in zink, so it
/// is reported on the first file, with where it is; library warnings are dropped.
fn user_diags(p: Parsed, users: usize) -> Vec<Vec<Diagnostic>> {
    // Back to each file's own offsets, for the caller's sources.
    let mut out: Vec<Vec<Diagnostic>> = p.diags[..users]
        .iter()
        .zip(&p.files)
        .map(|(ds, f)| {
            ds.iter()
                .map(|d| {
                    let mut d = d.clone();
                    d.span.start = d.span.start.saturating_sub(f.base);
                    d.span.end = d.span.end.saturating_sub(f.base);
                    d
                })
                .collect()
        })
        .collect();
    for (f, ds) in p.files.iter().zip(&p.diags).skip(users) {
        for d in ds.iter().filter(|d| d.severity == Severity::Error) {
            let (line, col) = f.line_col(d.span.start as usize);
            if let Some(first) = out.first_mut() {
                first.push(Diagnostic::error(
                    source::Span::default(),
                    format!(
                        "the standard library didn't compile, which is a bug in zink: {}:{}:{}: {}",
                        f.name, line, col, d.message
                    ),
                ));
            }
        }
    }
    for v in &mut out {
        v.sort_by_key(|d| d.span.start);
    }
    out
}

/// Parses and type-checks the files of one program. Returns each file's
/// diagnostics in source order. Type checking runs only when every file parses.
pub fn check(files: &[SourceFile]) -> Vec<Vec<Diagnostic>> {
    let mut p = parse_with_library(files);
    if p.ok {
        for (fi, d) in check::check(&p.asts, &p.files) {
            p.diags[fi].push(d);
        }
    }
    user_diags(p, files.len())
}

/// Checks a program and generates its C. Returns each file's diagnostics, and the C
/// when there were no errors.
pub fn compile_c(files: &[SourceFile]) -> (Vec<Vec<Diagnostic>>, Option<String>) {
    let mut p = parse_with_library(files);
    let mut c = None;
    if p.ok {
        let (diags, code) = check::check_and_generate(&p.asts, &p.files);
        for (fi, d) in diags {
            p.diags[fi].push(d);
        }
        c = code;
    }
    let diags = user_diags(p, files.len());
    (diags, c)
}
