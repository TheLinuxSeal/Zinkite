# The Zinkite compiler

The `zink` command, written in Rust (DESIGN §14.1). So far it has the front end:
a lexer and a parser for `GRAMMAR.md` v2, and the first part of the type
checker. Root inference and the C backend come next (DESIGN §14.3).

## Building and testing

```
cargo build
cargo test
cargo clippy --all-targets
```

The parser tests parse every program in `../examples` and the complete example
in `GRAMMAR.md` §7, check each disambiguation rule of GRAMMAR §6, and parse
every prefix of the examples to make sure broken input gives errors rather than
a crash.

The checker tests type-check the same programs, which must give no errors, and
then check each rule with a program that breaks it and the message it gives. A
last test deletes or repeats each line of each example and checks the result,
to make sure the checker never crashes.

## Using it

```
zink check FILE...          parse and type-check each file as its own program
zink parse FILE...          report syntax errors, with the line and a caret
zink parse --tree FILE      print the syntax tree, one statement per line
zink lex FILE               print the tokens
```

## What the checker covers

Done:
- names: locals, fields, statics, classes, imports, type parameters, and a
  local hiding a class;
- types: literals typed by where they are used, the implicit conversions, the
  mixed-integer rule, casts, conditions, `null` and narrowing;
- calls: overloading and its ranking, generic inference and bounds, lambdas,
  constructors (including default ones and `this.new`), and free functions
  called as methods;
- arrays: which methods and operators each slot type allows, slices and
  writable views;
- statuses: `#` and `@`, repeat loops, and a warning when a status that can be
  nonzero is dropped;
- statements: `switch` (exhaustive as an expression), `yield`, labels, `defer`,
  tuples, for-each over every kind of collection and iterator, and missing
  `return`s;
- declarations: interfaces and `implements ordered`, overloads that differ only
  in `&`, hashable keys, and the field rules for classes and roots;
- definite assignment (DESIGN §6.2): every field assigned on every path through
  a constructor, judged by the worst case, with failing exits, `this.new`, the
  fields that have no zero value, and no use of `this` before then; and no local
  read before every path has assigned it.

- `mut` and read-only checks: plain parameters are read-only copies, a method
  that isn't `mut` can't change its object or call one that is, a writable view
  needs an array the code may change, and a lambda's captures are read-only and
  never assigned after they're initialized. A reference bound to something the
  code can't change can't be written through.

Not yet:
- roots and lifetimes (`REFERENCES.md`): rules S and R, and copying roots;
- the lint for unused array capabilities (DESIGN §4.3).

The standard library isn't written yet. The checker models `zink.lang`
(output, `status`, `min`, `max` and the rest), `math`, and the methods on
numbers, strings, arrays, `hashmap`, `hashset` and `pool`. Names from the other
`zink.*` packages, such as `File` and `Args`, aren't checked, and a file that
imports one gets a warning saying so.

## Layout

| File | Contents |
|---|---|
| `src/source.rs` | source files and byte spans |
| `src/diag.rs` | diagnostics and how they are printed |
| `src/token.rs` | tokens, keywords and operators (GRAMMAR §1) |
| `src/lexer.rs` | text to tokens, including string interpolation and `#` directives |
| `src/ast.rs` | the syntax tree |
| `src/parser.rs` | tokens to the syntax tree (GRAMMAR §2–§6) |
| `src/sexpr.rs` | the tree as compact S-expressions, for tests and `--tree` |
| `src/types.rs` | the checker's types, array kinds and generic bounds |
| `src/program.rs` | every declaration in a program, and resolving written types |
| `src/builtins.rs` | signatures of the built-in functions and methods |
| `src/check.rs` | the type checker |
| `src/flow.rs` | the state definite assignment tracks: what is assigned on every path, and on some |
| `src/main.rs` | the `zink` command |
| `tests/parse.rs` | parser tests |
| `tests/check.rs` | type checker tests |
