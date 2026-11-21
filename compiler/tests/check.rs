//! Type checker tests: programs that must check cleanly, and one test per
//! rule that must be rejected, with the message it gives.

use zink::diag::{Severity, render};
use zink::source::SourceFile;

fn diags(text: &str) -> Vec<(Severity, String)> {
    let file = SourceFile::new("test.zn", text);
    let out = zink::check(std::slice::from_ref(&file));
    out[0]
        .iter()
        .map(|d| {
            (
                d.severity,
                format!(
                    "{}{}",
                    d.message,
                    d.help
                        .as_ref()
                        .map_or(String::new(), |h| format!(" | {}", h))
                ),
            )
        })
        .collect()
}

fn errors(text: &str) -> Vec<String> {
    diags(text)
        .into_iter()
        .filter(|d| d.0 == Severity::Error)
        .map(|d| d.1)
        .collect()
}

fn warnings(text: &str) -> Vec<String> {
    diags(text)
        .into_iter()
        .filter(|d| d.0 == Severity::Warning)
        .map(|d| d.1)
        .collect()
}

/// Wraps statements in `void main()`.
fn body(stmts: &str) -> String {
    format!("void main() {{\n{}\n}}\n", stmts)
}

#[track_caller]
fn ok(text: &str) {
    let file = SourceFile::new("test.zn", text);
    let out = zink::check(std::slice::from_ref(&file));
    if !out[0].is_empty() {
        let shown: String = out[0].iter().map(|d| render(&file, d)).collect();
        panic!("expected no diagnostics, got:\n{}", shown);
    }
}

#[track_caller]
fn rejects(text: &str, needle: &str) {
    let errs = errors(text);
    assert!(
        errs.iter().any(|e| e.contains(needle)),
        "expected an error containing {:?}, got {:#?}",
        needle,
        errs
    );
}

#[track_caller]
fn warns(text: &str, needle: &str) {
    let ws = warnings(text);
    assert!(
        ws.iter().any(|e| e.contains(needle)),
        "expected a warning containing {:?}, got {:#?}",
        needle,
        ws
    );
    assert!(
        errors(text).is_empty(),
        "expected no errors, got {:#?}",
        errors(text)
    );
}

// ---------------------------------------------------------------- whole programs

#[test]
fn example_programs_check() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "zn") {
            let text = std::fs::read_to_string(&path).unwrap();
            let errs = errors(&text);
            assert!(
                errs.is_empty(),
                "{} had errors: {:#?}",
                path.display(),
                errs
            );
            // The only warning allowed is the one about unwritten library packages.
            for w in warnings(&text) {
                assert!(
                    w.contains("isn't written yet") || w.contains("aren't written yet"),
                    "{}: {}",
                    path.display(),
                    w
                );
            }
        }
    }
}

#[test]
fn grammar_complete_example_checks() {
    let grammar =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../GRAMMAR.md")).unwrap();
    let section = grammar.find("## 7. A complete example").unwrap();
    let start = grammar[section..].find("```\n").unwrap() + section + 4;
    let end = grammar[start..].find("\n```").unwrap() + start;
    ok(&grammar[start..end]);
}

// ---------------------------------------------------------------- literals and numbers (DESIGN §9.1, §9.2)

#[test]
fn literals_take_their_type_from_where_they_are_used() {
    ok(&body(
        "u8 b = 200; f64 x = 0.5; var i = 0; i64 big = 100000 * 100000; u32 c = 'a'; u8 comma = ','; f32 h = 1.5f32; i64 m = -1;",
    ));
    rejects(&body("u8 b = 300;"), "300 doesn't fit in u8");
    // 128-bit literals (DESIGN §9.1): above `i128.MAX`, only a `u128` holds one.
    ok(&body(
        "u128 a = 340282366920938463463374607431768211455; i128 b = -170141183460469231731687303715884105728; i128 c = 18446744073709551616; u128 d = 7u128;",
    ));
    rejects(
        &body("u8 a = 340282366920938463463374607431768211455;"),
        "340282366920938463463374607431768211455 doesn't fit in u8",
    );
    rejects(
        &body("i128 b = 170141183460469231731687303715884105728;"),
        "170141183460469231731687303715884105728 doesn't fit in i128",
    );
    rejects(
        &body("i128 b = 170141183460469231731687303715884105728i128;"),
        "doesn't fit in i128",
    );
    rejects(
        &body("i64 e = 18446744073709551616;"),
        "18446744073709551616 doesn't fit in i64",
    );
    // Widening to 128 bits is implicit; a third, wider type is never chosen.
    ok(&body(
        "i64 x = 1; u64 y = 2; i128 a = x; i128 b = y; u128 c = y; i128 s = a + y;",
    ));
    rejects(
        &body("i64 x = 1; u64 y = 2; var s = x + y;"),
        "neither i64 nor u64 holds every value of the other",
    );
    rejects(
        &body("i64[~.~2] q = [1, 2, 3];"),
        "this literal has 3 elements, and the array holds 2",
    );
    rejects(
        &body("u8[~4] g = \"hello\";"),
        "this literal has 5 bytes, and the array holds 4",
    );
    ok(&body("u8[~5] g = \"hello\"; i64[..~2] p = [1, 2];"));
    ok(&body(
        "i64[~.~4] q = [1, 2]; i64[:] a = q; i64[:] b = q[1:]; u8[~8] g = \"ab\"; string t = g; bool s = g.startsWith(\"a\");",
    ));
    rejects(
        &body("i64[.~.] c = []; i64[:] d = c;"),
        "doesn't keep its elements in order in one block",
    );
    rejects(&body("u8 b = 200 + 100;"), "300 doesn't fit in u8");
    rejects(&body("u8 b = -1;"), "-1 doesn't fit in u8");
    rejects(&body("u8 e = 'é';"), "isn't ASCII");
    rejects(&body("u16 c = '\\u{1F600}';"), "doesn't fit in u16");
    rejects(&body("i64 x = 1.5;"), "expected i64");
    rejects(&body("i64 x = 300u8;"), "");
    ok(&body("i64 x = 300i64;"));
}

#[test]
fn implicit_conversions_never_lose_information() {
    ok(&body(
        "i8 a = 1; i32 b = a; u8 c = 1; i32 d = c; u32 e = 1; i64 f = e; f64 g = e; f32 h = 1.0f32; f64 k = h;",
    ));
    rejects(
        &body("i64 x = 1; i32 y = x;"),
        "i64 doesn't convert to i32 implicitly",
    );
    rejects(
        &body("i32 x = 1; u32 y = x;"),
        "i32 doesn't convert to u32 implicitly",
    );
    rejects(&body("u64 x = 1; i64 y = x;"), "u64 doesn't convert to i64");
    rejects(&body("f64 x = 1.0; i64 y = x;"), "truncates toward zero");
    rejects(&body("i64 n = 1; f64 x = n;"), "i64 doesn't convert to f64");
}

#[test]
fn mixed_integer_arithmetic() {
    ok(&body(
        "u8 b = 1; i32 n = 2; i32 r = n + b; u32 c = 3; i64 len = 4; i64 s = len - c; u32 d = c - 1; u8 e = b + 1;",
    ));
    rejects(
        &body("u32 c = 1; i32 d = 2; var x = c + d;"),
        "neither u32 nor i32 holds every value of the other",
    );
    rejects(
        &body("u64 h = 1; i64 len = 2; var x = h % len;"),
        "neither u64 nor i64 holds every value of the other",
    );
    rejects(
        &body("i32 n = 1; i64 len = 2; n += len;"),
        "i64 doesn't convert to i32 implicitly",
    );
    rejects(
        &body("i64 n = 1; f64 x = 2.0; var y = n * x;"),
        "neither i64 nor f64 holds every value of the other",
    );
    ok(&body("i32 n = 1; f64 x = 2.0; f64 y = n * x;"));
    rejects(
        &body("i64 n = 1; var y = n * 2.5;"),
        "a float literal with an integer",
    );
    rejects(&body("f64 x = 1.0; var y = x % 2.0;"), "needs integers");
    rejects(&body("i64 n = 1; var y = n / 0;"), "divides by zero");
    rejects(&body("u8 b = 1; var y = b + 300;"), "300 doesn't fit in u8");
}

