# The Zinkite compiler

The `zink` command, written in Rust (DESIGN §14.1). So far it has the front end:
a lexer and a parser for `GRAMMAR.md` v2. Type checking, root inference and the
C backend come next (DESIGN §14.3).

## Building and testing

```
cargo build
cargo test
cargo clippy --all-targets
```

The tests parse every program in `../examples` and the complete example in
`GRAMMAR.md` §7, check each disambiguation rule of GRAMMAR §6, and parse every
prefix of the examples to make sure broken input gives errors rather than a
crash.

## Using it

```
zink parse FILE...          report syntax errors, with the line and a caret
zink parse --tree FILE      print the syntax tree, one statement per line
zink lex FILE               print the tokens
```

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
| `src/main.rs` | the `zink` command |
| `tests/parse.rs` | parser tests |
