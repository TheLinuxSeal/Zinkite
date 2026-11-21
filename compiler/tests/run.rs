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
    exec(&build(text), args)
}

/// Compiles a program to an executable, failing the test if it doesn't compile.
fn build(text: &str) -> std::path::PathBuf {
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
    let name = format!("prog-{:x}", {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    });
    let exe = dir.join(name);
    zink::driver::compile(&c, &exe, false).unwrap_or_else(|e| panic!("{}", e));
    exe
}

fn exec(exe: &std::path::Path, args: &[&str]) -> Run {
    let out = Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("ran");
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
            "i128 m = i128.MAX; m = m + 1;",
            "prog.zn:3: integer overflow",
        ),
        (
            "u128 a = u128.MAX; u128 b = a * 2;",
            "prog.zn:3: integer overflow",
        ),
        (
            "i128 big = i128.MAX; u64 t = (u64) big;",
            "prog.zn:3: the value doesn't fit in u64",
        ),
        (
            "string s = \"abcdef\"; u8[~4] g = s;",
            "prog.zn:3: the value doesn't fit in an array that holds 4",
        ),
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
        "i64 apply(func(i64 => i64) f, i64 x) { return f(x); }\nvoid main() { i64 k = 3; println(\"${apply((i64 x) => x * k, 2)}\"); }\n",
    );
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    assert!(c.is_none());
    assert!(diags[0].iter().any(|d| {
        d.message.contains(
            "the C backend doesn't support a lambda that captures, passed to a function of your own yet",
        )
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

/// The same operations on cursor arrays with a chunked index (`$`, DESIGN §4.1).
#[test]
fn chunked_cursor_arrays() {
    prints(
        r#"root LineBuffer {
    u32[.~.$] text;

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
    i64[~$] xs = [];
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
    i64[.~.$] h = [];
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

/// Random edits at random positions against a plain array: enough for the chunk size
/// to double twice.
#[test]
fn chunked_index_against_a_model() {
    prints(
        r#"u64 seed = 12345;

i64 below(i64 n) {
    seed = seed *% 6364136223846793005 +% 1442695040888963407;
    return (i64) ((seed >> 33) % (u64) n);
}

void main() {
    i64[.~.$] chunked = [];
    i64[..~] model = [];
    i64 next = 0;
    i64 checks = 0;
    for (i64 step = 0; step < 16000; step++) {
        if (below(4) < 3 || model.len == 0) {
            i64 i = below(model.len + 1);
            @_ = chunked.insertAt(i, next);
            model +> 0;
            for (i64 k = model.len - 1; k > i; k--) { model[k] = model[k - 1]; }
            model[i] = next;
            next++;
        } else {
            i64 i = below(model.len);
            i64 got @_ = chunked.removeAt(i);
            if (got != model[i]) { println("wrong element at step ${step}"); exit(1); }
            for (i64 k = i; k + 1 < model.len; k++) { model[k] = model[k + 1]; }
            @_ = model ->;
        }
        if (step % 1000 == 0) {
            if (chunked.len != model.len) { println("length ${chunked.len} against ${model.len}"); exit(1); }
            for (i64 k = 0; k < model.len; k++) {
                if (chunked[k] != model[k]) { println("mismatch at ${k}, step ${step}"); exit(1); }
            }
            checks++;
        }
    }
    i64 sum = 0;
    for (i64 x : chunked) { sum += x; }
    i64 msum = 0;
    for (i64 x : model) { msum += x; }
    println("ok: ${chunked.len} elements after ${next} inserts, ${checks} checks, sums agree: ${sum == msum}");
}
"#,
        "ok: 8008 elements after 12004 inserts, 16 checks, sums agree: true
",
    );
}

/// Capped arrays with a start slot are inline ring buffers, and with a cursor inline gap
/// buffers (DESIGN §4.3): wrapping, FULL, sorting and truncating around the cursor, and
/// both as fields held inside an object.
#[test]
fn capped_ring_and_gap_buffers() {
    prints(
        r#"class Recent {
    i64[~.~4] last = [];
    u8[~8] word = [];


    mut void see(i64 x) {
        if (this.last.len == 4) {
            @_ = this.last -<;
        }
        @_ = this.last +> x;
    }
}

void main() {
    // A ring buffer: pushes and pops at both ends wrap around inside its 4 slots.
    i64[~.~4] q = [];
    @_ = q +> 1;
    @_ = q +> 2;
    @_ = q +< 0;
    i64 a @_ = q -<;
    @_ = q +> 3;
    @_ = q +> 4;
    i64& r @st = q.push(5);
    println("ring ${q} len ${q.len} took ${a}, full: ${st == status.FULL}, last ${r}");
    i64 b @_ = q -<;
    @_ = q +> 6;
    @_ = q +< -1;
    println("ring ${q}, q[0] ${q[0]} q[3] ${q[3]} took ${b}");
    q.sort();
    println("sorted ${q}, then [0]=${q[0]}");
    q.reverse();
    q.swap(0, 3);
    println("reversed and swapped ${q}, sum ${q.sum()} has 6: ${q.contains(6)} index of 3: ${q.indexOf(3)}");
    for (i64 x : q) { print("${x * 10} "); }
    println("");
    q.truncate(2);
    i64 e1 @_ = q ->;
    i64 e2 @_ = q ->;
    i64 e3 @s3 = q ->;
    println("drained ${e1} ${e2}, then ${e3} ${s3 == status.EMPTY}, len ${q.len}");

    // A gap buffer: inserts at the cursor move the elements after it to the far end.
    u8[~8] g = [];
    @_ = g +^ 'a';
    @_ = g +^ 'c';
    g.move(-1);
    @_ = g +^ 'b';
    @_ = g +> 'z';
    @_ = g +< '<';
    println("gap ${g} len ${g.len} cursor ${g.cursor}");
    @_ = g.seek(1);
    u8 d @_ = g -^;
    u8 f @_ = g ^-;
    println("removed '${d}' and '${f}': ${g} cursor ${g.cursor}");
    @_ = g.insertAt(2, 'Q');
    u8 w @_ = g.removeAt(0);
    println("insertAt and removeAt: ${g}, removed '${w}', cursor ${g.cursor}");
    for (i64 i = 0; i < 5; i++) { @_ = g +^ 'x'; }
    u8& last @full = g.insert('y');
    println("${g} full: ${full == status.FULL}, len ${g.len}, ref to '${last}'");
    g.move(-3);
    g.sort();
    println("sorted ${g}, cursor ${g.cursor}");
    g.truncate(3);
    println("truncated ${g} len ${g.len} cursor ${g.cursor}");
    g[1] = 'M';
    g.seekStart();
    @_ = g +^ '_';
    println("${g} g[0] '${g[0]}' g[g.len - 1] '${g[g.len - 1]}'");
    @_ = g.seekFromEnd(0);
    g.clear();
    println("cleared: len ${g.len} cursor ${g.cursor}");

    // Inline in a class: the object holds both buffers directly.
    Recent rc = new Recent();
    for (i64 i = 1; i <= 7; i++) { rc.see(i * i); }
    @_ = rc.word +> 'h';
    @_ = rc.word +> 'i';
    println("recent ${rc.last} word ${rc.word}");
}
"#,
        r#"ring [1, 2, 3, 4] len 4 took 0, full: true, last 4
ring [2, 3, 4, 6], q[0] 2 q[3] 6 took 1
sorted [2, 3, 4, 6], then [0]=2
reversed and swapped [2, 4, 3, 6], sum 15 has 6: true index of 3: 2
20 40 30 60 
drained 4 2, then 0 true, len 0
gap <abcz len 5 cursor 3
removed '60' and '97': bcz cursor 0
insertAt and removeAt: cQz, removed '98', cursor 0
xxxxxcQz full: true, len 8, ref to '122'
sorted Qcxxxxxz, cursor 2
truncated Qcx len 3 cursor 2
_QMx g[0] '95' g[g.len - 1] '120'
cleared: len 0 cursor 0
recent [16, 25, 36, 49] word hi
"#,
    );
}

/// Literals and text fill capped arrays; copies are values; references reach callees.
#[test]
fn capped_literals_copies_and_parameters() {
    prints(
        r#"void rotate(i64[~.~4]& r) {
    i64 x @_ = r -<;
    @_ = r +> x;
}

i64 total(i64[~8]& g) {
    i64 t = 0;
    for (i64 x : g) { t += x; }
    return t;
}

void main() {
    i64[..~4] plain = [7, 8];
    i64[~.~4] r = [1, 2, 3];
    u8[~8] g = "abc";
    println("${plain} ${r} ${g} cursor ${g.cursor}");
    rotate(r);
    rotate(r);
    i64[~.~4] copy = r;
    @_ = copy +> 9;
    println("rotated ${r}, a copy ${copy}");
    i64[~8] h = [5, 6, 7];
    @_ = h.seek(1);
    @_ = h +^ 100;
    for (i64& x : h) { x *= 2; }
    println("${h} total ${total(h)} min ${h.min()} max ${h.max()} getOr ${h.getOr(9, -1)}");
    h.sortDescending();
    println("${h} cursor ${h.cursor}");
    h.fill(3);
    println("${h}");
}
"#,
        r#"[7, 8] [1, 2, 3] abc cursor 0
rotated [3, 1, 2], a copy [3, 1, 2, 9]
[10, 200, 12, 14] total 236 min 10 max 200 getOr -1
[200, 14, 12, 10] cursor 2
[3, 3, 3, 3]
"#,
    );
}

/// Random inserts, removals, seeks and moves on a gap buffer and a ring buffer, checked
/// against plain arrays and a cursor position kept by hand.
#[test]
fn capped_buffers_against_a_model() {
    prints(
        r#"u64 seed = 99;

i64 below(i64 n) {
    seed = seed *% 6364136223846793005 +% 1442695040888963407;
    return (i64) ((seed >> 33) % (u64) n);
}

void put(i64[..~]& m, i64 i, i64 x) {
    @_ = m +> 0;
    for (i64 k = m.len - 1; k > i; k--) { m[k] = m[k - 1]; }
    m[i] = x;
}

i64 cut(i64[..~]& m, i64 i) {
    i64 x = m[i];
    for (i64 k = i; k + 1 < m.len; k++) { m[k] = m[k + 1]; }
    @_ = m ->;
    return x;
}

void main() {
    i64[~16] g = [];
    i64[~.~16] r = [];
    i64[..~] gm = [];
    i64[..~] rm = [];
    i64 cur = 0;
    i64 next = 0;
    i64 bad = 0;
    i64 fulls = 0;
    i64 empties = 0;
    for (i64 step = 0; step < 20000; step++) {
        i64 op = below(11);
        i64 x = next;
        next++;
        u8 st = 0;
        if (op == 0) {
            @st = g +^ x;
            if (st == 0) { put(gm, cur, x); cur++; }
        } else if (op == 1) {
            @st = g +> x;
            if (st == 0) { put(gm, gm.len, x); }
        } else if (op == 2) {
            @st = g +< x;
            if (st == 0) { put(gm, 0, x); cur++; }
        } else if (op == 3) {
            i64 i = below(gm.len + 1);
            @st = g.insertAt(i, x);
            cur = i;
            if (st == 0) { put(gm, i, x); cur++; }
        } else if (op == 4) {
            i64 v @s = g -^;
            st = s;
            if (s == 0) { if (v != cut(gm, cur - 1)) { bad++; } cur--; }
        } else if (op == 5) {
            i64 v @s = g ^-;
            st = s;
            if (s == 0 && v != cut(gm, cur)) { bad++; }
        } else if (op == 6) {
            i64 v @s = g ->;
            st = s;
            if (s == 0) { if (cur == gm.len) { cur--; } if (v != cut(gm, gm.len - 1)) { bad++; } }
        } else if (op == 7) {
            i64 v @s = g -<;
            st = s;
            if (s == 0) { if (cur > 0) { cur--; } if (v != cut(gm, 0)) { bad++; } }
        } else if (op == 8 && gm.len > 0) {
            i64 i = below(gm.len);
            i64 v @s = g.removeAt(i);
            cur = i;
            if (v != cut(gm, i)) { bad++; }
        } else if (op == 9) {
            i64 i = below(gm.len + 1);
            @_ = g.seek(i);
            cur = i;
        } else {
            i64 d = below(9) - 4;
            g.move(d);
            cur = clamp(cur + d, 0, gm.len);
        }
        if (st == status.FULL) { fulls++; }
        if (st == status.EMPTY) { empties++; }
        if (g.len != gm.len || g.cursor != cur) { bad++; }
        for (i64 k = 0; k < gm.len; k++) { if (g[k] != gm[k]) { bad++; } }

        i64 rop = below(5);
        if (rop == 0) {
            @st = r +> x;
            if (st == 0) { put(rm, rm.len, x); }
        } else if (rop == 1) {
            @st = r +< x;
            if (st == 0) { put(rm, 0, x); }
        } else if (rop == 2) {
            i64 v @s = r ->;
            if (s == 0 && v != cut(rm, rm.len - 1)) { bad++; }
        } else if (rop == 3) {
            i64 v @s = r -<;
            if (s == 0 && v != cut(rm, 0)) { bad++; }
        } else if (rm.len > 0) {
            i64 i = below(rm.len);
            r[i] = x;
            rm[i] = x;
        }
        if (r.len != rm.len) { bad++; }
        i64 k = 0;
        for (i64 v : r) { if (v != rm[k]) { bad++; } k++; }
    }
    println("mismatches ${bad}, FULL ${fulls > 0}, EMPTY ${empties > 0}, lengths ${g.len} ${r.len}");
}
"#,
        "mismatches 0, FULL true, EMPTY true, lengths 9 5\n",
    );
}

/// Slices of capped ring and gap buffers (DESIGN §4.10): the first slice of a split buffer
/// joins all of it, so slices held at once agree; the order and the cursor don't change;
/// writable views, text methods and temporaries work.
#[test]
fn slices_of_ring_and_gap_buffers() {
    prints(
        r#"i64 total(i64[:] s) {
    i64 t = 0;
    for (i64 x : s) { t += x; }
    return t;
}

void upcase(u8[:]& s) {
    for (u8& b : s) { b = b.toUpper(); }
}

u8[~.~8] letters() {
    u8[~.~8] r = "wxyz";
    @_ = r -<;
    @_ = r +> 'a';
    return r;
}

void main() {
    // A ring that wraps: 3 and 4 sit at the end of the buffer, 5 and 6 at the start.
    i64[~.~4] q = [1, 2, 3, 4];
    @_ = q -<;
    @_ = q -<;
    @_ = q +> 5;
    @_ = q +> 6;
    i64[:] head = q[0:2];
    i64[:] tail = q[2:];
    println("head ${head} tail ${tail}, total ${total(q)}");
    i64[:] mid = q[1:^1];
    println("across the wrap ${mid}, ring still ${q}, first ${q[0]}");
    @_ = q -<;
    @_ = q +> 7;
    println("${q} as a slice ${q[:]} sum ${total(q[1:])}");

    // A gap buffer: slices that the gap splits move it, never the cursor.
    u8[~16] g = "hello world";
    @_ = g.seek(5);
    @_ = g +^ ',';
    println("${g} cursor ${g.cursor}, before ${g[0:3]} after ${g[7:]}");
    string all = g;
    println("whole '${all}' cursor ${g.cursor}, split '${g[3:9]}' cursor ${g.cursor}");
    @_ = g +^ '!';
    @_ = g -^;
    @_ = g +^ '?';
    println("edits at the cursor still go there: ${g} cursor ${g.cursor}");
    upcase(g[0:5]);
    println("through a writable view ${g}");
    @_ = g.seek(2);
    println("text methods: starts ${g.startsWith("HE")} find ${g.find("wor")} trim '${g.trim()}' equal ${g == "HELLO,? world"}");
    upcase(g);
    println("${g} cursor ${g.cursor}");

    // Slices held while others are taken: the first one joined the buffer.
    u8[~8] h = "abcdef";
    @_ = h.seek(3);
    @_ = h +^ 'X';
    string p1 = h[0:2];
    string p2 = h[5:];
    string p3 = h[2:6];
    println("${p1} ${p2} ${p3} ${h} cursor ${h.cursor}");

    // A temporary lasts to the end of its statement.
    println("from a call: ${letters()} sliced ${letters()[1:]} len ${letters().len}");
    u8[~.~8] l = letters();
    string ls = l;
    println("${ls} ${ls.len}");
}
"#,
        r#"head [3, 4] tail [5, 6], total 18
across the wrap [4, 5], ring still [3, 4, 5, 6], first 3
[4, 5, 6, 7] as a slice [4, 5, 6, 7] sum 18
hello, world cursor 6, before hel after world
whole 'hello, world' cursor 6, split 'lo, wo' cursor 6
edits at the cursor still go there: hello,? world cursor 7
through a writable view HELLO,? world
text methods: starts true find 8 trim 'HELLO,? world' equal true
HELLO,? WORLD cursor 2
ab ef cXde abcXdef cursor 4
from a call: xyza sliced yza len 4
xyza 4
"#,
    );
}

/// A reference follows its element: after a front insert it is the next index along and
/// still reads the same value (REFERENCES §5.4). A capped array that changes only at the
/// start sits at the top of its buffer, so it never wraps and nothing in it moves.
#[test]
fn references_follow_their_element() {
    prints(
        r#"i64 total(i32[:] s) {
    i64 t = 0;
    for (i32 v : s) { t += v; }
    return t;
}

void main() {
    // On a data stack: x is the element, which a front insert leaves in place.
    i32[>..] myarr = [1, 2, 3];
    i32& x = myarr[0];
    myarr +< 0;
    println("x ${x}, myarr[1] ${myarr[1]}, myarr[0] ${myarr[0]}, ${myarr}");

    // Capped, changing only at the start: it sits at the top of its buffer, so front
    // inserts never wrap it and a slice never has to move anything.
    i32[>..4] f = [2, 3];
    i32& y = f[0];
    i32[:] before = f;
    @_ = f +< 1;
    @_ = f +< 0;
    i32& last @full = f.pushFront(-1);
    println("y ${y}, f[2] ${f[2]}, ${f} full ${full == status.FULL}, last ${last}, all ${f[:]} total ${total(f)}");
    println("a slice from before the inserts still shows ${before}");

    i32[~..4] w = [5, 6];
    @_ = w +< 4;
    i32 gone @_ = w -<;
    i32& z2 = w[^1];
    println("z ${z2}, front was ${gone}, ${w}");
}
"#,
        r#"x 1, myarr[1] 1, myarr[0] 0, [0, 1, 2, 3]
y 2, f[2] 2, [0, 1, 2, 3] full true, last 3, all [0, 1, 2, 3] total 6
a slice from before the inserts still shows [2, 3]
z 6, front was 4, [5, 6]
"#,
    );
}

/// `i128` and `u128` (DESIGN §9.1): literals of every size, limits, the full product of
/// two `u64`s, mixed-sign comparisons, casts in every mode, `from`, text and parsing,
/// shifts, wrapping and saturating operators, arrays, fields and `switch`.
#[test]
fn wide_integers() {
    prints(
        r#"class Account {
    i128 cents;
    new(i128 c) { this.cents = c; }
}

u128 mulWide(u64 a, u64 b) {
    return (u128) a * b;
}

void main() {
    // Limits and literals of every size.
    println("${i128.MIN} ${i128.MAX}");
    println("${u128.MAX}");
    i128 neg = -170141183460469231731687303715884105728;
    u128 mask = 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff;
    i128 big = 18446744073709551616;
    u128 small = 42u128;
    println("${neg == i128.MIN} ${mask == u128.MAX} ${big} ${small}");

    // The full product of two u64s.
    u128 p = mulWide(u64.MAX, u64.MAX);
    println("${p} hi ${(u64) (p >> 64)} lo ${(u64%) p}");

    // Arithmetic, widening and mixed-sign comparison.
    i64 x = -5;
    u64 y = 7;
    i128 s = x + (i128) y;
    i128 q = big / 3;
    i128 r = big % 3;
    println("${s} ${q} ${r} ${-big} ${abs(neg + 1)} ${min(big, s)} ${max(big, s)}");
    u128 u = 5;
    println("${neg < u} ${x < u} ${mask > big} ${(i128) 10 == u}");

    // Casts in every mode.
    println("${(u64%) mask} ${(u64|) mask} ${(i8|) neg} ${(u128|) neg} ${(i128) 1.5e30} ${(f64) mask}");
    u64 got @bad = u64.from(big);
    println("checked: ${(i64) (big - 18446744073709551616)} from: ${got} ${bad}");

    // Text.
    println("${big.pad(25)}|${s.zeroPad(6)}|${mask.hex()}|${((u128) 5).bin()}|${((u128) 64).oct()}");
    u8[..~32] h = mask.hex();
    println("${h.len} ${((u128) 255).popCount()} ${mask.popCount()}");
    i128 parsed @e1 = i128.parse("-170141183460469231731687303715884105728");
    u128 parsed2 @e2 = u128.parse("340282366920938463463374607431768211455");
    u128 over @e3 = u128.parse("340282366920938463463374607431768211456");
    i128 hexed @e4 = i128.parse("-ff", 16);
    println("${parsed == i128.MIN} ${e1} ${parsed2 == u128.MAX} ${e2} ${over} ${e3} ${hexed} ${e4}");

    // Shifts, wrapping and saturating operators.
    u128 one = 1;
    println("${one << 127} ${(one << 127) >> 120} ${i128.MAX +% 1 == i128.MIN} ${u128.MAX +| 1 == u128.MAX} ${neg >> 126}");

    // Arrays, fields and switch.
    i128[..~] xs = [big, -big, 3, neg + 1];
    xs.sort();
    println("${xs} sum ${xs[1] + xs[2] + xs[3]}");
    Account acct = new Account(big * 100);
    acct.cents += 1;
    println("${acct.cents}");
    switch (s) {
        case 2 => println("two");
        default => println("other");
    }
}
"#,
        r#"-170141183460469231731687303715884105728 170141183460469231731687303715884105727
340282366920938463463374607431768211455
true true 18446744073709551616 42
340282366920938463426481119284349108225 hi 18446744073709551614 lo 1
2 6148914691236517205 1 -18446744073709551616 170141183460469231731687303715884105727 2 18446744073709551616
true true true false
18446744073709551615 18446744073709551615 -128 0 1499999999999999889089448902656 3.402823669209385e38
checked: 0 from: 0 34
     18446744073709551616|000002|ffffffffffffffffffffffffffffffff|101|100
32 8 128
true 0 true 0 0 34 -255 0
170141183460469231731687303715884105728 128 true true -2
[-170141183460469231731687303715884105727, -18446744073709551616, 3, 18446744073709551616] sum 3
1844674407370955161601
two
"#,
    );
}

