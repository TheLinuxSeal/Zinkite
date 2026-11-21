//! Roots and lifetimes: the accepted and rejected programs of REFERENCES.md §10,
//! and the rules for roots (DESIGN §6.6).

use zink::diag::{Severity, render};
use zink::source::SourceFile;

fn errors(text: &str) -> Vec<String> {
    let file = SourceFile::new("test.zn", text);
    let out = zink::check(std::slice::from_ref(&file));
    out[0]
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            format!(
                "{}{}",
                d.message,
                d.help
                    .as_ref()
                    .map_or(String::new(), |h| format!(" | {}", h))
            )
        })
        .collect()
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

fn main_with(defs: &str, body: &str) -> String {
    format!("{}\nvoid main() {{\n{}\n}}\n", defs, body)
}

const SHAPES: &str = "interface Shape { f64 area(); }\nclass Circle implements Shape { f64 r; f64 area() { return r; } }\nclass Item { i64 n; }";

// ---------------------------------------------------------------- rule S (REFERENCES §3, §4)

#[test]
fn stored_references_must_not_outlive_their_roots() {
    // §10 test 1: an array of references declared after what it points into.
    ok(&main_with(
        SHAPES,
        "Circle[..>] circles = [];\nShape&[..>] shapes = [];\nshapes.push(circles.push(new Circle(1.0)));",
    ));
    rejects(
        &main_with(
            SHAPES,
            "Shape&[..>] early = [];\nCircle[..>] rects = [];\nearly.push(rects.push(new Circle(1.0)));",
        ),
        "a reference into `rects` can't be stored in `early`: `rects` is declared after it",
    );
    // §3.5: a variable's roots are everything stored into it, even if never used again.
    rejects(
        &main_with(
            "",
            "u8[..>] outer = [];\nstring w = outer;\n{\n  u8[..>] inner = [];\n  w = inner;\n}",
        ),
        "`inner` is in an inner block, so it is released first",
    );
}

#[test]
fn call_results_are_rooted_at_their_arguments() {
    // §10 test 2: `longest` may return either argument.
    let longest = "string longest(string a, string b) { return a.len >= b.len ? a : b; }";
    rejects(
        &main_with(
            longest,
            "u8[..>] outer = [];\nstring w = outer;\n{\n  u8[..>] inner = [];\n  inner.append(\"inner text\");\n  w = longest(outer, inner);\n}",
        ),
        "a reference into `inner` can't be stored in `w`",
    );
    ok(&main_with(
        longest,
        "u8[..>] outer = [];\nu8[..>] other = [];\nstring w = outer;\n{\n  w = longest(outer, other);\n}",
    ));
    // §10 test 7: a recursive function rooted at its argument and at a literal.
    let pick = "string pick(string s, i64 n) { if (n == 0) { return s; } if (n == 1) { return \"lit\"; } return pick(s, n - 1); }";
    rejects(
        &main_with(
            pick,
            "u8[..>] outer = [];\nstring w = outer;\n{\n  u8[..>] inner = [];\n  w = pick(inner, 2);\n}",
        ),
        "a reference into `inner`",
    );
    ok(&main_with(
        pick,
        "u8[..>] outer = [];\nstring w = outer;\n{\n  w = pick(\"x\", 2);\n}",
    ));
}

#[test]
fn returned_references_are_rooted_in_the_caller() {
    // §10 test 3.
    rejects(
        "string f() { u8[..>] b = []; return b; }\n",
        "this returns a reference into `b`, which is released when the function returns",
    );
    rejects(
        "class Item { i64 n; }\nItem& f(Item it) { Item c = it; return c; }\n",
        "this returns a reference into `c`",
    );
    rejects(
        "class Item { i64 n; }\nItem& f(Item it) { return it; }\n",
        "can't return a reference to this: `it` is a plain parameter",
    );
    ok(
        "class Item { i64 n; }\nstring g(string s) { return s[1:]; }\nItem& h(Item[..>]& items) { return items[0]; }\n",
    );
}