#[test]
fn comparisons() {
    ok(&body(
        "u32 c = 0; i32 d = -1; bool x = d < c; u64 h = 1; i64 n = 2; bool y = h == n; u8 b = 1; bool z = b == ',';",
    ));
    rejects(
        &body("u8 b = 1; bool x = b == 300;"),
        "300 doesn't fit in u8",
    );
    rejects(&body("u8 b = 1; bool x = b == 'é';"), "isn't ASCII");
    rejects(&body("bool x = \"a\" == 1;"), "can't compare");
    ok(&body(
        "string a = \"x\"; u8[..>] buf = []; bool x = a == buf; bool y = a < \"y\";",
    ));
}

#[test]
fn casts() {
    ok(&body(
        "i64 n = 300; u8 a = (u8) n; u8 b = (u8%) n; u8 c = (u8|) n; u8 d = (u8%) 300; f64 x = (f64) n; i64 t = (i64) 3.7; u8 s = (u8|) x;",
    ));
    rejects(&body("u8 a = (u8) 300;"), "300 doesn't fit in u8");
    rejects(&body("f64 x = 1.5; u8 a = (u8%) x;"), "a float can't wrap");
    rejects(
        &body("i64 n = 1; f64 x = (f64%) n;"),
        "don't apply to a float target",
    );
    rejects(
        &body("string s = \"a\"; i64 n = (i64) s;"),
        "casts exist only for numbers",
    );
    rejects(&body("i64 n = 1; bool b = (bool) n;"), "no cast to bool");
}

#[test]
fn bounds_are_not_types() {
    rejects(&body("int x = 5;"), "write `i64` or `i32`");
    rejects(&body("float x = 5.0;"), "write `f64`");
    rejects("class number { }\n", "built-in name");
}

// ---------------------------------------------------------------- conditions and statements (DESIGN §9.4)

#[test]
fn conditions() {
    ok(&body(
        "u8 err = 0; if (err) { } bool b = true; while (!b) { } u64 n = 1; var x = n ? 1 : 2;",
    ));
    rejects(
        &body("i32 n = 1; if (n) { }"),
        "a signed integer (i32) isn't a condition",
    );
    rejects(
        &body("f64 x = 1.0; if (x) { }"),
        "a condition is a bool or an unsigned integer",
    );
    rejects(
        &body("i64 x = 0; if (x = 0) { }"),
        "an assignment can't be a condition",
    );
    rejects(
        &body("u8 err = 0; bool b = err;"),
        "expected bool, found u8",
    );
    rejects(&body("while (1) { }"), "a number isn't a condition");
}

#[test]
fn statements_that_do_nothing() {
    rejects(&body("i64 a = 1; a + 1;"), "does nothing");
    rejects(&body("break;"), "`break` is only for loops");
    rejects(
        &body("for (;;) { continue nope; }"),
        "no enclosing loop is labeled `nope`",
    );
    ok(&body(
        "outer: for (var i = 0; i < 3; i++) { for (;;) { break outer; } }",
    ));
}

#[test]
fn names() {
    rejects(&body("x = 1;"), "unknown name `x`");
    rejects(&body("foo(1);"), "unknown function `foo`");
    rejects(
        &body("i64 a = 1; i64 a = 2;"),
        "already declared in this block",
    );
    rejects(&body("i64 x = _;"), "`_` can't be read");
    rejects(&body("_ += 1;"), "compound assignment");
    rejects(
        "class C { i64 n; static i64 f() { return n; } }\n",
        "`f` is static",
    );
    rejects(
        "class C { static void f() { var c = this; } }\n",
        "`this` is only in",
    );
    ok("class Item { i64 n; }\nvoid main() { i64 Item = 3; i64 y = Item + 1; }\n");
}

#[test]
fn missing_returns() {
    rejects(
        "i64 f(bool b) { if (b) { return 1; } }\n",
        "can reach its end without returning",
    );
    ok("i64 f(bool b) { if (b) { return 1; } else { return 2; } }\n");
    ok("i64 f() { while (true) { } }\n");
    ok("i64 f(i64 n) { for (;;) { if (n > 0) { return n; } } }\n");
    rejects("i64 f() { while (true) { break; } }\n", "can reach its end");
    ok("i64 f() { fail(\"never\"); }\n");
    ok("i64 f(i64 n) { switch (n) { case 0 => { return 1; } default => { return 2; } } }\n");
    rejects("void f() { return 1; }\n", "returns nothing");
    rejects("i64 f() { return; }\n", "needs a value");
    rejects(
        "i64 f() { return #1; }\n",
        "needs a value, even with a status",
    );
}

#[test]
fn var_needs_a_type_to_copy() {
    rejects(&body("var x = null;"), "doesn't say which type");
    rejects(
        &body("var a = [];"),
        "`[]` takes its type from where it's used",
    );
    rejects(&body("var x;"), "needs an initializer");
    ok(&body("var a = [1, 2, 3]; i64 first = a[0];"));
}

// ---------------------------------------------------------------- arrays (DESIGN §4)

#[test]
fn array_capabilities() {
    ok(&body(
        "i64[..>] a = []; a +> 1 +> 2; a.push(3); i64[..~] b = []; @_ = b ->; u32[.~.] t = []; t +^ 5; t.seek(0); i64 c = t.cursor;",
    ));
    rejects(
        &body("i64[.] a = [1, 2]; a.push(3);"),
        "`push` needs an array that can insert at the end",
    );
    rejects(
        &body("i64[.] a = [1, 2]; a +> 3;"),
        "`+>` needs an array that can insert at the end",
    );
    rejects(
        &body("i64[..>] a = []; @_ = a ->;"),
        "`->` needs an array that can remove at the end",
    );
    rejects(&body("i64[..>] a = []; a.seek(0);"), "needs a cursor");
    rejects(
        &body("i64[..>] a = []; i64 c = a.cursor;"),
        "`cursor` needs a cursor",
    );
    rejects(
        &body("string s = \"abc\"; s.sort();"),
        "needs an array or a writable view",
    );
    rejects(
        &body("string s = \"abc\"; s +> 'x';"),
        "a slice can't change its length",
    );
    rejects(&body("i64[..>] a = []; i64 x = a[^0];"), "past the end");
    rejects(
        &body("i64[..>] a = []; string s = \"x\"; i64 x = a[s];"),
        "an index is an integer",
    );
}

#[test]
fn slices_and_views() {
    ok(&body(
        "u8[..>] buf = []; string s = buf; string t = buf[1:]; u8[:]& w = buf[0:2]; w .= buf[2:4]; i64[..>] xs = []; xs[1:].sort();",
    ));
    rejects(
        &body("u8[:]& w = \"text\";"),
        "a read-only slice can't become a writable view",
    );
    rejects(
        &body("u32[.~.] t = []; u32[:] s = t[0:1];"),
        "can't be sliced",
    );
    rejects(
        &body("u32[.~.] t = []; u32[:] s = t;"),
        "can't be viewed as a slice",
    );
}