/// Lambdas (DESIGN §9.3): comparators, keys and predicates inlined into the array
/// methods, a block body with `return`s, a captured local, and function values passed to
/// a function. Maps and sets (DESIGN §4.14): every method, for-each in insertion order and
/// writing through it, a `getOrAdd` reference that survives 100000 inserts, compaction
/// after removals keeping the order, and a map in a class field.
#[test]
fn lambdas_maps_and_sets() {
    prints(
        r#"class Row {
    string key;
    i64 num;
}

class Index {
    hashmap<string, i64> seen = [];

    mut i64 note(string w) {
        i64& c = this.seen.getOrAdd(w, 0);
        c++;
        return c;
    }
}

i64 twice(func(i64 => i64) f, i64 x) {
    return f(f(x));
}

i64 square(i64 x) {
    return x * x;
}

void main() {
    // Comparators, keys and predicates, inlined; one captures a local.
    Row[..~] rows = [];
    rows +> new Row("pear", 3) +> new Row("apple", 7) +> new Row("fig", 3) +> new Row("kiwi", 1) +> new Row("date", 7);
    rows.stableSort((Row a, Row b) => a.num < b.num);
    for (Row& r : rows) { print("${r.key}:${r.num} "); }
    println("");
    rows.sortBy((Row r) => r.key);
    for (Row& r : rows) { print("${r.key} "); }
    println("");
    rows.sortByDescending((Row r) => r.num);
    for (Row& r : rows) { print("${r.num} "); }
    println("sorted ${rows.isSortedBy((Row r) => -r.num)}");
    i64 limit = 3;
    println("any ${rows.any((Row r) => r.num > limit)} all ${rows.all((Row r) => r.num >= 1)} count ${rows.count((Row r) => r.num == limit)}");
    i64 at @missing = rows.findIndex((Row r) => r.key == "fig");
    i64 none @gone = rows.findIndex((Row r) => r.key == "plum");
    println("fig at ${at} ${missing}, plum ${none} ${gone == status.MISSING}");
    i64 total = rows.fold(0, (i64 acc, Row r) => acc + r.num);
    println("total ${total}");

    i64[..~] xs = [5, 3, 9, 1, 7, 3, 3, 8];
    xs.partialSort(3, (i64 a, i64 b) => a > b);
    println("top three ${xs[0]} ${xs[1]} ${xs[2]}");
    xs.sort((i64 a, i64 b) => {
        if (a % 2 != b % 2) {
            return a % 2 == 0;
        }
        return a < b;
    });
    println("evens first ${xs}");
    xs.dedup();
    println("dedup ${xs}");
    xs.retain((i64 x) => x != 9 && x > limit - 2);
    println("retain ${xs}");
    string[..~] words = ["b", "a", "c", "a"];
    words.sort();
    words.dedup();
    println("words ${words.len} ${words[0]}${words[1]}${words[2]}");

    // Function values: a lambda in a variable, a function by name, passed as a parameter.
    func(i64 => i64) inc = (i64 x) => x + 1;
    println("inc ${inc(41)} twice ${twice(inc, 5)} ${twice((i64 x) => x * 3, 2)}");

    // Maps.
    hashmap<string, i64> m = [];
    m.put("one", 1);
    m.put("two", 2);
    m.put("three", 3);
    m.put("two", 22);
    i64 two @_ = m.get("two");
    i64 four @nf = m.get("four");
    println("len ${m.len} two ${two} four ${four} ${nf == status.MISSING} has ${m.has("three")} ${m.has("ten")} or ${m.getOr("ten", -1)}");
    for (var& e : m) { e.value *= 10; }
    for (var e : m) { print("${e.key}=${e.value} "); }
    println("");
    u8 st1 = @m.remove("one");
    u8 st2 = @m.remove("one");
    println("removed ${st1} again ${st2 == status.MISSING} len ${m.len}");

    // A reference from getOrAdd survives inserts; entries never move.
    hashmap<i64, i64> big = [];
    i64& first = big.getOrAdd(-1, 100);
    for (i64 i = 0; i < 100000; i++) { big.put(i, i * 2); }
    first += 5;
    i64 back @_ = big.get(-1);
    i64 mid @_ = big.get(50000);
    println("big ${big.len} first ${back} mid ${mid}");
    // Removing more than half compacts the entries, keeping insertion order.
    for (i64 i = 0; i < 99990; i++) { @_ = big.remove(i); }
    for (var e : big) { print("${e.key} "); }
    println("len ${big.len}");
    big.clear();
    println("cleared ${big.len} ${big.has(99995)}");

    hashset<string> seen = [];
    i64 fresh = 0;
    for (string w : "a b a c b a".splitBy(" ")) { if (seen.add(w)) { fresh++; } }
    for (string w : seen) { print(w); }
    println(" fresh ${fresh} len ${seen.len}");

    Index ix = new Index();
    i64 n1 = ix.note("x");
    i64 n2 = ix.note("x");
    i64 n3 = ix.note("y");
    println("index ${n1} ${n2} ${n3} ${ix.seen.len}");
}
"#,
        r#"kiwi:1 pear:3 fig:3 apple:7 date:7 
apple date fig kiwi pear 
7 7 3 3 1 sorted true
any true all true count 2
fig at 2 0, plum 0 true
total 21
top three 9 8 7
evens first [8, 1, 3, 3, 3, 5, 7, 9]
dedup [8, 1, 3, 5, 7, 9]
retain [8, 3, 5, 7]
words 3 abc
inc 42 twice 7 18
len 3 two 22 four 0 true has true false or -1
one=10 two=220 three=30 
removed 0 again true len 2
big 100001 first 105 mid 100000
-1 99990 99991 99992 99993 99994 99995 99996 99997 99998 99999 len 11
cleared 0 false
abc fresh 3 len 3
index 1 2 1 2
"#,
    );
}

