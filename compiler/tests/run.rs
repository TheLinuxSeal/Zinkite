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
            "u8 b = 250; println(\"${b + 10} ${b +| 10} ${b -| 255} ${max(3, 9)} ${min(-1, 2)}\");",
        ),
        "4 255 0 9 -1\n",
    );
}

/// The C generated for a program, failing the test if it doesn't compile.
fn c_of(text: &str) -> String {
    let file = SourceFile::new("prog.zn", text);
    zink::compile_c(std::slice::from_ref(&file))
        .1
        .expect("compiles")
}

/// How many loops a program's C runs without their index checks (DESIGN §14.2).
fn versioned(text: &str) -> usize {
    c_of(text)
        .matches("/* in range: the loop without its index checks */")
        .count()
}

/// A `for` loop whose indexes move with its counter is generated twice, and runs without
/// its index checks when a test before it shows every index in range (DESIGN §14.2):
/// counting up and down, by a step, with an `i32` counter over a fixed array, through
/// a local computed from the counter, and over views rebound between runs of the loop.
#[test]
fn loops_whose_indexes_are_in_range_run_without_checks() {
    let text = r##"i64 W = 5;

u32 distance(string s1, string s2) {
    string a = s1.len <= s2.len ? s1 : s2;
    string b = s1.len <= s2.len ? s2 : s1;
    i64 m = a.len;
    u32[..>] prevRow = [];
    u32[..>] currRow = [];
    for (i64 i = 0; i <= m; i++) {
        prevRow +> (u32) i;
        currRow +> 0;
    }
    u32[:]& prev = prevRow;
    u32[:]& curr = currRow;
    for (i64 j = 0; j < b.len; j++) {
        u8 c = b[j];
        curr[0] = (u32) (j + 1);
        for (i64 i = 0; i < m; i++) {
            u32 cost = a[i] == c ? 0 : 1;
            curr[i + 1] = min(min(prev[i + 1] + 1, curr[i] + 1), prev[i] + cost);
        }
        u32[:]& t = prev;
        prev .= curr;
        curr .= t;
    }
    return prev[m];
}

void main() {
    println("${distance("kitten", "sitting")} ${distance("", "abc")} ${distance("flaw", "lawn")}");
    i64[..>] a = [];
    for (i64 i = 0; i < 10; i++) { a +> i * i; }
    i64 down = 0;
    for (i64 i = a.len - 1; i >= 0; i--) { down = down * 3 + a[i]; }
    i64 stepped = 0;
    for (i64 j = 2; j < a.len; j += 3) { stepped += a[j]; }
    i32[.8] f = [];
    for (i32 i = 0; i < 7; i++) { f[i + 1] = f[i] + i; }
    u8[..>] img = [];
    for (i64 i = 0; i < W * W; i++) { img +> (u8) (i * i * 13 % 256); }
    i64 edges = 0;
    for (i64 y = 1; y < W - 1; y++) {
        for (i64 x = 1; x < W - 1; x++) {
            i64 k = y * W + x;
            edges += (i64) img[k - W] + img[k + W] - img[k - 1] - img[k + 1];
        }
    }
    println("${down} ${stepped} ${f} ${edges}");
}
"##;
    assert_eq!(versioned(text), 5);
    prints(text, "3 3 2\n2155287 93 [0, 0, 1, 3, 6, 10, 15, 21] -16\n");
}

/// A versioned loop whose test fails runs with its checks, so it fails where it always
/// did, after what came before (DESIGN §14.2). And a loop that could change its bound or
/// its array through a reference, a removal or a call isn't versioned at all.
#[test]
fn a_loop_that_goes_out_of_range_still_fails_where_it_did() {
    let text = r##"i64[..~] shrinking = [];

void shrink() { i64 e @s = shrinking.pop(); }

void main(string[.] args) {
    i64[..>] a = [];
    a +> 10 +> 20 +> 30;
    string mode = args[0];
    if (mode == "over") {
        for (i64 i = 0; i < 5; i++) { println("${a[i]}"); }
    }
    if (mode == "down") {
        for (i64 i = 2; i >= -1; i--) { println("${a[i]}"); }
    }
    if (mode == "caught") {
        for (i64 i = 0; i < 4; i++) {
            u8 st = @(a[i]);
            print("${st} ");
        }
        println("");
    }
    if (mode == "ref") {
        i64 n = 3;
        i64& r = n;
        i64 total = 0;
        for (i64 i = 0; i < n; i++) {
            if (i == 1) { r = 5; }
            total += a[i];
            println("${total}");
        }
    }
    if (mode == "pop") {
        i64[..~] b = [1, 2, 3, 4];
        i64 total = 0;
        for (i64 i = 0; i < 4; i++) {
            total += b[i];
            i64 e = b.pop();
            println("${total}");
        }
    }
    if (mode == "call") {
        shrinking +> 1 +> 2 +> 3 +> 4;
        i64 total = 0;
        for (i64 i = 0; i < 4; i++) {
            total += shrinking[i];
            shrink();
            println("${total}");
        }
    }
}
"##;
    // `over`, `down` and `caught`; the other three loops can't count on their bounds.
    assert_eq!(versioned(text), 3);
    let exe = build(text);
    for (mode, out, err) in [
        ("over", "10\n20\n30\n", "prog.zn:10: index 3 is out of range for length 3"),
        ("down", "30\n20\n10\n", "prog.zn:13: index -1 is out of range for length 3"),
        ("caught", "0 0 0 34 \n", ""),
        ("ref", "10\n30\n60\n", "prog.zn:28: index 3 is out of range for length 3"),
        ("pop", "1\n3\n", "prog.zn:36: index 2 is out of range for length 2"),
        ("call", "1\n3\n", "prog.zn:45: index 2 is out of range for length 2"),
    ] {
        let r = exec(&exe, &[mode]);
        assert_eq!(r.stdout, out, "{}: {}", mode, r.stderr);
        assert!(r.stderr.contains(err), "{}: {}", mode, r.stderr);
        assert_eq!(r.code, if err.is_empty() { 0 } else { 1 }, "{}: {}", mode, r.stderr);
    }
}

/// A function that calls itself at least twice, and starts with a base case, has its
/// calls to itself test that case first and compute its result in place (DESIGN
/// §14.2): a test and a return, locals before tests, a `return switch` whose cases
/// that call nothing are the base case, a `void` function, a method, and a generic
/// function in each instantiation. Only the first test is copied: `weight` has two. A
/// function that calls itself once, `sumTo`, is left alone. An index out of range in
/// the copied part fails where it always did.
#[test]
fn calls_to_itself_copy_a_functions_base_case() {
    let text = r##"enum Kind { NUM, ADD, NEG }

class Node {
    Kind kind;
    i32 a;
    i32 b;
}

Node[..>] pool = [];
i64 visits = 0;

i32 fib(i32 n) {
    if (n < 2) { return n; }
    return fib(n - 1) + fib(n - 2);
}

i64 eval(i32 i) {
    Node& n = pool[i];
    return switch (n.kind) {
        case NUM => n.a;
        case ADD => eval(n.a) + eval(n.b);
        case NEG => 0 - eval(n.a);
    };
}

i64 depth(i32 i) {
    Node& n = pool[i];
    if (n.kind == Kind.NUM) { return 1; }
    i32 l = n.a;
    if (n.kind == Kind.NEG) { return 1 + depth(l); }
    return 1 + max(depth(l), depth(n.b));
}

i64 weight(i32 i) {
    Node& n = pool[i];
    if (n.kind == Kind.NUM) { return 1; }
    if (n.b < 0) { return 0; }
    return weight(n.a) + weight(n.b);
}

void visit(i64 d) {
    if (d == 0) { return; }
    visits++;
    visit(d - 1);
    visit(d - 1);
}

class Stairs {
    i64 calls;
    mut i64 ways(i64 n) {
        if (n < 3) { return n == 0 ? 1 : n; }
        calls++;
        return ways(n - 1) + ways(n - 2) + ways(n - 3);
    }
}

i64 sumTo(i64 n) {
    if (n == 0) { return 0; }
    return n + sumTo(n - 1);
}

<T implements int> T fibT(T n) {
    if (n < 2) { return n; }
    return fibT(n - 1) + fibT(n - 2);
}

void main(string[.] args) {
    pool +> new Node(Kind.NUM, 5, 0);
    pool +> new Node(Kind.NUM, 7, 0);
    pool +> new Node(Kind.ADD, 0, 1);
    pool +> new Node(Kind.NEG, 2, 0);
    pool +> new Node(Kind.ADD, 3, 0);
    if (args.len > 0) {
        pool +> new Node(Kind.ADD, 0, 99);
        println("${eval(5)}");
    }
    visit(10);
    Stairs s = new Stairs(0);
    println("${fib(20)} ${eval(4)} ${depth(4)} ${weight(4)} ${visits} ${s.ways(10)} ${s.calls} ${sumTo(100)} ${fibT((i64) 30)} ${fibT((u8) 13)}");
}
"##;
    let c = c_of(text);
    let weight_copy: String = c
        .split("_weight_base(int32_t p_i_0) {")
        .nth(1)
        .and_then(|b| b.split("\n}\n").next())
        .expect("weight's copy")
        .to_string();
    assert_eq!(weight_copy.matches("if (").count(), 1, "{}", weight_copy);
    let copies: Vec<&str> = c
        .lines()
        .filter(|l| l.contains("_base(") && l.ends_with(" {"))
        .collect();
    // fib, eval, depth, weight, visit, ways, and fibT for i64 and u8.
    assert_eq!(copies.len(), 8, "{:#?}", copies);
    assert!(!c.contains("sumTo_base"));
    prints(text, "6765 -7 4 4 1023 274 96 5050 832040 233\n");
    let r = run(text, &["bad"]);
    assert_eq!(r.stdout, "");
    assert!(
        r.stderr.contains("prog.zn:18: index 99 is out of range for length 6"),
        "{}",
        r.stderr
    );
    assert_eq!(r.code, 1);
}

/// An array of self-relative links holds offsets, each from its own slot (REFERENCES
/// §7.2): appended to by `+>` and its chains, read by index and by `for`, an element
/// rebound or written through, nullable ones, `clear`, and a constructor given
/// references. A full array fails as any capped one does, and a 1-byte link whose
/// target is too far away stops the program.
#[test]
fn arrays_of_links_hold_offsets() {
    let text = r##"class N {
    i32 v;
    N&<i32>[..~3] kids;
    N&<i32>?[..~2] opt;
    N&<i8>[..~2] near;
}

N[..>] pool = [];

i64 sum(N& n) {
    i64 t = n.v;
    for (N& k : n.kids) { t += sum(k); }
    return t;
}

void main(string[.] args) {
    N& root = pool.push(new N(1, [], [], []));
    N& a = pool.push(new N(2, [], [], []));
    N& b = pool.push(new N(3, [], [], []));
    root.kids +> a +> b;
    a.kids +> b;
    root.opt +> null;
    root.opt +> a;
    println("${root.kids.len} ${sum(root)} ${root.kids[1].v} ${a.kids[0].v}");
    root.kids[0] .= b;
    root.kids[1].v = 30;
    println("${sum(root)} ${b.v}");
    i64 nulls = 0;
    for (N&? o : root.opt) {
        if (o == null) { nulls++; } else { print("${o.v} "); }
    }
    println("${nulls}");
    root.kids.clear();
    println("${root.kids.len}");
    N& c = pool.push(new N(4, [a, b], [], []));
    println("${sum(c)}");
    if (args.len > 0 && args[0] == "full") {
        root.kids +> a +> a +> a +> a;
    }
    if (args.len > 0 && args[0] == "far") {
        for (i64 i = 0; i < 10; i++) { pool +> new N(0, [], [], []); }
        N& z = pool[pool.len - 1];
        root.near +> z;
    }
}
"##;
    let c = c_of(text);
    assert!(c.contains("int32_t e[3];"), "the kids are 4-byte offsets");
    assert!(c.contains("int8_t e[2];"), "the near ones are 1-byte offsets");
    let out = "2 9 3 3\n61 30\n2 1\n0\n66\n";
    prints(text, out);
    let exe = build(text);
    for (mode, err) in [
        ("full", "prog.zn:38: `+>` failed with FULL, and nothing read its status"),
        ("far", "prog.zn:43: this link's target is too far away for its 1-byte offset"),
    ] {
        let r = exec(&exe, &[mode]);
        assert_eq!(r.stdout, out, "{}", mode);
        assert!(r.stderr.contains(err), "{}: {}", mode, r.stderr);
        assert_eq!(r.code, 1, "{}", mode);
    }
}

/// A `packed` class is laid out with no padding (DESIGN §6.1): an enum and two `i32`s
/// take 9 bytes, not 12. It has methods, implements interfaces, is referred to whole,
/// holds other `packed` values, slices and references, and its fields are read,
/// written and copied as any class's are.
#[test]
fn packed_classes_have_no_padding() {
    let text = r##"enum Kind { NUM, ADD, NEG }

interface Valued { i64 value(); }

packed Node implements Valued {
    Kind kind;
    i32 a;
    i32 b;
    i64 value() { return a + b; }
    mut void bump() { a += 1; }
}

public packed Tagged {
    u8 tag;
    Node node;
    string label;
    Tagged&? next;
}

class Plain { u8 tag; i32 v; }

Node[..>] nodes = [];
Tagged[..>] tags = [];

void bumpRef(i32& x) { x += 10; }

void main() {
    i32 packed = 3;
    for (i32 i = 0; i < 5; i++) { nodes +> new Node(Kind.ADD, i, i * packed); }
    nodes[1].a += 100;
    nodes[2].bump();
    Node& n = nodes[3];
    n.b = -1;
    i64 t = 0;
    for (Node& x : nodes) { t += x.value(); }
    Valued& v = nodes[4];
    Tagged& first = tags.push(new Tagged(1, nodes[0], "first", null));
    tags +> new Tagged(2, nodes[1], "second", first);
    Tagged&? nx = tags[1].next;
    Node& inner = tags[1].node;
    i32 copied = nodes[0].a;
    bumpRef(copied);
    Plain p = new Plain(1, 2);
    if (nx != null) {
        println("${t} ${v.value()} ${nx.label} ${tags[1].label.len} ${inner.a} ${copied} ${p.v}");
    }
}
"##;
    let c = c_of(text);
    let line = |name: &str| {
        c.lines()
            .find(|l| l.starts_with("struct ") && l.contains(&format!("_{} {{", name)))
            .unwrap_or_else(|| panic!("no struct for {}", name))
            .to_string()
    };
    assert!(line("Node").starts_with("struct __attribute__((packed)) "), "{}", line("Node"));
    assert!(line("Tagged").starts_with("struct __attribute__((packed)) "), "{}", line("Tagged"));
    assert!(!line("Plain").contains("packed"), "{}", line("Plain"));
    // A packed value's alignment is 1, so it goes after the pointer.
    assert!(line("Tagged").contains("f_next; zc"), "{}", line("Tagged"));
    prints(text, "131 16 first 6 101 10 2\n");
}

