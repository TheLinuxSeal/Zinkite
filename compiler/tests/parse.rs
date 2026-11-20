//! Parser tests: the example programs, GRAMMAR's complete example, and the
//! cases GRAMMAR §1 and §6 single out.

use zink::ast::{Member, StmtKind, TopItem, TypeDeclKind};
use zink::diag::render;
use zink::source::SourceFile;
use zink::{lexer, parser, sexpr};

/// Parses a file and fails the test with rendered diagnostics if there are any.
fn parse_ok(name: &str, text: &str) -> zink::ast::File {
    let file = SourceFile::new(name, text);
    let (ast, diags) = zink::parse(&file);
    if !diags.is_empty() {
        let shown: String = diags.iter().map(|d| render(&file, d)).collect();
        panic!("{} had errors:\n{}", name, shown);
    }
    ast
}

/// The error messages (and help lines) a file produces.
fn file_errors(text: &str) -> Vec<String> {
    let file = SourceFile::new("test.zn", text);
    let (_, diags) = zink::parse(&file);
    diags
        .iter()
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

/// An expression as an S-expression; fails on any diagnostic.
fn expr(src: &str) -> String {
    let (toks, lex_diags) = lexer::lex(src);
    assert!(
        lex_diags.is_empty(),
        "lexer errors for {:?}: {:?}",
        src,
        lex_diags
    );
    let (e, diags) = parser::parse_expr(toks);
    assert!(diags.is_empty(), "parse errors for {:?}: {:?}", src, diags);
    sexpr::expr(&e.unwrap())
}

fn expr_errors(src: &str) -> Vec<String> {
    let (toks, mut diags) = lexer::lex(src);
    let (_, parse_diags) = parser::parse_expr(toks);
    diags.extend(parse_diags);
    diags
        .iter()
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

/// Statements, one S-expression each; fails on any diagnostic.
fn stmts(src: &str) -> Vec<String> {
    let (toks, lex_diags) = lexer::lex(src);
    assert!(
        lex_diags.is_empty(),
        "lexer errors for {:?}: {:?}",
        src,
        lex_diags
    );
    let (ss, diags) = parser::parse_stmts(toks);
    assert!(diags.is_empty(), "parse errors for {:?}: {:?}", src, diags);
    ss.iter().map(sexpr::stmt).collect()
}

fn stmt(src: &str) -> String {
    let ss = stmts(src);
    assert_eq!(
        ss.len(),
        1,
        "expected one statement in {:?}, got {:?}",
        src,
        ss
    );
    ss.into_iter().next().unwrap()
}

fn stmt_errors(src: &str) -> Vec<String> {
    let (toks, mut diags) = lexer::lex(src);
    let (_, parse_diags) = parser::parse_stmts(toks);
    diags.extend(parse_diags);
    diags
        .iter()
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

/// The type of the single field declared in `class C { <decl> }`.
fn field_type(decl: &str) -> String {
    let ast = parse_ok("field.zn", &format!("class C {{ {} }}", decl));
    let TopItem::Type(t) = &ast.items[0] else {
        panic!()
    };
    let TypeDeclKind::Class(c) = &t.kind else {
        panic!()
    };
    let Member::Field(f) = &c.members[0] else {
        panic!("not a field: {:?}", c.members[0])
    };
    sexpr::ty(&f.ty)
}

fn has(errors: &[String], needle: &str) -> bool {
    errors.iter().any(|e| e.contains(needle))
}

// ---------------------------------------------------------------- whole files

#[test]
fn example_programs_parse() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "zn") {
            let text = std::fs::read_to_string(&path).unwrap();
            parse_ok(&path.display().to_string(), &text);
            count += 1;
        }
    }
    assert!(
        count >= 8,
        "expected at least 8 example programs, found {}",
        count
    );
}

