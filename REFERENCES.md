# Zinkite Reference Rules (draft)

> Companion to `DESIGN.md` §3 and §5. This is the specification the compiler's safety checks are written against (DESIGN §14.3, step 1). It follows Goose's specification (v4 draft) wherever Zinkite has the same feature; §11 lists where it differs. Everything here is **Decided** unless marked otherwise.
> **Updated:** 2026-11-19. Revised to follow Goose on removal, variable-size values, data stacks and recursion, and to reuse cursor-array holes.

---

## 1. What has to be guaranteed

**Memory safety**: whenever a reference is used, it points to storage that
- (a) is still allocated, and
- (b) holds a value of the reference's type, laid out the way that type says.

The memory model gives part of this for free:
- **Storage never changes type while it is allocated.** An array of `T` only ever holds `T`s, and a field only ever holds its own type.
- **Nothing moves.** Growth never moves elements (DESIGN §3.1), and cursor arrays on a data stack keep each element in the slot it was inserted into (DESIGN §4.3). A capped cursor array's small gap buffer does move elements, which L3 accounts for (§7.4). That includes cursor arrays with a chunked index (`$`, DESIGN §4.1): only their index moves, so every rule here applies to them unchanged.

Five rules cover the rest:

| Rule | Guarantees | § |
|---|---|---|
| **S, roots** | No reference outlives the variable it points into | 3, 4 |
| **R, removal** | No reference into an array is used after elements are removed from it, so freed slots can be reused | 5 |
| **V, variable-size values** | Variable-size values never change after they are built | 6 |
| **L, links** | A link always lands inside the array it belongs to | 7 |
| **G, growth** | Each growable value is alone on top of its data stack, so growing never runs into another value | 8 |

There are **no aliasing or exclusivity rules**: two references to the same element can both read and write it. Without shared-memory threads, aliasing alone can't break type safety (Goose §9.1). Programs are single-threaded (threads are Open, §12).

---

## 2. Terms

- **Variable**: a local, a parameter, a static, or a **temporary** (an expression's result that isn't stored anywhere; it lives until the end of its statement).
- **Place**: anything that can be read or written: a variable, a field of a place (`p.f`), an element of an array place (`a[i]`), or the target of a reference.
- **Base variable** of a place: the variable its path starts from, as long as the path doesn't go through a reference. The base variable of `items[3].name` is `items`.
- **Lifetimes**:
  - a local lives from its declaration to the end of its block, and the locals of one block are released in reverse declaration order;
  - a parameter lives for the call;
  - a static lives for the whole run;
  - a temporary lives until the end of its statement.
- **Outlives**: variable A outlives B when A is released no earlier than B. That is the case when:
  - A is a static;
  - A is a local of a block that encloses B's block;
  - A and B are in the same block and A is declared first;
  - A belongs to a caller and B to a function it is calling.

  Every variable outlives itself.
- **Reference values**: `T&`, `T&?`, interface references (`Shape&`), slices (`T[:]`, including `string`, and writable views `T[:]&`), and links (§7). A **value that contains references** is a class value with reference fields, or an array of reference values, at any depth.
- **Removable array**: an array on a data stack with `<` or `~` in any slot, so elements can be taken out of it. **Grow-only arrays** (`T[..>]`, `T[>..]`, `T[>.>]`) and frozen arrays aren't removable. **Capped arrays** (a number or constant after the slots) are stored inline and have their own rule (§5.4).

---

## 3. Roots (rule S)

### 3.1 Root sets
Every reference value has a **root set**: the variables whose storage it may point into, plus `static` for static data such as string literals. For a root (DESIGN §6.6), whose parts are stored separately, the set names the part: `cache.nodes`, not just `cache` (§8.2).
- A value that contains references has the union of their root sets.
- A value with no references inside has an empty root set and can be stored anywhere.

### 3.2 How roots are computed