#[test]
fn sorting_needs_ordered_elements() {
    let row = "class Row { string key; f64 num; }\n";
    ok(&format!(
        "{}{}",
        row,
        body(
            "Row[..>] rows = []; rows.sort((Row a, Row b) => a.key < b.key); rows.sortBy((Row r) => r.key); rows.radixSortBy((Row r) => r.num);"
        )
    ));
    rejects(
        &format!("{}{}", row, body("Row[..>] rows = []; rows.sort();")),
        "needs elements that are `ordered`",
    );
    rejects(
        &format!(
            "{}{}",
            row,
            body("Row[..>] rows = []; rows.radixSortBy((Row r) => r);")
        ),
        "a radix sort's key is a number or a string",
    );
    rejects(
        &body("u8[..>] bytes = []; bytes.sort((i64 a, i64 b) => a < b);"),
        "expected func(u8, u8 => bool)",
    );
}

// ---------------------------------------------------------------- functions and overloading (DESIGN §6.5)

#[test]
fn overloads_are_ranked() {
    let defs = "i64 f(i64 x) { return x; }\nstring f(u64 x) { return \"u\"; }\nf64 f(f64 x) { return x; }\n";
    ok(&format!(
        "{}{}",
        defs,
        body("i64 a = f(42); u32 c = 1; string b = f(c); f32 r = 1.0f32; f64 d = f(r);")
    ));
    rejects(
        &format!("{}{}", defs, body("u32 c = 1; i64 a = f(c);")),
        "expected i64, found string",
    );
    let amb = "void g(i32 a, i64 b) { }\nvoid g(i64 a, i32 b) { }\n";
    rejects(
        &format!("{}{}", amb, body("i32 x = 1; g(x, x);")),
        "is ambiguous",
    );
}

#[test]
fn overload_declarations_must_differ_in_a_type() {
    rejects(
        "class Item { i64 n; }\nvoid f(Item x) { }\nvoid f(Item& x) { }\n",
        "declared twice with the same parameter types",
    );
    ok(
        "void f(string s) { }\nvoid f(u8[..>]& b) { }\nvoid main() { u8[..>] buf = []; f(buf); f(\"x\"); }\n",
    );
}

#[test]
fn arguments() {
    let f = "i64 f(i64 x) { return x; }\n";
    rejects(
        &format!("{}{}", f, body("f(1, 2);")),
        "`f` takes 1 argument, not 2",
    );
    rejects(
        &format!("{}{}", f, body("f(\"x\");")),
        "expected i64, found string",
    );
    rejects(
        "class Item { i64 n; }\nItem make() { return new Item(1); }\nvoid touch(Item& i) { }\nvoid main() { touch(make()); }\n",
        "temporary value",
    );
    warns(
        "void touch(i64& n) { }\nvoid main() { i64 n = 1; touch(&n); }\n",
        "`&` isn't needed",
    );
}

#[test]
fn lambdas_carry_their_types() {
    ok(&body(
        "func(i32, i32 => i32) add = (i32 a, i32 b) => a + b; i32 x = add(2, 3); func(i64, i64 => i64) first = (i64 a, i64 _) => a;",
    ));
    rejects(
        &body("func(i32, i32 => i32) add = (i64 a, i64 b) => a + b;"),
        "expected func(i32, i32 => i32)",
    );
    rejects(
        &body("func(i64 => i64)? f = null; i64 y = f(1);"),
        "may be null",
    );
}

// ---------------------------------------------------------------- generics (DESIGN §10)

#[test]
fn generic_inference() {
    let sum = "<T implements number> T sum(T[:] xs) { T total = 0; for (T x : xs) { total += x; } return total; }\n";
    ok(&format!(
        "{}{}",
        sum,
        body(
            "i64[..>] ints = []; i64 a = sum(ints); f64[..>] fs = []; f64 b = sum(fs); i64 m = max(3, 5); u8 x = 1; u8 y = max(x, 2);"
        )
    ));
    rejects(
        &body("u8 a = 1; u32 b = 2; var m = max(a, b);"),
        "would be both u8 and u32",
    );
    rejects(
        "class Row { i64 n; }\nvoid main() { Row a = new Row(1); Row b = new Row(2); var m = max(a, b); }\n",
        "and Row isn't",
    );
}

#[test]
fn generic_bodies_use_only_their_bounds() {
    rejects(
        "<T implements number> T f(T x) { T k = 1000; return k; }\n",
        "doesn't fit every type `T` can be",
    );
    ok("<T implements number> T f(T x) { T k = 100; return (T) 1000 + x * 2; }\n");
    rejects(
        "<T implements ordered> T f(T x) { return x * 2; }\n",
        "needs `T` to be `number`",
    );
    rejects(
        "<T implements number> T f(T x) { return x % 2; }\n",
        "needs `T` to be `int`",
    );
    rejects(
        "<T> bool f(T a, T b) { return a < b; }\n",
        "`T` isn't `ordered`",
    );
    ok("<T implements sint> T neg(T x) { return x * -1; }\n");
    ok(
        "<K implements hashable> i64 distinct(K[:] xs) { hashset<K> seen = []; for (K x : xs) { seen.add(x); } return seen.len; }\n",
    );
    rejects(
        "<K> i64 distinct(K[:] xs) { hashset<K> seen = []; return seen.len; }\n",
        "a hashset element must be hashable",
    );
}

#[test]
fn ordered_classes() {
    let v = "class V implements ordered { u32 major; i32 compareTo(V o) { return major < o.major ? -1 : (major > o.major ? 1 : 0); } }\n";
    ok(&format!(
        "{}{}",
        v,
        body("V a = new V(1); V b = new V(2); bool x = a < b; var m = max(a, b);")
    ));
    rejects(
        "class V { u32 major; }\nvoid main() { V a = new V(1); bool x = a < a; }\n",
        "`V` isn't ordered",
    );
    rejects(
        "class V implements ordered { u32 major; }\n",
        "needs a `compareTo` method",
    );
}

// ---------------------------------------------------------------- classes and interfaces (DESIGN §6)

#[test]
fn interfaces() {
    let shape = "interface Shape { f64 area(); }\n";
    ok(&format!(
        "{}class Circle implements Shape {{ f64 r; f64 area() {{ return r * r; }} }}\n{}",
        shape,
        body(
            "Circle[..>] cs = []; Shape&[..>] shapes = []; shapes.push(cs.push(new Circle(1.0))); f64 t = 0.0; for (Shape& s : shapes) { t += s.area(); if (s instanceof Circle c && c.r > 1.0) { } }"
        )
    ));
    rejects(
        &format!("{}class Sq implements Shape {{ f64 side; }}\n", shape),
        "has no method `area`",
    );
    rejects(
        &format!(
            "{}class Sq implements Shape {{ f64 side; i64 area() {{ return 1; }} }}\n",
            shape
        ),
        "doesn't match",
    );
    rejects(
        &format!("{}{}", shape, body("Shape s;")),
        "is an interface, which isn't a value type",
    );
    rejects(
        &format!("{}{}", shape, body("var s = new Shape();")),
        "can't be constructed",
    );
    rejects(
        "class Item { i64 n; }\nvoid main() { Item i = new Item(1); bool b = i instanceof Item; }\n",
        "tests an interface reference",
    );
}

