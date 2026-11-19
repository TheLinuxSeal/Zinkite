# Zinkite Language Design Notes

> **Name:** Zinkite. The command is `zink` and source files end in `.zn`. Zinkite is a variant spelling of zincite, the zinc oxide mineral: Rust is iron oxide, and zinc is what keeps iron from rusting.
> **Updated:** 2026-11-15. This consolidates the original memory-management notes and the design discussion that followed. Ideas that were superseded or rejected are kept in Appendix A with the reasons.

**Status labels**
- **Decided**: settled.
- **Proposed**: recommended during the design discussion, not yet confirmed.
- **Open**: still to be decided (collected in §20).

---

## 1. Goals

- A small, statically typed language for **embedded scripts** and **Linux CLI utilities**.
- **Java-style classes and OOP**: single inheritance, interfaces, constructors, `static` members, exceptions, closures.
- **No manual memory management**, with **deterministic cleanup**: an object is freed on the line where its last reference goes away. No tracing collector, no cycle collector.
- **Memory safety**: no use-after-free, no dangling references. Problems are caught at compile time where possible, and never by reading freed memory.
- Small, fast-starting static binaries. Startup time and binary size matter more than peak throughput.

Left out to stay light: reflection, dynamic class loading, checked exceptions, inner-class capture rules, multiple inheritance, and operator overloading beyond a few basics.

---

## 2. Decision summary

| Area | Decision | Status | § |
|---|---|---|---|
| Typing | Static; every type is primitive or reference | Decided | 12 |
| Output | Native code; optional bytecode VM for embedding | Decided | 15, 16 |
| Implementation | Compiler in Rust, runtime in C, compiler emits machine code directly (not C) | Decided | 17 |
| Memory model | Reference counting with the heap kept acyclic | Decided | 3–5 |
| Cycles | Impossible by construction: a strong field can't lie on a cycle in the class graph. Checked at compile time; no runtime check, no `CycleError`, no opt-out | Decided | 5 |
| Moving owned objects | `take` and `replace`; element kinds `list<owned T>` and `list<weak T>` | Decided | 6.5, 12.4 |
| Fresh returns | `fresh Dog make()` promises a new object nothing else refers to; only fresh values go into owned slots, so ownership needs no runtime check | Decided | 7.4 |
| Static initialization | Every static field is set before `main`, in dependency order; initializers that depend on each other in a loop are a compile error | Decided | 11.7 |
| Constructors | Declared `new(...) { }`; a constructor sets its own fields before calling `super(...)`, so `this` is fully set whenever it is used | Decided | 8.2 |
| Destructors | `delete() { }`, called by the runtime when the object is freed, never by code. Every destructor in a freed owned tree runs before anything in it is released; destructors can't change that tree's owned fields, and a detached object can't be freed if its destructors read past it through `owner` (both checked at compile time) | Decided | 10.2 |
| Resource cleanup | Explicit `close()`; a resource freed while still open is closed by its destructor, with failures reported on stderr; no `with` statement | Decided | 10.1 |
| Borrow returns | `borrow Dog getRover()` returns something reached from `this` or a `borrow` parameter; the caller keeps those alive while using it | Decided | 7.5 |
| Stack references | Counted; the compiler removes most count updates | Decided | 4 |
| Value types | `struct`: a class with value semantics (copied on assignment, stored inline), with the same constructors, methods, interfaces and generics | Decided | 9 |
| Composition | `owned` fields: never counted, freed with their owner or when replaced; reassignable unless `final`; reads and writes checked at compile time where possible | Decided | 6.1 |
| Forever objects | `static owned` (replaces the earlier `permanent`); string literals are permanent | Decided | 6.3 |
| Back-pointers to owners | `owner` fields: never null, never stale | Decided | 6.2 |
| Borrows | Reaching an owned object keeps its owner alive | Decided | 7 |
| Parameter modes | Parameters are storable by default; `borrow` marks borrow-only parameters, which owned objects require; `fresh` parameters take ownership of a fresh object | Decided | 7.2 |
| Non-owning references | `weak`, read as nullable, checked by the compiler | Decided | 8.1 |
| Nullability | Non-null by default, `T?` for nullable; reference arrays are created with a fill function or as a `nullarray` | Decided | 8.2 |
| Program settings | One setting per program, as `#` directives at the top of the file containing `main` | Decided | 11.2 |
| Conditional compilation | `#if` / `#else` / `#endif` | Decided | 11.3 |
| Annotations | `@` attaches to declarations; none are defined yet | Decided | 11.1 |
| String interpolation | `"${expr}"`; `$` reserved for this | Decided | 12.2 |
| Object header | 16 bytes: class id (32 bits), flags (32 bits), count (64 bits) | Decided | 14.2 |
| Object references | 64-bit pointers behind one `Ref` type; 32-bit compressed pointers possible later if benchmarks justify them | Decided | 14.3 |
| Name | Zinkite; command `zink`, source files `.zn` | Decided | — |
| `zink run` | Compiles to a cached native binary, then `exec`s it | Decided | 17.4 |
| Grammar | `GRAMMAR.md`, version 1 | Proposed | `GRAMMAR.md` |
| Packages and imports | Java-style packages that match directories; `import a.b.C as D` for aliases | Decided | 11.4, 11.5 |
| Closures in owned fields | A closure stored in an `owned` field captures its owners as `owner` references, so callbacks don't create cycles | Decided | 6.4 |
| Script files | A file with no class declaration is allowed; its top-level methods and fields go into a hidden class | Decided | 11.6 |
| Lambdas and function types | `(a, b) -> a + b`; built-in function types written `func(int, int -> int)`; captures are read-only | Decided | 12.5 |
| Statements and expressions | Java's, plus `var`, for-each, arrow-only `switch`, simple enums | Decided | 12.6 |
| Integer overflow | Throws an error; `+%` wraps and `+|` saturates explicitly | Decided | 12.1 |
| Casts | No casts between reference types; downcasts use `instanceof` patterns, which can't fail | Decided | 12.1 |
| Index modes | `arr[i]` checked, `arr[%i]` wraps, `arr[|i]` clamps | Decided | 12.3 |
| Errors and exceptions | `Error` for bugs (arithmetic, index, argument, out of memory, stack overflow), `Exception` for expected failures; all catchable; no `throws` clauses | Decided | 12.7 |

---

## 3. Memory model overview

Every reference has one of these kinds:

| Kind | Example | Counted | Keeps target alive | Can be null | Can go stale | § |
|---|---|---|---|---|---|---|
| Strong | `Address home;` | yes | yes | only if declared `Node?` | no | 4 |
| Borrow | `borrow` parameters and returns, `Dog d = k.rover;` | its owner, and the object itself for non-final owned fields, briefly (often removed) | yes, through its owner | no | no | 7 |
| Owned | `owned Dog rover = new Dog();` | no | yes | no | no | 6.1 |
| Owner | `owner Kennel home;` | no | not needed | no | no | 6.2 |
| Weak | `weak Node prev;` | no | no | yes, compiler-checked | no: reads as null | 8.1 |
| Value | `struct Point { ... }` | stored inline | n/a | no | no | 9 |
| Permanent | `static owned Config cfg = ...;`, string literals | no | until exit | no | no | 6.3 |

Two invariants make this work:

1. **Strong and owned edges never form a cycle.** The compiler guarantees this from the class definitions (§5); nothing is checked at runtime.
2. **Every strong reference is counted, including locals** (§4), so a count of zero really means unreachable.

Together they mean plain reference counting frees every object as soon as it becomes garbage, on that line, with no collector. Weak and owner edges don't keep anything alive, so they are allowed to point "backward".

---

## 4. Strong references and counting

### 4.1 Counts
- Each object header has one 64-bit **`count`** (§14.2): the number of strong references to the object from anywhere (fields of other objects, array and list slots, locals, temporaries, closure captures), plus pins from borrows (§7.1). An ordinary object is freed when its count reaches zero.
- **Owned objects** are never counted by their owner, so their count only holds pins. An owned object is freed with its owner, or, once detached (§6.5), when its count reaches zero.
- Earlier versions split this into `heap_count` and `stack_count` so the runtime cycle check could skip stores (Appendix A.22). With cycles ruled out at compile time, one count is enough.
- Null is `0` and is never counted.
- Counts are **non-atomic**. The heap is single-threaded; if threads are added, use one heap per thread.
- **Counts can't wrap.** Reaching 2^64 would take 2^64 references held at once, more than a 64-bit address space can store, so increments need no overflow check.

### 4.2 Removing count updates
Counting locals naively would be slow. The compiler removes most updates:

```
Node n = makeNode();   // returned with +1, moved into n: no increment
use(n);                // parameters aren't counted on entry: no inc/dec
list.add(n);           // last use of n: moved, no inc/dec
}                      // n was moved: no decrement at scope exit
```

- **Move on last use**: the last use of a local transfers its reference instead of copying it.
- **Uncounted parameter passing**: the callee doesn't count its parameters; the caller guarantees they stay alive for the call. That is free when the argument is a local the caller doesn't reassign during the call. Otherwise (e.g. `f(global.items.get(0))`) the caller holds a temporary count for the duration. This calling convention applies to every parameter and is separate from the `borrow` parameter mode (§7.2): a callee that stores a storable parameter increments it then.
- **Returns transfer +1** to the caller.
- **Escape analysis**: objects that never leave a function live on the stack or in registers and are never counted:
  ```
  int dist(int ax, int ay, int bx, int by) {
      Point d = new Point(bx - ax, by - ay);   // never stored or returned
      return d.x * d.x + d.y * d.y;            // → two ints in registers
  }
  ```

How much of this the compiler achieves decides whether performance lands near Swift or well behind it (§18).

### 4.3 Freeing
- When an object's count reaches zero it is freed immediately: owned fields are freed directly, strong fields are decremented (which can cascade), weak and owner fields are ignored.
- **Order is specified.** Locals are released in reverse declaration order at scope exit. An object is freed together with the tree of objects it owns, in two phases:
  1. **Destructors.** Every `delete()` in the tree runs before anything is released. The object's own runs first (the subclass's, then the base class's), then each owned child's subtree in declaration order, parents before their children. Each object is flagged `dying` (§14.2) as its `delete()` starts.
  2. **Release.** For each object, its owned children are released first, in declaration order, then its other fields, in declaration order, and then its memory is returned.

  So every destructor sees the whole tree intact: `this`, its owned children, its owners up the chain through `owner` references (§6.2), and its siblings, whatever order the fields were declared in. Two compile-time rules keep it that way: destructors can't change the owned fields of a tree they may be part of, and a detached object can't be freed if its destructors read past it through `owner` (§10.2).
- **Other frees wait.** While a tree is being freed, any other object whose count reaches zero goes on the worklist and is freed after the tree is done, in the order it was queued. Its destructor never sees the tree half-released, and by then weak references into the tree read null.
- **Cost**: phase 1 is a second pass over the tree. The compiler skips it when no class that can be in the tree has a `delete()`.
- Cascades use an explicit worklist, not recursion.
- Dropping a huge structure frees all of it on that line. That is deterministic, but the pause is proportional to the structure's size, as in C++.
- **Exceptions**: unwinding must decrement the counted locals of every frame it passes through (§15.3).

### 4.4 Write barrier
Only stores into strong and owned reference fields go through the barrier. Primitive stores are plain stores.

```c
void write_strong_field(Obj *b, int idx, Obj *a) {        // b.field[idx] = a
    Obj *old = b->strong[idx];
    if (a != NULL && !is_permanent(a)) a->count++;        // increment first: a may equal old
    b->strong[idx] = a;
    if (old != NULL && !is_permanent(old) && --old->count == 0)
        free_object(old);
}
```

There is no cycle check: the class graph rules cycles out at compile time (§5). `is_permanent` costs nothing at sites where the compiler knows the declared type can't hold a permanent object (§6.3).

---

## 5. Cycle prevention (Decided)

### 5.1 The rule
The compiler builds a **class graph**: one node per class (after monomorphization), and an edge for every strong or owned field, from the class that has the field to every class its value could be. A program compiles only if **every cycle in the class graph consists of owned edges only**. A strong field that lies on a cycle is a compile error. `weak` and `owner` fields add no edges.

```
class Node { Node next; }
```
```
error: Node.next can form a reference cycle
    Node.next → Node
  A strong field can't be part of a cycle. Make it one of:
    owned Node? next;     each node owns the next one (a forward link)
    weak Node next;       it refers to a node owned somewhere else
```

Why this is enough:
- **Owned edges alone can't form a cycle at runtime.** An owned field only accepts a fresh object, one that nothing else refers to and that no borrow reaches into (§6.1). Owned edges therefore always form trees.
- **A runtime cycle containing a strong edge** would follow a cycle in the class graph containing that strong edge, which the rule forbids.

So the heap is acyclic by construction. No store is checked at runtime, nothing is ever walked, and there is no `CycleError`. There is no opt-out. This is Swift's rule (strong references must not form cycles), enforced before the program runs: Swift leaks such cycles silently.

### 5.2 Building the class graph
The compiler sees the whole program (§11.4), so the graph is exact for the program being compiled. It assumes the worst case wherever a field's type allows more than one class:

- **Class types**: an edge to the declared class and to every subclass of it in the program. **Interface types**: an edge to every implementing class. **`Object`**: an edge to every class, so a strong `Object` field is almost always rejected.
- **Generics** are checked after monomorphization (§13): `list<Dog>` has an edge to `Dog`, and each instantiation of `class Box<T> { T item; }` has its own edges.
- **Element kinds**: `list<owned Node>` makes owned edges, `list<weak Node>` makes none, and plain `list<Node>` makes strong ones (§12.4). Arrays work the same way.
- **Function types**: a strong `func(...)` field has an edge to every class captured strongly by any lambda that could be stored in it. The compiler tracks where each lambda can go; at worst, that is every lambda of that function type. Owner captures (§6.4) and weak captures add no edges.
- **Structs** are inlined, so their reference fields become edges of the class that contains them.
- **Strings and enums** add no edges: strings hold no references, and enum constants are permanent (§6.3).
- **Adding a class can break a distant field.** A new subclass in another file can close a cycle through an existing field. The error then names both the field and the subclass.