/// Function values in a local, a class field, an array and a nullable field.
#[test]
fn function_values() {
    prints(
        r#"i64 square(i64 x) { return x * x; }
class Op { func(i64 => i64) f; func(i64 => i64)? g = null; }
void main() {
  func(i64 => i64) f = (i64 x) => square(x);
  Op o = new Op((i64 x) => x - 1);
  func(i64 => i64)[..~] fs = [];
  fs +> f;
  fs +> o.f;
  i64 v = 3;
  for (func(i64 => i64) h : fs) { v = h(v); }
  println("${f(4)} ${o.f(10)} ${v} ${o.g == null}");
}
"#,
        r#"16 9 8 true
"#,
    );
}

/// Random puts, removals, `getOrAdd`s and gets against a plain-array model.
#[test]
fn maps_against_a_model() {
    prints(
        r#"u64 seed = 5;

i64 below(i64 n) {
    seed = seed *% 6364136223846793005 +% 1442695040888963407;
    return (i64) ((seed >> 33) % (u64) n);
}

void main() {
    hashmap<i64, i64> m = [];
    // The model: present[k] and value[k] for keys 0..999.
    bool[..>] present = [];
    i64[..>] value = [];
    for (i64 k = 0; k < 1000; k++) { present +> false; value +> 0; }
    i64 bad = 0;
    for (i64 step = 0; step < 200000; step++) {
        i64 k = below(1000);
        i64 op = below(4);
        if (op == 0) {
            m.put(k, step);
            present[k] = true;
            value[k] = step;
        } else if (op == 1) {
            u8 st = @m.remove(k);
            if ((st == 0) != present[k]) { bad++; }
            present[k] = false;
        } else if (op == 2) {
            i64& r = m.getOrAdd(k, -step);
            if (!present[k]) { present[k] = true; value[k] = -step; }
            if (r != value[k]) { bad++; }
            r += 1;
            value[k] += 1;
        } else {
            i64 v @st = m.get(k);
            if ((st == 0) != present[k] || (st == 0 && v != value[k])) { bad++; }
        }
    }
    i64 n = 0;
    for (var e : m) {
        n++;
        if (!present[e.key] || value[e.key] != e.value) { bad++; }
    }
    i64 live = 0;
    for (bool p : present) { if (p) { live++; } }
    println("mismatches ${bad}, entries ${n} of ${live}, len ${m.len}");
}
"#,
        "mismatches 0, entries 656 of 656, len 656\n",
    );
}

/// `examples/wordfreq.zn` on stdin: a hashmap of views into the text, a for-each over it
/// and `partialSort` with a lambda. Checked against a Python version of the same rules.
#[test]
fn wordfreq_example_runs() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/wordfreq.zn");
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
    let exe = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("wordfreq");
    zink::driver::compile(&c, &exe, false).unwrap();
    use std::io::Write;
    let mut child = Command::new(&exe)
        .arg("4")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            r#"The cat and the dog. The END, the end; Ünïcode wörds stay whole: wörds!
A cat
"#
            .as_bytes(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        r#"      4 the
      2 cat
      2 end
      2 wörds
"#
    );
    assert_eq!(out.status.code(), Some(0));
}

/// Generics, monomorphized (DESIGN §10): DESIGN's `sum`, `clamp` and `countDistinct` at
/// several types, a generic function calling another, a cast to `T`, generic classes
/// with a generic method, a recursive `Node<K>`, nested `Box<Box<i64>>` (copied by
/// value), an inlined lambda and a lambda value inside generic functions.
#[test]
fn generic_functions_and_classes() {
    prints(
        r#"<T implements number> T sum(T[:] xs) {
    T total = 0;
    for (T x : xs) { total += x; }
    return total;
}

<T implements ordered> T clamp(T x, T lo, T hi) { return x < lo ? lo : (hi < x ? hi : x); }

<K implements hashable> i64 countDistinct(K[:] xs) {
    hashset<K> seen = [];
    for (K x : xs) { seen.add(x); }
    return seen.len;
}

<T implements number> f64 mean(T[:] xs) {
    return (f64) sum(xs) / (f64) xs.len;
}

<T implements ordered> T largest(T[:] xs) {
    T best = xs[0];
    for (T x : xs) { if (best < x) { best = x; } }
    return best;
}

<T implements int> T scaled(T x, i64 k) {
    return x * (T) k;
}

<T implements number> T applyTwice(T x) {
    func(T => T) f = (T y) => y + y;
    return f(f(x));
}

class Pair<A, B> {
    A first;
    B second;

    <C> Pair<A, C> withSecond(C c) {
        return new Pair<A, C>(first, c);
    }

    void show() {
        print("(${first}, ${second}) ");
    }
}

class Node<K> {
    K key;
    Node<K>&? next;
}

class Box<T> {
    T value;

    T get() { return value; }

    mut void set(T v) { value = v; }
}

<T implements ordered> void sortDesc(T[..~]& xs) {
    xs.sort((T a, T b) => b < a);
}

void main() {
    i64[..>] ints = [3, 1, 4, 1, 5, 9, 2, 6];
    f64[..>] floats = [0.5, 0.25, 2.0];
    u8[..>] bytes = [20, 10];
    string[..>] words = ["b", "a", "b", "c", "a"];
    println("sums ${sum(ints)} ${sum(floats)} ${sum(bytes)}");
    println("clamp ${clamp(15, 0, 10)} ${clamp(-2.5, -1.0, 1.0)} ${clamp("m", "a", "k")}");
    println("distinct ${countDistinct(ints)} ${countDistinct(words)}");
    println("mean ${mean(ints)} largest ${largest(ints)} ${largest(words)}");
    println("scaled ${scaled(7, 3)} ${scaled((u8) 20, 2)}");

    Pair<i64, string> p = new Pair<i64, string>(1, "one");
    Pair<i64, f64> q = p.withSecond(2.5);
    p.show();
    q.show();
    println("first+1 ${q.first + 1}");

    Node<string>[..>] nodes = [];
    Node<string>& c = nodes.push(new Node<string>("c", null));
    Node<string>& b = nodes.push(new Node<string>("b", c));
    Node<string>& a = nodes.push(new Node<string>("a", b));
    Node<string>&? cur = a;
    while (cur != null) { print(cur.key); cur .= cur.next; }
    println("");

    Box<Box<i64>> nested = new Box<Box<i64>>(new Box<i64>(41));
    Box<i64> inner = nested.get();
    inner.set(inner.get() + 1);
    println("nested ${nested.get().get()} copy ${inner.get()}");
    Box<string> bs = new Box<string>("hi");
    bs.set("there");
    println("box ${bs.get()}");

    i64[..~] ys = [3, 8, 1];
    sortDesc(ys);
    string[..~] zs = ["x", "z", "y"];
    sortDesc(zs);
    println("desc ${ys} ${zs[0]}${zs[1]}${zs[2]}");
    println("twice ${applyTwice(3)} ${applyTwice(1.25)}");
}
"#,
        r#"sums 31 2.75 30
clamp 10 -1.0 k
distinct 7 3
mean 3.875 largest 9 c
scaled 21 40
(1, one) (1, 2.5) first+1 2
abc
nested 41 copy 42
box there
desc [8, 3, 1] zyx
twice 12 5.0
"#,
    );
}