#[test]
fn constructors() {
    let rect = "class Rect { f64 w; f64 h; new(f64 w, f64 h) { this.w = w; this.h = h; } new(f64 side) { this.new(side, side); } }\n";
    ok(&format!(
        "{}{}",
        rect,
        body(
            "Rect a = new Rect(1.0, 2.0); Rect b = new Rect(3.0); Rect[..>] rs = []; rs.push(new(1.0, 1.0));"
        )
    ));
    rejects(
        &format!("{}{}", rect, body("Rect a = new Rect(\"x\");")),
        "expected f64, found string",
    );
    ok(
        "class Row { string line; f64 num; }\nclass Opts { bool on = false; i64 n = 0; }\nclass Words { string[..>] list; }\nvoid main() { var r = new Row(\"a\", 1.0); var o = new Opts(); var w = new Words(); }\n",
    );
    rejects(
        "class Row { string line; f64 num; }\nvoid main() { var r = new Row(\"a\"); }\n",
        "takes 2 arguments, not 1",
    );
    rejects(
        &body("var x = new(1);"),
        "takes its class from where the value goes",
    );
    rejects(
        "class C { i64 n; void f() { this.new(1); } }\n",
        "only in a constructor",
    );
}

#[test]
fn fields() {
    rejects(
        "class C { i64[..>] a; i64[..>] b; }\n",
        "only one growable field",
    );
    rejects("class C { u32[.~.] text; }\n", "only a root can hold one");
    ok("root R { i64[..>] a; i64[..>] b; u32[.~.] text; }\n");
    rejects(
        "class C { private i64 n; }\nvoid main() { C c = new C(1); i64 x = c.n; }\n",
        "private to `C`",
    );
    rejects(
        "class C { i64 n; }\nvoid main() { C c = new C(1); i64 x = c.m; }\n",
        "has no field `m`",
    );
    rejects(
        "class C { i64 n; }\nvoid main() { C c = new C(1); i64 x = c.n(); }\n",
        "`n` isn't a function; it is i64",
    );
    rejects("class C { i64 n; void delete() { } }\n", "use `defer`");
    rejects(
        &body("hashmap<f64, i64> m = [];"),
        "a hashmap key must be hashable",
    );
    rejects(
        &body("hashmap<string, i64[..>]> m = [];"),
        "hashmap values can't grow",
    );
    rejects(
        &body("i64[..>][..>] m = [];"),
        "an array of growable arrays",
    );
}

#[test]
fn enums_and_switch() {
    let color = "enum Color { RED, GREEN, BLUE }\n";
    ok(&format!(
        "{}{}",
        color,
        body(
            "Color c = Color.RED; string s = switch (c) { case RED => \"r\"; case GREEN, BLUE => \"gb\"; }; switch (c) { case RED => println(\"r\"); default => { } }"
        )
    ));
    rejects(
        &format!(
            "{}{}",
            color,
            body("Color c = Color.RED; string s = switch (c) { case RED => \"r\"; };")
        ),
        "must cover every case",
    );
    rejects(
        &format!("{}{}", color, body("Color c = Color.PINK;")),
        "has no constant `PINK`",
    );
    rejects(
        &format!(
            "{}{}",
            color,
            body("Color c = Color.RED; switch (c) { case PINK => { } default => { } }")
        ),
        "isn't a constant of `Color`",
    );
    rejects(
        &body("i64 n = 1; switch (n) { case 1 => { } case 1 => { } default => { } }"),
        "already covered",
    );
    rejects(
        &body("u8 b = 1; switch (b) { case 300 => { } default => { } }"),
        "300 doesn't fit in u8",
    );
    rejects(
        &body("string s = \"x\"; switch (s) { case 1 => { } default => { } }"),
        "this label is an integer, and the switch is on a string",
    );
    ok(&body(
        "i64 n = 2; string s = switch (n) { case 0 => \"none\"; case 1, 2 => { yield \"few\"; } default => \"many\"; };",
    ));
    rejects(&body("yield 1;"), "`yield` gives the value of a block arm");
}

// ---------------------------------------------------------------- references and null (DESIGN §5)

#[test]
fn null_narrowing() {
    let node = "class Node { i64 value; Node&? next; }\n";
    ok(&format!(
        "{}i64 sum(Node& head) {{ i64 t = 0; Node&? cur = head; while (cur != null) {{ t += cur.value; cur .= cur.next; }} return t; }}\n",
        node
    ));
    ok(&format!(
        "{}i64 f(Node&? n) {{ if (n == null) {{ return 0; }} return n.value; }}\n",
        node
    ));
    ok(&format!(
        "{}i64 f(Node&? n) {{ if (n != null && n.value > 0) {{ return n.value; }} return 0; }}\n",
        node
    ));
    rejects(
        &format!("{}i64 f(Node&? n) {{ return n.value; }}\n", node),
        "may be null",
    );
    rejects(
        &format!("{}i64 f(Node& n) {{ return n.next.value; }}\n", node),
        "narrowing doesn't apply to fields",
    );
    rejects(
        &format!(
            "{}i64 f(Node&? n) {{ if (n != null) {{ n .= n.next; return n.value; }} return 0; }}\n",
            node
        ),
        "may be null",
    );
    rejects(
        &format!("{}void f(Node&? n, Node& m) {{ n = m; }}\n", node),
        "`=` writes through it",
    );
    rejects(
        &body("i64 x = null;"),
        "`null` is only for nullable references",
    );
    rejects(
        &body("i64 x = 1; i64 y = 2; x .= y;"),
        "`.=` rebinds a reference",
    );
}

#[test]
fn references_need_places() {
    ok(
        "class Item { i64 n; }\nvoid main() { Item[..>] items = []; items +> new Item(1); Item& r = items[0]; r .= items[0]; var& v = items[0]; for (Item& it : items) { it.n++; } }\n",
    );
    rejects(
        "class Item { i64 n; }\nItem make() { return new Item(1); }\nvoid main() { Item& r = make(); }\n",
        "temporary value",
    );
    rejects(&body("var& x = 1 + 2;"), "temporary value");
    rejects(
        &body("string s = \"ab\"; for (u8& b : s) { }"),
        "can't be referred to with `&`",
    );
}

// ---------------------------------------------------------------- statuses (DESIGN §7)

#[test]
fn status_writes() {
    ok(
        "i64 f(i64 x) { if (x < 0) { return #status.INVALID 0; } u8 why = 1; if (x == 0) { return #why 0; } return #(x > 9) x; }\n",
    );
    rejects("void f() { return #300; }\n", "a status is a u8");
    rejects(
        "void f() { i64 n = 1; return #n; }\n",
        "`#` takes a u8 or bool variable",
    );
    rejects(
        "void f() { i64 n = 1; return #(n); }\n",
        "`#( ... )` takes a u8 or a bool",
    );
    rejects(
        "void f() { return #status.not_found; }\n",
        "isn't a status code",
    );
    rejects("void main() { return #1; }\n", "`main` can't set a status");
    rejects(
        &body("i64 x = 0; u8 e = @x;"),
        "`@` reads the status of a call",
    );
}

#[test]
fn dropped_statuses_warn() {
    warns(
        &body("i64[..~] a = []; i64 x = a.pop();"),
        "the status of `pop` can be nonzero, and it is dropped",
    );
    warns(&body("i64[..~] a = []; a ->;"), "the status of `->`");
    warns(
        "i64 f() { return #1 0; }\nvoid main() { i64 x = f(); }\n",
        "the status of `f`",
    );
    warns(&body("i64 n = i64.parse(\"1\");"), "the status of `parse`");
    ok(&body(
        "i64[..~] a = []; i64 x @e = a.pop(); i64 y @_ = a.pop(); @_ = a.pop(); u8 s = @a.pop(); if (@a.pop()) { } x @e = a.pop();",
    ));
    ok(&body(
        "i64[..>] a = []; a.push(1); u8[..>] t = []; t.append(\"x\");",
    ));
    warns(
        &body("i64[..>16] a = []; a.push(1);"),
        "the status of `push`",
    );
    rejects(
        &body("i64 n @e = i64.parse(\"1\"); @nope = i64.parse(\"2\");"),
        "`nope` isn't declared",
    );
    rejects(
        &body("i64 n = 0; @n = i64.parse(\"2\");"),
        "a status goes into a u8",
    );
}

