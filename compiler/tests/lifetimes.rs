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

/// V2 (REFERENCES §6): assigning a variable-size variable rebuilds it, so a view into its
/// old value can't be used afterwards.
#[test]
fn rebuilding_a_record_ends_views_into_it() {
    let e = errors(
        r##"class Order { i32 id; u8[.*] customer; }

void main() {
    Order o = new Order(1, "alice");
    string name = o.customer;
    o = new Order(2, "alexander");
    println(name);
}
"##,
    );
    assert_eq!(e.len(), 1, "{:#?}", e);
    assert!(
        e[0].contains("assigning `o` removes from `o` while `name` may still refer into it"),
        "{}",
        e[0]
    );
}

/// A variable-size record lives on a data stack of its own, so a recursive call can't
/// happen while one is in scope (REFERENCES §8.4).
#[test]
fn records_in_scope_block_recursion() {
    let e = errors(
        r##"class Item { u8[.*] sku; i32 qty; }

i32 depth(i32 n) {
    Item here = new Item("level", n);
    if (n == 0) { return here.qty; }
    return depth(n - 1) + here.qty;
}

void main() { println("${depth(3)}"); }
"##,
    );
    assert_eq!(e.len(), 1, "{:#?}", e);
    assert!(e[0].contains("`here` is a variable-size record and in scope during this call, which can recurse back into `depth`"), "{}", e[0]);
}

/// A call to a method declared further down the file has its summary (REFERENCES §4.2):
/// what it stores is checked, which a store through a later method once escaped, and what
/// it returns keeps its roots, so a reader whose next() returns a helper's view of its
/// input gives views rooted at that input, not at the reader.
#[test]
fn calls_to_functions_declared_later_have_summaries() {
    rejects(
        "class Keep {\n    string[..>]& out;\n    new(string[..>]& out) { this.out = out; }\n    mut void add(string s) { later(s); }\n    mut void later(string s) { out +> s; }\n}\nvoid main() {\n    string[..>] out = [];\n    Keep k = new Keep(out);\n    {\n        u8[..>] inner = [];\n        inner.append(\"short-lived\");\n        k.add(inner);\n    }\n    println(\"${out.len}\");\n}\n",
        "a reference into `inner` can't be stored in `out`",
    );
    ok(
        "class Ev { string text; }\nclass R {\n    string input;\n    i64 pos = 0;\n    new(string input) { this.input = input; }\n    mut Ev next() {\n        if (pos >= input.len) { return #status.END new Ev(\"\"); }\n        return one();\n    }\n    private mut Ev one() {\n        pos++;\n        return new Ev(input[pos - 1:pos]);\n    }\n}\nvoid keep(string t, string[..>]& out) { R r = new R(t); while (Ev e : r.next()) { out +> e.text; } }\nvoid main() { string[..>] out = []; keep(\"ab\", out); println(\"${out.len}\"); }\n",
    );
}

/// A method called on a temporary value, as in `cfg.get("a").text()`, gives what the
/// temporary holds, which outlives it; a reference into the temporary itself doesn't.
#[test]
fn a_temporary_receiver_passes_on_what_it_holds() {
    ok(
        "class V {\n    string[:] items;\n    V first() { return new V(items[0:1]); }\n    string name() { return items[0]; }\n}\nvoid main() {\n    string[..>] all = [];\n    all +> \"a\";\n    V v = new V(all);\n    string n = v.first().first().name();\n    println(n);\n}\n",
    );
    rejects(
        "class Box {\n    i64 n;\n    i64& get() { return n; }\n}\nBox make() { return new Box(1); }\nvoid main() {\n    i64& r = make().get();\n    println(\"${r}\");\n}\n",
        "temporary",
    );
}

/// A removal from an object's field is checked against references into that field, which
/// are rooted at it however the object is reached (REFERENCES §5.3): a reference to the
/// object itself doesn't refer into the field's elements.
#[test]
fn a_reference_to_an_object_survives_removals_from_its_fields() {
    let class = "class Stack {\n    u8[..~] items = [];\n    mut void push(u8 x) { items +> x; }\n    mut u8 pop() { u8 x @err = items.pop(); return #err x; }\n    u8& top() { return items[^1]; }\n}\n";
    ok(&format!(
        "{}void drain(Stack& s) {{ while (u8 x : s.pop()) {{ println(\"${{x}}\"); }} }}\nvoid main() {{ Stack s = new Stack(); s.push(1); drain(s); }}\n",
        class
    ));
    rejects(
        &format!(
            "{}void main() {{ Stack s = new Stack(); s.push(1); s.push(2); u8& t = s.top(); @_ = s.pop(); println(\"${{t}}\"); }}\n",
            class
        ),
        "while `t` may still refer into it",
    );
}