#[test]
fn grammar_complete_example_parses() {
    let grammar =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../GRAMMAR.md")).unwrap();
    let section = grammar.find("## 7. A complete example").unwrap();
    let start = grammar[section..].find("```\n").unwrap() + section + 4;
    let end = grammar[start..].find("\n```").unwrap() + start;
    let ast = parse_ok("GRAMMAR.md §7", &grammar[start..end]);
    assert_eq!(ast.settings.len(), 1);
    assert_eq!(ast.items.len(), 6); // Shape, Circle, Rect, Stats, parseSize, main
}

#[test]
fn roots_and_generic_bounds() {
    let ast = parse_ok(
        "root.zn",
        "root LruCache<K implements hashable & ordered> { pool<Node<K>> nodes; new(i64 c) { this.new(c, 1); } }\n\
         <T implements number> T twice(T x) { return (T) (x * 2); }",
    );
    let TopItem::Type(t) = &ast.items[0] else {
        panic!()
    };
    let TypeDeclKind::Root(c) = &t.kind else {
        panic!("not a root")
    };
    assert_eq!(c.type_params[0].bounds.len(), 2);
    let TopItem::Method(m) = &ast.items[1] else {
        panic!()
    };
    let body = m.body.as_ref().unwrap();
    assert_eq!(sexpr::stmt(&body.stmts[0]), "(return (cast T (* x 2)))");
}

#[test]
fn root_is_a_keyword_only_before_a_declaration() {
    assert_eq!(
        stmt("Node& root = tree.push(n);"),
        "(local Node& root = (call tree.push n))"
    );
    assert_eq!(stmt("root.left .= x;"), "(.= root.left x)");
}

#[test]
fn directives() {
    let ast = parse_ok(
        "d.zn",
        "#max_memory_mb 64\n#if debug\n#data_stack_gb 4\n#endif\n\
         #if os(linux) && !arch(aarch64)\nvoid f() {}\n#else\nvoid g() {}\n#endif\n",
    );
    assert_eq!(ast.settings.len(), 2);
    assert!(matches!(ast.items[0], TopItem::If(_)));
}

#[test]
fn status_write_on_the_line_after_return() {
    let s = stmts("return\n    #status.NOT_FOUND x;\n");
    assert_eq!(s, vec!["(return #status.NOT_FOUND x)"]);
    assert_eq!(stmt("return #why;"), "(return #why)");
    assert_eq!(
        stmt("return #(bad || d <= 0.0) d;"),
        "(return #((|| bad (<= d 0.0))) d)"
    );
    assert_eq!(
        stmt("return #status.INVALID (0, 0);"),
        "(return #status.INVALID (tuple 0 0))"
    );
}

// ---------------------------------------------------------------- lexing

#[test]
fn number_literals() {
    assert_eq!(expr("255u8"), "255u8");
    assert_eq!(expr("0x1f32"), "7986"); // the integer 0x1F32: float suffixes are decimal only
    assert_eq!(expr("2f32"), "2f32");
    assert_eq!(expr("1.5e3f64"), "1.5e3f64");
    assert_eq!(expr("1_000_000"), "1000000");
    assert_eq!(expr("0b1010"), "10");
    assert_eq!(expr("1.hex()"), "(call 1.hex)");
    assert_eq!(expr("'\\u{1F600}'"), "'😀'");
    assert!(has(&expr_errors("12abc"), "isn't a number suffix"));
    assert!(has(&expr_errors("1_"), "can only separate digits"));
    assert!(has(&expr_errors("99999999999999999999999"), "too large"));
}

#[test]
fn strings_and_interpolation() {
    assert_eq!(expr("\"a ${x + 1} b\""), "\"a ${(+ x 1)} b\"");
    assert_eq!(
        expr("\"${f == \"\" ? \"stdin\" : f}:${e.line}\""),
        "\"${(? (== f \"\") \"stdin\" f)}:${e.line}\""
    );
    assert_eq!(expr("\"cost: \\$5\""), "\"cost: $5\"");
    assert_eq!(expr("\"${n.pad(8)}\""), "\"${(call n.pad 8)}\"");
    assert!(has(&expr_errors("\"open"), "isn't closed"));
    assert!(has(&expr_errors("\"${x\""), "no matching `}`"));
}