/// In an instantiation, a type parameter bounded by an interface is a known class, so
/// its method is called directly; `T.MIN`, `T.MAX` and `T.parse` follow `T`; a generic
/// root keeps its growable field.
#[test]
fn generic_bounds_and_roots() {
    prints(
        r#"interface Shape {
    f64 area();
}

class Square implements Shape {
    f64 side;
    f64 area() { return side * side; }
}

class Circle implements Shape {
    f64 r;
    f64 area() { return 3.0 * r * r; }
}

<S implements Shape> f64 total(S[:] shapes) {
    f64 t = 0.0;
    for (S s : shapes) { t += s.area(); }
    return t;
}

<T implements int> string limits(T x) {
    return x == T.MIN ? "min" : x == T.MAX ? "max" : "neither";
}

<T implements number> T parsed(string s, T fallback) {
    T v @bad = T.parse(s);
    return bad ? fallback : v;
}

root Stack<T> {
    T[..~] items;

    new() {}

    mut void push(T x) { items +> x; }

    mut T pop() {
        T x @_ = items ->;
        return x;
    }

    i64 size() { return items.len; }
}

void main() {
    Square[..>] sq = [];
    sq +> new Square(2.0) +> new Square(3.0);
    Circle[..>] ci = [];
    ci +> new Circle(1.0);
    println("areas ${total(sq)} ${total(ci)}");
    println("limits ${limits((u8) 255)} ${limits(i64.MIN)} ${limits(5)}");
    println("parsed ${parsed("12", 0)} ${parsed("x", -1)} ${parsed("2.5", 0.0)} ${parsed("300", (u8) 7)}");
    Stack<string> st = new Stack<string>();
    st.push("a");
    st.push("b");
    Stack<i128> wide = new Stack<i128>();
    wide.push(i128.MAX);
    string b = st.pop();
    println("stack ${b} ${st.size()} ${wide.pop()} ${wide.size()}");
}
"#,
        r#"areas 13.0 3.0
limits max min neither
parsed 12 -1 2.5 7
stack b 1 170141183460469231731687303715884105727 0
"#,
    );
}

/// Polymorphic recursion that grows its type arguments without end is rejected when
/// instantiating (DESIGN §10), rather than generating forever.
#[test]
fn runaway_instantiation_is_an_error() {
    let file = SourceFile::new(
        "prog.zn",
        r#"class Pair<A, B> { A first; B second; }
<T> i64 grow(T x, i64 n) {
  if (n == 0) { return 0; }
  return 1 + grow(new Pair<T, T>(x, x), n - 1);
}
void main() { println("${grow(1, 3)}"); }
"#,
    );
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    assert!(c.is_none());
    assert!(diags[0].iter().any(|d| {
        d.message
            .contains("instantiates generic code with ever larger type arguments")
    }));
}

/// Whether clang is installed, which reading C headers needs (DESIGN §12.6).
fn has_clang() -> bool {
    Command::new("clang")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Calling C (DESIGN §12.6): output from Zinkite and from C's `puts` in order, through one
/// `stdout`; functions from six headers called as they are, `const char *` as text, macro
/// constants, `FILE` handles compared with `null`, `extern.errno()`, an explicit `extern`
/// for what a header can't say, and `extern link m`.
#[test]
fn calling_c() {
    if !has_clang() {
        eprintln!("skipped: no clang to read C headers");
        return;
    }
    prints(
        r#"extern import stdio.h;
extern import math.h;
extern import string.h;
extern import stdlib.h;
extern import ctype.h;
extern import unistd.h;
extern link m;

// write's buffer is a `const void *`, which the header can't say is bytes: declared here.
extern("write") i64 writeBytes(i32 fd, u8[:] data, u64 n);

void main() {
    // Zinkite's output goes through C's stdout, so the two interleave in order.
    println("from Zinkite");
    puts("from C");
    println("from Zinkite again");
    f64 r = sqrt(2.0);
    println("sqrt ${r.fixed(6)} pow ${pow(2.0, 10.0)} strlen ${strlen("zinkite")} atoi ${atoi("-42")}");
    println("toupper ${(u8) toupper('q')} abs ${abs(-3)} EOF ${EOF} SEEK_END ${SEEK_END} pid ${getpid() > 0}");

    string path = "/tmp/zink-c-calling-test.txt";
    FILE f = fopen(path, "w");
    if (f == null) {
        println("can't open ${path}");
        return;
    }
    fputs("line one\n", f);
    fputs("line two\n", f);
    i32 closed = fclose(f);
    FILE g = fopen(path, "r");
    u8[..>] back = [];
    i32 c = fgetc(g);
    while (c != EOF) {
        back +> (u8) c;
        c = fgetc(g);
    }
    @_ = fclose(g);
    print("closed ${closed}, read back:\n${back}");

    FILE missing = fopen("/nonexistent/dir/x", "r");
    u8 why = extern.errno();
    println("missing null ${missing == null} errno ${why} ${status.message(why)}");
    u8[..>] msg = [];
    msg.append("to stderr through write(2)\n");
    i64 n = writeBytes(2, msg, (u64) msg.len);
    println("wrote ${n} bytes, removed ${remove(path)}");
}
"#,
        r#"from Zinkite
from C
from Zinkite again
sqrt 1.414214 pow 1024.0 strlen 7 atoi -42
toupper 81 abs 3 EOF -1 SEEK_END 2 pid true
closed 0, read back:
line one
line two
missing null true errno 2 No such file or directory
wrote 27 bytes, removed 0
"#,
    );
}

// ---------------------------------------------------------------- the standard library

/// zink.io (STDLIB §4), written in Zinkite on the runtime's zr_io and zr_fs functions:
/// File's reads, writes, seeks and statuses, a copy of a closed File failing even once
/// its descriptor is reused, whole-file reads and atomic writes, fs, Dir and path.
#[test]
fn zink_io_files_and_directories() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-io");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let r = run(
        r##"import zink.io.*;

// A path in buf, which only grows, so the views stay valid.
string at(u8[..>]& buf, string dir, string name) {
    i64 start = buf.len;
    buf.append(dir);
    buf +> '/';
    buf.append(name);
    return buf[start:];
}

void main(string[.] args) {
    string dir = args[0];
    u8[..>] names = [];
    string a = at(names, dir, "a.txt");
    string b = at(names, dir, "b.txt");
    string c = at(names, dir, "c.txt");
    string x = at(names, dir, "x");
    string sub = at(names, dir, "x/y/z");
    string link = at(names, dir, "l");
    string missing = at(names, dir, "missing");

    // Writing, then reading back line by line.
    File f @_ = File.create(a);
    f.println("first");
    f.write("second\r\nthird");
    println("size ${f.size()} at ${f.position()} terminal ${f.isTerminal()}");
    f.close();

    File g @_ = File.open(a);
    u8[..~] line = [];
    while (string l : g.nextLine(line)) {
        println("line '${l}' ${l.len}");
    }
    g.seek(6);
    u8[..>] rest = [];
    g.readAll(rest);
    println("after seek: ${rest.len} bytes, at ${g.position()}");
    g.seek(0);
    u8[..>] some = [];
    i64 n = g.read(some, 3);
    println("read ${n}: ${some}, at ${g.position()}");
    i64 more = g.read(some, 100);
    i64 none @end = g.read(some, 100);
    println("then ${more}, then ${none} ${end == status.END}");

    // A copy of a closed File fails, even once a new file has its descriptor.
    File copy = g;
    g.close();
    u8 twice = @g.close();
    File other @_ = File.open(a);
    u8 stale = @copy.readLine(line);
    println("closed twice ${twice == status.BAD_HANDLE}, stale copy ${stale == status.BAD_HANDLE}");
    other.close();

    // Opening fails with a status.
    File m @notFound = File.open(missing);
    File e @exists = File.createNew(a);
    println("open missing ${notFound == status.NOT_FOUND}, createNew existing ${exists == status.EXISTS}");

    File ap @_ = File.append(a);
    ap.print("!");
    ap.close();
    u8[..>] whole = [];
    File.readAll(a, whole);
    println("appended: ends with ${whole[whole.len - 2:]}");

    // Whole files.
    File.writeAll(b, "one\n");
    File.appendAll(b, "two\n");
    File.writeAll(b, "zero\n");
    File.appendAll(b, "one\n");
    u8[..>] both = [];
    File.readAll(b, both);
    print(both);
    u8 noFile = @File.readAll(missing, both);
    println("readAll missing ${noFile == status.NOT_FOUND}");

    // The file system.
    println("exists ${fs.exists(b)} ${fs.exists(sub)}");
    fs.mkdirAll(sub);
    u8 again = @fs.mkdirAll(sub);
    u8 onFile = @fs.mkdirAll(b);
    u8 dup = @fs.mkdir(sub);
    println("mkdirAll again ${again}, over a file ${onFile == status.NOT_DIRECTORY}, mkdir existing ${dup == status.EXISTS}");
    Stat sd @_ = fs.stat(sub);
    Stat sf @_ = fs.stat(b);
    println("dir ${sd.isDir} ${sd.isFile}, file ${sf.size} ${sf.isFile} ${sf.modified > 1600000000000000000}");
    fs.chmod(b, 384);
    Stat sm @_ = fs.stat(b);
    println("mode ${sm.mode % 512}");
    fs.symlink("b.txt", link);
    u8[..>] target = [];
    fs.readLink(link, target);
    Stat sl @_ = fs.lstat(link);
    Stat st @_ = fs.stat(link);
    println("link -> ${target}, lstat ${sl.isLink}, stat ${st.isLink} ${st.isFile}");
    fs.rename(b, c);
    u8 gone = @fs.remove(b);
    u8 full = @fs.remove(x);
    Stat dangling @broken = fs.stat(link);
    println("renamed ${fs.exists(c)} ${gone == status.NOT_FOUND}, not empty ${full == status.NOT_EMPTY}, dangling ${broken == status.NOT_FOUND}");

    // Listing a directory, in sorted order.
    Dir d @_ = Dir.open(dir);
    u8[..>] dn = [];
    string[..~] found = [];
    while (DirEntry en : d.next(dn)) {
        found +> en.name;
    }
    found.sort();
    for (string fname : found) {
        print("${fname} ");
    }
    println("");
    d.close();
    u8 dclosed = @d.close();
    Dir nd @noDir = Dir.open(missing);
    Dir fd @notDir = Dir.open(a);
    println("dir closed twice ${dclosed == status.BAD_HANDLE}, missing ${noDir == status.NOT_FOUND}, a file ${notDir == status.NOT_DIRECTORY}");

    // Paths.
    println("${path.base("dir/file.tar.gz")} ${path.dir("dir/file.tar.gz")} ${path.ext("dir/file.tar.gz")} ${path.stem("dir/file.tar.gz")}");
    println("${path.base("/")} ${path.dir("/a")} ${path.dir("a")} '${path.ext(".bashrc")}' ${path.base("a/b//")} ${path.dir("a/b//c")} ${path.isAbsolute("/x")} ${path.isAbsolute("x")}");
    path.chdir(x);
    u8[..>] here = [];
    path.cwd(here);
    println("cwd ends in /x ${path.base(here) == "x"}, relative ${fs.exists("y/z")}");
}
"##,
        &[dir.to_str().unwrap()],
    );
    assert_eq!(
        r.stdout,
        "size 19 at 19 terminal false
line 'first' 5
line 'second' 6
line 'third' 5
after seek: 13 bytes, at 19
read 3: fir, at 3
then 16, then 0 true
closed twice true, stale copy true
open missing true, createNew existing true
appended: ends with d!
zero
one
readAll missing true
exists true false
mkdirAll again 0, over a file true, mkdir existing true
dir true false, file 9 true true
mode 384
link -> b.txt, lstat true, stat false true
renamed true true, not empty true, dangling true
a.txt c.txt l x 
dir closed twice true, missing true, a file true
file.tar.gz dir .gz file.tar
/ / . '' b a/b true false
cwd ends in /x true, relative true
",
        "stderr: {}",
        r.stderr
    );
    assert_eq!(r.code, 0);
}

