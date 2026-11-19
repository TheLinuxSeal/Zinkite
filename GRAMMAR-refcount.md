# Zinkite Grammar (v1)

> Companion to `DESIGN.md`. It covers everything marked Decided there. Features still Open are listed in §8 and are not in this grammar yet.
> **Updated:** 2026-11-15.

**Notation**
- `rule = ... ;` defines a rule.
- `"text"` is a terminal: a keyword or a symbol.
- `UPPER_CASE` names a token from the lexical grammar (§1).
- `a b` is a sequence, `a | b` a choice, `[ a ]` optional, `{ a }` zero or more, `( ... )` grouping.

---

## 1. Lexical grammar

### 1.1 Source text
- Files are UTF-8 with the extension `.zn`. A `\r` before `\n` is ignored.
- Line 1 may be a shebang: `#!` to the end of the line, ignored by the compiler.

### 1.2 Whitespace and comments
- Whitespace: space, tab, newline. It separates tokens and is otherwise ignored.
- `// ...` runs to the end of the line. `/* ... */` may span lines and does not nest.

### 1.3 Identifiers and keywords
```
IDENT   = LETTER { LETTER | DIGIT } ;          (* not a keyword *)
LETTER  = "A".."Z" | "a".."z" | "_" ;
DIGIT   = "0".."9" ;
```

Reserved keywords:

| Group | Keywords |
|---|---|
| Types | `bool` `byte` `char` `int` `long` `float` `double` `string` `void` `func` |
| Declarations | `package` `import` `as` `class` `struct` `interface` `enum` `extends` `implements` `abstract` `final` `static` `public` `protected` `private` `delete` (`new` is listed under Expressions) |
| Reference kinds and modes | `owned` `owner` `weak` `borrow` `fresh` |
| Statements | `if` `else` `while` `do` `for` `break` `continue` `return` `switch` `case` `default` `yield` `try` `catch` `finally` `throw` `var` |
| Expressions | `new` `this` `super` `null` `true` `false` `instanceof` `take` `replace` |

`array` and `nullarray` are built-in type names. They are not keywords, but no declaration may reuse them.

### 1.4 Literals
```
INT       = ( DEC | HEX | BIN ) [ "L" ] ;
DEC       = DIGIT { DIGIT | "_" } ;
HEX       = "0x" HEXDIGIT { HEXDIGIT | "_" } ;
BIN       = "0b" BINDIGIT { BINDIGIT | "_" } ;
FLOAT     = DEC "." DEC [ EXPONENT ] [ "f" ]
          | DEC EXPONENT [ "f" ]
          | DEC "f" ;
EXPONENT  = ( "e" | "E" ) [ "+" | "-" ] DEC ;
CHAR      = "'" ( CHAR_CHAR | ESCAPE ) "'" ;
STRING    = '"' { STRING_CHAR | ESCAPE | INTERP } '"' ;
INTERP    = "${" expression "}" ;
ESCAPE    = "\" ( "n" | "t" | "r" | "0" | "\" | "'" | '"' | "$" | "u{" HEXDIGIT { HEXDIGIT } "}" ) ;
```

- An `INT` without `L` is an `int` and must fit in 32 bits; with `L` it is a `long`. A `FLOAT` is a `double`, or a `float` with the `f` suffix.
- `_` separates digits (`1_000_000`). It can't start or end a number.
- `2147483648` (and the `long` equivalent) is allowed only directly after unary `-`, so `int` and `long` minimums can be written, as in Java.
- `\u{...}` is a Unicode code point, at most `10FFFF`. A `char` holds one code point (DESIGN §12.1).
- In a string, `${` starts an interpolation, which ends at the matching `}`. The embedded expression is lexed recursively and may contain strings and braces. A `$` not followed by `{` is an ordinary character; `\$` writes a literal `$`.
- Strings can't span lines (multi-line strings are Open, §8).

