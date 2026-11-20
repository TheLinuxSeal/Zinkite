//! `zink`, the command-line tool (DESIGN §14.4). Only the front end exists so
//! far, so the commands that work are `zink parse` and `zink lex`.

use std::process::ExitCode;

use zink::diag::{self, Severity};
use zink::source::SourceFile;
use zink::{lexer, sexpr};

const USAGE: &str = "\
usage: zink <command> [arguments]

commands that work so far:
  zink parse [--tree] FILE...   parse each file and report errors; --tree prints the syntax tree
  zink lex FILE                 print the tokens of a file

planned (DESIGN §14.4):
  zink build FILE -o OUT, zink run FILE [args...], zink FILE [args...]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("parse") => parse(&args[1..]),
        Some("lex") => lex(&args[1..]),
        Some("build" | "run") => {
            eprintln!(
                "zink: `{}` isn't implemented yet; only `zink parse` and `zink lex` work so far",
                args[0]
            );
            ExitCode::from(2)
        }
        Some("-h" | "--help") | None => {
            println!("{}", USAGE);
            ExitCode::SUCCESS
        }
        Some(other) if other.ends_with(".zn") => {
            eprintln!(
                "zink: running programs isn't implemented yet; try `zink parse {}`",
                other
            );
            ExitCode::from(2)
        }
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