/// zink.cli's Args (STDLIB §6.1): combined and attached letters, `--name=value`, `--`,
/// counts, repeated options, rest values, the generated help, --version, and usage errors
/// on stderr with exit status 2.
#[test]
fn zink_cli_arguments() {
    let exe = build(
        r##"import zink.cli.*;

void main(string[.] args) {
    Args cli = new Args("tool", "Does things to files.");
    cli.version = "1.2";
    cli.flag('v', "verbose", "say more; twice for even more");
    cli.flag(0, "dry-run", "change nothing");
    cli.option('o', "out", "FILE", "write here");
    cli.option(0, "scale", "X", "a factor");
    cli.positional("mode", "what to do");
    cli.rest("files", "the files");
    cli.parseOrExit(args);
    f64 scale @bad = cli.getF64("scale", 1.5);
    print("v=${cli.count("verbose")} dry=${cli.has("dry-run")} out=${cli.get("out", "-")} scale=${scale}/${bad} mode=${cli.get("mode", "?")} files:");
    ArgIterator files = cli.restValues();
    while (string f : files.next()) {
        print(" ${f}");
    }
    print(" | out:");
    ArgIterator outs = cli.getAll("out");
    while (string o : outs.next()) {
        print(" ${o}");
    }
    println("");
}
"##,
    );
    let out = |args: &[&str]| {
        let r = exec(&exe, args);
        (r.stdout, r.stderr, r.code)
    };
    assert_eq!(
        out(&["--help"]).0,
        "usage: tool [-v] [--dry-run] [-o FILE] [--scale X] [mode] [files...]

Does things to files.

  -v, --verbose     say more; twice for even more
      --dry-run     change nothing
  -o, --out FILE    write here
      --scale X     a factor
  -h, --help        show this help and exit
      --version     show the version and exit
  mode              what to do
  files...          the files
"
    );
    assert_eq!(out(&["--version"]), ("tool 1.2\n".into(), "".into(), 0));
    let cases: &[(&[&str], &str)] = &[
        (
            &["-vv", "-o", "out.txt", "run", "a", "b", "c"],
            "v=2 dry=false out=out.txt scale=1.5/0 mode=run files: a b c | out: out.txt\n",
        ),
        (
            &["-o", "x", "--out=y", "-oz", "go"],
            "v=0 dry=false out=z scale=1.5/0 mode=go files: | out: x y z\n",
        ),
        (
            &["--dry-run", "--scale=2.5", "go"],
            "v=0 dry=true out=- scale=2.5/0 mode=go files: | out:\n",
        ),
        (
            &["--scale", "x", "go"],
            "v=0 dry=false out=- scale=0.0/22 mode=go files: | out:\n",
        ),
        (
            &["run", "--", "-x", "--y"],
            "v=0 dry=false out=- scale=1.5/0 mode=run files: -x --y | out:\n",
        ),
        (
            &[],
            "v=0 dry=false out=- scale=1.5/0 mode=? files: | out:\n",
        ),
    ];
    for (args, expected) in cases {
        assert_eq!(
            out(args),
            (expected.to_string(), "".into(), 0),
            "{:?}",
            args
        );
    }
    let usage = "usage: tool [-v] [--dry-run] [-o FILE] [--scale X] [mode] [files...]\n";
    for (args, problem) in [
        (&["-x"][..], "unknown option -x"),
        (&["--bogus"][..], "unknown option --bogus"),
        (&["-o"][..], "this needs a value: -o"),
        (&["--out"][..], "this needs a value: --out"),
        (&["--dry-run=1"][..], "this takes no value: --dry-run"),
    ] {
        assert_eq!(
            out(args),
            ("".into(), format!("tool: {}\n{}", problem, usage), 2),
            "{:?}",
            args
        );
    }
}

/// selectNth against a full sort at random positions, with many repeats and with all
/// equal; parsePrefix; compareNatural.
#[test]
fn select_parse_prefix_and_natural_order() {
    prints(
        r##"u64 seed = 3;

i64 below(i64 n) {
    seed = seed *% 6364136223846793005 +% 1442695040888963407;
    return (i64) ((seed >> 33) % (u64) n);
}

void main() {
    // selectNth against a full sort, at every position, with many repeats.
    i64 bad = 0;
    for (i64 round = 0; round < 40; round++) {
        i64[..~] xs = [];
        i64 n = 1 + below(50);
        for (i64 i = 0; i < n; i++) { xs +> below(round % 2 == 0 ? 5 : 1000); }
        i64[..~] sorted = [];
        for (i64 x : xs) { sorted +> x; }
        sorted.sort();
        i64 k = below(n);
        i64& got = xs.selectNth(k);
        if (got != sorted[k]) { bad++; }
        for (i64 i = 0; i < k; i++) { if (xs[i] > got) { bad++; } }
        for (i64 i = k + 1; i < n; i++) { if (xs[i] < got) { bad++; } }
    }
    i64[..~] same = [];
    for (i64 i = 0; i < 2000; i++) { same +> 7; }
    i64 mid = same.selectNth(1000);
    i64[..~] ys = [5, 1, 4, 2, 3];
    i64 top = ys.selectNth(1, (i64 a, i64 b) => a > b);
    println("selectNth mismatches ${bad}, all-equal ${mid}, second largest ${top}");

    (f64 v, string rest) @e1 = f64.parsePrefix("3.25kg");
    (i64 n2, string r2) @e2 = i64.parsePrefix("-42px");
    (f64 v3, string r3) @e3 = f64.parsePrefix("abc");
    (f64 v4, string r4) @e4 = f64.parsePrefix("1e5x 2.e");
    println("parsePrefix ${v} '${rest}' ${e1} | ${n2} '${r2}' ${e2} | ${v3} '${r3}' ${e3} | ${v4} '${r4}' ${e4}");

    println("natural ${"file2".compareNatural("file10")} ${"v1.9".compareNatural("v1.10")} ${"01".compareNatural("1")} ${"a".compareNatural("a")} ${"b10".compareNatural("b9")}");
}
"##,
        "selectNth mismatches 0, all-equal 7, second largest 4
parsePrefix 3.25 'kg' 0 | -42 'px' 0 | 0.0 'abc' 22 | 100000.0 'x 2.e' 0
natural -1 -1 -1 0 1
",
    );
}

/// A diagnostic in a program that imports the library still points into its own file.
#[test]
fn diagnostics_beside_the_library_point_into_the_program() {
    let text = "import zink.io.*;\n\nvoid main() {\n    i64 x = \"no\";\n}\n";
    let file = SourceFile::new("prog.zn", text);
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    assert!(c.is_none());
    let shown: Vec<String> = diags[0].iter().map(|d| render(&file, d)).collect();
    assert_eq!(shown.len(), 1, "{:?}", shown);
    assert!(shown[0].contains("prog.zn:4:13"), "{}", shown[0]);
}

/// Builds one of the examples.
fn example(name: &str) -> std::path::PathBuf {
    let path = format!("{}/../examples/{}.zn", env!("CARGO_MANIFEST_DIR"), name);
    let file = SourceFile::new(path.clone(), std::fs::read_to_string(&path).unwrap());
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    let Some(c) = c else {
        panic!(
            "{} didn't compile: {:?}",
            name,
            diags[0]
                .iter()
                .map(|d| render(&file, d))
                .collect::<Vec<_>>()
        )
    };
    let exe = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("ex-{}", name));
    zink::driver::compile(&c, &exe, false).unwrap();
    exe
}

/// The examples that need zink.io and zink.cli: csvsort, sortlines, stats and dirwalk.
#[test]
fn library_examples_run() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-examples");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tree/sub")).unwrap();
    let put = |name: &str, text: &str| {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p.to_str().unwrap().to_string()
    };
    let csv = put("fruit.csv", "name,qty\npear,3\napple,10\nfig,7\n");
    let lines = put(
        "lines.txt",
        "file10\nfile2\napple\n3 pears\n10 figs\nfile2\nBanana\n",
    );
    let nums = put("nums.txt", "4\n8\n15\n16\n23\n42\n8\n");
    put("tree/a", "abc");
    put("tree/sub/b", "hello");
    let tree = dir.join("tree");
    let tree = tree.to_str().unwrap();

    let check = |exe: &std::path::Path, args: &[&str], expected: &str| {
        let r = exec(exe, args);
        assert_eq!(r.stdout, expected, "{:?}: {}", args, r.stderr);
        assert_eq!(r.code, 0, "{:?}: {}", args, r.stderr);
    };
    let csvsort = example("csvsort");
    check(&csvsort, &[&csv], "apple,10\nfig,7\nname,qty\npear,3\n");
    check(
        &csvsort,
        &["-n", "-k", "2", &csv],
        "name,qty\npear,3\nfig,7\napple,10\n",
    );

    let sortlines = example("sortlines");
    let sorted = |args: &[&str]| {
        let mut a = args.to_vec();
        a.push(&lines);
        exec(&sortlines, &a).stdout.replace('\n', "|")
    };
    assert_eq!(
        sorted(&[]),
        "10 figs|3 pears|Banana|apple|file10|file2|file2|"
    );
    assert_eq!(
        sorted(&["-n"]),
        "file10|file2|apple|file2|Banana|3 pears|10 figs|"
    );
    assert_eq!(
        sorted(&["-V"]),
        "3 pears|10 figs|Banana|apple|file2|file2|file10|"
    );
    assert_eq!(
        sorted(&["-u", "-r"]),
        "file2|file10|apple|Banana|3 pears|10 figs|"
    );
    assert_eq!(sorted(&["--top", "2"]), "10 figs|3 pears|");

    check(
        &example("stats"),
        &[&nums],
        "count   7\nmin     4.0\nmax     42.0\nmean    16.5714\nstddev  12.8823\nmedian  15.0\np90     30.60000000000001\np99     40.85999999999999\n",
    );

    check(
        &example("dirwalk"),
        &[tree],
        &format!("           8  {t}\n           5  {t}/sub\n", t = tree),
    );
}

/// The rest of zink.io's paths and file system: join, normalize, relative and absolute
/// (lexical), resolve, copy keeping the mode, move, removeAll not following a link out of
/// the tree, and temporary files and directories under $TMPDIR.
#[test]
fn zink_io_paths_and_file_system() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-io-2");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let exe = build(
        r##"import zink.io.*;

string at(u8[..>]& buf, string dir, string name) {
    i64 start = buf.len;
    buf.append(dir);
    buf +> '/';
    buf.append(name);
    return buf[start:];
}

void norm(string p) {
    print("'${path.normalize(p)}' ");
}

void rel(string a, string b) {
    u8[.] r @bad = path.relative(a, b);
    print("${r}${bad != 0 ? "!" : ""} ");
}

