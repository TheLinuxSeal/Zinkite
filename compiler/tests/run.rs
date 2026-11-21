//! Programs compiled to C, built with the system C compiler, and run.

use std::process::Command;

use zink::diag::{Severity, render};
use zink::source::SourceFile;

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Compiles and runs a program, failing the test if it doesn't compile.
fn run(text: &str, args: &[&str]) -> Run {
    let file = SourceFile::new("prog.zn", text);
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    let errors: Vec<String> = diags[0]
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| render(&file, d))
        .collect();
    let Some(c) = c else {
        panic!("didn't compile:\n{}", errors.concat())
    };
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let n = std::sync::atomic::AtomicU64::new(0);
    let _ = &n;
    let name = format!("prog-{:x}", {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    });
    let exe = dir.join(name);
    zink::driver::compile(&c, &exe, false).unwrap_or_else(|e| panic!("{}", e));
    let out = Command::new(&exe).args(args).output().expect("ran");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into(),
        stderr: String::from_utf8_lossy(&out.stderr).into(),
        code: out.status.code().unwrap_or(-1),
    }
}

#[track_caller]
fn prints(text: &str, expected: &str) {
    let r = run(text, &[]);
    assert_eq!(r.stdout, expected, "stderr: {}", r.stderr);
    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
}

fn body(stmts: &str) -> String {
    format!("void main() {{\n{}\n}}\n", stmts)
}

#[test]
fn hello() {
    prints(
        "void main() { println(\"hello, world\"); }\n",
        "hello, world\n",
    );
}

#[test]
fn classes_and_methods() {
    prints(
        "class Point {\n  i64 x;\n  i64 y;\n  i64 dist2() { return x * x + y * y; }\n  mut void move(i64 dx) { x += dx; }\n}\nvoid main() {\n  Point p = new Point(3, 4);\n  p.move(1);\n  println(\"${p.x},${p.y} ${p.dist2()}\");\n}\n",
        "4,4 32\n",
    );
    prints(
        "class Rect {\n  f64 w; f64 h;\n  new(f64 w, f64 h) { this.w = w; this.h = h; }\n  new(f64 s) { this.new(s, s); }\n  f64 area() { return w * h; }\n}\nvoid main() { Rect r = new Rect(3.0); Rect q = new Rect(2.0, 5.0); println(\"${r.area()} ${q.area()}\"); }\n",
        "9.0 10.0\n",
    );
}

#[test]
fn numbers() {
    prints(
        &body(
            "u32 c = 3; i32 d = -1; println(\"${d < c} ${(u8%) 300} ${(u8|) -5} ${1 << 40} ${255.hex()} ${7.zeroPad(3)} ${2.5.fixed(3)} ${0.1 + 0.2} ${-7 / 2} ${-7 % 2}\");",
        ),
        "true 44 0 1099511627776 ff 007 2.500 0.30000000000000004 -3 -1\n",
    );
    prints(
        &body(
            "i64 m = i64.MIN; println(\"${m} ${u64.MAX} ${(i64) 3.7} ${(i64) -3.7} ${(f64) 1}\");",
        ),
        "-9223372036854775808 18446744073709551615 3 -3 1.0\n",
    );
    prints(
        &body(
            "u8 b = 250; println(\"${b +% 10} ${b +| 10} ${b -| 255} ${max(3, 9)} ${min(-1, 2)}\");",
        ),
        "4 255 0 9 -1\n",
    );
}

#[test]
fn statuses() {
    let half = "i64 half(i64 n) { if (n % 2 != 0) { return #status.INVALID 0; } return n / 2; }\n";
    prints(
        &format!(
            "{}{}",
            half,
            body(
                "i64 h @err = half(7); println(\"${h} ${err} ${status.message(err)}\"); i64 k @_ = half(8); println(\"${k}\"); u8 s = @half(3); println(\"${s}\");"
            )
        ),
        "0 22 Invalid argument\n4\n22\n",
    );
    prints(
        &body(
            "i64 n @bad = i64.parse(\"12x\"); i64 m @ok = i64.parse(\"-42\"); u8 r = @u8.parse(\"300\"); println(\"${n} ${bad} ${m} ${ok} ${r}\");",
        ),
        "0 22 -42 0 34\n",
    );
}