#[test]
fn repeat_loops() {
    ok(&body(
        "i64[..~] todo = []; while (i64 t : todo ->) { todo +> t - 1; }",
    ));
    ok(
        "i64 next() { return #1 0; }\nvoid main() { while (i64 x : next()) @why { } if (why != status.END) { } }\n",
    );
    rejects(
        "i64 next() { return 0; }\nvoid main() { while (i64 x : next()) { } }\n",
        "never sets a status, so the loop would never end",
    );
    rejects(
        "void next() { return #1; }\nvoid main() { while (i64 x : next()) { } }\n",
        "returns nothing",
    );
    ok(
        "(string, string) pair() { return #1 (\"a\", \"b\"); }\nvoid main() { while ((string k, string v) : pair()) { } }\n",
    );
}

#[test]
fn iterators() {
    let cd = "class Countdown { i64 n; mut i64 next() { if (n == 0) { return #status.END 0; } n--; return n + 1; } }\n";
    ok(&format!(
        "{}{}",
        cd,
        body(
            "for (i64 x : new Countdown(3)) { } Countdown c = new Countdown(2); while (i64 x : c.next()) { }"
        )
    ));
    rejects(
        &format!(
            "{}{}",
            cd,
            body("Countdown c = new Countdown(3); for (i64 x : c) { }")
        ),
        "a for-each over an iterator variable isn't allowed",
    );
    ok(&body(
        "string text = \"a b\"; for (string l : text.lines()) { } for (u32 c : text.codePoints()) { } hashmap<string, i64> m = []; for (var& e : m) { e.value++; }",
    ));
    rejects(
        &body("hashmap<string, i64> m = []; for (var& e : m) { e.key = \"x\"; }"),
        "a hashmap entry's key can't be changed",
    );
}

// ---------------------------------------------------------------- tuples and text (DESIGN §9.5, §4.11)

#[test]
fn tuples() {
    let split = "(string, string) split(string s) { return (s, \"\"); }\n";
    ok(&format!(
        "{}{}",
        split,
        body(
            "(string a, string b) = split(\"x\"); (a, b) = split(\"y\"); (string w, _) = split(\"z\"); (a, b) = (b, a); _ = split(\"q\");"
        )
    ));
    rejects(
        &format!("{}{}", split, body("var t = split(\"x\");")),
        "must be destructured",
    );
    rejects(
        &format!(
            "{}{}",
            split,
            body("(string a, string b, string c) = split(\"x\");")
        ),
        "this gives 2 results, and there are 3 targets",
    );
    rejects(
        &body("(i64 a, i64 b) = 5;"),
        "only a call with several results",
    );
}

#[test]
fn text() {
    ok(&body(
        "string s = \"a,b\"; i64 n = 1; f64 x = 2.0; println(\"${n} ${x.fixed(2)} ${n.pad(4)} ${s.trim()} ${n.hex()}\"); var parts = s.split(\",\"); (string a, string b) = s.splitFirst(\",\"); bool c = s.startsWith(\"a\");",
    ));
    rejects(
        "class P { i64 x; }\nvoid main() { P p = new P(1); println(\"${p}\"); }\n",
        "a class value can't be printed",
    );
    rejects(
        &body("string a = \"x\"; var b = a + \"y\";"),
        "there's no `+` on strings",
    );
    rejects(
        &body("i64 n = 1; var s = n.fixed(2);"),
        "has no method `fixed`",
    );
    rejects(
        &body("string s = \"a\"; s.nope();"),
        "string has no method `nope`",
    );
}

#[test]
fn main_signature() {
    ok("u8 main(string[.] args) { return 0; }\n");
    rejects(
        "i64 main() { return 0; }\n",
        "`main` returns `void`, or `u8`",
    );
    rejects(
        "void main(i64 n) { }\n",
        "`main` takes no parameters, or `string[.] args`",
    );
}

#[test]
fn defer_rules() {
    rejects(
        &body("defer { return; }"),
        "a deferred statement can't `return`",
    );
    rejects(
        &body("for (;;) { defer { break; } }"),
        "can't `return`, `break` or `continue`",
    );
    ok(&body("defer { for (;;) { break; } }"));
    ok(&body("i64[..~] a = []; defer @_ = a.pop();"));
}

/// Each example with one line deleted, then with one line duplicated: whatever
/// still parses must type-check to completion, with errors rather than a panic.
#[test]
fn edited_examples_never_panic() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "zn") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for i in 0..lines.len() {
            for dup in [false, true] {
                let mut edited: Vec<&str> = lines.clone();
                if dup {
                    edited.insert(i, lines[i]);
                } else {
                    edited.remove(i);
                }
                let file = SourceFile::new("edited.zn", edited.join("\n"));
                let _ = zink::check(std::slice::from_ref(&file));
                checked += 1;
            }
        }
    }
    assert!(checked > 1000);
}

// ---------------------------------------------------------------- definite assignment (DESIGN §6.2)

/// A class `R` with fields `w` and `h`, and the given constructor.
fn rect(ctor: &str) -> String {
    format!(
        "class R {{\n    f64 w;\n    f64 h;\n    {}\n    void show() {{ }}\n}}\n",
        ctor
    )
}

#[test]
fn constructors_assign_every_field() {
    ok(&rect("new(f64 w, f64 h) { this.w = w; this.h = h; }"));
    ok(&rect("new() { w = 1.0; h = 2.0; }"));
    rejects(
        &rect("new(f64 w) { this.w = w; }"),
        "the constructor can end here without assigning `h`",
    );
    rejects(&rect("new() { }"), "without assigning `w` and `h`");
    // Fields that start assigned: initializers, editable arrays, hashmaps.
    ok(
        "root C { i64 n = 0; i64[..>] xs; hashmap<string, i64> m; i64 k; new(i64 k) { this.k = k; } }\n",
    );
    ok("class C { static i64 count; i64 k; new() { k = 1; } }\n");
}

#[test]
fn the_worst_path_counts() {
    // An `if` may go either way.
    rejects(
        &rect("new(bool b) { if (b) { w = 1.0; } h = 2.0; }"),
        "without assigning `w`",
    );
    ok(&rect(
        "new(bool b) { if (b) { w = 1.0; } else { w = 2.0; } h = 2.0; }",
    ));
    // A loop may run zero times, except `while (true)` and `for (;;)`, which end by `break`.
    rejects(
        &rect("new(i64 n) { for (var i = 0; i < n; i++) { w = 1.0; } h = 2.0; }"),
        "without assigning `w`",
    );
    rejects(
        &rect("new(bool b) { while (b) { w = 1.0; } h = 2.0; }"),
        "without assigning `w`",
    );
    ok(&rect("new() { while (true) { w = 1.0; break; } h = 2.0; }"));
    ok(&rect("new() { for (;;) { w = 1.0; break; } h = 2.0; }"));
    rejects(
        &rect("new(bool b) { while (true) { if (b) { break; } w = 1.0; } h = 2.0; }"),
        "without assigning `w`",
    );
    // A `do` loop runs at least once.
    ok(&rect("new(bool b) { do { w = 1.0; } while (b); h = 2.0; }"));
    // `&&` may skip its right side.
    rejects(
        &rect("new(bool b) { if (b && (w = 1.0) > 0.0) { } h = 2.0; }"),
        "without assigning `w`",
    );
    // Only the active branch of an `#if` is checked.
    ok(&rect(
        "new() {\n#if debug\nw = 1.0;\n#else\nw = 2.0;\n#endif\nh = 2.0; }",
    ));
    rejects(
        &rect("new() {\n#if debug\nw = 1.0;\n#endif\nh = 2.0; }"),
        "without assigning `w`",
    );
}