/// Rule L1 (REFERENCES §7.4): a self-relative link points into the array it is stored in,
/// and a pool link into its static array, followed through calls, constructors and
/// `push(new ...)`; and a link whose array can't be told is an error.
#[test]
fn links_point_into_their_own_array() {
    let defs = "class Node { i32 key; Node&<i32>? left; }\nclass Pair {\n    i32 a;\n    Pair&<i32>? partner;\n    new(Pair&? to, i32 a) { this.a = a; partner = to; }\n}\nclass P { i32 v; P&<u32 in ps>? next; }\nP[..>] ps = [];\nP[..>] others = [];\nvoid attach(Node& into, Node& what) { into.left .= what; }\n";
    let main = |body: &str| {
        format!(
            "{}void main(string[.] args) {{ Node[..>] a = []; Node[..>] b = []; Node& x = a.push(new Node(1, null)); Node& y = b.push(new Node(2, null)); Pair[..>] p1 = []; Pair[..>] p2 = []; Pair& q = p1.push(new Pair(null, 1)); {} }}\n",
            defs, body
        )
    };
    ok(&main(
        "x.left .= a.push(new Node(3, x)); a +> new Node(4, x); attach(x, x); p1.push(new Pair(q, 2)); P& g = ps.push(new P(1, null)); ps +> new P(2, g);",
    ));
    rejects(&main("x.left .= y;"), "this link is in an element of `a`, and what it would refer to is in `b`");
    rejects(&main("a +> new Node(3, y);"), "this link is in an element of `a`, and what it would refer to is in `b`");
    rejects(&main("attach(x, y);"), "this link is in an element of `a`, and what it would refer to is in `b`");
    rejects(&main("p2.push(new Pair(q, 2));"), "this link is in an element of `p2`, and what it would refer to is in `p1`");
    rejects(
        &main("Node& either = args.len > 0 ? x : y; either.left .= x;"),
        "can't tell which array this link is stored in: it may be `a` or `b`",
    );
    rejects(
        &main("P& o = others.push(new P(1, null)); ps +> new P(2, o);"),
        "this pool link is measured from `ps`, and what it would refer to is in `others`",
    );
}

/// Rule L1 for arrays of links: each link goes into the array of the value holding them,
/// by `+>`, by binding an element, through a parameter, or as a constructor's argument.
#[test]
fn arrays_of_links_point_into_their_own_array() {
    let defs = "class Node { i32 key; Node&<i32>[..~4] kids; }\nvoid adopt(Node& into, Node& kid) { into.kids +> kid; }\n";
    let main = |body: &str| {
        format!(
            "{}void main() {{ Node[..>] a = []; Node[..>] b = []; Node& x = a.push(new Node(1, [])); Node& w = a.push(new Node(3, [])); Node& y = b.push(new Node(2, [])); {} }}\n",
            defs, body
        )
    };
    ok(&main("x.kids +> w +> x; x.kids[0] .= x; adopt(x, w); a +> new Node(4, [x, w]); x.kids +> a.push(new Node(5, []));"));
    let wrong = "this link is in an element of `a`, and what it would refer to is in `b`";
    rejects(&main("x.kids +> y;"), wrong);
    rejects(&main("x.kids +> w +> y;"), wrong);
    rejects(&main("x.kids +> w; x.kids[0] .= y;"), wrong);
    rejects(&main("adopt(x, y);"), wrong);
    rejects(&main("a +> new Node(4, [y]);"), wrong);
}

/// What a function stores into a static from its parameters is mapped to each caller's
/// roots: the parameter means nothing in another function, so it isn't kept in the
/// static's own roots, where it would be taken for that function's first parameter.
#[test]
fn a_parameter_stored_into_a_static_stays_its_own_functions() {
    let defs = "class Node { Node&? next; i32 v; }\nNode[..>] nodes = [];\nNode& add(Node&? prev, i32 v) { return nodes.push(new Node(prev, v)); }\n";
    ok(&format!("{}void main() {{ Node& a = add(null, 1); add(a, 2); u8[..>] buf = []; buf +> 1; buf.clear(); }}\n", defs));
    rejects(
        &format!("{}void main() {{ Node[..>] mine = []; Node& m = mine.push(new Node(null, 1)); add(m, 2); }}\n", defs),
        "a reference into `mine`, a variable of this function, can't be stored in a static",
    );
}