#[test]
fn statics_hold_references_only_to_statics() {
    // §10 test 5.
    rejects(
        "string saved = \"\";\nvoid main() { u8[..>] b = []; saved = b; }\n",
        "can't be stored in a static",
    );
    ok("string saved = \"\";\nvoid main() { saved = \"x\"; }\n");
}

#[test]
fn summaries_record_stores_between_arguments() {
    // §10 test 6: `register` stores its second argument into its first.
    let register = format!(
        "{}\nvoid register(Shape&[..>]& all, Circle& c) {{ all.push(c); }}",
        SHAPES
    );
    rejects(
        &main_with(
            &register,
            "Shape&[..>] all = [];\nCircle[..>] cs = [];\ncs +> new Circle(1.0);\nregister(all, cs[0]);",
        ),
        "`register(...)` stores a reference from one argument into another, and a reference into `cs` can't be stored in `all`",
    );
    ok(&main_with(
        &register,
        "Circle[..>] cs = [];\nShape&[..>] all = [];\ncs +> new Circle(1.0);\nregister(all, cs[0]);",
    ));
}

#[test]
fn temporaries_cant_be_kept() {
    // §3.3: interpolated text is a temporary.
    rejects(
        &main_with("", "i64 n = 1;\nstring s = \"${n} items\";"),
        "this text is a temporary, built only for this statement",
    );
    ok(&main_with(
        "",
        "i64 n = 1;\nu8[.] s = \"${n} items\";\nprintln(\"${n} items\");",
    ));
    rejects(
        &main_with("", "string w = \"abc\".toUpper();"),
        "this refers into a temporary value",
    );
    ok(&main_with("", "u8[.] w = \"abc\".toUpper();"));
}

#[test]
fn stored_lambdas_cant_capture() {
    // §10 test 29, DESIGN §9.3.
    rejects(
        &main_with("", "i64 k = 1;\nfunc(i64 => i64) f = (i64 a) => a + k;"),
        "a lambda that captures variables can't be stored",
    );
    ok(&main_with(
        "",
        "i64 k = 1;\ni64[..>] xs = [];\nvar ys = xs.filter((i64 v) => v > k);\nfunc(i64 => i64) f = (i64 a) => a + 1;",
    ));
}

// ---------------------------------------------------------------- rule R (REFERENCES §5)

#[test]
fn removals_while_a_reference_is_still_used() {
    // §10 test 8.
    let stack = "i32[..~] stack = [];\nstack +> 1 +> 2 +> 3;\ni32& top = stack[^1];\n";
    rejects(
        &main_with(
            "",
            &format!("{}@_ = stack ->;\nprintln(\"${{top}}\");", stack),
        ),
        "`->` removes from `stack` while `top` may still refer into it",
    );
    rejects(
        &main_with(
            "",
            &format!("{}@_ = stack ->;\nprintln(\"${{top}}\");", stack),
        ),
        "declared on line 5",
    );
    ok(&main_with(
        "",
        &format!("{}println(\"${{top}}\");\n@_ = stack ->;", stack),
    ));
    // Binding it again after the removal is fine.
    ok(&main_with(
        "",
        &format!(
            "{}println(\"${{top}}\");\n@_ = stack ->;\ntop .= stack[^1];\nprintln(\"${{top}}\");",
            stack
        ),
    ));
    // In a loop, the next pass's use counts.
    rejects(
        &main_with(
            "",
            &format!(
                "{}for (var i = 0; i < 2; i++) {{\n  println(\"${{top}}\");\n  @_ = stack ->;\n}}",
                stack
            ),
        ),
        "while `top` may still refer into it",
    );
}

#[test]
fn removals_by_called_functions() {
    // §10 test 9: `trim` removes from its argument.
    let trim = "void trim(i32[..~]& xs) { while (xs.len > 10) { @_ = xs ->; } }";
    rejects(
        &main_with(
            trim,
            "i32[..~] nums = [];\nnums +> 1;\ni32& r = nums[0];\ntrim(nums);\nprintln(\"${r}\");",
        ),
        "`trim(...)` removes from `nums` while `r` may still refer into it",
    );
    ok(&main_with(
        trim,
        "i32[..~] nums = [];\nnums +> 1;\ni32& r = nums[0];\nprintln(\"${r}\");\ntrim(nums);",
    ));
}