/// A value with links is built in its slot, but its constructor's arguments are
/// evaluated first, in order, as for any call: one that reads the array's length sees
/// it before the value is added, and one that pushes into the array takes the slot
/// before it. The slot used to be taken first, so `ls.len` read one more.
#[test]
fn a_value_with_links_is_built_after_its_arguments() {
    prints(
        r##"class L {
    i64 n;
    L&<i32>? next;
}
L[..>] ls = [];
void main() {
    L& a = ls.push(new L(ls.len, null));
    ls +> new L(ls.len, a);
    ls +> new L(ls.len, ls.push(new L(100, null)));
    L&? nx = ls[3].next;
    if (nx != null) { println("${ls[0].n} ${ls[1].n} ${ls[2].n} ${ls[3].n} ${nx.n}"); }
}
"##,
        "0 1 100 2 100\n",
    );
}

/// A `simd` function is compiled for AVX2 and AVX-512 too, and runs the version for the
/// widest vector unit the CPU has (DESIGN §12.8). Every version gives the baseline's
/// results: a multiply and an add round twice, where one FMA would round once, so
/// `0.1 * 10.0 - 1.0` is 0, not 5.55e-17. A failure inside one is reported as anywhere.
#[test]
fn simd_functions_give_the_baselines_results() {
    let text = r##"simd void axpy(f64 a, f64[:] x, f64[:]& y) {
    for (i64 i = 0; i < y.len; i++) { y[i] = a * x[i] + y[i]; }
}

simd i64 sumAt(i64[:] v, i64 at) { return v[at] + v[at + 1]; }

simd <T implements number> T total(T[:] v) {
    T t = 0;
    for (T x : v) { t += x; }
    return t;
}

class Acc {
    i64 n;
    simd mut void add(i64[:] v) {
        for (i64 i = 0; i < v.len; i++) { n += v[i] * v[i]; }
    }
}

void main(string[.] args) {
    f64[..>] x = [];
    f64[..>] y = [];
    for (i64 i = 0; i < 100; i++) {
        x +> 0.1;
        y +> -1.0;
    }
    axpy(10.0, x, y);
    f64 worst = 0.0;
    for (f64 v : y) { worst = max(worst, abs(v)); }
    i64[..>] v = [];
    for (i64 i = 1; i <= 10; i++) { v +> i; }
    Acc acc = new Acc(0);
    acc.add(v);
    println("${worst} ${total(v)} ${total(x)} ${acc.n} ${sumAt(v, 3)}");
    if (args.len > 0) { println("${sumAt(v, 9)}"); }
}
"##;
    let c = c_of(text);
    for name in ["axpy", "sumAt", "add"] {
        assert!(c.contains(&format!("_{}_simd2(", name)), "{} has no AVX-512 version", name);
    }
    let exe = build(text);
    let r = exec(&exe, &[]);
    assert_eq!(r.stdout, "0.0 55 9.99999999999998 385 9\n", "stderr: {}", r.stderr);
    let r = exec(&exe, &["out"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("prog.zn:5: index 10 is out of range for length 10"), "{}", r.stderr);
}

/// `+`, `-`, `*`, unary `-`, `++` and `--` wrap at every width; `+#`, `-#` and `*#`, and
/// their compound forms, fail with RANGE; division by zero fails with DOMAIN and
/// `MIN / -1` wraps (DESIGN §9.2). `sum()` and `abs` still stop on overflow.
#[test]
fn arithmetic_wraps_and_the_hash_forms_check() {
    let exe = build(
        r##"void main(string[.] args) {
    i64 big = i64.MAX;
    u8 b = 250;
    u32 z = 0;
    i32 m = i32.MIN;
    i64 minus = -1;
    i64 lo = i64.MIN;
    u128 top = u128.MAX;
    i64 inc = i64.MAX;
    inc++;
    u8 dec = 0;
    dec--;
    i64 acc = i64.MAX;
    acc += 2;
    println("${big + 1 == i64.MIN} ${b + 10} ${z - 1} ${-m} ${m * 2} ${lo / minus == lo} ${lo % minus} ${top + 1} ${inc == i64.MIN} ${dec} ${acc}");
    i64 a @e1 = big +# 1;
    u8 c @e2 = b +# 10;
    u32 d @e3 = z -# 1;
    i32 f @e4 = m *# 2;
    i64 g @e5 = 3 +# 4;
    i64 h = 10;
    u8 st = @(h +#= big);
    i64 q @e6 = 10 / (minus + 1);
    println("${a} ${e1} ${c} ${e2} ${d} ${e3} ${f} ${e4} ${g} ${e5} ${st} ${h} ${q} ${e6}");
    if (args.len > 0 && args[0] == "sum") {
        i64[..>] xs = [];
        xs +> i64.MAX +> 1;
        println("${xs.sum()}");
    }
    if (args.len > 0 && args[0] == "abs") { println("${abs(lo)}"); }
    if (args.len > 0 && args[0] == "add") { println("${big +# 1}"); }
}
"##,
    );
    let r = exec(&exe, &[]);
    assert_eq!(
        r.stdout,
        "true 4 4294967295 -2147483648 0 true 0 0 true 255 -9223372036854775807\n0 34 0 34 0 34 0 34 7 0 34 10 0 33\n",
        "stderr: {}",
        r.stderr
    );
    for (arg, line) in [("sum", "prog.zn:28: integer overflow"), ("abs", "prog.zn:30: integer overflow"), ("add", "prog.zn:31: integer overflow")] {
        let r = exec(&exe, &[arg]);
        assert_eq!(r.code, 1, "{}", arg);
        assert!(r.stderr.contains(line), "{}: {}", arg, r.stderr);
    }
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
        ("i64 x = i64.MAX; x = x +# 1;", "prog.zn:3: integer overflow"),
        (
            "i64[..>] a = []; a +> 1; i64 y = a[5];",
            "prog.zn:3: index 5 is out of range for length 1",
        ),
        ("i64 z = 0; i64 q = 10 / z;", "prog.zn:3: division by zero"),
        (
            "i64 n = 300; u8 b = (u8) n;",
            "prog.zn:3: the value doesn't fit in u8",
        ),
        (
            "i64[..~] e = []; i64 v = e.pop();",
            "prog.zn:3: `pop` failed with EMPTY, and nothing read its status",
        ),
        (
            "i128 m = i128.MAX; m = m +# 1;",
            "prog.zn:3: integer overflow",
        ),
        (
            "u128 a = u128.MAX; u128 b = a *# 2;",
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
        "interface Op { i64 run(func(i64 => i64) f, i64 x); }\nclass P implements Op { new() {} i64 run(func(i64 => i64) f, i64 x) { return f(x); } }\nvoid main() { i64 k = 3; P p = new P(); Op& o = p; println(\"${o.run((i64 x) => x * k, 2)}\"); }\n",
    );
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    assert!(c.is_none());
    assert!(diags[0].iter().any(|d| {
        d.message.contains(
            "the C backend doesn't support a lambda that captures, passed to a method called through an interface or to a constructor yet",
        )
    }));
}

/// A lambda that captures, passed to a function of the program's own (DESIGN §9.3): a
/// copy of the function per lambda, inlining it, for locals, a `string`, an array, a
/// class value and a reference; called twice on its own result; a block body; a generic
/// function and a generic class's method; passed on to a built-in, to another function,
/// and to itself recursively; a new lambda around it; `filter` with it in a function
/// returning an array; a failing lambda whose status the copy reads; and `this` both
/// where the lambda is written and in the function it's passed to.
#[test]
fn lambdas_that_capture_get_a_copy_of_the_function() {
    prints(
        r#"class Box<T> {
    T v;
    new(T v) { this.v = v; }
    T mapped(func(T => T) f) { return f(v); }
}
class Pt { i64 x; i64 y; new(i64 x, i64 y) { this.x = x; this.y = y; } }

i64 twice(i64 x, func(i64 => i64) f) { return f(f(x)); }
<T> T apply(T x, func(T => T) f) { return f(x); }
i64 countIn(i64[:] xs, func(i64 => bool) keep) { return xs.count(keep); }
i64 countVia(i64[:] xs, func(i64 => bool) keep) { return countIn(xs, keep) + countIn(xs, (i64 x) => !keep(x)); }
i64 sumTo(i64 n, func(i64 => i64) f) { if (n == 0) { return 0; } return f(n) + sumTo(n - 1, f); }
i64[..>] keepAll(i64[:] xs, func(i64 => bool) p) { return xs.filter(p); }
i64 checked(i64 x, func(i64 => i64) f) {
    i64 v @st = f(x);
    if (st != 0) { return -1; }
    return v;
}

class Scaler {
    i64 k;
    new(i64 k) { this.k = k; }
    i64 scale(i64 x) { return x * k; }
    i64 run(Box<i64>& b, i64 add) { return b.mapped((i64 v) => scale(v) + add + k); }
}

void main() {
    i64 a = 3;
    string name = "zink";
    i64[..>] xs = [1, 2, 3, 4, 5, 6];
    Pt p = new Pt(10, 20);
    Pt& pr = p;
    println("${twice(1, (i64 x) => x + a)} ${apply("x", (string s) => name)} ${apply(5, (i64 x) => x * xs.len)}");
    println("${apply(1, (i64 x) => x + p.x + pr.y)} ${twice(2, (i64 x) => { if (x > a) { return x * 10; } return x + a; })}");
    println("count ${countIn(xs, (i64 x) => x > a)} via ${countVia(xs, (i64 x) => x > a)} sum ${sumTo(4, (i64 x) => x * a)}");
    println("keep ${keepAll(xs, (i64 x) => x % a == 0)}");
    println("checked ${checked(4, (i64 x) => { if (x > a) { return #status.INVALID 0; } return x; })} ${checked(2, (i64 x) => { if (x > a) { return #status.INVALID 0; } return x; })}");
    Box<i64> b = new Box<i64>(7);
    Box<string> bs = new Box<string>("q");
    println("box ${b.mapped((i64 v) => v + a)} ${bs.mapped((string s) => name)}");
    Scaler sc = new Scaler(2);
    println("scaler ${sc.run(b, a)}");
}
"#,
        "7 zink 30\n31 50\ncount 3 via 6 sum 30\nkeep [3, 6]\nchecked -1 2\nbox 10 zink\nscaler 19\n",
    );
    // A status the copy doesn't read stops the program at the call, as through a pointer.
    let r = run(
        "i64 call(i64 x, func(i64 => i64) f) { return f(x); }\nvoid main() {\n    i64 lim = 3;\n    println(\"${call(2, (i64 x) => { if (x > lim) { return #status.INVALID 0; } return x; })}\");\n    println(\"${call(5, (i64 x) => { if (x > lim) { return #status.INVALID 0; } return x; })}\");\n}\n",
        &[],
    );
    assert_eq!(r.stdout, "2\n");
    assert!(r.stderr.contains("prog.zn:1: `f` failed with INVALID"), "{}", r.stderr);
    // Recursion that wraps the lambda in a new one at every call would need copies
    // without end.
    let file = SourceFile::new(
        "prog.zn",
        "i64 nest(i64 n, func(i64 => i64) f) {\n    if (n == 0) { return f(0); }\n    return nest(n - 1, (i64 y) => f(y) + 1);\n}\nvoid main() { i64 k = 1; println(\"${nest(3, (i64 x) => x + k)}\"); }\n",
    );
    let (diags, c) = zink::compile_c(std::slice::from_ref(&file));
    assert!(c.is_none());
    assert!(diags[0].iter().any(|d| d.message.contains("recursion that wraps a lambda in a new one at every call")));
}

/// `filter`, `map` and `sortedIndexes` (STDLIB §7.1): built where the result goes, a new
/// variable's storage, an existing one's, a function's result or a temporary; on plain,
/// fixed and ring arrays; of classes, `string`s and floats; a lambda that captures, a
/// nested `filter`, and an empty result.
#[test]
fn filter_map_and_sorted_indexes() {
    prints(
        r#"class Dog { string name; i64 age; new(string name, i64 age) { this.name = name; this.age = age; } }

i64[..>] evens(i64[:] xs) { return xs.filter((i64 x) => x % 2 == 0); }

void main() {
    i64[..>] a = [5, 2, 8, 1, 9, 4];
    i64 minAge = 3;
    println("${a.filter((i64 x) => x % 2 == 0)} ${a.map((i64 x) => x * 10)}");
    i64[..>] big = a.filter((i64 x) => x > minAge);
    big +> 100;
    println("big ${big} evens ${evens(a)} len ${a.filter((i64 x) => x > 4).len}");
    big = big.filter((i64 x) => x != 100);
    println("big again ${big}");
    Dog[..>] dogs = [];
    dogs +> new Dog("rex", 5);
    dogs +> new Dog("ada", 2);
    dogs +> new Dog("bo", 7);
    var adults = dogs.filter((Dog d) => d.age >= minAge);
    string[..>] names = dogs.map((Dog d) => d.name);
    f64[..>] halves = a.map((i64 x) => (f64) x / 2.0);
    println("adults ${adults.len} ${adults[0].name} ${adults[1].name}; names ${names}; halves ${halves}");
    i64[..>] order = dogs.sortedIndexesBy((Dog d) => d.age);
    i64[..>] byName = dogs.sortedIndexes((Dog x, Dog y) => x.name < y.name);
    println("by age ${order} by name ${byName} sorted ${a.sortedIndexes()} a unchanged ${a}");
    i64[.4] fixed = [3, 1, 2, 0];
    i64[~.~] ring = [];
    ring +> 7; ring +> 3; ring.pushFront(9);
    println("fixed ${fixed.map((i64 x) => x + 1)} ${fixed.sortedIndexes()} ring ${ring.filter((i64 x) => x > 3)} ${ring.sortedIndexes()}");
    i64 total = 0;
    for (i64 x : a.map((i64 x) => x * x)) { total += x; }
    println("total ${total} nested ${a.filter((i64 x) => a.filter((i64 y) => y < x).len >= 2)}");
    i64[..>] none = a.filter((i64 x) => x > 1000);
    println("none ${none.len} ${none} text ${a.map((i64 x) => "<${x}>")}");
}
"#,
        "[2, 8, 4] [50, 20, 80, 10, 90, 40]
big [5, 8, 9, 4, 100] evens [2, 8, 4] len 3
big again [5, 8, 9, 4]
adults 2 rex bo; names [rex, ada, bo]; halves [2.5, 1.0, 4.0, 0.5, 4.5, 2.0]
by age [1, 0, 2] by name [1, 2, 0] sorted [3, 1, 5, 0, 2, 4] a unchanged [5, 2, 8, 1, 9, 4]
fixed [4, 2, 3, 1] [3, 1, 2, 0] ring [9, 7] [2, 1, 0]
total 191 nested [5, 8, 9, 4]
none 0 [] text [<5>, <2>, <8>, <1>, <9>, <4>]
",
    );
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
    seed = seed * 6364136223846793005 + 1442695040888963407;
    return (i64) ((seed >> 33) % (u64) n);
}