#[test]
fn longest_match() {
    assert_eq!(stmt("i-->0;"), "(> (post-- i) 0)");
    assert!(has(
        &stmt_errors("x = a ^-b;"),
        "`^-` removes the element after the cursor"
    ));
    assert_eq!(stmt("x = a ^ -b;"), "(= x (^ a (-u b)))");
}

// ---------------------------------------------------------------- precedence

#[test]
fn binary_precedence() {
    assert_eq!(expr("a + b * c"), "(+ a (* b c))");
    assert_eq!(expr("a || b && c"), "(|| a (&& b c))");
    assert_eq!(expr("a == b < c"), "(== a (< b c))");
    assert_eq!(expr("a & b == c"), "(& a (== b c))");
    assert_eq!(expr("a << 2 + 1"), "(<< a (+ 2 1))");
    assert_eq!(expr("a - b - c"), "(- (- a b) c)");
    assert_eq!(expr("x +% y *| z"), "(+% x (*| y z))");
    assert_eq!(expr("shapes +> c.push(x)"), "(+> shapes (call c.push x))");
    assert_eq!(expr("text +^ 'h' +^ 'i'"), "(+^ (+^ text 'h') 'i')");
    assert_eq!(expr("a +> b < c"), "(< (+> a b) c)");
    assert_eq!(
        expr("s instanceof Circle c && c.r > 1.0"),
        "(&& (instanceof s Circle c) (> c.r 1.0))"
    );
}

#[test]
fn assignment_and_conditional() {
    assert_eq!(expr("x = y = z"), "(= x (= y z))");
    assert_eq!(expr("a ? b : c ? d : e"), "(? a b (? c d e))");
    assert_eq!(expr("x += a ? 1 : 2"), "(+= x (? a 1 2))");
    assert_eq!(expr("r .= items[1]"), "(.= r (index items 1))");
    assert_eq!(expr("x @err = f()"), "(= @err x (call f))");
    assert_eq!(expr("@err = f()"), "(@= err (call f))");
    assert_eq!(expr("@_ = stack ->"), "(@= _ (-> stack))");
}

#[test]
fn unary_status_and_postfix() {
    assert_eq!(expr("-a.b()"), "(-u (call a.b))");
    assert_eq!(expr("!@a.pop()"), "(! (@ (call a.pop)))");
    assert_eq!(expr("@a + b"), "(+ (@ a) b)");
    assert_eq!(expr("@(a +> x)"), "(@ (+> a x))");
    assert_eq!(expr("todo ->"), "(-> todo)");
    assert_eq!(expr("text ^-"), "(^- text)");
    assert_eq!(expr("a.b[i].c(1)++"), "(post++ (call (index a.b i).c 1))");
    assert_eq!(expr("i64.parse(s)"), "(call i64.parse s)");
    assert_eq!(expr("u64.MAX"), "u64.MAX");
}

// ---------------------------------------------------------------- disambiguation (GRAMMAR §6)

#[test]
fn rule1_lambda_destructuring_or_parentheses() {
    assert_eq!(
        expr("(i32 a, i32 b) => a + b"),
        "(lambda (i32 a, i32 b) (+ a b))"
    );
    assert_eq!(
        expr("(Row a) => { return a.num; }"),
        "(lambda (Row a) {1 stmts})"
    );
    assert_eq!(expr("() => 1"), "(lambda () 1)");
    assert_eq!(expr("(a + b) * c"), "(* (+ a b) c)");
    assert_eq!(
        expr("f((Keyed k) => k.num, 2)"),
        "(call f (lambda (Keyed k) k.num) 2)"
    );
    assert_eq!(stmt("(a, b) = f();"), "(destructure (a, b) (call f))");
    assert_eq!(
        stmt("(string w, _) = f(x);"),
        "(destructure (string w, _) (call f x))"
    );
    assert_eq!(
        stmt("(i64 q, i64 r) @err = divMod(a, b);"),
        "(destructure (i64 q, i64 r) @err (call divMod a b))"
    );
    assert_eq!(stmt("(x, y) = (y, x);"), "(destructure (x, y) (tuple y x))");
    assert!(has(
        &expr_errors("(a, b) => a + b"),
        "needs a type and a name"
    ));
    assert!(has(&expr_errors("(var a) => a"), "`var` isn't allowed"));
}