#[test]
fn strings() {
    prints(
        "(string, string) split2(string s) { for (i64 i = 0; i < s.len; i++) { if (s[i] == ' ') { return (s[:i], s[i + 1:]); } } return (s, \"\"); }\nvoid main() {\n  (string a, string b) = split2(\"hello big world\");\n  println(\"[${a}] [${b}]\");\n  for (string w : \"one two  three\".words()) { print(\"<${w}>\"); }\n  println(\"\");\n  string s = \"  padded  \";\n  println(\"[${s.trim()}] ${s.trim().len} ${s.startsWith(\"  p\")} ${\"abc\" < \"abd\"} ${\"abc\".toUpper()}\");\n  (string k, string v) = \"key=value\".splitFirst(\"=\");\n  println(\"${k}:${v}\");\n}\n",
        "[hello] [big world]\n<one><two><three>\n[padded] 6 true true ABC\nkey:value\n",
    );
    prints(
        &body(
            "u8[..>] out = []; out.append(\"a\"); out +> 'b'; out.append(\"${1 + 2}\"); println(out); u8[.] msg = \"built ${out.len} in place\"; println(msg);",
        ),
        "ab3\nbuilt 3 in place\n",
    );
    prints(
        &body(
            "i64 n = 0; for (u32 c : \"héllo\".codePoints()) { n++; } i64 lines = 0; for (string l : \"a\\nb\\nc\\n\".lines()) { lines++; } println(\"${n} ${lines}\");",
        ),
        "5 3\n",
    );
}

#[test]
fn arrays_and_references() {
    prints(
        "class Node { i64 v; Node&? next; }\nvoid main() {\n  Node[..>] nodes = [];\n  Node& first = nodes.push(new Node(1, null));\n  Node& second = nodes.push(new Node(2, null));\n  first.next .= second;\n  i64 sum = 0;\n  Node&? cur = first;\n  while (cur != null) { sum += cur.v; cur .= cur.next; }\n  println(\"${sum}\");\n}\n",
        "3\n",
    );
    prints(
        &body(
            "u16[..~8] small = []; for (i64 i = 0; i < 10; i++) { if (@(small +> (u16) i)) { println(\"full at ${i}\"); break; } } println(\"${small.len} ${small}\");",
        ),
        "full at 8\n8 [0, 1, 2, 3, 4, 5, 6, 7]\n",
    );
    // A u8 array prints as text (DESIGN §4.13).
    prints(
        &body("u8[..~4] t = []; t +> 'o'; t +> 'k'; println(\"${t}\");"),
        "ok\n",
    );
    prints(
        &body(
            "i64[..~] st = []; st +> 1 +> 2 +> 3; while (i64 x : st ->) { print(\"${x} \"); } println(\"\"); i64[..>] xs = [3, 1, 2]; xs.sort(); xs.reverse(); for (i64& x : xs) { x *= 10; } println(\"${xs} ${xs[^1]} ${xs[%-1]} ${xs[1:]}\");",
        ),
        "3 2 1 \n[30, 20, 10] 10 10 [20, 10]\n",
    );
}

#[test]
fn statements() {
    prints(
        &body(
            "outer: for (i64 i = 0; i < 3; i++) { for (i64 j = 0; j < 3; j++) { if (j == 2) { continue outer; } if (i == 2) { break outer; } print(\"${i}${j} \"); } } println(\"\");",
        ),
        "00 01 10 11 \n",
    );
    prints(
        &body("{ defer println(\"d1\"); defer println(\"d2\"); println(\"in\"); }"),
        "in\nd2\nd1\n",
    );
    prints(
        "enum Color { RED, GREEN, BLUE }\nstring name(Color c) { return switch (c) { case RED => \"red\"; case GREEN => \"green\"; case BLUE => \"blue\"; }; }\nvoid main() { i64 n = 2; string s = switch (n) { case 0 => \"none\"; case 1, 2 => { yield \"few\"; } default => \"many\"; }; switch (\"b\") { case \"a\" => println(\"A\"); case \"b\" => println(\"B\"); default => {} } println(\"${name(Color.BLUE)} ${s}\"); }\n",
        "B\nblue few\n",
    );
    prints(
        "i64 total = 0;\nvoid add(i64 n) { total += n; }\nvoid main() { add(2); add(40); println(\"${total}\"); }\n",
        "42\n",
    );
    prints(
        "i64 fact(i64 n) { return n <= 1 ? 1 : n * fact(n - 1); }\nvoid main() { println(\"${fact(20)}\"); }\n",
        "2432902008176640000\n",
    );
}

