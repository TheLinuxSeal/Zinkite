# Zinkite Grammar (v2)

> Companion to `DESIGN.md`. It covers everything marked Decided there. Features still Open are listed in §8 and are not in this grammar yet.
> **Updated:** 2026-11-19. Version 2 is for the Goose-style design: slot-grammar arrays, `T&` references, the status code (`#` and `@`), interfaces without inheritance, `=>` for lambdas and cases, and no exceptions. Version 1, for the reference-counting design, is kept in `GRAMMAR-refcount.md`.

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
IDENT   = LETTER { LETTER | DIGIT } ;          (* not a keyword, and not a lone "_" *)
LETTER  = "A".."Z" | "a".."z" | "_" ;
DIGIT   = "0".."9" ;
HEXDIGIT = DIGIT | "a".."f" | "A".."F" ;
BINDIGIT = "0" | "1" ;
```

Reserved keywords:

| Group | Keywords |
|---|---|
| Types | `bool` `i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64` `f32` `f64` `string` `void` `func` |
| Declarations | `package` `import` `as` `class` `interface` `enum` `implements` `static` `public` `private` `mut` `extern` (`new` is listed under Expressions) |
| Statements | `if` `else` `while` `do` `for` `break` `continue` `return` `switch` `case` `default` `yield` `var` `defer` |
| Expressions | `new` `this` `null` `true` `false` `instanceof` |
| Reserved, not used | `extends` `protected` `abstract` `final` `struct` `super` `try` `catch` `finally` `throw` `long` `short` `byte` `double` `char` `delete` |

The unused keywords are reserved so the compiler can explain why they aren't available: there is no inheritance (DESIGN §6.1), nothing is thrown (DESIGN §7), there are no destructors (`delete()` suggests `defer`, DESIGN §6.4), the number types are spelled `i32`, `u8`, `f64` and so on, and code points are `u32` (DESIGN §9.1).

`root` is a keyword only at the start of a type declaration (`root Parser { ... }`, DESIGN §6.6), so variables and fields may still be named `root`.

`hashmap`, `hashset` and `pool` are built-in generic type names (DESIGN §4.14, `STDLIB.md` §7), and `sint`, `uint`, `int`, `float`, `number`, `ordered` and `hashable` are built-in generic bounds (DESIGN §10.1). They aren't keywords, but no declaration may reuse them. `int` and `float` used as a type are errors that suggest `i64` and `f64`.

### 1.4 Literals
```
INT       = ( DEC | HEX | BIN ) [ INT_SUFFIX ] ;
INT_SUFFIX = "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" ;
DEC       = DIGIT { DIGIT | "_" } ;
HEX       = "0x" HEXDIGIT { HEXDIGIT | "_" } ;
BIN       = "0b" BINDIGIT { BINDIGIT | "_" } ;
FLOAT     = DEC "." DEC [ EXPONENT ] [ FLOAT_SUFFIX ]
          | DEC EXPONENT [ FLOAT_SUFFIX ]
          | DEC FLOAT_SUFFIX ;