u8 main() {
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
            if (got != model[i]) { println("wrong element at step ${step}"); return 1; }
            for (i64 k = i; k + 1 < model.len; k++) { model[k] = model[k + 1]; }
            @_ = model ->;
        }
        if (step % 1000 == 0) {
            if (chunked.len != model.len) { println("length ${chunked.len} against ${model.len}"); return 1; }
            for (i64 k = 0; k < model.len; k++) {
                if (chunked[k] != model[k]) { println("mismatch at ${k}, step ${step}"); return 1; }
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
    seed = seed * 6364136223846793005 + 1442695040888963407;
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
    println("${one << 127} ${(one << 127) >> 120} ${i128.MAX + 1 == i128.MIN} ${u128.MAX +| 1 == u128.MAX} ${neg >> 126}");

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
    seed = seed * 6364136223846793005 + 1442695040888963407;
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
    println("dir ${sd.isDir} ${sd.isFile}, file ${sf.size} ${sf.isFile} ${sf.modified.nanos > 1600000000000000000}");
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

u8 main(string[.] args) {
    Args cli = new Args("tool", "Does things to files.");
    cli.version = "1.2";
    cli.flag('v', "verbose", "say more; twice for even more");
    cli.flag(0, "dry-run", "change nothing");
    cli.option('o', "out", "FILE", "write here");
    cli.option(0, "scale", "X", "a factor");
    cli.positional("mode", "what to do");
    cli.rest("files", "the files");
    u8 code @stop = cli.parseOrStop(args);
    if (stop) { return code; }
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
    seed = seed * 6364136223846793005 + 1442695040888963407;
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

u8 main(string[.] args) {
    Countdown c = new Countdown(4);
    println("sum ${sum(c)}");
    Args cli = new Args("t", "");
    cli.rest("files", "the files");
    u8 code @stop = cli.parseOrStop(args);
    if (stop) { return code; }
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

/// Constructors of a record's own: computing its fields, leaving one to its initializer,
/// delegating with `this.new`, and failing, which leaves the zero record. Records returned
/// by functions and through an interface, built where they're kept (DESIGN §3.3): into a
/// variable, an array, another record and a parameter, through two levels of `return`, and
/// from a parameter's field.
#[test]
fn record_constructors_and_returns() {
    prints(
        r##"interface Maker { Item make(i32 n); }

class Item {
    u8[.*] sku;
    i32 qty;
    u8[.*] note = "none";
    u8[.*] origin;

    // Computes its fields; leaves `note` to its initializer.
    new(string line) {
        (string code, string rest) = line.splitFirst(":");
        i32 n @_ = i32.parse(rest.trim());
        qty = n;
        this.sku = code.trim().toUpper();
        origin = "parsed";
    }

    new(string sku, i32 qty) {
        this.new("${sku}:${qty}");
    }

    new(string sku, i32 qty, string note) {
        this.sku = sku;
        this.qty = qty;
        this.note = note;
        if (qty < 0) { return #status.INVALID; }
        this.origin = "checked";
    }
}

class Order { i32 id; Item first; Item[.*] rest; }

Item make(string sku) { return new Item(sku, 3); }

Item built(string sku) {
    Item it = new Item(sku, 4, "built");
    it.qty += 1;
    return it;
}

Item pickFirst(Order o) { return o.first; }

Item twice(string sku) { return make(sku); }

class Factory implements Maker {
    Item make(i32 n) { return new Item("F${n}", n, "factory"); }
}

void show(Item it) { print("[${it.sku} ${it.qty} ${it.note} ${it.origin}]"); }

void main() {
    Item a = new Item(" abc : 7 ");
    show(a);
    Item b = new Item("xyz", 2);
    show(b);
    Item bad @err = new Item("neg", -1, "oops");
    show(bad);
    println(" ${err}");

    Item m = make("m1");
    Item k = built("k1");
    Item t = twice("t1");
    show(m); show(k); show(t);
    println("");

    Item[..>] items = [];
    items +> make("in-array");
    items +> built("in-array-too");
    for (Item& it : items) { show(it); }
    println("");

    Order o = new Order(1, make("first"), [built("r1"), make("r2")]);
    show(o.first);
    for (Item& it : o.rest) { show(it); }
    show(pickFirst(o));
    println("");

    Factory f = new Factory();
    Maker& mk = f;
    Item fi = mk.make(9);
    show(fi);
    show(mk.make(10));
    println(" ${make("len").sku.len}");
}
"##,
        "[ABC 7 none parsed][XYZ 2 none parsed][ 0  ] 22
[M1 3 none parsed][k1 5 built checked][T1 3 none parsed]
[IN-ARRAY 3 none parsed][in-array-too 5 built checked]
[FIRST 3 none parsed][r1 5 built checked][R2 3 none parsed][FIRST 3 none parsed]
[F9 9 factory checked][F10 10 factory checked] 3
",
    );
    // A recursive function passes where to build the record down to the bottom.
    prints(
        r##"class Item { u8[.*] sku; i32 qty; }


Item chain(i32 n) {
    if (n == 0) { return new Item("bottom", 0); }
    return chain(n - 1);
}

void main() {

    Item c = chain(5);
    println(c.sku);
}
"##,
        "bottom\n",
    );
}

// ---------------------------------------------------------------- the rest of the v1 library

/// zink.time (STDLIB §5.3): Duration's units and human text, Timestamp to calendar fields
/// and RFC 3339, parsing with offsets, date-only and time-only forms and a leap second,
/// INVALID for a bad date, weekdays, a negative timestamp, sleeping and the clocks. The
/// calendar code agrees with Python's datetime over 5,000 random instants.
#[test]
fn zink_time() {
    prints(
        r##"import zink.time.*;

void main() {
    println("${Duration.ns(850).human()} ${Duration.us(1500).human()} ${Duration.ms(350).human()} ${Duration.ms(1250).human()} ${Duration.seconds(3).human()} ${Duration.seconds(427).human()} ${Duration.minutes(123).human()} ${Duration.ms(-1500).human()}");
    Timestamp t = Timestamp.fromUnix(1795512600);
    println("${t.iso()} ${t.unixSeconds()}");
    DateTime d = t.utc();
    println("${d.year}-${d.month}-${d.day} ${d.weekday()}");
    DateTime p @e1 = DateTime.parse("2026-11-18T09:30:00.25+01:30");
    Timestamp pt @e2 = p.toTimestamp();
    println("${e1} ${p.iso()} ${pt.iso()} ${e2}");
    DateTime dateOnly @e3 = DateTime.parse("2024-02-29");
    DateTime timeOnly @e4 = DateTime.parse("23:59:60.123");
    DateTime bad @e5 = DateTime.parse("2023-02-29");
    Timestamp none @e6 = timeOnly.toTimestamp();
    println("${dateOnly.iso()} ${e3} ${timeOnly.iso()} ${e4} ${e5} ${e6}");
    Timestamp old = Timestamp.fromUnix(-1);
    println("${old.iso()} ${Timestamp.fromUnix(0).utc().weekday()}");
    Instant a = time.monotonic();
    time.sleep(Duration.ms(20));
    Duration took = a.elapsed();
    println("slept ${took.toMs() >= 20} ${took.toMs() < 500}");
    println("now ${time.now().utc().year >= 2026}");
}
"##,
        "850ns 1.5ms 350ms 1.25s 3s 7m07s 2h03m -1.5s
2026-11-24T09:30:00Z 1795512600
2026-11-24 2
0 2026-11-18T09:30:00.25+01:30 2026-11-18T08:00:00.25Z 0
2024-02-29 0 23:59:60.123 0 22 22
1969-12-31T23:59:59Z 4
slept true true
now true
",
    );
}

/// env (STDLIB §2.3): get, has, set (a view from before keeping the old value), INVALID
/// names, unset and all().
#[test]
fn environment() {
    prints(
        r##"void main() {
    string home @e1 = env.get("HOME");
    string none @e2 = env.get("ZINK_SURELY_UNSET");
    println("${home.len > 0} ${e1} ${e2 == status.MISSING} ${env.has("PATH")}");
    @_ = env.set("ZINK_T", "one");
    string v1 @_ = env.get("ZINK_T");
    @_ = env.set("ZINK_T", "two");
    string v2 @_ = env.get("ZINK_T");
    println("${v1} ${v2} ${@env.set("", "x")} ${@env.set("a=b", "x")}");
    env.unset("ZINK_T");
    i64 n = 0;
    for (EnvVar v : env.all()) { if (v.name == "ZINK_T") { n++; } }
    println("after unset ${n} ${env.has("ZINK_T")}");
}
"##,
        "true 0 true true\none two 22 22\nafter unset 0 false\n",
    );
}

/// zink.os (STDLIB §5.1, §5.2): running a program and capturing its output, an exit code, a
/// signal, NOT_FOUND, a Command with arguments, a working directory, environment changes,
/// input and stderr captured, a timeout, a spawned child killed, which, pid, name,
/// term.size off a terminal, and exec.
#[test]
fn zink_os() {
    let r = run(
        r##"import zink.os.*;
import zink.time.*;

void main(string[.] args) {
    u8[..>] out = [];
    Exit e @err = process.run("echo", ["hello", "two words"], out);
    print("run ${err} ${e.code} ${e.ok()} out=${out}");
    u8[..>] none = [];
    Exit f @_ = process.run("sh", ["-c", "exit 3"], none);
    Exit g @_ = process.run("sh", ["-c", "kill -TERM $$"], none);
    Exit h @nf = process.run("zink-surely-missing", [], none);
    println("exit ${f.code} ${f.ok()}, signal ${g.signal}, missing ${nf == status.NOT_FOUND}");

    Command c = new Command("sh");
    c.arg("-c");
    c.arg("pwd; echo \"$GREETING\"; echo [$UNSET_ME]; tr a-z A-Z; echo err >&2; exit 5");
    c.cwd = "/tmp";
    c.setEnv("GREETING", "hi there");
    c.setEnv("UNSET_ME", "x");
    c.unsetEnv("UNSET_ME");
    c.input = "fed through stdin\n";
    u8[..>] o2 = [];
    u8[..>] e2 = [];
    Exit r2 @_ = c.outputBoth(o2, e2);
    print("command ${r2.code}\n${o2}stderr ${e2}");

    Command slow = new Command("sleep");
    slow.arg("5");
    slow.timeout = Duration.ms(100);
    Instant t0 = time.monotonic();
    Exit s @timeout = slow.run();
    println("timeout ${timeout == status.TIMEOUT} after ${t0.elapsed().toMs() < 2000} signal ${s.signal}");

    Command bg = new Command("sleep");
    bg.arg("30");
    Child ch @_ = bg.spawn();
    @_ = ch.kill();
    Exit w @_ = ch.wait();
    println("spawned and killed: signal ${w.signal}");

    u8[.] sh @_ = process.which("sh");
    u8[.] nope @wn = process.which("zink-surely-missing");
    println("which ${sh.endsWith("/sh")} ${wn == status.NOT_FOUND}, pid ${process.pid() > 0}, name ${process.name().len > 0}");
    (i64 cols, i64 rows) @nt = term.size();
    println("term ${nt == status.NOT_TERMINAL}");
    if (args.len > 0) {
        u8 xe = @process.exec("echo", ["exec replaced the process"]);
        println("exec failed ${xe}");
    }
}
"##,
        &["exec"],
    );
    assert_eq!(
        r.stdout,
        "run 0 0 true out=hello two words
exit 3 false, signal 15, missing true
command 5
/tmp
hi there
[]
FED THROUGH STDIN
stderr err
timeout true after true signal 9
spawned and killed: signal 15
which true true, pid true, name true
term true
exec replaced the process
",
        "{}",
        r.stderr
    );
}

/// zink.random (STDLIB §8.2): xoshiro256** seeded by SplitMix64, nextF64, Lemire's
/// unbiased range for small, unsigned and 128-bit spans, an even spread, and the kernel's
/// random bytes. The numbers are what a Python model of the same algorithms gives.
#[test]
fn zink_random() {
    prints(
        r##"import zink.random.*;

void main() {
    Rng r = random.seeded(42);
    for (i64 i = 0; i < 5; i++) { print("${r.nextU64()} "); }
    println("");
    Rng f = random.seeded(7);
    println("${f.nextF64()} ${f.nextF64()}");
    Rng g = random.seeded(99);
    for (i64 i = 0; i < 6; i++) { print("${g.range(-3, 4)} "); }
    for (i64 i = 0; i < 3; i++) { print("${g.range((u8) 10, (u8) 250)} "); }
    print("${g.range((i128) -100000000000000000000000, (i128) 100000000000000000000000)} ");
    println("");
    i64[..>] counts = [0, 0, 0, 0, 0, 0];
    Rng h = random.seeded(1);
    for (i64 i = 0; i < 600000; i++) { counts[h.range(0, 6)] += 1; }
    bool even = true;
    for (i64 c : counts) { if (c < 99000 || c > 101000) { even = false; } }
    println("even ${even}");
    u8[..>] bytes = [];
    random.secureBytes(bytes, 16);
    Rng os = random.fromOs();
    println("secure ${bytes.len} ${random.secureU64() != random.secureU64()} ${os.chance(1.0)} ${os.chance(0.0)}");
}
"##,
        "1546998764402558742 6990951692964543102 12544586762248559009 17057574109182124193 18295552978065317476 
0.7005764821796896 0.2787512294737843
-1 0 -1 2 2 -2 76 24 213 31889431257593731978611 
even true
secure 16 true true false
",
    );
}

/// zink.hash, zink.encoding, ansi, math.roundToI64 and the bit methods (STDLIB §6.2, §8):
/// the values are Python's hashlib, zlib and base64 ones.
#[test]
fn hashing_encoding_colors_and_bits() {
    prints(
        r##"import zink.hash.*;
import zink.encoding.*;
import zink.cli.*;

void main() {
    string t = "The quick brown fox jumps over the lazy dog";
    println("${hash.fnv1a(t)} ${hash.crc32(t)} ${hash.sha2_256(t).hex()}");
    Sha2_256 h = new Sha2_256();
    h.update("The quick brown ");
    h.update("fox jumps over the lazy dog");
    Crc32 c = new Crc32();
    c.update("The quick brown ");
    c.update("fox jumps over the lazy dog");
    println("${h.finish().hex()} ${c.finish()}");
    u8[..>] raw = [];
    raw +> 0xfb;
    raw +> 0xff;
    raw +> 0xfe;
    println("${t.hex()} ${base64.encode(t)} ${base64.encodeUrl(raw)} ${base64.encode(raw)}");
    u8[..>] back = [];
    @_ = base64.decodeUrl("-__-", back);
    u8[..>] hb = [];
    @_ = hex.decode("DEADbeef", hb);
    println("${back == raw} ${hb.hex()} ${@hex.decode("abc", hb)} ${@base64.decode("QR==", hb)}");
    Colors piped = ansi.colors(stdout);
    Colors on = ansi.always();
    println("[${piped.red}plain${piped.reset}] [${on.green}${on.bold}x${on.reset}]");
    i64 r1 @e1 = math.roundToI64(2.5);
    i64 r2 @e2 = math.roundToI64(0.0 / 0.0);
    u64 bits = 0x8000000000000001;
    println("${r1} ${e1} ${r2} ${e2} ${bits.rotateLeft(1)} ${bits.leadingZeros()} ${((u16) 0x1234).reverseBytes()} ${(64).isPowerOf2()} ${(-64).isPowerOf2()}");
}
"##,
        "17580284887202820368 1095738169 d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592
d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592 1095738169
54686520717569636b2062726f776e20666f78206a756d7073206f76657220746865206c617a7920646f67 VGhlIHF1aWNrIGJyb3duIGZveCBqdW1wcyBvdmVyIHRoZSBsYXp5IGRvZw== -__- +//+
true deadbeef 22 22
[plain] [\u{1b}[32m\u{1b}[1mx\u{1b}[0m]
3 0 0 34 3 0 13330 true false
",
    );
}

/// zink.csv (STDLIB §9.1): quoted separators, doubled quotes, a blank line, spaces kept
/// and quoted on the way out, a quoted line break, \r\n, and where a bad file fails. 2,000
/// random rows round-trip through Python's csv module and back.
#[test]
fn zink_csv() {
    let exe = build(
        r##"import zink.csv.*;
import zink.data.*;

void main() {
    u8[.] text @_ = stdin.readAll();
    CsvReader r = new CsvReader(text);
    u8[..>] strings = [];
    string[..~] fields = [];
    u8[..>] out = [];
    CsvWriter w = new CsvWriter(out);
    while (true) {
        u8 st = @r.next(fields, strings);
        if (st == status.END) { break; }
        if (st != 0) {
            ParseError e = r.error();
            println("error ${e.code} line ${e.line} column ${e.column}: ${e.message}");
            return;
        }
        w.writeRow(fields);
    }
    print(out);
}
"##,
    );
    let feed = |input: &str| {
        use std::io::Write;
        let mut child = Command::new(&exe)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
    };
    assert_eq!(
        feed("name,note\n\"a, b\",\"say \"\"hi\"\"\"\n\nplain, spaced \n\"multi\nline\",x\r\nlast"),
        "name,note\n\"a, b\",\"say \"\"hi\"\"\"\n\nplain,\" spaced \"\n\"multi\nline\",x\nlast\n"
    );
    assert_eq!(
        feed("a,\"b\nc"),
        "error 22 line 1 column 3: this quote is never closed\n"
    );
    assert_eq!(
        feed("a,\"b\"c,d\n"),
        "error 22 line 1 column 6: text after a closing quote\n"
    );
}

/// fs.walk (STDLIB §4.3): depth-first from the root, skip(), links not followed, and
/// followed with a loop back to an open directory reported as LOOP.
#[test]
fn walking_a_tree() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-walk");
    let _ = std::fs::remove_dir_all(&dir);
    let t = dir.join("tree");
    std::fs::create_dir_all(t.join("a/b")).unwrap();
    std::fs::create_dir_all(t.join(".git/objects")).unwrap();
    std::fs::create_dir_all(t.join("c")).unwrap();
    std::fs::write(t.join("a/one.zn"), "1").unwrap();
    std::fs::write(t.join("a/b/two.zn"), "22").unwrap();
    std::fs::write(t.join(".git/objects/obj"), "x").unwrap();
    std::fs::write(t.join("c/three.txt"), "3").unwrap();
    std::os::unix::fs::symlink("../a", t.join("c/link")).unwrap();
    std::os::unix::fs::symlink("..", t.join("a/b/up")).unwrap();
    let exe = build(
        r##"import zink.io.*;

void main(string[.] args) {
    u8[..~] buf = [];
    Walker w = fs.walk(args[0], buf);
    w.followLinks = args.len > 1;
    u8[..>] lines = [];
    while (WalkEntry e : w.next()) {
        if (e.isDir && e.name == ".git") { w.skip(); continue; }
        lines.append("${e.depth} ${e.name} ${e.isDir} ${e.isLink} ${e.err}\n");
    }
    print(lines);
}
"##,
    );
    let sorted = |args: &[&str]| {
        let mut v: Vec<String> = exec(&exe, args).stdout.lines().map(String::from).collect();
        v.sort();
        v
    };
    let root = t.to_str().unwrap();
    assert_eq!(
        sorted(&[root]),
        [
            "0 tree true false 0",
            "1 a true false 0",
            "1 c true false 0",
            "2 b true false 0",
            "2 link false true 0",
            "2 one.zn false false 0",
            "2 three.txt false false 0",
            "3 two.zn false false 0",
            "3 up false true 0",
        ]
    );
    assert_eq!(
        sorted(&[root, "follow"]),
        [
            "0 tree true false 0",
            "1 a true false 0",
            "1 c true false 0",
            "2 b true false 0",
            "2 link true true 0",
            "2 one.zn false false 0",
            "2 three.txt false false 0",
            "3 b true false 0",
            "3 one.zn false false 0",
            "3 two.zn false false 0",
            "3 up true true 40",
            "4 two.zn false false 0",
            "4 up true true 40",
        ]
    );
}

/// A File's writes are buffered: position() counts what's buffered, close() writes it,
/// and a program that stops as a bug with the file open still writes it out.
#[test]
fn file_writes_are_buffered() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zink-wbuf");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let exe = build(
        r##"import zink.io.*;

void main(string[.] args) {
    File f @_ = File.create(args[0]);
    for (i64 i = 0; i < 100000; i++) { @_ = f.println("line ${i}"); }
    i64 at @_ = f.position();
    @_ = f.write("tail");
    if (args.len > 1) {
        // A status nothing reads stops the program, with the file still open.
        i64[..~] none = [];
        i64 x = none.pop();
    }
    @_ = f.close();
    println("position ${at}");
}
"##,
    );
    let a = dir.join("a.txt");
    let r = exec(&exe, &[a.to_str().unwrap()]);
    assert_eq!(r.stdout, "position 1088890\n");
    let text = std::fs::read_to_string(&a).unwrap();
    assert_eq!(text.matches('\n').count(), 100000);
    assert!(text.ends_with("line 99999\ntail"));
    let b = dir.join("b.txt");
    let r = exec(&exe, &[b.to_str().unwrap(), "stop"]);
    assert_eq!(r.code, 1);
    assert!(
        std::fs::read_to_string(&b)
            .unwrap()
            .ends_with("line 99999\ntail")
    );
}

/// SHA-2 and SHA-3 (STDLIB §8.3): all twelve on nothing, "abc" and 1,000 bytes, SHAKE
/// past one block of output, and in 37-byte pieces the same as all at once. The values
/// are Python's hashlib; random data of every padding and block boundary agrees too.
#[test]
fn sha2_and_sha3() {
    prints(
        r##"import zink.hash.*;

void all(string t) {
    println("${hash.sha2_224(t).hex()} ${hash.sha2_256(t).hex()}");
    println("${hash.sha2_384(t).hex()} ${hash.sha2_512(t).hex()}");
    println("${hash.sha2_512_224(t).hex()} ${hash.sha2_512_256(t).hex()}");
    println("${hash.sha3_224(t).hex()} ${hash.sha3_256(t).hex()}");
    println("${hash.sha3_384(t).hex()} ${hash.sha3_512(t).hex()}");
    u8[.] a = hash.shake_128(t, 200);
    u8[.] b = hash.shake_256(t, 5);
    println("${a.hex()} ${b.hex()}");
}

void main() {
    u8[..>] m = [];
    for (i64 i = 0; i < 1000; i++) {
        m +> (u8) (i * 7 % 251);
    }
    all("");
    all("abc");
    all(m);
    // In pieces, the same as all at once; finish starts the object over.
    Sha2_512 h = new Sha2_512();
    Sha3_384 k = new Sha3_384();
    Shake_128 x = new Shake_128();
    for (i64 i = 0; i < 1000; i += 37) {
        i64 j = i + 37 < 1000 ? i + 37 : 1000;
        h.update(m[i:j]);
        k.update(m[i:j]);
        x.update(m[i:j]);
    }
    u8[.] xs = x.finish(200);
    u8[.] xo = hash.shake_128(m, 200);
    println("${h.finish() == hash.sha2_512(m)} ${k.finish() == hash.sha3_384(m)} ${xs == xo}");
    println("${h.finish() == hash.sha2_512("")} ${x.finish(0).len}");
}
"##,
        "d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e
6ed0dd02806fa89e25de060c19d3ac86cabb87d6a0ddd05c333b84f4 c672b8d1ef56ed28ab87c3622c5114069bdd3ad7b8f9737498d0c01ecef0967a
6b4e03423667dbb73b6e15454f0eb1abd4597f9a1b078e3f5b5a6bc7 a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a
0c63a75b845e4f7d01107d852e4c2485c51a50aaaa94fc61995e71bbee983a2ac3713831264adb47fb6bd1e058d5f004 a69f73cca23a9ac5c8b567dc185a756e97c982164fe25859e0d1dcc1475c80a615b2123af1f5f94c11e3e9402c3ac558f500199d95b6d3e301758586281dcd26
7f9c2ba4e88f827d616045507605853ed73b8093f6efbc88eb1a6eacfa66ef263cb1eea988004b93103cfb0aeefd2a686e01fa4a58e8a3639ca8a1e3f9ae57e235b8cc873c23dc62b8d260169afa2f75ab916a58d974918835d25e6a435085b2badfd6dfaac359a5efbb7bcc4b59d538df9a04302e10c8bc1cbf1a0b3a5120ea17cda7cfad765f5623474d368ccca8af0007cd9f5e4c849f167a580b14aabdefaee7eef47cb0fca9767be1fda69419dfb927e9df07348b196691abaeb580b32def58538b8d23f877 46b9dd2b0b
23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7 ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f
4634270f707b6a54daae7530460842e20e37ed265ceee9a43e8924aa 53048e2681941ef99b2e29b76b4c7dabe4c2d0c634fc6d46e0e2f13107e7af23
e642824c3f8cf24ad09234ee7d3c766fc9a3a5168d0c94ad73b46fdf 3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532
ec01498288516fc926459f58e2c6ad8df9b473cb0fc08c2596da7cf0e49be4b298d88cea927ac7f539f1edf228376d25 b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0
5881092dd818bf5cf8a3ddb793fbcba74097d5c526a6d35f97b83351940f2cc844c50af32acd3f2cdd066568706f509bc1bdde58295dae3f891a9a0fca5783789a41f8611214ce612394df286a62d1a2252aa94db9c538956c717dc2bed4f232a0294c857c730aa16067ac1062f1201fb0d377cfb9cde4c63599b27f3462bba4a0ed296c801f9ff7f57302bb3076ee145f97a32ae68e76ab66c48d51675bd49acc29082f5647584e6aa01b3f5af057805f973ff8ecb8b226ac32ada6f01c1fcd4818cb006aa5b4cd 4833666013
b3bb83df5b19419d6efee61258cdd7bee0b86acaba1f71c6d07f9c08 59425e4412e296fc74736673ce067027f384203f59c0d2c3e6be7b13347b3ffc
f1417a0e1c0e48cb613e3d98e2ecc4326a79f5fcb5d1bfb0f4b2461ebe45971161f3e0e6e4cfbdec32e6273a0bd56cf6 7e658e1b50b8375e8ffe4c26591de6ea5c387436a5169172ea07649fc85e7bc989471fad8307cb20b5bf2785bbfad2ef47945e5558c3be23aa8a08d7bae3e7d0
bdb28b8d0b9716e3f0a0029e315414830c1a09e0ca62ee9bc5677cc4 e0de432912cc5bf2848384167bc36eefe1b66fd192acb08fb820b886420fc8b3
5ba166a036c528a0d952f1c596c68f48807908c375835e69fb484d69 5fb531fe026618353eb601c3b227d68d6f935b8fb59177a76ad484ada72b7e32
4a399833cf8524e10a3097c193b22edc5fa474435ba525169af5322d608986a2e3ced0ec121211971a74294a573ba777 459d9fbc2ed3407fa9a30834aa10d1554dfb51e98fe77434c58f52ae0bba671f9606a5f4829ee599433d3fff0257bc9aeaeaeee7cd2eed0a1024cd3b04e2e517
5c309506792c1ac87cd838b0f16ee986e27fa28e6d936d8fd9004c33084bb616b90c12e42cd9e8f584c8be3b16a1b58212c6cdca5f063f19a076ce391062568747dbb1b0c1dbf41659b8ef1ced99fa99cc7883ee4aaaa509b5e0a078c9466fa27a11d55793dcd1d8a90eb4dd0ba8afcff315314f58efa129e8b36ee4e080b317f083f07ab8913ed2f4a2fb9426b3004c0a907a9f8c778546616ad8e757c293a6667ec87dcb4788f72a3cbcadb1aeccae711b80c130927e361cf040e2b3aaa7d9186bf7e718eb9d87 94156ae189
true true true
true 0
",
    );
}

/// An argument value a function can't take fails with INVALID and gives the zero value
/// (STDLIB §1.3 rule 8): an empty range, a negative SHAKE length (the object is left as it
/// was), a negative count of random bytes, clamp's bounds the wrong way round, a find
/// start outside the text, and a chunk or window size below 1, whose iterator gives nothing.
#[test]
fn improper_arguments_fail_with_invalid() {
    prints(
        r##"import zink.hash.*;
import zink.random.*;

void main() {
    Rng r = random.seeded(1);
    i64 a @e1 = r.range(5, 1);
    u8 b @e2 = r.range((u8) 3, (u8) 3);
    i64 c @e3 = r.range(1, 7);
    u8[.] s @e4 = hash.shake_128("abc", -1);
    Shake_256 h = new Shake_256();
    h.update("abc");
    u8[.] t1 @e5 = h.finish(-5);
    u8[.] t2 @e6 = h.finish(4);
    u8[..>] k = [];
    u8 e7 = @random.secureBytes(k, -3);
    i64 cl @e8 = clamp(5, 10, 1);
    f64 cf @e9 = clamp(2.5, 0.0, 1.0);
    i64 f1 @e10 = "hello".find("l", 9);
    i64 f2 @e11 = "hello".find("l", -1);
    i64 f3 @e12 = "hello".find("l", 3);
    i64 f4 @e13 = "hello".find("", 5);
    println("${a} ${e1} ${b} ${e2} ${c > 0} ${e3} ${s.len} ${e4} ${t1.len} ${e5} ${t2.hex()} ${e6} ${k.len} ${e7}");
    println("${cl} ${e8} ${cf} ${e9} ${f1} ${e10} ${f2} ${e11} ${f3} ${e12} ${f4} ${e13}");
    i64[..>] xs = [1, 2, 3];
    i64 n = 0;
    ChunkIterator<i64> ci @e14 = xs.chunks(0);
    while (i64[:] part : ci.next()) { n++; }
    WindowIterator<i64> wi @e15 = xs.windows(-1);
    while (i64[:] w : wi.next()) { n++; }
    println("${n} ${e14} ${e15}");
}
"##,
        "0 22 0 22 true 0 0 22 0 22 48336660 0 0 22\n0 22 1.0 0 0 22 0 22 3 0 5 0\n0 22 22\n",
    );
}

/// `repeat` is `zink.lang`'s `TextMethods.repeat`, so it can fail: INVALID and no text for
/// a negative count. It still works in interpolation and as an argument.
#[test]
fn repeat_fails_for_a_negative_count() {
    prints(
        r##"void main() {
    u8[.] a = "ab".repeat(3);
    u8[.] b @err = "ab".repeat(-2);
    u8 e2 = @"x".repeat(0);
    println("[${a}] [${b}] ${err} ${e2} [${"-".repeat(5)}] ${"=".repeat(4).len}");
    string s = "xy";
    u8[..>] grow = [];
    grow.append(s.repeat(2));
    println("${grow}");
}
"##,
        "[ababab] [] 22 0 [-----] 4\nxyxy\n",
    );
}

/// A call that fails stops the program when nothing reads its status (DESIGN §7.6), at
/// the call's line, naming it and its status, and for a function of the program's own,
/// where it returned the status: in a declaration, inside `${...}`, as a statement, in the
/// library, and for an option `Args` never declared. `@err`, `@_` and `@` read it.
#[test]
fn an_unread_failure_stops_the_program() {
    let exe = build(
        r##"import zink.hash.*;
import zink.cli.*;

i64 half(i64 n) {
    if (n % 2 != 0) {
        return #status.INVALID 0;
    }
    return n / 2;
}

void main(string[.] args) {
    i64 a = half(4);
    i64 b @err = half(3);
    i64 c @_ = half(5);
    u8 s = @half(1);
    println("${a} ${b} ${err} ${c} ${s} ${@half(2)}");
    if (args[0] == "own") { i64 d = half(7); }
    if (args[0] == "interp") { println("${half(9)}"); }
    if (args[0] == "lib") { u8[.] h = hash.shake_256("abc", -1); }
    if (args[0] == "stmt") { half(11); }
    if (args[0] == "typo") {
        Args cli = new Args("t", "");
        cli.flag('n', "number", "show numbers");
        @_ = cli.parse(args[1:]);
        println("given ${cli.has("number")}");
        println("given ${cli.has("nmber")}");
    }
    println("end");
}
"##,
    );
    let first = "2 0 22 0 22 0\n";
    let r = exec(&exe, &["ok"]);
    assert_eq!((r.stdout.as_str(), r.code), ("2 0 22 0 22 0\nend\n", 0));
    let own = "    `half` returned it at prog.zn:6\n";
    for (arg, line, what, more) in [
        ("own", 17, "half", own),
        ("interp", 18, "half", own),
        ("lib", 19, "shake_256", ""),
        ("stmt", 20, "half", own),
        ("typo", 26, "has", ""),
    ] {
        let r = exec(&exe, &[arg]);
        assert_eq!(r.code, 1, "{}", arg);
        assert!(r.stdout.starts_with(first), "{}: {}", arg, r.stdout);
        assert!(!r.stdout.contains("end"), "{}", arg);
        assert_eq!(
            r.stderr,
            format!(
                "prog.zn:{}: `{}` failed with INVALID, and nothing read its status\n{}",
                line, what, more
            ),
            "{}",
            arg
        );
    }
}

/// `@` catches every failure in the expression it reads (DESIGN §7.6): an index, a slice,
/// overflow, division by zero, a cast that doesn't fit, a negation, and a call inside
/// that nothing else reads, which skips the rest. The value is then zero, and a reference
/// refers to a zero value of its own; a reference that doesn't fail refers to the element.
/// The same index without `@` stops the program.
#[test]
fn at_catches_every_failure_in_its_expression() {
    let r = run(
        r##"i64 half(i64 n) {
    if (n % 2 != 0) {
        return #status.INVALID 0;
    }
    return n / 2;
}

i64 twice(i64 n) { return n * 2; }

void main(string[.] args) {
    i64[..>] a = [10, 20];
    i64 big = i64.MAX;
    i64 zero = args.len - args.len;
    i64 x @e1 = a[5];
    i64 y @e2 = a[0] + 1;
    u8 s3 = @(a[0] / zero);
    i64 o @e4 = big +# 1;
    i64 n = 300;
    u8 b @e5 = (u8) n;
    i64 t @e6 = twice(half(3));
    i64 u @e7 = twice(a[9]);
    i64 w @e8 = half(a[0]) + a[1];
    i64& r @e9 = a[1];
    r = 99;
    i64& q @e10 = a[7];
    q = 5;
    string text = "hello";
    string sl @e11 = text[2:9];
    i64 neg @e12 = 0 -# (i64.MIN + zero);
    println("${x} ${e1} ${y} ${e2} ${s3} ${o} ${e4} ${b} ${e5} ${t} ${e6} ${u} ${e7} ${w} ${e8}");
    println("${a[1]} ${e9} ${q} ${e10} [${sl}] ${e11} ${neg} ${e12} ${@a[1]} ${@a[3]}");
    i64 v = a[2];
}
"##,
        &[],
    );
    assert_eq!(
        r.stdout,
        "0 34 11 0 33 0 34 0 34 0 22 0 34 25 0\n99 0 5 34 [] 34 0 34 0 34\n"
    );
    assert_eq!(
        r.stderr,
        "prog.zn:32: index 2 is out of range for length 2\n"
    );
    assert_eq!(r.code, 1);
}

/// examples/jsonfmt.zn (STDLIB §9.3, §9.7): re-indented event by event, compact, and
/// through a tree with every object's keys sorted; numbers keep their text, escapes are
/// decoded and written back as JSON needs. Malformed input fails with its line and
/// column. Random documents and their one-character mutations agree with Python's json.
#[test]
fn jsonfmt_reindents_sorts_and_reports_errors() {
    use std::io::Write;
    let exe = example("jsonfmt");
    let feed = |args: &[&str], input: &str| {
        let mut child = Command::new(&exe)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // jsonfmt stops reading at the first error, which can close the pipe first.
        if let Err(e) = child.stdin.take().unwrap().write_all(input.as_bytes()) {
            assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "{}", e);
        }
        let o = child.wait_with_output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    let doc = "{\"name\": \"zink\", \"tags\": [\"a\", \"b\\u00e9\\ud83d\\ude00\"], \"n\": 12345678901234567890, \"x\": 1.50, \"nested\": {\"z\": 1, \"a\": [true, false, null, {}], \"e\": []}, \"esc\": \"line\\nbreak \\\"q\\\" \\\\\"}";
    assert_eq!(
        feed(&[], doc),
        (0, "{\n  \"name\": \"zink\",\n  \"tags\": [\n    \"a\",\n    \"bé😀\"\n  ],\n  \"n\": 12345678901234567890,\n  \"x\": 1.50,\n  \"nested\": {\n    \"z\": 1,\n    \"a\": [\n      true,\n      false,\n      null,\n      {}\n    ],\n    \"e\": []\n  },\n  \"esc\": \"line\\nbreak \\\"q\\\" \\\\\"\n}\n".to_string(), "".to_string()),
        "plain"
    );
    assert_eq!(
        feed(&["-c"], doc),
        (0, "{\"name\":\"zink\",\"tags\":[\"a\",\"bé😀\"],\"n\":12345678901234567890,\"x\":1.50,\"nested\":{\"z\":1,\"a\":[true,false,null,{}],\"e\":[]},\"esc\":\"line\\nbreak \\\"q\\\" \\\\\"}\n".to_string(), "".to_string()),
        "compact"
    );
    assert_eq!(
        feed(&["-s", "--indent", "1"], doc),
        (0, "{\n \"esc\": \"line\\nbreak \\\"q\\\" \\\\\",\n \"n\": 12345678901234567890,\n \"name\": \"zink\",\n \"nested\": {\n  \"a\": [\n   true,\n   false,\n   null,\n   {}\n  ],\n  \"e\": [],\n  \"z\": 1\n },\n \"tags\": [\n  \"a\",\n  \"bé😀\"\n ],\n \"x\": 1.50\n}\n".to_string(), "".to_string()),
        "sorted"
    );
    assert_eq!(
        feed(&["-c"], "[1, {\"a\": 2,}]"),
        (
            1,
            "".to_string(),
            "jsonfmt: stdin:1:13: expected a key in double quotes\n".to_string()
        ),
        "trailing"
    );
    assert_eq!(
        feed(&["-c"], "{\"deep\": \"unterminated"),
        (
            1,
            "".to_string(),
            "jsonfmt: stdin:1:10: this string is never closed\n".to_string()
        ),
        "unterminated"
    );
    assert_eq!(
        feed(&["--indent", "99"], "[]"),
        (2, "".to_string(), "jsonfmt: --indent takes a number from 0 to 16\nusage: jsonfmt [--indent N] [-c] [-s] [file]\n".to_string()),
        "usage"
    );
}

/// zink.data's Value over a JSON tree (STDLIB §9.2): navigation that never fails, readers
/// that fail with MISSING, TYPE and RANGE, fallbacks, a number's text past i64, repeated
/// keys, JSON Pointer with its escapes, iterators, the Parser interface; the reader's
/// skipValue and asI64; JSON Lines with a bad line; and JsonWriter's finish() reporting
/// misuse, NaN and a bad number while the output stays well formed, with asciiOnly.
#[test]
fn json_trees_readers_and_writers() {
    prints(
        r##"import zink.data.*;
import zink.json.*;

void main() {
    string text = "{\"server\": {\"host\": \"example.org\", \"port\": 8080, \"ratio\": 0.75},\n \"users\": [{\"name\": \"ann\"}, {\"name\": \"bo\\u00e9\"}, {}],\n \"big\": 99999999999999999999, \"neg\": -3, \"flag\": true, \"none\": null,\n \"a/b\": 1, \"m~n\": 2, \"dup\": 1, \"dup\": 2}";
    u8[..>] strings = [];
    Node[..>] tree = [];
    JsonParser p = new JsonParser(text);
    Parser& any = p;
    Value cfg @bad = any.parse(tree, strings);
    println("parsed ${bad} ${tree.len} nodes, ${cfg.len()} entries, line of users ${cfg.get("users").line()}");
    string host @_ = cfg.get("server").get("host").asStringOr("localhost");
    i64 port @e1 = cfg.get("server").get("port").asI64();
    f64 ratio @e2 = cfg.get("server").get("ratio").asF64();
    i64 asInt @e3 = cfg.get("server").get("ratio").asI64();
    i64 missing @e4 = cfg.get("server").get("nope").asI64();
    i64 big @e5 = cfg.get("big").asI64();
    u64 neg @e6 = cfg.get("neg").asU64();
    i64 fb @e7 = cfg.get("absent").asI64Or(7);
    i64 fbType @e8 = cfg.get("flag").asI64Or(7);
    println("${host} ${port}/${e1} ${ratio}/${e2} ${asInt}/${e3} ${missing}/${e4} ${big}/${e5} ${neg}/${e6} ${fb}/${e7} ${fbType}/${e8}");
    string bigText @_ = cfg.get("big").text();
    bool flag @_ = cfg.get("flag").asBool();
    println("${bigText} ${flag} ${cfg.get("none").isNull()} ${cfg.get("none").exists()} ${cfg.get("zzz").exists()} ${cfg.get("dup").asI64Or(0)}");
    for (Value user : cfg.get("users").items()) {
        string name @_ = user.get("name").asStringOr("?");
        print("[${name}] ");
    }
    println("");
    string second @_ = cfg.pointer("/users/1/name").asString();
    i64 slash @_ = cfg.pointer("/a~1b").asI64();
    i64 tilde @_ = cfg.pointer("/m~0n").asI64();
    println("${second} ${slash} ${tilde} ${cfg.pointer("/users/9/name").exists()} ${cfg.pointer("/users/01").exists()} ${cfg.pointer("").len()}");
    i64 keys = 0;
    for (Entry e : cfg.entries()) { keys++; }
    println("entries ${keys}, users ${cfg.get("users").len()}, at(1) ${cfg.get("users").at(1).get("name").asStringOr("?")}, at on an object ${cfg.at(0).exists()}");

    // The reader: skip a value, read numbers.
    JsonReader r = new JsonReader("{\"skip\": {\"deep\": [1, [2, {\"x\": 3}]]}, \"id\": 42, \"f\": 2.5}");
    while (JsonEvent e : r.next()) {
        if (e.kind == JsonToken.KEY && e.text == "skip") { r.skipValue(); }
        if (e.kind == JsonToken.NUMBER) {
            i64 n @why = e.asI64();
            print("number ${e.text} as i64 ${n}/${why} ");
        }
    }
    println("| error code ${r.error().code}");

    // JSON Lines.
    string lines = "{\"n\": 1}\n{\"n\": 2}\n{\"n\": oops}\n";
    i64 line = 0;
    for (string l : lines.lines()) {
        line++;
        u8[..>] lstrings = [];
        Node[..>] ltree = [];
        JsonParser lp = new JsonParser(l);
        Value v @lbad = lp.parse(ltree, lstrings);
        if (lbad) {
            println("line ${line}: ${lp.error().message} at column ${lp.error().column}");
        } else {
            print("n=${v.get("n").asI64Or(0)} ");
        }
    }

    // The writer: misuse and values JSON can't hold are reported by finish.
    u8[..>] out = [];
    JsonWriter w = new JsonWriter(out);
    w.beginObject();
    w.key("ok");
    w.write(1.5);
    w.write(true);
    w.key("nan");
    w.write(0.0 / 0.0);
    w.end();
    u8 st = @w.finish();
    println("${out} -> ${st}");
    out.clear();
    JsonWriter w2 = new JsonWriter(out);
    w2.beginArray();
    w2.writeNumber("12e");
    w2.writeNumber("-0.5E+2");
    w2.end();
    w2.end();
    u8 st2 = @w2.finish();
    println("${out} -> ${st2}");
    out.clear();
    JsonWriter w3 = new JsonWriter(out);
    w3.asciiOnly = true;
    w3.beginArray();
    w3.write("bé😀\u{1}");
    w3.write((u64) 18446744073709551615);
    w3.write(-5);
    w3.beginObject();
    w3.key("open");
    u8 st3 = @w3.finish();
    println("${out} -> ${st3}");
    out.clear();
    u8 st4 = @json.write(out, cfg.get("server"), 0);
    println("${out} -> ${st4}");
}
"##,
        "parsed 0 34 nodes, 10 entries, line of users 2\nexample.org 8080/0 0.75/0 0/197 0/195 0/34 0/34 7/0 7/197\n99999999999999999999 true true true false 2\n[ann] [boé] [?] \nboé 1 2 false false 10\nentries 10, users 3, at(1) boé, at on an object false\nnumber 42 as i64 42/0 number 2.5 as i64 0/197 | error code 0\nn=1 n=2 line 3: expected a value at column 7\n{\"ok\":1.5,\"\":true,\"nan\":null} -> 22\n[null,-0.5E+2] -> 22\n[\"b\\u00e9\\ud83d\\ude00\\u0001\",18446744073709551615,-5,{\"open\":null}] -> 22\n{\"host\":\"example.org\",\"port\":8080,\"ratio\":0.75} -> 0\n",
    );
}

/// isValidUtf8 is strict UTF-8: overlong forms, surrogates, past U+10FFFF and cut-off
/// sequences are invalid; it agrees with Python's decoder on 3,000 random byte strings.
#[test]
fn valid_utf8_is_strict() {
    prints(
        r##"void main() {
    u8[..>] b = [];
    string ok = "aé😀中";
    print("${ok.isValidUtf8()} ${"".isValidUtf8()} ");
    for (i64 k = 0; k < 6; k++) {
        b.clear();
        if (k == 0) { b +> 0xc0; b +> 0xaf; }
        if (k == 1) { b +> 0xe0; b +> 0x80; b +> 0xaf; }
        if (k == 2) { b +> 0xed; b +> 0xa0; b +> 0x80; }
        if (k == 3) { b +> 0xf4; b +> 0x90; b +> 0x80; b +> 0x80; }
        if (k == 4) { b +> 0xf0; b +> 0x9f; b +> 0x98; }
        if (k == 5) { b +> 0xf0; b +> 0x9f; b +> 0x98; b +> 0x80; }
        print("${b.isValidUtf8()} ");
    }
    println("");
}
"##,
        "true true false false false false false true \n",
    );
}

/// `@@` and `@@@` also catch a failure nothing read inside the call, abandoning it (DESIGN
/// §7.6): `@` then reads ABANDONED and `@@` the failure itself; the trail says where it was
/// returned and where nothing read it. Without them, the program stops where nothing read
/// it. `@` with `@@@` catches the same; `@@` in an expression gives the failure.
#[test]
fn deep_reads_catch_what_nothing_read_inside() {
    let r = run(
        r##"i64 parsePort(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    if (n < 1 || n > 65535) {
        return #status.RANGE 0;
    }
    return n;
}

i64 portOf(string addr) {
    i64 colon @none = addr.find(":");
    if (none) {
        return #status.MISSING 0;
    }
    return parsePort(addr[colon + 1:addr.len]);
}

void show(Failure[..~32] t) {
    for (var f : t) {
        println("    ${f.function} ${status.name(f.code)} ${f.file}:${f.line}${f.unread ? " unread" : ""}");
    }
}

void main(string[.] args) {
    string[.] addrs = ["example.com:8080", "example.com", "example.com:http"];
    for (string a : addrs) {
        i64 p @one @@all @@@t = portOf(a);
        println("${a}: ${p} ${status.name(one)} ${status.name(all)} ${t.len}");
        show(t);
        i64 q @one2 @@@t2 = portOf(a);
        println("  ${q} ${status.name(one2)} ${t2.len}");
    }
    u8 s = @@portOf("x:y");
    u8 s1 = @portOf("x");
    i64 v = 0;
    u8 e = 0;
    v @@e = portOf("x:z");
    @@e = portOf("x:z");
    @@_ = portOf("x:z");
    println("${s} ${s1} ${e}");
    i64 last = portOf(args[0]);
    println("end");
}
"##,
        &["x:80000"],
    );
    assert_eq!(
        r.stdout,
        "example.com:8080: 8080 OK OK 0\n  8080 OK 0\n\
         example.com: 0 MISSING MISSING 1\n    portOf MISSING prog.zn:15\n  0 MISSING 1\n\
         example.com:http: 0 ABANDONED INVALID 2\n    parsePort INVALID prog.zn:4\n    portOf INVALID prog.zn:17 unread\n  0 ABANDONED 2\n\
         22 195 22\n"
    );
    assert_eq!(
        r.stderr,
        "prog.zn:17: `parsePort` failed with RANGE, and nothing read its status\n    `parsePort` returned it at prog.zn:7\n"
    );
    assert_eq!(r.code, 1);
}