### 1.5 Operators and punctuation
```
(  )  {  }  [  ]  ;  ,  .  :  ?  @  ->
=  +=  -=  *=  /=  %=  &=  |=  ^=  <<=  >>=  >>>=  +%=  -%=  *%=  +|=  -|=  *|=
||  &&  |  ^  &  ==  !=  <  >  <=  >=  <<  >>  >>>
+  -  *  /  %  +%  -%  *%  +|  -|  *|
!  ~  ++  --
```

The lexer takes the longest match. Inside type arguments, the parser splits `>>` and `>>>` into separate `>` tokens (§6, rule 3).

### 1.6 Directive lines
A line whose first non-blank character is `#` is a directive, except the shebang on line 1. The lexer turns each directive line into one token:

```
SETTING   = "#" IDENT VALUE ;            (* #leakcheck ON, #max_heap_mb 256 *)
HASH_IF   = "#if" condition ;
HASH_ELSE = "#else" ;
HASH_END  = "#endif" ;

condition = cond_and { "||" cond_and } ;
cond_and  = cond_not { "&&" cond_not } ;
cond_not  = "!" cond_not | "(" condition ")" | IDENT [ "(" IDENT ")" ] ;
```

- `VALUE` is the rest of the line: a word such as `ON`, or an integer.
- Condition names are build flags (`debug`, `--define color` gives `color`), the target (`os(linux)`, `arch(aarch64)`) and settings (`leakcheck(on)`); see DESIGN §11.3.
- `#if` blocks are part of the syntax tree (§2, §4, §5). Both branches must parse; only the active one is type-checked.

---

## 2. Files

```
file          = [ SHEBANG ] { setting_item } [ package_decl ] { import_decl } { top_item } ;

setting_item  = SETTING
              | HASH_IF { setting_item } [ HASH_ELSE { setting_item } ] HASH_END ;

package_decl  = "package" qualified_name ";" ;
import_decl   = "import" qualified_name [ "." "*" | "as" IDENT ] ";" ;
qualified_name = IDENT { "." IDENT } ;

top_item      = type_decl
              | field_decl                     (* script files only *)
              | method_decl                    (* script files only *)
              | HASH_IF { top_item } [ HASH_ELSE { top_item } ] HASH_END ;
```

- Settings are allowed only in the file that contains the entry point (DESIGN §11.2).
- A file with top-level fields or methods is a **script file** (DESIGN §11.6): it may not have a `package` line, and its top-level fields and methods are static members of a hidden class.
- Otherwise the file holds one public top-level type named after the file, plus any non-public ones (DESIGN §11.4).

---

## 3. Types

```
type           = base_type [ "?" ] ;
base_type      = primitive_type
               | "string"
               | class_type
               | func_type ;
primitive_type = "bool" | "byte" | "char" | "int" | "long" | "float" | "double" ;
class_type     = qualified_name [ type_args ] ;
type_args      = "<" type_arg { "," type_arg } ">" ;
type_arg       = [ "owned" | "weak" ] type
               | INT ;                          (* only as the rank of array/nullarray *)

func_type      = "func" "(" func_params "->" func_result ")" ;
func_params    = "void" | param_type { "," param_type } ;
param_type     = [ "borrow" | "fresh" ] type ;
func_result    = "void" | [ "borrow" | "fresh" ] type ;

return_type    = "void" | [ "borrow" | "fresh" ] type ;
```

Checked after parsing:
- `?` is allowed only on reference types: `int?` and a struct with `?` are errors. `weak` slots are written without `?` and read as nullable (DESIGN §8.1).
- `array<T>` takes an optional rank of 2 or 3: `array<int, 2>`. `nullarray<T?>` appears only after `new` (DESIGN §8.2).
- `func(...)` always writes both sides: `func(void -> void)`, `func(string -> void)`, `func(void -> bool)`, `func(int, int -> int)`. `func()` and `func(string)` are errors (DESIGN §12.5).
- A nullable function type puts `?` after the closing parenthesis: `func(void -> Dog)?`.

