//! The documentation's examples (`docs/*.md`), checked as the compiler sees them, so the
//! documentation can't drift from the language (`docs/README.md`, "Examples").
//!
//! - Every ```java block that declares `main` is a complete program, and must type-check
//!   with no diagnostics, unless an ```error or ```warning block follows it.
//! - An ```output block after one runs it and compares its stdout, exactly.
//! - A ```stderr block runs it and checks that each of its lines is in its stderr.
//! - An ```error block checks that it doesn't compile, and that each of the block's lines
//!   is in what the compiler printed; a ```warning block, that it compiles with them.
//! - `exit=N` after `output` or `stderr` checks the exit status, which is otherwise 0
//!   with an ```output block and anything with a ```stderr one.
//! - `java no-check` is shown and not checked: a program that needs SDL, say.
//!
//! The program is named `example.zn` in messages. `ZINK_DOCS=03-text.md` checks one file.

use std::path::{Path, PathBuf};
use std::process::Command;

use zink::diag::{Severity, render};
use zink::source::SourceFile;

struct Block {
    info: String,
    text: String,
    line: usize,
}

/// The fenced blocks of a Markdown file, with each block's indentation removed.
fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(String, usize, usize, Vec<String>)> = None;
    for (i, line) in md.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        match &mut open {
            None => {
                if let Some(info) = trimmed.strip_prefix("```") {
                    open = Some((info.trim().to_string(), indent, i + 1, Vec::new()));
                }
            }
            Some((info, ind, start, lines)) => {
                if trimmed.starts_with("```") {
                    out.push(Block {
                        info: std::mem::take(info),
                        text: lines.join("\n") + "\n",
                        line: *start,
                    });
                    open = None;
                } else {
                    let cut = line
                        .char_indices()
                        .take_while(|(k, c)| *k < *ind && *c == ' ')
                        .count();
                    lines.push(line[cut..].to_string());
                }
            }
        }
    }
    out
}

/// A program and what the blocks after it expect of it.
struct Example {
    file: String,
    line: usize,
    text: String,
    output: Option<String>,
    stderr: Option<String>,
    error: Option<String>,
    warning: Option<String>,
    exit: Option<i32>,
}

fn examples(path: &Path) -> Vec<Example> {
    let md = std::fs::read_to_string(path).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let mut out: Vec<Example> = Vec::new();
    let mut current: Option<Example> = None;
    for b in blocks(&md) {
        let mut words = b.info.split_whitespace();
        let kind = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        let exit = rest
            .iter()
            .find_map(|w| w.strip_prefix("exit="))
            .and_then(|n| n.parse().ok());
        match kind {
            "java" => {
                if let Some(e) = current.take() {
                    out.push(e);
                }
                let has_main = b.text.lines().any(|l| {
                    let l = l.trim_start();
                    (l.starts_with("void main(") || l.starts_with("u8 main("))
                        && !l.starts_with("//")
                });
                if has_main && !rest.contains(&"no-check") {
                    current = Some(Example {
                        file: name.clone(),
                        line: b.line,
                        text: b.text,
                        output: None,
                        stderr: None,
                        error: None,
                        warning: None,
                        exit: None,
                    });
                }
            }
            "output" | "stderr" | "error" | "warning" => {
                if let Some(e) = &mut current {
                    let slot = match kind {
                        "output" => &mut e.output,
                        "stderr" => &mut e.stderr,
                        "error" => &mut e.error,
                        _ => &mut e.warning,
                    };
                    *slot = Some(b.text);
                    if exit.is_some() {
                        e.exit = exit;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(e) = current.take() {
        out.push(e);
    }
    out
}

fn has_clang() -> bool {
    Command::new("clang")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Each line of `want` that isn't blank, in `got`: why not, or None.
fn lines_in(want: &str, got: &str, what: &str) -> Option<String> {
    for l in want
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
    {
        if !got.contains(l.trim_start()) {
            return Some(format!(
                "{} has no line `{}`; it was:\n{}",
                what,
                l.trim(),
                got
            ));
        }
    }
    None
}

/// What's wrong with an example, or None.
fn check(e: &Example, clang: bool) -> Option<String> {
    if e.text.contains("extern import") && !clang {
        return None;
    }
    let file = SourceFile::new("example.zn", e.text.clone());
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    let shown: String = diags[0].iter().map(|d| render(&file, d)).collect();
    let errors = diags[0].iter().any(|d| d.severity == Severity::Error);
    let notes = diags[0].iter().any(|d| {
        !(d.message.contains("isn't written yet") || d.message.contains("aren't written yet"))
    });
    if let Some(want) = &e.error {
        if !errors {
            return Some(format!("expected errors, and it compiled:\n{}", shown));
        }
        return lines_in(want, &shown, "what the compiler printed");
    }
    if errors {
        return Some(format!("doesn't compile:\n{}", shown));
    }
    if let Some(want) = &e.warning {
        if let Some(why) = lines_in(want, &shown, "what the compiler printed") {
            return Some(why);
        }
    } else if notes {
        return Some(format!("has diagnostics:\n{}", shown));
    }
    if e.output.is_none() && e.stderr.is_none() {
        return None;
    }
    let c = c?;
    let exe =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("docs-{}-{}", e.file, e.line));
    if let Err(why) = zink::driver::compile(&c, &exe, false) {
        return Some(format!("the C didn't compile: {}", why));
    }
    let run = Command::new(&exe)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("ran");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let code = run.status.code().unwrap_or(-1);
    if let Some(want) = &e.output
        && stdout != want.as_str()
    {
        return Some(format!(
            "printed:\n{}\nwhere the block says:\n{}\nstderr: {}",
            stdout, want, stderr
        ));
    }
    if let Some(want) = &e.stderr
        && let Some(why) = lines_in(want, &stderr, "stderr")
    {
        return Some(why);
    }
    let want_code = e.exit.or(if e.stderr.is_none() { Some(0) } else { None });
    if let Some(w) = want_code
        && code != w
    {
        return Some(format!(
            "exited with {}, not {}; stderr: {}",
            code, w, stderr
        ));
    }
    None
}

#[test]
fn documentation_examples() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs");
    let only = std::env::var("ZINK_DOCS").ok();
    let mut all = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter(|p| {
            only.as_ref().is_none_or(|o| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy() == o.as_str())
            })
        })
        .collect();
    paths.sort();
    for p in &paths {
        all.extend(examples(p));
    }
    let clang = has_clang();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = all.len().div_ceil(threads).max(1);
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = all
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .filter_map(|e| {
                            check(e, clang)
                                .map(|why| format!("docs/{}:{}: {}", e.file, e.line, why))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    assert!(
        failures.is_empty(),
        "{} of {} examples are wrong:\n\n{}",
        failures.len(),
        all.len(),
        failures.join("\n\n")
    );
    if only.is_none() {
        assert!(
            all.len() >= 10,
            "found only {} examples in docs/",
            all.len()
        );
    }
}