/// What a call abandoned by `@@` leaves (DESIGN §6.4, §7.6): every `defer` in the functions
/// it abandons runs, innermost first, seeing the variables as they were when it stopped.
/// The innermost `@@` catches; a check deep inside, such as an index or a division by zero,
/// abandons the calls as an unread failure does; a repeat loop with `@@why` ends at one.
#[test]
fn abandoned_calls_run_their_defers() {
    prints(
        r##"i64[..>] log = [];

i64 inner(i64 i) {
    defer log +> 1;
    i64 count = 10;
    defer log +> count;
    count = 20;
    i64[..>] a = [1, 2, 3];
    {
        defer log +> 3;
        return a[i];
    }
}

i64 middle(i64 i) {
    defer log +> 4;
    i64 r = inner(i);
    return r * 2;
}

i64 outer(i64 i) {
    i64 v @@why = middle(i);
    if (why) {
        log +> 100 + (i64) why;
    }
    return v;
}

i64 divide(i64 a, i64 b) { return a / b; }

i64[..>] items = [5, 6, 7];

i64 step(i64 k) {
    if (k >= 5) {
        return #status.END 0;
    }
    return items[k];
}

void main() {
    i64 ok = outer(1);
    println("${ok} ${log}");
    log.clear();
    i64 bad = outer(7);
    println("${bad} ${log}");
    log.clear();
    i64 n @@all = outer(9);
    println("${n} ${all} ${log}");
    i64 d @@e @@@t = divide(1, 0);
    println("${d} ${status.name(e)} ${t.len} ${t[0].function} ${t[0].line} ${t[0].unread}");
    i64 k = 0;
    i64 hits = 0;
    while (i64 q : step(k)) @@why {
        hits++;
        k++;
    }
    println("${hits} ${status.name(why)}");
}
"##,
        "4 [3, 20, 1, 4]\n0 [3, 20, 1, 4, 134]\n0 0 [3, 20, 1, 4, 134]\n0 DOMAIN 1 divide 29 true\n3 RANGE\n",
    );
}

