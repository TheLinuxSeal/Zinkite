# 6. Classes, interfaces and generics

Zinkite's classes look like Java's, but they are values: a class is exactly its fields,
stored where it is declared, and `=` copies it. There is no inheritance, no heap and no
destructor. This chapter covers classes and their constructors, visibility, `mut` methods,
`final` fields, static members and constants, interfaces, enums, generics and their bounds,
`packed` classes, and cleanup with `defer`. Where class values are stored, and what references to them may do, is in
[Memory and references](05-memory.md). The reasons behind each decision are in
[DESIGN §6](../DESIGN.md#6-classes-and-interfaces) and [§10](../DESIGN.md#10-generics).

## Classes

A class declares fields, constructors, methods and static members, in any order. Inside a
method, the object's fields are reached by their bare names, or through `this`.

```java
class Item {
    i32 id;
    private f32 weight;
    string label = "none";

    new(i32 id, f32 weight) {
        this.id = id;
        this.weight = weight;
    }

    mut void heavier(f32 by) { weight += by; }
    bool isHeavy() { return weight > 10.0; }
    f32 getWeight() { return this.weight; }
    static Item zero() { return new Item(0, 0.0); }
}

void main() {
    Item a = new Item(1, 2.5);
    a.heavier(9.0);
    println("${a.id} ${a.getWeight()} ${a.isHeavy()} ${a.label}");
    Item z = Item.zero();
    println("${z.id} ${z.label}");
}
```

```output
1 11.5 true none
0 none
```

- **A class has no header and no hidden fields.** A value is exactly as large as its fields,
  plus the padding that aligns them. Fixed-size fields are laid out largest first, whatever
  order the source declares them in ([DESIGN §11](../DESIGN.md#11-memory-layout-decided)).
- **`this.id`** names the field when a parameter of the same name hides it.
- **`new` constructs a value where it goes**, in a local, a field or an array element, never
  on a heap ([DESIGN §3.3](../DESIGN.md#33-building-in-place-decided)).
- **Methods live inside the class body.** A free function can be called as if it were a
  method: with `i64 manhattan(Point p)` declared, `p.manhattan()` calls `manhattan(p)`.
- **A method can be called on a temporary**: `new Temp(100.0).fahrenheit()`.
- **There is no inheritance.** `class Dog extends Animal` is an error, "Zinkite has no
  inheritance", that suggests an interface ([Interfaces](#interfaces)), and `protected` is
  reserved.

## Fields

A field may have an initializer: a literal, a call, a `new`, an array literal. Initializers
run before the constructor's body, in a default constructor too.

```java
class Point { i64 x; i64 y; }

class Settings {
    string name;
    i64 width = 40 * 2;
    Point origin = new Point(0, 0);
    i64[..>] tabs = [4, 8];
}

void main() {
    Settings s = new Settings("term");
    s.origin.x = 5;
    println("${s.name} ${s.width} ${s.origin.x},${s.origin.y} ${s.tabs.len}");
}
```

```output
term 80 5,0 2
```

- **A field that is a class holds the whole value inline**, not a pointer to it:
  `s.origin.x` is part of `s`'s bytes.
- **Arrays on a data stack, and `hashmap`, `hashset` and `pool` fields, start empty**
  without an initializer. A capped array (`u8[..~16]`) or a frozen one (`T[.]`) needs a
  value like any other field.
- **A class has at most one growable field**: an array on a data stack, a `hashmap`, a
  `hashset` or a `pool`. A second is an error, "a class can hold only one growable field,
  and `list` is already one", that suggests a root ([Roots](#roots)). A cursor array needs
  two regions, so only a root can hold one.
- **A variable-size field** (`u8[.*]`, `T[.]`) makes the class variable-size, with rules of
  its own ([Memory and references](05-memory.md)).
- **An initializer reads only the fields set before it.** Initializers run in declaration
  order, before the constructor, so `i64 b = a + 1;` can follow `i64 a = 1;`, and a field
  without an initializer can't be read in one, nor a later field, nor a method called:

```java
class Rect {
    i64 w;
    i64 area = w * 2;
}

void main() {
    Rect r = new Rect(3);
    println("${r.area}");
}
```

```error
example.zn:3:16: error: `w` can't be read in `area`'s initializer: it has no initializer, so it isn't set until the constructor runs
help: compute it in a constructor (DESIGN §6.2)
```


## Constructors

### The default constructor

A class that declares no constructor gets one. It takes every field that has no initializer
and doesn't start empty, in the order the source declares them, and assigns each.

```java
class Row { string line; string key; f64 num; }
class Options { bool numeric = false; i64 column = 0; string path = ""; }
class Words { string title; string[..>] list; }

void main() {
    Row r = new Row("b,2", "2", 2.0);
    Options o = new Options();
    Words w = new Words("fruit");
    w.list +> "apple" +> "pear";
    println("${r.line} ${r.key} ${r.num} / ${o.numeric} ${o.column} '${o.path}'");
    println("${w.title}: ${w.list.len} words");
}
```

```output
b,2 2 2.0 / false 0 ''
fruit: 2 words
```

- **The order is the source's**, not the order of the fields in memory.
- **A class that declares any constructor gets no default one**, as in Java. A call with the
  wrong number of arguments says so: "`new Row` takes 3 arguments, not 1".
- **It is as visible as the least visible field it sets** ([Visibility](#visibility)).

### Writing constructors

A constructor is named `new` and has no return type. Constructors can be overloaded like
methods ([Functions and lambdas](08-functions.md)), and one can hand the work to another
with `this.new(...)`.

```java
class Rect {
    f64 w;
    f64 h;

    new(f64 w, f64 h) {
        this.w = w;
        this.h = h;
    }

    new(f64 side) {
        this.new(side, side);
    }

    f64 area() { return w * h; }
}

void main() {
    Rect a = new Rect(2.0, 3.0);
    Rect b = new Rect(4.0);
    Rect[..>] rects = [];
    rects.push(new(1.0, 5.0));
    rects +> new(2.0);
    println("${a.area()} ${b.area()} ${rects[0].area()} ${rects[1].area()}");
}
```

```output
6.0 16.0 5.0 4.0
```

- **`return;` ends a constructor early**, once every field is assigned.
- **`this.new(...)` runs at most once on any path**, and not inside a loop. Before it,
  statements may compute its arguments, but nothing may assign a field or use `this`:
  "`w` is assigned before `this.new(...)`, which assigns every field". After it, every field
  counts as assigned.
- **`this` can't be used until every field is assigned**, except to assign fields, since a
  method could read a field that isn't set: "`describe` can't be called until every field
  is assigned, and `h` isn't yet".
- **A constructor can't keep a reference to `this`**, in a field, an argument's object or a
  static: the value is built where the compiler chooses and copied to where it goes, so
  the reference would point at the old place. Set it once the value is in place, as
  `n.me .= n;`.
- **Field initializers run as the start of every constructor**, the default one included,
  so they follow the same rules for references: interpolated text, which lasts only for
  its statement, can't initialize a `string` field.

### Every field on every path

Every field without an initializer must be assigned before a constructor ends, and can't be
read before it is assigned. There are no zero defaults. The compiler checks the worst case,
as Java's definite assignment does: a field counts as assigned at a point only when every
path to that point assigns it.

```java
class Rect {
    f64 w;
    f64 h;

    new(f64 w, bool square) {
        this.w = w;
        if (square) { h = w; }
    }

    new(f64 w) {
        h = this.w * 2.0;
        this.w = w;
    }
}

void main() {}
```

```error
example.zn:8:5: error: the constructor can end here without assigning `h`
help: every field without an initializer is assigned on every path through a constructor, or by `this.new(...)`; only a failing `return #code;` needn't (DESIGN §6.2)
example.zn:11:18: error: `w` is read before it is assigned
help: in a constructor, a field can't be read until every path to the read has assigned it (DESIGN §6.2)
```

- **An `if` may go either way**, so a field assigned in one branch only, or in an `if`
  without an `else`, isn't assigned after it.
- **A loop may run zero times**, so what a `while`, `for` or for-each body assigns doesn't
  count after it. `while (true)` and `for (;;)` end only through a `break`, so after one a
  field is assigned when it is assigned before every `break`. A `do` loop's body runs at
  least once, so it counts.
- **A `switch` counts** when it covers every case and every arm assigns the field, and
  **`&&`, `||` and `?:`** count only what every path through them assigns.
- **Every `return;` is an end**, so the fields must be assigned there too.
- **A reference field is bound by its first assignment**: `this.next = n;` on a field that
  isn't assigned yet binds it. After that, `=` writes through it and `.=` rebinds it
  ([Memory and references](05-memory.md)).
- **Locals follow the same rule**: one declared without an initializer can't be read until
  every path has assigned it.

### Constructors that fail

A constructor can fail like any call, with `return #code;`
([Errors and statuses](07-errors.md)). A failing return needn't assign anything: the object
is then its class's zero value, with every fixed-size field zeroed and every array empty, as
the result of any failed call is
([DESIGN §7.5](../DESIGN.md#75-the-value-on-failure-decided)). Fields assigned before the
failure are reset. The caller reads the status with `@`.

```java
class Size {
    i64 w;
    i64 h;

    // "3x4" is 3 by 4; anything else fails with INVALID.
    new(string spec) {
        i64 x @none = spec.indexOf('x');
        if (none) { return #status.INVALID; }
        i64 width @badWidth = i64.parse(spec[0:x]);
        w = width;
        i64 height @badHeight = i64.parse(spec[x + 1:]);
        if (badWidth || badHeight) { return #status.INVALID; }
        h = height;
    }
}

void main() {
    Size a @bad = new Size("3x4");
    println("${a.w} ${a.h} ${bad}");
    Size b @why = new Size("3x");
    println("${b.w} ${b.h} ${status.message(why)}");
}
```

```output
3 4 0
0 0 Invalid argument
```

- **`this.new(...)` is a call too**, so its status can be read, or passed on with
  `return ## this.new(side, side);`.
- **A status that isn't a constant may be 0**, which would be a success, so `return #(b);`
  counts as an end that must assign every field.
- **A field with no zero value must be assigned on every path, failing ones included**: a
  non-nullable reference (`T&`, `Shape&`), a non-nullable function, or a value that contains
  one: "`item` has no zero value, so it must be assigned even when the constructor fails". A
  nullable one has `null` as its zero value.
- **A failing `new` whose status nothing reads stops the program**, as other failures do
  ([Errors and statuses](07-errors.md)). Read the status with `@` wherever a constructor
  can fail and the program should go on.

### `new(...)` without the class name

Where the destination's type is known, `new(...)` takes its class from it: a declared local,
a field, a parameter, a return value, or an array being pushed to.

```java
class Point { i64 x; i64 y; }

class Segment { Point from; Point to; }

Point origin() { return new(0, 0); }

void main() {
    Point p = new(3, 4);
    Segment s = new(origin(), new(6, 8));
    Point[..>] path = [];
    path +> new(1, 1) +> p;
    println("${s.to.x} ${path.len} ${path[1].y}");
}
```

```output
6 2 4
```

Where nothing says what the class is, as in `var p = new(3, 4);`, it is an error: "`new(...)`
takes its class from where the value goes, and nothing says it here".

## Visibility

A member is `public`, `private`, or package-private when nothing is written.

- **`private`** members are visible only to the class's own methods, constructors and
  static functions, which can reach them on other values of the class too.
- **Package-private** members are visible to every file of the same package. A program in
  one file is one package, so there they are visible everywhere.
- **`public`** members are visible to other packages, which can import only public
  declarations ([Programs, packages and building](10-programs.md)).
- **There is no `protected`**, since there are no subclasses.

```java
class Account {
    private i64 cents = 0;
    mut void deposit(i64 c) { cents += c; }
    i64 balance() { return cents; }
    private bool overdrawn() { return cents < 0; }
}

void main() {
    Account a = new Account();
    a.deposit(250);
    println("${a.balance()} ${a.cents} ${a.overdrawn()}");
}
```

```error
example.zn:11:33: error: `cents` is private to `Account`
example.zn:11:44: error: `overdrawn` is private to `Account`
```

**A default constructor is as visible as the least visible field it sets**: private when one
of them is, package-private when one of them is. `Account`'s private field has an
initializer, so its default constructor doesn't set it, and `new Account()` works anywhere.
A class whose constructor sets a private field is made by its own static functions:

```java
class Token {
    private i64 value;
    string label;

    static Token issue(string label) { return new Token(label.len * 7919, label); }
    bool matches(i64 v) { return v == value; }
}

void main() {
    Token t = Token.issue("door");
    println("${t.label} ${t.matches(31676)}");
}
```

```output
door true
```

The library's `File` works this way: its handle is private, so `new File(5)` is an error,
"`File`'s implicit constructor, which sets its private fields, can't be used here", and a
`File` comes from `File.open` and the other static functions
([The library: files, processes, time and the command line](13-library-system.md)).

Across packages, package-private members are refused the same way. With this in
`shapes/Box.zn`:

```java
package shapes;

public class Box {
    public i64 w;
    i64 h;
    public static Box square(i64 s) { return new Box(s, s); }
}
```

a file of another package can use `Box.square(3)` and `b.w`, while `b.h` and
`new Box(1, 2)` are errors: "`h` is package-private to `shapes`" and "`Box`'s implicit
constructor, which sets its package-private fields, can't be used here". The `zink` command
compiles one file at a time today, so this case is checked by the compiler's own tests
rather than here.

## `mut` methods and read-only objects

A method that changes its object is marked `mut`. Every other method is read-only, which the
compiler checks.

```java
class Counter {
    string name;
    i64 n = 0;

    new(string name) { this.name = name; }

    mut void bump() { n++; }
    mut void bumpTwice() { bump(); this.bump(); }
    i64 doubled() { return n * 2; }
}

void main() {
    Counter c = new Counter("clicks");
    c.bump();
    c.bumpTwice();
    println("${c.name}: ${c.n}, doubled ${c.doubled()}");
}
```

```output
clicks: 3, doubled 6
```

A method that isn't `mut` can't assign its fields, call a `mut` method on its object, call a
changing method on one of its arrays or maps, or pass a field to a parameter that may change
it. The same holds wherever a value can't be changed: a plain parameter is a read-only copy,
and a reference bound to something read-only is read-only too.

```java
class Counter {
    i64 n = 0;
    mut void bump() { n++; }
    void reset() { n = 0; }
    void again() { bump(); }
}

void tryBump(Counter c) {
    c.bump();
}

void peek(Counter[..>] all) {
    Counter& first = all[0];
    first.bump();
}

void main() {}
```

```error
example.zn:4:20: error: can't assign to this: `n` is a field, and `reset` isn't `mut`, so it can't change its object
help: declare it `mut void reset(...)` if it should change it (DESIGN §6.1)
example.zn:5:20: error: can't call `bump` here, since it changes its object: `again` isn't `mut`, so it can't change its object
example.zn:9:5: error: can't call `bump` here, since it changes what it's called on: `c` is a plain parameter, which is a read-only copy
help: declare it `Counter& c` to let the function change the caller's value (DESIGN §5.1)
example.zn:14:5: error: can't call `bump` here, since it changes what it's called on: `first` refers to something that can't be changed here
help: it was bound to it: `all` is a plain parameter, which is a read-only copy
```

- **Read-only is deep**: in a method that isn't `mut`, a reference read from one of the
  object's fields can't be written through either. After `Node&? n = next;`, assigning
  `n.value` is an error, "`n` refers to something that can't be changed here". Reading
  through it, as a loop that walks a list with `cur .= cur.next` does, needs no `mut`.
- **Static fields aren't part of an object**, so any method can change them, `mut` or not,
  unless they're `final` ([Constants](#constants)). `static mut` is an error, since a static
  function has no object.
- **An interface method's `mut` must match** in every class that implements it
  ([Interfaces](#interfaces)).

## `final` fields

A `final` field is set when its value is made: by its initializer, or else by the class's
constructors. After that nothing changes it, as nothing changes a plain parameter.

```java
class Item {
    final i64 id;
    final i64 kind = 1;
    f64 weight;

    new(i64 id, f64 weight) {
        this.id = id;
        this.weight = weight;
    }

    mut void heavier(f64 by) { weight += by; }
}

void main() {
    Item a = new Item(7, 12.5);
    a.heavier(1.5);
    println("${a.id} ${a.kind} ${a.weight}");
    a = new Item(8, 1.0);         // a new value, with an id of its own
    println("${a.id} ${a.weight}");
}
```

```output
7 1 14.0
8 1.0
```

Changing one anywhere else is an error, in a `mut` method too:

```java
class Item {
    final i64 id;
    final i64 kind = 1;

    new(i64 id) {
        this.id = id;
        kind = 2;
    }

    mut void renumber() { id = 0; }
}

void main() {
    Item a = new Item(7);
    a.id++;
}
```

```error
example.zn:7:9: error: can't assign to this: `kind` is `final`, and its initializer sets it for good
help: drop the initializer to set it in the constructors instead (DESIGN §6.1)
example.zn:10:27: error: can't assign to this: `id` is `final`, so only a constructor sets it
help: make a new value with the one it should have, or drop `final` (DESIGN §6.1)
example.zn:15:5: error: can't change this: `id` is `final`, so only a constructor sets it
help: make a new value with the one it should have, or drop `final` (DESIGN §6.1)
```

- **Read-only means what it does for a plain parameter**: no `=`, `++` or `.=`, no
  insertion or removal, no `mut` method, and no passing it to a `&` parameter or taking a
  writable view of it. A reference bound to it can be read and not written through.
- **The whole value can still be replaced**, as `a = new Item(8, 1.0)` is above, and so can
  an array element holding one, which sorting does. A value has no identity, so `final`
  works as Swift's `let` does in a struct, not as Java's `final` does on an object.
- **A field with an initializer is fixed by it**, so a constructor can't change it either.
  An editable array, a map, a set or a pool needs no initializer, since it starts empty, so
  a `final string[..>] tags;` is filled by the constructors and fixed after.
- **Only the constructor itself sets one**, through `this`: not a method it calls, another
  value of the class, or a lambda inside it.
- **A `final` reference field can't be rebound with `.=`**, but what it refers to can still
  change.
- **The default constructor** takes the `final` fields that have no initializer, like the
  others ([The default constructor](#the-default-constructor)).
- **`final` goes only on fields.** On a class, a method or a constructor it is an error,
  since there are no subclasses. On a local or a parameter it is one too: the compiler
  already knows which locals change, and a plain parameter can't.

## Static members

`static` marks fields and functions that belong to the class rather than to a value. They
are reached through the class's name, or by their bare names inside it.

```java
class Ticket {
    static i64 issued = 0;
    i64 number;

    new() {
        issued++;
        number = issued;
    }
}

void main() {
    Ticket a = new Ticket();
    Ticket b = new Ticket();
    println("${a.number} ${b.number} ${Ticket.issued}");
}
```

```output
1 2 2
```

- **A static field lives for the whole run** and is initialized once, before `main`, in the
  order its dependencies need ([Programs, packages and building](10-programs.md)).
- **A class with only static members is named in lowercase**, as the library's `status`,
  `math`, `fs` and `path` are. This is a convention, which the compiler doesn't enforce.
- **A script file**, whose top level has functions and fields but no class around them, is
  wrapped in a hidden class, and its top-level functions and fields are static members of it
  ([Programs, packages and building](10-programs.md)).

### Constants

A `static final` field is set only by its initializer, which it must have. An integer,
`bool` or `string` one whose initializer is a constant expression is a **constant**: its
value is worked out when the program is compiled, so it can give an array's capacity and be
a case label. A constant expression is made of literals, other constants, `T.MIN` and
`T.MAX`, and the arithmetic, bitwise, shift, comparison and logical operators.

```java
class Blocks {
    public static final u8 AIR = 0;
    public static final u8 GRASS = AIR + 1;
    public static final u8 STONE = GRASS + 1;
    public static final i64 MAX_NAME = 8;
}

static final string GREETING = "hello";

string describe(u8 b) {
    return switch (b) {
        case Blocks.AIR => "air";
        case Blocks.GRASS => "grass";
        case Blocks.STONE => "stone";
        default => "unknown";
    };
}

void main() {
    u8[..~Blocks.MAX_NAME] name = [];
    name.append("quarry");
    println("${GREETING} ${name}: ${describe(Blocks.GRASS)} ${describe(9)}");
}
```

```output
hello quarry: grass unknown
```

A plain static, or a `static final` set by a call, is a variable as far as a cap or a case
label goes, and a constant that doesn't fit its type is an error:

```java
static i64 size = 4;
static final i64 TWICE = twice(2);
static final u8 BIG = 200 * 2;

i64 twice(i64 n) { return n * 2; }

void main() {
    u8[..~size] a = [];
    i64 v = 2;
    switch (v) {
        case size => println("size");
        case TWICE => println("twice");
        default => println("other");
    }
}
```

```error
example.zn:3:23: error: 400 doesn't fit in u8
help: `(u8%) x` wraps and `(u8|) x` saturates, if that's what you want (DESIGN §9.1)
example.zn:8:11: error: `size` isn't a constant, so it can't give an array's capacity
help: a capacity is a number or a constant, a `static final` set by a constant expression: `static final i64 MAX_NAME = 64;` (DESIGN §4.2, §6.1)
example.zn:11:14: error: `size` isn't a constant, and a case label is a literal, an enum constant or a constant
help: declare it `static final` with a value worked out from literals: `static final u8 GRASS = 1;` (DESIGN §6.1, §9.4)
example.zn:12:14: error: `TWICE` is `static final`, but its initializer isn't a constant expression, so it can't be a case label
help: a constant's initializer is literals, other constants and operators on them (DESIGN §6.1)
```

- **A constant is worked out exactly**: one that doesn't fit its type, a division by zero,
  or a shift by the type's width or more is an error. `~` on an unsigned type flips that
  type's bits, so `static final u32 ALL = ~0;` is 4294967295.
- **A constant is named like any static**: by its bare name in its own class or script
  file, and as `Blocks.GRASS` elsewhere, case labels included.
- **A constant is the same in every thread**, since nothing changes it
  ([Threads](11-threads.md)).
- **Other `static final` fields are read-only statics**, set before `main` like any
  static: one set by a call, and floats, such as `static final f64 PI = 3.14159;`, which
  nothing needs as constants.

## Class values

A class value has no identity: `=` copies its fields, and so do passing it to a plain
parameter and looping over an array by value. A reference (`T&`) refers to a value in place
([Memory and references](05-memory.md)).

```java
class Counter {
    i64 n = 0;
    mut void bump() { n++; }
}

void main() {
    Counter[..>] all = [];
    all +> new Counter() +> new Counter();
    for (Counter c : all) { c.bump(); }
    println("${all[0].n}");
    for (Counter& c : all) { c.bump(); }
    println("${all[0].n}");
    all[1].bump();
    println("${all[1].n}");
}
```

```output
0
1
2
```

The first loop bumps copies, the second the elements themselves, and `all[1].bump()` works
on the element in place.

- **A copy of a value with reference or view fields** still points where the original's did,
  such as a `string` field's text ([Memory and references](05-memory.md)).
- **`==` and `!=` on class values compare their fields**
  ([DESIGN §6.1](../DESIGN.md#61-classes-decided)). The checker accepts them, but code for
  them isn't generated yet ([What isn't there yet](15-not-yet.md)).
- **A class value can't be printed** until default formatting is decided
  ([DESIGN §17](../DESIGN.md#17-open-decisions)): "a class value can't be printed until
  default formatting is decided". Print its fields.

```java
class Point { i64 x; i64 y; }

void main() {
    Point p = new Point(1, 2);
    Point q = new Point(1, 2);
    println("${p != q}");
}
```

```error
example.zn:6:16: error: the C backend doesn't support comparing class values yet
help: `zink check` accepts it; code generation for it comes in a later milestone
```

## Interfaces

An interface declares methods, and a class says which interfaces it implements. A value is
used through an interface reference, `Shape&`, whose calls go to the method of the value's
class.

```java
interface Shape {
    f64 area();
}

class Circle implements Shape {
    f64 r;
    f64 area() { return 3.0 * r * r; }
}

class Rect implements Shape {
    f64 w;
    f64 h;
    f64 area() { return w * h; }
}

f64 total(Shape&[:] shapes) {
    f64 sum = 0.0;
    for (Shape& s : shapes) { sum += s.area(); }
    return sum;
}

void main() {
    Circle[..>] circles = [];
    Rect[..>] rects = [];
    Shape&[..>] shapes = [];
    shapes +> circles.push(new Circle(1.0));
    shapes +> rects.push(new Rect(2.0, 3.0));
    shapes +> circles.push(new Circle(2.0));

    for (Shape& s : shapes) {
        if (s instanceof Circle c) {
            println("circle of radius ${c.r}: ${s.area()}");
        } else {
            println("other: ${s.area()}");
        }
    }
    println("total ${total(shapes)}");
}
```

```output
circle of radius 1.0: 3.0
other: 6.0
circle of radius 2.0: 12.0
total 21.0
```

- **An interface reference is two pointers**, 16 bytes: one to the value and one to the
  method table for its class and that interface. Values carry no class id, so plain data
  stays header-free. A call through one is an indirect call.
- **An interface is never a value type.** `Shape s;`, a `Shape` field and a `Shape[..>]`
  are errors, since the size isn't known. Keep each class in its own array, and an array of
  references to them, as above. Those references follow the root rules
  ([Memory and references](05-memory.md)).
- **A class value converts to a reference to an interface it implements**: `circles.push`
  gives a reference to the new element, which goes into `shapes`, and a `Circle` local can
  be passed to a `Shape&` parameter. A temporary can't, since a reference needs a place to
  refer to.
- **A class can implement several interfaces**: `class Doc implements Named, Sized`.
- **`instanceof`** tests an interface reference's class, and with a name binds it as a
  reference of that class, `Circle&`, which can be written through. There are no casts
  between reference types.
- **A method must match the interface's declaration**, `mut` included: a `mut` method
  where the interface's isn't `mut` is an error, since the interface's callers expect it not
  to change the value.
- **`=` through an interface reference and `==` on two of them are errors**: the first would
  copy a value whose class isn't known, so `.=` rebinds instead, and the second needs an
  `instanceof` test first.
- **Interfaces can be generic** ([Generics](#generics)).

```java
interface Shape { f64 area(); }

class Square implements Shape {
    f64 side;
    f64 area() { return side * side; }
}

class Bad implements Shape { f64 side; }

class Holder { Shape s; }

void main() {
    Square a = new Square(1.0);
    Square b = new Square(2.0);
    Shape& s = a;
    Shape& t = b;
    s = t;
    bool same = s == t;
}
```

```error
example.zn:8:1: error: `Bad` implements `Shape` but has no method `area`
help: add `f64 area()`
example.zn:10:16: error: `Shape` is an interface, which isn't a value type
help: refer to one with `Shape&` (DESIGN §6.3)
example.zn:17:5: error: `=` writes through a reference, and an interface reference's value has no known class to copy
help: to make it refer to something else, rebind it with `.=` (DESIGN §5.1, §6.3)
example.zn:18:17: error: can't compare interface references: their values' classes aren't known here; test them with `instanceof` first (DESIGN §6.3)
```

**An interface method can fail when any class's method of that name can.** The compiler sees
the whole program, so a call needs no `@` when no implementation fails. Here `Decimal.read`
passes on the status of `i64.parse` with `return ##`, so every call of `read` can fail:

```java
interface Reader { i64 read(string s); }

class Decimal implements Reader {
    i64 read(string s) { return ## i64.parse(s); }
}

class Length implements Reader {
    i64 read(string s) { return s.len; }
}

void main() {
    Decimal d = new Decimal();
    Length l = new Length();
    Reader&[..>] readers = [];
    readers +> d +> l;
    for (Reader& r : readers) {
        i64 v @bad = r.read("12x");
        println("${v} ${bad}");
    }
    i64 w = readers[0].read("oops");
}
```

```output
0 22
3 0
```

```stderr exit=1
example.zn:20: `read` failed with INVALID, and nothing read its status
`Decimal.read` returned it at example.zn:4
```

**Not yet**: a lambda that captures variables, passed to a method called through an
interface, isn't compiled yet; the checker accepts it, and the code generator says so.
Default methods, static methods in interfaces, and storing values of several classes inline
are open ([DESIGN §17](../DESIGN.md#17-open-decisions), [What isn't there yet](15-not-yet.md)).

## Enums

An enum is a set of named constants. A constant is written with its enum's name,
`Suit.HEARTS`, except in a `case` label, which takes the bare name.

```java
enum Suit { CLUBS, DIAMONDS, HEARTS, SPADES }

class Card {
    Suit suit;
    u8 rank;

    bool isRed() { return suit == Suit.DIAMONDS || suit == Suit.HEARTS; }
}

string symbol(Suit s) {
    return switch (s) {
        case CLUBS => "C";
        case DIAMONDS => "D";
        case HEARTS => "H";
        case SPADES => "S";
    };
}

void main() {
    Card[..>] hand = [];
    hand +> new Card(Suit.SPADES, 1) +> new Card(Suit.HEARTS, 12) +> new Card(Suit.CLUBS, 7);
    hand.sortBy((Card c) => c.suit);
    for (Card& c : hand) { println("${symbol(c.suit)}${c.rank} red=${c.isRed()}"); }
}
```

```output
C7 red=false
H12 red=true
S1 red=false
```

- **A `switch` expression must cover every constant**, or have a `default` arm. A `switch`
  statement needn't.
- **Enums are ordered by declaration**: `Suit.CLUBS < Suit.SPADES`, and sorting by an enum
  key, as above, follows that order. They are `hashable`, so they can be map keys.
- **An enum value is its constant's index**, in the narrowest unsigned integer that holds
  every index: one byte for up to 256 constants, two for up to 65,536. A field of an enum
  type is laid out by that width, after the wider fields, so a `Card` takes 2 bytes
  ([DESIGN §11](../DESIGN.md#11-memory-layout-decided)).
- **There is no conversion between an enum and an integer**, in either direction. A table
  indexed by a number, or a byte written to a file, needs a `switch` or a number of its own.
- **An enum value can't be printed yet**: the compiler refuses it as it does a class value.
  Print a name from a `switch`, as `symbol` does.

```java
enum Color { RED, GREEN, BLUE }

void main() {
    Color c = Color.GREEN;
    string s = switch (c) {
        case RED => "warm";
        case GREEN => "cool";
    };
    switch (c) {
        case RED => println("red");
    }
    i64 n = (i64) c;
    Color e = Color.PINK;
}
```

```error
example.zn:5:16: error: a switch expression must cover every case
help: add a `default` arm, or a case for every enum constant (DESIGN §9.4)
example.zn:12:19: error: casts exist only for numbers, and this is Color
example.zn:13:21: error: `Color` has no constant `PINK`
```

## Generics

Classes, interfaces, functions and methods can take type parameters, in angle brackets:
after the name of a class or interface, and before the result type of a function or method.

```java
class Pair<A, B> {
    A first;
    B second;

    Pair<B, A> swapped() { return new Pair<B, A>(second, first); }
}

<T implements number> T sum(T[:] xs) {
    T total = 0;
    for (T x : xs) { total += x; }
    return total;
}

<T implements ordered> T clamp(T x, T lo, T hi) {
    return x < lo ? lo : (hi < x ? hi : x);
}

void main() {
    Pair<string, i64> p = new Pair<string, i64>("apples", 3);
    Pair<i64, string> q = p.swapped();
    println("${p.first} ${p.second} / ${q.first} ${q.second}");

    i64[..>] ints = [1, 2, 3, 4];
    f64[..>] floats = [0.5, 0.25];
    println("${sum(ints)} ${sum(floats)}");
    println("${clamp(15, 0, 10)} ${clamp("m", "a", "k")} ${clamp(0.5, 0.0, 1.0)}");
}
```

```output
apples 3 / 3 apples
10 0.75
10 k 0.5
```

### Type arguments are inferred

A generic function's type arguments come from its arguments. Each parameter whose type
mentions a type parameter fixes it from its argument, and an array passed where a slice is
expected gives its element type, so `sum(ints)` with an `i64[..>]` is `T = i64`. A literal
takes the type the other arguments fix. A class passed for an interface fixes them through
what it implements: with `class Cell<T> implements Box<T>`, a `Cell<string>` passed to
`<T> T unwrap(Box<T>& b)` gives `T = string`.

- **Arguments that fix one type parameter differently are an error**: `clamp(small, 0,
  limit)` with a `u8` and a `u32` says "`T` would be both u8 and u32 here; no conversion is
  applied to make them agree".
- **A generic class's type arguments are written** in `new Pair<string, i64>(...)`, or come
  from the destination with `new(...)`: `Pair<string, i64> p = new("apples", 3);`.
- **Type arguments aren't inferred from the result**, and can't be written on a call
  (`f<i64>(x)`), which is open ([What isn't there yet](15-not-yet.md)).

### Bounds

A generic body can use only what its type parameters' bounds allow, and it is checked once,
against them. A bound is written with `implements`, and `&` combines several:
`<T implements ordered & hashable>`. A bound is an interface, or one of seven built-in
bounds for the built-in operations
([DESIGN §10.1](../DESIGN.md#101-built-in-bounds-decided)):

| Bound | Satisfied by | Allows, besides `==` |
|---|---|---|
| `sint` | `i8` `i16` `i32` `i64` `i128` | what `int` allows, and negative literals |
| `uint` | `u8` `u16` `u32` `u64` `u128` | what `int` allows, and use as a condition |
| `int` | every `sint` and `uint` | what `number` allows, plus `%`, the checked and saturating operators (`+#`, `+|` and the others), bitwise operators and shifts, `T.MIN`, `T.MAX` and the bit methods |
| `float` | `f32` `f64` | what `number` allows, plus `T.NAN`, `T.INFINITY`, `T.EPSILON`, `isNaN()`, `isFinite()` and the `math` functions |
| `number` | every `int` and `float` | what `ordered` allows, plus `+`, `-`, `*`, `/`, unary `-`, casts between `T` and any number type, `T.from(x)` and `T.parse(s)` |
| `ordered` | numbers, `bool`, `string`, enums, and classes that implement it | `<`, `<=`, `>`, `>=` and `compareTo` |
| `hashable` | integers, `bool`, `string`, enums, and classes whose fields are all `hashable` | use as a `hashmap` key or a `hashset` element |

```java
interface Shape { f64 area(); }

class Square implements Shape {
    f64 side;
    f64 area() { return side * side; }
}

<T implements sint> T negate(T x) { return x * -1; }
<T implements uint> bool any(T x) { if (x) { return true; } return false; }
<T implements float> bool bad(T x) { return x.isNaN() || !x.isFinite(); }

<T implements ordered & hashable> i64 distinctAbove(T[:] xs, T floor) {
    hashset<T> seen = [];
    for (T x : xs) {
        if (x > floor) { seen.add(x); }
    }
    return seen.len;
}

<T implements Shape> f64 totalArea(T[:] xs) {
    f64 total = 0.0;
    for (T x : xs) { total += x.area(); }
    return total;
}

void main() {
    i8 small = 5;
    u16 flags = 0;
    f32 z = 0.0;
    println("${negate(small)} ${any(flags)} ${bad(z / z)} ${bad(1.5)}");
    string[..>] words = ["b", "a", "c", "b", "a"];
    Square[..>] squares = [];
    squares +> new Square(2.0) +> new Square(3.0);
    println("${distinctAbove(words, "a")} ${totalArea(squares)}");
}
```

```output
-5 false true false
2 13.0
```

- **Which bounds include which**: `sint` and `uint` are `int`; `int` and `float` are
  `number`; every `number` is `ordered`, and every `int` is `hashable`.
- **A type parameter bounded by an interface** is a known class in each instance, so its
  methods are called directly, with no method table.
- **No bound** (`<T>`) allows only what every type can do: copying, passing and `==`. A type
  that doesn't satisfy a bound is an error at the call: "`clamp` needs a type that is
  `ordered` for `T`, and Row isn't".
- **Bounds aren't types.** `ordered x;` is an error, and `int` and `float` used as types
  suggest `i64` and `f64`.
- **Only built-in types satisfy the number bounds**: a class can't implement `sint`,
  `uint`, `int`, `float` or `number`, so classes get no arithmetic operators.
- **`hashable` is worked out from a class's fields**; a class can't opt in or out. Floats
  aren't `hashable`, since NaN isn't equal to itself. Class keys in a `hashmap` or
  `hashset` aren't compiled yet ([What isn't there yet](15-not-yet.md)).

```java
<T> bool less(T a, T b) {
    return a < b;
}

void main() {
    int n = 3;
}
```

```error
example.zn:2:12: error: `T` isn't `ordered`; add it to the bound: `<T implements ordered>` (DESIGN §10.1)
example.zn:6:5: error: `int` is a generic bound, not a type (DESIGN §10.1)
help: for a type, write `i64` or `i32`
```

### Literals and casts in generic code

A literal in a generic body must fit every type its bound allows: 0 to 127 under `number`
and `int`, -128 to 127 under `sint`, 0 to 255 under `uint`, and anything that fits an `f32`
under `float`. So `T total = 0;` compiles under `number`, and `T k = 1000;` is an error,
"this literal doesn't fit every type `T` can be".

`(T) x` is a cast when `T` is a type parameter in scope. It is checked in each instance, so
a value that doesn't fit stops the program there, as any cast does:

```java
<T implements int> T scaled(T x, i64 percent) {
    return (T) ((i64) x * percent / 100);
}

void main() {
    u8 b = 200;
    println("${scaled(b, 50)}");
    println("${scaled(b, 200)}");
}
```

```output
100
```

```stderr exit=1
example.zn:2: the value doesn't fit in u8
```

### Each instance is compiled on its own

Generic code is monomorphized: each generic function, and each method and constructor of a
generic class, is compiled once for each set of type arguments the program uses, with the
sizes and offsets of that instance built in, and a generic class gets a layout for each
instantiation. `clamp` above becomes three C functions, for `i64`, `string` and `f64`, and
`Pair<string, i64>` and `Pair<i64, string>` are two structs. There is no runtime type
information. Generic code that would instantiate ever larger type arguments without end,
such as `Foo<T>` making a `Foo<Pair<T, T>>`, is rejected when the program is compiled.

### Classes that implement `ordered`

A class implements `ordered` with one method, `i32 compareTo(C other)`, which returns below
0, 0 or above 0, as `string.compareTo` does. A class that says it implements `ordered`
without one is an error that names the method to add.

```java
class Version implements ordered {
    u32 major;
    u32 minor;

    i32 compareTo(Version o) {
        if (major != o.major) { return major < o.major ? -1 : 1; }
        return minor < o.minor ? -1 : (minor > o.minor ? 1 : 0);
    }
}

void main() {
    Version[..>] vs = [];
    vs +> new Version(1, 10) +> new Version(0, 9) +> new Version(1, 2);
    vs.sort((Version a, Version b) => a.compareTo(b) < 0);
    for (Version& v : vs) { println("${v.major}.${v.minor}"); }
}
```

```output
0.9
1.2
1.10
```

The design lets `<`, `<=`, `>` and `>=` work on such values everywhere, in generic code too,
and lets `sort()` and `max` order them by `compareTo`. The checker accepts all of these, but
today only a direct call of `compareTo`, outside generic code, is compiled, so the example
sorts with a lambda that calls it. `a < b` on two class values is refused by the code
generator with the message shown in [Class values](#class-values), `sort()` and `max()` on
an array of them are refused too, and `max(a, b)` doesn't compile either
([What isn't there yet](15-not-yet.md)).

## `packed` classes

`packed`, in place of `class` and after the same modifiers (`public packed Instr`),
declares a class with no padding at all. The `Instr` below takes 9 bytes, two `i32`s and a
one-byte enum, where a `class` would take 12.

```java
enum Op { ADD, MUL }

packed Instr {
    Op op;
    i32 a;
    i32 b;

    i32 run() {
        return switch (op) {
            case ADD => a + b;
            case MUL => a * b;
        };
    }
}

void main() {
    Instr[..>] code = [];
    code +> new Instr(Op.ADD, 2, 3) +> new Instr(Op.MUL, 4, 5);
    Instr& second = code[1];
    second.a += 1;
    for (Instr& i : code) { println("${i.run()}"); }
}
```

```output
5
25
```

- **Its fields can sit at any address**, so a reference can't point at one: binding a
  reference to `code[0].a`, passing it to a `T&` parameter, returning it as a reference or
  binding a reference field to it are errors ("`a` is a field of `Instr`, which is `packed`,
  so it may sit at any address and a reference can't point at it"). A reference to the whole value, `Instr&`, is fine, and
  so is one to a field that is `packed` itself. Fields are read, written and copied as in any
  class.
- **Its fields** are numbers, `bool`, enums, references, links, slices (so `string`),
  functions and other `packed` classes. Arrays and other classes inside one, and type
  parameters, aren't supported yet.
- **It pays where a class with tail padding is the bulk of the data**, such as an
  interpreter's nodes. Elsewhere it changes nothing
  ([DESIGN §6.1](../DESIGN.md#61-classes-decided)).

## Cleanup: `defer`, and no destructors

Nothing runs when a value's scope ends, so freeing stays one store with no per-element work,
and every class can be copied. Cleanup is written with `defer stmt;`, which runs the
statement when its enclosing block ends: at the closing brace, or on a `return`, `break` or
`continue` that leaves the block.

```java
void step(i64 n) {
    println("start ${n}");
    defer println("end ${n}");
    defer println("cleanup ${n}");
    if (n == 2) { return; }
    println("work ${n}");
}

void main() {
    step(1);
    step(2);
    for (i64 i = 0; i < 3; i++) {
        if (i == 1) { continue; }
        defer println("leave ${i}");
        println("pass ${i}");
    }
}
```

```output
start 1
work 1
cleanup 1
end 1
start 2
cleanup 2
end 2
pass 0
leave 0
pass 2
leave 2
```

- **Several defers run in reverse order.**
- **A `defer` in a loop body runs at the end of each pass** that reached it. A `continue`
  before it skips it, since that pass never got to the `defer`.
- **A deferred statement can't `return`, `break` or `continue`**, and so can't write a
  status: "a deferred statement can't `return`, `break` or `continue`". A loop inside one may
  `break` out of itself.
- **A deferred call that can fail needs its status read or discarded**, as any call does:
  `defer @_ = f.close();`. Otherwise a failure stops the program when the block ends.
- **It doesn't run when the program stops** on a failure nothing read. It does run when
  `@@` abandons the call it is in ([Errors and statuses](07-errors.md)).
- **There is no conditional `defer`**: one inside an `if` runs when the `if`'s block ends.
  Cleanup that only one path needs is written at the end of the function instead.
- **`delete` is reserved**: declaring `delete()` is an error, "`delete` is reserved: there
  are no destructors; use `defer`".

Files are the usual case. A written file's `close` reports whether its buffered data could be
written, so it is checked; a file that was only read can be closed by a `defer`:

```java
import zink.io.*;

u8 main() {
    u8[..>] tmp = [];
    File out @cantCreate = fs.tempFile(tmp);
    if (cantCreate) { return 1; }
    defer @_ = fs.remove(tmp);              // runs last, however main ends

    @_ = out.println("first");
    @_ = out.println("second");
    u8 failed = @out.close();               // a written file's close is checked
    if (failed) { return 1; }

    File back @cantOpen = File.open(tmp);
    if (cantOpen) { return 1; }
    defer @_ = back.close();                // runs before the remove
    u8[.] text @_ = back.readAll();
    print(text);
    return 0;
}
```

```output
first
second
```

- **At exit, the runtime flushes and closes every file still open**, at normal exit and when
  the program stops, and reports any failure on stderr. A forgotten `close()` keeps a
  descriptor open until then, but never loses data silently.
- **A `File` is a handle**, a slot and a generation number, so copying one copies the
  handle, and closing it through one copy makes the others' operations fail with
  `BAD_HANDLE` ([The library: files, processes, time and the command line](13-library-system.md)).
- **Not yet**: the lint that warns about a local `File` that may still be open at the end of
  the scope that opened it
  ([DESIGN §6.4](../DESIGN.md#64-resources-and-cleanup-no-destructors-decided)).

## Roots

A class keeps its fields in one block, so it holds at most one growable field. A **root**,
declared with `root` in place of `class`, gives each growable field a data stack of its own,
so it can hold any number of them. It can have everything a class has: fields, methods,
constructors, `implements` and type parameters.

```java
root Tokens {
    string[..>] words;
    i64[..>] starts;

    mut void add(string w, i64 at) {
        words +> w;
        starts +> at;
    }
}

void main() {
    Tokens t = new Tokens();
    t.add("to", 0);
    t.add("be", 3);
    Tokens& r = t;
    println("${r.words.len} ${r.starts[1]} ${r.words[1]}");
}
```

```output
2 3 be
```

A root's value is a record of where each part lives, so it is never copied, passed or
returned by value, stored in a class or an array, or made as a temporary: `Tokens copy = t;`
is an error, "`Tokens` is a root, which can't be copied". It is passed by reference. What
roots are for, and how references into their parts are checked, is in
[Memory and references](05-memory.md#roots-classes-with-several-growable-arrays).

## Overloading

Methods, static functions and constructors can share a name when their parameter types
differ. The compiler picks one at compile time from the arguments' static types; the rules
for ranking the candidates are in [Functions and lambdas](08-functions.md).

```java
class Log {
    u8[..>] text;

    mut void add(string s) { text.append(s); text.append("\n"); }
    mut void add(i64 n) { add("${n}"); }
    mut void add(string key, i64 n) { add("${key}=${n}"); }
}

void main() {
    Log log = new Log();
    log.add("start");
    log.add(42);
    log.add("count", 3);
    print(log.text);
}
```

```output
start
42
count=3
```

- **Overloads must differ in a parameter's type.** Differing only in `&`, `?`, an array
  parameter's slots, the result type or the parameter names is an error.
- **Static types decide**: a `Shape&` argument calls the `Shape&` overload even when it
  refers to a `Circle`. Dynamic dispatch comes only from interface methods.
- **Interfaces can declare overloads**, and a class implements each one.