FLOAT_SUFFIX = "f32" | "f64" ;
EXPONENT  = ( "e" | "E" ) [ "+" | "-" ] DEC ;
CHAR      = "'" ( CHAR_CHAR | ESCAPE ) "'" ;
STRING    = '"' { STRING_CHAR | ESCAPE | INTERP } '"' ;
INTERP    = "${" expression "}" ;
CHAR_CHAR = any character except "'", "\" and a line break ;
STRING_CHAR = any character except '"', "\" and a line break, and "$" only when no "{" follows ;
ESCAPE    = "\" ( "n" | "t" | "r" | "0" | "\" | "'" | '"' | "$" | "u{" HEXDIGIT { HEXDIGIT } "}" ) ;
```

- **Literals take their type from where they're used** (DESIGN §9.1): in `u8 b = 200;` the `200` is a `u8`. With nothing to go by, an `INT` is an `i64` and a `FLOAT` an `f64`. The type also flows into arithmetic: in `i64 big = 100000 * 100000;` both literals are `i64`. A suffix states the type: `255u8`, `-1i64`, `1.5f32`, `2f32`. A literal must fit its type.
- `_` separates digits (`1_000_000`). It can't start or end a number.
- A signed type's minimum, such as `128i8` or `2147483648`, is allowed only directly after unary `-`, so `-128i8` and `-2147483648` can be written, as in Java.
- Suffixes never clash with hex digits: `i` and `u` aren't hex digits, and float suffixes apply only to decimal literals, so `0x1f32` is the integer `0x1F32`.
- A `FLOAT` needs a digit after its `.`, so `1.hex()` is a method call on the integer `1`.
- **A `CHAR` literal is an integer**: the value of its code point, so `'a'` is 97 (DESIGN §9.1). Like other literals it takes its type from where it's used, and is a `u32` with nothing to go by. Used as a `u8`, it must be ASCII: `u8 b = 'é';` is an error.
- `\u{...}` is a Unicode code point, at most `10FFFF`, and never a surrogate (`D800`–`DFFF`).
- A `STRING` literal is a `string`: a view of static data (DESIGN §4.11).
- In a string, `${` starts an interpolation, which ends at the matching `}`. The embedded expression is lexed recursively and may contain strings and braces. A `$` not followed by `{` is an ordinary character; `\$` writes a literal `$`.
- Strings can't span lines (multi-line strings are Open, §8).

### 1.5 Operators and punctuation
```
(  )  {  }  [  ]  ;  ,  .  :  ?  @  #  $  =>  &
=  .=  +=  -=  *=  /=  %=  &=  |=  ^=  <<=  >>=  +%=  -%=  *%=  +|=  -|=  *|=
||  &&  |  ^  ==  !=  <  >  <=  >=  <<  >>
+  -  *  /  %  +%  -%  *%  +|  -|  *|
+>  +<  +^  ->  -<  -^  ^-
!  ~  ++  --
```

- **The lexer takes the longest match.** Some consequences:
  - `a^-b` lexes as `a ^- b` (remove after the cursor), so an XOR with a negated value needs a space: `a ^ -b`. The compiler's error suggests it.
  - `i-->0` lexes as `i -- > 0`, as in Java.
  - In a type, the characters between `[` and `]` are read directly as slots (§3), so `>>` and `<<` count as two slot characters there.
- Inside type arguments, the parser splits `>>` into separate `>` tokens (§6, rule 3).
- There is no `>>>`: `>>` is an arithmetic shift on signed types and a logical one on unsigned types (DESIGN §9.2).
- `&` is both bitwise AND and the reference marker in types (`Item&`). `#` and `@` are covered in §1.6 and §6.

### 1.6 Directive lines and `#`
A `#` that is the first non-blank character on a line **and is followed directly by a letter** starts a directive, unless the token before it is `return`. The lexer turns each directive line into one token:

```
SETTING   = "#" IDENT VALUE ;            (* #max_memory_mb 256 *)
HASH_IF   = "#if" condition ;
HASH_ELSE = "#else" ;
HASH_END  = "#endif" ;

condition = cond_and { "||" cond_and } ;
cond_and  = cond_not { "&&" cond_not } ;
cond_not  = "!" cond_not | "(" condition ")" | IDENT [ "(" IDENT ")" ] ;
```

- `VALUE` is the rest of the line: a word, or an integer.
- Condition names are build flags (`debug`, `--define color` gives `color`) and the target (`os(linux)`, `arch(aarch64)`); see DESIGN §8.3.
- `#if` blocks are part of the syntax tree (§2, §4, §5). Both branches must parse; only the active one is type-checked.
- **Every other `#` is the `#` token**, which writes a call's status after `return` (§5). So `return` followed on the next line by `#1 x;` or `#status.NOT_FOUND x;` still works.

---

## 2. Files

```
file          = [ SHEBANG ] { setting_item } [ package_decl ] { import_decl | extern_decl } { top_item } ;
                                    (* SHEBANG: "#!" and the rest of the first line *)

setting_item  = SETTING
              | HASH_IF { setting_item } [ HASH_ELSE { setting_item } ] HASH_END ;

package_decl  = "package" qualified_name ";" ;
import_decl   = "import" qualified_name [ "." "*" | "as" IDENT ] ";" ;
extern_decl   = "extern" "import" c_name ";"   (* a C header: extern import SDL3/SDL.h; *)
              | "extern" "link" c_name ";" ;   (* a C library: extern link SDL3; *)
c_name        = token { token } ;              (* spelled out to the ";", no quotes: stdio.h, sys/stat.h *)
qualified_name = IDENT { "." IDENT } ;

top_item      = type_decl
              | field_decl                     (* script files only *)
              | method_decl                    (* script files only *)
              | HASH_IF { top_item } [ HASH_ELSE { top_item } ] HASH_END ;
```

- Settings are allowed only in the file that contains the entry point (DESIGN §8.2).
- `link` after `extern` is a plain word, not a keyword. C headers and libraries are DESIGN §12.6.
- A file with top-level fields or methods is a **script file** (DESIGN §8.6). It can't have a `package` line, and its top-level fields and methods are static members of a hidden class.
- Otherwise the file holds one public top-level class, root, interface or enum named after the file, plus any non-public ones (DESIGN §8.4).

---

## 3. Types

```
type           = base_type { type_suffix } ;
base_type      = primitive_type
               | "string"                       (* the same type as u8[:] *)
               | class_type
               | func_type ;
primitive_type = "bool"
               | "i8" | "i16" | "i32" | "i64"
               | "u8" | "u16" | "u32" | "u64"
               | "f32" | "f64" ;
class_type     = qualified_name [ type_args ] ;
type_args      = "<" type { "," type } ">" ;

type_suffix    = array_suffix                   (* an array of the type so far *)
               | "&" [ link ]                   (* a reference, or a link *)
               | "?" ;                          (* nullable: only after & or a func type *)

link           = "<" ( "i8" | "i16" | "i32" ) ">"                          (* self-relative *)
               | "<" ( "u8" | "u16" | "u32" ) "in" qualified_name ">" ;   (* pool link *)

array_suffix   = "[" slots "]"
               | "[" ":" "]" ;                  (* a slice *)
slots          = slot [ slot slot ] [ cap | "$" ] ;   (* one slot stands for all three; "$": chunked index *)
slot           = ">" | "<" | "~" | "." ;        (* insert, remove, both, nothing *)
cap            = INT | qualified_name | "*" ;

func_type      = "func" "(" func_params "=>" func_result ")" ;
func_params    = "void" | type { "," type } ;
func_result    = "void" | type | tuple_type ;

return_type    = "void" | type | tuple_type ;
tuple_type     = "(" type "," type { "," type } ")" ;    (* results only: DESIGN §9.5 *)
```

Suffixes apply left to right:

| Written | Meaning |
|---|---|
| `Item&` | a reference to an `Item` |
| `Item&?` | a nullable reference |
| `Item[.~.$]` | a cursor array with a chunked index (DESIGN §4.1) |
| `Node&<i32>?` | a nullable self-relative link: a 4-byte offset to another element of the same array (REFERENCES §7) |
| `Node&<u32 in nodes>` | a pool link: an offset from the start of the static array `nodes` (REFERENCES §7.3) |
| `u8[..>]` | an append-only array of bytes on a data stack |
| `Shape&[..>]` | an array of interface references |
| `Item[..>]&` | a reference to an array, for a parameter the callee appends to |
| `string[.]` | a frozen array of string views (what `main` receives) |
| `u8[:]` | a slice; the same type as `string` |
| `func(i32, i32 => i32)?` | a function that may be null |

Checked after parsing:
- **Slots**, as in DESIGN §4.1–§4.2:
  - there are three slot characters, for start, cursor and end (`[>~>]`), or one that stands for all three (`[~]` is `[~~~]`, `[.]` is `[...]`). Two is an error;
  - `*` is allowed only on frozen arrays, whose slots are all `.` (`T[.*]`);
  - a trailing `$` needs a cursor slot other than `.`, and takes the place of a cap: `[.~.$]` and `[~$]` are fine, `[..>$]` is an error, and `[.~.16$]` doesn't parse;
  - a numeric or named cap must be a compile-time constant.
- **Nullable types**: `?` is allowed only directly after `&`, after a link (`Node&<i32>?`), or after a `func(...)` type. `i32?`, `Item?` and `Item?&` are errors.
- **Links** (`T&<i32>`, `T&<u32 in arr>`) appear only as field types: a local or parameter that refers to an element uses `T&` (REFERENCES §7). `in` is a keyword only inside a link's angle brackets, and `arr` must be a static grow-only array.
- **No reference to a reference**: `Item&&` lexes as `&&` and is an error, and so is `Item& &`.
- **`T[:]&`** is a writable view (DESIGN §4.10), not a reference to a slice.
- **Interfaces** appear only directly before `&`, or as a generic bound: `Shape s;` and `Shape[..>]` are errors (DESIGN §6.3).
- **Built-in bounds** appear only as generic bounds, and `ordered` also after a class's `implements`: `ordered x;` and `number&` are errors (DESIGN §10.1).
- **`func(...)` always writes both sides**: `func(void => void)`, `func(string => void)`, `func(void => bool)`, `func(i32, i32 => i32)`. `func()` and `func(string)` are errors (DESIGN §9.3).
- `T[k]` (a bare number in the brackets) is an error that suggests `T[.k]`.

---

## 4. Declarations

```
modifier       = "public" | "private" | "static" | "mut"
               | "extern" [ "(" STRING ")" ] ;    (* a C function, with its C name: extern("glClearColor") *)

type_decl      = { modifier } ( class_decl | root_decl | interface_decl | enum_decl ) ;

class_decl     = "class" IDENT [ type_params ]
                 [ "implements" class_type { "," class_type } ]
                 class_body ;
root_decl      = "root" IDENT [ type_params ]                  (* DESIGN §6.6 *)
                 [ "implements" class_type { "," class_type } ]
                 class_body ;
interface_decl = "interface" IDENT [ type_params ]
                 "{" { interface_member } "}" ;
enum_decl      = "enum" IDENT "{" [ IDENT { "," IDENT } [ "," ] ] "}" ;

type_params    = "<" type_param { "," type_param } ">" ;
type_param     = IDENT [ "implements" class_type { "&" class_type } ] ;   (* <T implements ordered & hashable> *)

class_body     = "{" { member } "}" ;
member         = field_decl
               | method_decl
               | constructor_decl
               | HASH_IF { member } [ HASH_ELSE { member } ] HASH_END
               | ";" ;
interface_member = [ "mut" ] [ type_params ] method_header ";"
               | HASH_IF { interface_member } [ HASH_ELSE { interface_member } ] HASH_END ;

field_decl     = { modifier } type var_declarator { "," var_declarator } ";" ;
var_declarator = IDENT [ "=" expression ] ;

method_decl    = { modifier } [ type_params ] method_header block ;
method_header  = return_type IDENT "(" [ param { "," param } ] ")" ;
param          = type name ;
name           = IDENT | "_" ;                  (* "_" discards: DESIGN §7.3 *)

constructor_decl = { modifier } "new" "(" [ param { "," param } ] ")" block ;
```

Checked after parsing:
- **`mut`** goes only on instance methods and interface methods (DESIGN §6.1). `static` goes only on fields and methods. `public` and `private` go on declarations, not on locals.
- **Constructors** are written `new(...)` and have no return type (DESIGN §6.2). Each must assign every field on every path that doesn't fail (DESIGN §6.2), which the type checker verifies. `this.new(...)` appears only in a constructor, as a statement or after `@err =`.
- **Roots** (DESIGN §6.6): a `class` may have only one growable field, and no cursor-array field; either is an error that suggests `root`. A root can't be copied, passed as a plain parameter, returned by value, stored by value in a class, root or array, or created as an unnamed temporary.
- **Overloads** (DESIGN §6.5): methods, static functions and constructors with the same name in one class must differ in a parameter's type, not only in `&`, `?`, array slots, the return type or the parameter names. A slice and an array differ in kind, so `f(string)` beside `f(u8[..>]&)` is allowed.
- **Interfaces** declare methods only; they have no fields and no bodies (default methods are Open, §8).
- **Script files**: top-level declarations take no access modifiers.
- **`extends`, `protected` and `abstract`** are errors that explain there is no inheritance.

---

## 5. Statements

```
block         = "{" { statement } "}" ;

statement     = block
              | local_decl ";"
              | "if" "(" expression ")" statement [ "else" statement ]
              | "while" "(" expression ")" statement
              | "while" "(" loop_target ":" expression ")" statement               (* repeat loop *)
              | "while" "(" loop_target ":" expression ")" status_capture block
              | "do" statement "while" "(" expression ")" ";"
              | "for" "(" [ for_init ] ";" [ expression ] ";" [ expr_list ] ")" statement
              | "for" "(" local_type name ":" expression ")" statement
              | switch_block
              | "break" [ IDENT ] ";"
              | "continue" [ IDENT ] ";"
              | "return" [ status_write ] [ expression | tuple_expr ] ";"
              | destructure ";"
              | "defer" ( block | expression ";" )
              | "yield" expression ";"
              | IDENT ":" statement                       (* label *)
              | expression ";"
              | HASH_IF { statement } [ HASH_ELSE { statement } ] HASH_END
              | ";" ;

local_decl    = local_type local_declarator { "," local_declarator } ;
local_type    = type | "var" [ "&" ] ;
local_declarator = IDENT [ status_capture ] [ "=" expression ] ;
status_capture   = "@" name ;
status_write  = "#" ( INT | qualified_name | "(" expression ")" ) ;

destructure   = "(" target "," target { "," target } ")" [ status_capture ] "=" ( expression | tuple_expr ) ;
target        = local_type name                 (* declares a new local *)
              | name ;                          (* an existing variable, or _ *)
tuple_expr    = "(" expression "," expression { "," expression } ")" ;

for_init      = local_decl | expr_list ;
loop_target   = local_type name
              | "(" loop_item "," loop_item { "," loop_item } ")" ;    (* a tuple result *)
loop_item     = local_type name | "_" ;
expr_list     = expression { "," expression } ;

switch_block  = "switch" "(" expression ")" "{" { switch_rule } "}" ;
switch_rule   = ( "case" case_label { "," case_label } | "default" ) "=>"
                ( expression ";" | block ) ;
case_label    = [ "-" ] INT | CHAR | STRING | IDENT ;
```

Checked after parsing:
- **Expression statements** must be an assignment, `++`/`--`, a call, or an array operator (`a +> x;`, `a ->;`).
- **`var`** needs an initializer and can't be `null`. `var` copies and `var&` binds a reference (DESIGN §5.1).
- **Status writes** (DESIGN §7.2):
  - an `INT` must be from 0 to 255, and a `qualified_name` must name a `u8` constant, such as `status.FULL`, or a `u8` or `bool` variable, such as `why`;
  - the expression in `#( ... )` must have type `u8` or `bool` (a `bool` counts as 0 or 1), so `#(count)` with an `i64` `count` is an error;
  - `return #1;` without a value is allowed only in a `void` method or a constructor;
  - `return @(g());` is an error that suggests `return #(@(g()));`.
- **Multiple results** (DESIGN §9.5):
  - a tuple type appears only as a return type or a function type's result;
  - a destructuring either declares every target or assigns to existing ones; `_` works in both, and `(string a, b) = f();` is an error;
  - a `tuple_expr` appears only after `return`, on the right of a destructuring, and as a lambda's expression body;
  - a call returning a tuple can't be used inside a larger expression.
- **Status captures in declarations**: a `local_declarator` with `@name` needs an initializer, and the declaration must have exactly one declarator. It declares `name` as a new `u8` along with the variable: `Item x @err = a.pop();`. Assignments that capture statuses are in §6.
- **Conditions** (in `if`, `while`, `do`, `for`, `?:`, and the operands of `!`, `&&` and `||`) take a `bool` or an unsigned integer, which counts as false when 0 (DESIGN §9.4). An assignment can't be a condition by itself: `if (x = 0)` is an error.
- **Repeat loops** (DESIGN §9.4): `while (T x : call)` makes the call before every pass and ends when its status is nonzero.
  - The expression after `:` must be one `@` accepts: a call, an array operator, or a parenthesized one (§6). It must be able to fail, and a call to a method that never sets a status is an error that suggests `while (true)`. A `void` call can't be the source: write `while (!@f())`.
  - The header only declares; it can't assign existing variables.
  - A status capture after the header declares a `u8` in the enclosing block and needs a block body, so in `while (T x : f()) @err = g();` the `@err = g();` is the body.
- **For-each over an iterator variable** is an error that suggests the repeat loop (DESIGN §9.4). An iterator expression, such as `text.lines()`, is fine.
- **`defer`** runs its statement when the enclosing block ends, in reverse order of the `defer`s (DESIGN §6.4). The deferred expression must be one that could be an expression statement, and the deferred code can't contain a `return`, a `break` or `continue` that leaves it, a status write, or another `defer`.
- **`yield`** gives the value of a `switch` expression from a block arm.
- **`case` labels** are constants: a literal (strings without interpolation) or an enum constant written by its bare name. A `switch` used as an expression must cover every case (DESIGN §9.4).

---

## 6. Expressions

Precedence, lowest first:

| Level | Operators | Associativity |
|---|---|---|
| 1 | lambda `=>` | right |
| 2 | `=`, `.=`, every compound assignment, and status captures (`x @err = ...`) | right |
| 3 | `? :` | right |
| 4 | `\|\|` | left |
| 5 | `&&` | left |
| 6 | `\|` | left |
| 7 | `^` | left |
| 8 | `&` | left |
| 9 | `==` `!=` | left |
| 10 | `<` `>` `<=` `>=` `instanceof` | left |
| 11 | array inserts `+>` `+<` `+^` | left |
| 12 | `<<` `>>` | left |
| 13 | `+` `-` `+%` `-%` `+\|` `-\|` | left |
| 14 | `*` `/` `%` `*%` `*\|` | left |
| 15 | prefix `+` `-` `!` `~` `++` `--`, status read `@`, numeric casts `(i32) x`, `(u8%) x`, `(u8\|) x` | right |
| 16 | postfix `++` `--`, array removes `->` `-<` `-^` `^-`, member `.`, call `( )`, index `[ ]` | left |

```
expression     = lambda | assignment ;

lambda         = lambda_params "=>" ( expression | tuple_expr | block ) ;
lambda_params  = "(" [ lambda_param { "," lambda_param } ] ")" ;
lambda_param   = type name ;                    (* the type is always written: DESIGN §9.3 *)

assignment     = conditional [ [ status_capture ] assign_op expression ]
               | status_capture "=" expression ;                  (* @failed = f(); *)
assign_op      = "=" | ".=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^="
               | "<<=" | ">>=" | "+%=" | "-%=" | "*%=" | "+|=" | "-|=" | "*|=" ;

conditional    = or_expr [ "?" expression ":" conditional ] ;
or_expr        = and_expr { "||" and_expr } ;
and_expr       = bitor_expr { "&&" bitor_expr } ;
bitor_expr     = xor_expr { "|" xor_expr } ;
xor_expr       = bitand_expr { "^" bitand_expr } ;
bitand_expr    = equality { "&" equality } ;
equality       = relational { ( "==" | "!=" ) relational } ;
relational     = insertion { ( "<" | ">" | "<=" | ">=" ) insertion
                           | "instanceof" class_type [ IDENT ] } ;
insertion      = shift { ( "+>" | "+<" | "+^" ) shift } ;
shift          = additive { ( "<<" | ">>" ) additive } ;
additive       = multiplicative { ( "+" | "-" | "+%" | "-%" | "+|" | "-|" ) multiplicative } ;
multiplicative = unary { ( "*" | "/" | "%" | "*%" | "*|" ) unary } ;

unary          = ( "+" | "-" | "!" | "~" | "++" | "--" ) unary
               | "@" postfix                                    (* the call's status *)
               | "(" cast_type [ "%" | "|" ] ")" unary         (* checked, wrapping, saturating *)
               | postfix ;
cast_type      = primitive_type | IDENT ;                       (* IDENT: a type parameter in scope *)
postfix        = primary { selector } { "++" | "--" | "->" | "-<" | "-^" | "^-" } ;
selector       = "." IDENT
               | arguments
               | "[" index "]"
               | "[" slice "]" ;
index          = [ "%" | "|" | "^" ] expression ;
slice          = [ slice_bound ] ":" [ slice_bound ] ;          (* a[i:j], a[i:], a[:j], a[:] *)
slice_bound    = [ "^" ] shift ;
arguments      = "(" [ argument { "," argument } ] ")" ;
argument       = [ "&" ] expression ;                   (* "&" is accepted with a warning *)

primary        = literal
               | IDENT
               | "_"                                          (* only as an assignment target *)
               | "this"
               | "this" "." "new" arguments                  (* constructor delegation: DESIGN §6.2 *)
               | primitive_type "." IDENT                     (* i32.parse(...) *)
               | "(" expression ")"
               | "new" [ class_type ] arguments                (* new(...) infers the class *)
               | "[" [ expression { "," expression } [ "," ] ] "]"   (* array literal *)
               | switch_block ;
literal        = INT | FLOAT | CHAR | STRING | "true" | "false" | "null" ;
```

Checked after parsing:
- **Assignment targets**: the left side of an assignment must be a variable, a field, an array element or a reference.
  - `.=` needs a reference variable or field, a link field, or a writable view on the left, and rebinds it; `=` through a reference writes its target (DESIGN §5.1).
  - A writable slice, such as `buf[0:4]` of a changeable array or a `u8[:]&` variable, can be assigned: `=` copies elements into it (DESIGN §4.10).
- **`_` discards** (DESIGN §7.3). A lone `_` lexes as its own token, not an `IDENT`. It may appear only:
  - on the left of a plain `=` (`_ = f();`), which evaluates the right side and throws away its value, whatever its type;
  - as a status destination (`x @_ = f();`, `@_ = f();`);
  - as the name of a parameter, a lambda parameter, a for-each or repeat-loop variable, or a destructuring target, which is then never readable. Several parameters may be named `_`.

  Reading `_` anywhere, or using it with a compound assignment or `.=`, is an error.
- **Status captures in assignments** (DESIGN §7.3):
  - they take only `=`, not compound operators;
  - in `x @err = f();` and `@err = f();`, `err` must already be a `u8` variable;
  - to declare one, use a declaration (`Item x @err = f();`, §5) or `u8 err = @f();`.
- **The operand of `@`** must be a call, an array operator, or a parenthesized one: `@a.pop()`, `@(a +> x)`. `@a + b` parses as `(@a) + b` and is an error that asks for parentheses.
- **Array operators**:
  - inserts take an array on the left and return it, so they chain: `a +> 1 +> 2`;
  - removes are postfix and return the removed value;
  - each needs the capability of its method (DESIGN §4.7–§4.8).
- **Index modes**: `[%i]` wraps, `[|i]` clamps and `[^k]` counts from the end (DESIGN §4.10). `^` is also allowed before either slice bound: `a[2:^1]`, `a[:^1]`, `a[:]`.
- **For-each targets**: the expression after `:` must be an array, a slice, a `hashmap`, a `hashset`, a `pool`, an iterator (a value with a `mut T next()` method), or a value whose class has an `iterator()` method returning one (DESIGN §9.4).
- **Arrays convert to slices** wherever a slice is expected, so `[:]` is rarely written (DESIGN §4.10).
- **Arguments are never marked** (DESIGN §5.1, as in Goose). Whether an argument can change depends only on the parameter's type. An argument written `&x` means the same as `x` and draws a warning, which helps people used to Swift, C# or Rust.
- **Lambdas** write every parameter's type, inside parentheses: `(i32 a, i32 b) => a + b`, `(Dog d) => d.age`. `var` isn't allowed there (DESIGN §9.3).
- **Casts** exist only between number types; interface references use `instanceof` with a binding name (DESIGN §6.3).
  - `(u8) x` stops the program when `x` doesn't fit, `(u8%) x` wraps and `(u8|) x` saturates (DESIGN §9.1). `%` and `|` need an integer target.
  - In a generic body, `(T) x` casts to the type parameter `T`, and `(f64) t` casts a value of type `T` to a concrete type. Either way `T` must be bounded by a number bound (DESIGN §10.1).
- **`new`** names a class, or leaves it out when the destination's type is known: `items.push(new(1, 0.0))` (DESIGN §6.1).
- **Array literals** take their type from the destination: `u8[..>] scratch = [];`. An empty `[]` also creates an empty `hashmap`, `hashset` or `pool` (DESIGN §4.14, `STDLIB.md` §7).
- **`null`** is allowed only where a `T&?` or a nullable function is expected.

**Disambiguation rules**
1. **Lambda, destructuring or parentheses**: at `(`, the parser looks past the matching `)`. If `=>` follows, it is a lambda. At the start of a statement, if `=` or `@` follows, it is a destructuring (`(a, b) = f();`). Otherwise it is a parenthesized expression or, with commas inside, a tuple expression.
2. **Cast or parentheses**: `(` followed by a primitive type, an optional `%` or `|`, and `)` is a cast. So is `(T)` where `T` is a type parameter of an enclosing generic declaration; the parser knows these names, since they are declared before use. Nothing else is, since there are no reference casts.
3. **Closing type arguments**: inside `type_args`, `>>` is split into `>` tokens, so `Pair<string, Pair<i32, i32>>` parses. A `>` inside slot brackets (`hashmap<string, u8[..>16]>`) belongs to the slots, since the brackets are parsed first.
4. **Declaration or expression**: a statement that starts with something that parses as a type, suffixes included, followed by an `IDENT` is a declaration. These are declarations:
   - `Item& r = items[0];`
   - `u8[..>] scratch = [];`
   - `Shape&[..>] shapes = [];`
   - `Item x @failed = a.pop();`

   These are expressions:
   - `items[0].heavier(1.0);`: `items[0]` isn't a valid slot spec;
   - `x @failed = f();`: `@` follows, not an `IDENT`;
5. **Slot brackets or index brackets**: after a type, `[` starts slots or `[:]`; after an expression, `[` starts an index or a slice; at the start of an operand, `[` starts an array literal.
6. **Index modes**: `%`, `|` or `^` directly after `[`, and `^` directly after a slice's `:`, is a mode marker, not an operator. None of them can start an expression, so this never conflicts.
7. **`@`**: `@ IDENT =` or `@ _ =` (with `=`, not `==`) names a status destination directly after an assignment target, and at the start of any expression statement: one standing alone, after `defer`, in a `switch` arm, or in a `for` update. Directly after a repeat loop's header, `@ IDENT` followed by `{` is the loop's status capture (§5). Anywhere else, `@` reads a status.
8. **`#`**: see §1.6. A `#` that isn't a directive may appear only right after `return`.
9. **Members**: after modifiers, `new (` starts a constructor. Otherwise the parser reads optional type parameters (`<T implements ordered>`), a return type (`void`, a type, or a tuple type in parentheses) and a name; `(` next means a method, anything else a field.
10. **`switch`**: at the start of a statement it is a statement; anywhere else it is an expression.
11. **Qualified names** in expressions, such as `com.example.Thing.count`, parse as a chain of `.` selectors and are resolved by the type checker: a local variable or parameter first, then a package, a class, and a field (DESIGN §8.5).
12. **`while` headers**: at `while (`, the parser looks for a declaration, or a parenthesized list of them for a tuple result (`while ((string k, string v) : r.next())`), followed by `:`, as rule 4 does at the start of a statement, and takes that as a repeat loop. `?` counts as part of a type only directly after `&`, a link or a `func(...)` type, so `while (a ? b : c)` stays a condition.
13. **`:` in brackets**: after an expression, a `:` inside `[ ]` separates slice bounds. A conditional `?:` takes its own `:` first, so `a[c ? i : j]` is an index, and a slice that starts at a conditional needs parentheses: `a[(c ? i : j):]`. Both bounds are shift-level expressions, so a bound that uses a comparison or a bitwise operator needs them too: `a[0:(n & 7)]`. An array literal starts at an operand (rule 5), so a `:` in it never means a slice.

---

## 7. A complete example

```
#!/usr/bin/env zink
#max_memory_mb 64

interface Shape {
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

class Stats {
    i32 count;
    private f64 total;

    new() { count = 0; total = 0.0; }

    mut void add(f64 x) { count++; total += x; }
    f64 mean() { return count == 0 ? 0.0 : total / count; }
}

f64 parseSize(string s) {
    f64 d @bad = f64.parse(s);
    return #(bad || d <= 0.0) d;
}

void main(string[.] args) {
    Circle[..>] circles = [];
    Rect[..>] rects = [];
    Shape&[..>] shapes = [];                       // references into the two arrays

    for (string arg : args) {
        f64 d @bad = parseSize(arg);
        if (bad) {
            println("not a size: ${arg}");
            continue;
        }
        if (d < 10.0) {
            shapes +> circles.push(new Circle(d));
        } else {
            shapes +> rects.push(new(d, d / 2));   // new(...) infers Rect
        }
    }

    Stats st = new Stats();
    for (Shape& s : shapes) { st.add(s.area()); }

    u32[.~.] text = [];                            // a cursor array of code points
    text +^ 'h' +^ 'i';                            // "hi", cursor at the end
    @_ = text.seek(0);
    text +^ '>';                                   // ">hi", cursor after '>'
    u32 removed @empty = text ^-;                  // removes 'h'
    println("${text.utf8()}");                     // >i

    func(i32, i32 => i32) add = (i32 a, i32 b) => a +% b;
    string size = switch (shapes.len) {
        case 0 => "none";
        case 1, 2 => "a few";
        default => "many";
    };
    println("${size}: mean area ${st.mean().fixed(2)}, ${add(2, 3)}");
    if (shapes.len > 0) {
        println("last area: ${shapes[^1].area()}");
    }
}
```

---

## 8. Not in v2 (Open)

These need a decision before they are added. Until then, the parser rejects them with an error that says the feature is undecided (DESIGN §17):
- Hash map literals with entries, such as `["a": 1]`, and `m[key]` indexing.
- Null-safe member access (`?.`) and a null-coalescing operator, for `T&?`.
- Annotations: `@` now reads statuses (DESIGN §17).
- Compile-time constant type parameters, such as `<const N: i32>` (DESIGN §10).
- Multi-dimensional arrays (`m[i, j]`).
- Default and static methods in interfaces.
- Nested type declarations.
- `final` fields and locals.
- Method references such as `this::close`.
- Varargs parameters.
- Static initializer blocks (`static { ... }`).
- Explicit type arguments on generic method calls.
- Multi-line strings.
- `assert`.
- `#elif`.
- Patterns in `switch`, and enums with fields, methods or payloads.
- Type aliases, such as `type Action = func(void => void);`.
- Unicode letters in identifiers.
- Integers narrower than a byte (`i4`, `u4`) (DESIGN §9.1).