/// The trail (DESIGN §7.6): a failure passed on adds to the trail of the one read, and so
/// does a code of the function's own returned where it tests that status; any other failure
/// starts a new one. A failure from the library, or one `pop` gives, starts at the line that
/// reads it. A long trail keeps its first 24 entries and its last 8.
#[test]
fn the_trail_says_where_a_failure_went() {
    let exe = build(
        r##"i64[..~] items = [];

i64 load(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    return n;
}

i64 config(string s) {
    i64 n @err = load(s);
    if (err) {
        return #224 0;
    }
    return n;
}

i64 handled(string s) {
    i64 n @err = load(s);
    if (n < 1) {
        return #status.RANGE 0;
    }
    return n;
}

i64 popped() {
    i64 v @err = items.pop();
    return #err v;
}

i64 down(i64 n) {
    if (n == 0) {
        return #status.MISSING 0;
    }
    i64 v @err = down(n - 1);
    return #err v;
}

void show(string what, Failure[..~32] t) {
    print(what);
    for (var f : t) {
        print(" ${f.function}:${f.line}:${f.code}");
    }
    println("");
}

void main(string[.] args) {
    i64 a @@e1 @@@t1 = config("x");
    show("config", t1);
    i64 b @@e2 @@@t2 = handled("x");
    show("handled", t2);
    i64 c @@e3 @@@t3 = popped();
    show("popped", t3);
    i64 d @@e4 @@@t4 = items.pop();
    show("pop", t4);
    i64 e @@e5 @@@t5 = down(40);
    println("down ${t5.len} ${t5[0].line} ${t5[23].line} ${t5[24].line} ${t5[31].line} ${t5[31].code}");
    if (args[0] == "config") { i64 x = config("y"); }
    if (args[0] == "down") { i64 x = down(40); }
}
"##,
    );
    let r = exec(&exe, &["config"]);
    assert_eq!(
        r.stdout,
        "config load:6:22 config:14:224\nhandled handled:22:34\npopped popped:29:192\npop main:55:192\ndown 32 34 37 37 37 195\n"
    );
    assert_eq!(
        r.stderr,
        "prog.zn:59: `config` failed with 224, and nothing read its status\n    `load` returned INVALID at prog.zn:6\n    `config` returned it at prog.zn:14\n"
    );
    let r = exec(&exe, &["down"]);
    let lines: Vec<&str> = r.stderr.lines().collect();
    assert_eq!(lines[0], "prog.zn:60: `down` failed with MISSING, and nothing read its status");
    assert_eq!(lines[1], "    `down` returned it at prog.zn:34");
    assert_eq!(lines[2], "    `down` passed it on at prog.zn:37");
    assert_eq!(lines[25], "    ... 10 more");
    assert_eq!(lines[26], "    `down` passed it on at prog.zn:37");
    assert_eq!(lines.len(), 33);
}