---

## 4. Declarations

```
annotations    = { "@" IDENT } ;                          (* none are defined yet *)
modifier       = "public" | "protected" | "private" | "static" | "final" | "abstract" ;

type_decl      = annotations { modifier }
                 ( class_decl | struct_decl | interface_decl | enum_decl ) ;

class_decl     = "class" IDENT [ type_params ]
                 [ "extends" class_type ]
                 [ "implements" class_type { "," class_type } ]
                 class_body ;
struct_decl    = "struct" IDENT [ type_params ]
                 [ "implements" class_type { "," class_type } ]
                 class_body ;
interface_decl = "interface" IDENT [ type_params ]
                 [ "extends" class_type { "," class_type } ]
                 "{" { interface_member } "}" ;
enum_decl      = "enum" IDENT "{" [ IDENT { "," IDENT } [ "," ] ] "}" ;

type_params    = "<" type_param { "," type_param } ">" ;
type_param     = IDENT [ "extends" class_type ] ;

class_body     = "{" { member } "}" ;
member         = field_decl
               | method_decl
               | constructor_decl
               | destructor_decl
               | type_decl                              (* nested types are implicitly static *)
               | HASH_IF { member } [ HASH_ELSE { member } ] HASH_END
               | ";" ;
interface_member = annotations { modifier } [ type_params ] method_header ";"
               | HASH_IF { interface_member } [ HASH_ELSE { interface_member } ] HASH_END ;

field_decl     = annotations { modifier } [ field_kind ] type
                 var_declarator { "," var_declarator } ";" ;
field_kind     = "owned" | "owner" | "weak" ;
var_declarator = IDENT [ "=" expression ] ;

method_decl    = annotations { modifier } [ type_params ] method_header ( block | ";" ) ;
method_header  = return_type IDENT "(" [ param { "," param } ] ")" ;
param          = [ "final" ] [ "borrow" | "fresh" ] type IDENT ;

constructor_decl = annotations { modifier } "new" "(" [ param { "," param } ] ")" block ;
destructor_decl  = "delete" "(" ")" block ;
```

Checked after parsing:
- Constructors are written `new(...)` and have no return type (DESIGN §8.2). A class has at most one `delete()`, which takes no modifiers; structs and interfaces have none (DESIGN §10.2). A struct can't use `extends`, and has no `abstract` or `owned`/`owner` fields (DESIGN §9).
- A method body is `;` only for `abstract` methods and interface methods.
- `owner` fields have no initializer. `static owned` declares a permanent object (DESIGN §6.3).
- In a script file, top-level declarations take no access modifiers.

---

## 5. Statements

```
block         = "{" { statement } "}" ;

statement     = block
              | local_decl ";"
              | "if" "(" expression ")" statement [ "else" statement ]
              | "while" "(" expression ")" statement
              | "do" statement "while" "(" expression ")" ";"
              | "for" "(" [ for_init ] ";" [ expression ] ";" [ expr_list ] ")" statement
              | "for" "(" ( type | "var" ) IDENT ":" expression ")" statement
              | switch_block
              | "break" [ IDENT ] ";"
              | "continue" [ IDENT ] ";"
              | "return" [ expression ] ";"
              | "yield" expression ";"
              | "throw" expression ";"
              | try_stmt
              | IDENT ":" statement                       (* label *)
              | expression ";"
              | HASH_IF { statement } [ HASH_ELSE { statement } ] HASH_END
              | ";" ;

local_decl    = [ "final" ] [ "weak" ] ( type | "var" ) var_declarator { "," var_declarator } ;
for_init      = local_decl | expr_list ;
expr_list     = expression { "," expression } ;

switch_block  = "switch" "(" expression ")" "{" { switch_rule } "}" ;
switch_rule   = ( "case" case_label { "," case_label } | "default" ) "->"
                ( expression ";" | block | "throw" expression ";" ) ;
case_label    = [ "-" ] INT | CHAR | STRING | IDENT ;

try_stmt      = "try" block { catch_clause } [ "finally" block ] ;
catch_clause  = "catch" "(" class_type { "|" class_type } IDENT ")" block ;
```