#[test]
fn switches_count_when_every_arm_assigns() {
    ok(&rect(
        "new(i64 n) { switch (n) { case 0 => w = 1.0; default => w = 2.0; } h = 1.0; }",
    ));
    rejects(
        &rect("new(i64 n) { switch (n) { case 0 => w = 1.0; case 1 => w = 2.0; } h = 1.0; }"),
        "without assigning `w`",
    );
    rejects(
        &rect("new(i64 n) { switch (n) { case 0 => w = 1.0; default => { } } h = 1.0; }"),
        "without assigning `w`",
    );
    let e = "enum E { A, B }\n";
    ok(&format!(
        "{}{}",
        e,
        rect("new(E k) { switch (k) { case A => w = 1.0; case B => w = 2.0; } h = 1.0; }")
    ));
    rejects(
        &format!(
            "{}{}",
            e,
            rect("new(E k) { switch (k) { case A => w = 1.0; } h = 1.0; }")
        ),
        "without assigning `w`",
    );
}

#[test]
fn every_successful_exit_counts() {
    rejects(
        &rect("new(bool b) { w = 1.0; if (b) { return; } h = 2.0; }"),
        "the constructor can end here without assigning `h`",
    );
    // A failing `return #code;` needn't assign anything (DESIGN §6.2's example).
    ok(
        "class R {\n    i64 w; i64 h;\n    new(string spec) {\n        i64 v @bad = i64.parse(spec);\n        if (bad) { return #status.INVALID; }\n        w = v; h = v;\n    }\n}\n",
    );
    ok(&rect(
        "new(bool b) { if (b) { return #1; } w = 1.0; h = 2.0; }",
    ));
    // A status that isn't a constant may be 0, which would be a success.
    rejects(
        &rect("new(bool b) { if (b) { return #(b); } w = 1.0; h = 2.0; }"),
        "without assigning `w` and `h`",
    );
    rejects(
        &rect("new(bool b) { if (b) { return #0; } w = 1.0; h = 2.0; }"),
        "without assigning `w` and `h`",
    );
}

#[test]
fn references_have_no_zero_value() {
    let node = "class Node { i64 v; }\n";
    rejects(
        &format!(
            "{}class Link {{ Node& to; i64 n; new(Node& t, bool b) {{ if (b) {{ return #1; }} to = t; n = 1; }} }}\n",
            node
        ),
        "`to` has no zero value, so it must be assigned even when the constructor fails",
    );
    ok(&format!(
        "{}class Link {{ Node& to; i64 n; new(Node& t, bool b) {{ to = t; if (b) {{ return #1; }} n = 1; }} }}\n",
        node
    ));
    // A value holding a reference has none either.
    rejects(
        &format!(
            "{}class Link {{ Node& to; }}\nclass Pair {{ Link a; i64 n; new(Link l, bool b) {{ if (b) {{ return #1; }} a = l; n = 1; }} }}\n",
            node
        ),
        "`a` has no zero value",
    );
    // A nullable reference has one: null.
    ok(&format!(
        "{}class Link {{ Node&? to; i64 n; new(bool b) {{ if (b) {{ return #1; }} to = null; n = 1; }} }}\n",
        node
    ));
}

#[test]
fn a_reference_field_is_bound_by_its_first_assignment() {
    let node = "class Node { i64 v; }\nNode make() { return new Node(1); }\n";
    ok(&format!(
        "{}class Link {{ Node& to; Node&? prev; new(Node& n) {{ this.to = n; this.prev = null; to = n; }} }}\n",
        node
    ));
    rejects(
        &format!(
            "{}class Link {{ Node& to; new() {{ to = make(); }} }}\n",
            node
        ),
        "temporary value",
    );
}

#[test]
fn fields_cant_be_read_before_they_are_assigned() {
    rejects(
        &rect("new() { h = w; w = 1.0; }"),
        "`w` is read before it is assigned",
    );
    rejects(
        &rect("new() { this.h = this.w; this.w = 1.0; }"),
        "`w` is read before it is assigned",
    );
    rejects(
        &rect("new(bool b) { if (b) { w = 1.0; } h = w; w = 2.0; }"),
        "`w` may be read before it is assigned",
    );
    rejects(
        &rect("new() { w += 1.0; h = 1.0; }"),
        "`w` is read before it is assigned",
    );
    ok(&rect("new() { w = 1.0; h = w * 2.0; }"));
    ok(&rect("new() { w = 1.0; h = this.w * 2.0; }"));
}

#[test]
fn this_waits_for_every_field() {
    rejects(
        &rect("new() { w = 1.0; this.show(); h = 2.0; }"),
        "`this` can't be used until every field is assigned, and `h` isn't yet",
    );
    rejects(
        &rect("new() { w = 1.0; show(); h = 2.0; }"),
        "`show` can't be called until every field is assigned",
    );
    rejects(
        &rect("new() { var me = this; w = 1.0; h = 2.0; }"),
        "and `w` and `h` aren't yet",
    );
    ok(&rect("new() { w = 1.0; h = 2.0; this.show(); show(); }"));
}

#[test]
fn this_new_delegates() {
    let base = "new(f64 w, f64 h) { this.w = w; this.h = h; }";
    ok(&rect(&format!(
        "{} new(f64 s) {{ f64 side = s * 2.0; this.new(side, side); show(); }}",
        base
    )));
    ok(&rect(&format!(
        "{} new(string spec) {{ f64 v @bad = f64.parse(spec); if (bad) {{ return #1; }} this.new(v, v); }}",
        base
    )));
    rejects(
        &rect(&format!("{} new(f64 s) {{ w = s; this.new(s, s); }}", base)),
        "`w` is assigned before `this.new(...)`",
    );
    rejects(
        &rect(&format!(
            "{} new(f64 s) {{ this.new(s, s); this.new(s, s); }}",
            base
        )),
        "may already have run",
    );
    rejects(
        &rect(&format!(
            "{} new(f64 s, bool b) {{ if (b) {{ this.new(s, s); }} this.new(s, 1.0); }}",
            base
        )),
        "may already have run",
    );
    rejects(
        &rect(&format!(
            "{} new(f64 s) {{ for (;;) {{ this.new(s, s); break; }} }}",
            base
        )),
        "is inside a loop",
    );
    rejects(
        &rect(&format!("{} new(f64 s) {{ this.new(this.w, s); }}", base)),
        "`w` is read before it is assigned",
    );
    rejects(
        &rect(&format!(
            "{} new(f64 s, bool b) {{ if (b) {{ this.new(s, s); }} }}",
            base
        )),
        "without assigning `w` and `h`",
    );
}