/// A variable-size record built in place keeps the status of what built it: a call's,
/// with its value, and a constructor's. Reading it claims the trail, so a failure passed
/// on says so (DESIGN §3.3, §7.6).
#[test]
fn records_built_in_place_keep_their_status() {
    let exe = build(
        r##"class Item {
    u8[.*] sku;
    i32 qty;

    new(string sku, i32 qty) {
        this.sku = sku;
        this.qty = qty;
        if (qty < 0) { return #status.INVALID; }
    }
}

Item parse(string line) {
    (string code, string rest) = line.splitFirst(":");
    i32 n @err = i32.parse(rest.trim());
    if (err) { return #err new Item(code, 0); }
    return new Item(code, n);
}

Item fromCtor() {
    Item it @e = new Item("z", -1);
    if (e) { return #e it; }
    return it;
}

void main(string[.] args) {
    Item a @e1 = parse("SKU-9:lots");
    Item b @e2 = parse("SKU-7:3");
    Item c @e3 = fromCtor();
    println("${e1} ${a.sku} ${e2} ${b.sku} ${b.qty} ${e3}");
    if (args[0] == "stop") { Item d = fromCtor(); }
}
"##,
    );
    let r = exec(&exe, &["go"]);
    assert_eq!(r.stdout, "22 SKU-9 0 SKU-7 3 22\n", "stderr: {}", r.stderr);
    let r = exec(&exe, &["stop"]);
    assert_eq!(
        r.stderr,
        "prog.zn:30: `fromCtor` failed with INVALID, and nothing read its status\n    `Item.new` returned it at prog.zn:8\n    `fromCtor` passed it on at prog.zn:21\n"
    );
}

/// Self-relative links (REFERENCES §7.2): a field holds the signed byte offset to what it
/// refers to, in 1, 2 or 4 bytes, so a node is smaller; a value with links is built in its
/// slot; following a link is an add; copying the whole array keeps every link inside the
/// copy; and an offset that doesn't fit its width stops the program (L4).
#[test]
fn self_relative_links() {
    let text = r##"class Cell {
    i64 v;
    Cell&<i16>? next = null;
}

class Tiny { u8 v; Tiny&<i8>? prev; }

class Pair {
    i32 a;
    Pair&<i32>? partner;

    new(Pair&? to, i32 a) {
        this.a = a;
        partner = to;
    }
}

class Word {
    u8[.*] text;
    Word&<i32>? up;
}

i64 total(Cell& c) {
    i64 s = 0;
    Cell&? at = c;
    while (at != null) {
        Cell& here = at;
        s += here.v;
        at .= here.next;
    }
    return s;
}

void main(string[.] args) {
    Cell[..>] cells = [];
    Cell& first = cells.push(new Cell(1));
    Cell& prev = first;
    for (i64 i = 2; i <= 5; i++) {
        Cell& c = cells.push(new Cell(i));
        prev.next .= c;
        prev .= c;
    }
    Cell[..>] copy = cells;
    copy[0].v = 100;
    copy[1].v = 200;
    println("${total(first)} ${total(copy[0])}");

    Tiny[..>] ts = [];
    Tiny& t0 = ts.push(new Tiny(7, null));
    Tiny& t1 = ts.push(new Tiny(8, t0));
    Tiny&? back = t1.prev;
    println("${back == null ? 0 : back.v} ${t0.prev == null}");

    Pair[..>] ps = [];
    Pair& p0 = ps.push(new Pair(null, 1));
    Pair& p1 = ps.push(new Pair(p0, 2));
    ps +> new Pair(p1, 3);
    Pair&? pp = ps[2].partner;
    println("${pp == null ? 0 : pp.a}");

    Word[..>] ws = [];
    Word& root = ws.push(new Word("root", null));
    Word& leaf = ws.push(new Word("leaf", root));
    Word&? u = leaf.up;
    println("${leaf.text} ${u == null ? "-" : u.text}");

    if (args.len > 0 && args[0] == "far") {
        for (i64 i = 0; i < 200; i++) { ts +> new Tiny(1, null); }
        Tiny& last = ts.push(new Tiny(2, null));
        last.prev .= t0;
    }
}
"##;
    let exe = build(text);
    let r = exec(&exe, &[]);
    assert_eq!(r.stdout, "15 312\n7 true\n2\nleaf root\n", "stderr: {}", r.stderr);
    let r = exec(&exe, &["far"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("prog.zn:70: this link's target is too far away for its 1-byte offset"), "{}", r.stderr);
    // Each link is its offset: Tiny is 2 bytes, Pair 8.
    let file = SourceFile::new("prog.zn", text);
    let (_, c) = zink::compile_c(std::slice::from_ref(&file));
    let c = c.expect("compiles");
    assert!(c.contains("struct zc1_Tiny { int8_t f_prev; uint8_t f_v; };"), "Tiny");
    assert!(c.contains("struct zc2_Pair { int32_t f_a; int32_t f_partner; };"), "Pair");
}

/// `pushAll` (STDLIB §7.1): every element of a slice at the end, one copy on a data
/// stack, all or nothing with FULL on a capped array, one by one on a deque.
#[test]
fn push_all() {
    prints(
        r##"void main() {
    i64[..>] a = [];
    a +> 1 +> 2 +> 3;
    a.pushAll(a);
    i64[..~4] c = [];
    @_ = c.pushAll(a[0:3]);
    u8 st = @c.pushAll(a[0:3]);
    i64[~.~] d = [];
    d +< 9;
    d.pushAll(a[1:3]);
    println("${a} ${c} ${st} ${d}");
}
"##,
        "[1, 2, 3, 1, 2, 3] [1, 2, 3] 193 [9, 2, 3]\n",
    );
}

/// A for-each over an array of variable-size arrays: by reference, writes reach the
/// records; as `string`s, the elements are slices, usable as hashmap keys.
#[test]
fn for_each_over_arrays_of_variable_size_arrays() {
    prints(
        r##"void main() {
    u8[.*][..>] words = [];
    words +> "abc" +> "de" +> "abc";
    for (u8[.*]& w : words) { w[0] = 'X'; }
    hashmap<string, i32> counts = [];
    for (string w : words) { counts.getOrAdd(w, 0)++; }
    for (var& e : counts) { println("${e.key} ${e.value}"); }
    i64 total = 0;
    for (u8[.*]& w : words) { total += w.len; }
    println("${total}");
}
"##,
        "Xbc 2\nXe 1\n8\n",
    );
}