/// What a lambda returns (DESIGN §9.3): a built-in's result is rooted at it, so text built
/// only for the statement can't be kept; a lambda passed anywhere else can't return such
/// text at all; and the array `filter` builds can't be built while its lambda recurses back
/// into the function (§8.4).
#[test]
fn what_a_lambda_returns_lives_long_enough() {
    rejects(
        &main_with("", "i64[..>] a = [1, 2];\nstring r = a.fold(\"\", (string acc, i64 x) => \"${acc}${x}\");\nprintln(r);"),
        "this text is a temporary, built only for this statement, so `r` can't keep a view of it",
    );
    rejects(
        &main_with("", "i64[..>] a = [1, 2];\nstring[..>] t = a.map((i64 x) => \"<${x}>\");\nprintln(\"${t}\");"),
        "this text is a temporary, built only for this statement, so `t` can't keep a view of it",
    );
    ok(&main_with("", "i64[..>] a = [1, 2];\nprintln(\"${a.map((i64 x) => \"<${x}>\")} ${a.fold(\"\", (string acc, i64 x) => \"${acc}${x}\")}\");\nstring[..>] w = [\"ab\", \"c\"];\nstring longest = w.fold(\"\", (string m, string s) => s.len > m.len ? s : m);\nprintln(longest);"));
    rejects(
        &main_with("string wrap(string s, func(string => string) f) { return f(s); }", "string pre = \"<\";\nstring r = wrap(\"x\", (string s) => \"${pre}${s}\");\nprintln(r);"),
        "this lambda returns something built only for the statement it's in",
    );
    ok(&main_with("string wrap(string s, func(string => string) f) { return f(s); }", "string pre = \"<\";\nstring r = wrap(\"x\", (string s) => pre);\nprintln(r);"));
    rejects(
        "i64 depth(i64[:] xs, i64 d) {\n    if (d == 0) { return 0; }\n    i64[..>] kept = xs.filter((i64 x) => depth(xs, d - 1) >= 0 && x > d);\n    return kept.len;\n}\nvoid main() { i64[..>] a = [1, 2]; println(\"${depth(a, 2)}\"); }\n",
        "`filter`'s result is still being built during this call, which can recurse back into `depth`",
    );
}

// ---------------------------------------------------------------- threads (STDLIB §10.3)

/// A worker gets copies of the statics a thread can copy, so it may reach only those,
/// through its lambda and everything that calls, and of those with views, only views of
/// string literals.
#[test]
fn workers_reach_only_statics_they_get_copies_of() {
    let t = "import zink.thread.*;\nhashmap<string, i64> CACHE = [];\nu8[..>] BUF = [];\nstring FIRST = \"\";\nstring GREETING = \"hello\";\ni64[..>] PRIMES = [];\nclass Lookup { i64 find(string k) { i64 v @_ = CACHE.get(k); return v; } }\ninterface Shape { i64 area(); }\nclass Sq implements Shape { i64 area() { return CACHE.len; } }\n";
    ok(&format!(
        "{}void main() {{\n    PRIMES +> 2;\n    Worker w = thread.spawn(() => println(\"${{GREETING}} ${{PRIMES}}\"));\n    w.join();\n    println(\"${{CACHE.len}}\");\n}}\n",
        t
    ));
    rejects(
        &format!(
            "{}void main() {{\n    Worker w = thread.spawn(() => println(\"${{CACHE.len}}\"));\n}}\n",
            t
        ),
        "the worker spawned on line 11 can't use the static `CACHE`: a new thread doesn't get a copy of a `hashmap<string, i64>`",
    );
    rejects(
        &format!(
            "{}void main() {{\n    Worker w = thread.spawn(() => {{\n        Lookup l = new Lookup();\n        i64 v = l.find(GREETING);\n        println(\"${{v}}\");\n    }});\n}}\n",
            t
        ),
        "can't use the static `CACHE`",
    );
    rejects(
        &format!(
            "{}i64 measure(Shape& s) {{ return s.area(); }}\nvoid main() {{\n    Worker w = thread.spawn(() => {{\n        Sq q = new Sq();\n        println(\"${{measure(q)}}\");\n    }});\n}}\n",
            t
        ),
        "can't use the static `CACHE`",
    );
    rejects(
        &format!(
            "{}void main() {{\n    BUF.append(\"abc\");\n    FIRST = BUF[0:2];\n    Worker w = thread.spawn(() => println(FIRST));\n}}\n",
            t
        ),
        "can't use the static `FIRST`: it holds views that may point into the spawning thread's memory",
    );
}

// ---------------------------------------------------------------- the audit of 2026-11-25 (TODO.md)

/// Text built in an inner block, which a later round of the loop overwrites: a view of it
/// kept outside must be refused.
fn escape(defs: &str, escape: &str, after: &str) -> String {
    format!(
        "{}\nvoid main() {{\n    for (i64 round = 0; round < 2; round++) {{\n        u8[..>] buf = [];\n        buf.append(round == 0 ? \"secret-AAAAAAAA\" : \"overwritten-BBB\");\n        if (round == 0) {{ {} }}\n    }}\n    {}\n}}\n",
        defs, escape, after
    )
}

