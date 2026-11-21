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