#[test]
fn main_gets_arguments_and_gives_the_exit_status() {
    let r = run(
        "u8 main(string[.] args) { for (string a : args) { println(a); } return (u8) args.len; }\n",
        &["one", "two"],
    );
    assert_eq!(r.stdout, "one\ntwo\n");
    assert_eq!(r.code, 2);
}

#[test]
fn bugs_stop_the_program_with_their_location() {
    for (stmt, msg) in [
        ("i64 x = i64.MAX; x = x + 1;", "prog.zn:3: integer overflow"),
        (
            "i64[..>] a = []; a +> 1; i64 y = a[5];",
            "prog.zn:3: index 5 is out of range for length 1",
        ),
        ("i64 z = 0; i64 q = 10 / z;", "prog.zn:3: division by zero"),
        (
            "i64 n = 300; u8 b = (u8) n;",
            "prog.zn:3: the value doesn't fit in u8",
        ),
        ("fail(\"unreachable\");", "prog.zn:3: unreachable"),
        (
            "i64[..>] a = []; string s = \"abc\"; string t = s[2:9];",
            "prog.zn:3: slice 2:9 is out of range for length 3",
        ),
    ] {
        let r = run(
            &format!(
                "void main() {{\n    println(\"before\");\n    {}\n    println(\"after\");\n}}\n",
                stmt
            ),
            &[],
        );
        assert_eq!(r.stdout, "before\n", "{}", stmt);
        assert!(
            r.stderr.contains(msg),
            "{}: expected {:?} in {:?}",
            stmt,
            msg,
            r.stderr
        );
        assert_eq!(r.code, 1, "{}", stmt);
    }
}

#[test]
fn unsupported_features_are_reported_not_miscompiled() {
    let file = SourceFile::new(
        "prog.zn",
        "void main() { hashmap<string, i64> m = []; m.put(\"a\", 1); }\n",
    );
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    assert!(c.is_none());
    assert!(diags[0].iter().any(|d| {
        d.message
            .contains("the C backend doesn't support hash maps yet")
    }));
}

/// Each example either compiles to C that the C compiler accepts, or reports what the
/// backend doesn't support yet; never a crash or C that doesn't compile.
#[test]
fn examples_compile_or_say_what_is_missing() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "zn") {
            continue;
        }
        let file = SourceFile::new(
            path.display().to_string(),
            std::fs::read_to_string(&path).unwrap(),
        );
        let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
        match c {
            Some(c) => {
                let exe = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("example");
                zink::driver::compile(&c, &exe, false)
                    .unwrap_or_else(|e| panic!("{}: {}", path.display(), e));
            }
            None => {
                for d in diags[0].iter().filter(|d| d.severity == Severity::Error) {
                    assert!(
                        d.message.contains("the C backend doesn't support"),
                        "{}: {}",
                        path.display(),
                        d.message
                    );
                }
            }
        }
    }
}

#[test]
fn floats_print_their_shortest_text() {
    prints(
        &body(
            "println(\"${10.0} ${0.1} ${1.5e300} ${1e-7} ${100.0 / 3.0} ${-2.5} ${1.0 / 0.0} ${123456789.0}\");",
        ),
        "10.0 0.1 1.5e300 1e-7 33.333333333333336 -2.5 Infinity 123456789.0\n",
    );
}