#[test]
fn rule2_casts() {
    assert_eq!(expr("(i64) x"), "(cast i64 x)");
    assert_eq!(expr("(u8%) n"), "(cast u8% n)");
    assert_eq!(expr("(u8|) -5"), "(cast u8| (-u 5))");
    assert_eq!(expr("(f64) a.len * 2.0"), "(* (cast f64 a.len) 2.0)");
    assert_eq!(expr("(x) + 1"), "(+ x 1)");
    // `(T) x` is a cast only where T is a type parameter in scope:
    assert!(!expr_errors("(T) x").is_empty());
}

#[test]
fn rule3_closing_type_arguments() {
    assert_eq!(
        field_type("Pair<string, Pair<i32, i32>> p;"),
        "Pair<string, Pair<i32, i32>>"
    );
    assert_eq!(
        field_type("hashmap<K, Handle<Node<K>>> index;"),
        "hashmap<K, Handle<Node<K>>>"
    );
    assert_eq!(
        field_type("hashmap<string, u8[..>16]> m;"),
        "hashmap<string, u8[..>16]>"
    );
    // A `>>` that isn't closing type arguments stays a shift:
    assert_eq!(stmt("x = a >> 2;"), "(= x (>> a 2))");
}

#[test]
fn rule4_declaration_or_expression() {
    assert_eq!(
        stmt("Item& r = items[0];"),
        "(local Item& r = (index items 0))"
    );
    assert_eq!(
        stmt("u8[..>] scratch = [];"),
        "(local u8[..>] scratch = [])"
    );
    assert_eq!(
        stmt("Shape&[..>] shapes = [];"),
        "(local Shape&[..>] shapes = [])"
    );
    assert_eq!(
        stmt("Item x @failed = a.pop();"),
        "(local Item x @failed = (call a.pop))"
    );
    assert_eq!(
        stmt("items[0].heavier(1.0);"),
        "(call (index items 0).heavier 1.0)"
    );
    assert_eq!(stmt("x @failed = f();"), "(= @failed x (call f))");
    assert_eq!(stmt("a[~16] = 5;"), "(= (index a (~ 16)) 5)");
    assert_eq!(
        stmt("var& c = counts.getOrAdd(w, 0);"),
        "(local var& c = (call counts.getOrAdd w 0))"
    );
    assert_eq!(stmt("i64 a = 1, b = 2;"), "(local i64 a = 1, b = 2)");
}

#[test]
fn types() {
    assert_eq!(field_type("Node&<i32>? left;"), "Node&<i32>?");
    assert_eq!(field_type("Node&<u32 in nodes> n;"), "Node&<u32 in nodes>");
    assert_eq!(field_type("Item[.~.$] t;"), "Item[.~.$]");
    assert_eq!(field_type("u32[~$] t;"), "u32[~$]");
    assert_eq!(field_type("u8[.*] s;"), "u8[.*]");
    assert_eq!(field_type("u8[..~16] s;"), "u8[..~16]");
    assert_eq!(field_type("i32[.8] s;"), "i32[.8]");
    assert_eq!(field_type("T[>>.] s;"), "T[>>.]");
    assert_eq!(field_type("Item[..>MAX_ITEMS] s;"), "Item[..>MAX_ITEMS]");
    assert_eq!(field_type("string[:]& v;"), "string[:]&");
    assert_eq!(
        field_type("func(i32, i32 => i32)? f;"),
        "func(i32, i32 => i32)?"
    );
    assert_eq!(field_type("func(void => void) f;"), "func(void => void)");
    assert_eq!(
        field_type("func(string => (string, string)) f;"),
        "func(string => (string, string))"
    );
    assert_eq!(field_type("zink.io.File f;"), "zink.io.File");
}

