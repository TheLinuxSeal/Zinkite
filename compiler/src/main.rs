//! `zink`, the command-line tool (DESIGN §14.4).

use std::path::Path;
use std::process::ExitCode;

use zink::diag::{self, Severity};
use zink::source::SourceFile;
use zink::{lexer, sexpr};

const USAGE: &str = "\
usage: zink <command> [arguments]

commands:
  zink run [-O] FILE [args...]  compile into the cache if needed, then run it
  zink FILE [args...]           the same as `zink run`, for #!/usr/bin/env zink
  zink build FILE -o OUT        compile to an executable, with the system C compiler at -O2
  zink check FILE...            parse and type-check each file as its own program
  zink c FILE                   print the C generated for a program
  zink cache clean              empty the cache
  zink parse [--tree] FILE...   parse each file and report errors; --tree prints the syntax tree
  zink lex FILE                 print the tokens of a file
";

/// Output piped to a program that stops reading, as `zink c f.zn | head`, ends `zink`
/// quietly, as it does other Unix tools, rather than as a failed write.
#[cfg(unix)]
fn default_sigpipe() {
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: restores the default action for one signal, before any thread starts.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn default_sigpipe() {}

fn main() -> ExitCode {
    default_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(&args[1..]),
        Some("parse") => parse(&args[1..]),
        Some("lex") => lex(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("build") => build(&args[1..]),
        Some("c") => emit_c(&args[1..]),
        Some("cache") if args.get(1).is_some_and(|a| a == "clean") => {
            match zink::driver::clean_cache() {
                Ok(n) => {
                    eprintln!("removed {}", plural(n, "cached program"));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("zink: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
        Some("-h" | "--help") | None => {
            println!("{}", USAGE);
            ExitCode::SUCCESS
        }
        Some(other) if other.ends_with(".zn") || Path::new(other).is_file() => run(&args),
        Some(other) => {
            eprintln!("zink: unknown command `{}`\n\n{}", other, USAGE);
            ExitCode::from(2)
        }
    }
}

fn read(path: &str) -> Option<SourceFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(SourceFile::new(path, text)),
        Err(e) => {
            eprintln!("zink: can't read {}: {}", path, e);
            None
        }
    }
}

/// Compiles one program to C, printing its diagnostics.
fn to_c(path: &str) -> Option<String> {
    let file = read(path)?;
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    let mut errors = 0;
    for d in &diags[0] {
        eprint!("{}", diag::render(&file, d));
        if d.severity == Severity::Error {
            errors += 1;
        }
    }
    if errors > 0 {
        eprintln!("{} in {}", plural(errors, "error"), path);
    }
    c
}

fn run(args: &[String]) -> ExitCode {
    let optimize = args.first().is_some_and(|a| a == "-O");
    let rest = if optimize { &args[1..] } else { args };
    let Some(path) = rest.first() else {
        eprintln!("usage: zink run [-O] FILE [args...]");
        return ExitCode::from(2);
    };
    let Some(c) = to_c(path) else {
        return ExitCode::FAILURE;
    };
    match zink::driver::cached(&c, optimize) {
        Ok(exe) => {
            let e = zink::driver::exec(&exe, &rest[1..]);
            eprintln!("zink: {}", e);
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("zink: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn build(args: &[String]) -> ExitCode {
    let mut out = None;
    let mut file = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            out = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if args[i].starts_with('-') {
            eprintln!(
                "zink: unknown option `{}`\nusage: zink build FILE -o OUT",
                args[i]
            );
            return ExitCode::from(2);
        }
        if file.is_some() {
            eprintln!(
                "zink: a program is built from one file, and `{}` is a second\nusage: zink build FILE -o OUT",
                args[i]
            );
            return ExitCode::from(2);
        }
        file = Some(args[i].clone());
        i += 1;
    }
    let (Some(file), Some(out)) = (file, out) else {
        eprintln!("usage: zink build FILE -o OUT");
        return ExitCode::from(2);
    };
    let Some(c) = to_c(&file) else {
        return ExitCode::FAILURE;
    };
    match zink::driver::compile(&c, Path::new(&out), true) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zink: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn emit_c(args: &[String]) -> ExitCode {
    let [path] = args else {
        eprintln!("usage: zink c FILE");
        return ExitCode::from(2);
    };
    match to_c(path) {
        Some(c) => {
            print!("{}", c);
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

fn check(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("usage: zink check FILE...");
        return ExitCode::from(2);
    }
    let (mut errors, mut warnings) = (0, 0);
    for path in args {
        let Some(file) = read(path) else {
            errors += 1;
            continue;
        };
        let diags = zink::check(std::slice::from_ref(&file));
        for d in &diags[0] {
            eprint!("{}", diag::render(&file, d));
            match d.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
            }
        }
    }
    let files = plural(args.len(), "file");
    if errors > 0 {
        eprintln!(
            "{} and {} in {}",
            plural(errors, "error"),
            plural(warnings, "warning"),
            files
        );
        ExitCode::FAILURE
    } else if warnings > 0 {
        eprintln!(
            "checked {} with no errors and {}",
            files,
            plural(warnings, "warning")
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("checked {} with no errors", files);
        ExitCode::SUCCESS
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{} {}{}", n, word, if n == 1 { "" } else { "s" })
}

fn parse(args: &[String]) -> ExitCode {
    let tree = args.iter().any(|a| a == "--tree");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if files.is_empty() {
        eprintln!("usage: zink parse [--tree] FILE...");
        return ExitCode::from(2);
    }
    let mut errors = 0;
    for path in &files {
        let Some(file) = read(path) else {
            errors += 1;
            continue;
        };
        let (ast, diags) = zink::parse(&file);
        for d in &diags {
            eprint!("{}", diag::render(&file, d));
            if d.severity == Severity::Error {
                errors += 1;
            }
        }
        if tree {
            print_tree(&ast);
        }
    }
    if errors > 0 {
        eprintln!(
            "{} error{} in {} file{}",
            errors,
            if errors == 1 { "" } else { "s" },
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
        ExitCode::FAILURE
    } else {
        eprintln!(
            "parsed {} file{} with no errors",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
        ExitCode::SUCCESS
    }
}

fn print_tree(file: &zink::ast::File) {
    use zink::ast::*;
    fn members(ms: &[Member], indent: &str) {
        for m in ms {
            match m {
                Member::Field(f) => println!(
                    "{}field {} {}",
                    indent,
                    sexpr::ty(&f.ty),
                    f.vars
                        .iter()
                        .map(|v| v.name.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Member::Method(m) => method(m, indent),
                Member::Ctor(c) => {
                    println!(
                        "{}new({})",
                        indent,
                        c.params
                            .iter()
                            .map(|p| format!("{} {}", sexpr::ty(&p.ty), sexpr::name(&p.name)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    block(&c.body, &format!("{}  ", indent));
                }
                Member::If(_) => println!("{}#if ...", indent),
            }
        }
    }
    fn method(m: &MethodDecl, indent: &str) {
        let params = m
            .params
            .iter()
            .map(|p| format!("{} {}", sexpr::ty(&p.ty), sexpr::name(&p.name)))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{}{} {}({})",
            indent,
            sexpr::ret(&m.ret),
            m.name.name,
            params
        );
        if let Some(b) = &m.body {
            block(b, &format!("{}  ", indent));
        }
    }
    fn block(b: &Block, indent: &str) {
        for s in &b.stmts {
            println!("{}{}", indent, sexpr::stmt(s));
            let inner: Vec<&Stmt> = match &s.kind {
                StmtKind::Block(b) => b.stmts.iter().collect(),
                StmtKind::If { then, els, .. } => std::iter::once(then.as_ref())
                    .chain(els.as_deref())
                    .collect(),
                StmtKind::While { body, .. }
                | StmtKind::Repeat { body, .. }
                | StmtKind::For { body, .. }
                | StmtKind::ForEach { body, .. }
                | StmtKind::DoWhile { body, .. } => vec![body.as_ref()],
                _ => vec![],
            };
            for s in inner {
                if let StmtKind::Block(b) = &s.kind {
                    block(b, &format!("{}  ", indent));
                } else {
                    println!("{}  {}", indent, sexpr::stmt(s));
                }
            }
        }
    }
    for item in &file.items {
        match item {
            TopItem::Type(t) => match &t.kind {
                TypeDeclKind::Class(c) | TypeDeclKind::Root(c) => {
                    let kind = if matches!(t.kind, TypeDeclKind::Root(_)) {
                        "root"
                    } else {
                        "class"
                    };
                    println!("{} {}", kind, c.name.name);
                    members(&c.members, "  ");
                }
                TypeDeclKind::Interface(i) => {
                    println!("interface {} ({} methods)", i.name.name, i.members.len())
                }
                TypeDeclKind::Enum(e) => {
                    println!("enum {} ({} constants)", e.name.name, e.constants.len())
                }
            },
            TopItem::Field(f) => println!(
                "field {} {}",
                sexpr::ty(&f.ty),
                f.vars
                    .iter()
                    .map(|v| v.name.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TopItem::Method(m) => method(m, ""),
            TopItem::If(_) => println!("#if ..."),
        }
    }
}

fn lex(args: &[String]) -> ExitCode {
    let [path] = args else {
        eprintln!("usage: zink lex FILE");
        return ExitCode::from(2);
    };
    let Some(file) = read(path) else {
        return ExitCode::FAILURE;
    };
    let (tokens, diags) = lexer::lex(&file.text);
    for t in &tokens {
        let (line, col) = file.line_col(t.span.start as usize);
        println!("{}:{}\t{:?}", line, col, t.tok);
    }
    for d in &diags {
        eprint!("{}", diag::render(&file, d));
    }
    if diags.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