#[test]
fn locals_are_assigned_before_they_are_read() {
    rejects(
        &body("i64 x; println(\"${x}\");"),
        "`x` is read before it is assigned",
    );
    rejects(
        &body("bool c = true; i64 x; if (c) { x = 1; } println(\"${x}\");"),
        "`x` may be read before it is assigned",
    );
    ok(&body(
        "bool c = true; i64 x; if (c) { x = 1; } else { x = 2; } println(\"${x}\");",
    ));
    rejects(
        &body("bool c = true; i64 x; while (c) { x = 1; } println(\"${x}\");"),
        "`x` may be read",
    );
    ok(&body(
        "i64 x; for (;;) { x = 1; break; } println(\"${x}\");",
    ));
    ok(&body(
        "i64 x; outer: for (;;) { for (;;) { x = 1; break outer; } } println(\"${x}\");",
    ));
    rejects(
        &body(
            "i64 x; for (var i = 0; i < 3; i++) { if (i == 0) { continue; } x = i; } println(\"${x}\");",
        ),
        "`x` may be read",
    );
    ok(&body(
        "i64 n = 1; i64 x; switch (n) { case 0 => x = 1; default => x = 2; } println(\"${x}\");",
    ));
    rejects(
        &body("i64 n = 1; i64 x; switch (n) { case 0 => x = 1; } println(\"${x}\");"),
        "`x` may be read",
    );
    ok(&body(
        "i64 n = 1; i64 x; string s = switch (n) { case 0 => { x = 1; yield \"a\"; } default => { x = 2; yield \"b\"; } }; println(\"${x} ${s}\");",
    ));
    rejects(&body("i64 x; x += 1;"), "`x` is read before it is assigned");
    ok(&body(
        "bool c = true; f64 x; var y = c ? (x = 1.0) : (x = 2.0); println(\"${x} ${y}\");",
    ));
    rejects(
        &body("bool c = true; f64 x; var y = c ? (x = 1.0) : 2.0; println(\"${x} ${y}\");"),
        "`x` may be read",
    );
}

#[test]
fn exits_end_a_path() {
    ok("void f(bool c) { i64 x; if (c) { return; } else { x = 1; } println(\"${x}\"); }\n");
    ok("void f(bool c) { i64 x; if (c) { x = 1; } else { fail(\"no\"); } println(\"${x}\"); }\n");
    ok(
        "void f(i64 n) { i64 x; switch (n) { case 0 => x = 1; default => fail(\"no\"); } println(\"${x}\"); }\n",
    );
}

#[test]
fn other_ways_of_assigning() {
    let split = "(string, string) split(string s) { return (s, \"\"); }\n";
    ok(&format!(
        "{}{}",
        split,
        body("string a; string b; (a, b) = split(\"x\"); println(\"${a}${b}\");")
    ));
    ok(&body("u8 err; @err = i64.parse(\"1\"); if (err) { }"));
    ok(&body(
        "i64 v; u8 err; v @err = i64.parse(\"1\"); println(\"${v}\");",
    ));
    rejects(
        &body("i64 x; defer x = 1; println(\"${x}\");"),
        "`x` is read before it is assigned",
    );
    rejects(
        &body("i64 x; func(i64 => i64) f = (i64 a) => a + x; x = 1;"),
        "`x` is read before it is assigned",
    );
}

#[test]
fn reference_locals_are_bound_by_their_first_assignment() {
    let item = "class Item { i64 n; }\n";
    ok(&format!(
        "{}{}",
        item,
        body(
            "Item[..>] items = []; items +> new Item(1); Item& r; r = items[0]; r.n++; r = new Item(2);"
        )
    ));
    rejects(
        &format!(
            "{}{}",
            item,
            body(
                "Item[..>] items = []; items +> new Item(1); bool c = true; Item& r; if (c) { r = items[0]; } r = items[0];"
            )
        ),
        "could bind it or write through it",
    );
    rejects(
        &format!(
            "{}Item make() {{ return new Item(1); }}\n{}",
            item,
            body("Item& r; r = make();")
        ),
        "temporary value",
    );
}

#[test]
fn functions_have_no_zero_value() {
    rejects(
        "class Op { func(i64 => i64) f; i64 n; new(bool b) { if (b) { return #1; } f = (i64 x) => x; n = 1; } }\n",
        "`f` has no zero value",
    );
    ok(
        "class Op { func(i64 => i64) f; i64 n; new(bool b) { f = (i64 x) => x; if (b) { return #1; } n = 1; } }\n",
    );
    ok(
        "class Op { func(i64 => i64)? f; i64 n; new(bool b) { if (b) { return #1; } f = null; n = 1; } }\n",
    );
    rejects(
        "class Op { func(i64 => i64) f; }\nclass Holder { Op op; i64 n; new(Op o, bool b) { if (b) { return #1; } op = o; n = 1; } }\n",
        "`op` has no zero value",
    );
}

// ---------------------------------------------------------------- mut and read-only (DESIGN §5.1, §6.1, §4.10, §9.3)

#[test]
fn plain_parameters_are_read_only_copies() {
    rejects(
        "void f(i64 n) { n = 1; }\n",
        "`n` is a plain parameter, which is a read-only copy",
    );
    rejects("void f(i64 n) { n = 1; }\n", "declare it `i64& n`");
    rejects("void f(i64 n) { n++; }\n", "can't change this");
    rejects("void f(string s) { s = \"x\"; }\n", "declare it `u8[:]& s`");
    rejects(
        "void f(i64[..>] xs) { xs.push(1); }\n",
        "can't call `push` here, since it changes what it's called on: `xs` is a plain parameter",
    );
    rejects("void f(i64[..>] xs) { xs +> 1; }\n", "can't use `+>` here");
    rejects(
        "void f(i64[..~] xs) { @_ = xs ->; }\n",
        "can't use `->` here",
    );
    rejects(
        "class P { i64 x; }\nvoid f(P p) { p.x = 1; }\n",
        "`p` is a plain parameter",
    );
    rejects(
        "u8 main(string[.] args) { args[1:].sort(); return 0; }\n",
        "`args` is a plain parameter",
    );
    rejects(
        "void f(u8 err) { @err = i64.parse(\"1\"); }\n",
        "can't assign to `err`",
    );
    rejects(
        &body("func(i64 => i64) g = (i64 a) => { a = 2; return a; };"),
        "`a` is a plain parameter",
    );
    ok(
        "void f(i64& n) { n = 1; n++; }\nvoid g(i64[..>]& xs) { xs.push(1); xs +> 2; }\nvoid h(i64 n) { i64 m = n; m++; }\n",
    );
}

#[test]
fn only_mut_methods_change_their_object() {
    rejects(
        "class C { i64 n; void bump() { n++; } }\n",
        "`n` is a field, and `bump` isn't `mut`, so it can't change its object",
    );
    rejects(
        "class C { i64 n; void set() { this.n = 1; } }\n",
        "`set` isn't `mut`",
    );
    rejects(
        "class C { i64[..>] xs; void add() { xs.push(1); } }\n",
        "can't call `push` here",
    );
    rejects(
        "class C { i64 n; mut void inc() { n++; } void f() { inc(); } }\n",
        "can't call `inc` here, since it changes its object: `f` isn't `mut`",
    );
    rejects(
        "class C { i64 n; mut void inc() { n++; } void f() { this.inc(); } }\n",
        "can't call `inc` here",
    );
    rejects(
        "void touch(i64& x) { }\nclass C { i64 n; void f() { touch(n); } }\n",
        "can't pass this to `touch`, which may change it",
    );
    rejects(
        "class C { i64 n; void f() { g(this); } static void g(C& c) { } }\n",
        "can't pass this to `g`",
    );
    rejects(
        &format!(
            "{}{}",
            "class C { hashmap<string, i64> m; void f() { m.put(\"a\", 1); } }\n", ""
        ),
        "can't call `put` here",
    );
    ok(
        "class C { i64 n; mut void inc() { n++; this.n += 1; } mut void g() { inc(); this.inc(); } i64 get() { i64 m = n; m++; return m + n; } }\n",
    );
    ok(
        "class C { static i64 count = 0; void f() { count++; } }\ni64 total = 0;\nvoid main() { total = 1; }\n",
    );
    rejects(
        "class C { static mut void f() { } }\n",
        "`f` is static, so it has no object for `mut` to change",
    );
    rejects("mut void f() { }\n", "no object for `mut` to change");
}