#[test]
fn type_errors() {
    assert!(has(
        &file_errors("class C { u8[..] x; }"),
        "one or three slot characters"
    ));
    assert!(has(&file_errors("class C { Foo[16] x; }"), "write `T[.k]`"));
    assert!(has(
        &file_errors("class C { func(string) f; }"),
        "a function type writes both sides"
    ));
    assert!(has(
        &file_errors("class C { long x; }"),
        "the type is spelled `i64`"
    ));
    assert!(has(&file_errors("class C extends B { }"), "no inheritance"));
}

#[test]
fn rules5_6_13_index_and_slice() {
    assert_eq!(expr("a[1:2]"), "(slice a 1 2)");
    assert_eq!(expr("a[:3]"), "(slice a _ 3)");
    assert_eq!(expr("a[2:]"), "(slice a 2 _)");
    assert_eq!(expr("a[:]"), "(slice a _ _)");
    assert_eq!(expr("a[1:^1]"), "(slice a 1 ^1)");
    assert_eq!(expr("a[^2:]"), "(slice a ^2 _)");
    assert_eq!(expr("a[^1]"), "(index^ a 1)");
    assert_eq!(expr("a[%i]"), "(index% a i)");
    assert_eq!(expr("a[|i]"), "(index| a i)");
    assert_eq!(expr("a[k + 1:]"), "(slice a (+ k 1) _)");
    assert_eq!(expr("a[c ? i : j]"), "(index a (? c i j))");
    assert_eq!(expr("a[(c ? i : j):]"), "(slice a (? c i j) _)");
    assert_eq!(expr("a[0:(n & 7)]"), "(slice a 0 (& n 7))");
    assert!(has(&expr_errors("a[0:n & 7]"), "needs parentheses"));
    assert!(has(&expr_errors("a[c ? i : j : k]"), "needs parentheses"));
    assert!(has(&expr_errors("m[i, j]"), "multi-dimensional"));
    assert_eq!(expr("[1, 2, 3,]"), "[1 2 3]");
    assert!(has(&expr_errors("[\"a\": 1]"), "hash map literals"));
}

#[test]
fn rule7_status_destinations() {
    assert_eq!(
        stmt("defer @_ = d.close();"),
        "(defer (@= _ (call d.close)))"
    );
    assert_eq!(stmt("u8 err = @f();"), "(local u8 err = (@ (call f)))");
}

#[test]
fn rule12_while_headers() {
    assert_eq!(stmt("while (a ? b : c) { }"), "(while (? a b c))");
    assert_eq!(stmt("while (i < n) i++;"), "(while (< i n))");
    assert_eq!(
        stmt("while (Pending p : todo ->) { }"),
        "(repeat Pending p : (-> todo))"
    );
    assert_eq!(
        stmt("while (JsonEvent e : r.next()) @why { }"),
        "(repeat JsonEvent e : (call r.next) @why)"
    );
    assert_eq!(
        stmt("while ((string k, string v) : ini.next()) { }"),
        "(repeat (string k, string v) : (call ini.next))"
    );
    assert_eq!(
        stmt("while (!@stdin.readLine(line)) { }"),
        "(while (! (@ (call stdin.readLine line))))"
    );
    // With a capture, `@err = g();` after the header is the body, not a capture.
    assert_eq!(stmts("while (T x : f()) @err = g();").len(), 1);
}