| Expression | Root set |
|---|---|
| a place whose path doesn't go through a reference | its base variable |
| `r.f`, `r[i]`, or a slice of `r`, where `r` is a reference | the roots of `r` |
| `a[:]`, `a[i:j]`, and an array converted to a slice | the roots of the place `a` |
| `a.push(x)`, `a.pushFront(x)`, `a.insert(x)` | the roots of the place `a` |
| a string literal | `static` |
| the references a parameter holds: a `T&` or slice parameter, or references inside a copied value | the roots of the argument (the caller's variables) |
| a call | from the function's summary (§4) |
| `c ? x : y` | the roots of `x` and of `y` |
| reading a variable, field or element that holds references | the root set of its variable (§3.5) |

### 3.3 The rule
**Rule S**: a value with root set R may be stored into a place with base variable V only if every root in R outlives V. Storing covers initialization, `=`, `.=`, `push`, `insert`, and passing the value to a function that stores it (§4). When the place is reached through a reference, the rule applies to each of that reference's roots instead of V.

What follows from it:
- **A function can't return a reference into its own locals.** A returned reference must be rooted in the caller's data, in a static, or in the function's own result as built in place (Goose §9.2).
- **A static can hold references only to statics** and to static data.
- **Within one block, a variable can hold references only into variables declared before it.**
- **A reference can't be bound to a temporary**: `Item& r = makeItem();` is an error, since the temporary dies at the end of the statement. Write `Item it = makeItem();` instead.
- **The same holds for interpolated text** (DESIGN §4.11): `string s = "${x}";` is an error, since the text is a temporary. The error suggests `u8[.] s = "${x}";`, which builds the text in `s`'s own storage.

### 3.4 Examples
```
Circle[..>] circles = [];
Shape&[..>] shapes = [];
shapes.push(circles.push(new Circle(1.0)));    // fine: circles outlives shapes

Shape&[..>] early = [];
Rect[..>] rects = [];
early.push(rects.push(new Rect(1.0, 2.0)));    // error: rects is declared after early,
                                               //   so it is released first
```

```
string longest(string a, string b) { return a.len >= b.len ? a : b; }

u8[..>] outer = [];
string w = outer;
{
    u8[..>] inner = [];
    inner.append("inner text");
    w = longest(outer, inner);         // error: the result may be rooted at inner,
}                                        //   which doesn't outlive w
```

### 3.5 Precision: per variable, not per point in the program
A variable's root set is the union of everything stored into it, or into any part of it, anywhere in its scope. So this is rejected, even though `w` is never used after the inner block:

```
string w = outer;
{
    u8[..>] inner = [];
    w = inner;               // error: inner doesn't outlive w
}
```

This keeps the first version simple. Goose is similar: a reference variable commits to the root of its first binding, and can be rebound only within that root or at the same scope depth (Goose §9.2). A check that follows the program's order could be added later, and it would only accept more programs (§12). The fix today is to declare the view inside the inner block.

---

## 4. Functions: summaries instead of annotations

### 4.1 Summaries
For every function, the compiler infers a **summary** from its body:
- **Result roots**: which parameters the result's references may be rooted at, through those parameters' own roots, and whether it may be rooted at `static`. A function with several results (DESIGN §9.5) has result roots for each one, and destructuring gives each new local the roots of its own result.
- **Store requirements**: pairs "references from parameter a are stored into storage reached through parameter b". Each needs the roots of a to outlive the roots of b, at every call.
- **Effects**:
  - which arrays, reached through `&` parameters or statics, it may remove from or rebuild (for rule R);
  - which it may grow (for rule G).

```
string longest(string a, string b) { return a.len >= b.len ? a : b; }
// summary: result rooted at a or b

void register(Shape&[..>]& all, Circle& c) { all.push(c); }
// summary: c must outlive all; grows all

void trim(i32[..~]& xs) { while (xs.len > 10) { @_ = xs ->; } }
// summary: removes from xs
```

### 4.2 Calls
- **The result's roots** are the roots of the arguments its summary names.
- **Each store requirement** is checked against the arguments' roots, by rule S.
- **The effects** apply to the arguments: calling `trim(nums)` is a removal from `nums` for rule R.

### 4.3 Recursion, interfaces and function values
- **Recursion**: summaries are sets of parameter positions and statics, so the compiler computes them by iterating over the call graph until nothing changes. This always terminates.
- **Interfaces**: a call through an interface reference uses the union of the summaries of every implementing method.
- **Function values**: a call through a `func` value uses the union over every function stored in a value of that type.
- The whole program is visible, so both of those sets are known.

### 4.4 No annotations
Nothing is written by the programmer. Error messages show the summary that caused them: "`longest(...)` may return a view rooted at its second argument, `inner`".

### 4.5 Compiled once
Each function is compiled once per array layout and generic instance (DESIGN §4.3, §10), never once per root, and the summaries are used only for checking. Goose instead compiles a function once per combination of roots, writability and destination stacks (Goose §7.7), and keeps summaries as well for removals and stores (Goose §5.1). A function that builds a value for its caller gets the destination stack as a hidden argument instead (§8.3).

---

## 5. Removal (rule R)

### 5.1 What counts as a removal
- `pop`, `popFront`, `removeBefore`, `removeAfter`, `removeAt` and `clear`;
- the operators that call them (`->`, `-<`, `-^`, `^-`);
- assigning a whole array, or rebuilding a variable-size value (§6);
- calling a function whose summary says it does any of these to an argument (§4.1).

### 5.2 The rule
**Rule R**: a removal from an array on a data stack is a compile error at any point where a reference into that array, or a slice of it, may still be used afterwards. The array may be removable, or grow-only with `clear` or whole assignment.

- **"May still be used" means liveness**: the check follows the program's order, through branches and loops. A loop body is checked in the state its earlier iterations leave it in (Goose §9.2).
- **The error names the reference and the line where it was bound.**
- **Using the reference only before the removal is fine**, and so is binding a new one after it.

```
i32[..~] stack = [];
stack +> 1 +> 2 +> 3;
i32& top = stack[^1];
@_ = stack ->;               // error: top (bound on line 3) is used on line 5
println("${top}");
```

```
i32& top = stack[^1];
println("${top}");
@_ = stack ->;               // fine: top isn't used after this
```

### 5.3 Where references into removable arrays can be held
- **Removable arrays**: references into one can be held only in local variables and parameters, never in fields, array elements or statics. The compiler can then see every reference that might still be used (Goose §5.2). A reference to a whole array, rather than into its elements, is different: it may be a field (§7.1).
- **Grow-only and frozen arrays**: references into them can be stored anywhere rule S allows. For these arrays, `clear` and whole assignment are the only removals. The compiler records which fields and arrays hold references into each array, so rule R sees those holders too (Goose §5.1).

### 5.4 Capped arrays and pools
- **Capped arrays** (a number or constant after the slots, such as `u8[..~16]`) are exempt from rule R. Their storage is inline in their parent and never holds another type, so a reference into one survives removals. It reads whatever value of the same type is in the slot: a logic bug, never memory corruption (Goose §5.3).
- **Pools** (`pool<T>`, DESIGN §3.5) work the same way. Freeing a slot only marks it reusable, so a stale reference may see the value that replaced it.

### 5.5 What rule R buys
After a removal, no reference into the freed slots can be used. So the slots are reused right away:
- a popped end slot by the next push;
- a cursor array's hole by the next insert (DESIGN §4.5).

The reuse is never visible to the program.

---

## 6. Variable-size values (rule V)

A value is **variable-size** when its size isn't fixed by its type: a frozen array with a runtime length (`T[.]`, `T[.*]`), or a class value with such a field.

- **V1, written once**: a variable-size value is written only when it is built. Its variable-size fields can't be assigned, and neither can the elements of an array of variable-size values. To change one, build a new value (Goose §4.4).
- **V2, whole variables**: a variable holding a variable-size value can be assigned as a whole. That rebuilds the value on its data stack, and counts as a removal for rule R: references into the old value must not be used afterwards.
- **V3, arrays of variable-size values**:
  - they can't be indexed (DESIGN §3.2);
  - they have no `pop` or `popFront`, since the start of the last element can't be found (Goose §3.3);
  - their cursor slot must be `.`, since a cursor array's holes are reused by elements of the same size;
  - `clear` works, under rule R.
- **V4, building in place**: while a value is being built in place in an array, nothing may grow that array (Goose §4.2). `items.push(make(items))` is an error when `make` can grow `items`.

```
class Order { i32 id; u8[.*] customer; }

Order o = new Order(1, "alice");
string name = o.customer;
o.customer = "bob";                // error: a variable-size field can't be assigned (V1)
o = new Order(2, "alexander");     // fine on its own: rebuilds o (V2) ...
println(name);                     // ... but this use makes it an error under rule R
```

---

## 7. Links inside data (rule L)

This settles DESIGN §5.4. There are three kinds of link.

### 7.1 References in fields
A class may have `T&`, `T&?`, interface-reference and slice fields. A value of that class carries their roots (§3.1), so rule S applies to it everywhere. By §5.3 and §5.4, they can point only into the arrays L3 lists (§7.4), into pools, or into variables that aren't arrays. An array may hold references rooted at itself, so this also links elements of one array:

```
class Node { i32 key; Node&? next; }

Node[..>] list = [];
Node& a = list.push(new Node(1, null));
a.next .= list.push(new Node(2, null));    // fine: rooted at list, stored in list
```

The costs:
- each reference takes 8 bytes;
- a copy of such a value still points at the original's storage;
- values with absolute references can't be saved as bytes.

**A field may also refer to a whole array**, as the library's writers do (`STDLIB.md` §1.3, rule 11): a `u8[..>]&` field, for example. Rule S applies as to any reference, so the array must outlive the object. Growing the array through the field is growing that array, which rule G checks as it would a direct call, and a removal through it, such as `clear` through a `u8[..~]&` field, is a removal from that array under rule R.

### 7.2 Self-relative links
`T&<i32>` stores a link as a signed byte offset from the field to its target, in either direction. `T&<i16>` and `T&<i8>` are 2- and 1-byte versions, and `T&<i32>?` uses offset 0 as null. This is Goose's self-relative reference, which Goose spells `T&<u32>`; its offsets are signed too (Goose §3.9).

```
class Node { i32 key; Node&<i32>? left; Node&<i32>? right; }   // 12 bytes

Node[..>] tree = [];
Node& root = tree.push(new Node(5, null, null));
root.left .= tree.push(new Node(3, null, null));
if (root.left != null) { println("${root.left.key}"); }   // 3: following a link is an add and a load
```

A structure built from self-relative links:
- is compact, with 12 bytes per node instead of 24;
- means the same thing wherever the whole array is copied;
- can be saved and loaded as bytes, after a check that every link lands on an element (Open, §12).

### 7.3 Pool links
`T&<u32 in arr>` stores an unsigned byte offset from the start of `arr`, plus one, so that 0 can mean null in `T&<u32 in arr>?`. `arr` must be a static grow-only array (Goose §3.9); the built-in `pool<T>` isn't one.

- A value whose links are all pool links can be copied anywhere, since its links don't depend on where the value sits.
- Following one is an add from the pool's start and a load.
- Typical uses: a graph whose nodes all live in one static array, and an index of references into an array of variable-size records (DESIGN §3.2).

### 7.4 The rules
- **L1, same array**:
  - a self-relative link can be stored only from a reference into the same array as the field it is stored in: the same root variable and field path (Goose §3.9);
  - a pool link only from a reference into its array;
  - when the compiler can't tell which array a reference points into, for example because two arrays were passed as parameters, it is a compile error.
- **L2, no copying out**: a value with self-relative links can't be copied out of its array. `Node n = tree[0];` is an error; use `Node& n = tree[0];`. The links are measured from where the value sits, so a copy elsewhere would point at the wrong memory. Copying a whole array is fine, and so is copying values whose links are all pool links.
- **L3, arrays that keep their elements in place**: links, and references in fields (§7.1), can only point into arrays whose elements never move and whose memory never holds another type. That means grow-only and frozen arrays, and capped arrays whose cursor slot is `.`: a removal from a capped array leaves its slots holding values of the same type (§5.4). A capped cursor array's small gap buffer moves elements, so it is excluded (DESIGN §4.3). References in fields may also point into pools (§5.4); links can't.
- **L4, width**:
  - storing a link whose offset doesn't fit its width stops the program, like an overflow (DESIGN §7.6), for example more than 127 bytes for `<i8>`;
  - no check is needed when the array's reservation is no larger than the width can reach, for example 2 GB for 32-bit links (Goose §3.9).

L1–L3 keep every link inside its array, and that memory never holds another type, so following a link needs no bounds check.

---

## 8. Data stacks (rule G)

### 8.1 The rule
**Rule G**: values on one data stack have nested lifetimes, and at most one of them is resizable. That one stays the topmost value on its stack for its whole life (Goose §1.3). A resizable value is a growable array, or a variable-size local that is rebuilt (§6, V2).
- Values with disjoint lifetimes reuse a stack.
- **Temporaries that need storage**, such as interpolated text passed to a `string` parameter (DESIGN §4.11), go on a scratch data stack. Each is written once at its exact size after its parts are evaluated, so none grows, and all are released at the end of their statement. A temporary built while another is being prepared, as in `log("${f("${x}")}")`, is finished first and sits below it.
- The compiler assigns stacks by walking the call graph from `main`, so the number of stacks is a compile-time constant (Goose §10.3).
- Each stack has one bump pointer. It moves up when a value is placed or grows, and back down when a scope ends.

### 8.2 Growable fields
A class has at most one growable field, and it must be the last field, so the value can grow at its end. A root (DESIGN §6.6), Goose's root struct, gives each growable field its own data stack instead. Its value is a record of where each part lives, so it is never copied, and the compiler assigns each root variable's stacks as it does a growable local's. A reference into one of its parts is rooted at that part, such as `cache.nodes`, and each part lives as long as the root variable. Rule R checks a removal from one part, such as `cache.index.remove(k)`, only against references into that part, so references into `cache.nodes` stay usable; and a pool part keeps the pool exemption (§5.4).

### 8.3 Calls and returns
- **Growing a parameter**: a function that grows a `T[..>]&` parameter grows it on the argument's stack. Its own resizable locals go on other stacks, which its summary records (§4.1).
- **Returning a value**: a function that returns a resizable or variable-size value builds it where the caller will keep it, on a stack the caller passes in as a hidden argument (DESIGN §3.3).

### 8.4 Recursion
A function in a recursive cycle can't call into the cycle while one of its resizable or variable-size locals is in scope, since that local would need a stack per activation (Goose §7.8).
- Growable data that has to outlive the recursive calls is passed in by reference from outside the cycle.
- A resizable value can't be passed by value into the cycle.

```
void collect(Dir& d, Item[..>]& out) {
    for (Dir& sub : d.subdirs) { collect(sub, out); }    // fine: out comes from outside the cycle
    out.push(summarize(d));
}

void collectEach(Dir& d, Item[..>]& out) {
    Item[..>] mine = [];
    for (Dir& sub : d.subdirs) {
        collectEach(sub, mine);    // error: mine is in scope during the recursive call
    }
    out.push(summarizeAll(mine));
}
```

Goose marks the entry to a recursive cycle with `recursive fn`. Zinkite finds cycles from the call graph instead, so nothing is written.

---

## 9. Other cases

- **Plain parameters**: they are read-only copies (DESIGN §5.1). The compiler passes a large one by pointer only when no `&` argument of the same call, and no reference the callee can reach, shares a root with it. Otherwise it copies.
- **Slices are read-only; writable views are `T[:]&`** (DESIGN §4.10). A writable view follows the same root rules as a slice, and can only be taken of an array the code may change, so never of a string literal, which lives in read-only memory. Goose instead tracks which views are writable from where they came from, with literals typed `const u8[:]` (Goose §3.10, §9.5). Zinkite writes it in the type, as plain and `&` parameters already say whether a callee may change an argument.
- **Interface references** follow the same rules as `T&`. The slot they point into keeps its class, so the method table stays right.
- **Lambdas** passed as parameters are compile-time values. A captured reference keeps its roots, and the lambda can't outlive the call. A `func` stored in a variable or field can't capture (DESIGN §9.3).
- **Hash maps** (DESIGN §4.14) follow the rules for removable arrays. Their entries never move on insert, so `put` and `getOrAdd` invalidate nothing. `remove` and `clear` are removals under rule R, and may compact the entries. References into a `hashmap` can be held only in local variables and parameters (§5.3). Each map is one region on one data stack under rule G: its entries, then its index (DESIGN §4.14).
- **Repeat loops** (`while (T x : call)`, DESIGN §9.4): the header's variables are declared afresh in the body on each pass, and the call is made before they exist. So when the call runs, no binding from the previous pass is live, and the call may remove from what that binding viewed: `stdin.nextLine(buf)` clears `buf` on every pass. Storing `line` in an outer array is still rejected, by §5.3: `buf` is removable, so a view into it can't be stored in an array.
- **For-each over an iterator** (DESIGN §9.4): the loop keeps the iterator in a hidden local for the whole loop. An iterator that holds a view or a reference into something, like `text.lines()`, keeps it live for the whole loop, so a removal from that data inside the loop is a compile error under rule R. A `mut T& next()` returns references, which follow rule S like any other.
- **For-each by reference** (`for (Item& it : items)`): the loop variable is a reference rooted at the array, so removing from that array inside the loop body is a compile error under rule R. For-each by value holds no reference; removing inside one is a compile error until DESIGN §17 decides whether to allow it.
- **`defer`** (DESIGN §6.4): for rules S and R, a deferred statement counts as running at the end of its block, before any of the block's variables are released. A reference it uses is therefore live until then, so a removal later in the block from the array it points into is a compile error.
- **`File` handles** (DESIGN §6.4) are plain values, a slot and a generation number, so copies and stale copies are safe: the runtime checks the generation on every operation.
- **Windows** (`T[<.<]`) made from another array are views with that array's roots.
- **Enums with payloads** (Open in DESIGN §17): if they are added, a value never changes variant in place, as in Goose (Goose §3.5), so references into a payload stay valid.

---

## 10. Accepted and rejected programs

**Roots**
1. Pushing references to elements of `circles` into a `shapes` array declared after it compiles; declared before it, it is an error naming `circles` (§3.4).
2. The `longest` example (§3.4) is an error naming `inner`; with both arguments rooted at `outer`, it compiles and `w` gets `outer`'s roots.
3. A function returning a reference to its own local is an error; returning one rooted at a `&` or slice parameter compiles.
4. `Item& r = makeItem();` is an error that suggests a plain local.
5. A static holding a view of a local is an error; a static holding a view of a string literal compiles.
6. `register(all, c)` (§4.1) with `c` declared after `all` is an error that shows the summary "c must outlive all".
7. A recursive function that returns its argument on one path and a static on another is rooted at both.

**Removal**
8. The `stack ->` example (§5.2) is an error naming `top` and line 3; moving the `println` before the pop makes it compile.
9. `trim(nums)` (§4.1) while a reference into `nums` is used afterwards is an error.
10. Storing a reference into a `T[..~]` in a field, array element or static is an error; storing one into a `T[..>]` compiles.
11. `clear()` on a `T[..>]` while a field elsewhere holds a reference into it that is used later is an error.
12. A reference into a capped array survives a pop and reads whatever value is in the slot.
13. After a pop, the next push reuses the slot; after a cursor-array removal, the next insert reuses the hole, and the data array never holds more slots than the most elements live at once.

**Variable-size values**
14. `o.customer = "bob";` is an error; assigning a whole `Order` to `o` compiles, and a view into the old `o.customer` used afterwards is an error (§6).
15. `pop` on an array of variable-size values, and a variable-size element type in an array with a cursor, are errors.
16. `items.push(make(items))` is an error when `make` can grow `items`.

**Links**
17. A `Node&<i32>` link between two elements of `tree` compiles; storing a reference into another array is an error (L1).
18. `Node n = tree[0];` is an error when `Node` has self-relative links; `Node& n = tree[0];` and copying the whole `tree` compile (L2).
19. A link into a removable array, or into a capped array with a cursor, is an error (L3).
20. A `Node&<i8>` link whose offset is more than 127 bytes stops the program with a message (L4).
21. A copied array of `Node`s with self-relative links works on its own after the original's scope ends.
22. A value whose links are all `T&<u32 in arr>` can be copied to a local and its links followed; an `arr` that isn't a static grow-only array is an error.

**Data stacks**
23. Two growable arrays that are live at the same time get different stacks; two with disjoint lifetimes share one.
24. A class with two growable fields, or a growable field that isn't last, is an error.
25. `collect` (§8.4) compiles; `collectEach` is an error naming `mine`.

**Other**
26. A plain parameter that shares a root with a `&` argument of the same call is copied, so writing through the `&` argument doesn't change what the callee sees in the plain one.
27. Writing through a `string` is an error.
28. Removing from an array inside a for-each over it by reference is an error.
29. A lambda passed to `filter` can capture a reference to a local; storing a capturing lambda in a `func` field is an error.
30. A reference used inside a `defer` is live until its block ends, so a later pop from the array it points into is an error.

---

## 11. Where this differs from Goose

| Topic | Goose | Here | Why |
|---|---|---|---|
| Checking functions | compiled once per combination of roots, writability and destination stacks | compiled once per layout and generic instance, never per root; summaries for checking, the destination stack as a hidden argument | smaller code and faster compiles; revisit if code generation needs constants |
| Roots of a reference variable | committed at its first binding | the union of everything stored into it | about as strict, and simpler to explain |
| Self-relative link spelling | `T&<u32>` (signed) | `T&<i32>` | the name says the offset is signed |
| Slices | writable or `const` depending on where they came from | read-only `T[:]`, and writable views `T[:]&` | the type says whether a callee may write, as for other parameters |
| Cursor arrays | none | data array plus index gap buffer, holes reused under rule R | a Zinkite addition (DESIGN §4.3) |
| Recursion | entries marked `recursive fn` | cycles found from the call graph | nothing to write |

---

## 12. Open questions

- **Order-following roots**: accept §3.5's example by tracking roots per point in the program. It would accept more programs without changing any.
- **Choosing a stack at run time**: Goose lists dynamic stacks as future work. They would let a recursive function keep growable locals.
- **Removal inside a for-each by value**.
- **Slices of cursor arrays**: their elements aren't contiguous in logical order (DESIGN §4.10).
- **Links between two parameters' arrays**: L1 rejects them whenever the compiler can't tell the arrays apart.
- **Saving and loading** values with links as bytes, with a check that rejects malformed links (Goose's `from_bytes`).
- **The host API**: references an embedding host holds into a script's data, and how long they stay valid.
- **Threads**: Goose gives each worker its own memory and passes flat values between them through queues, so no reference crosses a thread.