#[test]
fn read_only_is_deep() {
    let node = "class Node {\n  i64 v;\n  Node&? next;\n";
    rejects(
        &format!(
            "{}  void f() {{ Node&? n = next; if (n != null) {{ n.v = 1; }} }}\n}}\n",
            node
        ),
        "`n` refers to something that can't be changed here",
    );
    rejects(
        &format!(
            "{}  void f() {{ Node&? n = next; if (n != null) {{ n.v = 1; }} }}\n}}\n",
            node
        ),
        "`next` is a field, and `f` isn't `mut`",
    );
    ok(&format!(
        "{}  mut void f() {{ Node&? n = next; if (n != null) {{ n.v = 1; }} }}\n}}\n",
        node
    ));
    // Walking a list in a method that only reads it.
    ok(&format!(
        "{}  i64 sum() {{ i64 t = v; Node&? cur = next; while (cur != null) {{ t += cur.v; cur .= cur.next; }} return t; }}\n}}\n",
        node
    ));
    // A reference written through can't later be bound to read-only data, whatever the order.
    rejects(
        &format!(
            "{}  void f(Node& a) {{ Node&? r = a; if (r != null) {{ r.v = 1; }} r .= next; }}\n}}\n",
            node
        ),
        "this binds `r` to something that can't be changed here, and `r` is written through elsewhere",
    );
    let item = "class Item { i64 n; }\n";
    rejects(
        &format!(
            "{}void f(Item[..>] items) {{ Item& r = items[0]; r.n = 2; }}\n",
            item
        ),
        "`r` refers to something that can't be changed here",
    );
    rejects(
        &format!(
            "{}void f(Item[..>] items) {{ for (Item& it : items) {{ it.n++; }} }}\n",
            item
        ),
        "`it` refers to something",
    );
    ok(&format!(
        "{}void f(Item[..>]& items) {{ for (Item& it : items) {{ it.n++; }} Item& r = items[0]; r.n = 2; }}\n",
        item
    ));
    rejects(
        &format!(
            "{}void bump(Item& i) {{ i.n++; }}\nvoid f(Item[..>] items) {{ Item& r = items[0]; bump(r); }}\n",
            item
        ),
        "can't pass this to `bump`",
    );
}

#[test]
fn writable_views_need_changeable_arrays() {
    let up = "void up(u8[:]& s) { }\n";
    rejects(
        "void f(u8[..>] buf) { u8[:]& w = buf[0:2]; }\n",
        "can't take a writable view of this: `buf` is a plain parameter",
    );
    ok(&format!(
        "{}void main() {{ u8[..>] b = []; up(b); up(b[1:]); u8[:]& w = b[0:1]; w[0] = 1; }}\n",
        up
    ));
    rejects(
        &format!("{}void g(u8[..>] b) {{ up(b); }}\n", up),
        "can't pass this to `up`",
    );
    rejects(
        &body("string s = \"x\"; s[0] = 1;"),
        "a `string` is a read-only view",
    );
    rejects(
        "class C { u8[..>] buf; void f() { buf[0:2].fill(0); } }\n",
        "can't call `fill` here",
    );
    rejects(
        "void f(u8[..>] a) { u8[..>] b = []; u8[:]& w = b[0:1]; w .= a[0:1]; }\n",
        "can't make this a writable view",
    );
}

#[test]
fn captures_are_read_only() {
    rejects(
        &body("i64 x = 1; i64[..~] xs = []; xs.retain((i64 v) => { x = 2; return true; });"),
        "`x` is captured by this lambda, and captures are read-only",
    );
    rejects(
        &body(
            "i64[..>] seen = []; i64[..~] xs = []; xs.retain((i64 v) => { seen.push(v); return true; });",
        ),
        "can't call `push` here",
    );
    // A captured variable never changes after it's initialized (DESIGN §9.3).
    rejects(
        &body("i64 lim = 1; i64[..~] xs = []; xs.retain((i64 v) => v < lim); lim = 2;"),
        "`lim` is assigned again after a lambda captured it",
    );
    rejects(
        &body("i64 lim = 1; lim = 2; i64[..~] xs = []; xs.retain((i64 v) => v < lim);"),
        "this lambda captures `lim`, which is assigned again",
    );
    rejects(
        &body(
            "i64 lim; for (;;) { lim = 1; break; } i64[..~] xs = []; xs.retain((i64 v) => v < lim);",
        ),
        "assigned again",
    );
    ok(&body(
        "bool c = true; i64 lim; if (c) { lim = 1; } else { lim = 2; } i64[..~] xs = []; xs.retain((i64 v) => v < lim);",
    ));
    // Writing through a captured reference changes what it refers to, not the capture.
    ok(
        "class Item { i64 n; }\nvoid main() { Item[..>] items = []; items +> new Item(1); Item& r = items[0]; i64 k = items.count((Item i) => { r.n++; return true; }); }\n",
    );
}

#[test]
fn returned_references() {
    let item = "class Item { i64 n; }\n";
    rejects(
        &format!(
            "{}Item& first(Item[..>] items) {{ return items[0]; }}\n",
            item
        ),
        "can't return a reference to this: `items` is a plain parameter",
    );
    // A reference into a method's own object is as changeable as the object it's called on.
    let holder = "class Holder { Item[..>] items; Item& first() { return items[0]; } }\n";
    ok(&format!(
        "{}{}void main() {{ Holder h = new Holder(); h.first().n = 1; }}\n",
        item, holder
    ));
    rejects(
        &format!(
            "{}{}void f(Holder h) {{ h.first().n = 1; }}\n",
            item, holder
        ),
        "can't assign to this: `h` is a plain parameter",
    );
    rejects(
        &format!(
            "{}root R {{ pool<Item> p; void f(Handle<Item> h) {{ Item& i @_ = p.get(h); i.n = 1; }} }}\n",
            item
        ),
        "`i` refers to something that can't be changed here",
    );
}

fn has_clang() -> bool {
    std::process::Command::new("clang")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn calling_c_checks() {
    if !has_clang() {
        eprintln!("skipped: no clang to read C headers");
        return;
    }
    // A header that isn't there.
    rejects(
        "extern import nope/missing.h;\nvoid main() {}\n",
        "can't read the C headers: 'nope/missing.h' file not found",
    );
    // What the header can't say gets an error that suggests a declaration.
    rejects(
        "extern import stdio.h;\nvoid main() { printf(\"x\"); }\n",
        "`printf` can't be called as its C header declares it: it takes a variable number of arguments",
    );
    rejects(
        "extern import string.h;\nvoid main() { u8[..>] b = []; memset(b, 0, 4); }\n",
        "`memset` can't be called as its C header declares it: it returns a `void *`",
    );
    // Handles compare with null; C constants are literals of the type they're used as.
    ok(
        "extern import stdio.h;\nvoid main() { FILE f = fopen(\"x\", \"r\"); bool none = f == null; i64 e = EOF; u8 seek = SEEK_SET; }\n",
    );
    // Explicit declarations: static, with types that cross.
    rejects(
        "class K { extern i32 abs(i32 x); }\nvoid main() {}\n",
        "a C function is `static`",
    );
    rejects(
        "class Row { i64 n; }\nextern i32 f(Row r);\nvoid main() {}\n",
        "Row can't cross into C",
    );
    rejects(
        "extern string g();\nvoid main() {}\n",
        "a C function can't return string",
    );
    ok(
        "extern(\"write\") i64 writeBytes(i32 fd, u8[:] data, u64 n);\nextern(\"read\") i64 readInto(i32 fd, u8[:]& buf, u64 n);\nvoid main() {}\n",
    );
}
