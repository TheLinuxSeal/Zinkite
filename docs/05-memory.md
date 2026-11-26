# 5. Memory and references

Zinkite looks like Java, but it has no heap, no garbage collector and no reference
counting. Every value lives on the native stack, in static data, or on a data stack, a
region that grows at its top and is given back when a scope ends. A value
that holds others holds them inline, and a reference is a plain pointer. What keeps those
pointers safe is a set of rules the compiler checks, with nothing to annotate. This
chapter covers where values live, copies and references, and each rule with the errors it
gives, then roots that hold several arrays, pools, what the model costs, and how to design
programs that fit it.

| Rule | Guarantees |
|---|---|
| **S**, roots | no reference outlives the variable it points into |
| **R**, removal | no reference into an array is used after elements are removed from it |
| **V**, variable-size values | a variable-size value never changes after it is built |
| **L**, links | a link always lands inside the array it belongs to |
| **G**, growth | each growable value is alone on top of its data stack, so growing never runs into another value |

[`REFERENCES.md`](../REFERENCES.md) states them precisely. There are no aliasing or
exclusivity rules: two references to the same value can both read and write it, and the
same array can be passed twice, unless the function would use a reference into one
argument after removing from the other ([Summaries](#summaries-instead-of-annotations)). The
lifetime checks (rules S and R, which array a link is in, and recursion) run after type
checking, so their errors appear only once a program has no type errors.

## Where values live

A program has three kinds of memory:
- **the native call stack**, which holds numbers, references, fixed-size class values and
  capped arrays (`u8[..~16]`) in locals, as C does;
- **static data**, which holds string literals, statics and interfaces' method tables;
- **data stacks**: large reservations of address space, each with a pointer to its top.
  Arrays that can grow, and variable-size records (below), live on them.

There is no other memory. Each declaration of a growable local gets a data stack of its
own. When its scope ends, the stack's top goes back to where it was, one store however
much was built, and the next pass through the scope reuses the same memory:

```java
void main() {
    for (i32 round = 0; round < 3; round++) {
        u8[..>] scratch = [];                   // grow-only, on a data stack
        for (i32 i = 0; i < 100000; i++) {
            scratch.push((u8) (i % 256));
        }
        println("round ${round}: ${scratch.len} bytes");
    }                                           // freed here, in one store
}
```

```output
round 0: 100000 bytes
round 1: 100000 bytes
round 2: 100000 bytes
```

Growing an array never moves its elements, since nothing else is above it on its stack
([rule G](#rule-g-one-growable-value-on-top-of-each-data-stack)). So a reference into an
array stays valid however much the array grows:

```java
class Item { i32 id; f64 weight; }

void main() {
    Item[..>] items = [];
    Item& first = items.push(new Item(1, 0.5));     // a reference to element 0
    for (i32 i = 2; i <= 1000000; i++) {
        items.push(new Item(i, 0.0));
    }
    first.weight += 99.0;                           // still valid, a million pushes later
    println("${items.len} items; the first weighs ${items[0].weight}");
}
```

```output
1000000 items; the first weighs 99.5
```

- **A push checks no capacity and copies nothing**: it writes the element and moves the
  top.
- **The kernel commits pages when they are first touched**, so an unused reservation
  costs only address space. Today each data stack reserves 8 GB of it.
- **Temporaries** go on a separate scratch stack and are released at the end of their
  statement ([Temporaries](#temporaries)).
- The reasons are in [DESIGN §3.1](../DESIGN.md#31-data-stacks-decided).

## Values are stored inline

A value is stored where it is declared: in a local, a field or an array element. A class
value has no header, and an array of class values holds them one after another. A class
with a `T[.*]` or `T[.]` field, text or an array whose length is set when it is built, is
a **variable-size record**: its parts follow its fixed-size fields inline, so the record is
one run of bytes with no pointers in it.

```java
class Item { u8[.*] sku; i32 qty; i32 cents; }
class Order { i32 id; u8[.*] customer; Item[.*] items; }

void main() {
    Order[..>] book = [];
    book.push(new Order(1001, "alice", [new Item("SKU-441", 2, 1999), new Item("SKU-7", 1, 500)]));
    book +> new Order(1002, "bob", [new Item("SKU-9", 3, 250)]);
    for (Order& o : book) {
        i64 total = 0;
        for (Item& it : o.items) { total += it.qty * it.cents; }
        println("${o.id} ${o.customer}: ${o.items.len} items, ${total} cents");
    }
}
```

```output
1001 alice: 2 items, 4498 cents
1002 bob: 1 items, 750 cents
```

The whole book is one block, and `new Order(...)` writes each order once, at the end of
`book`, its items included.

- **`T[.*]`** is a frozen array whose length is a varint; **`T[.]`** keeps it in 8 bytes
  ([Arrays](04-arrays.md)). A field of either kind, or of a variable-size class, makes its
  class variable-size.
- **An array of variable-size records can't be indexed**, since element `i` has no fixed
  offset. Loop over it, or keep an array of references to its elements and index or sort
  that, as below.
- **Fixed-size alternatives keep indexing**: a capped array (`u8[..~16]`), stored at its
  full size, or a `string` view into text kept elsewhere.
- What can be done with one once it is built is [rule V](#rule-v-variable-size-values).
- The reasons are in [DESIGN §3.2](../DESIGN.md#32-nested-data-is-stored-inline-decided).

```java
class Item { u8[.*] sku; i32 qty; }

void main() {
    Item[..>] items = [];
    items +> new Item("a", 1) +> new Item("bb", 2);
    Item&[..>] index = [];                      // references into items
    for (Item& it : items) { index +> it; }
    index.sort((Item& a, Item& b) => a.qty > b.qty);
    println("${index[0].sku} has ${index[0].qty}");
    Item& first = items[0];
}
```

```error
example.zn:10:19: error: Item[..>] can't be indexed: its elements are variable-size, so it is only added to at its end, looped over and cleared
help: keep an array of references to the elements to index or sort them (DESIGN §3.2, REFERENCES §6 V3)
```

## Copies and references

`=` copies. A reference is declared with `&` after the type, and it is never implicit, as
the memory model depends on knowing what refers to what. References are transparent: a
reference's fields and methods are reached with `.`, as a value's are.

```java
class Item {
    i32 id;
    f64 weight;

    mut void heavier(f64 by) { weight += by; }
}

void main() {
    Item[..>] items = [];
    items +> new Item(1, 0.0) +> new Item(2, 0.0);

    Item copy = items[0];          // copies the value
    copy.heavier(2.5);             // items[0] is unchanged
    println("copy ${copy.weight}, items[0] ${items[0].weight}");

    Item& r = items[0];            // a reference: nothing is copied
    r.heavier(2.5);                // changes items[0]
    r = new Item(7, r.weight);     // writes a new value into items[0]
    println("items[0] is ${items[0].id}, ${items[0].weight}");

    r .= items[1];                 // rebinds r to items[1]
    r.heavier(4.0);
    println("items[1] ${items[1].weight}");

    var c = items[1];              // a copy
    var& v = items[1];             // a reference
    v.id = 9;
    println("c.id ${c.id}, items[1].id ${items[1].id}");
}
```

```output
copy 2.5, items[0] 0.0
items[0] is 7, 2.5
items[1] 4.0
c.id 2, items[1].id 9
```

- **A reference is bound when it is declared**, or by its first assignment when it is
  declared without a value. After that, `=` writes through it, and `.=` rebinds it.
- **`var x = e` copies and `var& x = e` binds a reference**, with the type taken from `e`.
- **A reference needs a place** to refer to: a variable, a field or an element, not a
  function's result.
- **Two references to one element, or one element passed to two `&` parameters**, both
  read and write it.
- **A reference is an 8-byte pointer.** An interface reference (`Shape&`) adds a pointer
  to a method table ([Classes, interfaces and generics](06-classes.md)), and a slice
  (`string`, `T[:]`) adds a length.

```java
class Item { i64 n; }

Item makeItem() {
    return new Item(1);
}

void main() {
    Item[..>] items = [];
    items +> new Item(1) +> new Item(2);
    i64 x = 1;
    i64 y = 2;
    x .= y;                        // a value can't be rebound
    Item& r = makeItem();          // a reference needs a place
    Item& s;
    if (items.len > 1) {
        s = items[1];              // binds s
    }
    s = items[0];                  // binds s, or writes through it?
}
```

```error
example.zn:12:5: error: `.=` rebinds a reference, and this is a value
help: use `=` to assign it (DESIGN §5.1)
example.zn:13:15: error: a reference needs a variable, field or element to refer to, and this is a temporary value
help: store the value in a variable first, or copy it with a plain type (DESIGN §5.1)
example.zn:18:5: error: this reference is assigned on some paths to here and not on others, so `=` could bind it or write through it
help: assign it on every path first, or rebind it with `.=` (DESIGN §5.1)
```

Write `Item it = makeItem();` to keep a function's result. The reasons are in
[DESIGN §5.1](../DESIGN.md#51-copies-and-references-decided).

## Nullable references

`T&?` is a reference that may be `null`. It must be narrowed, by testing it against
`null`, before what it refers to is used. Rebinding with `.=` is how code walks linked
data without recursion:

```java
class Node { i64 value; Node&? next; }

i64 sum(Node&? head) {
    i64 total = 0;
    Node&? cur = head;
    while (cur != null) {
        total += cur.value;
        cur .= cur.next;           // cur = cur.next would overwrite the node instead
    }
    return total;
}

void main() {
    Node[..>] nodes = [];
    Node& c = nodes.push(new Node(3, null));
    Node& b = nodes.push(new Node(2, c));
    Node& a = nodes.push(new Node(1, b));
    println("${sum(a)} ${sum(null)}");
}
```

```output
6 0
```

- **Narrowing follows the program's flow**: it holds inside `if (r != null)`, after
  `if (r == null) { return; }`, and after a `break` or `continue` taken when `r` is null.
- **It applies to local variables and parameters, not to fields**, since a call could
  change a field. Copy the field into a local first.
- **Rebinding the variable with `.=` ends it.**
- **`=` on a reference that may be null is an error**, since it would write through it.
- **`null` is only for nullable references.**

```java
class Node { i64 value; Node&? next; }

i64 second(Node& n) {
    Node&? after = n.next;
    if (after == null) { return -1; }
    return after.value;            // fine: narrowed by the early return
}

i64 third(Node& n) {
    return n.next.value;           // a field isn't narrowed
}

i64 first(Node&? n) {
    return n.value;                // nothing tested n
}

i64 skip(Node&? n) {
    if (n != null) {
        n .= n.next;               // rebinding ends the narrowing
        return n.value;
    }
    return 0;
}

void point(Node&? n, Node& m) {
    n = m;                         // `=` would write through n
    i64 x = null;
}

void main() {}
```

```error
example.zn:10:12: error: this may be null here
help: check `x != null` first, in a local variable; narrowing doesn't apply to fields (DESIGN §5.1)
example.zn:14:12: error: this may be null here
example.zn:20:16: error: this may be null here
example.zn:26:5: error: this may be null here, and `=` writes through it
help: to rebind it, write `.=`; to write the value it refers to, check `x != null` first (DESIGN §5.1)
example.zn:27:13: error: `null` is only for nullable references (`T&?`), and this is i64
```

## Parameters

A plain parameter is a **read-only copy**: the function can't change it, and nothing it
does changes the caller's value. A `T&` parameter lets the function change the caller's
value. Arrays follow the same rule: a function that grows an array takes it as
`T[..>]&`. A slice parameter (`T[:]`, `string`) takes any array of those elements, or a
part of one, read-only, and a **writable view** (`T[:]&`) lets the function write the
elements without adding or removing any.

```java
class Point { i64 x; i64 y; }

void moveRight(Point& p, i64 by) {
    p.x += by;
}

i64 sumAll(i64[:] xs) {
    i64 total = 0;
    for (i64 x : xs) { total += x; }
    return total;
}

void addSquares(i64[..>]& out, i64 n) {
    for (i64 i = 1; i <= n; i++) { out +> i * i; }
}

void zero(i64[:]& xs) {
    for (i64& x : xs) { x = 0; }
}

void main() {
    Point p = new Point(1, 2);
    moveRight(p, 10);
    println("${p.x} ${p.y}");

    i64[..>] squares = [];
    addSquares(squares, 4);
    println("${squares} sum ${sumAll(squares)}");
    zero(squares[1:3]);
    println("${squares}");
}
```

```output
11 2
[1, 4, 9, 16] sum 30
[1, 0, 0, 16]
```

- **Nothing marks a changeable argument at the call site.** `addSquares(squares, 4)` may
  grow `squares`, and only the declaration says so. Writing `&squares` means the same,
  and draws a warning.
- **The slots of an array parameter say what the function may do**: a `T[..>]&` one can
  append and never remove ([Arrays](04-arrays.md)). It may still write and reorder the
  elements, as a sort does.
- **A plain parameter may be passed by pointer** when that can't be told apart from a
  copy. When the same call passes a `&` argument that may share its storage, it is
  copied, so writing through the `&` one doesn't change what the plain one shows.
- **A writable view can only be taken of an array the code may change**: not of a plain
  parameter, and not of a `string`, which is read-only and may be a literal in read-only
  memory. It is rebound with `.=`, like a reference.
- **A function can't return a reference to a plain parameter**, which is its own copy.

```java
class Point { i64 x; i64 y; }

void bump(i64 n) {
    n++;
}

void shift(Point p) {
    p.x = 1;
}

Point& pick(Point p) {
    return p;
}

void add(i64[..>] xs) {
    xs +> 1;
}

void zero(i64[:]& xs) {
    for (i64& x : xs) { x = 0; }
}

void clearStart(i64[..>] xs) {
    zero(xs[0:2]);
}

void main() {
    string s = "literal";
    s[0] = 'L';
}
```

```error
example.zn:4:5: error: can't change this: `n` is a plain parameter, which is a read-only copy
help: declare it `i64& n` to let the function change the caller's value (DESIGN §5.1)
example.zn:8:5: error: can't assign to this: `p` is a plain parameter, which is a read-only copy
example.zn:12:12: error: can't return a reference to this: `p` is a plain parameter, which is a read-only copy
example.zn:16:5: error: can't use `+>` here, since it inserts into the array: `xs` is a plain parameter, which is a read-only copy
help: declare it `i64[..>]& xs` to let the function change the caller's value (DESIGN §5.1)
example.zn:24:10: error: can't pass this to `zero`, which may change it: `xs` is a plain parameter, which is a read-only copy
example.zn:29:5: error: can't assign to this: a `string` is a read-only view
help: take a writable view (`T[:]&`) of the array itself (DESIGN §4.10)
```

```java
void addOne(i64[..>]& xs) {
    xs +> 1;
}

void main() {
    i64[..>] nums = [];
    addOne(&nums);
    println("${nums}");
}
```

```warning
example.zn:7:13: warning: `&` isn't needed at the call
help: the parameter's declaration says whether the argument may change (DESIGN §5.1)
```

## Roots and rule S

Every reference and slice has a **root set**: the variables whose storage it may point
into, plus `static` for static data such as string literals. The compiler works it out.
**Rule S** is that a reference may be stored only where every one of its roots lives at
least as long as the place it is stored in. So a reference never outlives what it points
into.

| Expression | Its roots |
|---|---|
| a variable, or a field or element of one (`items[3].name`) | that variable |
| `r.f`, `r[i]` or a slice of `r`, where `r` is a reference | the roots of `r` |
| `a[i:j]`, `a.push(x)`, an array passed as a slice | the roots of `a` |
| a string literal | `static` |
| a call | the roots of the arguments its result may point into |
| `c ? x : y` | the roots of `x` and of `y` |

Which variable lives longer:
- a static lives for the whole run, so it outlives everything;
- a local lives to the end of its block, and a block's locals are released in reverse
  order, so a local outlives those declared after it and those in blocks inside it;
- a caller's variables outlive those of the functions it calls.

So within one block, a variable can hold references only into variables declared before
it:

```java
interface Shape { f64 area(); }
class Circle implements Shape { f64 r; f64 area() { return 3.0 * r * r; } }

void main() {
    Shape&[..>] early = [];
    Circle[..>] circles = [];
    early.push(circles.push(new Circle(1.0)));
}
```

```error
example.zn:7:5: error: a reference into `circles` can't be stored in `early`: `circles` is declared after it, so it is released first
help: declare `circles` before `early`, in the same block or an enclosing one (REFERENCES §3.3)
```

### Summaries instead of annotations

For every function, the compiler infers a **summary** from its body: which arguments its
result may point into, which arguments it stores into which others or into statics, which
arrays it may remove from or grow, and which references into one argument it still uses
after removing from another. Each call is checked against the summary with its own
arguments, so nothing is written in the declaration. This result may be rooted at either
argument:

```java
string longest(string a, string b) {
    return a.len >= b.len ? a : b;
}

void main() {
    u8[..>] first = [];
    u8[..>] second = [];
    first.append("short");
    second.append("much longer");
    string w = longest(first, second);
    println(w);
    w = longest(w, "a literal that is longer still");
    println(w);
}
```

```output
much longer
a literal that is longer still
```

Storing it where one of those arguments doesn't live long enough is an error. A
variable's root set is the union of everything stored into it anywhere in its scope, not
what it holds at one point, so this is rejected although `w` isn't used after the inner
block:

```java
string longest(string a, string b) {
    return a.len >= b.len ? a : b;
}

void main() {
    u8[..>] outer = [];
    outer.append("outer");
    string w = outer;
    println(w);
    {
        u8[..>] inner = [];
        inner.append("inner text");
        w = longest(outer, inner);     // w isn't used after this, and it's still an error
    }
}
```

```error
example.zn:13:9: error: a reference into `inner` can't be stored in `w`: `inner` is in an inner block, so it is released first
help: declare `inner` before `w`, in the same block or an enclosing one (REFERENCES §3.3)
```

Declare the view inside the inner block instead. A check that follows the program's order
would accept more programs, and is an open question
([REFERENCES §3.5](../REFERENCES.md#35-precision-per-variable-not-per-point-in-the-program)).

- **Summaries go through any depth of calls**, recursion included: the compiler iterates
  over the call graph until nothing changes. A function that returns its argument on one
  path and a literal on another is rooted at both.
- **A call through an interface** uses the union of the summaries of every method that
  implements it, and a call through a function value the union over every lambda with as
  many parameters. The whole program is visible, so both are known.
- **A function is compiled once** per array layout and generic instance, never once per
  root: summaries are only for checking.

Inside a function, two `&` parameters are separate storage, so removing from one while a
reference into the other is in use is fine there. A call that passes the same array as
both is the error, and says what the function does:

```java
void refill(i64[..~]& from, i64[..~]& into) {
    i64& first = from[0];
    into.clear();
    into +> 7;
    println("${first}");
}

void main() {
    i64[..~] a = [1];
    i64[..~] b = [2];
    refill(a, b);
    refill(a, a);
}
```

```error
example.zn:12:5: error: `refill(...)` removes from `into` while it still uses a reference into `from`, and in this call both are in `a`
help: pass separate storage, or have `refill` bind the reference again after the change (REFERENCES §4.2)
```

- **`this` and statics count as parameters**: `b.merge(b)` is refused when `merge` uses a
  reference into its object after removing from its argument.
- **A call through a function value** is checked against every lambda with as many
  parameters, since any of them may be the one it runs: `g(x, x)` is refused when one of
  them would use a reference into its first parameter after removing from its second.
- **A function that uses the reference only before the change**, or one that only grows an
  array while reading a slice of it, as `appendAll(xs, xs)` would, takes the same array
  twice.
- The reasons are in [REFERENCES §4](../REFERENCES.md#4-functions-summaries-instead-of-annotations).

### Returning and storing references

A function can return a reference rooted at a `&` or slice parameter, or at a static, but
not one into its own locals, which are released when it returns. A class may have
reference fields (`T&`, `T&?`, interface references and slices), and a value of that class
carries their roots, so rule S applies to it wherever it goes:

```java
class Word { string text; i64 line; }       // text: a view into text kept elsewhere

Word firstWord(string s, i64 line) {
    i64 i = 0;
    while (i < s.len && s[i] != ' ') { i++; }
    return new Word(s[0:i], line);          // rooted at the caller's s
}

void main() {
    u8[..>] text = [];
    text.append("hello world");
    Word[..>] words = [];
    words +> firstWord(text, 1);
    words +> firstWord("from a literal", 2);
    Word copy = words[0];                   // a copy still points into text
    println("${copy.text} ${words[1].text}");
}
```

```output
hello from
```

A static lives for the whole run, so it can hold references only to statics, such as a
static buffer, and to static data, such as string literals. Stores made by called
functions count, as their summaries record:

```java
class Word { string text; i64 line; }

string saved = "";

void remember(string s) {
    saved = s;
}

string make() {
    u8[..>] b = [];
    b.append("built here");
    return b;
}

void main() {
    remember("a literal");                 // fine: static data
    Word[..>] words = [];
    {
        u8[..>] text = [];
        text.append("short lived");
        words +> new Word(text[0:5], 1);
        saved = text;
        remember(text);
    }
}
```

```error
example.zn:12:5: error: this returns a reference into `b`, which is released when the function returns
help: return a copy, or a reference rooted in what a `&` or slice parameter refers to (REFERENCES §3.3)
example.zn:21:18: error: a reference into `text` can't be stored in `words`: `text` is in an inner block, so it is released first
example.zn:22:9: error: a reference into `text`, a variable of this function, can't be stored in a static, which lives for the whole run
help: it would outlive what it points into; store a copy instead (REFERENCES §3.3)
example.zn:23:9: error: `remember(...)` stores a reference from one argument into another, and a reference into `text`, a variable of this function, can't be stored in a static, which lives for the whole run
```

- **To return text built in a function**, return it as an array, `u8[.] make()`, which
  is built where the caller keeps it ([Results](#results-are-built-where-the-caller-keeps-them)).
- **A summary records stores between arguments too**: a `void register(Shape&[..>]& all,
  Circle& c) { all.push(c); }` makes `register(all, cs[0])` an error when `cs` is
  declared after `all`.
- **A field may refer to a whole array**, as `u8[..>]& out;` does in a writer that
  appends to its caller's buffer. The array must outlive the object.
- **References into an array that can remove elements can't be stored in fields at all**
  ([below](#references-into-removable-arrays-stay-in-locals-and-parameters)).

## Temporaries

A **temporary** is a value no variable holds: a function's result used in an expression,
or interpolated text passed on. Temporaries that need storage go on a **scratch stack**,
each written once at its exact size, and a statement's temporaries are all released when
it ends, so ten million calls such as `measure("item ${i}")` in a loop run in under 2 MB.
Text kept in a variable is built in the variable's own storage instead:

```java
void show(string s) {
    println("[${s}]");
}

void main() {
    i64 n = 3;
    show("${n * 2} more");         // a temporary on the scratch stack
    u8[.] s = "${n} items";        // built in s's own storage
    u8[.] upper = "abc".toUpper();
    println("${s} ${upper}");
}
```

```output
[6 more]
3 items ABC
```

Keeping a view of a temporary is an error, which suggests `u8[.]`. That covers views a
function returns into a temporary argument, views of a temporary's contents, a for-each
over a temporary array, and what `fold` or `map` returns when their lambda builds text. A
lambda passed to a function of the program's own can't return a temporary at all, since
the function may keep the result ([Functions and lambdas](08-functions.md)):

```java
string firstWord(string s) {
    i64 i = 0;
    while (i < s.len && s[i] != ' ') { i++; }
    return s[0:i];
}

string wrap(string s, func(string => string) f) {
    return f(s);
}

void main() {
    i64 n = 3;
    println(firstWord("${n} items"));      // fine: used within its statement
    string s = "${n} items";
    string w = firstWord("${n} items");
    string up = "abc".toUpper();
    for (string p : "a,b".split(",")) { println(p); }
    i64[..>] xs = [1, 2, 3];
    string all = xs.fold("", (string acc, i64 x) => "${acc}${x}");
    string r = wrap("x", (string t) => "<${t}>");
}
```

```error
example.zn:14:16: error: this text is a temporary, built only for this statement, so `s` can't keep a view of it
help: build it in the variable's own storage instead, as in `u8[.] s = "${x}";` (REFERENCES §3.3)
example.zn:15:16: error: this text is a temporary, built only for this statement, so `w` can't keep a view of it
example.zn:16:17: error: this refers into a temporary value, which is released at the end of the statement
help: store the value in a variable first, and refer to that (REFERENCES §3.3)
example.zn:17:21: error: this refers into a temporary value, which is released at the end of the statement
example.zn:19:18: error: this text is a temporary, built only for this statement, so `all` can't keep a view of it
example.zn:20:26: error: this lambda returns something built only for the statement it's in, which is released before its result is used
help: only a lambda passed straight to a built-in, such as `map` or `fold`, can return one; build it in a variable outside the lambda (DESIGN §9.3)
```

The fixes: `u8[.] s = ...` for the text and the `fold`, `var parts = "a,b".split(",");`
before the loop, and building the wrapped text in a variable outside the lambda.

## Rule R: removals

Removing elements from an array frees their slots, and the array reuses them right away: a
popped slot by the next push, a cursor array's hole by the next insert. **Rule R** makes
that safe: a removal is an error at any point where a reference into the array, or a slice
of it, may still be used afterwards.

```java
void main() {
    i32[..~] stack = [];
    stack +> 1 +> 2 +> 3;
    i32& top = stack[^1];
    @_ = stack ->;
    println("${top}");
}
```

```error
example.zn:5:10: error: `->` removes from `stack` while `top` may still refer into it
help: `top`, declared on line 4, is used after this; use it before the removal, or bind it again after (REFERENCES §5.2)
```

Using the reference only before the removal is fine, and so is binding it again after:

```java
void main() {
    i32[..~] stack = [];
    stack +> 1 +> 2 +> 3;
    i32& top = stack[^1];
    println("${top}");
    @_ = stack ->;
    top .= stack[^1];
    println("${top}");
}
```

```output
3
2
```

- **"May still be used" is liveness**: the check follows the program's order through
  branches and loops, so a removal in one branch is an error only if that branch can
  reach a use.
- **A loop body is checked in the state its earlier passes leave it in**: a use at the top
  of the next pass counts.
- **The error names the reference and the line it was declared on.**
- **A reference follows its element.** Inserts move nothing on a data stack, at either
  end or at a cursor, so they end nothing: after a front insert, a reference reads the
  same value at the next index.

```java
void inLoop(i32[..~]& stack) {
    i32& top = stack[^1];
    for (i64 i = 0; i < 2; i++) {
        println("${top}");         // the second pass uses top after the pop
        @_ = stack ->;
    }
}

void inBranch(i32[..~]& stack, bool full) {
    i32& top = stack[^1];
    if (full) {
        println("${top}");
        @_ = stack ->;             // fine: nothing uses top after this
    } else {
        @_ = stack ->;
        println("${top}");
    }
}

void main() {}
```

```error
example.zn:5:14: error: `->` removes from what `stack` refers to while `top` may still refer into it
example.zn:15:14: error: `->` removes from what `stack` refers to while `top` may still refer into it
```

### What counts as a removal

- `pop`, `popFront`, `removeBefore`, `removeAfter`, `removeAt`, `truncate`, `clear`,
  `retain` and `dedup`, and the operators `->`, `-<`, `-^` and `^-`;
- assigning a whole array, and rebuilding a variable-size value
  ([rule V](#rule-v-variable-size-values));
- replacing a value whose fields hold an array that can lose elements, such as a class
  with an `i32[..~8]` field: assigning it, directly or through a reference, or moving such
  values between slots with `swap`, a sort or `fill`;
- `remove` and `clear` on a `hashmap`, whose entries never move on insert, so `put` and
  `getOrAdd` end nothing;
- calling a function whose summary says it does any of these to an argument, or a
  function value when any lambda with as many parameters does.

A deferred statement runs at the end of its block, the last one first, and also when a
`break` or `continue` leaves the block, so a reference it uses is live until then.

Otherwise writes, `sort`, `reverse`, `swap` and `fill` aren't removals: a reference keeps
its slot, which may then hold another element's value of the same type.

```java
class Item { i64 n; }

void trim(i32[..~]& xs, i64 keep) {
    while (xs.len > keep) {
        @_ = xs ->;
    }
}

void main() {
    i32[..~] nums = [1, 2, 3];
    i32& r = nums[0];
    trim(nums, 1);                 // trim's summary: it removes from its argument
    println("${r}");

    hashmap<string, i64> counts = [];
    i64& a = counts.getOrAdd("a", 0);
    counts.put("b", 5);            // fine: entries never move on insert
    @_ = counts.remove("b");
    a++;

    Item[..~] items = [];
    items +> new Item(1);
    Item& it = items[0];
    defer println("${it.n}");      // runs at the end of the block
    @_ = items ->;
}
```

```error
example.zn:12:5: error: `trim(...)` removes from `nums` while `r` may still refer into it
example.zn:18:10: error: `remove` removes from `counts` while `a` may still refer into it
example.zn:25:10: error: `->` removes from `items` while `it` may still refer into it
```

The summary says only that `trim` may remove from its argument, not how much, so the first
is an error though `nums[0]` survives the call.

### For-each loops

A for-each keeps its place in the array, so removing from the array inside the loop is an
error, whether the loop binds its elements by value or by reference. A for-each over an
iterator that views something, such as `text.lines()`, keeps that view for the whole loop:

```java
void main() {
    i64[..~] xs = [1, 2, 3];
    for (i64 x : xs) {
        if (x == 2) { xs.clear(); }
    }
    for (i64& x : xs) {
        if (x == 2) { @_ = xs ->; }
    }
    u8[..~] text = [];
    text.append("a\nb\n");
    for (string line : text.lines()) {
        if (line == "a") { text.clear(); }
    }
}
```

```error
example.zn:4:23: error: `clear` removes from `xs` inside a for-each over it
help: the loop keeps its place in it; remove afterwards, or use `retain` or a repeat loop (REFERENCES §9)
example.zn:7:28: error: `->` removes from `xs` inside a for-each over it
example.zn:12:28: error: `clear` removes from `text` inside a for-each over it
```

`retain`, such as `xs.retain((i64 v) => v % 2 == 0)`, removes what a test rejects in one
pass. A repeat loop, `while (T x : xs ->)`, pops until the array is empty and may push more
as it goes, as the explicit stack in [Recursion](#recursion) does.

### Capped arrays, rings and gap buffers

Capped arrays (`T[..~16]`), stored inline, follow rule R too: a removal ends references
into one. One that changes only at its end, or only at its start, never moves an element,
so its inserts end nothing:

```java
void main() {
    i32[..~16] small = [];
    @_ = small +> 1;
    i32& r = small[0];
    @_ = small +> 2;               // fine: nothing moves
    println("${r}");
    i32[>..4] front = [2, 3];
    i32& x = front[0];
    @_ = front +< 1;               // fine: x follows its element
    println("${x} ${front}");
}
```

```output
1
2 [1, 2, 3]
```

Two kinds do move elements: a **ring**, a capped array that changes at both ends
(`T[~.~4]`), which wraps, and a **gap buffer**, a capped array with a cursor (`u8[~8]`),
which moves elements on every edit. An insert into one counts as a removal, and so does
anything that uses it as one slice, which may first make it one block: a slice, a sort, a
comparison, a text method, or passing it where a slice or `string` goes. Going element by
element, as indexing, `contains` or `${}` do, doesn't. Slices taken since its last change
agree with each other, since nothing moves until the next change. A ring or gap buffer
can't be passed to a parameter whose elements stay where they are, such as a
`T[..>]&` ([Arrays as parameters](04-arrays.md#arrays-as-parameters)).

```java
void main() {
    i64[~.~4] q = [1, 2];          // a ring
    i64& r = q[0];
    @_ = q +> 5;
    println("${r}");

    i64& r2 = q[0];
    i64[:] s = q;
    println("${r2} ${s}");

    u8[~8] g = "ab";               // a gap buffer
    string v = g;
    @_ = g +^ 'x';
    println(v);
}
```

```error
example.zn:4:10: error: `+>` can move the elements of `q` while `r` may still refer into it
help: `r`, declared on line 3, is used after this, and an insert into a ring or gap buffer can move its elements; use it before, or bind it again after (REFERENCES §5.4)
example.zn:8:16: error: this slice can rearrange `q` while `r2` may still refer into it
help: `r2`, declared on line 7, is used after this; a slice, sort or comparison makes a ring or gap buffer one block first, which can move its elements; use it before, or bind it again after (REFERENCES §5.4)
example.zn:13:10: error: `+^` can move the elements of `g` while `v` may still refer into it
```

Pools are exempt from rule R ([Pools](#pools-for-long-running-programs)). The reasons for
the rule are in [REFERENCES §5](../REFERENCES.md#5-removal-rule-r).

## References into removable arrays stay in locals and parameters

A **removable array** has `<` or `~` in a slot, so elements can be taken out of it.
References into one, or into a ring, a gap buffer or a hash map, can be kept only in local
variables and parameters, never in fields, array elements or statics. The compiler then
sees every reference that might still be used when something is removed.

```java
class Item { i64 n; }
class Holder { Item& it; }

void main() {
    Item[..~] items = [new Item(1)];
    Item&[..>] refs = [];
    Item& local = items[0];        // fine: a local
    Holder h = new Holder(items[0]);
    refs.push(items[0]);
}
```

```error
example.zn:8:16: error: a reference into `items`, which can remove elements, can be kept only in a local variable or a parameter
help: a field, an element or a static could keep it past a removal; store an index instead, or make the array grow-only (`T[..>]`) or a `pool` (REFERENCES §5.3)
example.zn:9:5: error: a reference into `items`, which can remove elements, can be kept only in a local variable or a parameter
```

References into **grow-only** arrays (`T[..>]`, `T[>..]`, `T[>.>]`) and frozen ones can be
stored anywhere rule S allows. Their only removals are `clear` and assigning the whole
array, and the compiler records which variables hold references into each array, so rule R
sees those holders too:

```java
class Item { i64 n; }
class Holder { Item& it; }

void main() {
    Item[..>] items = [];
    Item&[..>] refs = [];
    items +> new Item(1) +> new Item(2);
    refs.push(items[1]);
    Holder h = new Holder(items[0]);
    println("${refs[0].n} ${h.it.n}");
    items.clear();
    println("${h.it.n}");
}
```

```error
example.zn:11:5: error: `clear` removes from `items` while `h` may still refer into it
```

### A removable field is storage of its own

A field holding a removable array is checked as storage of its own, like a part of a
[root](#roots-classes-with-several-growable-arrays). A reference into it is rooted at that
field however the object is reached, and a removal from it is checked only against
references into it. So a method can't keep a reference into one of its object's removable
arrays in another field, while a reference to the object itself survives a pop from its
array:

```java
class Stack {
    u8[..~] items = [];
    u8&? last = null;

    mut void push(u8 x) {
        items +> x;
        last .= items[^1];
    }
    mut u8 pop() {
        u8 x @err = items.pop();
        return #err x;
    }
    u8& top() { return items[^1]; }
}

void drain(Stack& s) {
    while (u8 x : s.pop()) { println("${x}"); }
}

void main() {
    Stack s = new Stack();
    s.push(1);
    u8& t = s.top();
    println("top ${t}");
    drain(s);                      // fine: t isn't used after this
    s.push(2);
    u8& t2 = s.top();
    @_ = s.pop();
    println("${t2}");
}
```

```error
example.zn:7:9: error: a reference into `this.items`, which can remove elements, can be kept only in a local variable or a parameter
example.zn:28:10: error: `pop(...)` removes from `s.items` while `t2` may still refer into it
```

Keep an index, such as `items.len - 1`, where a field would need a reference
([Designing programs](#designing-programs-under-these-rules)).

### Repeat loops over a reused buffer

A repeat loop's variables are declared afresh on each pass, after its call runs, so the
call may clear the buffer that the previous pass's value viewed: `stdin.nextLine(buf)`
does. Keeping those values past their pass, in an array declared outside the loop, is the
error above, since the buffer is removable. Copy what has to stay into a buffer that only
grows, and keep views of that:

```java
// The next word of text, in buf; fails with END after the last.
string nextWord(string text, i64& pos, u8[..~]& buf) {
    buf.clear();
    while (pos < text.len && text[pos] == ' ') { pos++; }
    if (pos >= text.len) { return #status.END buf; }
    while (pos < text.len && text[pos] != ' ') { buf +> text[pos]; pos++; }
    return buf;
}

void main() {
    string text = "one two three";
    i64 pos = 0;
    u8[..>] store = [];            // only grows, so views into it stay valid
    string[..>] kept = [];
    u8[..~] buf = [];
    while (string w : nextWord(text, pos, buf)) {
        i64 start = store.len;
        store.append(w);
        kept +> store[start:];     // `kept +> w` would keep a view into buf
    }
    println("${kept}");
}
```

```output
[one, two, three]
```

The reasons are in [REFERENCES §5.3](../REFERENCES.md#53-where-references-into-removable-arrays-can-be-held).

## Rule V: variable-size values

A variable-size value, a `T[.]` or `T[.*]` array or a record with such a field, is laid
out by its contents, so changing its size in place would overwrite what follows it.
**Rule V** is that it is written once, when it is built:

- **V1**: a variable-size field can't be assigned, and neither can a variable-size value
  through a reference, as `for (Item& it : items) { it = ...; }` would. Its fixed-size
  fields can.
- **V2**: a variable holding a variable-size value can be assigned as a whole. That
  rebuilds the value on its data stack, and counts as a removal under rule R.
- **V3**: an array of variable-size values is added to at its end, looped over by
  reference and cleared. It can't be indexed, sliced, popped, sorted or given a cursor.
- **V4**: a record built in an array whose building can grow that array, as in
  `items.push(make(items))`, is built aside and moved to the array's end, so it compiles.
  A constructor building a value in place, as `new` does there, can't grow, move or remove
  from that array, nor can anything it calls.
- **V5**: a constructor writes the record from its start, so it assigns each
  variable-size field at most once, in declaration order, by a statement of its own.

```java
class Order { i32 id; u8[.*] customer; }

void main() {
    Order o = new Order(1, "alice");
    o.id = 7;                      // fine: a fixed-size field
    o.customer = "bob";
    Order[..~] book = [];
    book +> new Order(3, "cy") +> new Order(1, "al");
    for (Order x : book) { }
    @_ = book ->;
    Order[.~.] cur = [];
}
```

```error
example.zn:6:5: error: a variable-size field can't be assigned: u8[.*] is stored inline, so its size is set when the value is built
help: build a new value instead (REFERENCES §6, V1)
example.zn:9:20: error: the elements are variable-size records of Order, looped over in place
help: bind each by reference: `for (Order& x : ...)` (REFERENCES §6)
example.zn:10:10: error: Order[..~] can't be removed from with `->`: its elements are variable-size, so it is only added to at its end, looped over and cleared
example.zn:11:5: error: an array of Order can't have a cursor: its elements are variable-size, and a cursor array reuses holes for elements of one size
```

Assigning the whole variable rebuilds it, and a view into the old value can't be used
after that:

```java
class Order { i32 id; u8[.*] customer; }

void main() {
    Order o = new Order(1, "alice");
    string name = o.customer;
    o = new Order(2, "alexander");
    println(name);
}
```

```error
example.zn:6:5: error: assigning `o` removes from `o` while `name` may still refer into it
```

A constructor that assigns its variable-size fields out of order, or inside an `if`, is an
error:

```java
class Entry {
    u8[.*] key;
    i32 n;
    u8[.*] value;

    new(string k, string v) {
        value = v;
        key = k;
        n = 1;
    }

    new(string k, string v, bool upper) {
        n = 0;
        key = k;
        if (upper) { value = v.toUpper(); } else { value = v; }
    }
}

void main() {}
```

```error
example.zn:8:9: error: `key` is assigned after `value`, which is declared after it
help: a variable-size record is written from its start, so assign its variable-size fields in the order they're declared (REFERENCES §6)
example.zn:15:22: error: `value` is variable-size, so it's assigned by a statement of its own in the constructor's body
help: not inside an `if`, a loop or an expression; choose its value with `?:` (REFERENCES §6)
```

`?:` chooses the value in one statement. A `T[.]` field, or an array of frozen arrays, is
built from a growable array, which is copied in once it is complete:

```java
class Entry {
    u8[.*] key;
    i32 n;
    i64[.] samples;

    new(string k, i64 count, bool squares) {
        n = (i32) k.len;           // fixed-size fields are assigned anywhere
        key = k;
        i64[..>] xs = [];
        for (i64 i = 1; i <= count; i++) { xs +> (squares ? i * i : i); }
        samples = xs;
    }
}

void main() {
    Entry e = new Entry("squares", 4, true);
    println("${e.key} (${e.n}): ${e.samples}, last ${e.samples[^1]}");
    i64[.][..>] rows = [];
    for (i64 r = 1; r <= 3; r++) {
        i64[..>] row = [];
        for (i64 c = 1; c <= r; c++) { row +> r * c; }
        rows +> row;
    }
    for (i64[.]& row : rows) { println("${row}"); }
}
```

```output
squares (7): [1, 4, 9, 16], last 16
[1]
[2, 4]
[3, 6, 9]
```

- **A variable-size local lives on a data stack of its own**, so it falls under the
  [recursion rule](#recursion).
- **Not yet**: variable-size values in hash maps, sets, pools and tuples, which the C
  backend refuses ([What isn't there yet](15-not-yet.md)).
- The reasons are in [REFERENCES §6](../REFERENCES.md#6-variable-size-values-rule-v).

## Rule L: links

There are three ways for a value in an array to point at another:
1. **a reference field**, `T&` or `T&?`: an 8-byte pointer, rooted like any reference, as
   in the list in [Nullable references](#nullable-references);
2. **a self-relative link**, `T&<i32>` (or `<i16>`, `<i8>`): a signed byte offset from
   the field to its target in the same array;
3. **a pool link**, `T&<u32 in arr>`: an offset from the start of `arr`, a static
   grow-only array.

All three point only into arrays whose elements never move and whose memory never holds
another type: grow-only and frozen arrays, capped ones included, except a capped ring that
grows at both ends. Reference fields may also point into pools. An array may hold
references rooted at itself, which is how its elements link to each other.

### Self-relative links

A self-relative link is smaller than a reference, and means the same thing wherever the
whole array is copied. A link field reads as a `T&?`, and is rebound with `.=`:

```java
class Node { i32 key; Node&<i32>? left; Node&<i32>? right; }

void main() {
    Node[..>] tree = [];
    Node& root = tree.push(new Node(5, null, null));
    root.left .= tree.push(new Node(3, null, null));
    root.right .= tree.push(new Node(8, null, null));
    Node&? left = root.left;                   // following a link is an add
    if (left != null) { println("left of 5 is ${left.key}"); }
    Node[..>] copy = tree;                     // the links mean the same in the copy
    Node&? r = copy[0].right;
    if (r != null) { println("right of 5 in the copy is ${r.key}"); }
}
```

```output
left of 5 is 3
right of 5 in the copy is 8
```

Each node takes 12 bytes, where two `Node&?` fields would make it 24.

- **L1, same array**: a self-relative link is stored only from a reference into the array
  that holds the value it is in. The compiler checks this with the roots of rule S,
  through calls and constructors, at each call with its own arguments, as `attach` below
  shows. When it can't tell which array a reference is in, that is an error too.
- **L2, never copied out**: the offsets are measured from where the value sits, so a
  value with self-relative links is built in its array by `push(new ...)` or `+> new ...`
  and never copied. It can't be a local, a parameter, a result or a by-value loop
  variable, and `sort`, `reverse`, `retain` and `filter` can't be used on an array of
  them. A whole array of them copies, since its links stay inside it.
- **L4, width**: storing a link whose offset doesn't fit its width stops the program.
- **A link can't refer to an interface**, which would need a method table too.

```java
class Node { i32 key; Node&<i32>? left; }

void attach(Node& into, Node& what) {
    into.left .= what;
}

void main() {
    Node[..>] a = [];
    Node[..>] b = [];
    Node& x = a.push(new Node(1, null));
    Node& y = b.push(new Node(2, null));
    attach(x, a.push(new Node(3, null)));  // fine: both in a
    x.left .= y;
    attach(x, y);
}
```

```error
example.zn:13:5: error: this link is in an element of `a`, and what it would refer to is in `b`
help: a self-relative link refers only into its own array; use a plain reference, `T&?`, to refer elsewhere (REFERENCES §7.4, L1)
example.zn:14:5: error: this link is in an element of `a`, and what it would refer to is in `b`
```

```java
interface Shape { f64 area(); }

class Node { i32 key; Node&<i32>? left; }
class Bad { Shape&<i32>? s; }
class City { i32 id; City&<u32 in cities>? next; }

City[..~] cities = [];

Node make() { return new Node(1, null); }

void main() {
    Node[..>] tree = [];
    tree +> new Node(5, null);
    Node n = tree[0];
    tree.reverse();
}
```

```error
example.zn:4:26: error: a link can't refer to an interface: it holds only an offset, and an interface reference needs its method table too
example.zn:5:35: error: a pool link is measured from a grow-only array, whose elements never move, and `cities` is City[..~]
help: declare it `T[..>]` (REFERENCES §7.3)
example.zn:9:1: error: `Node` holds self-relative links, so it can't be returned by value
example.zn:14:10: error: `Node` holds self-relative links, so it can't be kept in a local by value
help: use a reference, `Node&` (REFERENCES §7.4, L2)
example.zn:15:10: error: `reverse` moves or copies elements, and `Node` holds self-relative links, which are measured from where each one sits
help: keep the elements where they are, and reorder references to them instead (REFERENCES §7.4, L2)
```

A link whose target is out of its width's reach stops the program where it is stored:

```java
// Each node takes more than 127 bytes, out of an i8 offset's reach.
class Node {
    i32 key;
    Node&<i8>? next;
    u8[..~128] label;
}

void main() {
    Node[..>] list = [];
    Node& first = list.push(new Node(1, null, "first"));
    Node& second = list.push(new Node(2, null, "second"));
    println("linking");
    second.next .= first;
    println("linked");
}
```

```stderr exit=1
example.zn:13: this link's target is too far away for its 1-byte offset
```

### Arrays of links

A capped array field of links, `T&<i32>[..~k]`, holds each as an offset from its own
slot. It grows and shrinks at its end, so its elements stay in their slots:

```java
class Node {
    i32 key;
    Node&<i32>[..~4] kids;
}

i32 total(Node& n) {
    i32 sum = n.key;
    for (Node& k : n.kids) { sum += total(k); }
    return sum;
}

void main() {
    Node[..>] tree = [];
    Node& root = tree.push(new Node(1, []));
    Node& two = tree.push(new Node(2, []));
    root.kids +> two +> tree.push(new Node(3, []));
    two.kids +> tree.push(new Node(4, []));
    for (Node& k : root.kids) { println("${k.key}"); }
    println("total ${total(root)}, ${root.kids.len} children");
}
```

```output
2
3
total 10, 2 children
```

An array of links takes `+>`, indexing, for-each, `.len` and `clear` so far. **Not yet**:
`push`, which would give a reference to a slot that holds only an offset, the removals
that give an element back, slices, and passing the array whole.

### Pool links

A pool link is an offset from the start of a static grow-only array, so a value whose
links are all pool links copies anywhere:

```java
class City {
    u8[..~16] name;
    City&<u32 in cities>? next;
}

City[..>] cities = [];

void main() {
    City& a = cities.push(new City("Oslo", null));
    City& b = cities.push(new City("Bergen", a));
    City copy = b;                             // fine: pool links copy
    City&? n = copy.next;
    if (n != null) { println("${copy.name} -> ${n.name}"); }
}
```

```output
Bergen -> Oslo
```

- **The array** is a static `T[..>]` of what the link refers to: a top-level one, as
  here, or a class's static field, written `C.name`. The built-in `pool<T>` isn't one.
- **L1** applies: a pool link is stored only from a reference into its own array.
- The reasons, and what each kind costs, are in
  [REFERENCES §7](../REFERENCES.md#7-links-inside-data-rule-l).

## Rule G: one growable value on top of each data stack

Growing never moves anything because each growable value is the topmost value on its data
stack for its whole life. **Rule G** keeps it there, and it decides what can hold a
growable array:

- **Each growable local, and each part of a root, gets a data stack of its own.**
- **A class can hold one growable field**, which lives on the data stack of the variable
  holding the object. A second one is an error that suggests a root, and so is a cursor
  array field, which needs two regions, its data and its index.
- **An array's elements can't grow**: there is no array of growable arrays, and a
  `hashmap`'s values can't grow. Freeze each inner array once it is built, as `rows` does
  above, or keep the elements in one shared array and store indexes.
- **A function that grows a `T[..>]&` parameter** grows it on the argument's stack.

```java
class Log {
    i64 lines;
    u8[..>] text;

    mut void add(string s) {
        text.append(s);
        text +> '\n';
        lines++;
    }
}

void main() {
    Log a = new Log(0);            // the growable field starts empty: not a parameter
    Log b = new Log(0);
    for (i64 i = 0; i < 1000; i++) { a.add("entry ${i}"); }
    b.add("one");
    b.add("two");
    println("${a.lines} lines, ${a.text.len} bytes");
    print(b.text);
}
```

```output
1000 lines, 9890 bytes
one
two
```

```java
class Parser {
    i64[..>] tokens;
    i64[..~] work;
}

class Doc {
    u32[.~.] text;
}

void main() {
    i64[..>][..>] rows = [];
    hashmap<string, i64[..>]> byName = [];
}
```

```error
example.zn:3:14: error: a class can hold only one growable field, and `tokens` is already one
help: declare `root Parser`, which gives each growable field its own data stack (DESIGN §6.6)
example.zn:7:14: error: a cursor array needs two regions, its data and its index, so only a root can hold one
example.zn:11:5: error: an element can't grow: an array of growable arrays isn't allowed
help: keep the elements in one shared array and store indexes (DESIGN §3.4)
example.zn:12:5: error: hashmap values can't grow
help: use a composite key, or values that are indexes into one shared array (DESIGN §4.14)
```

**Not yet**: copying a value with a growable field (`Log c = a;`, `logs +> a`, or
returning it), a temporary one (`logs +> new Log(0)`) and a static one pass `zink check`,
and the C backend reports that it doesn't support them yet. Passing one to a function's
plain parameter works: the parameter is a read-only copy for the call. Nor is there yet a check that nothing grows an array while text is being
built in it. The reasons for the rule are in
[REFERENCES §8](../REFERENCES.md#8-data-stacks-rule-g).

## Recursion

The compiler assigns data stacks to declarations when it compiles the program, so their
number is fixed. A growable local in a recursive function would need a new stack for every
call in progress. So **a function in a recursive cycle can't call into the cycle while one
of its growable or variable-size locals is in scope**. The compiler finds cycles from the
call graph, mutual recursion included, so nothing is marked.

```java
class Node { i64 value; Node&? left; Node&? right; }
class Item { u8[.*] sku; i32 qty; }

void collectEach(Node& n, i64[..>]& out) {
    i64[..>] mine = [];
    Node&? l = n.left;
    if (l != null) { collectEach(l, mine); }
    mine +> n.value;
    out.pushAll(mine);
}

i64 depth(i64 n) {
    if (n == 0) { return 0; }
    {
        u8[..>] label = [];        // fine: released before the call
        label.append("level ${n}");
    }
    return 1 + depth(n - 1);
}

bool isEven(i64 n) {
    if (n == 0) { return true; }
    return isOdd(n - 1);
}

bool isOdd(i64 n) {
    i64[..>] trace = [n];
    if (n == 0) { return false; }
    return isEven(n - 1);
}

i64 count(i64[..>] xs, i64 n) {
    if (n == 0) { return xs.len; }
    return count(xs, n - 1);
}

i32 levels(i32 n) {
    Item here = new Item("level", n);
    if (n == 0) { return here.qty; }
    return levels(n - 1) + here.qty;
}

void main() {}
```

```error
example.zn:7:22: error: `mine` is growable and in scope during this call, which can recurse back into `collectEach`
help: a growable local would need a new data stack on every call; pass it in by reference from outside the cycle (REFERENCES §8.4)
example.zn:29:12: error: `trace` is growable and in scope during this call, which can recurse back into `isOdd`
example.zn:34:18: error: a growable value can't be passed by value into a recursive cycle
help: pass it by reference, `T&` (REFERENCES §8.4)
example.zn:40:12: error: `here` is a variable-size record and in scope during this call, which can recurse back into `levels`
help: such a local would need a new data stack on every call; pass it in by reference from outside the cycle (REFERENCES §8.4)
```

- **A growable local in a block that ends before the recursive call is fine**, as in
  `depth`.
- **A growable value can't be passed by value into the cycle**: pass it by reference. A
  slice parameter is fine.
- **A variable-size record and a root** count as growable locals.
- **The array `filter`, `map` or `sortedIndexes` is building** counts as one while its
  lambda runs, so a lambda that calls back into the cycle is an error.

Data that has to outlive the recursive calls is passed in by reference from outside the
cycle:

```java
class Node { i64 value; Node&? left; Node&? right; }

void collect(Node& n, i64[..>]& out) {
    Node&? l = n.left;
    if (l != null) { collect(l, out); }
    out +> n.value;
    Node&? r = n.right;
    if (r != null) { collect(r, out); }
}

void main() {
    Node[..>] nodes = [];
    Node& one = nodes.push(new Node(1, null, null));
    Node& three = nodes.push(new Node(3, null, null));
    Node& two = nodes.push(new Node(2, one, three));
    i64[..>] out = [];
    collect(two, out);
    println("${out}");
}
```

```output
[1, 2, 3]
```

The other way is an explicit stack in a loop. It needs no recursion, so a deep structure
can't overflow the native stack either:

```java
class Node { i64 value; Node&? left; Node&? right; }

i64[..>] values(Node& top) {
    i64[..>] out = [];
    Node&[..~] todo = [top];       // references into a grow-only array may go anywhere
    while (Node& n : todo ->) {
        out +> n.value;
        Node&? l = n.left;
        if (l != null) { todo +> l; }
        Node&? r = n.right;
        if (r != null) { todo +> r; }
    }
    return out;
}

void main() {
    Node[..>] nodes = [];
    Node& one = nodes.push(new Node(1, null, null));
    Node& three = nodes.push(new Node(3, null, null));
    Node& two = nodes.push(new Node(2, one, three));
    println("${values(two)}");
}
```

```output
[2, 3, 1]
```

`examples/dirwalk.zn` walks directories with a stack of pending ones, and
`examples/jsonfmt.zn` writes sorted JSON with a stack of open containers of its own.
More on recursion is in [Functions and lambdas](08-functions.md), and the reasons are in
[REFERENCES §8.4](../REFERENCES.md#84-recursion).

## Results are built where the caller keeps them

`new` means "construct here", never "allocate", and a value is built at its final
destination through any depth of calls: `items.push(parse(line))` writes the parsed record
straight into the array. A function that returns a growable or variable-size value builds
it where the caller keeps it, so returning it copies nothing:

```java
i64[..>] squares(i64 n) {
    i64[..>] out = [];
    for (i64 i = 1; i <= n; i++) { out +> i * i; }
    return out;
}

u8[.] shout(string s) {
    u8[..>] b = [];
    b.append(s.toUpper());
    b +> '!';
    return b;
}

u8[.] countdown(i64 n) {
    if (n == 0) { return "liftoff"; }
    return countdown(n - 1);       // passes the caller's storage on
}

void main() {
    i64[..>] sq = squares(4);      // built in sq, which can go on growing
    sq +> 25;
    println("${sq}");
    u8[.] loud = shout("hello");
    println("${loud} ${shout("inline")} ${countdown(3)}");
}
```

```output
[1, 4, 9, 16, 25]
HELLO! INLINE! liftoff
```

- **The local a function returns by name starts in the caller's storage**, so a 100 MB
  array that a function builds and its caller keeps takes 100 MB, not 200.
- **Two cases copy, once**: assigning to an array that already exists, as in
  `x = rev(x)`, since the arguments may be reading it; and a temporary result, which is
  copied to the scratch stack and lasts to the end of its statement.
- The reasons are in [DESIGN §3.3](../DESIGN.md#33-building-in-place-decided).

## Roots: classes with several growable arrays

A class keeps its fields in one record, so it can hold one growable field. A **root** is
declared like a class, with `root` in place of `class`, and gives each growable field a
data stack of its own, so it may have any number of them, in any order:

```java
class Token { string text; i64 line; }

root Lexer {
    Token[..>] tokens;             // a data stack of its own
    i64[..~] depth;                // another one
    i64 lines = 0;                 // ordinary fields stay in the root's record

    mut void scan(string src) {
        for (string line : src.lines()) {
            lines++;
            for (string w : line.words()) { tokens +> new Token(w, lines); }
        }
    }
}

void report(Lexer& lx) {
    for (Token& t : lx.tokens) { println("${t.line}: ${t.text}"); }
}

void main() {
    Lexer lx = new Lexer();
    lx.scan("let x\nlet y");
    lx.depth +> 0;
    report(lx);
    println("${lx.tokens.len} tokens, depth ${lx.depth.len}");
}
```

```output
1: let
1: x
2: let
2: y
4 tokens, depth 1
```

- **A root can have everything a class can**: fields, methods, constructors, `implements`
  and type parameters, and also cursor arrays.
- **Its value is a small record of where each part lives**, not one block of bytes, so it
  is never copied. It is a named local, passed by reference as `Lexer&`. A static root
  passes `zink check`, and isn't compiled to C yet.
- **A reference into a part is rooted at that part**, such as `lx.tokens`, which lives as
  long as `lx`. Rule R checks a removal from one part only against references into that
  part.
- **The compiler assigns each root variable's parts their stacks**, as it does a growable
  local's. So a root can't be an array element, and a root local falls under the
  [recursion rule](#recursion): pass one declared outside the cycle by reference.
- **`root` is a keyword only at the start of a declaration**, so a variable may be named
  `root`.

```java
root Pair {
    i64[..>] a;
    i64[..~] b;
}

void show(Pair p) {}

Pair make() {
    Pair p = new Pair();
    return p;
}

void fill(Pair& p) {
    p.a +> 1;
}

class Holder { Pair inner; }

void main() {
    Pair p = new Pair();
    Pair q = p;
    fill(new Pair());
    Pair[..>] many = [];
}
```

```error
example.zn:6:11: error: `Pair` is a root, so it can't be passed by value
help: use a reference, `Pair&` (DESIGN §6.6)
example.zn:8:1: error: `Pair` is a root, so it can't be returned by value
example.zn:17:21: error: a root can't be stored by value in a class, a root or an array
example.zn:21:14: error: `Pair` is a root, which can't be copied
help: refer to it instead: `Pair& r = ...;` (DESIGN §6.6)
example.zn:22:10: error: `Pair` is a root, so it can't be a temporary
help: create it as a named local or static: `Pair x = new Pair(...);` (DESIGN §6.6)
example.zn:23:5: error: a root can't be an element
```

Parts are separate storage, so a pop from one doesn't end references into another:

```java
root Work {
    i64[..~] a;
    i64[..~] b;
}

void main() {
    Work w = new Work();
    w.a +> 1;
    w.b +> 2;
    i64& x = w.a[0];
    @_ = w.b ->;                   // fine: x is in w.a
    println("${x}");
    @_ = w.a ->;
    println("${x}");
}
```

```error
example.zn:13:10: error: `->` removes from `w.a` while `x` may still refer into it
```

Programs with roots: `examples/cachesim.zn`, an LRU cache of a pool and a map;
`examples/lisp.zn`, an interpreter whose heap, text, roots and work lists are parts of one
root; and `examples/quarry/quarry.zn`, a world and a mesh builder. Classes are in
[Classes, interfaces and generics](06-classes.md), and the reasons for roots in
[DESIGN §6.6](../DESIGN.md#66-roots-several-growable-fields-decided).

## Pools for long-running programs

A grow-only array gives memory back only when its scope ends, and references into a
removable one can't be kept in fields. Data that keeps changing for a whole run, such as a
cache or a server's sessions, goes in a **pool**: `pool<T>` pairs a grow-only array with a
list of free slots. `add` reuses a freed slot or pushes a new one, `free` gives one back,
and a `Handle<T>` names a slot:

```java
class Session { i64 id; i64 requests; }

void main() {
    pool<Session> live = [];
    Handle<Session> a = live.add(new Session(1, 0));
    Handle<Session> b = live.add(new Session(2, 0));
    Session& s @_ = live.get(a);
    s.requests++;
    @_ = live.free(a);
    Handle<Session> c = live.add(new Session(3, 0));   // reuses a's slot
    println("s now reads session ${s.id}");
    Session& gone @missing = live.get(a);
    println("a is stale: ${missing == status.MISSING}");
    println("${live.len} live sessions");
}
```

```output
s now reads session 3
a is stale: true
2 live sessions
```

- **Pools are exempt from rule R.** A slot's memory is never released while the pool
  lives, and only ever holds a `T`, so a reference kept past a `free` reads whatever value
  of the same type is in the slot: a logic bug, never memory corruption.
- **A stale handle fails** with `status.MISSING`, since each slot counts its reuses.
- **References into a pool can be kept in fields**, as `cachesim`'s list does with its
  `Node<K>&?` fields.
- **A pool's memory is the most slots live at once**, so a loop that adds and frees for
  ever stays the same size.
- Pools and handles are in [Maps, sets, pools and iterators](09-collections.md), and the
  reasons in [DESIGN §3.5](../DESIGN.md#35-pools-for-long-running-programs-decided).

## What the model costs

These are known trade-offs
([DESIGN §3.4](../DESIGN.md#34-what-the-model-costs-decided-as-known-trade-offs)):

- **Memory goes back to a data stack only when its scope ends.** An array keeps the slots
  it frees for its own later inserts at the same end, so one that once held 50 million
  elements keeps that memory after `clear`, until its scope ends.
- **A queue that pushes at one end and pops at the other moves through its reservation.**
  A `T[~.~]` used that way takes one more slot for every push, however few elements it
  holds: ten million pushes through a `Handle<Job>[~.~]` that never held more than 101
  took 80 MB. A capped ring, `Handle<Job>[~.~128]`, ran the same loop in 1.7 MB.
- **Long-running programs need structure**: per-iteration data in an inner scope, and a
  pool or a periodic rebuild into a fresh array for what changes for the whole run.
- **The rules above limit the shapes of data**: one growable field per class, no array of
  growing arrays, no growable local across a recursive call, no removal while a reference
  may be used, and no reference in a field that could outlive what it points into or
  point into an array that moves or removes elements.
- **A reference field takes 8 bytes**, and a copy of a value holding one still points at
  the original's storage.
- **Virtual memory is required**, since each data stack is a large reservation.

## Designing programs under these rules

The rules favor a few patterns, which the example programs use throughout.

**Keep indexes instead of references** into arrays that change. An index is a number,
which can go in any field. `examples/lisp.zn` keeps every value as an `i64`, a tagged
integer or an index into one `Cell[..>]` heap, and reuses freed cells from a free list.

```java
class Song { u8[..~32] title; i64 seconds; }

class Playlist {
    i64 playing = 0;               // an index: a `Song&` field couldn't be kept
    Song[~.~] songs = [];

    mut void add(string title, i64 seconds) {
        songs +> new Song(title, seconds);
    }

    mut void dropFirst() {
        @_ = songs -<;
        if (playing > 0) { playing--; }
    }

    Song& current() { return songs[playing]; }
}

void main() {
    Playlist p = new Playlist();
    p.add("intro", 30);
    p.add("theme", 200);
    p.add("outro", 45);
    p.playing = 1;
    p.dropFirst();
    println("now playing ${p.current().title}, ${p.songs.len} songs left");
}
```

```output
now playing theme, 2 songs left
```

**Read the input into one buffer that only grows, and keep views into it.** A `u8[..>]`
never removes, so views of it can go in arrays, fields and maps. `examples/csvsort.zn`
keeps each row's line and key as views into the whole input, and `examples/wordfreq.zn`
counts words in a `hashmap<string, i64>` whose keys are views into it:

```java
class Row {
    string line;                   // a view into text
    string name;                   // a view into the same text
    i64 score;
}

void main() {
    u8[..>] text = [];             // the whole input; every row points into it
    text.append("ada,92\nbrian,75\ncleo,88\n");

    Row[..>] rows = [];
    for (string line : text.lines()) {
        i64 comma = line.indexOf(',');
        i64 score @_ = i64.parse(line[comma + 1:]);
        rows +> new Row(line, line[0:comma], score);
    }
    rows.sort((Row a, Row b) => a.score > b.score);
    for (Row& r : rows) { println("${r.name} ${r.score}"); }
}
```

```output
ada 92
cleo 88
brian 75
```

**Reuse one line buffer, and declare the views into it inside the loop.** Rule R then has
nothing to object to when the next line clears it; `cachesim` reads its trace with
`while (string line : input.nextLine(buf))`. Text that has to outlive its line is copied
into a grow-only buffer, as in [Repeat loops](#repeat-loops-over-a-reused-buffer).

**Declare what refers into an array after the array**: arrays of references, and objects
holding views, go after what they point into, in the same block or an inner one.

**Put per-iteration data in an inner scope**, where it is freed in one store at the end of
each pass, and pass long-lived arrays down by reference.

**Use an explicit stack instead of recursion** when a recursive function would need a
growable local ([Recursion](#recursion)).

**Use a pool, or a root holding one,** for data that is added and removed for the whole
run, and a capped ring for a queue that never empties.

## Not yet

Designed and not implemented yet; [What isn't there yet](15-not-yet.md) collects them all:
- copying a value with a growable field, a temporary one, and a static one (a static
  root included), in the C backend;
- rule G's check that nothing grows an array while text is being built in it;
- variable-size values in hash maps, sets, pools and tuples;
- on an array of links, `push`, the removals that give an element back, slices, and
  passing it whole; and constructors that can fail, for a value with self-relative links;
- leaving out L4's width check when an array's reservation is small enough for the width.

Open questions, which could change what the rules accept, are in
[REFERENCES §12](../REFERENCES.md#12-open-questions): roots that follow the program's
order, a data stack chosen at run time (which would let a recursive function keep
growable locals), removal inside a for-each by value, slices of cursor arrays, and links
between two parameters' arrays.