/// Item 3: an interface call is checked against the method that implements it, not
/// another of the same name and count of parameters.
#[test]
fn interface_calls_use_the_implementing_overload() {
    rejects(
        &main_with(
            "interface Z { void zap(i32[..~]& a); }\nclass K implements Z { i64 q; void zap(i64 a) {} void zap(i32[..~]& a) { a.clear(); } }",
            "i32[..~] a = [];\na +> 5;\nK[..>] ks = [];\nZ& z = ks.push(new K(1));\ni32& x = a[0];\nz.zap(a);\na +> 777;\nprintln(\"${x}\");",
        ),
        "`zap(...)` removes from `a` while `x` may still refer into it",
    );
    rejects(
        &escape(
            "string saved = \"\";\ninterface Sink { void put(string s); }\nclass Keeper implements Sink { i64 n = 0; void put(i64 x) {} void put(string s) { saved = s; } }\nvoid send(Sink& k, string s) { k.put(s); }",
            "Keeper[..>] ks = []; Keeper& k = ks.push(new Keeper()); send(k, buf);",
            "println(saved);",
        ),
        "can't be stored in a static",
    );
}

/// Item 4: deferred statements run last first, and also when `break` or `continue`
/// leaves their block.
#[test]
fn deferred_statements_are_checked_where_they_run() {
    let pre = "i32[..~] a = [];\na +> 5;\ni32& x = a[0];\n";
    rejects(
        &main_with(
            "",
            &format!(
                "{}defer println(\"${{x}}\");\ndefer a +> 777;\ndefer a.clear();",
                pre
            ),
        ),
        "`clear` removes from `a` while `x` may still refer into it",
    );
    for jump in ["break", "continue"] {
        rejects(
            &main_with(
                "",
                &format!(
                    "{}for (i64 i = 0; i < 1; i++) {{\n    defer println(\"${{x}}\");\n    a.clear();\n    a +> 777;\n    {};\n}}",
                    pre, jump
                ),
            ),
            "`clear` removes from `a` while `x` may still refer into it",
        );
    }
    // A block with no deferred statements adds nothing at a jump.
    ok(&main_with(
        "class Frame { i64 next; i64 end; }",
        "Frame[..~] stack = [];\nstack +> new Frame(0, 1);\nwhile (stack.len > 0) {\n    Frame& top = stack[^1];\n    if (top.next == top.end) {\n        @_ = stack ->;\n        continue;\n    }\n    top.next++;\n}",
    ));
}

/// Item 7: what makes a ring or gap buffer one block, or moves its elements, ends
/// references into it.
#[test]
fn every_rearrangement_of_a_ring_or_gap_buffer_counts() {
    let ring = "i64[~.~4] q = [1, 2, 3, 4];\n@_ = q -<;\n@_ = q +> 5;\ni64& r = q[0];\n";
    for m in [
        "q.partialSort(1);",
        "q.radixSort();",
        "@_ = q.selectNth(0);",
        "q.sortByDescending((i64 v) => v);",
    ] {
        rejects(
            &main_with("", &format!("{}{}\nprintln(\"${{r}}\");", ring, m)),
            "can rearrange `q` while `r` may still refer into it",
        );
    }
    let gap = "u8[~8] g = \"abcde\";\n@_ = g.seek(3);\n@_ = g +^ 'X';\nu8& r = g[4];\n";
    rejects(
        &main_with("", &format!("{}g.append(\"zz\");\nr = 'Z';", gap)),
        "`append` can move the elements of `g`",
    );
    rejects(
        &main_with("", &format!("{}println(g);\nr = 'Z';", gap)),
        "can rearrange `g` while `r` may still refer into it",
    );
    rejects(
        &main_with(
            "",
            &format!(
                "{}var parts = g.split(\"b\");\nr = 'Z';\nprintln(\"${{parts.len}}\");",
                gap
            ),
        ),
        "`split` can rearrange `g`",
    );
    // Going element by element leaves it as it is.
    ok(&main_with(
        "",
        &format!(
            "{}println(\"${{g}} ${{g.contains(98)}} ${{g.indexOf(98)}}\");\ng.reverse();\nr = 'Z';",
            gap
        ),
    ));
}

/// Item 9: a pool's exemption covers its elements, not removals from an array that is one.
#[test]
fn arrays_in_pools_follow_rule_r() {
    rejects(
        &main_with(
            "",
            "pool<i64[..~8]> p = [];\nHandle<i64[..~8]> h = p.add([1]);\ni64[..~8]& arr = p.get(h);\ni64& r = arr[0];\narr.clear();\narr +> 777;\nprintln(\"${r}\");",
        ),
        "`clear` removes from `p` while `r` may still refer into it",
    );
}

/// Item 10: a function storing views of one part of an object in another part of it.
#[test]
fn summaries_keep_stores_within_an_object() {
    rejects(
        &main_with(
            "root Lexer { u8[..>] text; string[..>] words; }\nvoid add(Lexer& lx, string s) { i64 st = lx.text.len; lx.text.append(s); lx.words +> lx.text[st:]; }",
            "Lexer lx = new Lexer();\nadd(lx, \"secret\");\nlx.text.clear();\nadd(lx, \"overwritten\");\nprintln(lx.words[0]);",
        ),
        "`clear` removes from `lx.text` while `lx` may still refer into it",
    );
    rejects(
        &main_with(
            "class Doc {\n    u8[..>16] text;\n    string[..>] words;\n    mut void add(string s) { i64 st = text.len; text.append(s); words +> text[st:]; }\n    mut void reset() { text.clear(); }\n}",
            "Doc d = new Doc(\"\");\nd.add(\"secret\");\nd.reset();\nd.add(\"overwritten\");\nprintln(d.words[0]);",
        ),
        "`reset(...)` removes from `d` while `d` may still refer into it",
    );
}