Checked after parsing:
- An expression statement must be an assignment, `++`/`--`, a call, `new`, or `super(...)`.
- `var` needs an initializer and can't be `null` (DESIGN §12.6). `weak` is the only reference kind allowed on a local.
- `try` needs at least one `catch` or a `finally`.
- `yield` gives the value of a `switch` expression from a block arm.
- A `case` label is a constant: a literal (strings without interpolation) or an enum constant written by its bare name. A `switch` used as an expression must cover every case (DESIGN §12.6).
- `super(...)` is allowed only in a constructor, after the class's own fields are assigned and before any other use of `this` (DESIGN §8.2).

---

## 6. Expressions

Precedence, lowest first:

| Level | Operators | Associativity |
|---|---|---|
| 1 | lambda `->` | right |
| 2 | `=` and every compound assignment | right |
| 3 | `? :` | right |
| 4 | `\|\|` | left |
| 5 | `&&` | left |
| 6 | `\|` | left |
| 7 | `^` | left |
| 8 | `&` | left |
| 9 | `==` `!=` | left |
| 10 | `<` `>` `<=` `>=` `instanceof` | left |
| 11 | `<<` `>>` `>>>` | left |
| 12 | `+` `-` `+%` `-%` `+\|` `-\|` | left |
| 13 | `*` `/` `%` `*%` `*\|` | left |
| 14 | prefix `+` `-` `!` `~` `++` `--`, numeric cast `(int) x` | right |
| 15 | postfix `++` `--`, member `.`, call `( )`, index `[ ]` | left |

```
expression     = lambda | assignment ;

lambda         = lambda_params "->" ( expression | block ) ;
lambda_params  = IDENT
               | "(" [ lambda_param { "," lambda_param } ] ")" ;
lambda_param   = [ type ] IDENT ;

assignment     = conditional [ assign_op expression ] ;
assign_op      = "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^="
               | "<<=" | ">>=" | ">>>=" | "+%=" | "-%=" | "*%=" | "+|=" | "-|=" | "*|=" ;

conditional    = or_expr [ "?" expression ":" conditional ] ;
or_expr        = and_expr { "||" and_expr } ;
and_expr       = bitor_expr { "&&" bitor_expr } ;
bitor_expr     = xor_expr { "|" xor_expr } ;
xor_expr       = bitand_expr { "^" bitand_expr } ;
bitand_expr    = equality { "&" equality } ;
equality       = relational { ( "==" | "!=" ) relational } ;
relational     = shift { ( "<" | ">" | "<=" | ">=" ) shift
                       | "instanceof" class_type [ IDENT ] } ;
shift          = additive { ( "<<" | ">>" | ">>>" ) additive } ;
additive       = multiplicative { ( "+" | "-" | "+%" | "-%" | "+|" | "-|" ) multiplicative } ;
multiplicative = unary { ( "*" | "/" | "%" | "*%" | "*|" ) unary } ;

unary          = ( "+" | "-" | "!" | "~" | "++" | "--" ) unary
               | "(" primitive_type ")" unary
               | postfix ;
postfix        = primary { selector } { "++" | "--" } ;
selector       = "." IDENT
               | arguments
               | "[" index { "," index } "]" ;
index          = [ "%" | "|" ] expression ;
arguments      = "(" [ expression { "," expression } ] ")" ;

primary        = literal
               | IDENT
               | "this"
               | "super" ( "." IDENT | arguments )
               | ( primitive_type | "string" ) "." IDENT       (* int.parse(...) *)
               | "(" expression ")"
               | "new" class_type arguments
               | "take" "(" expression ")"
               | "replace" "(" expression "," expression ")"
               | switch_block ;
literal        = INT | FLOAT | CHAR | STRING | "true" | "false" | "null" ;
```