### 5.3 Common structures
```
class ListNode {                   // singly linked list
    int value;
    owned ListNode? next;          // forward: owned
}

class TreeNode {                   // trees, ASTs, JSON
    owned TreeNode? left;
    owned TreeNode? right;
    owner TreeNode parent;         // back: owner (never null, never stale)
}

class DNode {                      // doubly linked list
    owned DNode? next;
    weak DNode prev;               // back: weak
}

class Graph {                      // arbitrary links
    list<owned GNode> nodes;       // the graph owns every node
}
class GNode {
    list<weak GNode> edges;        // edges may point anywhere, including back
}

class Department {                 // a two-way association
    list<owned Employee> staff;
}
class Employee {
    owner Department dept;
}
```

If employees must exist independently of any department, `staff` becomes a plain `list<Employee>` and `dept` becomes `weak Department dept`.

### 5.4 Costs
- **No sharing inside a recursive structure.** A node can't be strongly referenced from two places in a structure of its own class, so DAGs, ropes and persistent data structures that share subtrees must copy, or keep their nodes in an owning list and link them with `weak` references.
- **Two-way links need a direction.** One side of every back-and-forth relationship is `owner` or `weak`. The error message shows the cycle and suggests which field to change.
- **Broad field types**: strong fields of type `Object` or of a widely implemented interface tend to be rejected. Generics, `owned` or `weak` usually fix this.
- **Tree algorithms move subtrees with `take`** (§6.5) instead of plain assignment.

In return, a strong store costs only its count update; there are no walks, so tree rotations stay O(1); the runtime has no cycle code; and whether a program can fail never depends on its input.

---

## 6. Owned fields, owner references, permanent objects

### 6.1 `owned` fields
Composition: the containing object owns the field's object, which lives until its owner dies or the field is given a new object. This started as `permanent`. Decided, including the keyword `owned`.

```
class Kennel {
    owned Dog rover = new Dog();        // freed with this Kennel, or when replaced; never counted
    final owned Bed bed = new Bed();    // never replaced
    list<Dog> visitors;                 // ordinary counted references
}

k.rover = new Dog();                    // the old Dog is freed on this line
```

Rules:
- **Reassignable unless `final`**, like any Java field. Reassigning frees the old object on that line, or, if a borrow is still using it, when that borrow ends. A `final owned` field is assigned once, in the initializer or the constructor.
- **Only a fresh object can be stored.** `new Dog()`, lambdas, `fresh` parameters (§7.2) and results of `fresh` functions (§7.4) always qualify. Other values are checked as described below.
- **Cycles through owned fields are ruled out at compile time** (§5). A `Dog` class with a strong field that can lead back to `Kennel` doesn't compile.
- **Never counted while owned.** Freeing the owner frees its owned objects directly, in field declaration order, before it releases its other fields (§4.3). Whatever an owned object holds is counted normally.
- **Reading an owned field gives a borrow** (§7). You can call methods on it, pass it to a borrowing parameter, or store it in a `weak` slot. Storing it in a strong slot is a compile error:
  ```
  k.rover.bark();              // fine
  walk(k.rover);               // fine: declared as void walk(borrow Dog d), see §7.2
  weak Dog w = k.rover;        // fine
  k.visitors.add(k.rover);     // compile error: owned objects can't go in strong slots
  ```
- **A borrow through non-final owned fields also pins the objects in them**: each object on the borrow's path that sits in a non-final owned field gets one count update. Usually that is just the borrowed object itself. Replacing or taking such a field (§6.5) then detaches the object instead of freeing it, and a detached object is freed, with everything it owns, when its last pin ends. The compiler removes these counts whenever it can prove the fields aren't written or taken while the borrow lives (below).
- **Header flags `owned` and `detached`**, used when pins end (above).
- **Structs can't contain `owned` fields**: copying a value would duplicate ownership.
- **Later optimization**: a `final owned` object dies exactly when its owner does, so the compiler could embed it inside the owner's block, as C++ does with member objects. `weak` references into an embedded object would then need the owner's id plus an offset. Non-final owned objects stay separate blocks.

**Compile-time checks on reads and writes.** Both follow the three outcomes of §7.3:

| Operation | Proven wrong: compile error | Proven safe: no runtime work | Unknown: runtime work |
|---|---|---|---|
| Write `x.f = v` | `v` is a borrow, a storable or `borrow` parameter, a field read, an owned object, or a local or `fresh` parameter used again after the store; `v` appears in the target (`x.child.f = x`) or a borrow into `v` or anything it owns is live; `f` is `final` and this isn't its initializer or constructor | `v` is `new ...`, a lambda, the result of a `fresh` function (§7.4), `take` or `replace`, or a local or `fresh` parameter at its last use that holds a fresh object | None: a call to a function that isn't `fresh` is a compile error, so every write is decided at compile time |
| Read `x.f` (a borrow) | Storing it in a strong slot, or passing it to a storable parameter | No write to `f` can happen while the borrow lives: no direct store, and no call that can reach one | The borrow pins the object as well as its owner |

```
k.rover.bark();          // bark() never writes Kennel.rover, directly or through calls: no pin
Dog d = k.rover;
k.rover = new Dog();     // warning: d still refers to the old Dog and is used below
d.bark();                // safe: the old Dog stays alive until d goes out of scope
Dog n = new Dog();
k.rover = n;             // compile error if n is used again after this line
button.onClick();        // the closure may replace onClick: pinned (§6.4)
```

- **Which functions can write a field** comes from a whole-program summary: for every function, the set of owned fields it can write, directly or through the functions it calls. Calls through interfaces, overridable methods and closures include every implementation the compiler can't rule out.
- **Using a borrow of the old object after reassigning its field** in the same function is safe, because the pin keeps the old object alive, but it gets a warning. It usually means the code expected the new object.
- **Borrows obtained through `weak` references** are the case the compiler can't always trace: it can't tell which object a weak reference points to. If one is live at a store and its class could be inside `v`'s owned subtree, the compiler assumes the worst and the store is a compile error.

### 6.2 `owner` references
A child owned by its parent can never outlive that parent. Any code that can reach the child either goes through the parent or holds a borrow that keeps the parent alive. So a reference back to the owner can never be null or stale, and it needs no count.

```
class Kennel {
    owned Dog rover = new Dog();
    string name = "Main St";
}

class Dog {
    owner Kennel home;          // set automatically; never null, never stale, never counted

    void bark() {
        print("Woof from ${home.name}");    // no null check needed
    }
}

Dog stray = new Dog();          // compile error: Dog has an owner field
```

Rules:
- **A class with an `owner` field can only live in owned slots**: it is created as the value of an `owned` field or `fresh` argument, in an owner compatible with the declared owner type. After `take` (§6.5) it can sit in a local until it is stored again. Storing it in a strong slot is a compile error: it would have no owner.
- **The runtime sets the field** when the object is assigned to the owned field. Code can't change it.
- **It can't be read while the object is detached**: in its constructor, where the owner is still being set up (Java calls this "this-escape"), and between `take` and the next store into an owned slot. The compiler checks this, including through method calls and through the destructors that run when a detached object is freed (§10.2).
- **Owner fields add no edges to the class graph** (§5).
- Prior art: Swift's `unowned` references are similar but checked at runtime. Here safety follows from the ownership rules.

This covers the most common use of back-pointers: parent pointers in trees, a component pointing back at its window, a part pointing back at its whole.

### 6.3 Permanent objects: `static owned`
A static field's owner is the program, so a `static owned` object lives until exit. This replaces the earlier `permanent` keyword.

```
public class Kennel {
    public static owned Dog rover = new Dog();   // one Dog for the whole program, never counted
}
```

- **Never counted, never freed.** Their destructors still run at normal exit (§10.2). A `new` initializer allocates into a bump region that is never freed or compacted. Any other initializer marks the object with a header bit, and its block is never freed.
- **Unlike instance-owned objects, permanent objects may be stored in strong slots**, because they never die. Those stores skip the count update.
- **Objects a permanent object references are counted normally**, so if it drops one, that one can be freed.
- **String literals are permanent** (and interned).
- **`static final` objects** may be treated as permanent silently by the compiler; nothing observable changes.
- **`weak` references to a permanent object never read as null.**
- **Cost only where needed**: the compiler sees the whole program, so only store sites whose declared type could hold a permanent object test the header bit.
- **Future**: with one heap per thread, permanent objects that are never modified could be shared between threads without counting.

### 6.4 Closures that capture their owner (Decided)
A closure is an ordinary heap object whose captured variables are its fields. Captures are strong by default: counted, and edges in the class graph (§5). On its own, that breaks the most common callback pattern:

```
class Window {
    owned Button ok = new Button();

    void init() {
        ok.onClick = () -> this.close();   // Window → Button → closure → Window
    }
}
```

With a strong capture, the class graph would contain Window → Button → closure → Window, a cycle with strong edges, so the program wouldn't compile. But the closure lives inside the Button, which the Window owns, so it can never outlive the Window. That is the same situation as an `owner` field (§6.2), and it gets the same treatment. There are two ways to set such a callback.

**At construction**, when the callback never changes (preferred):

```
class Button {
    final owned func(void -> void) onClick;

    new(fresh func(void -> void) onClick) {      // ownership moves in (§7.2)
        this.onClick = onClick;
    }

    void press() {
        onClick();
    }
}

class Window {
    owned Button ok = new Button(() -> this.close());
}
```

**By reassignment**, when the callback changes later:

```
class Button {
    owned func(void -> void) onClick = () -> {};   // starts as a no-op; reassignable

    void press() {
        onClick();
    }
}

class Window {
    owned Button ok = new Button();

    void init()          { ok.onClick = () -> this.close(); }
    void enterEditMode() { ok.onClick = () -> this.save(); }
}
```

Rules:
- **A closure stored in an `owned` field is owned** by the object holding the field, and is freed with it. The value must be a lambda written at the store, or a `fresh` parameter carrying one, so no other reference to it can exist.
- **Captures of the owner chain become `owner` references.** If a captured object is the field's object or one of its owners (the Button, the Window, the Window's owner, ...), the capture is uncounted, adds no edge to the class graph, and is never null or stale. The compiler works this out from where the closure ends up. Inside `Window`, `ok.onClick` is reached from `this` through owned fields only, so `this` is an owner. In `new Button(() -> this.close())`, it follows the lambda through Button's `fresh` parameter into `onClick`, and the new Button into `ok`. Other captures, and any the compiler can't prove are owners, stay strong and count as class-graph edges.
- **Prefer `final owned` with a constructor parameter** when the callback never changes. Borrows of it never need the extra count (§6.1), and the closure could later be stored inside the Button's own block.
- **A closure isn't run during construction.** In the first form, the lambda captures the Window before the Window has finished constructing. Creating the closure is fine; running it isn't, since `close()` could read fields that aren't set yet. So a constructor can only store a `fresh` closure parameter or pass it on, never call it, matching the rule that a constructor can't read its `owner` field (§6.2). The Window's own constructor can run it once all the Window's fields are set (definite initialization, §8.2).
- **Replacing a callback** is an ordinary reassignment of a non-final owned field (§6.1). The old closure is freed then, unless a borrow is still using it: a running closure that replaces its own field finishes safely and is freed when the call returns.
- **Closures in collections aren't covered.** List elements are strong slots, so `model.listeners.add(...)` can't use owner captures. Capture a `weak` local instead; no special syntax is needed:
  ```
  weak Window w = this;
  model.listeners.add(() -> {
      if (w != null) w.redraw();         // does nothing once the window is gone
  });
  ```
  For listeners this is usually the intended meaning anyway: a listener shouldn't keep its window alive (§8.1).

### 6.5 Moving owned objects: `take` and `replace` (Decided)
Owned objects move between owned slots without copying:

```
class ListNode {
    int value;
    owned ListNode? next;
}

// remove the node after n
ListNode? gone = take(n.next);     // n.next is now null; gone holds that node and the rest
if (gone != null) {
    n.next = take(gone.next);      // the rest moves back after n
}                                  // gone is freed here
```

- **`take(x.f)`** works on a nullable owned field or array slot (`owned T?`). It sets the slot to null and returns the object, **detached** from its owner and fresh. It counts as a write to the field (§6.1).
- **`replace(x.f, v)`** works on any owned field or slot: it stores the fresh `v` and returns the old object, detached.
- They are built-in operations rather than functions, because they act on a field, not on a value.
- **A detached object** can be stored in an owned slot, passed to a `fresh` parameter, returned from a `fresh` function (§7.4), or kept in a local while the code works on it. Otherwise it is freed, with everything it owns, when its local goes out of scope. If its class has no `owner` fields, it can also be stored in a strong slot and becomes an ordinary counted object.
- **While detached, its `owner` fields can't be read** (§6.2).
- Tree rotations and node removal are written with `take` and `replace`. Moving a subtree is a pointer move, whatever its size.

---

## 7. Borrows

A **borrow** is a temporary reference that can't be stored in a strong field: a `borrow` parameter (§7.2), the receiver `this` in a class used as an owned field, or a local read from an owned field (`Dog d = k.rover;`).

### 7.1 Borrowing an owned object keeps its owner alive
A borrow of an owned object holds a counted reference to its **owner**, or, if the owner is itself owned, to the nearest counted ancestor. A borrow through non-final owned fields may also pin the objects in them, so that reassigning or taking a field can't free them mid-use (§6.1).

```
Dog d = k.rover;      // keeps k's Kennel alive while d is in scope
k = null;             // Kennel not freed yet: d still holds it
d.bark();             // fine
}                     // d goes out of scope → Kennel freed → rover freed
```

This also protects methods running on the object:

```
Kennels.main.rover.bark();   // if bark() replaces Kennels.main, the old Kennel
                             // stays alive until bark() returns
```

- **No use-after-free**: an owned object can't die while anything is using it, including inside its own methods.
- **Free in the common case**: if the owner is held by a local that isn't reassigned while the borrow lives (as in `k.rover.bark()`), the compiler skips the extra count. A chain like `Kennels.main.rover.bark()` costs one count update on the Kennel.
- **Borrows can't be stored in strong fields**, so an owned object never gains a second owner.
- **Pins are released innermost first.** When a borrow ends, the borrowed object's pin goes before its owner's. An object whose free waited for the borrow (§6.1) is therefore freed while its owner is still alive, and its destructor can read `owner` fields (§10.2).
- Returning a borrow from a function: §7.5.

### 7.2 Parameter modes (Decided)
A callee has to know whether it may store a parameter, because an owned object must never end up in a strong slot, and whether it receives ownership of one. Parameters therefore have three modes, checked at compile time:

```
void adopt(Dog d) {               // storable (the default)
    shelter.dogs.add(d);          // fine
}

void walk(borrow Dog d) {         // borrow-only
    d.bark();                     // fine
    log(d);                       // fine if log's parameter is also borrow
    weak Dog w = d;               // fine
    shelter.dogs.add(d);          // compile error: a borrow parameter can't be stored strongly
}

walk(k.rover);                    // fine
adopt(k.rover);                   // compile error: owned object passed to a storable parameter
adopt(new Dog());                 // fine
walk(new Dog());                  // fine: any reference can be borrowed
```

```
class Kennel {
    owned Dog rover;

    new(fresh Dog first) {        // fresh: ownership moves into the call
        rover = first;            // fine: stored in an owned field at its last use
    }
}

new Kennel(new Dog());            // fine: a fresh object
Dog d = new Dog();
new Kennel(d);                    // fine if this is d's last use; compile error otherwise
new Kennel(k.rover);              // compile error: k.rover already has an owner
```

Rules:
- **Storable is the default.** That matches Java-style code, where setters and `add` store their arguments.
- **A `borrow` parameter** can be used, passed to other `borrow` parameters, and stored in `weak` slots. It can't be stored in a strong field, returned as a strong reference, or passed to a storable parameter.
- **Owned objects and other borrows can only be passed to `borrow` parameters.** Ordinary strong references can be passed to storable or `borrow` parameters.
- **A `fresh` parameter takes ownership of a fresh object.** The argument must be `new ...`, a lambda, a local at its last use, or another `fresh` parameter at its last use. A call result qualifies only if the function is declared `fresh` (§7.4). Anything else is a compile error.
- **Inside the callee, a `fresh` parameter** can be borrowed: used, passed to `borrow` parameters, stored in `weak` slots. At its last use it can be stored in an owned field or passed to another `fresh` parameter. It can't be stored in a strong slot or passed to a storable parameter. If it is neither stored nor passed on, it is freed when the call returns.
- **In a constructor**, a `fresh` closure parameter can only be stored or passed on, not called (§6.4).
- **`this` in a class used as an owned field** follows the same rules as a `borrow` parameter: storing it strongly, or passing it to a storable parameter, is a compile error. The compiler sees the whole program, so it knows which classes are ever owned. The error names the `owned` field that made the class owned, since that field may be in another file.
- **Overrides**: an override may change a storable parameter to `borrow` (it then accepts more), but not the reverse, so calls through a base class or interface stay checkable. `fresh` parameters stay `fresh`, since the caller hands over ownership.
- **Library guidance**: functions that only read an argument declare it `borrow` (e.g. `print(borrow Object o)`, `list<T>.contains(borrow T item)`), so they accept owned objects. The compiler can suggest `borrow` for parameters that are never stored.
- **The caller's pin covers the call**: when the argument is a borrow of an owned object, the caller keeps the owner alive (§7.1) for the duration of the call.
- Modes exist only at compile time. All three use the same uncounted calling convention (§4.2).

### 7.3 Compile-time analysis: three outcomes
There is no borrow checker. For each analyzed use, the compiler reaches one of three outcomes:

| Outcome | Result |
|---|---|
| Proven wrong | Compile error |
| Proven safe | Runtime work (count update, check) removed |
| Unknown | Runtime work stays |

The analyses are local to one function, plus whole-program summaries such as which functions can write each owned field (§6.1). There are no lifetime annotations, no `&mut` split, and no rejected safe code. Anything the compiler later learns to prove turns runtime work into removed work or compile errors, without changing what valid programs mean. Vale takes a similar approach to its generation checks.

### 7.4 Fresh returns (Decided)
A function can promise that it returns a new object that nothing else refers to:

```
fresh Dog makeDog(string name) {
    Dog d = new Dog();
    d.name = name;
    return d;                  // fine: created here and never shared
}

fresh Dog bad1() {
    return this.favorite;      // compile error: favorite is shared
}

fresh Dog bad2() {
    Dog d = new Dog();
    registry.add(d);           // d may now be stored in the registry
    return d;                  // compile error: no longer fresh
}

k.rover = makeDog("Rex");      // fine, with no runtime check
Dog d = makeDog("Rex");
k.rover = d;                   // fine if this is d's last use
dogs.add(makeDog("Rex"));      // fine: a fresh object can become an ordinary shared one
k.rover = getFavorite();       // compile error: an ordinary return may be shared
```

- **Every `return` in a `fresh` function must give a fresh value**: `new ...`, a lambda, a local that was never shared, a `fresh` parameter, the result of `take`, `replace` or `removeAt`, or the result of another `fresh` function.
- **Only fresh values go into owned slots** (owned fields, `fresh` parameters, owned list elements). A result from a function that isn't `fresh` is a compile error there, so ownership never needs a runtime check.
- **`new` is a built-in fresh return**, which is why `k.rover = new Dog()` works.
- **Overrides** may change an ordinary return to `fresh`, since a fresh object is fine where a shared one is expected, but not the reverse, since callers may rely on the promise.
- **Interfaces and function types can say it too**: `interface Factory { fresh Widget create(); }`, `func(void -> fresh Dog)`.
- **A returned object of a class with `owner` fields is detached** (§6.5), so the caller must put it in an owned slot.
- **It is part of the signature, not inferred.** The compiler could work it out for each function, but then editing a function's body could silently break callers in other files, and calls through interfaces can't be inferred at all.
- No runtime cost: the object is returned the usual way (§4.2).

### 7.5 Borrow returns (Decided)
A function can return a borrow, which is how getters hand out owned fields:

```
class Kennel {
    private owned Dog rover = new Dog();

    borrow Dog getRover() { return rover; }
}

k.getRover().bark();               // fine
walk(k.getRover());                // fine: walk takes a borrow parameter
weak Dog w = k.getRover();         // fine
Dog d = k.getRover();              // fine: d is a borrow and keeps k's Kennel alive while in use
k.visitors.add(k.getRover());      // compile error: a borrow can't be stored strongly
```

- **The caller gets exactly what reading the field directly gives** (§7.1): it can use the result, pass it to `borrow` parameters and store it in `weak` slots, but not store it strongly.
- **A `borrow` function may only return something reached from `this` or from one of its `borrow` parameters**, so that it stays alive after the function returns:
  ```
  borrow Dog bad() {
      Dog d = new Dog();
      return d;                    // compile error: d dies when the function returns
  }

  static borrow Dog firstDog(borrow Kennel a, borrow Kennel b) {
      return a.rover;              // fine
  }
  ```
- **The caller keeps the sources alive** while it uses the result: the receiver for a method, and for a function with several `borrow` parameters, all of them (the worst case). The pins are the same as for reading the field directly, and are usually removed. No lifetime annotations are needed.
- **Not only for owned fields**: `borrow Address getAddress()` is a read-only getter for an ordinary field that skips the count update.
- **Overrides** may return something more permissive than promised: a `borrow` return can be overridden with an ordinary or `fresh` one, but not the reverse.
- **Function types**: `func(borrow Kennel -> borrow Dog)`.

Parameters and returns now have the same three modes:

| Mode | Parameter | Return |
|---|---|---|
| ordinary | may be stored by the callee | the caller gets a shared reference |
| `borrow` | used only during the call | the caller can use it but not store it |
| `fresh` | ownership moves into the callee | the caller gets a new object nothing else holds |

---

## 8. Weak references and nullability

### 8.1 `weak`
```
weak Dog w = k.rover;  // doesn't keep anything alive
k = null;              // Kennel freed → w now reads as null
w.bark();              // compile error: w may be null
if (w != null) {
    w.bark();          // fine; the call keeps the target alive while it runs
}
```

- Not counted. Reading gives a nullable type (`Dog?`), and using it requires a null check the compiler can see.
- **Declared without `?`**: `weak Dog w`, never `weak Dog? w`, since a weak reference is always nullable when read. The same goes for element kinds: `list<weak Dog>`.
- A call through a weak reference keeps the target (or its counted owner) alive for the call's duration, like a borrow.
- **Implementation (Proposed)**: a side table, as Swift uses. The first time a weak reference to an object is taken, the object gets a slot holding its pointer and a generation, and its header records the slot index. A weak reference stores the slot index plus the generation. Freeing the object bumps the slot's generation, so every weak reference to it reads as null from then on. Slots are reused, except that a slot whose generation is about to wrap around is retired instead, so a stale weak reference can never match a new object.

`weak` is the only place null can appear without being declared. Use it only for relationships where the target can genuinely disappear first:
- a listener on a window the user may close
- a "currently selected item" that may be deleted
- a cache entry whose object is freed elsewhere
- `prev` at the head of a doubly linked list

In those cases null is the true state of the program ("the thing you were watching is gone"); the compiler makes sure you can't forget it. Alternatives:
- For back-pointers to an owner, use `owner` (§6.2), which can't be null.
- Let the target own the relationship: a window owns its listener list and drops the listeners when it closes.
- Use a strong reference where the class graph allows it (§5), if keeping the target alive is acceptable.

### 8.2 Nullability (Decided)
- References are **non-null by default**. `T?` means "a `T`, or null". The compiler tracks null checks through control flow (`if (x != null) ...`), as Kotlin, Swift and C# do:
  ```
  Dog a = null;          // compile error: a Dog can't be null
  Dog? b = null;         // fine
  b.bark();              // compile error: b may be null
  if (b != null) {
      b.bark();          // fine
  }
  ```
- This removes `NullPointerException`s generally, not just for `weak`.
- **Null checks on fields count too**, as long as the compiler can prove the field isn't written between the check and the use, directly or through calls (the same write summaries as §6.1). Otherwise, copy the field into a local first.

**Creating arrays.** A non-null slot can't start out empty, so reference arrays have two creation forms:

```
array<Dog>  dogs  = new array<Dog>(10, i -> new Dog());   // each slot filled by the function
array<Dog?> slots = new nullarray<Dog?>(10);              // each slot starts as null
array<int>  nums  = new array<int>(10);                   // primitives: zero-filled
```

- **`new array<T>(len, func(int -> T))`** calls the function once per index, in order. It works for any element type. For rank 2 and 3 the function takes one index per dimension: `new array<Dog, 2>(rows, cols, (i, j) -> new Dog())`. If the function throws partway, the elements created so far are released and the array is freed.
- **`new nullarray<T?>(len)`** creates an ordinary `array<T?>` with every slot null. `nullarray` is only a creation form, not a separate type, so the result can be passed anywhere an `array<T?>` is expected. The element type must be nullable: `new nullarray<Dog>(10)` is a compile error. Rank 2 and 3 work the same way: `new nullarray<Dog?, 2>(rows, cols)`.
- **`new array<T>(len)` without a function** works only for primitive elements and for structs with no non-null reference fields, which are zero-filled. For anything else it is a compile error that suggests the two forms above.

**Constructors (definite initialization).** A constructor must assign every non-null reference field before `this` is used: calling its methods, passing it anywhere, or storing it. Creating a lambda that captures `this` is allowed earlier, but the lambda can't run until then (§6.4).

**Constructors are declared `new(...) { }`** and called as `new Dog(...)`, so declaration and use read the same; `delete() { }` is the matching destructor (§10.2).

**With inheritance, a constructor sets its own fields before calling `super(...)`** (Swift's order; Java 25's flexible constructor bodies allow the same). Java calls `super(...)` first, so a base constructor that calls an overridden method sees the subclass's fields still unset. Here every field of the whole object is set before any constructor uses `this`:

```
class Animal {
    string name;

    new(string name) {
        this.name = name;
        describe();               // fine: Dog's fields are already set
    }

    void describe() { print(name); }
}

class Dog extends Animal {
    string breed;

    new(string name, string breed) {
        this.breed = breed;       // own fields first
        super(name);              // then the base constructor
    }

    void describe() { print("${name}, ${breed}"); }
}
```

- Field initializers (`string breed = "mutt";`) run first, then the constructor body.
- Before `super(...)`, a constructor may assign its own fields and compute the arguments for `super`, but not otherwise use `this`. That is a compile error.
- Without an explicit `super(...)`, the base class's no-argument constructor is called implicitly once the class's own fields are assigned, before `this` is first used.

**Strings.** `string` is non-null, and `""` is the empty string. `string?` is for values that may be missing; for example, looking up an environment variable returns `string?`.
---

## 9. Structs (value types)

A `struct` is a class with value semantics. It is declared like a class and has constructors, methods, interfaces and generics, but its values are copied on assignment and stored inline. It is the same idea as Swift's structs and Java Project Valhalla's value classes; only the keyword differs.

```
struct Point {
    int x;
    int y;
    new(int x, int y) { this.x = x; this.y = y; }
    int lengthSquared() { return x * x + y * y; }
}

array<Point> pts = new array<Point>(1000);   // one 8,000-byte block, zero-filled
pts[3].x = 5;                                // updates the element in place
Point p = pts[3];                            // copies 8 bytes
p.x = 7;                                     // pts[3].x is still 5
```

Rules:
- **Assignment copies.** Fields stay mutable, but writes change only that copy; `pts[3].x = 5` writes straight into the array. These are Swift's struct semantics. (Valhalla made its value classes immutable, mainly for Java compatibility.)
- **Implicitly final**: no subclasses, so the size is always known.
- **No identity**: `==` compares fields. A value can't be `weak` and can't be a node in a cycle.
- **Not nullable**: the default value is all fields zero, which matches zero-filled arrays.
- **Boxed only through an interface**: using a `Point` as `Comparable` copies it to the heap. Generics are monomorphized, so `list<Point>` stores points inline.
- **No `owned` or `owner` fields** (copying would duplicate ownership).
- Constructors, methods, interfaces and generics work as for classes.

**Why plain classes can't be inlined automatically**: they have identity. After `Point p = pts[0]; p.x = 5;`, a reference type must show the change in `pts[0]`, which would need pointers into the middle of the array's block. Null slots and subclasses of different sizes also get in the way. Escape analysis (§4.2) still removes heap allocation for non-escaping locals of any class.

**Layout**: a value's references go into the containing object's matching reference section, and its primitives into the prim section. Arrays of values split the same way, giving a column layout:

```
struct Entry { string key; int count; }

array<Entry> (n elements):
[ header | key0 key1 ... key(n-1) | count0 count1 ... count(n-1) ]
           ^ strong refs: n refs     ^ prims: n ints
```

The free path still only loops over reference sections. Copying a value that contains references increments their counts, and storing it into a field runs the barrier for each reference field.