/// Item 11: destructuring into a static, a field or a reference stores into it.
#[test]
fn destructuring_stores_into_existing_names() {
    let pair = "(string, i64) pair(string s) { return (s, s.len); }";
    rejects(
        &escape(
            &format!("string saved = \"\";\n{}", pair),
            "i64 n = 0; (saved, n) = pair(buf);",
            "println(saved);",
        ),
        "can't be stored in a static",
    );
    rejects(
        &escape(
            &format!("class Holder {{ string s; }}\n(Holder, i64) mk(string s) {{ return (new Holder(s), s.len); }}\n{}", pair),
            "",
            "println(outer.s);",
        )
        .replace("for (i64 round", "Holder outer = new Holder(\"\");\n    for (i64 round")
        .replace("if (round == 0) {  }", "if (round == 0) { Holder& h = outer; i64 n = 0; (h, n) = mk(buf); }"),
        "a reference into `buf` can't be stored in `outer`",
    );
}

/// Item 13: the analysis goes on until nothing changes, however long a chain is.
#[test]
fn long_chains_are_followed_to_the_end() {
    let n = 80;
    let decls: String = (1..=n).map(|i| format!("string v{} = \"\"; ", i)).collect();
    let mut chain = String::from("out = v1; ");
    for i in 1..n {
        chain.push_str(&format!("v{} = v{}; ", i, i + 1));
    }
    chain.push_str(&format!("v{} = buf; ", n));
    let body = format!("{} {}", decls, chain.repeat(n + 2));
    rejects(
        &escape("", &body, "println(out);")
            .replace("for (i64 round", "string out = \"\";\n    for (i64 round"),
        "a reference into `buf` can't be stored in `out`",
    );
    let mut fns = String::new();
    for k in 0..120 {
        fns.push_str(&format!(
            "string g{}(string s) {{ return g{}(s); }}\n",
            k,
            k + 1
        ));
    }
    fns.push_str("string g120(string s) { return s; }\nstring saved = \"\";");
    rejects(
        &escape(&fns, "saved = g0(buf);", "println(saved);"),
        "can't be stored in a static",
    );
}

/// Item 16: an array arm of a `?:` or a switch that gives a view is a view of that array.
#[test]
fn conditional_arms_keep_the_roots_of_what_they_convert() {
    rejects(
        "string f(string t, bool p) { u8[..>] b = []; b.append(t); return p ? b : \"literal\"; }\nvoid main() { println(f(\"x\", true)); }\n",
        "this returns a reference into `b`",
    );
    for e in [
        "saved = round == 0 ? buf : \"literal\";",
        "saved = switch (round) { case 0 => buf; default => \"literal\"; };",
        "saved = switch (round) { case 0 => { yield buf; } default => \"literal\"; };",
    ] {
        rejects(
            &escape("string saved = \"\";", e, "println(saved);"),
            "can't be stored in a static",
        );
    }
}

/// Item 18: whether a class holds references is right for classes that contain each other.
#[test]
fn recursive_classes_hold_what_their_fields_hold() {
    rejects(
        &escape(
            "class A { B b; string s; }\nclass B { A[.] xs; }",
            "A[..>] tmp = []; tmp +> new A(new B([]), buf); saved = new B(tmp);",
            "for (A& a : saved.xs) { println(a.s); }",
        )
        .replace("for (i64 round", "B saved = new B([]);\n    for (i64 round"),
        "a reference into `buf` can't be stored in `saved`",
    );
}