/// A function returning an array of variable-size records builds it where the caller
/// keeps it, and copies it by its bytes when that is a temporary.
#[test]
fn returning_arrays_of_variable_size_records() {
    prints(
        r##"class Item { u8[.*] sku; i32 qty; }
class Resp { i64 id; Item[.*] items; }

Item[..>] makeItems(i64 n) {
    Item[..>] t = [];
    for (i64 i = 0; i < n; i++) { t +> new Item(i % 2 == 0 ? "ab" : "cdef", (i32) i); }
    return t;
}

Item[..>] passOn(i64 n) { return makeItems(n); }

void main() {
    Item[..>] direct = makeItems(4);
    Item[..>] passed = passOn(3);
    Resp r = new Resp(1, makeItems(3));
    i64 a = 0;
    for (Item& it : direct) { a += it.qty * 10 + it.sku.len; }
    i64 b = 0;
    for (Item& it : passed) { b += it.qty * 10 + it.sku.len; }
    i64 c = 0;
    for (Item& it : r.items) { c += it.qty + it.sku.len; }
    println("${direct.len} ${a} ${passed.len} ${b} ${c} ${makeItems(5).len}");
}
"##,
        "4 72 3 38 11 5\n",
    );
}

/// `=` on a slice or a writable view copies into what it views, overlapping or not, and
/// stops the program when the lengths differ (DESIGN §4.10); `.=` rebinds the view.
#[test]
fn copying_into_views() {
    let exe = build(
        r##"void fill(u8[:]& w, string from) { w = from; }

void main(string[.] args) {
    i32[..>] a = [];
    for (i32 i = 0; i < 6; i++) { a +> i; }
    a[1:] = a[0:5];
    i32[..>] b = [];
    for (i32 i = 0; i < 4; i++) { b +> i * 10; }
    a[0:2] = b[2:4];
    u8[..>] buf = [];
    buf.append("..........");
    u8[:]& w = buf[2:6];
    w = "abcd";
    fill(buf[6:8], "XY");
    w .= buf[0:2];
    w = "<>";
    println("${a} ${buf}");
    if (args.len > 0) { a[0:2] = a[0:3]; }
}
"##,
    );
    let r = exec(&exe, &[]);
    assert_eq!(r.stdout, "[20, 30, 1, 2, 3, 4] <>abcdXY..\n", "stderr: {}", r.stderr);
    let r = exec(&exe, &["bad"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("prog.zn:18: the lengths differ, so the copy doesn't fit the view"), "{}", r.stderr);
}

/// A constructor that fails leaves its class's zero value (DESIGN §6.2): the fields it set
/// are reset, and a growable field is emptied but keeps its storage.
#[test]
fn a_failing_constructor_resets_its_fields() {
    prints(
        r##"class Box {
    i64 a;
    i32[.3] cells;
    i64[..>] items;

    new(i64 t) {
        a = 5;
        cells = [7, 8, 9];
        items +> 1 +> 2;
        if (t < 0) { return #status.INVALID; }
    }
}

class Wrap {
    string name;
    hashmap<i64, i64> m;

    new(bool fail) {
        name = "set";
        m.getOrAdd(1, 10);
        if (fail) { return #status.RANGE; }
    }
}

void main() {
    Box b @e = new Box(-1);
    b.items +> 3;
    Box ok @e2 = new Box(1);
    Wrap w @e3 = new Wrap(true);
    w.m.getOrAdd(2, 20);
    println("${b.a} ${b.cells} ${b.items} ${e} | ${ok.a} ${ok.items} ${e2} | [${w.name}] ${w.m.len} ${e3}");
}
"##,
        "0 [0, 0, 0] [3] 22 | 5 [1, 2] 0 | [] 1 34\n",
    );
}

/// Pool links (REFERENCES §7.3): the offset of the target from the start of a static
/// grow-only array, plus one, so 0 is null. A value holding them can be copied anywhere
/// and still reaches its targets, and an offset that doesn't fit stops the program (L4).
#[test]
fn pool_links() {
    let text = r##"class TNode {
    i32 v;
    TNode&<u32 in pool>? l;
    TNode&<u32 in pool>? r;
}

class Small { u8 v; Small&<u8 in smalls>? prev; }

TNode[..>] pool = [];
Small[..>] smalls = [];

TNode& build(i64 depth, i32 v) {
    if (depth == 0) { return pool.push(new TNode(v, null, null)); }
    TNode& l = build(depth - 1, v * 2);
    TNode& r = build(depth - 1, v * 2 + 1);
    return pool.push(new TNode(v, l, r));
}

i64 sum(TNode& n) {
    i64 t = n.v;
    TNode&? l = n.l;
    if (l != null) { t += sum(l); }
    TNode&? r = n.r;
    if (r != null) { t += sum(r); }
    return t;
}

void main(string[.] args) {
    TNode& root = build(3, 1);
    TNode copy = root;
    TNode&? cl = copy.l;
    TNode[..>] moved = [];
    moved +> root;
    println("${pool.len} ${sum(root)} ${cl == null ? 0 : cl.v} ${sum(moved[0])}");
    Small& s0 = smalls.push(new Small(1, null));
    for (i64 i = 0; i < 300; i++) { smalls +> new Small(2, s0); }
    if (args.len > 0) {
        Small& far = smalls.push(new Small(3, null));
        smalls +> new Small(4, far);
    }
}
"##;
    let exe = build(text);
    let r = exec(&exe, &[]);
    assert_eq!(r.stdout, "15 120 2 120\n", "stderr: {}", r.stderr);
    let r = exec(&exe, &["far"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("this pool link's target is too far into its array for its 1-byte offset"), "{}", r.stderr);
    let file = SourceFile::new("prog.zn", text);
    let (_, c) = zink::compile_c(std::slice::from_ref(&file));
    let c = c.expect("compiles");
    assert!(c.contains("struct zc0_TNode { uint32_t f_l; uint32_t f_r; int32_t f_v; };"), "TNode");
    assert!(c.contains("struct zc1_Small { uint8_t f_prev; uint8_t f_v; };"), "Small");
}

/// A fixed array, `T[.k]`, always has exactly `k` elements (DESIGN §4.2): `[]` gives `k`
/// zero values, and so does the zero value of a failed call, which is all zero bytes, since
/// the length isn't stored. Taken from a slice, it must be given exactly `k`.
#[test]
fn fixed_arrays_always_have_their_length() {
    prints(
        r##"class P { i32 x; i32 y; }
class Holder { i32[.4] cells; i64 tag; }
class Rec { u8[.*] name; i16[.3] marks; }

i32[..>] small = [];

i64 sum(i32[.4] a) {
    i64 s = 0;
    for (i32 v : a) { s += v; }
    return s;
}

i64 sumSlice(i32[:] a) {
    i64 s = 0;
    for (i64 i = 0; i < a.len; i++) { s += a[i]; }
    return s;
}

i32[.4] make(i32 base) {
    i32[.4] r = [];
    for (i64 i = 0; i < r.len; i++) { r[i] = base + (i32) i; }
    return r;
}

i32[.4] deep() {
    i32[.4] r = [1, 2, 3, 4];
    r[0] = small[5];
    return r;
}

void bump(i32[.4]& a) {
    for (i32& v : a) { v += 100; }
}

void main() {
    i32[.4] b = [];
    i32[.4] c = [1, 2, 3, 4];
    P[.3] ps = [];
    string[.2] names = [];
    Holder h = new Holder([], 7);
    u8[.3] tag = "abc";
    println("${b.len} ${c.len} ${ps.len} ${names.len} ${h.cells.len} ${b} ${names[1].len} ${tag}");
    b[3] = 9;
    ps[2].x = 5;
    c = [5, 6, 7, 8];
    h.cells[0] = 3;
    println("${b} ${c} ${ps[2].x} ${sum(c)} ${sum(b)} ${sumSlice(c)} ${sumSlice(c[1:3])} ${h.cells}");
    i32[.4] m = make(10);
    bump(m);
    i32[.4] z @@e = deep();
    i32[.4] whole = c[0:4];
    println("${m} ${m == make(10)} ${z.len} ${z} ${e} ${whole}");
    i32[.4] s = [4, 1, 3, 2];
    s.sort();
    println("${s} ${s[^1]} ${s.contains(3)}");
    Rec[..>] recs = [];
    recs +> new Rec("ann", [1, 2, 3]);
    recs +> new Rec("bo", []);
    for (Rec& r : recs) { println("${r.name} ${r.marks} ${r.marks.len}"); }
    i32[.2][.3] grid = [];
    grid[2][1] = 7;
    hashmap<i64, i32[.2]> map = [];
    i32[.2]& slot = map.getOrAdd(5, []);
    slot[1] = 3;
    i32[.2] got @eg = map.get(5);
    i32[.2] none @en = map.get(6);
    println("${grid.len} ${grid[0].len} ${grid[2]} ${got} ${none} ${none.len} ${en}");
}
"##,
        "4 4 3 2 4 [0, 0, 0, 0] 0 abc
[0, 0, 0, 9] [5, 6, 7, 8] 5 26 9 26 13 [3, 0, 0, 0]
[110, 111, 112, 113] false 4 [0, 0, 0, 0] 34 [5, 6, 7, 8]
[1, 2, 3, 4] 4 true
ann [1, 2, 3] 3
bo [0, 0, 0] 3
3 2 [0, 7] [0, 3] [0, 0] 2 195
",
    );
    let r = run(
        "void main() { i32[..>] c = [1, 2, 3]; i32[.4] part = c[0:2]; println(\"${part}\"); }\n",
        &[],
    );
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("the value isn't the 4 elements the array holds"), "{}", r.stderr);
}

/// The runtime's short compares and copies, its search, its string hash and its varint
/// lengths, each a faster form of a simple loop: they agree with the simple loop at every
/// length to 40 and some beyond, at every place a difference can be, for needles of each
/// length found from each start, for map keys of each length that differ in one byte, for
/// lengths that take one varint byte and two, and for every byte base64 must reject.
#[test]
fn short_compares_copies_and_searches_agree_with_simple_loops() {
    prints(
        r##"import zink.encoding.*;

// The simple loops the runtime's faster forms must agree with.
bool same(string a, string b) {
    if (a.len != b.len) { return false; }
    for (i64 i = 0; i < a.len; i++) { if (a[i] != b[i]) { return false; } }
    return true;
}

i64 slowFind(string s, string x, i64 from) {
    for (i64 i = from; i + x.len <= s.len; i++) {
        if (same(s[i:i + x.len], x)) { return i; }
    }
    return -1;
}

void main() {
    // Equality at every length to 40, equal and differing at each place.
    u8[..>] base = [];
    for (i64 i = 0; i < 300; i++) { base +> (u8) ('a' + i * 7 % 26); }
    i64 bad = 0;
    i64 checks = 0;
    for (i64 n = 0; n <= 300; n = n < 40 ? n + 1 : n + 37) {
        u8[..>] a = [];
        a.append(base[0:n]);
        u8[..>] b = [];
        b.append(base[0:n]);
        checks++;
        if (!(a == b) || a != b) { bad++; }
        for (i64 k = 0; k < n; k++) {
            b[k] = (u8) (b[k] + 1);
            checks++;
            if (a == b || same(a, b)) { bad++; }
            b[k] = (u8) (b[k] - 1);
        }
        // A copy of each length appended after a prefix lands whole.
        u8[..>] c = [];
        c.append("xyz");
        c.append(a);
        c.append("!");
        checks++;
        if (c.len != n + 4 || !same(c[3:3 + n], a) || c[n + 3] != '!') { bad++; }
        checks++;
        if (!a.startsWith(base[0:n / 2]) || !a.endsWith(a[n / 3:n]) || (n > 0 && a.startsWith(c))) { bad++; }
    }
    println("equal and copy: ${checks} checks, ${bad} wrong");

    // find, count and split against the simple loop, for needles of each length.
    string hay = "abcabcabd aaab ab, a,b,,c abcabcabcabd zzz abcabd";
    bad = 0;
    checks = 0;
    for (i64 at = 0; at < hay.len; at++) {
        for (i64 m = 1; m <= 6 && at + m <= hay.len; m++) {
            string x = hay[at:at + m];
            for (i64 from = 0; from <= hay.len; from += 5) {
                i64 got @missing = hay.find(x, from);
                if (missing) { got = -1; }
                checks++;
                if (got != slowFind(hay, x, from)) { bad++; }
            }
        }
    }
    i64 n @missing = hay.find("abd!");
    println("find: ${checks} checks, ${bad} wrong, ${missing} ${hay.count("ab")} ${hay.count("abc")} ${hay.count("a")} ${hay.count("zzzz")}");
    i64 parts = 0;
    for (string p : "a,b,,c,".splitBy(",")) { print("[${p}]"); parts++; }
    for (string p : "x--y----z".splitBy("--")) { print("<${p}>"); }
    println(" ${parts}");

    // A map with keys of every length to 20, each differing from the others in one byte.
    u8[.*][..>] keys = [];
    for (i64 len = 0; len <= 20; len++) {
        for (i64 k = 0; k <= len; k++) {
            u8[..>] key = [];
            key.append(base[0:len]);
            if (k < len) { key[k] = 'Z'; }
            keys +> key;
        }
    }
    hashmap<string, i64> m = [];
    i64 added = 0;
    for (string key : keys) {
        m.getOrAdd(key, 0)++;
        added++;
    }
    for (string key : keys) { m.getOrAdd(key, 0)++; }
    i64 twos = 0;
    for (var& e : m) { if (e.value == 2) { twos++; } }
    println("map: ${added} keys, ${m.len} held, ${twos} seen twice");

    // Records whose lengths take one varint byte and two.
    u8[.*][..>] words = [];
    for (i64 len = 0; len < 400; len += 9) { words +> base[0:len % 300]; }
    i64 total = 0;
    for (string w : words) { total += w.len * 1000 + (w.len > 0 ? w[w.len - 1] : 0); }
    println("records: ${words.len} ${total}");

    // Every byte outside the alphabet makes decoding fail, in either alphabet.
    i64 rejected = 0;
    i64 urlRejected = 0;
    for (i64 b = 0; b < 256; b++) {
        u8[..>] text = [];
        text.append("QUJDRA");
        text +> (u8) b;
        text.append("B");
        u8[..>] out = [];
        if (@base64.decode(text, out) != 0) { rejected++; }
        if (@base64.decodeUrl(text, out) != 0) { urlRejected++; }
    }
    println("base64: ${rejected} ${urlRejected}");
}
"##,
        "equal and copy: 2280 checks, 0 wrong\n\
         find: 2790 checks, 0 wrong, 195 11 6 14 0\n\
         [a][b][][c][]<x><y><><z> 5\n\
         map: 231 keys, 231 held, 231 seen twice\n\
         records: 45 5614802\n\
         base64: 192 192\n",
    );
}

/// base64 (STDLIB §8.4) at every length a group loop can end on, through whichever of the
/// runtime's loops this CPU runs, against Python's base64: and invalid text, which leaves
/// `out` as it was.
#[test]
fn base64_round_trips_and_rejects() {
    prints(
        r##"import zink.encoding.*;

u64 fnv(string s) {
    u64 h = 14695981039346656037;
    for (u8 b : s) { h = (h ^ b) * 1099511628211; }
    return h;
}

void main() {
    u8[..>] data = [];
    for (i64 i = 0; i < 1000; i++) { data +> (u8) ((i * 37 + i / 7) % 256); }
    u8[.] e = base64.encode(data);
    u8[.] u = base64.encodeUrl(data);
    println("${e.len} ${fnv(e)} ${u.len} ${fnv(u)}");
    i64 bad = 0;
    for (i64 n = 0; n <= 300; n++) {
        string part = data[0:n];
        u8[..>] back = [];
        @_ = base64.decode(base64.encode(part), back);
        u8[..>] back2 = [];
        @_ = base64.decodeUrl(base64.encodeUrl(part), back2);
        if (back != part || back2 != part) { bad++; }
    }
    u8[..>] out = [];
    out.append("keep");
    println("${bad} ${@base64.decode("QUJDRA==QUJD", out)} ${@base64.decode("QR==", out)} ${@base64.decodeUrl("QQ=", out)} ${out.len} ${@base64.decodeUrl("QUJDRA", out)} ${out.len}");
}
"##,
        "1336 1823936081133242498 1334 6181256220034438240\n0 22 22 22 4 0 8\n",
    );
}

/// An enum value takes the narrowest unsigned integer that holds its constants (DESIGN
/// §11): a byte for up to 256, two bytes beyond. It works as a field of a fixed-size and of
/// a variable-size class, as a map key, sorted, compared and switched on.
#[test]
fn enums_take_the_narrowest_width() {
    let mut big = String::from("enum Big { ");
    for i in 0..300 {
        big.push_str(&format!("B{}, ", i));
    }
    big.push_str("}\n");
    let text = format!(
        r##"enum Kind {{ TICK, TRADE, NOTE }}
{big}
class Ev {{ i64 at; Kind kind; i32 n; }}
class Tagged {{ u8[.*] text; Kind kind; Big big; i16 x; }}

string name(Kind k) {{
    return switch (k) {{ case TICK => "tick"; case TRADE => "trade"; case NOTE => "note"; }};
}}

void main() {{
    Ev[..>] evs = [];
    for (i64 i = 0; i < 9; i++) {{
        Kind k = i % 3 == 0 ? Kind.NOTE : i % 3 == 1 ? Kind.TRADE : Kind.TICK;
        evs +> new Ev(i, k, (i32) i);
    }}
    i64 trades = 0;
    hashmap<Kind, i64> counts = [];
    for (Ev& e : evs) {{
        if (e.kind == Kind.TRADE) {{ trades += e.n; }}
        i64& c = counts.getOrAdd(e.kind, 0);
        c += e.at;
    }}
    Kind[..>] ks = [];
    for (Ev& e : evs) {{ ks +> e.kind; }}
    ks.sort();
    Tagged[..>] tags = [];
    tags +> new Tagged("first", Kind.NOTE, Big.B299, 7);
    tags +> new Tagged("second", Kind.TICK, Big.B43, -7);
    for (Tagged& t : tags) {{
        println("${{t.text}} ${{name(t.kind)}} ${{t.big == Big.B299}} ${{t.big == Big.B43}} ${{t.x}}");
    }}
    i64 tick @e1 = counts.get(Kind.TICK);
    i64 note @e2 = counts.get(Kind.NOTE);
    println("${{trades}} ${{tick}} ${{note}} ${{name(ks[0])}} ${{name(ks[8])}} ${{Kind.TICK < Kind.NOTE}} ${{Big.B43 < Big.B299}}");
}}
"##
    );
    prints(
        &text,
        "first note true false 7\nsecond tick false true -7\n12 15 9 tick note true true\n",
    );
    let file = SourceFile::new("prog.zn", &text);
    let (_, c) = zink::compile_c(std::slice::from_ref(&file));
    let c = c.expect("compiles");
    assert!(c.contains("uint8_t f_kind;"), "Kind should be one byte");
    assert!(c.contains("uint16_t f_big;"), "Big should be two bytes");
    // Largest first, so the one-byte field doesn't pad the 16-byte struct out to 24.
    assert!(c.contains("int64_t f_at; int32_t f_n; uint8_t f_kind;"));
}

/// `status.name` (STDLIB §1.4): the names of DESIGN §7.7, the C name of an operating-system
/// code without one, and "" for a program's own codes.
#[test]
fn status_names() {
    prints(
        r##"void main() {
    println("${status.name(0)} ${status.name(2)} ${status.name(3)} ${status.name(200)} [${status.name(230)}] ${status.message(200)}");
}
"##,
        "OK NOT_FOUND ESRCH ABANDONED [] Abandoned: a failure inside wasn't read\n",
    );
}