**Later**: with `readonly` fields, the compiler could inline final classes whose fields are all `readonly` and that never use identity `==`, `weak` or null. When nothing can change, copying and sharing look the same to the program.

---

## 10. Resource cleanup and destructors (Decided)

### 10.1 Files and other resources
Resources are closed with an explicit `close()`:

```
File out = File.create(path);
out.write(data);
out.close();          // throws IOException if buffered data can't be written
```

- **`close()` throws `IOException` on failure**, so code that writes knows whether its data was saved. Using a closed resource throws `IOException`.
- **Backstop**: a resource freed while still open is closed by its destructor (§10.2). A destructor can't throw, so a failure there is reported on stderr: data loss is never silent. Read-only files can simply rely on this.
- **Locals are released at the end of their block** (or when reassigned), never earlier at their last use, so the point where a forgotten resource is closed is visible in the code. This includes unwinding after an exception.
- **No `with` statement and no `Closeable` interface.** Counting already frees every object at a precise point (§4.3), so explicit `close()` plus the destructor backstop covers what `with` did (Appendix A.23).
- **No `finalize()`.**

### 10.2 Destructors: `delete()`
```
class BufferedLog {
    File file;
    StringBuilder buffer = new StringBuilder();

    new(string path) {
        file = File.create(path);
    }

    void log(string line) {
        buffer.append(line);
    }

    delete() {
        file.write(buffer.toString());   // runs when this object is freed
        file.close();
    }
}
```

- **Called by the runtime, never by code.** `x.delete()` is a compile error. This is not manual memory management (Appendix A.13).
- **When it runs**: exactly once, when the object is freed: its last reference goes away, its owner is freed, or a detached object's last pin ends (§6.5).
- **Order** (§4.3): an object is freed with the whole tree it owns. Every `delete()` in that tree runs first: the subclass's before the base class's, parents before their children, siblings in declaration order. Only then is anything released.
  - Inside `delete()`, everything the destructor can reach in the tree is intact: the fields of `this`, its owned children, its owners up the chain through `owner` references (§6.2), and its siblings:
    ```
    class Kennel {
        string name = "Main St";
        owned Dog rover = new Dog();
        owned Dog spot = new Dog();
    }
    class Dog {
        owner Kennel home;
        delete() {
            print("Bye from ${home.name}");       // safe: nothing is released yet
            print("Rover was ${home.rover.age}"); // safe, even inside spot's delete()
        }
    }
    ```
- **Destructors can't reshape an owned tree.** If a `delete()`, or anything it can call, can write an owned field (assignment, `take`, `replace`, or adding to or removing from a `list<owned T>`) of a class that can be in the same owned tree as the destructor's class, it is a compile error. The compiler uses the class graph (§5.2) and the summary of which functions write which owned fields (§6.1), and assumes the worst case. Strong and weak fields can still be written.
  ```
  class Kennel { list<owned Dog> dogs; }
  class Dog {
      owner Kennel home;
      delete() { home.dogs.remove(this); }   // compile error: changes Kennel.dogs,
  }                                          //   and that Kennel may be being freed
  ```
- **A detached object can't be freed if its destructors read past it.** A detached object (§6.5) can outlive its old owner, so its `owner` fields can't be read (§6.2). The compiler treats every place a detached object can be freed (its local going out of scope or being reassigned, or a `fresh` parameter that isn't stored) as a call to every `delete()` in its tree, assuming the worst case for subclasses. If one of them can read an `owner` reference that leads past the detached object, it is a compile error:
  ```
  Dog? d = take(k.rover);
  if (d != null) { d.train(); }
  }      // compile error: d would be freed here, and Dog's delete() reads home,
         //   which d no longer has. Store it in an owned slot instead.
  ```
  - A child's destructor reading `home` when `home` is the detached object itself is fine: only references leading past it are rejected.
  - An object freed because its field was reassigned, or by a discarded `take` or `replace`, is freed while its owner is still alive, so the rule doesn't apply. If a borrow delays that free, the borrow's pins are released innermost first (§7.1), so the object is still freed before its owner.
- **No resurrection**: inside `delete()`, `this` is a borrow (§7). It can't be stored strongly, passed to a storable parameter, or captured by a lambda that outlives the call. Weak references to the object already read as null.
- **Exceptions don't escape**: one thrown out of `delete()` is reported on stderr and discarded, and freeing continues. Every function may throw, so this can't be checked at compile time.
- **Consistent heap**: the runtime frees an object only after the operation that released it has finished (for example, after a field store has written its new value), so `delete()` never sees a half-done update.
- **At normal exit**, when `main` returns or after an uncaught exception has been reported, static fields are released in reverse order of initialization (§11.7) and permanent objects' destructors run, so buffered data is written. Their memory is simply returned with the process.
- At most one per class, with no parameters and no return value. Structs can't have one, since every copy would run it.
- **Cost**: nothing for classes without `delete()`. The class descriptor has a destructor entry that the free path checks.

---

## 11. Program structure: directives, packages and imports

### 11.1 Syntax roles
- **`#`** means "decided by the compiler": settings directives and conditional compilation. Every `#` line is resolved before code generation.
- **`@`** attaches to a declaration. Annotations are compiler-only (no reflection) and come from a small fixed set. None are defined yet: the earlier `@acyclic` became the rule for every class (§5).
- **Runtime operations are ordinary functions**, never sigils.
- **`$`** is reserved for string interpolation. **`%`** stays the modulo operator.

### 11.2 Program-wide settings (Decided)
Settings apply to the whole program and are written as directives at the top of the file that contains the entry point (`static main`, or a top-level `main` in a script file, §11.6):

```
#!/usr/bin/env zink
#leakcheck ON
#max_heap_mb 256

public class Main {
    public static void main(array<string> args) { ... }
}
```

Rules:
- Allowed only in the file containing the entry point (identified by its `main`, not by file name). Anywhere else they are a compile error, so libraries can't change program behavior.
- They come after the shebang and before any declaration.
- Unknown directives are errors, so a typo is never silently ignored.
- Each setting may appear once, after `#if` is resolved.
- A setting that doesn't apply produces a warning.
- **Precedence: host API > build flag (e.g. `--max_heap_mb=512`) > directive > default.**
- Embedded scripts usually have no `main`, so the host sets these through the C API. If a loaded script has its own directives, the host's settings win and the compiler warns.

Setting names (Proposed; they replace the earlier `#gc_*` set, see Appendix A.6):

| Directive | Values | Default | Meaning |
|---|---|---|---|
| `#leakcheck` | `ON`, `OFF` | `OFF` | At exit, report every object still allocated other than permanent ones, with class names |
| `#max_heap_mb` | integer | none | Heap ceiling; exceeding it raises an out-of-memory error |

### 11.3 `#if` (Decided)
```
#!/usr/bin/env zink
#if debug
#leakcheck ON
#endif

public class Main {
    public static void main(array<string> args) {
        #if debug
        log("args: ${args.length}");
        #endif
        ...
    }
}
```

- Conditions use only compile-time values: build flags (`debug`, `--define color`), the target (`os(linux)`, `arch(aarch64)`), and settings (`leakcheck(on)`).
- No `#define` and no text substitution, so `#if` can't turn into the C preprocessor.
- Each branch must be complete statements or declarations, and both branches must parse. Only the active branch is type-checked, so typos are caught while platform-specific APIs in the inactive branch don't cause errors (Swift's approach).
- `#if` may wrap settings directives, which gives per-build settings for free.
- The lexer accepts `#!` on line 1 so scripts can be run directly.

### 11.4 Packages and files (Decided)
Java's model: a package is a directory, and each file's `package` line must match its path.

```
src/
  com/example/app/Main.zn        package com.example.app;
  com/example/util/MyArray.zn    package com.example.util;
```

- **File order**: shebang, directives (entry file only, §11.2), `package`, imports, declarations.
- **One public top-level class per file**, named after the file: `MyArray.zn` holds `public class MyArray`. A file may also hold non-public classes, visible only inside its package.
- **No `package` line** puts the file in the unnamed package. That is meant for single-file scripts; as in Java, named packages can't import from it.
- **The source root is inferred from the entry file**: `zink run src/com/example/app/Main.zn` containing `package com.example.app;` gives the root `src/`. A `package` line that doesn't match the path is an error. Extra roots for libraries come from a `--source-path` flag (Proposed).
- **Visibility** is Java's: `public`, `protected`, `private`, and package-private when nothing is written.
- **`zink.*` is reserved** for the standard library. `zink.lang` is imported into every file automatically (`print`, the exception classes and the other built-ins).
- **Circular dependencies between packages are allowed.** The compiler sees the whole program at once, which monomorphization needs anyway (§13).

### 11.5 Imports (Decided; details Proposed)
```
package com.example.app;

import com.example.util.MyArray;              // usable as MyArray
import com.example.MyArray as ExampleArray;   // usable as ExampleArray only
import com.example.io.*;                      // every public class in com.example.io

public class Main {
    public static void main(array<string> args) {
        MyArray a = new MyArray();
        ExampleArray b = new ExampleArray();
        com.example.other.Thing t = new com.example.other.Thing();   // full names always work
    }
}
```

Rules:
- **`as` renames a class for this file only.** The original simple name isn't imported: after `import com.example.MyArray as ExampleArray;`, a bare `MyArray` means whatever it would mean without that line.
- **Two imports can't bring in the same name.** `import com.example.util.MyArray;` together with `import com.example.MyArray;` is an error, and the message suggests `as`. An import that clashes with a class declared in the same file is also an error.
- **Wildcards work as in Java**: an explicit import or alias wins over a wildcard, and a name provided by two wildcards is an error only where it's used.
- **Only public classes can be imported** from another package.
- **Unused imports** are a warning.
- Static imports and aliases for whole packages are Open (§20).

### 11.6 Script files (Decided)
A file doesn't need a class. A file whose top level has methods and fields but no class declaration is a **script file**, as in Java 25's compact source files:

```
#!/usr/bin/env zink
#max_heap_mb 64

int total = 0;

void main(array<string> args) {
    total = args.length;
    print(shout("got ${total} arguments"));
}

string shout(string s) {
    return s.toUpperCase() + "!";
}
```

- **The compiler wraps the file in a hidden final class.** Its top-level methods and fields become `static` members of that class, so no hidden object is created and they work exactly like static members everywhere else.
- **Entry point**: `void main()` or `void main(array<string> args)` at the top level. Directives (§11.2) go at the top of the file as usual.
- **A script file can also declare classes**, which are visible only inside that file.
- **It is in the unnamed package and has no `package` line.** It can import other packages, but nothing can import it. Once code is worth sharing, it moves into a class in a package.
- Internally everything is still a class, so the type checker, the layout rules and the backends need no special case.

### 11.7 Static initialization (Decided)
- **Every static field is initialized before `main` runs**, once. For an embedded script, this happens when the host loads it, before its first call.
- **The order follows dependencies.** If the initializer of `A.x` reads `B.y`, directly or through any function it calls, `B.y` is initialized first. The compiler computes this order over the whole program. Independent fields keep declaration order, and unrelated files follow a fixed order, so every run is the same.
- **Initializers that depend on each other in a loop are a compile error** that lists the loop. Java instead lets one of them see the other's field before it is set, which would put null into a non-null field.
- **No lazy initialization on first use**, unlike Java's classes. Startup cost is predictable, and dead-code elimination drops statics that are never used.
- **If a static initializer throws**, the program stops with that exception before `main`; for an embedded script, the load call fails.
- **At normal exit, statics are released in reverse order of initialization**, so their destructors run (§10.2).

---

## 12. Types and syntax

### 12.1 Primitives and references
Every type is a primitive or a reference, decided by the type; the programmer never writes the kind. The compiler has a built-in table, and any type not in it is a reference:

| Type | Kind | Size |
|---|---|---|
| `int` | prim | 4 |
| `long` | prim | 8 |
| `float` | prim | 4 |
| `double` | prim | 8 |
| `bool` | prim | 1 |
| `char` | prim | 4 |
| `byte` | prim | 1 |
| structs | inline; refs and prims split (§9) | sum of fields |
| everything else (`string`, `array<T>`, classes, generic instantiations) | ref | 8 |

- Primitive stores are plain stores; reference stores go through the barrier (§4.4).
- `null` applies only to nullable references: `int x = null` is a compile error.
- **Integer overflow is an error.** `+`, `-`, `*`, unary `-`, `++` and `--` throw `ArithmeticError` when the result doesn't fit the type, and so do `x / 0`, `x % 0` and `MIN / -1`. `MIN % -1` is 0, its mathematical value; the compiler adds the check that x86's divide instruction needs. Bitwise operators and shifts never throw. The check is one branch on the CPU's overflow flag, as in Swift.
- **Other behavior is written explicitly**, with the same symbols as index modes (§12.3):
  ```
  int x = a + b;      // throws ArithmeticError on overflow
  int y = a +% b;     // wraps around
  int z = a +| b;     // saturates at the type's minimum or maximum
  int q = a.divOr(b, 0);   // 0 instead of dividing by zero
  ```
  `-%`, `*%`, `-|`, `*|` and compound forms such as `+%=` work the same way.
- **No casts between reference types.** Upcasts are implicit, and a downcast is an `instanceof` pattern that binds a new variable only when it matches, so it can't fail:
  ```
  if (animal instanceof Dog d) {
      d.bark();
  }
  Dog d = (Dog) animal;      // compile error: use instanceof
  ```
  `instanceof` uses the class id, as a range check (§14.2). Cast syntax remains only for numbers, such as `(int) 3.7`.

### 12.2 Strings
- `string` is a reference in storage (its UTF-8 bytes live in its own block, like an immutable `array<byte>`) that behaves like a value: literals without `new`, immutable, and `==` compares contents.
- **String literals are interned and permanent** (§6.3), so they are never counted. Strings built at runtime are not interned; hashing every runtime string would slow down string-heavy CLI work.
- Strings hold no references, so they add no edges to the class graph (§5).
- Interpolation: `"Processed ${count} files in ${path}"` (Decided).
- Open: indexing by byte or by code point (`char` is 4 bytes).

### 12.3 Arrays
`array<T>` is a fixed-size reference type whose length is set at creation.