/// Item 2: a call through a function value does what any lambda with as many parameters
/// does to its arguments; a lambda passed to a built-in runs once for each element.
#[test]
fn function_values_and_built_in_lambdas_have_effects() {
    let pre = "i32[..~] a = [];\na +> 5;\n";
    rejects(
        &main_with(
            "",
            &format!(
                "{}func(i32[..~]& => void) g = (i32[..~]& b) => {{ @_ = b ->; }};\ni32& x = a[0];\ng(a);\na +> 777;\nprintln(\"${{x}}\");",
                pre
            ),
        ),
        "`g(...)` removes from `a` while `x` may still refer into it",
    );
    rejects(
        &main_with(
            "void apply(func(i32[..~]& => void) f, i32[..~]& xs) { f(xs); }",
            &format!(
                "{}i32& x = a[0];\napply((i32[..~]& b) => {{ b.clear(); }}, a);\na +> 777;\nprintln(\"${{x}}\");",
                pre
            ),
        ),
        "`apply(...)` removes from `a` while `x` may still refer into it",
    );
    // Run once per element: what one pass removes, a later one may still use.
    let arr = "i64[..~] a = [];\na +> 30 +> 10;\ni64[..~]& ar = a;\n";
    rejects(
        &main_with(
            "",
            &format!(
                "{}var m = a.map((i64 v) => {{ if (v == 30) {{ ar.clear(); ar +> 777; }} return v; }});\nprintln(\"${{m}}\");",
                arr
            ),
        ),
        "`clear` removes from `a` inside a for-each over it",
    );
    rejects(
        &main_with(
            "",
            &format!(
                "{}i64[..~] b = [];\nb +> 1 +> 2;\ni64& x = a[0];\nvar n = b.count((i64 v) => {{ println(\"${{x}}\"); ar.clear(); ar +> 777; return true; }});",
                arr
            ),
        ),
        "`clear` removes from `a` while `x` may still refer into it",
    );
    // A lambda's result can't outlive what it captures, when anything may call it.
    rejects(
        &escape(
            "void apply(string[..>]& out, func(i64 => string) f) { out +> f(0); }",
            "apply(keep, (i64 k) => { string v = buf; return v[k:15]; });",
            "println(keep[0]);",
        )
        .replace(
            "for (i64 round",
            "string[..>] keep = [];\n    for (i64 round",
        ),
        "this lambda returns a reference into `buf`, which it captures",
    );
    ok(&main_with(
        "",
        "i64[..>] xs = [];\nxs +> 3 +> 1;\nxs.sortBy((i64 v) => -v);\nstring all = \"\";\nvar n = xs.fold(0, (i64 acc, i64 v) => acc + v);\nprintln(\"${n}\");",
    ));
}

/// Item 5: replacing a whole value through a reference, or a class value holding an
/// array that can remove elements, removes what references point at.
#[test]
fn replacing_a_value_removes_its_elements() {
    rejects(
        &main_with(
            "",
            "i64[..~] a = [];\na +> 5;\ni64[..~]& ar = a;\ni64& r = a[0];\nar = [];\na +> 777;\nprintln(\"${r}\");",
        ),
        "assigning through `ar` removes from `a` while `r` may still refer into it",
    );
    let bag = "class Bag { i32[..~8] xs; }";
    let body = |change: &str| {
        format!(
            "Bag[..>] bags = [];\nbags +> new Bag([5]) +> new Bag([6]);\ni32& r = bags[0].xs[0];\n{}\nprintln(\"${{r}}\");",
            change
        )
    };
    for change in ["bags[0] = new Bag([]);", "bags.swap(0, 1);"] {
        rejects(
            &main_with(bag, &body(change)),
            "while `r` may still refer into it",
        );
    }
    rejects(
        &main_with(
            &format!("{}\nvoid reset(Bag& b) {{ b = new Bag([]); }}", bag),
            "Bag b = new Bag([5]);\ni32& r = b.xs[0];\nreset(b);\nprintln(\"${r}\");",
        ),
        "`reset(...)` removes from `b.xs` while `r` may still refer into it",
    );
}

/// Item 6: a constructor run while its value is built in place in an array can't change
/// that array (REFERENCES §6, V4).
#[test]
fn a_value_built_in_place_leaves_its_array_alone() {
    rejects(
        &main_with(
            "class Node { i64 key; Node&<i32>? next; new(Node[..>]& xs, i64 k) { xs.clear(); key = k; next = null; } }",
            "Node[..>] tree = [];\nNode& a = tree.push(new Node(tree, 1));",
        ),
        "`new Node` removes from the array this value is being built in, while it's built there",
    );
    rejects(
        &main_with(
            "class Rec { i64 id; u8[.] name; new(Rec[..>]& rs, i64 i) { rs +> new Rec(rs, 0); id = i; name = \"x\"; } }",
            "Rec[..>] recs = [];\nrecs +> new Rec(recs, 1);",
        ),
        "adds to the array this value is being built in",
    );
}

/// Item 8: a ring or gap buffer isn't given to a function checked for an array whose
/// elements stay where they are.
#[test]
fn moving_layouts_arent_passed_as_fixed_ones() {
    rejects(
        "void add(i64[..>]& xs, i64 v) { @_ = xs +> v; }\nvoid main() { i64[~8] g = [1, 2]; add(g, 3); }\n",
        "an `i64[~8]` can't be passed to `add`, which takes an `i64[..>]&`",
    );
    ok(
        "void add(i64[..>]& xs, i64 v) { @_ = xs +> v; }\nvoid main() { i64[..~8] g = [1, 2]; add(g, 3); println(\"${g}\"); }\n",
    );
}