Checked after parsing:
- The left side of an assignment, and the argument of `take` and the first argument of `replace`, must be a variable, a field or an array slot.
- Lambda parameters are either all typed or all untyped.
- Casts exist only between number types; reference downcasts use `instanceof` with a binding name (DESIGN §12.1).
- `[%i]` wraps and `[|i]` clamps; each index of a multi-dimensional access has its own mode (DESIGN §12.3).
- Array creation is ordinary `new` syntax: `new array<int>(100)`, `new array<int, 2>(rows, cols)`, `new array<Dog>(10, i -> new Dog())`, `new nullarray<Dog?>(10)` (DESIGN §8.2).

**Disambiguation rules**
1. **Lambda or parentheses**: at `(`, the parser looks past the matching `)`. If `->` follows, it is a lambda.
2. **Cast or parentheses**: `(` followed by a primitive type and `)` is a cast. Nothing else is, since reference types have no casts.
3. **Closing type arguments**: inside `type_args`, `>>` and `>>>` are split into `>` tokens, so `list<list<int>>` parses.
4. **Declaration or expression**: a statement that starts with something that parses as a type followed by an `IDENT` is a declaration. So `list<Dog> d = ...;` and `Dog? d = null;` are declarations, while `a < b;` and `x ? y : z;` are expressions.
5. **Members**: after annotations and modifiers, `new (` starts a constructor and `delete (` a destructor. Otherwise the parser reads a return type (or `void`) and a name; `(` next means a method, anything else a field.
6. **Index modes**: a `%` or `|` directly after `[` or after a `,` inside index brackets is a mode marker, not an operator. Neither can start an expression, so this never conflicts.
7. **`switch`**: at the start of a statement it is a statement; anywhere else it is an expression.
8. **Qualified names** in expressions, such as `com.example.Thing.count`, parse as a chain of `.` selectors and are resolved by the type checker: package, class, then field.

---

## 7. A complete example

```
#!/usr/bin/env zink
#max_heap_mb 64

struct Point {
    int x;
    int y;
}

class Tree {
    owned Node? root;

    void insert(int value) {
        if (root == null) {
            root = new Node(value);
        } else {
            root.insert(value);
        }
    }
}

class Node {
    int value;
    owned Node? left;
    owned Node? right;

    new(int value) { this.value = value; }

    void insert(int v) {
        if (v < value) {
            if (left == null) left = new Node(v); else left.insert(v);
        } else {
            if (right == null) right = new Node(v); else right.insert(v);
        }
    }
}

void main(array<string> args) {
    var tree = new Tree();
    for (string arg : args) {
        try {
            tree.insert(int.parse(arg));
        } catch (ParseException e) {
            print("not a number: ${arg}");
        }
    }

    array<Point> pts = new array<Point>(4);
    pts[%-1].x = 7;                                  // the last point
    func(int, int -> int) add = (a, b) -> a +% b;

    string size = switch (args.length) {
        case 0 -> "none";
        case 1, 2 -> "a few";
        default -> "many";
    };
    print("${size}: ${add(2, 3)}");
}
```

---

## 8. Not in v1 (Open)

These need a decision before they are added:
- Array, list and map literals.
- Null-safe member access (`?.`) and a null-coalescing operator.
- Method references such as `this::close`.
- Varargs parameters.
- Static initializer blocks (`static { ... }`).
- Constructor delegation with `this(...)`.
- Default method bodies in interfaces.
- Explicit type arguments on generic method calls.
- Multi-line strings.
- `assert`.
- `#elif`.
- Patterns in `switch`, and enums with fields and methods.
- Type aliases, such as `type Action = func(void -> void);`.
- Unicode letters in identifiers.