#[test]
fn references_into_removable_arrays_stay_in_locals() {
    // §10 test 10.
    let holder = "class Item { i64 n; }\nclass Holder { Item& it; }";
    rejects(
        &main_with(
            holder,
            "Item[..~] items = [];\nitems +> new Item(1);\nHolder h = new Holder(items[0]);",
        ),
        "a reference into `items`, which can remove elements, can be kept only in a local variable or a parameter",
    );
    rejects(
        &main_with(
            holder,
            "Item[..~] items = [];\nItem&[..>] refs = [];\nitems +> new Item(1);\nrefs.push(items[0]);",
        ),
        "can be kept only in a local variable",
    );
    ok(&main_with(
        holder,
        "Item[..>] items = [];\nItem&[..>] refs = [];\nitems +> new Item(1);\nrefs.push(items[0]);\nHolder h = new Holder(items[0]);",
    ));
    // §10 test 11: `clear()` on a grow-only array while a field holds a reference into it.
    rejects(
        &main_with(
            holder,
            "Item[..>] items = [];\nitems +> new Item(1);\nHolder h = new Holder(items[0]);\nitems.clear();\nprintln(\"${h.it.n}\");",
        ),
        "removes from `items` while `h` may still refer into it",
    );
}

#[test]
fn capped_arrays_follow_rule_r() {
    // §10 test 12: a pop ends references into a capped array, as on a data stack.
    rejects(
        &main_with(
            "",
            "i32[..~16] small = [];\n@_ = small +> 1;\ni32& r = small[0];\n@_ = small ->;\nprintln(\"${r}\");",
        ),
        "`->` removes from `small` while `r` may still refer into it",
    );
    // An append doesn't move anything.
    ok(&main_with(
        "",
        "i32[..~16] small = [];\n@_ = small +> 1;\ni32& r = small[0];\n@_ = small +> 2;\nprintln(\"${r}\");",
    ));
    // A capped array that changes only at the start sits at the top of its buffer, so a
    // front insert moves nothing either; the reference follows its element.
    ok(&main_with(
        "",
        "i32[>..4] f = [2, 3];\ni32& x = f[0];\n@_ = f +< 1;\nprintln(\"${x}\");",
    ));
    ok(&main_with(
        "",
        "i32[~..4] f = [2, 3];\ni32& x = f[^1];\n@_ = f +< 1;\nprintln(\"${x}\");",
    ));
    rejects(
        &main_with(
            "",
            "i32[~..4] f = [2, 3];\ni32& x = f[^1];\n@_ = f -<;\nprintln(\"${x}\");",
        ),
        "`-<` removes from `f`",
    );
    // Pools keep their exemption.
    ok(&main_with(
        "",
        "pool<i64> p = [];\ni64 a = 1;\nprintln(\"${a}\");",
    ));
}