void main(string[.] args) {
    // Lexical paths.
    println("${path.join("a", "b")} ${path.join("a/", "b")} ${path.join("a", "/b")} ${path.join("", "b")} ${path.join("a", "")}");
    norm("a/./b/../c/");
    norm("a/../..");
    norm("/..");
    norm("");
    norm("//x//y/.");
    norm("../../a/../b");
    println("");
    rel("a/b", "a/c/d");
    rel("/x/y", "/x/y");
    rel("/x/y", "/");
    rel("a", "/b");
    rel("../a", "b");
    rel("../a", "../a/b");
    println("");
    u8[.] abs = path.absolute("x/../y");
    println("absolute ends in /y ${path.base(abs) == "y"} ${path.isAbsolute(abs)}");

    string dir = args[0];
    u8[..>] names = [];
    string tree = at(names, dir, "tree");
    string deep = at(names, dir, "tree/a/b/c");
    string f1 = at(names, dir, "tree/a/one");
    string f2 = at(names, dir, "tree/a/b/c/two");
    string ln = at(names, dir, "tree/link");
    string outside = at(names, dir, "outside");
    string kept = at(names, dir, "outside/kept");
    string copied = at(names, dir, "copied");
    string moved = at(names, dir, "moved");
    @_ = fs.mkdirAll(deep);
    @_ = fs.mkdirAll(outside);
    @_ = File.writeAll(f1, "one");
    @_ = File.writeAll(f2, "two!");
    @_ = File.writeAll(kept, "keep");
    @_ = fs.chmod(f1, 416);
    @_ = fs.symlink(outside, ln);

    // copy keeps the mode; move renames.
    u8 c = @fs.copy(f1, copied);
    Stat cs @_ = fs.stat(copied);
    u8[..>] back = [];
    @_ = File.readAll(copied, back);
    u8 cd = @fs.copy(tree, copied);
    println("copy ${c} '${back}' mode ${cs.mode % 512}, a directory ${cd == status.IS_DIRECTORY}");
    u8 m = @fs.move(copied, moved);
    println("move ${m} ${fs.exists(copied)} ${fs.exists(moved)}");

    // resolve follows the link.
    u8[.] real @_ = path.resolve(ln);
    u8[.] realOut @_ = path.resolve(outside);
    u8[.] none @nf = path.resolve(at(names, dir, "nope"));
    println("resolve ${real == realOut} missing ${nf == status.NOT_FOUND} '${none}'");

    // removeAll doesn't follow the link out of the tree.
    u8 ra = @fs.removeAll(tree);
    u8 again = @fs.removeAll(tree);
    println("removeAll ${ra} ${fs.exists(tree)} kept ${fs.exists(kept)}, again ${again}");

    // Temporary files and directories.
    u8[..>] tp = [];
    File t @te = fs.tempFile(tp);
    @_ = t.write("temp");
    @_ = t.close();
    Stat ts @_ = fs.stat(tp);
    u8[..>] td = [];
    u8 tde = @fs.tempDir(td);
    Stat ds @_ = fs.stat(td);
    println("tempFile ${te} ${path.base(tp).startsWith("zink-")} ${ts.size} mode ${ts.mode % 512}, tempDir ${tde} ${ds.isDir} mode ${ds.mode % 512}");
    @_ = fs.remove(tp);
    @_ = fs.remove(td);
}
"##,
    );
    let out = Command::new(&exe)
        .arg(&dir)
        .env("TMPDIR", &dir)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "a/b a/b /b b a
'a/c' '..' '/' '.' '/x/y' '../../b' 
../c/d . ../.. ! ! b 
absolute ends in /y true true
copy 0 'one' mode 416, a directory true
move 0 false true
resolve true missing true ''
removeAll 0 false kept true, again 0
tempFile 0 true 4 mode 384, tempDir 0 true mode 448
",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["moved", "outside"]);
}

/// A function's array result used as a temporary lasts to the end of its statement, even
/// when the same function is called again in it (DESIGN §3.3).
#[test]
fn returned_arrays_as_temporaries() {
    prints(
        r##"u8[.] two(string a, string b) {
    u8[..>] out = [];
    out.append(a);
    out.append(b);
    return out;
}

void main() {
    println("${two("a", "b")} ${two("c", "d")}");
    bool same = two("x", "y") == two("z", "w");
    println("${same}");
    u8[.] k = two("k", "l");
    println(k);
}
"##,
        "ab cd\nfalse\nkl\n",
    );
}

// ---------------------------------------------------------------- interfaces

/// DESIGN §6.3's example: references into two arrays of different classes, in one array
/// of `Shape&`, called through their tables, and `instanceof` binding a `Circle&`.
#[test]
fn interfaces_dispatch() {
    prints(
        r##"interface Shape {
    f64 area();
}

class Circle implements Shape {
    f64 r;
    new(f64 r) { this.r = r; }
    f64 area() { return 3.14159 * r * r; }
}

class Rect implements Shape {
    f64 w;
    f64 h;
    new(f64 w, f64 h) { this.w = w; this.h = h; }
    f64 area() { return w * h; }
}

void main() {
    Circle[..>] circles = [];
    Rect[..>] rects = [];
    Shape&[..>] shapes = [];
    shapes.push(circles.push(new Circle(1.0)));
    shapes.push(rects.push(new Rect(2.0, 3.0)));

    f64 total = 0.0;
    for (Shape& s : shapes) { total += s.area(); }
    println("${total}");
    for (Shape& s : shapes) {
        if (s instanceof Circle c) { println("circle ${c.r}"); }
    }
}
"##,
        "9.14159\ncircle 1.0\n",
    );
}

/// Methods with arguments, `mut` and `void` ones through an interface, a status from
/// only some implementations, nullable references, `instanceof` in `&&` and `else if`,
/// references in a field, a capped array and a slice, one interface for two classes and
/// two interfaces on one class, and a generic class for a generic interface, with `T`
/// inferred through `implements`.
#[test]
fn interfaces_in_depth() {
    prints(
        r##"interface Shape {
    f64 area();
    string name();
    bool bigger(Shape& other);
}

interface Counter {
    mut void add(i64 n);
    i64 total();
    // Fails in some implementations only.
    mut i64 take();
}

class Circle implements Shape, Counter {
    f64 r;
    i64 hits = 0;
    new(f64 r) { this.r = r; }
    f64 area() { return 3.0 * r * r; }
    string name() { return "circle"; }
    bool bigger(Shape& other) { return area() > other.area(); }
    mut void add(i64 n) { hits += n; }
    i64 total() { return hits; }
    mut i64 take() {
        if (hits == 0) { return #status.END 0; }
        hits--;
        return hits;
    }
}

class Square implements Shape {
    f64 s;
    new(f64 s) { this.s = s; }
    f64 area() { return s * s; }
    string name() { return "square"; }
    bool bigger(Shape& other) { return area() > other.area(); }
}

class Tally implements Counter {
    i64 n = 0;
    mut void add(i64 k) { n += k; }
    i64 total() { return n; }
    mut i64 take() { return n; }
}

interface Box<T> {
    T get();
}

class Cell<T> implements Box<T> {
    T v;
    new(T v) { this.v = v; }
    T get() { return v; }
}

class Holder {
    Shape& shape;
    new(Shape& s) { this.shape = s; }
}

Shape& largest(Shape&[:] all) {
    Shape& best = all[0];
    for (Shape& s : all) {
        if (s.bigger(best)) { best .= s; }
    }
    return best;
}

void bump(Counter& c, i64 times) {
    for (i64 i = 0; i < times; i++) { c.add(i); }
}

string describe(Shape&? s) {
    if (s == null) { return "nothing"; }
    if (s instanceof Circle c && c.r > 1.0) { return "a big circle"; }
    else if (s instanceof Circle) { return "a small circle"; }
    return s.name();
}

<T> T unbox(Box<T>& b) { return b.get(); }

void main() {
    Circle c1 = new Circle(1.0);
    Circle c2 = new Circle(2.0);
    Square q = new Square(3.0);
    Shape&[..~4] shapes = [];
    @_ = shapes +> c1;
    @_ = shapes +> q;
    @_ = shapes +> c2;
    Shape& big = largest(shapes);
    println("largest ${big.name()} ${big.area()}");

    Holder h = new Holder(q);
    println("held ${h.shape.name()} ${h.shape.area()}");

    bump(c1, 4);
    Tally t = new Tally();
    bump(t, 3);
    println("counts ${c1.total()} ${t.total()}");
    Counter& k = c1;
    i64 sum = 0;
    while (i64 x : k.take()) { sum += x; }
    println("took ${sum}, left ${k.total()}");
    Counter& tk = t;
    i64 v @st = tk.take();
    println("tally take ${v} ${st}");

    Shape&? none = null;
    println("${describe(none)}, ${describe(c1)}, ${describe(c2)}, ${describe(q)}");

    Cell<i64> ci = new Cell<i64>(42);
    Cell<string> cs = new Cell<string>("hi");
    Box<i64>& bi = ci;
    println("boxes ${bi.get()} ${unbox(cs)} ${unbox(ci)}");
}
"##,
        "largest circle 12.0
held square 9.0
counts 6 3
took 15, left 0
tally take 3 0
nothing, a small circle, a big circle, square
boxes 42 hi 42
",
    );
    prints(
        r##"interface Named { string name(); }
interface Marker {}
interface Box<T> { T& slot(); }

class Cell<T> implements Box<T>, Marker, Named {
    T v;
    new(T v) { this.v = v; }
    T& slot() { return v; }
    string name() { return "cell"; }
}

class Dog implements Named, Marker {
    string n;
    string name() { return n; }
}

<T> string twice(T& x) { return "?"; }

void main() {
    Cell<i64> c = new Cell<i64>(5);
    Box<i64>& b = c;
    b.slot() = 9;
    println("slot ${c.v} ${b.slot()}");
    Dog d = new Dog("rex");
    Named& n1 = d;
    Named& n2 = c;
    Marker& m = d;
    println("${n1.name()} ${n2.name()} ${n2 instanceof Cell<i64>} ${n2 instanceof Cell<string>} ${m instanceof Dog}");
    println(twice(n1));
}
"##,
        "slot 9 9\nrex cell true false true\n?\n",
    );
}