- **Layout**: `[ header | dims | data ]`. The header holds the usual counts and flags (§14.2) and the class id. The stride comes from the element type and is never stored per array.
- **Access**: bounds check, then a load at `data + i * stride`. Because the length is fixed, the compiler can often remove bounds checks in loops.
- **Index modes**: `arr[i]` throws `IndexOutOfBoundsError` when out of range. `arr[%i]` wraps, using true modulo, so `arr[%-1]` is the last element and `arr[%length]` the first. `arr[|i]` clamps to the first or last element. Both work for writes too, apply per index in multi-dimensional arrays (`m[%row, col]`), and still throw on an empty array, which has no valid index. `arr.getOr(i, fallback)` returns the fallback when out of range. Clamping costs two compare-and-select instructions; wrapping needs a division unless the length is a power of two or the index is known to be in range.
- **Creation**: primitive arrays, and struct arrays without non-null reference fields, are zero-filled. Reference arrays are created with a fill function or as a `nullarray` (§8.2).
- **Primitive arrays hold no references**: freeing one is a single block release with no per-element work. **Reference arrays** decrement every element when freed, and `arr[i] = x` goes through the barrier.
- **Resizing** means allocating a new array and copying; there is no `resize`.
- **Multi-dimensional arrays, rank 1 to 3** (`MAX_ARRAY_RANK = 3`):
  ```
  array<int>       a = new array<int>(100);           // rank 1
  array<int, 2>    m = new array<int, 2>(100, 100);   // 2D
  array<int, 3>    v = new array<int, 3>(10, 20, 30); // 3D
  ```
  - One contiguous row-major block with one dimension per header field. No inner row objects.
  - `m[i, j]` (not `m[i][j]`) checks each index and loads `data + (i * dim1 + j) * stride`; rank 3 uses `((i * dim1 + j) * dim2 + k) * stride`.
  - Wrong index counts and rank above 3 are compile errors.
  - The dimension product is computed in 64 bits, and creation fails if it exceeds the 4-byte length limit (about 4 billion elements).
  - `array<array<int>>` stays a jagged array of separate rows.
  - Dimensions: `m.length(0)`, `m.length(1)`.
- **Allocation**: large arrays can live in a separate region with a page allocator.
- **Alignment**: blocks start 8-byte aligned and the header is a multiple of 8, so `double` and `long` elements are aligned.

### 12.4 Lists
`list<T>` is an ordinary generic class over `array<T>`:

```
class list<T> {
    array<T?> data;   // capacity = data.length; slots past size are empty
    int size;
}
```

- Growth (e.g. doubling) allocates a larger array, copies, and drops the old one, which counting frees.
- **Vacated slots are set to null through the barrier** when elements are removed, so removed objects aren't kept alive.
- For reference lists, growth costs one count update per element (increment into the new array, decrement when the old one is freed). With doubling that is still amortized O(1) per append.
- `list<Point>` (a struct) stores points inline.
- **Element kinds**: a type argument can be `owned` or `weak`, and the class graph uses that kind (§5.2).
  - `list<owned T>` and `array<owned T>` own their elements. `add(fresh T)` takes a fresh object, reading an element gives a borrow, and `removeAt(i)` returns the element detached, like `take` (§6.5). `take` and `replace` also work on slots of owned arrays.
  - `list<weak T>` and `array<weak T>` hold weak references, which read as `T?`.
  - Plain `list<T>` holds strong references.

### 12.5 Function types and lambdas (Decided)
```
func(int, int -> int) add = (a, b) -> a + b;
func(string -> void) log = msg -> print("[log] ${msg}");
func(int -> bool) isEven = n -> {
    return n % 2 == 0;
};

int x = add(2, 3);                                    // called like a method
```