#[test]
fn ring_and_gap_buffers_end_references_when_they_change() {
    // §10 test 31: an insert into a ring or gap buffer can move its elements.
    rejects(
        &main_with(
            "",
            "i64[~.~4] q = [1, 2];\ni64& r = q[0];\n@_ = q +> 5;\nprintln(\"${r}\");",
        ),
        "`+>` can move the elements of `q` while `r` may still refer into it",
    );
    rejects(
        &main_with(
            "",
            "u8[~8] g = \"ab\";\nstring s = g;\n@_ = g +^ 'x';\nprintln(s);",
        ),
        "`+^` can move the elements of `g` while `s` may still refer into it",
    );
    rejects(
        &main_with(
            "",
            "u8[~8] g = \"ab\";\nu8& c = g.insert('x');\n@_ = g.insertAt(0, 'y');\nprintln(\"${c}\");",
        ),
        "`insertAt` can move the elements of `g`",
    );
    // §10 test 32: a slice, sort or comparison can rearrange one, which ends references
    // to its elements but not other slices.
    rejects(
        &main_with(
            "",
            "i64[~.~4] q = [1, 2];\ni64& r = q[0];\ni64[:] s = q;\nprintln(\"${r} ${s}\");",
        ),
        "this slice can rearrange `q` while `r` may still refer into it",
    );
    rejects(
        &main_with(
            "",
            "i64[~.~4] q = [1, 2];\ni64& r = q[0];\nq.sort();\nprintln(\"${r}\");",
        ),
        "`sort` can rearrange `q`",
    );
    rejects(
        &main_with(
            "",
            "u8[~8] g = \"ab\";\nu8& c = g[0];\nbool same = g == \"ab\";\nprintln(\"${c} ${same}\");",
        ),
        "this comparison can rearrange `g`",
    );
    rejects(
        &main_with(
            "",
            "u8[~8] g = \"ab\";\nu8& c = g[0];\nbool a = g.startsWith(\"a\");\nprintln(\"${c} ${a}\");",
        ),
        "`startsWith` can rearrange `g`",
    );
    ok(&main_with(
        "",
        "i64[~.~4] q = [1, 2];\ni64[:] a = q[0:1];\ni64[:] b = q[1:];\nq.sort();\nprintln(\"${a} ${b} ${q[0]}\");",
    ));
    ok(&main_with(
        "",
        "u8[~8] g = \"ab\";\nfor (u8 c : g) { string s = g; println(s); }\nu8& d = g[0];\nprintln(\"${d}\");",
    ));
    // Bound again after the change, it's fine.
    ok(&main_with(
        "",
        "i64[~.~4] q = [1, 2];\ni64& r = q[0];\nprintln(\"${r}\");\n@_ = q +> 5;\ni64& t = q[0];\nprintln(\"${t}\");",
    ));
    // Through a function's summary.
    rejects(
        &main_with(
            "void add(i64[~.~4]& q) { @_ = q +> 1; }",
            "i64[~.~4] q = [1, 2];\ni64& r = q[0];\nadd(q);\nprintln(\"${r}\");",
        ),
        "`add(...)` can move the elements of `q`",
    );
    // §10 test 33: references into one can't be kept in fields.
    rejects(
        "class Holder { i64&? r = null; }\nvoid main() {\ni64[~.~4] q = [1];\nHolder h = new Holder();\nh.r .= q[0];\n}\n",
        "a reference into `q`, whose elements can move, can be kept only in a local variable or a parameter",
    );
    rejects(
        "class Recent { i64[~.~4] q = []; i64&? last = null; mut void see(i64 x) { @_ = q +> x; last .= q[0]; } }\nvoid main() {}\n",
        "a reference into `this.q`, whose elements can move, can be kept only in a local variable or a parameter",
    );
    ok(
        "class Log { i64[..>8] xs = []; i64&? first = null; mut void add(i64 x) { @_ = xs +> x; first .= xs[0]; } }\nvoid main() {}\n",
    );
}

#[test]
fn for_each_keeps_a_position() {
    // §10 test 28, and by value (REFERENCES §9).
    rejects(
        &main_with("", "i64[..~] xs = [];\nfor (i64& x : xs) { @_ = xs ->; }"),
        "removes from `xs` inside a for-each over it",
    );
    rejects(
        &main_with("", "i64[..~] xs = [];\nfor (i64 x : xs) { xs.clear(); }"),
        "inside a for-each over it",
    );
    ok(&main_with(
        "",
        "i64[..~] xs = [];\nfor (i64 x : xs) { }\nxs.clear();",
    ));
    // An iterator keeps a view of its text.
    rejects(
        &main_with(
            "",
            "u8[..~] text = [];\nfor (string l : text.lines()) { text.clear(); }",
        ),
        "inside a for-each over it",
    );
}

#[test]
fn deferred_statements_run_at_the_end_of_their_block() {
    // §10 test 30.
    rejects(
        &main_with(
            "class Item { i64 n; }",
            "Item[..~] items = [];\nitems +> new Item(1);\nItem& r = items[0];\ndefer println(\"${r.n}\");\n@_ = items ->;",
        ),
        "while `r` may still refer into it",
    );
}