/// A for-each over an iterator class is a repeat loop on a hidden local (DESIGN §9.4):
/// with `continue` and `break`, on a class with `iterator()`, on a generic iterator, and
/// a repeat loop through an interface reference.
#[test]
fn iterator_classes() {
    prints(
        r##"class Range implements Iterable {
    i64 lo;
    i64 hi;
    Steps iterator() { return new Steps(lo, hi); }
}

interface Iterable {
    Steps iterator();
}

class Steps {
    i64 at;
    i64 hi;
    mut i64 next() {
        if (at >= hi) { return #status.END 0; }
        at++;
        return at - 1;
    }
}

class Words {
    string text;
    i64 at = 0;
    mut string next() {
        while (at < text.len && text[at] == ' ') { at++; }
        if (at >= text.len) { return #status.END ""; }
        i64 start = at;
        while (at < text.len && text[at] != ' ') { at++; }
        return text[start:at];
    }
}

interface Source<T> {
    mut T next();
}

class Pairs<T> implements Source<T> {
    T a;
    T b;
    i64 n = 0;
    mut T next() {
        n++;
        if (n == 1) { return a; }
        if (n == 2) { return b; }
        return #status.END a;
    }
}

Words split(string s) { return new Words(s); }

void main() {
    for (i64 i : new Range(2, 5)) { print("${i} "); }
    println("");
    for (string w : split("  the quick  fox ")) {
        if (w == "quick") { continue; }
        print("[${w}]");
    }
    println("");
    i64 found = -1;
    for (i64 i : new Steps(0, 100)) {
        if (i * i > 50) { found = i; break; }
    }
    println("found ${found}");
    Pairs<string> p = new Pairs<string>("x", "y");
    Source<string>& src = p;
    while (string s : src.next()) { print("${s} "); }
    println("");
    for (i64 v : new Pairs<i64>(7, 8)) { print("${v} "); }
    println("");
}
"##,
        "2 3 4 \n[the][fox]\nfound 8\nx y \n7 8 \n",
    );
}

/// `zink.lang`'s `Iterator<T>`, without an import: as a bound, as a reference, and
/// implemented by `zink.cli`'s `ArgIterator`, so a for-each loops over `restValues()`.
#[test]
fn the_iterator_interface() {
    let r = run(
        r##"import zink.cli.*;

class Countdown implements Iterator<i64> {
    i64 n;
    mut i64 next() {
        if (n == 0) { return #status.END 0; }
        n--;
        return n + 1;
    }
}

<I implements Iterator<i64>> i64 sum(I& it) {
    i64 total = 0;
    while (i64 x : it.next()) { total += x; }
    return total;
}

i64 count(Iterator<string>& it) {
    i64 n = 0;
    while (string s : it.next()) { n++; }
    return n;
}

void main(string[.] args) {
    Countdown c = new Countdown(4);
    println("sum ${sum(c)}");
    Args cli = new Args("t", "");
    cli.rest("files", "the files");
    cli.parseOrExit(args);
    for (string f : cli.restValues()) { print("<${f}>"); }
    println("");
    ArgIterator all = cli.restValues();
    println("count ${count(all)}");
}
"##,
        &["a", "bb", "c"],
    );
    assert_eq!(r.stdout, "sum 10\n<a><bb><c>\ncount 3\n", "{}", r.stderr);
}

/// The library's iterators over text and arrays are `zink.lang` classes that implement
/// `Iterator<T>`: kept in variables, stepped by hand, given to generic code and passed as
/// an `Iterator<string>&`.
#[test]
fn library_iterators_are_classes() {
    prints(
        r##"i64 count(Iterator<string>& it) {
    i64 n = 0;
    while (string s : it.next()) { n++; }
    return n;
}

<I implements Iterator<string>> string longest(I& it) {
    string best = "";
    while (string s : it.next()) {
        if (s.len > best.len) { best = s; }
    }
    return best;
}

void main() {
    string text = "one two\nthree four five\n\nsix";
    LineIterator lines = text.lines();
    string first @_ = lines.next();
    println("first '${first}'");
    while (string l : lines.next()) { println("then '${l}'"); }
    WordIterator ws = text.words();
    println("longest ${longest(ws)}");
    SplitIterator parts = "a,b,c".splitBy(",");
    println("parts ${count(parts)}");
    WordIterator all = text.words();
    println("words ${count(all)}");
    CodePointIterator cps = "héllo".codePoints();
    u32 h @_ = cps.next();
    u32 e @_ = cps.next();
    println("cps ${h} ${e}");
}
"##,
        "first 'one two'\nthen 'three four five'\nthen ''\nthen 'six'\nlongest three\nparts 3\nwords 6\ncps 104 233\n",
    );
    prints(
        r##"void main() {
    i64[..>] xs = [1, 2, 3, 4, 5, 6, 7];
    for (i64[:] c : xs.chunks(3)) { print("${c.len}:${c[0]} "); }
    println("");
    i64 best = 0;
    for (i64[:] w : xs.windows(3)) { best = max(best, w[0] + w[1] + w[2]); }
    println("best window ${best}");
    u8[..>] none = [];
    i64 n = 0;
    for (u8[:] c : none.chunks(2)) { n++; }
    for (i64[:] w : xs.windows(8)) { n++; }
    println("empty ${n}");
    string[..>] names = ["ann", "bo", "cy"];
    for (string[:] pair : names.windows(2)) { print("${pair[0]}-${pair[1]} "); }
    println("");
}
"##,
        "3:1 3:4 1:7 \nbest window 18\nempty 0\nann-bo bo-cy \n",
    );
}

/// lines(), words(), splitBy() and codePoints() at their edges: empty text, a lone `\r`,
/// `\r\n`, no newline at the end, separators at either end and empty ones, every ASCII
/// space, and invalid, cut-off and 4-byte UTF-8. The parts are what the runtime's C
/// versions gave before these were written in Zinkite.
#[test]
fn text_iterator_edges() {
    prints(
        r##"// A part in brackets, with bytes outside printable ASCII as <n>.
void part(string s) {
    print(" [");
    for (i64 i = 0; i < s.len; i++) {
        u8 b = s[i];
        if (b >= 32 && b < 127) {
            print(s[i:i + 1]);
        } else {
            print("<${b}>");
        }
    }
    print("]");
}

void show(string t) {
    print("lines:");
    for (string l : t.lines()) { part(l); }
    print(" words:");
    for (string w : t.words()) { part(w); }
    print(" split,:");
    for (string p : t.splitBy(",")) { part(p); }
    print(" split<>:");
    for (string p : t.splitBy("<>")) { part(p); }
    print(" split'':");
    for (string p : t.splitBy("")) { part(p); }
    print(" cps:");
    for (u32 c : t.codePoints()) { print(" ${c}"); }
    println("");
}

void main() {
    u8[..>] b = [];
    show("");
    show("\n");
    show("\r\n");
    show("\r");
    show("a\r");
    show("a\nb");
    show("a\r\nb\r\n\n");
    show(",");
    show("a,,b,");
    show("<>a<><>b<");
    show(" \t\u{0b}\u{0c}x\ry\n z ");
    show("déjà 😀");
    b.clear(); b +> 0xC3; show(b);
    b.clear(); b +> 0xE2; b +> 0x82; show(b);
    b.clear(); b +> 0xF0; b +> 0x9F; b +> 0x98; b +> 0x80; b +> 0x41; show(b);
    b.clear(); b +> 0x80; b +> 0xFF; b +> 0xC3; b +> 0x41; show(b);
}
"##,
        r##"lines: words: split,: [] split<>: [] split'': [] cps:
lines: [] words: split,: [<10>] split<>: [<10>] split'': [<10>] cps: 10
lines: [] words: split,: [<13><10>] split<>: [<13><10>] split'': [<13><10>] cps: 13 10
lines: [] words: split,: [<13>] split<>: [<13>] split'': [<13>] cps: 13
lines: [a] words: [a] split,: [a<13>] split<>: [a<13>] split'': [a<13>] cps: 97 13
lines: [a] [b] words: [a] [b] split,: [a<10>b] split<>: [a<10>b] split'': [a<10>b] cps: 97 10 98
lines: [a] [b] [] words: [a] [b] split,: [a<13><10>b<13><10><10>] split<>: [a<13><10>b<13><10><10>] split'': [a<13><10>b<13><10><10>] cps: 97 13 10 98 13 10 10
lines: [,] words: [,] split,: [] [] split<>: [,] split'': [,] cps: 44
lines: [a,,b,] words: [a,,b,] split,: [a] [] [b] [] split<>: [a,,b,] split'': [a,,b,] cps: 97 44 44 98 44
lines: [<>a<><>b<] words: [<>a<><>b<] split,: [<>a<><>b<] split<>: [] [a] [] [b<] split'': [<>a<><>b<] cps: 60 62 97 60 62 60 62 98 60
lines: [ <9><11><12>x<13>y] [ z ] words: [x] [y] [z] split,: [ <9><11><12>x<13>y<10> z ] split<>: [ <9><11><12>x<13>y<10> z ] split'': [ <9><11><12>x<13>y<10> z ] cps: 32 9 11 12 120 13 121 10 32 122 32
lines: [d<195><169>j<195><160> <240><159><152><128>] words: [d<195><169>j<195><160>] [<240><159><152><128>] split,: [d<195><169>j<195><160> <240><159><152><128>] split<>: [d<195><169>j<195><160> <240><159><152><128>] split'': [d<195><169>j<195><160> <240><159><152><128>] cps: 100 233 106 224 32 128512
lines: [<195>] words: [<195>] split,: [<195>] split<>: [<195>] split'': [<195>] cps: 65533
lines: [<226><130>] words: [<226><130>] split,: [<226><130>] split<>: [<226><130>] split'': [<226><130>] cps: 65533 65533
lines: [<240><159><152><128>A] words: [<240><159><152><128>A] split,: [<240><159><152><128>A] split<>: [<240><159><152><128>A] split'': [<240><159><152><128>A] cps: 128512 65
lines: [<128><255><195>A] words: [<128><255><195>A] split,: [<128><255><195>A] split<>: [<128><255><195>A] split'': [<128><255><195>A] cps: 65533 65533 65533 65
"##,
    );
}

/// A function's array result is built where the caller keeps it (DESIGN §3.3): in a new
/// variable, through two levels of `return`, beside other returns (a call on the result,
/// a slice of it, a literal, another local), with a status, from a loop, in generic code,
/// through an interface, and from `next()`. An assignment `x = rev(x)` reads its own
/// target, so it copies.
#[test]
fn arrays_built_where_they_are_kept() {
    prints(
        r##"u8[.] word(string s) {
    u8[..>] out = [];
    out.append(s);
    out.append("!");
    return out;
}

// Two levels: the destination is passed on.
u8[.] twice(string s) { return word(s); }
u8[.] thrice(string s) {
    u8[..>] out = word(s);
    out.append("?");
    return out;
}

u8[.] rev(string s) {
    u8[..>] out = [];
    for (i64 i = s.len - 1; i >= 0; i--) { out +> s[i]; }
    return out;
}

// The result local beside other returns.
u8[.] pick(string s, i64 how) {
    u8[..>] out = [];
    out.append(s);
    if (how == 1) { return rev(out); }
    if (how == 2) { return out[1:]; }
    if (how == 3) { return "literal"; }
    u8[..>] other = [];
    other.append("other");
    if (how == 4) { return other; }
    return out;
}

u8[.] check(string s) {
    u8[..>] out = [];
    if (s.len == 0) { return #status.INVALID out; }
    out.append(s);
    return out;
}

u8[.] firstLong(string[:] words) {
    for (string w : words) {
        u8[..>] out = [];
        out.append(w);
        if (out.len > 3) { return out; }
    }
    return "none";
}

<T> T[..>] doubled(T[:] xs) {
    T[..>] out = [];
    for (T x : xs) { out +> x; out +> x; }
    return out;
}

interface Render { u8[.] render(); }
class Box implements Render {
    string label;
    u8[.] render() {
        u8[..>] out = [];
        out +> '[';
        out.append(label);
        out +> ']';
        return out;
    }
}

class Pieces {
    i64 n = 0;
    mut u8[.] next() {
        n++;
        if (n > 3) { return #status.END ""; }
        u8[..>] out = [];
        for (i64 i = 0; i < n; i++) { out +> 'x'; }
        return out;
    }
}

void main() {
    u8[.] a = word("hi");
    u8[.] b = twice("yo");
    u8[.] c = thrice("ok");
    println("${a} ${b} ${c} ${word("t1")}${word("t2")}");
    u8[..>] x = [];
    x.append("abc");
    x = rev(x);
    println("assigned ${x}");
    for (i64 how = 0; how <= 4; how++) {
        u8[.] p = pick("hello", how);
        print("${p} ");
    }
    println("");
    u8[.] ok @e1 = check("fine");
    u8[.] bad @e2 = check("");
    println("${ok} ${e1} '${bad}' ${e2}");
    string[..>] ws = ["a", "bb", "cccc", "ddddd"];
    u8[.] fl = firstLong(ws);
    println("first long ${fl}");
    i64[..>] nums = [1, 2, 3];
    i64[..>] dd = doubled(nums);
    println("doubled ${dd.len} ${dd[0]} ${dd[5]}");
    Box b1 = new Box("one");
    Box b2 = new Box("two");
    Render& r1 = b1;
    Render& r2 = b2;
    println("${r1.render()}${r2.render()}");
    u8[.] kept = r1.render();
    println("kept ${kept}");
    Pieces ps = new Pieces();
    while (u8[.] p : ps.next()) { print("${p} "); }
    for (u8[.] p : new Pieces()) { print("<${p}>"); }
    println("");
    u8[..>] grow = word("g");
    grow.append("rows");
    println(grow);
}
"##,
        "hi! yo! ok!? t1!t2!
assigned cba
hello olleh ello literal other 
fine 0 '' 22
first long cccc
doubled 6 1 3
[one][two]
kept [one]
x xx xxx <x><xx><xxx>
g!rows
",
    );
}

/// Reads in one go return the text, built where the caller keeps it (STDLIB §1.3):
/// `File.readAll(path)`, `f.readAll()`, `fs.readLink(path)`, `path.cwd()` and
/// `stdin.readAll()`, also assigned to an array that exists. A failure gives an empty one.
#[test]
fn one_shot_reads_return_text() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-oneshot");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let exe = build(
        r##"import zink.io.*;

string at(u8[..>]& buf, string dir, string name) {
    i64 start = buf.len;
    buf.append(dir);
    buf +> '/';
    buf.append(name);
    return buf[start:];
}

void main(string[.] args) {
    u8[..>] names = [];
    string f = at(names, args[0], "f.txt");
    string l = at(names, args[0], "link");
    string missing = at(names, args[0], "missing");
    @_ = File.writeAll(f, "alpha\nbeta\n");
    @_ = fs.symlink("f.txt", l);
    u8[.] whole @e1 = File.readAll(f);
    u8[.] none @e2 = File.readAll(missing);
    File h @_ = File.open(f);
    @_ = h.seek(6);
    u8[.] rest @e3 = h.readAll();
    @_ = h.close();
    u8[.] target @e4 = fs.readLink(l);
    u8[.] gone @e5 = fs.readLink(missing);
    u8[.] here @e6 = path.cwd();
    println("whole ${whole.len} ${e1}, missing ${none.len} ${e2 == status.NOT_FOUND}, rest ${rest.trim()} ${e3}");
    println("link ${target} ${e4}, gone ${gone.len} ${e5 == status.NOT_FOUND}, cwd ${path.isAbsolute(here)} ${e6}");
    u8[.] input @e7 = stdin.readAll();
    println("stdin ${input.len} ${e7}: ${input.trim()}");
    u8[..>] again = [];
    again.append("old");
    again = stdin.readAll();
    println("again ${again.len}");
}
"##,
    );
    use std::io::Write;
    let mut child = Command::new(&exe)
        .arg(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"from stdin\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "whole 11 0, missing 0 true, rest beta 0
link f.txt 0, gone 0 true, cwd true 0
stdin 11 0: from stdin
again 0
"
    );
}