#[test]
fn loops_switch_and_labels() {
    assert_eq!(
        stmt("for (u32 c : s.codePoints()) { }"),
        "(foreach u32 c : (call s.codePoints))"
    );
    assert_eq!(
        stmt("for (var& e : counts) { }"),
        "(foreach var& e : counts)"
    );
    assert_eq!(
        stmt("for (var _ : items) { n++; }"),
        "(foreach var _ : items)"
    );
    assert_eq!(stmt("for (var i = 0; i < 10; i++) { }"), "(for (< i 10))");
    assert_eq!(stmt("for (;;) { break; }"), "(for _)");
    assert_eq!(
        stmt("outer: for (;;) { break outer; }"),
        "(label outer (for _))"
    );
    assert_eq!(stmt("do { x++; } while (x < 3);"), "(do-while (< x 3))");
    let s = stmt("switch (cmd) { case \"a\", \"b\" => f(); case -1 => { g(); } default => {} }");
    assert_eq!(s, "(switch cmd 3 rules)");
    assert_eq!(
        stmt("string size = switch (n) { case 0 => \"none\"; default => \"many\"; };"),
        "(local string size = (switch n 2 rules))"
    );
    assert!(has(
        &stmt_errors("switch (x) { case 1: f(); }"),
        "never fall through"
    ));
}

#[test]
fn constructors_and_members() {
    let ast = parse_ok(
        "c.zn",
        "class Rect {\n  f64 w; f64 h;\n  new(f64 w, f64 h) { this.w = w; this.h = h; }\n  new(f64 side) { this.new(side, side); }\n  public static i64 count = 0;\n  (string, string) split(string s) { return (s, s); }\n}\n\
         interface Shape { f64 area(); mut void grow(f64 by); }\nenum Color { RED, GREEN, BLUE, }\n",
    );
    let TopItem::Type(t) = &ast.items[0] else {
        panic!()
    };
    let TypeDeclKind::Class(c) = &t.kind else {
        panic!()
    };
    assert_eq!(c.members.len(), 6);
    let Member::Ctor(ctor) = &c.members[3] else {
        panic!()
    };
    assert!(matches!(ctor.body.stmts[0].kind, StmtKind::Expr(_)));
    assert_eq!(sexpr::stmt(&ctor.body.stmts[0]), "(this.new side side)");
}

#[test]
fn undecided_features_are_rejected() {
    assert!(has(&expr_errors("a?.b"), "`?.` is undecided"));
    assert!(has(&file_errors("#elif x\n"), "`#elif` is undecided"));
    assert!(has(
        &file_errors("class C { class D { } }"),
        "nested type declarations are undecided"
    ));
    assert!(has(
        &file_errors("interface I { void f() { } }"),
        "default methods are undecided"
    ));
    assert!(has(&file_errors("enum E { A; }"), "enums with fields"));
    assert!(has(
        &file_errors("class C { static { } }"),
        "static initializer blocks"
    ));
}

#[test]
fn reserved_words_explain_themselves() {
    assert!(has(&stmt_errors("try { f(); }"), "nothing is thrown"));
    assert!(has(
        &stmt_errors("double d = 1.0;"),
        "the type is spelled `f64`"
    ));
    assert!(has(
        &file_errors("class C { void delete() { } }"),
        "use `defer`"
    ));
}

#[test]
fn errors_recover_and_keep_going() {
    // Two broken statements: both are reported, and the method after them still parses.
    let errs = file_errors("void f() {\n  x = ;\n  y = = 2;\n}\nvoid g() { return 1; }\n");
    assert_eq!(errs.len(), 2, "{:?}", errs);
}

/// Every prefix of every example program, cut every few bytes, must parse to
/// completion with errors rather than panic or loop.
#[test]
fn truncated_input_never_panics() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "zn") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let mut cut = 0;
        while cut < text.len() {
            if text.is_char_boundary(cut) {
                let file = SourceFile::new("cut.zn", &text[..cut]);
                let _ = zink::parse(&file);
            }
            cut += 3;
        }
    }
}