#[test]
fn repeat_loops_bind_after_the_call() {
    // REFERENCES §9: the call may remove from what the previous pass's value viewed.
    let next = "string next(u8[..~]& buf) { buf.clear(); return #1 buf; }";
    ok(&main_with(
        next,
        "u8[..~] buf = [];\nwhile (string line : next(buf)) { println(line); }",
    ));
    rejects(
        &main_with(
            next,
            "u8[..~] buf = [];\nstring[..>] lines = [];\nwhile (string line : next(buf)) { lines +> line; }",
        ),
        "a reference into `buf`, which can remove elements",
    );
}

#[test]
fn parts_of_a_root_are_separate() {
    // REFERENCES §8.2: a removal from one part doesn't touch references into another.
    let r = "root R { i64[..~] a; i64[..~] b; }";
    ok(&main_with(
        r,
        "R r = new R();\nr.a +> 1;\nr.b +> 2;\ni64& x = r.a[0];\n@_ = r.b ->;\nprintln(\"${x}\");",
    ));
    rejects(
        &main_with(
            r,
            "R r = new R();\nr.a +> 1;\ni64& x = r.a[0];\n@_ = r.a ->;\nprintln(\"${x}\");",
        ),
        "removes from `r.a` while `x` may still refer into it",
    );
}

// ---------------------------------------------------------------- roots (DESIGN §6.6) and recursion (REFERENCES §8.4)

#[test]
fn roots_are_never_copied() {
    let p = "root P { i64[..>] a; }";
    ok(&main_with(p, "P p = new P();\nvar q = new P();\nP& r = p;"));
    rejects(
        &main_with(p, "P p = new P();\nP q = p;"),
        "`P` is a root, which can't be copied",
    );
    rejects(
        &main_with(p, "P p = new P();\nP q = new P();\nq = p;"),
        "can't be copied",
    );
    rejects(
        &format!("{}\nvoid f(P p) {{ }}\n", p),
        "`P` is a root, so it can't be passed by value",
    );
    rejects(
        &format!("{}\nP make() {{ P p = new P(); return p; }}\n", p),
        "can't be returned by value",
    );
    rejects(
        &format!("{}\nvoid f(P& p) {{ }}\nvoid main() {{ f(new P()); }}\n", p),
        "`P` is a root, so it can't be a temporary",
    );
    ok(&format!("{}\nP shared = new P();\nvoid main() {{ }}\n", p));
}

#[test]
fn no_growable_local_during_a_recursive_call() {
    // §10 test 25, with a tree instead of directories.
    let node = "class Node { i64 v; Node&? left; }";
    ok(&format!(
        "{}\nvoid walk(Node& n, i64[..>]& out) {{ out +> n.v; Node&? l = n.left; if (l != null) {{ walk(l, out); }} }}\n",
        node
    ));
    rejects(
        &format!(
            "{}\nvoid walkEach(Node& n, i64[..>]& out) {{ i64[..>] mine = []; Node&? l = n.left; if (l != null) {{ walkEach(l, mine); }} out.pushAll(mine); }}\n",
            node
        ),
        "`mine` is growable and in scope during this call, which can recurse back into `walkEach`",
    );
    rejects(
        "i64 count(i64[..>] xs, i64 n) { if (n == 0) { return xs.len; } return count(xs, n - 1); }\n",
        "a growable value can't be passed by value into a recursive cycle",
    );
}