/// Item 12: a lambda's parameters are rooted at what it is given: the elements, for a
/// built-in, and otherwise what its caller passed, which only its own locals can keep.
#[test]
fn lambda_parameters_have_roots() {
    rejects(
        &escape(
            "string saved = \"\";",
            "string[..>] parts = []; parts +> buf[0:15]; var n = parts.count((string s) => { saved = s; return true; });",
            "println(saved);",
        ),
        "can't be stored in a static",
    );
    rejects(
        "string saved = \"\";\nvoid remember(string s) { saved = s; }\nvoid main() { func(string => void) f = (string s) => remember(s); f(\"x\"); }\n",
        "the lambda's parameter `s` refers to what its caller passed",
    );
    ok(
        "void each(string[:] xs, func(string => void) f) { for (string x : xs) { f(x); } }\nvoid main() { each([\"a\", \"b\"], (string s) => { string t = s; println(t); }); }\n",
    );
}

/// Item 14: a constructor's `this` is moved into place, so no reference to it is kept.
#[test]
fn this_doesnt_escape_a_constructor() {
    rejects(
        "class Node { i64 v; Node&? me = null; new(i64 v) { this.v = v; me .= this; } }\nvoid main() { Node n = new Node(1); }\n",
        "a constructor's `this` is copied to where the value goes when it returns",
    );
    ok(
        "class Node { i64 v; Node&? me = null; new(i64 v) { this.v = v; } }\nvoid main() { Node n = new Node(1); n.me .= n; }\n",
    );
}

/// Item 15: a for-each over an iterator gives what its `next()` returns, rooted at the
/// loop's own copy of the iterator.
#[test]
fn for_each_over_an_iterator_follows_next() {
    rejects(
        "class Chars { u8[..~16] data; i64 at = 0;\n  mut string next() { if (at + 4 > data.len) { return #status.END \"\"; } at++; return data[at-1:at+3]; } }\nvoid main() {\n  string[..>] keep = [];\n  for (string s : new Chars(\"secret-AAAAAAAA\")) { if (keep.len == 0) { keep +> s; } }\n  println(keep[0]);\n}\n",
        "a reference into the for-each's iterator can't be stored in `keep`",
    );
}

/// Item 17: field initializers are checked as part of every constructor, and static
/// initializers as stores into their statics.
#[test]
fn initializers_are_checked() {
    rejects(
        "class Box { string label = \"box-${Box.count}\"; static i64 count = 7; }\nvoid main() { Box b = new Box(); println(b.label); }\n",
        "this text is a temporary, built only for this statement, so `this` can't keep a view of it",
    );
    rejects(
        "string saved = \"\";\ni64 keep(string s) { saved = s; return 1; }\ni64 X = keep(\"built-${X}\".trim());\nvoid main() { }\n",
        "this text is a temporary",
    );
}

/// Item 19: what a worker's for-each calls counts toward the statics it reaches.
#[test]
fn workers_follow_for_each_calls() {
    rejects(
        "import zink.thread.*;\nu8[..>] buffer = [];\nstring first = \"\";\nclass It { i64 n = 0; mut string next() { if (n > 0) { return #status.END \"\"; } n++; return first; } }\nvoid main() {\n    buffer.append(\"secret\");\n    first = buffer[0:6];\n    Worker w = thread.spawn(() => { for (string s : new It()) { println(s); } });\n    w.join();\n}\n",
        "can't use the static `first`: it holds views that may point into the spawning thread's memory",
    );
}

/// Item 20: a variable-size value isn't assigned through a reference (REFERENCES §6, V1).
#[test]
fn variable_size_values_arent_written_through_references() {
    let item = "class Item { u8[.*] sku; i32 qty; }";
    rejects(
        &main_with(
            item,
            "Item[..>] items = [];\nitems +> new Item(\"a\", 1);\nfor (Item& it : items) { it = new Item(\"much-longer\", 5); }",
        ),
        "a variable-size value can't be assigned through a reference",
    );
    rejects(
        &format!(
            "{}\nvoid set(Item& it) {{ it = new Item(\"b\", 2); }}\nvoid main() {{ }}\n",
            item
        ),
        "a variable-size value can't be assigned through a reference",
    );
}

/// Item 21: values with self-relative links aren't copied out of their array by a slice,
/// a built-in that stores its argument, or a view.
#[test]
fn links_arent_copied_by_slices_or_built_ins() {
    let node = "class Node { i64 key; Node&<i32>? next; }";
    let pre = "Node[..>] tree = [];\nNode& a = tree.push(new Node(1, null));\nNode& b = tree.push(new Node(2, a));\n";
    for copy in [
        "Node[..>] part = tree[1:];",
        "tree[0:1] = tree[1:2];",
        "tree.fill(tree[1]);",
        "pool<Node> p = []; var h = p.add(b);",
        "hashmap<i64, Node> m = []; m.put(1, b);",
        "Node[:]& w = tree[0:1]; w = tree[1:2];",
    ] {
        rejects(
            &main_with(node, &format!("{}{}", pre, copy)),
            "`Node` holds self-relative links, so it can't be copied",
        );
    }
    ok(&main_with(node, &format!("{}Node[..>] whole = tree;", pre)));
}