/// `return ## e;` passes on `e`'s value and the status `@` reads from it (DESIGN §7.2): a
/// call's, an index's, a `void` call's in a function or constructor that returns nothing,
/// and an array's built where the caller says. The trail says it was passed on.
#[test]
fn return_pass_on() {
    let exe = build(
        r##"import zink.hash.*;

i64 parsePort(string s) {
    i64 n @err = i64.parse(s);
    if (err) {
        return #status.INVALID 0;
    }
    return n;
}

i64 portOf(string addr) {
    i64 colon @none = addr.find(":");
    if (none) {
        return #status.MISSING 0;
    }
    return ## parsePort(addr[colon + 1:addr.len]);
}

i64[..~] stack = [];

void push(i64 x) {
    if (x < 0) {
        return #status.INVALID;
    }
    stack +> x;
}

void pushThen(i64 x) {
    push(1);
    return ## push(x);
}

u8[.] digest(string s, i64 n) {
    return ## hash.shake_128(s, n);
}

i64 at(i64 i) {
    return ## stack[i];
}

class Box {
    i64 v;
    new(i64 x) {
        v = x;
        return ## push(x);
    }
}

void main(string[.] args) {
    i64 p @err @@@t = portOf("example.com:http");
    println("${p} ${status.name(err)} ${t.len} ${t[1].function} ${t[1].line} ${t[1].unread}");
    i64 q @e2 = portOf("example.com:8080");
    @e2 = pushThen(-1);
    println("${q} ${status.name(e2)} ${stack.len}");
    u8[.] d @e3 = digest("abc", 4);
    u8[.] d2 @e4 = digest("abc", -1);
    println("${d.len} ${e3} ${d2.len} ${status.name(e4)}");
    i64 a @e5 = at(0);
    i64 b @e6 = at(9);
    Box k @e7 = new Box(-5);
    println("${a} ${e5} ${b} ${status.name(e6)} ${status.name(e7)}");
    if (args[0] == "stop") { i64 x = portOf("y:z"); }
}
"##,
    );
    let r = exec(&exe, &["go"]);
    assert_eq!(
        r.stdout,
        "0 INVALID 2 portOf 16 false\n8080 INVALID 1\n4 0 0 INVALID\n1 0 0 RANGE INVALID\n"
    );
    let r = exec(&exe, &["stop"]);
    assert_eq!(
        r.stderr,
        "prog.zn:62: `portOf` failed with INVALID, and nothing read its status\n    `parsePort` returned it at prog.zn:6\n    `portOf` passed it on at prog.zn:16\n"
    );
}

/// Every function value can fail (DESIGN §9.3): a call through one gives the lambda's
/// status, which `@` reads, and which stops the program when nothing does. A lambda passed
/// to a built-in fails that call, which stops partway: `sort`, `fold`, `findIndex`, `any`.
/// `return ##` works in a lambda, and the trail names it.
#[test]
fn lambdas_can_fail() {
    let exe = build(
        r##"i64 applyTwice(func(i64 => i64) f, i64 x) {
    i64 once @err = f(x);
    if (err) {
        return #err 0;
    }
    return ## f(once);
}

bool even(i64 x) {
    if (x < 0) {
        return #status.RANGE false;
    }
    return x % 2 == 0;
}

void main(string[.] args) {
    func(i64 => i64) half = (i64 x) => {
        if (x % 2 != 0) {
            return #status.INVALID 0;
        }
        return x / 2;
    };
    func(string => i64) parse = (string s) => { return ## i64.parse(s); };
    i64 a @e1 = half(8);
    i64 b @e2 = half(3);
    i64 c @e3 = applyTwice(half, 12);
    i64 d @e4 @@@t = applyTwice(half, 6);
    i64 p @e5 = parse("42");
    i64 q @e6 = parse("x");
    println("${a} ${e1} ${b} ${e2} ${c} ${e3} ${d} ${e4} ${p} ${e5} ${q} ${e6} ${t[0].function}:${t[0].line} ${t[1].function}:${t[1].line}");
    i64[..>] xs = [5, 3, 7, 1];
    u8 s1 = 0;
    @s1 = xs.sort((i64 x, i64 y) => {
        if (x == 7) {
            return #status.INVALID false;
        }
        return x < y;
    });
    i64 sum @s2 = xs.fold(0, (i64 acc, i64 x) => {
        if (x > 6) {
            return #status.RANGE acc;
        }
        return acc + x;
    });
    i64 at @s3 = xs.findIndex((i64 x) => { return ## even(x - 4); });
    bool any @s4 = xs.any((i64 x) => { return ## even(x - 2); });
    i64 at2 @s5 = xs.findIndex((i64 x) => { return ## even(x + 1); });
    println("${status.name(s1)} ${xs} ${sum} ${status.name(s2)} ${at} ${status.name(s3)} ${any} ${status.name(s4)} ${at2} ${s5}");
    if (args[0] == "value") { i64 z = half(5); }
    if (args[0] == "sort") { xs.sort((i64 x, i64 y) => { if (x == 7) { return #status.INVALID false; } return x < y; }); }
}
"##,
    );
    let r = exec(&exe, &["go"]);
    assert_eq!(
        r.stdout,
        "4 0 0 22 3 0 0 22 42 0 0 22 lambda:19 applyTwice:6\nINVALID [3, 5, 1, 7] 0 RANGE 0 RANGE false RANGE 0 0\n"
    );
    let r = exec(&exe, &["value"]);
    assert_eq!(
        r.stderr,
        "prog.zn:49: `half` failed with INVALID, and nothing read its status\n    `lambda` returned it at prog.zn:19\n"
    );
    let r = exec(&exe, &["sort"]);
    assert_eq!(
        r.stderr,
        "prog.zn:50: `sort` failed with INVALID, and nothing read its status\n    `lambda` returned it at prog.zn:50\n"
    );
}