#[test]
fn a_removable_field_is_storage_of_its_own() {
    // §10 test 34: a reference into an object's own removable or moving array can't be
    // kept in another of its fields, even when the store and the change are in
    // different methods.
    rejects(
        "root R {\n  i64[..~] xs = [];\n  i64&? last = null;\n  new() {}\n  mut void keep() { last .= xs[0]; }\n  mut void drop() { @_ = xs ->; }\n}\nvoid main() {}\n",
        "a reference into `this.xs`, which can remove elements, can be kept only in a local variable or a parameter",
    );
    rejects(
        "class Recent {\n  i64[~.~4] q = [];\n  i64&? last = null;\n  mut void keep() { last .= q[0]; }\n  mut void add(i64 x) { @_ = q +> x; }\n}\nvoid main() {}\n",
        "a reference into `this.q`, whose elements can move",
    );
    let defs = "class Recent { i64[~.~4] q = []; }\nclass Holder { i64&? r = null; }";
    // Reached through a nested value, a parameter, a local, or a called function.
    rejects(
        &format!(
            "{}\nclass Outer {{ Recent rec = new Recent(); i64&? r = null; mut void f() {{ r .= rec.q[0]; }} }}\nvoid main() {{}}\n",
            defs
        ),
        "a reference into the `q` of a `Recent` in `this`",
    );
    rejects(
        &format!(
            "{}\nvoid keep(Recent& rc, Holder& h) {{ h.r .= rc.q[0]; }}\nvoid main() {{}}\n",
            defs
        ),
        "a reference into `rc.q`",
    );
    rejects(
        &main_with(
            defs,
            "Recent rc = new Recent();\nHolder h = new Holder();\nh.r .= rc.q[0];",
        ),
        "a reference into `rc.q`",
    );
    rejects(
        &main_with(
            &format!("{}\nvoid put(Holder& h, i64& x) {{ h.r .= x; }}", defs),
            "Recent rc = new Recent();\nHolder h = new Holder();\nput(h, rc.q[0]);",
        ),
        "a reference into `rc.q`",
    );
    // Each such field is checked on its own: a pop from one doesn't end references
    // into another.
    let two = "class Two {\n  i64[..~4] a = [];\n  i64[..~4] b = [];\n  mut void f() { i64& x = a[0]; @_ = b ->; println(\"${x}\"); }\n}";
    ok(&main_with(two, ""));
    rejects(
        &main_with(
            "class Two {\n  i64[..~4] a = [];\n  mut void g() { i64& x = a[0]; @_ = a ->; println(\"${x}\"); }\n}",
            "",
        ),
        "`->` removes from `this.a` while `x` may still refer into it",
    );
}

/// Interface references follow rules S and R like any reference.
#[test]
fn interface_references_follow_the_rules() {
    let e = errors(
        r##"interface Shape { f64 area(); }
class Circle implements Shape {
    f64 r;
    f64 area() { return r; }
}

Shape& escape() {
    Circle c = new Circle(1.0);
    Shape& s = c;
    return s;
}

void main() {
    Shape&[..>] keep = [];
    {
        Circle inner = new Circle(2.0);
        keep +> inner;
    }
    Circle[..~] cs = [];
    cs +> new Circle(3.0);
    Shape& first = cs[0];
    cs.clear();
    println("${first.area()}");
}
"##,
    );
    assert_eq!(e.len(), 3, "{:#?}", e);
    assert!(e[0].contains("returns a reference into `c`"), "{}", e[0]);
    assert!(e[1].contains("can't be stored in `keep`"), "{}", e[1]);
    assert!(
        e[2].contains("`clear` removes from `cs` while `first` may still refer into it"),
        "{}",
        e[2]
    );
}

/// A text iterator holds a view into its text (REFERENCES §9): removing from the text
/// while one may still be used is an error, and so is keeping one over a temporary.
#[test]
fn text_iterators_follow_the_rules() {
    let e = errors(
        r##"u8[.] make() { u8[..>] t = []; t.append("a\nb"); return t; }

void main() {
    u8[..~] text = [];
    text.append("a\nb\n");
    for (string l : text.lines()) {
        if (l == "a") { text.clear(); }
    }
    LineIterator it = text.lines();
    text.clear();
    while (string l : it.next()) {}
    LineIterator gone = make().lines();
    while (string l : gone.next()) {}
}
"##,
    );
    assert_eq!(e.len(), 3, "{:#?}", e);
    assert!(
        e[0].contains("`clear` removes from `text` inside a for-each over it"),
        "{}",
        e[0]
    );
    assert!(
        e[1].contains("`clear` removes from `text` while `it` may still refer into it"),
        "{}",
        e[1]
    );
    assert!(e[2].contains("refers into a temporary value"), "{}", e[2]);
}