// ---------------------------------------------------------------- pools and streams

/// `pool<T>` (DESIGN §3.5, STDLIB §7.5): add, get, free, a stale handle failing with
/// MISSING while a stale reference still reads a value of the type, a freed slot reused,
/// for-each over the live slots by reference and by value, `len`, and `clear`.
#[test]
fn pools() {
    prints(
        r##"class Conn {
    i64 fd;
    string name;
}

void main() {
    pool<Conn> conns = [];
    Handle<Conn> a = conns.add(new Conn(3, "a"));
    Handle<Conn> b = conns.add(new Conn(4, "b"));
    Handle<Conn> c = conns.add(new Conn(5, "c"));
    println("len ${conns.len}");
    Conn& cb @_ = conns.get(b);
    cb.fd = 40;
    u8 f1 = @conns.free(b);
    u8 f2 = @conns.free(b);
    Conn& stale @gone = conns.get(b);
    println("free ${f1} again ${f2 == status.MISSING}, get freed ${gone == status.MISSING}, stale still reads ${stale.fd}");
    Handle<Conn> d = conns.add(new Conn(6, "d"));
    Conn& dd @e = conns.get(d);
    println("reused ${dd.name} ${e}, old handle ${b == d}, same ${d == d}, stale reads ${stale.name}");
    for (Conn& x : conns) { print("${x.fd}:${x.name} "); }
    println("");
    @_ = conns.free(a);
    for (Conn x : conns) { print("${x.name} "); }
    println("len ${conns.len}");
    conns.clear();
    Conn& after @ce = conns.get(c);
    println("cleared ${conns.len} ${ce == status.MISSING}");
    Handle<Conn> e1 = conns.add(new Conn(7, "e"));
    Handle<Conn> e2 = conns.add(new Conn(8, "f"));
    Conn& g1 @_ = conns.get(e1);
    Conn& g2 @_ = conns.get(e2);
    println("after clear ${g1.name}${g2.name} len ${conns.len}");
}
"##,
        "len 3
free 0 again true, get freed true, stale still reads 40
reused d 0, old handle false, same true, stale reads d
3:a 6:d 5:c 
d c len 2
cleared 0 true
after clear ef len 2
",
    );
}

/// stdin, stdout and stderr are `File`s (STDLIB §2.1): writes through them stay in order
/// with print, stdout.flush() works, and a copy of stdout writes there too.
#[test]
fn standard_streams_are_files() {
    let r = run(
        r##"import zink.io.*;

void main() {
    println("1 println");
    @_ = stdout.write("2 stdout.write\n");
    errprintln("3 errprintln");
    @_ = stderr.println("4 stderr.println");
    @_ = stdout.println("5 stdout.println");
    print("6 print");
    u8 f = @stdout.flush();
    println(" flushed ${f}");
    File out = stdout;
    @_ = out.println("7 through a copy");
    println("terminal ${stdin.isTerminal()} ${stdout.isTerminal()}");
}
"##,
        &[],
    );
    assert_eq!(
        r.stdout,
        "1 println\n2 stdout.write\n5 stdout.println\n6 print flushed 0\n7 through a copy\nterminal false false\n"
    );
    assert_eq!(r.stderr, "3 errprintln\n4 stderr.println\n");
}

/// cachesim: an LRU cache as a root holding a pool of nodes linked through references into
/// it, and a hash map of handles, reading a trace from a file or from stdin.
#[test]
fn cachesim_example_runs() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-cachesim");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let trace = dir.join("trace.txt");
    std::fs::write(&trace, "5\n6\n5\n7\n5\n8\n6\nx\n").unwrap();
    let exe = example("cachesim");
    let expected = "capacity  2\naccesses  7\nhits      2 (28.57%)\nmisses    5\n";
    let r = exec(&exe, &["-c", "2", trace.to_str().unwrap()]);
    assert_eq!(r.stdout, expected, "{}", r.stderr);
    assert_eq!(r.stderr, "cachesim: skipped 1 lines that weren't keys\n");
    use std::io::Write;
    let mut child = Command::new(&exe)
        .args(["-c", "2"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"5\n6\n5\n7\n5\n8\n6\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

// ---------------------------------------------------------------- variable-size records

/// DESIGN §3.2's order book: records with inline text and an inline array of records,
/// built in place at the end of their array from nested literals, read through views,
/// looped over, changed through a reference, and rebuilt by assigning the variable.
#[test]
fn records_stored_inline() {
    prints(
        r##"class Item { u8[.*] sku; i32 qty; i32 cents; }
class Order { i32 id; u8[.*] customer; Item[.*] items; }

i64 total(Order o) {
    i64 t = 0;
    for (Item& it : o.items) { t += (i64) it.qty * (i64) it.cents; }
    return t;
}

void show(Order o) {
    println("order ${o.id} for ${o.customer}: ${o.items.len} items, ${total(o)} cents");
    for (Item& it : o.items) { println("  ${it.sku} x${it.qty} @ ${it.cents}"); }
}

void main() {
    Order[..>] book = [];
    book.push(new Order(1001, "alice", [new Item("SKU-441", 2, 1999), new Item("SKU-7", 1, 500)]));
    book +> new Order(1002, "bob", []);
    Order& third = book.push(new Order(1003, "christopher", [new Item("X", 10, 3)]));
    third.id = 2003;
    println("book ${book.len}");
    for (Order& o : book) { show(o); }
    Order single = new Order(7, "dee", [new Item("Y", 1, 1)]);
    show(single);
    single = new Order(8, "edward", []);
    show(single);
}
"##,
        "book 3
order 1001 for alice: 2 items, 4498 cents
  SKU-441 x2 @ 1999
  SKU-7 x1 @ 500
order 1002 for bob: 0 items, 0 cents
order 2003 for christopher: 1 items, 30 cents
  X x10 @ 3
order 7 for dee: 1 items, 1 cents
  Y x1 @ 1
order 8 for edward: 0 items, 0 cents
",
    );
}

/// A record inside a record, a `u8[.]` field (an 8-byte count), a field initializer, a
/// method changing a fixed field and one called through an interface, copies of records
/// into an array and into a field, interpolated text, a generic record, an array of
/// `u8[.]`, a build whose argument grows the array (built aside, REFERENCES §6 V4),
/// `clear`, and a record passed by value or as a temporary.
#[test]
fn records_in_depth() {
    prints(
        r##"interface Named { string name(); }

class Address { u8[.*] city; i32 zip; }
class Person implements Named {
    u8[.*] first;
    Address home;
    i32 age;
    u8[.] notes;
    i32 visits = 1;
    string name() { return first; }
    mut void older() { age++; }
}

class Tagged<T> { u8[.*] tag; T value; }

class Item { u8[.*] sku; i32 qty; }

i32 grow(Item[..>]& items) {
    items +> new Item("pushed during the build", 0);
    return 99;
}

void main() {
    Person p = new Person("ann", new Address("Oslo", 150), 30, "likes tea");
    p.older();
    println("${p.first} ${p.home.city} ${p.home.zip} ${p.age} '${p.notes}' ${p.visits}");
    Named& n = p;
    println("named ${n.name()}");

    Person[..>] people = [];
    people +> p;
    people +> new Person("bo", new Address("Rome", 100), 41, "");
    people +> new Person("cy-${p.age}", p.home, 5, p.notes);
    for (Person& q : people) { print("${q.first}/${q.home.city}/${q.age}/${q.notes.len} "); }
    println("");

    Tagged<i64> t = new Tagged<i64>("count", 42);
    Tagged<string> s = new Tagged<string>("word", "hello");
    println("${t.tag}=${t.value} ${s.tag}=${s.value}");

    u8[.][..>] lines = [];
    lines +> "first line";
    lines +> "second";
    i64 total = 0;
    for (u8[.] l : lines) { total += l.len; print("[${l}]"); }
    println(" ${total}");

    Item[..>] items = [];
    items +> new Item("a", 1);
    items +> new Item("b", grow(items));
    for (Item& it : items) { print("${it.sku}:${it.qty} "); }
    println("${items.len}");
    items.clear();
    items +> new Item("again", 2);
    for (Item& it : items) { println("${it.sku} ${it.qty} ${items.len}"); }

    Person copy = p;
    copy.older();
    println("copy ${copy.age} original ${p.age}");
}
"##,
        "ann Oslo 150 31 'likes tea' 1
named ann
ann/Oslo/31/9 bo/Rome/41/0 cy-31/Oslo/5/9 
count=42 word=hello
[first line][second] 16
a:1 pushed during the build:0 b:99 3
again 2 1
copy 32 original 31
",
    );
    prints(
        r##"class Order { i32 id; u8[.*] customer; }

i64 who(Order o) { return o.customer.len; }

void main() {
    Order o = new Order(1, "alice");
    println("${who(o)}");
    o = new Order(2, "alexander");
    string name = o.customer;
    println("${name} ${o.id}");
    println("${who(new Order(3, "temporary"))}");
}
"##,
        "5\nalexander 2\n9\n",
    );
}