/// Item 1: a function that uses a reference into one argument after removing from
/// another is fine, until a call passes it the same storage for both (REFERENCES §4.2).
#[test]
fn calls_that_alias_what_a_function_keeps_apart() {
    let f = "void f(i64[..~]& a, i64[..~]& b) { i64& r = a[0]; b.clear(); b +> 777; println(\"${r}\"); }";
    rejects(
        &main_with(f, "i64[..~] x = [5];\nf(x, x);"),
        "`f(...)` removes from `b` while it still uses a reference into `a`, and in this call both are in `x`",
    );
    ok(&main_with(
        f,
        "i64[..~] x = [5];\ni64[..~] y = [6];\nf(x, y);",
    ));
    // A reference parameter, and a method call's receiver.
    rejects(
        &main_with(
            "void g(i32[..~]& a, i32& r) { @_ = a ->; a +> 777; println(\"${r}\"); }",
            "i32[..~] a = [5];\ng(a, a[0]);",
        ),
        "`g(...)` removes from `a` while it still uses `r`, and in this call both are in `a`",
    );
    rejects(
        &main_with(
            "void bump(i32& r, i32[..~]& a) { a.clear(); a +> 777; println(\"${r}\"); }",
            "i32[..~] a = [5];\na[0].bump(a);",
        ),
        "while it still uses `r`",
    );
    // `this` and an argument, and a static and an argument.
    rejects(
        &main_with(
            "class Bag {\n    i64[..~] xs;\n    mut void merge(Bag& other) { i64& r = xs[0]; other.xs.clear(); other.xs +> 777; println(\"${r}\"); }\n}",
            "Bag b = new Bag();\nb.xs +> 5;\nb.merge(b);",
        ),
        "`merge(...)` removes from `other` while it still uses a reference into `this`, and in this call both are in `b`",
    );
    rejects(
        "i64[..~] arr = [];\nvoid f(i64[..~]& a) { i64& r = a[0]; arr.clear(); arr +> 777; println(\"${r}\"); }\nvoid main() { arr +> 5; f(arr); }\n",
        "removes from the static `arr` while it still uses a reference into `a`",
    );
    // Through a function that passes the pair on.
    rejects(
        &main_with(
            &format!("{}\nvoid w(i64[..~]& p, i64[..~]& q) {{ f(p, q); }}", f),
            "i64[..~] x = [5];\nw(x, x);",
        ),
        "`w(...)`",
    );
    // What isn't used after the change, and growth, which moves nothing, are fine.
    ok(&main_with(
        "void g(i64[..~]& a, i64[..~]& b) { i64& r = a[0]; println(\"${r}\"); b.clear(); }\nvoid appendAll(i64[..>]& dst, i64[:] src) { for (i64 v : src) { dst +> v; } }",
        "i64[..~] x = [5];\ng(x, x);\ni64[..>] xs = [1, 2];\nappendAll(xs, xs);\nprintln(\"${xs}\");",
    ));
}

/// Item 1, through function values: a call may run any lambda with as many parameters,
/// so one that uses a reference into one parameter after removing from another makes a
/// call passing them the same storage an error (REFERENCES §4.3).
#[test]
fn calls_through_function_values_that_alias() {
    let lam =
        "(i64[..~]& a, i64[..~]& b) => { i64& r = a[0]; b.clear(); b +> 777; println(\"${r}\"); }";
    rejects(
        &main_with(
            "",
            &format!(
                "func(i64[..~]&, i64[..~]& => void) g = {};\ni64[..~] x = [5];\ng(x, x);",
                lam
            ),
        ),
        "`g(...)` may run a lambda that removes from its argument 2 while it still uses what its argument 1 refers to, and in this call both are in `x`",
    );
    let apply =
        "void apply(func(i64[..~]&, i64[..~]& => void) f, i64[..~]& p, i64[..~]& q) { f(p, q); }";
    rejects(
        &main_with(apply, &format!("i64[..~] x = [5];\napply({}, x, x);", lam)),
        "`apply(...)` removes from `q` while it still uses a reference into `p`, and in this call both are in `x`",
    );
    ok(&main_with(
        apply,
        &format!(
            "i64[..~] x = [5];\ni64[..~] y = [6];\napply({}, x, y);",
            lam
        ),
    ));
    rejects(
        &format!(
            "i64[..~] arr = [];\nvoid main() {{\n    arr +> 5;\n    func(i64[..~]& => void) g = (i64[..~]& a) => {{ i64& r = a[0]; arr.clear(); arr +> 777; println(\"${{r}}\"); }};\n    g(arr);\n}}\n"
        ),
        "may run a lambda that removes from the static `arr` while it still uses what its argument 1 refers to",
    );
}