/// Cursor arrays, deques and windows (DESIGN §4.1–§4.9): the line editor of
/// examples/linebuf.zn, driven by fixed commands, and the other operations.
#[test]
fn cursor_arrays_and_deques() {
    prints(
        r#"root LineBuffer {
    u32[.~.] text;

    mut void type(string s) { for (u32 c : s.codePoints()) { text +^ c; } }
    mut i64 back(i64 n) { for (i64 i = 0; i < n; i++) { if (@(text -^)) { return i; } } return n; }
    mut i64 del(i64 n) { for (i64 i = 0; i < n; i++) { if (@(text ^-)) { return i; } } return n; }
    mut void left(i64 n) { text.move(-n); }
    mut void right(i64 n) { text.move(n); }
    mut void home() { text.seekStart(); }
    mut void end() { text.seekFromEnd(0); }
    void show() {
        u8[..>] out = [];
        for (i64 i = 0; i <= text.len; i++) {
            if (i == text.cursor) { out +> '|'; }
            if (i < text.len) { @_ = out.append("${text[i].utf8()}"); }
        }
        println(out);
    }
}

void main() {
    LineBuffer buf = new LineBuffer();
    buf.type("hello world");
    buf.show();
    buf.left(5);
    buf.type("big ");
    buf.show();
    i64 gone = buf.back(2);
    buf.show();
    buf.home();
    buf.type(">> ");
    buf.end();
    buf.type("!");
    buf.show();
    buf.left(3);
    i64 deleted = buf.del(10);
    buf.show();
    println("back ${gone}, deleted ${deleted}");
    buf.type("héllo ✓");
    buf.show();

    // A cursor array of i64: seek, insertAt, removeAt, indexing, for-each.
    i64[~] xs = [];
    for (i64 i = 0; i < 5; i++) { xs +> i * 10; }
    @_ = xs.seek(2);
    xs +^ 15;
    xs +< -10;
    i64& first = xs[0];
    @_ = xs.insertAt(3, 99);
    println("first ${first}, after inserts and seeks");
    i64 removed @_ = xs.removeAt(1);
    println("${xs} len ${xs.len} cursor ${xs.cursor} removed ${removed}");
    i64 last @_ = xs.pop();
    i64 front @_ = xs.popFront();
    println("${xs} last ${last} front ${front}");
    xs.reverse();
    println("reversed ${xs}");
    xs.sort();
    println("sorted ${xs}, cursor still ${xs.cursor}");
    i64 total = 0;
    for (i64& x : xs) { x += 1; total += x; }
    println("${xs} total ${total} has 100: ${xs.contains(100)}");
    u8 bad = @xs.seek(100);
    println("seek 100: ${bad}");

    // Deques: both ends, and a window that only narrows.
    i64[~.~] q = [];
    q +> 1 +> 2;
    q +< 0;
    q.pushFront(-1);
    i64 a @_ = q.popFront();
    i64 b @_ = q.pop();
    println("deque ${q} took ${a} and ${b}, as a slice ${q[1:]}");
    for (i64 i = 0; i < 100000; i++) { q +> i; @_ = q.popFront(); }
    println("after a long queue: ${q.len}");
    i64[<.<] w = [1, 2, 3, 4, 5];
    @_ = w -<;
    @_ = w ->;
    println("window ${w}");

    // Holes are reused: removing and inserting doesn't grow the data array.
    i64[.~.] h = [];
    for (i64 r = 0; r < 1000; r++) { h +^ r; @_ = h -^; }
    println("holes reused, len ${h.len}");
}
"#,
        r#"hello world|
hello big |world
hello bi|world
>> hello biworld!|
>> hello biwor|
back 2, deleted 3
>> hello biworhéllo ✓|
first -10, after inserts and seeks
[-10, 10, 99, 15, 20, 30, 40] len 7 cursor 1 removed 0
[10, 99, 15, 20, 30] last 40 front -10
reversed [30, 20, 15, 99, 10]
sorted [10, 15, 20, 30, 99], cursor still 0
[11, 16, 21, 31, 100] total 179 has 100: true
seek 100: 196
deque [0, 1] took -1 and 2, as a slice [1]
after a long queue: 2
window [2, 3, 4]
holes reused, len 0
"#,
    );
}

/// examples/linebuf.zn, unchanged, fed commands on stdin.
#[test]
fn linebuf_example_runs() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/linebuf.zn");
    let file = SourceFile::new(path, std::fs::read_to_string(path).unwrap());
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    let Some(c) = c else {
        panic!(
            "didn't compile: {:?}",
            diags[0]
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        )
    };
    let exe = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("linebuf");
    zink::driver::compile(&c, &exe, false).unwrap();
    use std::io::Write;
    let mut child = Command::new(&exe)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all("type hello world\nshow\nleft 5\ntype big \nshow\nback 2\nhome\nshow\nend\ndel 1\nfrobnicate\ntype  ✓\nshow\n".as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hello world|\nhello big |world\n|hello biworld\nhello biworld ✓|\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "unknown command: frobnicate\n"
    );
    assert!(out.status.success());
}