- **`func(A, B -> R)`** is the type of a function that takes an `A` and a `B` and returns an `R`. Both sides are always written, with `void` for "none": `func(void -> void)` takes and returns nothing, `func(string -> void)` returns nothing, `func(void -> bool)` takes nothing. Leaving out either side (`func()`, `func(string)`) is a compile error, so each type has exactly one spelling. This replaces Java's separate interfaces (`Runnable`, `Function`, `BiFunction`, ...).
- **Lambdas** are written `(a, b) -> expr`, `(a, b) -> { statements }`, or `a -> expr` for one parameter. Parameter types come from the expected function type and can be written out: `(int a, int b) -> a + b`.
- **Parameter modes are part of the type** (§7.2): a `func(borrow Dog -> void)` accepts `k.rover`, a `func(Dog -> void)` doesn't, and a `func(fresh Dog -> void)` takes ownership.
- **Nullability reads unambiguously**, since the return type stays inside the parentheses: `func(void -> Dog)?` is a function that may be null, while `func(void -> Dog?)` is a function that returns a nullable `Dog`.
- **Function values are references.** A lambda that captures variables is a heap object whose captures are its fields (§6.4). A lambda that captures nothing is allocated once and is permanent (§6.3), so it is never counted.
- **Captures are read-only.** A lambda can read local variables, parameters and `this` from the surrounding code, but a captured variable must never be assigned after its initialization, inside the lambda or outside it (Java's "effectively final" rule):
  ```
  int minAge = readAge();
  var adults = dogs.filter(d -> d.age >= minAge);   // fine: reads minAge
  minAge = 0;                                       // compile error: minAge is captured, so it can't change

  int count = 0;
  func(void -> void) inc = () -> count++;           // compile error: a lambda can't change a captured variable
  func(int -> int) next = x -> x + 1;
  count = next(count);                              // fine: pass values in and out instead

  class Counter { int n; }
  Counter c = new Counter();
  func(void -> void) bump = () -> c.n++;            // fine: c never changes, only the object it points to
  ```
- **No hidden heap boxes.** Because captured values never change, a closure stores a copy of each when it is created. A captured reference is counted like any other strong field, or uncounted as an owner capture (§6.4). Capturing a `weak` local captures it weakly.
- **A lambda that captures a borrow is itself a borrow** (§7). It can be called and passed to `borrow` parameters, but not stored in a strong slot, since it must not outlive what it borrowed. Owner captures in owned fields (§6.4) are how a stored callback refers back to its owner.
- **Loops**: a for-each variable is new on each iteration, so it can be captured. A classic `for (int i = 0; ...; i++)` counter changes, so capturing it is a compile error (§12.6).
- Open (§20): method references such as `this::close`.

### 12.6 Statements and expressions (Decided)
Statements and expressions follow Java unless listed here: operators and precedence, `if`/`while`/`do`/`for`, `break`/`continue`, `?:`, casts and `instanceof`. The full grammar is in `GRAMMAR.md`.

```
var dogs = new list<Dog>();             // type inferred from the initializer

for (Dog d : dogs) {                    // for-each
    d.bark();
}

string size = switch (count) {          // arrow form only, no fall-through
    case 0 -> "none";
    case 1, 2 -> "a few";
    default -> "many";
};

enum Color { RED, GREEN, BLUE }
```

- **`var`** works only for local variables with an initializer, as in Java. It can't be used for fields, parameters or return types, and `var x = null;` is an error.
- **For-each** works over arrays and lists. The loop variable is a new local on each iteration, so lambdas created in the loop each capture their own element.
  - **Changing a list inside its own loop is safe.** The loop re-reads the list's backing array and size on every step, like `for (int i = 0; i < list.size; i++)`, so it never reads an array that growth has already freed. The loop simply sees the changes: no error is thrown.
  - Over a `list<owned T>`, the loop variable is a borrow that pins its element (§6.1), so removing that element inside the loop detaches it instead of freeing it while in use.
- **`switch`** has only the arrow form: no fall-through and no `break`. A case can list several values (`case 1, 2 ->`). It works on `int`, `char`, `string` and enums, as a statement or as an expression. As an expression it must cover every case, with `default` or every enum constant.
- **Enums** are Java-style lists of constants. The constants are permanent objects (§6.3), so they are never counted. Enums with fields and methods are Open (§20).

### 12.7 Errors and exceptions (Decided)
```
try {
    string text = File.read(path);
    int n = int.parse(args[1]);
} catch (FileNotFoundException e) {
    print("no such file: ${path}");
} catch (ParseException e) {
    print("not a number: ${args[1]}");
} finally {
    print("done");
}

throw new UsageError("missing --output");
```

- **Syntax**: Java's `try`/`catch`/`finally`/`throw`, with no `throws` clauses. Any function may throw (no checked exceptions, §1).
- **Hierarchy**:
  ```
  Throwable
  ├── Error                       bugs in the program; rarely caught
  │   ├── ArithmeticError         overflow, x / 0, x % 0, MIN / -1
  │   ├── IndexOutOfBoundsError   an index or range outside an array, list or string
  │   ├── ArgumentError           invalid arguments, e.g. new array<int>(-1)
  │   ├── OutOfMemoryError        allocation failed, or #max_heap_mb exceeded
  │   └── StackOverflowError      recursion too deep
  └── Exception                   expected failures that programs handle
      ├── IOException             FileNotFoundException and others
      ├── ParseException
      └── user-defined exceptions
  ```
- **Everything is catchable.** `catch (Exception e)` doesn't catch an `Error`; `catch (Error e)` or `catch (Throwable t)` does, for example at the top level of a long-running program.
- **Ruled out before the program runs**: null dereferences (§8.2), reference cycles (§5), ownership violations (§7.4), use-after-free, failed casts (§12.1), and changing a list while looping over it (§12.6).
- **Lookups that may find nothing** return `T?` instead of throwing: environment variables, map keys.
- **Explicit alternatives** for when an error isn't wanted: `+%` and `+|` (§12.1), `arr[%i]`, `arr[|i]` and `getOr` (§12.3), `divOr`.
- **Uncaught**: the message and stack trace go to stderr, unwinding releases every frame's references, running destructors (§10.2), and the program exits with status 1. For an embedded script, the error is returned to the host through the C API.
- **Closed pipes**: writing to stdout after the reading end has closed, as in `zink run tool.zn | head`, ends the program quietly, the way standard Unix tools do: cleanup runs, no message is printed, and the shell sees the usual SIGPIPE status. Writing to other files and pipes throws `IOException` instead.

---

## 13. Generics

### 13.1 Why generics are a layout problem
`list<int>` stores its elements in the prim section and `list<Node>` in a reference section. One compiled `list` can't serve both without branching at runtime on every access or boxing primitives.

### 13.2 Monomorphization with kind-aware sharing
- **Primitive and struct arguments**: specialize. Each instantiation is its own compiled class with sizes and instructions hardcoded.
- **Reference arguments**: every reference is the same size, so `list<Foo>` and `list<Bar>` can share one compiled instantiation; the static type system guarantees element types.
- Specializing everything is the simplest start. Whether to share reference instantiations from the start: Open.
- The kind is inferred from the argument using the built-in table. `T: class` / `T: prim` constraints exist for the rare body that depends on it.
- Multiple parameters multiply layouts (`map<K, V>` has up to four); each is just an instantiation.

### 13.3 Value generics
A later feature, with syntax like `<const N: int>` to keep it distinct from type parameters. Until then, array rank (§12.3) is a special case of the built-in `array` type. Open.

### 13.4 Runtime type information
No reified generics for shared reference instantiations at first; Java does without them. If needed later, a per-object generics slot (one class id per type argument) can be added.

### 13.5 Compiler details
- Instantiations get internal names users can't type, so they never collide with a user class like `list_int`. Errors show `list<int>`.
- Nested instantiations are cached by (generic, type arguments).
- Recursive types: register the instance in the cache before expanding its body (`class Node<T> { owned Node<T>? next; }`).
- Runaway instantiation (`Foo<T>` using `Foo<list<T>>`): depth limit, or reject.
- Instantiate only what is used. The compiler sees the whole program (no dynamic class loading).
- Type-check generic bodies once against declared bounds (`T extends Comparable`).
- Reference kinds in type arguments (`list<weak Node>`): Open.

---

## 14. Heap and object layout

### 14.1 Sections
Each object is one contiguous block, split by field kind:

```
[ header | strong refs | owned refs | weak refs | owner refs | prim bytes ]
```

- **The runtime needs no type information**: the free path decrements the strong section and frees the owned section; weak and owner sections are skipped. Each reference section is a plain array of 8-byte references.
- **Primitive writes need no barrier.**
- **Primitive fields are sorted by size (descending), then name**, so they stay aligned with almost no padding.
- **Inheritance works per section**: parent fields first, then child fields, in every section. A subclass layout is prefix-compatible with its parent's, so inherited methods use the same indices.
- The class descriptor records each section's size and each field's section and index or offset. Fields of struct type contribute their references and primitives to the matching sections.

```
class Node { int value; bool visited; Address addr; owned Node? next; weak Node prev; }

strong: addr    → slot 0
owned:  next    → slot 0
weak:   prev    → slot 0
prims:  value   → byte 0 (4 bytes)
        visited → byte 4 (1 byte)
```

### 14.2 Header (Decided)
```
16 bytes at the start of every object:
[ class id: 32 bits | flags: 32 bits | count: 64 bits ]
  offset 0            offset 4          offset 8
```

- **Class id**: an index into the class descriptor table (section sizes, method table, interface table). Every generic instantiation and every kind of closure gets its own id. Ids are numbered in class-tree order, so every subclass of `Animal` has an id between `Animal`'s first and last, and `x instanceof Animal` is one range check. Interfaces use a small per-class table.
- **Flags**: `owned`, `detached`, `permanent`, `has-weak-slot`, `dying`. The remaining bits are reserved. `dying` is set when the object's destructor phase starts (§4.3); it makes weak references read null and keeps a pin taken during destruction from freeing the object a second time.
- **Count** (§4.1): at an 8-byte-aligned offset, so one instruction updates it, and it could be updated atomically if threads ever share objects.
- **The weak-slot index** isn't in the header. A side map from object to slot is consulted only when `has-weak-slot` is set.
- **Arrays** store their dimensions right after the header (§12.3).
- **Cost**: 16 bytes per object, the same as Swift and within Java's 12–16. A list node with one reference and an `int` is 16 + 8 + 4 = 28 bytes, padded to 32. Bulk data avoids most of this: an array of a struct has one header for the whole array (§9), and inline owned arrays could do the same for objects later (§20).

### 14.3 References and allocation (Decided)
- **References are 64-bit pointers** to the object's block. A field read is one load at a fixed offset.
- **The size is invisible to the language**: there is no pointer arithmetic and no conversion between references and integers.
- **One `Ref` type everywhere.** The compiler's IR and the C runtime never use a raw pointer or integer type for references, so 32-bit compressed pointers can be added later as a contained change if benchmarks justify it. Compressed pointers would be offsets, scaled by 8, from a region reserved at startup: they halve reference size (up to 32 GB of heap) but need a guard area at the region's start, string literals copied into the region, and stack objects only in broken-up form.
- **Null is 0.** The compiler never dereferences a value that may be null (§8.2). As a backstop against bugs in the compiler or the C runtime, Linux never maps the lowest pages of the address space, so a dereference of 0 crashes immediately instead of reading or overwriting memory.
- **No moving and no compaction.** Short-lived CLI tools rarely fragment enough to need it.
- **Allocation**: objects are freed individually, so allocation uses size-class free lists. String literals live in the executable's read-only data. Other permanent objects use a separate bump region. Large arrays can use a page allocator.
- A handle table (id → object, with generations) is kept only where needed: host API handles and the weak side table (§8.1).

The original design routed every reference through a handle table (Appendix A.15).

---

## 15. Native compilation and runtime

### 15.1 Target
Programs compile to native machine code, so standalone CLI tools carry no VM. Types choose instructions and then disappear, except for class descriptors, class ids and method tables.

### 15.2 The runtime is a library
A small C static library linked into every program: allocator, counting and write barrier, weak side table, class descriptors, and built-ins (strings, arrays, I/O), plus the host API. With dead-code elimination and a static build it is tens of KB.

Keep the interface small: allocate, store a strong field (the barrier), free, load a weak reference, allocate a permanent object. Define the class descriptor format once as a C header, and have the compiler generate matching tables.

### 15.3 Stack references and exceptions
- Stack references are counted (§4), so the runtime never scans the stack. **No shadow stack, stack maps or safe points are needed for memory management**, and references can stay in registers.
- **Stack overflow**: each stack ends in an unmapped guard page. Touching it raises `StackOverflowError`, through a signal handler running on a separate small stack. Functions with frames larger than a page touch each page in order (stack probes), so a large frame can't jump past the guard page into other memory.
- **Exceptions**: unwinding must decrement the counted locals of every frame it leaves, as C++ runs destructors during unwinding. Options: unwind tables (`.eh_frame` and landing pads, which the compiler must emit itself since it writes the ELF) or checking an error flag after every call. Open.

### 15.4 Dispatch (Proposed)
- vtables for class methods: static typing makes a virtual call an array index.
- Per-class interface tables for interface calls.

### 15.5 Backends and executables
- **Decided**: the compiler emits machine code directly, not C, so users don't need a C toolchain.
- Backend options:
  1. **Cranelift** as a library: handles register allocation and instruction selection for x86-64, AArch64, RISC-V and s390x. Least effort per architecture; code quality below LLVM; a large dependency for the compiler (not for compiled programs).
  2. **Own backend**: start with x86-64, every IR value in a stack slot with fixed instruction templates, then linear-scan register allocation. Full control, most work.
  3. **QBE**: small with good code, but emits assembly text and needs `as` and `ld`, which undercuts emitting machine code directly.
- Producing the executable: the system linker (`ld`/`lld`), or writing the ELF directly (static, no libc, raw syscalls; the `object` crate handles the byte layouts). The runtime is compiled once to a relocatable object that the compiler embeds and applies relocations for.
- Calling convention: System V ABI on Linux x86-64 and ARM64.
- Debug info (DWARF via `gimli`): not needed on day one.

---

## 16. Optional bytecode VM (for embedding)

- Another backend over the same typed IR and the same runtime library. It is linked only into hosts that embed scripts, so CLI tools still carry no VM.
- Typed opcodes (`ADD_I32`, `ADD_F64`, `LOAD_REF`, `STORE_REF` with the barrier); no runtime type checks.
- Not free: the dispatch loop, a bytecode format, a loader that builds class descriptors, verification if scripts are untrusted, and a host C API with references as boundary types.
- Hosts usually precompile scripts to bytecode rather than shipping the compiler.
- Roughly 10–50× slower than native: fine for scripts, not for hot loops.
- Alternatives: AOT-compiled `.so` plugins, or compiled code linked into the host through a C ABI. Embedding model: Open.
- The host sets program settings through the API (precedence in §11.2).

---

## 17. Compiler implementation and build order

### 17.1 Rust for the compiler, C for the runtime (Decided)
**Rust for the compiler**
- Compilers are mostly tree and graph transformations. Enums with exhaustive matching flag every place that fails to handle a new AST or IR node.
- Many passes (parsing, type checking, monomorphization, ref/prim and reference-kind classification, IR lowering, codegen): the type system catches most cross-pass refactoring mistakes.
- The backend ecosystem is in Rust: Cranelift, `object` (ELF), `gimli` (DWARF), encoders such as `iced-x86` or `dynasmrt`.
- Graph-shaped data is stored in arenas and referred to by integer index.
- Cargo, tests and fuzzing work out of the box.

**C for the runtime**
- Tiny, linked into every program, with a clean C ABI for embedding.
- Compiled once ahead of time to a relocatable object the compiler embeds. No cross-language linking for users.
- Zig would also fit. Rust `#![no_std]` can work but needs care with panics, unwinding and accidentally pulling in std.

### 17.2 What emitting machine code requires
- Instruction selection and register allocation, per architecture.
- Encoding instructions into bytes.
- The object and executable format (ELF): sections, symbols, relocations.
- Linking generated code with the runtime object.
- Calling conventions shared with the C runtime.
- Unwind information for exceptions (§15.3).
- Debug info (DWARF), eventually.

### 17.3 Build order (Proposed)
1. **Typed IR**: ref/prim split, reference kinds (strong, owned, weak, owner, borrow), structs, with monomorphization done before the IR.
2. **VM backend**: brings up the language, counting and owned/weak semantics without machine code; easier to debug. The cycle rule is a front-end check and needs no runtime support.
3. **Compiler analyses**: move and borrow count removal, escape analysis, null-flow checking, borrow-pin removal.
4. **Cranelift native backend**: modest work once the IR is typed and specialized.
5. **Own backend** later, only if dependency size or code quality becomes a problem. Realistic if only x86-64 Linux is targeted; Cranelift saves a lot if ARM is wanted.

### 17.4 Command-line tool (Decided)
The compiler ships as one command, `zink`:

| Command | What it does |
|---|---|
| `zink build main.zn -o tool` | Compile to a native executable at the given path |
| `zink run main.zn [args...]` | Compile into the cache if needed, then run the result |
| `zink main.zn [args...]` | Same as `zink run`; this is what a `#!/usr/bin/env zink` shebang calls |
| `zink run --vm main.zn [args...]` | Run on the bytecode VM (§16) instead, for debugging and for checking that both backends agree |
| `zink cache clean` | Empty the cache |

Native code still needs a compile step, so `zink run` works like `go run` or `cargo run`, with a cache that makes repeat runs as fast as a prebuilt binary:

```
zink run main.zn arg1 arg2
  1. hash the sources + compiler version + target + build flags
  2. look in the cache for <hash>
       hit  -> go to 4
       miss -> compile to a native executable
  3. write it to a temporary file in the cache, then rename it into place
  4. exec it with arg1 arg2
```

Rules:
- **`exec`, not spawn.** The program replaces the `zink` process, so it keeps the process ID, stdin/stdout/stderr, signals and exit code. Pipes, `$?` and Ctrl-C behave as if the binary were run directly.
- **Arguments.** `zink`'s own flags go before the source file; everything after it goes to the program unchanged. `zink run --vm main.zn -v` passes `-v` to the program.
- **Cache key.** The entry file and everything it imports, the compiler version, the target, and the build flags (including `#if` symbols set on the command line). Upgrading Zinkite therefore rebuilds everything, and a binary from an older compiler is never run. File modification times can be checked first to skip rehashing unchanged sources.
- **Cache location.** `$XDG_CACHE_HOME/zink/`, or `~/.cache/zink/` if that is unset. The directory is created with mode 0700 and must be owned by the user, so another user can't plant a binary that `zink run` would execute.
- **Atomic writes.** Writing to a temporary file and renaming it means two runs at once never see a half-written binary; if both compile, the second rename harmlessly replaces the first.
- **Cost.** The first run pays for compilation, targeted at tens of milliseconds for a small script with Cranelift or the own backend (LLVM would be much slower). Later runs cost a hash of the sources plus about 1 ms of startup.
- Compiling in memory and running on the VM by default were considered and rejected (Appendix A.19).

---

## 18. Performance expectations

| Language | Object-heavy, long-running | Short CLI run | Why |
|---|---|---|---|
| C++ / Rust | They're faster | They're faster | No counting by default, values inline, LLVM |
| Java | Java is faster | **Zinkite wins** | Bump allocation, generational GC and a JIT, but the JVM takes tens of ms to start |
| Go | Go is probably faster | About even | No per-store cost, escape analysis, value types; a heap under 4 MB usually never gets collected; both start in about 1 ms |
| Swift | Similar; **Zinkite can win** | Similar | Both count references, but Swift's counts are atomic and these aren't; Swift has years of count-removal work and LLVM |
| Python / Ruby / Lua | **Zinkite wins by a lot** | **Wins** | Interpreted |
| JavaScript (Node) | V8's JIT is competitive | **Wins** | Node takes tens of ms to start |

What this design pays that others don't:
- A count update on every strong store, plus count updates on locals the compiler can't remove.
- Cranelift instead of LLVM on compute-heavy loops.

What moves it up the table:
1. **Structs** (§9; Decided). The largest single factor: contiguous `array<Point>` instead of an array of references.
2. **Count removal** (§4.2): decides whether it lands near Swift or well behind it.
3. **Direct field reads** through plain pointers (§14.3), and compressed pointers later if they pay off.
4. **Non-atomic counts**: an edge over Swift while each heap is single-threaded.

What it offers that Go and Java can't: deterministic cleanup, lower peak memory, and a compile error for any class design that could form a reference cycle.

---

## 19. Prior art

- **Swift**: ARC with atomic counts; `weak` (optional, side tables) and `unowned` references; strong cycles leak silently.
- **Nim**: ARC (move and borrow inference to remove count updates) and ORC (ARC plus a trial-deletion cycle collector).
- **Lobster**: compile-time removal of reference-count updates.
- **Vale**: single ownership plus generational references, with analyses to remove generation checks.
- **Hylo** (formerly Val): mutable value semantics with second-class references.
- **Rust**: ownership and borrowing; opt-in `Rc`/`Arc`/`Weak`; graphs through indices (e.g. `slotmap`, which uses generations like §8.1).
- **Java Project Valhalla**: value classes.
- **CPython**: reference counting plus a cycle collector; immortal objects (PEP 683, Python 3.12) skip counting, like §6.3.
- **Kotlin, Swift, C#**: compiler-checked nullability.
- **AngelScript**: statically typed, class-based embeddable language with reference counting and a cycle collector.
- **Wren, Lua**: small embeddable languages with tracing collectors.
- **Go**: single-binary CLI tools with a concurrent mark-sweep collector.
- ***Crafting Interpreters*** (clox): classes, single inheritance, cached dispatch and mark-sweep in about 5k lines of C.
- Papers:
  - Bacon & Rajan, "Concurrent Cycle Collection in Reference Counted Systems" (ECOOP 2001): trial deletion.
  - Bacon et al., "Java without the Coffee Breaks" (PLDI 2001): the Recycler, deferred counting with epoch stack buffering.
  - Shahriyar, Blackburn et al., "Taking Off the Gloves with Reference Counting Immix" (OOPSLA 2013): reference counting competitive with tracing.
  - Pearce & Kelly, "A Dynamic Topological Sort Algorithm for Directed Acyclic Graphs" (2006): incremental cycle detection.

---

## 20. Open decisions

**Memory model**
- **Final directive set** (§11.2), replacing the `#gc_*` directives.
- **Inline owned objects** (a later optimization):
  - `final owned` fields stored inside their owner's block.
  - `array<owned T>` with a `final` element class stored as one block with one header, the elements packed inside with no headers of their own. Reading an element gives a borrow that pins the array, a `weak` reference to an element is the array plus an index, and removing an element (`take`, `removeAt`) copies it out into its own block, which is allowed only while no borrow of it is in use.
- **Arenas**: deferred until profiling shows counting at the top of a profile.
- **Threads**: one heap per thread; sharing immutable permanent objects between them.
**Types**
- **Element kinds in user generic classes**: how `owned T` and `weak T` type arguments (§12.4) apply to the fields and methods of a user's own generic class, e.g. `map<string, owned Config>`.
- **Precision of the freshness check** when borrows obtained through `weak` references are live (§6.1); currently the worst case is assumed.
- **Value generics**, and array rank as a special case until then.
- **Generics**: specialize everything first, or share reference instantiations from the start.
- **String indexing**: bytes or code points.
- **`readonly` fields** and automatic inlining of immutable final classes.
- **Narrowing casts** such as `(byte) 300`: truncate or throw.
- **Later syntax**: enums with fields and methods; pattern matching in `switch`; an interface that lets for-each iterate user classes.

**Packages and closures**
- **Static imports** (`import static zink.math.Math.max;`) and **aliases for whole packages** (`import com.example.util as u;`).
- **Library search path**: `--source-path` for now; a package manager and versioning later.
- **Method references** such as `this::close`.

**Implementation**
- **Compressed 32-bit pointers** as a later optimization, decided by benchmarks (§14.3).
- **Exceptions**: unwind tables vs. checking a flag after every call.
- **Backend route**: Cranelift first (after the VM), own backend later; system linker vs. writing the ELF directly.
- **Embedding model**: precompiled bytecode, AOT `.so` plugins, or none; the host C ABI shape.

---

## 21. Suggested tests

**Counting and freeing**
1. Acyclic garbage is freed on the line its last strong reference drops: field overwrite, local reassignment, scope exit, discarded return value.
2. A returned object is not freed before the caller stores it, and is freed if the caller discards it.
3. Assigning null decrements the old target and increments nothing.
4. Two fields of one object pointing at the same target give a count of 2; clearing one keeps the target alive.
5. Free order: locals in reverse declaration order. A freed object and its owned tree run every `delete()` first (subclass before base, parents before children, siblings in declaration order); then each object releases its owned fields, then its other fields, in declaration order.
6. Exceptions: unwinding decrements the counted locals of every unwound frame; nothing leaks.

**Cycle prevention**
7. `class Node { Node next; }` is a compile error that names the cycle `Node.next → Node` and suggests `owned` or `weak`.
8. A cycle across classes (`Department.staff → Employee.dept → Department`, both strong) is a compile error that lists the whole path.
9. A cycle through an owned edge plus a strong edge (`Kennel` owns a `Dog`, and `Dog` has a strong `Kennel` field) is a compile error.
10. Cycles made only of owned edges (`owned TreeNode? left`) compile, and at runtime they only ever form trees.
11. Weak and owner fields add no edges: a doubly linked list with `owned next` and `weak prev` compiles.
12. A strong field of an interface type gets edges to every implementing class; a new subclass in another file that closes a cycle makes the existing field an error, and the message names both.

**Owned, owner, borrows, weak**
13. An owned object is freed with its owner, in declaration order; whatever only it referenced is freed in the same cascade.
14. Compile errors: storing an owned object in a strong slot; reassigning a `final owned` field; `new Dog()` outside an owned field for a class with an `owner` field; reading `home` in the constructor.
15. `Dog d = k.rover; k = null; d.bark();` works, and the Kennel is freed when `d` leaves scope.
16. A method running on an owned object whose owner is replaced in a global during the call stays valid until it returns.
17. An `owner` reference is valid in every method.
18. Reading a `weak` reference after its target is freed gives null; using it without a null check is a compile error.
19. `static owned` objects are never freed, stores of them skip counting, and `weak` references to them never read as null.

**Structs and elision**
20. Structs copy on assignment; `pts[i].x = 5` updates in place; the default value is all zeros.
21. `array<Entry>` lays out references then primitives; copying a value with references increments their counts; freeing the array decrements each.
22. A non-escaping local object never reaches the allocator (check with an allocation counter).
23. Moves on last use and uncounted parameter passing emit no count operations (IR-level test).

**Directives**
24. Unknown directive, directive outside the main file, and duplicate directive are compile errors.
25. `#if` around directives selects per-build settings; precedence is host > build flag > directive > default.
26. Both `#if` branches must parse; only the active one is type-checked.
27. `#leakcheck ON` reports nothing for a normal program.

**Layout, arrays, lists**
28. Subclass instances read inherited fields at the same indices as the parent in every section.
29. `array<int>` uses the right stride and bounds check; freeing a large primitive array does no per-element work.
30. Freeing an `array<Node>` decrements each element; setting a slot to null decrements the old element.
31. Rank 2 and 3 arrays index correctly with per-index bounds checks; wrong index counts and rank above 3 are compile errors; an overflowing dimension product fails at creation.
32. `list<T>` growth frees the old backing array, and removed elements aren't kept alive by vacated slots.

**Parameter modes**
33. Passing an owned object or a borrow to a storable parameter is a compile error; passing it to a `borrow` parameter works.
34. Storing a `borrow` parameter in a strong field, returning it as a strong reference, or passing it to a storable parameter is a compile error; storing it in a `weak` slot works.
35. Storing `this` strongly in a class used as an owned field is a compile error, and the message names the `owned` field.
36. An override can change a storable parameter to `borrow`; changing a `borrow` parameter to storable is a compile error.

**Performance**
37. A red-black tree written with `take` and `replace` keeps O(log n) inserts: rotations move subtrees in constant time.

**Command-line tool**
38. `zink run` passes the program's exit code, stdin/stdout and signals through unchanged, and arguments after the source file reach the program untouched.
39. A second `zink run` of unchanged sources does not recompile; changing any imported file, the compiler version or a build flag does.
40. Two concurrent `zink run`s of the same new script both succeed and never execute a half-written binary.
41. `zink run` refuses to use a cache directory that is writable by other users or owned by someone else.
42. `zink run` and `zink run --vm` produce the same output on the whole test suite.

**Packages and imports**
43. `import a.b.C as D;` makes `D` mean `a.b.C` in that file only, and leaves `C` unbound there.
44. Two imports of the same simple name are a compile error that suggests `as`; an explicit import beats a wildcard; a name from two wildcards is an error only where it's used.
45. A `package` line that doesn't match the file's directory is an error, and the source root is inferred correctly from the entry file.
46. Importing a package-private class from another package is an error; circular imports between packages compile.

**Closures**
47. In the `Window`/`Button` example (§6.4), `ok.onClick = () -> this.close();` compiles, and freeing the Window frees the Button and the closure.
48. The same lambda stored in a strong `func` field of `Button` is a compile error (class cycle Window → Button → closure → Window).
49. A closure that replaces its own `owned` field while running finishes safely, and the old closure is freed when the call returns.
50. A closure that captures a weak local never keeps its target alive and does nothing after the target is freed.
51. Storing an existing closure (anything other than a lambda written at the store or a `fresh` parameter) in an `owned` field is a compile error.

**Reassigning owned fields**
52. Reassigning an owned field frees the old object on that line when nothing borrows it. With a live borrow, the old object and its `owner` reference stay valid until the borrow ends, and then it is freed.
53. Storing a borrow, a parameter, a field read, or a local used again afterward in an owned field is a compile error; `new`, lambdas and results of `fresh` functions compile with no runtime check; a result from a function that isn't `fresh` is a compile error.
54. A borrow whose lifetime contains no write to the field, directly or through any call, emits no extra count update; a borrow spanning a call that can write the field keeps the pin.
55. Using a borrow of the old object after reassigning the field in the same function compiles, with a warning.

**`fresh` parameters**
56. Passing `new`, a lambda, or a local at its last use to a `fresh` parameter works; passing an owned field, a borrow, or a local used again later is a compile error.
57. A `fresh` parameter the callee neither stores nor passes on is freed when the call returns; one stored in an owned field lives as long as that field holds it.
58. Storing a `fresh` parameter in a strong slot, passing it to a storable parameter, or overriding it with another mode is a compile error.
59. In `owned Button ok = new Button(() -> this.close());`, the capture of the Window is an owner reference: it compiles, the Window isn't counted, and freeing the Window frees the Button and the closure.
60. A constructor that calls a closure it received through a `fresh` parameter is a compile error.

**Script files and function types**
61. A file with only top-level methods and fields compiles; `void main()` and `void main(array<string> args)` both work as entry points, and directives at its top apply.
62. Importing from a script file is a compile error.
63. A `func(int, int -> int)` accepts a matching lambda and rejects one with the wrong parameter count or types; a lambda assigned to `func(string -> void)` can't return a value; `func()` and `func(string)` are compile errors.
64. A `func(borrow Dog -> void)` accepts `k.rover`; a `func(Dog -> void)` rejects it.
65. A lambda that captures nothing is allocated once and never counted.

**Syntax and arithmetic**
66. `var` infers the initializer's type; `var` without an initializer, `var x = null;`, and `var` on a field are compile errors.
67. For-each visits every element of an array or list once, and lambdas created in the loop each capture their own element.
68. A `switch` expression that misses an enum constant and has no `default` is a compile error; arrow cases never fall through.
69. Overflowing `int` and `long` addition, subtraction, multiplication and negation throw `ArithmeticError`, as do division by zero and `MIN / -1`; the wrapping operations wrap.
70. Enum constants are permanent: storing them never counts.

**Nullability**
71. Calling a method on a `T?` without a null check the compiler can see is a compile error; inside `if (x != null)` it compiles.
72. `new array<Dog>(n, f)` calls `f` once per index, in order; `new array<Dog>(n)` is a compile error; `new nullarray<Dog?>(n)` gives an `array<Dog?>` of nulls; `new nullarray<Dog>(n)` is a compile error.
73. If the fill function throws partway, the elements created so far are freed and nothing leaks.
74. `new array<Point>(n)` without a function is a compile error when `Point` has a non-null reference field.
75. A constructor that calls a method on `this` before assigning every non-null field is a compile error.

**Captures**
76. A lambda that assigns a captured variable, or code that assigns a variable some lambda captured, is a compile error.
77. A lambda created in a for-each loop captures that iteration's element; capturing a classic `for` counter is a compile error.
78. A lambda that captures a borrow can be called and passed to `borrow` parameters; storing it in a strong slot is a compile error.
79. Capturing a `weak` local doesn't keep its target alive.

**Moving owned objects**
80. `take(n.next)` leaves the field null and returns the subtree; storing it in another owned field moves it without copying; dropping it frees it with everything it owns.
81. `take` on a non-nullable owned field is a compile error; `replace` returns the old object detached.
82. Reading an `owner` field of a detached object, directly or through a method, is a compile error, and so is storing a detached object of a class with `owner` fields in a strong slot.
83. `list<owned Node>.add` accepts only fresh objects, `removeAt` returns the element detached, and reading an element gives a borrow.
84. A borrow of a grandchild through two non-final owned fields stays valid when either field is replaced or taken while the borrow lives.
85. Storing an object in an owned field of something it owns (`x.child.f = x`, or while a borrow into `x` is live) is a compile error.

**Fresh returns**
86. A `fresh` function that returns a field, or a local after passing it to a storable parameter, is a compile error; one that returns `new`, a never-shared local, or another `fresh` call compiles.
87. The result of a `fresh` function can go into an owned field, a `fresh` parameter or a strong slot; the result of an ordinary function in an owned slot is a compile error.
88. An override can change an ordinary return to `fresh`; changing `fresh` to ordinary is a compile error.
89. No ownership check remains in the generated code (IR-level test).

**Borrow returns**
90. `borrow Dog getRover()` returning an owned field compiles; storing its result strongly is a compile error, and using it, passing it to a `borrow` parameter or storing it in a `weak` slot works.
91. A `borrow` function returning a local, a new object, or anything not reached from `this` or a `borrow` parameter is a compile error.
92. While the caller uses a borrow returned by a method, the receiver stays alive even if the caller's last other reference to it is dropped.
93. With several `borrow` parameters, all the arguments stay alive while the result is used.
94. A `borrow` return can be overridden with an ordinary or `fresh` one; overriding an ordinary return with `borrow` is a compile error.

**Initialization and limits**
95. A for-each over a list whose body adds or removes elements never reads freed memory, and sees the changes like an index loop.
96. Static initializers run before `main` in dependency order; initializers that depend on each other in a loop are a compile error that lists the loop.
97. A base constructor that calls a method the subclass overrides sees the subclass's fields already set; using `this` before `super(...)` other than to assign own fields is a compile error.
98. Every object starts with the 16-byte header: class id at offset 0, flags at 4, count at 8; a list node with one reference and an `int` takes 32 bytes.
99. A weak slot whose generation is about to wrap is retired, and existing weak references to it read as null.
100. Unbounded recursion raises `StackOverflowError`, including in a function whose frame is larger than the guard page.

**Errors and exceptions**
101. Each built-in error is thrown in its case: overflow and `x / 0` give `ArithmeticError`, an out-of-range index `IndexOutOfBoundsError`, `new array<int>(-1)` `ArgumentError`, exceeding `#max_heap_mb` `OutOfMemoryError`, and unbounded recursion `StackOverflowError`.
102. `catch (Exception e)` doesn't catch an `Error`; `catch (Error e)` and `catch (Throwable t)` do. An uncaught one prints the message and stack trace, runs destructors, and exits with status 1.
103. `(Dog) animal` is a compile error; `if (animal instanceof Dog d)` binds `d` only when it matches.
104. `MIN % -1` is 0, and `MIN / -1` throws `ArithmeticError`.
105. A program printing many lines, run as `zink run tool.zn | head -1`, exits quietly with no error output.
106. At the limits of `int` and `long`, `+` throws, `+%` wraps and `+|` saturates; the same holds for `-` and `*`.
107. `arr[%-1]` is the last element and `arr[%length]` the first; `arr[|-5]` is the first and `arr[|length + 5]` the last. Both work for writes and per index in multi-dimensional arrays, and both throw on an empty array.
108. `arr.getOr(i, fallback)` returns the fallback when `i` is out of range.

**Constructors, destructors and resources**
109. `delete()` runs exactly once when an object is freed: by its last reference, with its owner, or when a detached object's last pin ends. A subclass's `delete()` runs before its base class's, and the fields are released after both.
110. Calling `x.delete()`, storing `this` strongly inside `delete()`, and declaring `delete()` in a struct are compile errors.
111. Weak references to an object read as null once its `delete()` has started.
112. An exception thrown out of `delete()` is reported on stderr, and freeing continues.
113. Destructors of objects held by static fields, and of permanent objects, run at normal exit and after an uncaught exception, in reverse order of initialization.
114. `close()` on a file whose buffered data can't be written throws `IOException`; a file freed while still open is closed by its destructor, which reports a failure on stderr; using a closed file throws `IOException`.
115. Constructors are declared `new(...)` and called as `new Dog(...)`; a local holding a file is released at the end of its block, not at its last use.
116. A child's `delete()` reading a strong field of its owner through `owner` (`home.name`) sees it intact whatever order the owner's fields are declared in, and so does a grandchild reading `home.home.name`.
117. A child's `delete()` reading a sibling through its owner (`home.rover.age` inside `spot`'s destructor) sees it intact, whichever sibling is declared first.
118. A `delete()` that can write an owned field of a class in its own owned tree (directly, through a called method, or with `take`, `replace` or `list<owned T>.remove`) is a compile error; writing strong and weak fields is allowed.
119. Letting a detached object go out of scope is a compile error when a destructor in its tree can read an `owner` reference past it; storing it in an owned slot instead compiles. An object freed by reassigning its field, or by a discarded `take` or `replace`, can read `home` in its destructor, even when a borrow delayed the free.
120. An object whose count reaches zero while a tree is being freed (for example, a strong field released during the tree's release) is freed after the tree, and its weak references into the tree read null.

---

## Appendix A. Superseded and rejected designs

### A.1 Counts + forward trial deletion with uncounted stack references
*Original §4.1, §5, §7. Superseded by cycle prevention (§5).*

Deferred reference counting (locals not counted, a zero-count list, a `rooted` bit set by stack scans at safe points) with Bacon–Rajan trial deletion run on batched candidates. It needed a collector, freed cycles at unpredictable points, and scanned roots at safe points. A review of its pseudocode found:
1. **Overwriting a local leaked cycles.** Only shadow-stack *pops* pushed cycle candidates, so `Node x = makeCycle(); x = null;` dropped the only outside holder with no event.
2. **Stack-only acyclic objects could leak.** Zero-count-list entries skipped because they were `rooted` were never revisited if the list was cleared (e.g. a string from `readLine()`).
3. **`free_object` ignored the buffered bit**, leaving freed objects in the candidate buffer.
4. **The candidate buffer held `Obj*`**, which growing the arena with `realloc` invalidated.
5. **Every safe point rescanned the stack and statics**, costing O(stack depth) per call, allocation and loop back-edge.

If this design is ever revived, Recycler-style epoch stack buffering fixes 1, 2 and 5: at each collection, increment everything on the current stack, then decrement the previous snapshot. Deferred counting also loses uniqueness information (a count of 1 doesn't mean unique), which rules out in-place reuse optimizations.

### A.2 Referrer table with backward walk
*Original §4.2. Rejected.*

Its one advantage is stopping early at the first rooted ancestor, which disappears once structures have back-pointers (the ancestors become the whole structure). Costs: list insert and remove on every reference write (possibly allocating inside the barrier), fan-out through popular objects, every edge stored twice, and the same root-handling bugs as A.1.

Flattening the map into `(target, referrer)` pairs doesn't fix the barrier: a multimap still searches the target's group on removal; a pair-hashed set removes in O(1) but can't list a target's referrers; a sorted set costs O(log n) per write; a back-index gives O(1) at about 16 bytes per edge.

**Optimizations considered.** They make A.2 faster than its original form, but not faster than counting, and its walks stay O(structure) in the structures it exists for: doubly linked lists, trees with strong parent pointers, graphs.
- **Up to two referrers inline in the header**, spilling to a separate set after that. The common store becomes a few instructions with no allocation, an estimated 2–4× a count update while referrers fit, and about 10× or more once they spill.
- **Small object IDs** (26 or 32 bits) instead of pointers, to shrink referrer sets. They limit the heap (26 bits is about 67 million objects) and need either an object table (A.15) or compressed pointers (§14.3).
- **Compressed spilled sets** (delta-encoded varints, Roaring bitmaps, hash sets once dense). They save memory, but every insert and remove decodes and re-encodes, so stores get slower.
- **A root bit** that ends a walk at an object a stack slot or global refers to. One bit can't tell whether a second local still refers to the object after the first goes out of scope, so stack references still need a count. Tracking registers needs stack maps (A.16).
- **Levels and anchors**, an idea related to Even–Shiloach trees. Each object stores its distance from a root and the one referrer that proves it (its anchor). Removing a non-anchor edge is O(1), and losing the anchor first looks for another referrer with a lower level. The full search remains when none exists:
  - In a doubly linked list with strong `prev`, removing node *i* leaves node *i+1* with only node *i+2* as a referrer, which is further from the root. The search walks to the tail, and every later node's level is recomputed: O(n).
  - Tree rotations move subtrees under deeper parents and walk them, as in A.22.
  - An 8-bit level saturates at 255, so longer lists fall back to the search.
  - Even–Shiloach's bounds hold only when edges are only deleted; a program keeps adding references.
- **Regions.** Split the heap into small regions so that a walk only has to reach the edge of its region (about 8 steps) when the region is connected. Rejected:
  - *A connected region doesn't make each object in it reachable.* A walk must reach one of the region's entry points (an object with an incoming reference from outside the region), so it costs up to the region's size.
  - *Regions form their own graph, with cycles.* Two regions pointing into each other each look connected when nothing outside reaches either, so regions need referrer tracking and walks of their own. A doubly linked list of 1 million nodes in regions of 8 is a chain of 125,000 regions, and removing a node in the middle can walk all of them: 8× fewer steps, still O(n).
  - *Regions drift out of line with the structures they hold*, for example the nodes of a tree built by inserting keys in random order. Keeping them lined up means moving objects, which needs an object table (A.15) or fixing up stack references with stack maps (A.16).
  - Prior art: the Train algorithm (Hudson and Moss, 1992) moved objects between regions to gather cycles, was slow to collect cycles spanning many regions, and was later dropped from HotSpot. G1 gives each region a set of incoming references, treats them all as live, and leaves dead cycles across regions for a full marking pass.
  - A region freed all at once when its entry point dies is an arena (A.14, §20). A region with exactly one entry point and a shape checked at compile time is an owned tree (§6), which needs no walks at all.
- **Elision**: like the hybrid (A.24), any walk sees only recorded references, so the compiler couldn't skip recording stack references the way it skips count updates (§4.2).

### A.3 Skipping the increment when the target already references the source
*Original §4.3. Rejected: unsound.* It frees objects prematurely, lets counts drift because increments and decrements stop pairing up, only handles two-object cycles, and scans the target's reference slots on every write.

### A.4 Non-moving mark-sweep
The simplest option, and a good fit for short-lived tools (many never collect). Not chosen because deterministic cleanup is a goal.

### A.5 Running the cycle collector every N commands
Rejected. A check's cost depends on the graph reachable from candidates, not on N; programs that create no garbage still pay; and the collection point shifts with unrelated edits, optimization level and backend. The replacement was an adaptive candidate-count threshold plus `gc.collect()`, which is moot now that cycles are prevented.

### A.6 GC modes and `#gc_*` directives
`#gc_mode ADAPTIVE | MANUAL | OFF | LEAKCHECK`, `#gc_threshold`, `#gc_max_heap_mb`, with runtime `gc.collect()` and `gc.setMode()`. Designed for the cycle-collector model:
- ADAPTIVE: adaptive candidate threshold.
- MANUAL: collect only on `gc.collect()`, plus an emergency collection before out-of-memory.
- OFF: compile-time; no candidate buffering, collector not linked.
- LEAKCHECK: report leaked cycles at exit.

With cycles prevented there is no collector to schedule. The program-wide directive mechanism carries over, with the new settings listed in §11.2. Cycles are now ruled out at compile time with no opt-out (§5), so these modes have no use.

### A.7 Per-class GC modes (`@gcMode(...)` on classes)
Rejected. Cycles span classes, so a mixed garbage cycle must follow its most manual member; the mode spreads to everything a MANUAL object references; automatic checks still walk through MANUAL objects; and a library's annotations would control when the program's memory is freed.

### A.8 Settings as annotations on `Main` (`@gc(mode = ...)`)
Replaced by `#` directives (§11.2).

### A.9 Sigils for GC (`%`, `$`, `#collect`)
Rejected. `$` is better used for string interpolation, `%` is the modulo operator, and `#` is reserved for compile-time directives. Runtime operations are ordinary functions.

### A.10 Single ownership for every object
Every object with exactly one owner, plus Vale-style generational non-owning references. It would remove counting entirely, but every program design would need an ownership plan, and the language would feel like C++ with `unique_ptr` rather than Java. Partially adopted as `owned` fields and borrows (§6–7).

### A.11 Full compile-time borrow checking
Rejected. It needs lifetime annotations and a mutable/immutable split, rejects safe code, and still can't check non-owning references stored in fields. Replaced by the three-outcome analysis (§7.3) and compile-time parameter modes (§7.2).

### A.12 `permanent`
- On classes (every instance permanent): dropped; it is per-field only.
- Implying `static`: dropped in favor of per-owner lifetime.
- As never-freed instance fields: would leak one object per owner instance.

It became `owned` (freed with its owner) and `static owned` (lives until exit).

### A.13 Manual `delete`
Rejected. Safe only with a generation check on every dereference, which slows the whole language, and it brings back manual memory management. The `delete()` destructor (§10.2) is different: only the runtime calls it, when the object is already being freed.

### A.14 Arenas (`arena { ... }`)
Deferred. Useful for "process one file, discard everything" tools, but the escape rules are the hard part. Go's experimental arena package was put on hold partly for that reason.

### A.15 Table 2 handle indirection
Every reference was a 4-byte id into a table of `{ class_id, offset, generation }` pointing into a growable byte arena. It enabled compaction and stale-id detection, but cost two dependent loads per field access; caching the base address rarely survived calls, since almost any call can allocate; and `realloc` invalidated raw pointers. Replaced by direct pointers (§14.3), keeping generations only for weak references and host handles. Original id conventions: `0` null, `1` ROOT (unused once roots became a flag), `2+` objects.

### A.16 Stack scanning
Shadow stack, stack maps, conservative scanning and the `rooted` bit are unnecessary once stack references are counted (§15.3).

### A.17 `struct` as a separate kind
A separate kind with its own rules was rejected: a struct is a class with value semantics, with the same constructors, methods, interfaces and generics (§9). `value class` (Valhalla, Kotlin) was considered as the keyword; `struct` was chosen because it is short and widely known.

### A.18 Interning all strings
Only literals are interned (§12.2).

### A.19 Other ways to implement `zink run`
- **Compile in memory and jump into it** (like `tcc -run`): nothing is written to disk, but `zink` has to act as a loader for the runtime and the generated code, and nothing is reused between runs.
- **Run on the VM by default**: compiles fastest but runs slower, and any difference between the VM and native backends becomes a user-visible bug. Kept as `zink run --vm` (§17.4).

### A.20 `owned` implies `final`
Superseded. Owned fields were final so that a borrow, which pins only the owner, could never see its object freed. That ruled out replacing a component or a callback. Borrows of non-final owned fields now pin the object as well, with the count removed when the compiler proves the field isn't written during the borrow, so owned fields are reassignable unless marked `final` (§6.1). Allowing reassignment only for fields holding closures was considered and rejected as a special case.

### A.21 Shared captures, and no captures at all
- **Shared captures** (JavaScript, Python, Kotlin, Swift): the lambda and the surrounding code share a variable, so changes are visible both ways. Rejected: each shared variable becomes a hidden counted heap object; a recursive lambda (`fact = n -> ... fact(n - 1)`) creates a hidden cycle between the box and the closure, which would be a cycle error in code with no visible objects; and `for (int i ...)` loops capture one shared `i`, printing `3 3 3`.
- **No captures** (Zig, C function pointers): every lambda would be a plain function pointer, never counted and never in a cycle. Rejected because filters (`d -> d.age > minAge`) and callbacks that refer back to their object (§6.4) would need a small class each. Allowing only `this` was also considered; it fixes callbacks but not filters.

### A.22 Runtime cycle check on every store
Superseded by the compile-time class-graph rule (§5). Before `b.f = a` on a strong or owned field, the runtime walked forward from `a` over strong and owned edges, and threw a catchable `CycleError` before storing if it reached `b`. Constant-time filters skipped most walks: `a == b`; permanent objects; `b->heap_count == 0 && !is_owned(b)` (no edge into `b`, so nothing can reach it); a compile-time `class_can_reach` table; and a DFS with walk stamps in the header. Rejected because:
- whether a store threw depended on runtime data, such as which two nodes the user's input picked;
- moving subtrees inside self-referential structures walked them, so red-black tree rotations near the root walked about a quarter of the tree (Pearce–Kelly incremental ordering would avoid this but is complex);
- it needed the `#cycle_check OFF` escape hatch and an `@acyclic` annotation, both removed with it.

### A.23 `with` and `Closeable`
A `with (var f = File.open(path)) { ... }` statement over a `Closeable` interface, like Java's try-with-resources. Dropped: counting already releases every object at the end of the block that held it, so explicit `close()` (which reports errors) plus a destructor backstop (which reports failures on stderr) covers the same cases with one less construct (§10).

### A.24 Counting plus runtime walks for cycle fields (hybrid)
*Rejected in favor of the compile-time cycle rule (§5).*

Count every reference as in §4. For strong fields on cycles of the class graph only, also keep referrer lists, as in A.2. When such a field drops a reference, walk backwards to decide whether anything outside the cycle still reaches it, and free the cycle if not. This is close to the cyclic reference counting of Martinez, Wachenchauzer and Lins (1990). It would let programs keep strong back-pointers, instead of `weak` and `owner` fields.

Rejected for performance:
- In the structures that need it (doubly linked lists, trees with strong parent pointers, graphs), the walk covers the whole structure: O(structure) per removed edge, so a loop that removes many edges becomes O(n²).
- Every store into a cycle field also updates a referrer list (A.2).

Rejected for safety, and use-after-free in particular:
- **It breaks the compiler's count-skipping.** Uncounted parameters, moves on last use, and borrows pinned only through their owner (§4.2, §7.1) are safe because an object is freed only when its count reaches zero. A walk adds a second way to be freed, and it sees only counted references:
  ```
  void visit(Node n) {        // parameter: not counted (§4.2); the caller keeps n alive
      other.link = null;      // this store starts a walk over n's cycle
      n.print();              // the walk found no counted outside reference
  }                           //   and freed the cycle: use-after-free
  ```
  Making this safe means counting far more references, or scanning the stack during walks, which brings back the stack maps of A.16.
- **Destructors in a dead cycle have no safe order.** Whichever `delete()` runs first, the others can read an object already destroyed, or store it and bring it back to life. Java finalizers and Python's `__del__` had this problem; Python's PEP 442 handles it, but destructors in cycles can still see half-destroyed neighbors.
- **Walks run in the middle of stores**, so objects they free and destructors they run must be postponed until the store finishes. The runtime grows by the referrer lists, the walks and these rules, and every piece is C code where a bug frees a live object.
- It would make cycles a runtime matter again, which the no-opt-out decision for §5 rules out.

---

## Appendix B. Garbage collection background

**Reachability.** An object is live if the program can reach it by following references from the **roots**: stack variables, registers, globals and statics, and host handles.

**Reference counting.** Each object counts the references to it and is freed when the count reaches zero.
- Pros: immediate, deterministic cleanup; no big pauses.
- Cons: cycles never reach zero; every reference write updates counts; threads need atomic updates; freeing a large structure cascades; no help with fragmentation.

**Tracing strategies**

| Strategy | Idea | Tradeoff |
|---|---|---|
| Mark-and-sweep | Mark reachable objects, free the rest | Simple; can fragment |
| Mark-and-compact | Mark, then slide live objects together | No fragmentation; must update references |
| Copying (semi-space) | Copy live objects into an empty half | Cost scales with live data; needs 2× space |
| Generational | Collect young objects often, old ones rarely | Needs write barriers |

Modern collectors reduce pauses with incremental, concurrent and parallel techniques (tri-color marking), trading throughput, latency and memory footprint against each other.

**Options for a small language without manual memory management**
1. A small tracing GC (the Lua approach).
2. Reference counting with cycles designed away: immutable data (Koka's Perceus), `weak` references (Swift), a cycle collector (Python, Nim ORC), compile-time count removal (Lobster). **This design: counting with cycles prevented at store time.**
3. Mutable value semantics (Swift structs, Hylo).
4. Ownership and borrowing (Rust).
5. Regions and arenas.

**Fit for CLI utilities and embedded scripts**
- Processes are short-lived, so many never need a collection at all.
- Startup time and binary size matter more than peak throughput.
- Mostly single-threaded.
- Strings dominate the workload.
- The embedding host holds references into the heap, which the host API must account for.
