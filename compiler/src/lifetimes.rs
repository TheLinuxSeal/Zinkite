//! Roots and lifetimes (REFERENCES.md), run after type checking:
//! - rule S: no reference outlives the variable it points into (§3, §4);
//! - rule R: no reference into an array is used after a removal from it (§5.2);
//! - references into removable arrays are kept only in locals and parameters (§5.3);
//! - no recursive call while a growable local is in scope (§8.4);
//! - a lambda that captures variables can't be stored (DESIGN §9.3).
//!
//! Each function body is first turned into a small flow form: its stores, uses,
//! removals and calls, in the structure of its blocks and loops. Every variable's
//! root set is the union of everything stored into it (§3.5), and every function
//! gets a summary (§4.1), found by repeating until nothing changes. Rule R then
//! walks each function backward to find which references are still used.
//!
//! Rule V's removal, V2: assigning a variable-size variable rebuilds it. The rest of rule
//! V is in the checker and the code generator. A link's type is the reference it holds,
//! so these rules apply to it as to any reference field; rule L's L1, that a link points
//! into its own array, is here too, by comparing roots; L2 is in the checker and L4 in the
//! code generator (REFERENCES §7.5).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::ast::{
    self, Expr, ExprKind as E, LambdaBody, LocalType, Name, Stmt, StmtKind as S, StrPart,
    SwitchBody, Target,
};
use crate::builtins::Origin;
use crate::check::{CallFact, Facts, NameRes, key};
use crate::diag::Diagnostic;
use crate::program::{ClassKind, Program, active, starts_empty};
use crate::source::{SourceFile, Span};
use crate::types::*;

type FnId = usize;

// ---------------------------------------------------------------- roots

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
enum TempKind {
    Text,
    Lambda,
    Value,
}

/// What a root names. Inside a function, the caller's data is represented by
/// `Formal` roots, which a call maps to the arguments' own roots.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
enum Var {
    /// A local of the function, by index.
    Local(u32),
    /// A parameter's own variable, which lives for the call.
    Param(u32),
    /// What argument `i` is: for a `T&` or slice parameter the caller's storage, for a
    /// value parameter the roots of the references it holds.
    Formal(u32),
    /// The references stored inside what `Formal(i)` names.
    FormalHeld(u32),
    /// The object a method is called on.
    This,
    /// The references stored inside it.
    ThisHeld,
    /// A static field, by index.
    Static(u32),
    /// Static data: string literals.
    Literal,
    /// A temporary, which lives until the end of its statement, by span.
    Temp(TempKind, u32, u32),
    /// What the caller of a lambda that isn't passed straight to a built-in passed for its
    /// parameter, the local with this index: it lasts the call, and no longer for sure.
    Arg(u32),
}

impl Var {
    fn is_formal(self) -> bool {
        matches!(
            self,
            Var::Formal(_) | Var::FormalHeld(_) | Var::This | Var::ThisHeld
        )
    }

    fn is_cell(self) -> bool {
        matches!(self, Var::Local(_) | Var::Param(_))
    }
}

/// A field of a class that is storage of its own for rule R: a root's growable field,
/// on its own data stack (DESIGN §6.6), or any class's field holding an array that can
/// remove or move elements (REFERENCES §5.3, §5.4). Named by its class, so it means the
/// same field wherever it ends up: reached through a nested value, or mapped from a
/// callee's parameter to the caller's argument.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
struct Part {
    class: ClassId,
    field: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
struct Root {
    var: Var,
    /// A part of what the variable holds, when the reference is known to point into it.
    part: Option<Part>,
    /// A reference to a whole array or map, rather than into its elements: it survives
    /// removals (REFERENCES §5.3, §7.1).
    whole: bool,
}

impl Root {
    fn of(var: Var) -> Root {
        Root {
            var,
            part: None,
            whole: false,
        }
    }

    /// Whether a reference rooted at `self` may point into the elements of `other`. A
    /// reference into a field's own storage is rooted at that part, however the object was
    /// reached (§5.3), so one with no part, such as a reference to the object itself,
    /// doesn't point into any part's elements.
    fn overlaps(&self, other: &Root) -> bool {
        !self.whole && self.var == other.var && (other.part.is_none() || self.part == other.part)
    }
}

type Roots = BTreeSet<Root>;

/// What a link refers into, and where it must point (rule L1).
type RootPair = (Root, Root);

/// A root set, as an expression evaluated once the root sets of the variables and
/// the summaries of the functions are known.
#[derive(Clone, Debug)]
enum R {
    Empty,
    Root(Root),
    /// The roots of the references stored in the storage these roots name.
    Stored(Box<R>),
    /// A part of the storage these roots name.
    Part(Box<R>, Part),
    /// Result `k` of a call.
    Call(Rc<CallR>, usize),
    Union(Vec<R>),
    /// A reference to the whole container these roots name.
    Whole(Box<R>),
    /// A reference into the elements of the storage these roots name.
    Elems(Box<R>),
}

impl R {
    fn root(v: Var) -> R {
        R::Root(Root::of(v))
    }

    /// Where the array these roots name, a field's or its elements, is held: the value
    /// whose field it is. An array of links points into the array that value is in.
    fn container(r: &R) -> R {
        match r {
            R::Part(inner, _) => (**inner).clone(),
            R::Elems(inner) => R::container(inner),
            R::Union(v) => R::Union(v.iter().map(R::container).collect()),
            r => r.clone(),
        }
    }

    fn stored(r: R) -> R {
        match r {
            R::Empty => R::Empty,
            r => R::Stored(Box::new(r)),
        }
    }

    fn union(a: R, b: R) -> R {
        match (a, b) {
            (R::Empty, x) | (x, R::Empty) => x,
            (R::Union(mut v), x) => {
                v.push(x);
                R::Union(v)
            }
            (a, b) => R::Union(vec![a, b]),
        }
    }

    fn temp(kind: TempKind, s: Span) -> R {
        R::root(Var::Temp(kind, s.start, s.end))
    }
}

#[derive(Debug)]
struct CallR {
    /// The functions it may call; empty when unknown, which roots its result at every argument.
    callees: Vec<FnId>,
    recv: Option<R>,
    /// For a method called on a temporary value, such as `cfg.get("a").text()`: the
    /// references the temporary holds, which its `ThisHeld` stands for, where its own
    /// storage dies with the statement.
    recv_held: Option<R>,
    args: Vec<R>,
    /// For a `new` built in place in an array, as a value with self-relative links is:
    /// that array's storage, which `This` names for rule L1 (REFERENCES §7.4).
    into: Option<R>,
    /// A call through a function value, with this many arguments: it may run any lambda
    /// that takes as many (REFERENCES §4.3).
    value: Option<usize>,
}

// ---------------------------------------------------------------- the flow form

enum Node {
    Ev(Ev),
    Block {
        body: Vec<Node>,
        defers: Vec<Node>,
    },
    If(Vec<Node>, Vec<Node>),
    /// Arms, and whether one always runs.
    Switch(Vec<Vec<Node>>, bool),
    Loop(Box<Loop>),
    Break(Option<String>),
    Continue(Option<String>),
    Return,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// After `pre`, as in `while`.
    Pre,
    /// After `post`, as in `do ... while`.
    Post,
    /// Only by `break`.
    Never,
}

struct Loop {
    label: Option<String>,
    pre: Vec<Node>,
    body: Vec<Node>,
    /// Where `continue` goes: a `for` loop's update, a `do` loop's condition.
    post: Vec<Node>,
    exit: Exit,
    /// A for-each keeps a position in what it loops over.
    holder: Option<(R, Span)>,
}

enum Ev {
    Use(Var),
    Def(Var),
    /// References from `src` stored into the storage `dst` names. `cell` when `dst` is a
    /// variable's own storage rather than a field or an element of something.
    Store {
        dst: R,
        src: R,
        cell: bool,
        span: Span,
    },
    Remove {
        target: R,
        span: Span,
        what: String,
    },
    /// A change to a capped array whose elements can move (REFERENCES §5.4).
    Change {
        target: R,
        span: Span,
        what: String,
        kind: Change,
    },
    Call(CallEv),
    Ret {
        values: Vec<R>,
        span: Span,
    },
    /// What a lambda that isn't passed straight to a built-in returns, which must
    /// outlive it: not a temporary, nor a local of its own, in scope `scope`.
    LambdaRet {
        value: R,
        scope: u32,
        span: Span,
    },
    /// What one call of a lambda passed to `fold` returns, passed to the next as its first
    /// parameter: a store for the root sets, inside one statement, so nothing to check.
    Carry {
        dst: R,
        src: R,
    },
    /// Elements added to the storage `target` names: harmless for references into it,
    /// which survive growth, but not while a value is built in place in it (V4).
    Grow {
        target: R,
    },
    /// Something these rules can't follow, refused with this message and help.
    Refuse {
        span: Span,
        msg: String,
        help: String,
    },
    /// A link to what `src` names stored in a field, which must point into the storage
    /// `dst` names (REFERENCES §7.4, L1): the array the field is in, for a self-relative
    /// link, or its static array, for a pool link (`pool`).
    Link {
        dst: R,
        src: R,
        pool: bool,
        span: Span,
    },
}

struct CallEv {
    call: Rc<CallR>,
    span: Span,
    name: String,
    /// Variables the arguments read: the callee uses them after anything it removes.
    uses: Vec<Var>,
    /// Growable locals in scope, for §8.4.
    in_scope: Vec<u32>,
    /// Arguments passed by value to a growable parameter, for §8.4.
    by_value: Vec<Span>,
    /// A constructor building a new value, whose fields are holders (§5.3).
    is_new: bool,
}

struct LocalVar {
    name: String,
    ty: Ty,
    span: Span,
    scope: Vec<u32>,
    order: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FnKind {
    Method,
    Ctor,
    DefaultCtor,
}

struct Func {
    class: ClassId,
    file: usize,
    /// Where it is declared, for an error that names the whole function.
    span: Span,
    name: String,
    kind: FnKind,
    params: Vec<(String, Ty)>,
    locals: Vec<LocalVar>,
    body: Vec<Node>,
}

/// What an event does to references into an array, for rule R.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Change {
    /// Removes elements: every reference and slice into it ends.
    Remove,
    /// An insert that can move elements: every reference and slice into it ends.
    Move,
    /// A slice, a sort or a comparison that makes a ring or gap buffer one block: the
    /// references to its elements end; slices stay right, as nothing moves until the
    /// next change.
    Rearrange,
}

#[derive(Clone, Default, PartialEq, Debug)]
struct Summary {
    /// Per result: the formal roots it may have.
    results: Vec<Roots>,
    /// Pairs (a, b): references from formal a are stored into storage formal b names.
    stores: BTreeSet<(Root, Root)>,
    /// Formal storage it may remove from.
    removes: Roots,
    /// Formal storage whose elements it may move by inserting, or rearrange.
    moves: Roots,
    rearranges: Roots,
    /// Formal storage it may add elements to.
    grows: Roots,
    /// (a, b, how): a reference rooted at formal a is still used after the function
    /// removes from, moves or rearranges formal b. Fine while they are different storage;
    /// a call whose arguments make them the same is an error (REFERENCES §4.2).
    uses_after: BTreeSet<(Root, Root, Change)>,
    /// Rule L1 for the callers: (a, b, pool) when a link to what formal a names is stored
    /// where it must point into what b names.
    links: BTreeSet<(Root, Root, bool)>,
}

// ---------------------------------------------------------------- entry

/// Runs the checks on a program that type-checked without errors.
pub fn check(prog: &Program, facts: &Facts, sources: &[SourceFile]) -> Vec<(usize, Diagnostic)> {
    let mut an = Analysis::new(prog, facts, sources);
    an.build();
    an.solve();
    an.report();
    an.worker_statics();
    for (file, start, end) in an.gave_up.take() {
        an.error(
            file,
            Span { start, end },
            "the lifetime checker didn't settle on this function, so it can't be checked".into(),
            "split it into smaller functions (REFERENCES §4.3)".into(),
        );
    }
    an.diags
}

/// Rounds after which a fixpoint is taken not to settle.
const LIMIT: usize = 100_000;

struct Analysis<'p, 'a> {
    prog: &'p Program<'a>,
    facts: &'p Facts,
    sources: &'p [SourceFile],
    funcs: Vec<Func>,
    /// (class, is constructor, index) to function.
    fn_of: HashMap<(ClassId, bool, usize), FnId>,
    static_of: HashMap<(ClassId, usize), u32>,
    statics: Vec<(ClassId, usize)>,
    summaries: Vec<Summary>,
    static_rs: HashMap<u32, Roots>,
    diags: Vec<(usize, Diagnostic)>,
    reported: HashSet<(usize, u32, u32)>,
    /// Temporaries already reported as stored where they can't be, by file: what then
    /// holds one, and what is read from that, would only repeat the error.
    temps_reported: std::cell::RefCell<HashSet<(usize, Var)>>,
    holds_memo: std::cell::RefCell<HashMap<(ClassId, String), bool>>,
    /// Functions whose analysis didn't settle (`too_complex`).
    gave_up: std::cell::RefCell<BTreeSet<(usize, u32, u32)>>,
    /// Each lambda that isn't passed straight to a built-in: the function it is in, and
    /// the locals that are its parameters, in order.
    lambdas: std::cell::RefCell<Vec<(FnId, Vec<u32>)>>,
    /// What a call through a function value with this many arguments may do: the union
    /// of what each such lambda does to its parameters.
    value_sums: HashMap<usize, Summary>,
}

impl<'p, 'a> Analysis<'p, 'a> {
    fn new(prog: &'p Program<'a>, facts: &'p Facts, sources: &'p [SourceFile]) -> Self {
        let mut static_of = HashMap::new();
        let mut statics = Vec::new();
        for (c, ci) in prog.classes.iter().enumerate() {
            for (i, f) in ci.fields.iter().enumerate() {
                if f.is_static {
                    static_of.insert((c, i), statics.len() as u32);
                    statics.push((c, i));
                }
            }
        }
        Analysis {
            prog,
            facts,
            sources,
            funcs: Vec::new(),
            fn_of: HashMap::new(),
            static_of,
            statics,
            summaries: Vec::new(),
            static_rs: HashMap::new(),
            diags: Vec::new(),
            reported: HashSet::new(),
            temps_reported: Default::default(),
            holds_memo: Default::default(),
            gave_up: Default::default(),
            lambdas: Default::default(),
            value_sums: HashMap::new(),
        }
    }

    // ------------------------------------------------------------ types

    /// Whether a value of this type holds references, so it has roots.
    fn holds_refs(&self, t: &Ty) -> bool {
        self.holds_with(t, &|me, id, args| me.class_holds(id, args))
    }

    /// `holds_refs`, with `class` deciding for a class type.
    fn holds_with(&self, t: &Ty, class: &dyn Fn(&Self, ClassId, &[Ty]) -> bool) -> bool {
        match t {
            Ty::Ref(_)
            | Ty::Nullable(_)
            | Ty::Slice(_)
            | Ty::View(_)
            | Ty::Func(..)
            | Ty::MapEntry(..) => true,
            Ty::Param(_) | Ty::Unknown => true,
            Ty::Array(e, _) | Ty::Set(e) | Ty::Pool(e) => self.holds_with(e, class),
            Ty::Map(k, v) => self.holds_with(k, class) || self.holds_with(v, class),
            Ty::Tuple(ts) => ts.iter().any(|t| self.holds_with(t, class)),
            Ty::Class(id, args) => {
                let c = &self.prog.classes[*id];
                if c.kind == ClassKind::Enum || c.kind == ClassKind::Script {
                    return false;
                }
                class(self, *id, args)
            }
            _ => false,
        }
    }

    /// Whether a value of class `id` with these type arguments holds references. Classes
    /// can contain each other, so this is worked out for every class a value of it may
    /// contain at once, starting from "no" and repeating until nothing changes.
    fn class_holds(&self, id: ClassId, args: &[Ty]) -> bool {
        let key = |id: ClassId, args: &[Ty]| (id, format!("{:?}", args));
        if let Some(b) = self.holds_memo.borrow().get(&key(id, args)) {
            return *b;
        }
        // Every class instance reachable through fields held by value.
        let mut found: Vec<(ClassId, Vec<Ty>)> = vec![(id, args.to_vec())];
        let mut fields: Vec<Vec<Ty>> = Vec::new();
        let mut i = 0;
        while i < found.len() {
            let (c, a) = found[i].clone();
            let ci = &self.prog.classes[c];
            let map: Vec<(String, Ty)> = ci
                .type_params
                .iter()
                .map(|p| p.name.clone())
                .zip(a.iter().cloned())
                .collect();
            let tys: Vec<Ty> = ci
                .fields
                .iter()
                .filter(|f| !f.is_static)
                .map(|f| f.ty.subst(&map))
                .collect();
            for t in &tys {
                classes_in(t, &mut |c2, a2| {
                    if !found.iter().any(|(x, y)| *x == c2 && y.as_slice() == a2) {
                        found.push((c2, a2.to_vec()));
                    }
                });
            }
            fields.push(tys);
            i += 1;
            // Type arguments that keep growing: assume the worst.
            if found.len() > 1000 {
                return true;
            }
        }
        let mut val = vec![false; found.len()];
        loop {
            let mut changed = false;
            for k in 0..found.len() {
                if val[k] {
                    continue;
                }
                let c = &self.prog.classes[found[k].0];
                let b = c.kind == ClassKind::Interface
                    || fields[k].iter().any(|t| {
                        self.holds_with(t, &|_, c2, a2| {
                            found
                                .iter()
                                .position(|(x, y)| *x == c2 && y.as_slice() == a2)
                                .is_some_and(|j| val[j])
                        })
                    });
                if b {
                    val[k] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut memo = self.holds_memo.borrow_mut();
        for ((c, a), v) in found.iter().zip(&val) {
            memo.insert(key(*c, a), *v);
        }
        val[0]
    }

    /// A reference to a whole container: `T[..>]&`, `hashmap<K, V>&`.
    fn whole_ref(t: &Ty) -> bool {
        let inner = match t {
            Ty::Ref(x) => &**x,
            Ty::Nullable(x) => match &**x {
                Ty::Ref(y) => &**y,
                _ => return false,
            },
            _ => return false,
        };
        matches!(
            inner,
            Ty::Array(..) | Ty::Map(..) | Ty::Set(_) | Ty::Pool(_)
        )
    }

    /// The types a value place converts to by reference: references and slices.
    fn refish(t: &Ty) -> bool {
        matches!(t, Ty::Ref(_) | Ty::Slice(_) | Ty::View(_))
            || matches!(t, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)))
    }

    /// An array or map that can lose elements, or whose elements can move, so references
    /// into it can't be stored (REFERENCES §5.3, §5.4).
    fn removable_ty(t: &Ty) -> bool {
        match t.unref() {
            Ty::Array(_, k) => {
                k.pop_end() || k.pop_front() || k.cursor_remove() || k.moves_elements()
            }
            Ty::Map(..) | Ty::Set(_) => true,
            _ => false,
        }
    }

    /// Exempt from rule R: pools (REFERENCES §5.4), whose slots hold a value of their
    /// type even once freed. Not an array inside an element, which can lose elements.
    fn exempt_ty(t: &Ty) -> bool {
        matches!(t.unref(), Ty::Pool(el) if !Self::removable_ty(el))
    }

    /// The fields a value of `t` holds, itself or in values it holds, that are storage of
    /// their own (`Part`): replacing the value removes their elements.
    fn parts_of(&self, t: &Ty) -> Vec<Part> {
        let mut out = Vec::new();
        let mut seen = Vec::new();
        self.parts_into(t, &mut out, &mut seen);
        out
    }

    fn parts_into(&self, t: &Ty, out: &mut Vec<Part>, seen: &mut Vec<ClassId>) {
        match t {
            Ty::Array(e, _) => self.parts_into(e, out, seen),
            Ty::Tuple(ts) => ts.iter().for_each(|t| self.parts_into(t, out, seen)),
            Ty::Class(id, _) => {
                let c = &self.prog.classes[*id];
                if c.kind == ClassKind::Enum || seen.contains(id) {
                    return;
                }
                seen.push(*id);
                for (i, f) in c.fields.iter().enumerate().filter(|(_, f)| !f.is_static) {
                    if c.kind == ClassKind::Root && starts_empty(&f.ty) || Self::removable_ty(&f.ty)
                    {
                        out.push(Part {
                            class: *id,
                            field: i as u32,
                        });
                    }
                    self.parts_into(&f.ty, out, seen);
                }
            }
            _ => {}
        }
    }

    /// Resizable or variable-size: needs a data stack of its own (§8.1, §8.4).
    fn resizable_ty(&self, t: &Ty) -> bool {
        match t {
            Ty::Array(_, k) => k.cap == CapKind::None,
            Ty::Map(..) | Ty::Set(_) | Ty::Pool(_) => true,
            // A variable-size record lives on a data stack of its own too.
            Ty::Class(id, _) => {
                self.prog.classes[*id].kind == ClassKind::Root || self.prog.variable_size(t)
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------ building the flow form

    fn build(&mut self) {
        let prog = self.prog;
        // Every function's id first, so a call to one declared further on finds it: its
        // result's roots and what it stores come from its summary (§4.2).
        let mut next = 0;
        for (c, ci) in prog.classes.iter().enumerate() {
            for (i, m) in ci.methods.iter().enumerate() {
                if m.decl.body.is_some() {
                    self.fn_of.insert((c, false, i), next);
                    next += 1;
                }
            }
            for i in 0..ci.ctors.len() {
                self.fn_of.insert((c, true, i), next);
                next += 1;
            }
        }
        for (c, ci) in prog.classes.iter().enumerate() {
            for (i, m) in ci.methods.iter().enumerate() {
                let Some(body) = &m.decl.body else { continue };
                let params: Vec<(String, Ty)> = m
                    .decl
                    .params
                    .iter()
                    .zip(&m.params)
                    .map(|(p, t)| (name_of(&p.name), t.clone()))
                    .collect();
                debug_assert_eq!(self.fn_of.get(&(c, false, i)), Some(&self.funcs.len()));
                let mut b = Builder::new(self, !m.is_static);
                b.ret = m.ret.clone();
                b.fn_id = self.funcs.len();
                let f = b.function(&m.decl.params, body);
                self.funcs.push(Func {
                    class: c,
                    file: ci.file,
                    span: m.decl.name.span,
                    name: m.name.clone(),
                    kind: FnKind::Method,
                    params,
                    locals: f.0,
                    body: f.1,
                });
            }
            for (i, k) in ci.ctors.iter().enumerate() {
                debug_assert_eq!(self.fn_of.get(&(c, true, i)), Some(&self.funcs.len()));
                match k.decl {
                    Some(d) => {
                        let params = d
                            .params
                            .iter()
                            .zip(&k.params)
                            .map(|(p, t)| (name_of(&p.name), t.clone()))
                            .collect();
                        let mut b = Builder::new(self, true);
                        b.inits = Some(c);
                        b.fn_id = self.funcs.len();
                        let f = b.function(&d.params, &d.body);
                        self.funcs.push(Func {
                            class: c,
                            file: ci.file,
                            span: d.span,
                            name: format!("new {}", ci.name),
                            kind: FnKind::Ctor,
                            params,
                            locals: f.0,
                            body: f.1,
                        });
                    }
                    None => {
                        let params = k
                            .params
                            .iter()
                            .enumerate()
                            .map(|(i, t)| (format!("argument {}", i + 1), t.clone()))
                            .collect();
                        // Its body is the field initializers.
                        let mut b = Builder::new(self, true);
                        b.inits = Some(c);
                        b.fn_id = self.funcs.len();
                        let f = b.function(
                            &[],
                            &ast::Block {
                                stmts: Vec::new(),
                                span: ci.span,
                            },
                        );
                        self.funcs.push(Func {
                            class: c,
                            file: ci.file,
                            span: ci.span,
                            name: format!("new {}", ci.name),
                            kind: FnKind::DefaultCtor,
                            params,
                            locals: f.0,
                            body: f.1,
                        });
                    }
                }
            }
        }
        // Each class's static initializers, as a function of their own: what they store
        // goes into the statics' root sets, and a temporary can't be kept (§3.3).
        for (c, ci) in prog.classes.iter().enumerate() {
            if !ci.fields.iter().any(|f| f.is_static && f.init.is_some()) {
                continue;
            }
            let mut b = Builder::new(self, false);
            b.fn_id = self.funcs.len();
            let f = b.statics(c);
            self.funcs.push(Func {
                class: c,
                file: ci.file,
                span: ci.span,
                name: format!("the static initializers of `{}`", ci.name),
                kind: FnKind::Method,
                params: Vec::new(),
                locals: f.0,
                body: f.1,
            });
        }
        self.summaries = vec![Summary::default(); self.funcs.len()];
    }

    // ------------------------------------------------------------ evaluating root sets

    fn stored(&self, rs: &HashMap<Var, Roots>, root: &Root) -> Roots {
        let mut out: Roots = rs.get(&root.var).cloned().unwrap_or_default();
        match root.var {
            Var::Formal(i) | Var::FormalHeld(i) => {
                out.insert(Root::of(Var::FormalHeld(i)));
            }
            Var::This | Var::ThisHeld => {
                out.insert(Root::of(Var::ThisHeld));
            }
            Var::Static(id) => out.extend(self.static_rs.get(&id).cloned().unwrap_or_default()),
            Var::Temp(..) => {
                out.insert(*root);
            }
            // What the caller's value holds is the caller's too.
            Var::Arg(_) => {
                out.insert(Root::of(root.var));
            }
            _ => {}
        }
        out
    }

    fn eval(&self, rs: &HashMap<Var, Roots>, r: &R) -> Roots {
        match r {
            R::Empty => Roots::new(),
            R::Root(x) => [*x].into_iter().collect(),
            R::Stored(inner) => self
                .eval(rs, inner)
                .iter()
                .flat_map(|x| self.stored(rs, x))
                .collect(),
            R::Part(inner, p) => self
                .eval(rs, inner)
                .into_iter()
                .map(|x| {
                    if x.part.is_none() && !matches!(x.var, Var::Temp(..) | Var::Literal) {
                        Root {
                            part: Some(*p),
                            ..x
                        }
                    } else {
                        x
                    }
                })
                .collect(),
            R::Call(c, k) => self.call_result(rs, c, *k),
            R::Union(v) => v.iter().flat_map(|x| self.eval(rs, x)).collect(),
            R::Whole(inner) => self
                .eval(rs, inner)
                .into_iter()
                .map(|x| Root { whole: true, ..x })
                .collect(),
            R::Elems(inner) => self
                .eval(rs, inner)
                .into_iter()
                .map(|x| Root { whole: false, ..x })
                .collect(),
        }
    }

    fn call_result(&self, rs: &HashMap<Var, Roots>, c: &CallR, k: usize) -> Roots {
        if c.callees.is_empty() {
            let mut out = Roots::new();
            if let Some(r) = &c.recv {
                out.extend(self.eval(rs, r));
            }
            for a in &c.args {
                out.extend(self.eval(rs, a));
            }
            return out;
        }
        let mut out = Roots::new();
        for &f in &c.callees {
            if let Some(res) = self.summaries[f].results.get(k) {
                for fr in res {
                    out.extend(self.map_formal(rs, c, fr));
                }
            }
        }
        out
    }

    /// A callee's formal root, as roots of the caller.
    fn map_formal(&self, rs: &HashMap<Var, Roots>, c: &CallR, fr: &Root) -> Roots {
        let base: Roots = match fr.var {
            Var::Formal(i) => c
                .args
                .get(i as usize)
                .map(|a| self.eval(rs, a))
                .unwrap_or_default(),
            Var::FormalHeld(i) => c
                .args
                .get(i as usize)
                .map(|a| self.eval(rs, a))
                .unwrap_or_default()
                .iter()
                .flat_map(|x| self.stored(rs, x))
                .collect(),
            Var::This => c
                .recv
                .as_ref()
                .map(|r| self.eval(rs, r))
                .unwrap_or_default(),
            Var::ThisHeld => match &c.recv_held {
                Some(h) => self.eval(rs, h),
                None => c
                    .recv
                    .as_ref()
                    .map(|r| self.eval(rs, r))
                    .unwrap_or_default()
                    .iter()
                    .flat_map(|x| self.stored(rs, x))
                    .collect(),
            },
            _ => return [*fr].into_iter().collect(),
        };
        // A reference into the elements of what the argument names isn't a reference to the whole of it.
        let base = base.into_iter().map(|x| Root {
            whole: x.whole && fr.whole,
            ..x
        });
        match fr.part {
            None => base.collect(),
            Some(p) => base
                .map(|x| {
                    if x.part.is_none() {
                        Root { part: Some(p), ..x }
                    } else {
                        x
                    }
                })
                .collect(),
        }
    }

    /// Field `i` of class `c`, when it is a link: `None` for a self-relative one, or the
    /// index of the static array a pool link is measured from.
    fn link_of_field(&self, c: ClassId, i: usize) -> Option<Option<u32>> {
        let f = &self.prog.classes[c].fields[i];
        // An array of links: each element points into the array its value is in.
        if f.link.is_some() || f.elem_link.is_some() {
            return Some(None);
        }
        f.pool_link.as_ref()?;
        let at = self.facts.pool_arrays.get(&(c, i))?;
        Some(Some(*self.static_of.get(at)?))
    }

    /// A callee's formal root, for rule L1: as `map_formal`, except that the object a
    /// constructor builds in place in an array is that array (CallR::into).
    fn map_link(&self, rs: &HashMap<Var, Roots>, c: &CallR, fr: &Root) -> Roots {
        if fr.var == Var::This && c.recv.is_none() {
            return match &c.into {
                Some(r) => self.eval(rs, r),
                None => Roots::new(),
            };
        }
        self.map_formal(rs, c, fr)
    }

    /// The pairs of roots rule L1 compares for one link: each root `s` of what it refers
    /// to with each root `d` of where it must point. The ones that name the caller's data
    /// are the callers' to check, through the summary; the rest are checked here.
    fn link_pairs(s: &Roots, d: &Roots) -> (Vec<RootPair>, Vec<RootPair>) {
        let (mut here, mut callers) = (Vec::new(), Vec::new());
        let plain = |r: &Root| Root { whole: false, ..*r };
        for a in s {
            for b in d {
                let (a, b) = (plain(a), plain(b));
                if a.var == b.var && a.part == b.part {
                    continue;
                }
                if a.var.is_formal() || b.var.is_formal() {
                    callers.push((a, b));
                } else {
                    here.push((a, b));
                }
            }
        }
        (here, callers)
    }

    /// Rule L1 (REFERENCES §7.4): a self-relative link points into the array it is stored
    /// in, a pool link into its static array.
    #[allow(clippy::too_many_arguments)]
    fn link_rule(
        &self,
        f: &Func,
        s: &Roots,
        d: &Roots,
        pool: bool,
        span: Span,
        errs: &mut Vec<(Span, String, String)>,
    ) {
        let places: BTreeSet<(Var, Option<Part>)> = d
            .iter()
            .filter(|r| !r.var.is_formal())
            .map(|r| (r.var, r.part))
            .collect();
        if places.len() > 1 && !pool && !s.is_empty() {
            let names: Vec<String> = d.iter().map(|r| self.root_name(f, r)).collect();
            errs.push((
                span,
                format!("can't tell which array this link is stored in: it may be {}", names.join(" or ")),
                "a self-relative link must point into the array it is in, so the field must be reached through one array (REFERENCES §7.4, L1)".into(),
            ));
            return;
        }
        let (here, _) = Self::link_pairs(s, d);
        if let Some((a, b)) = here.first() {
            let (what, help) = if pool {
                (
                    format!("this pool link is measured from {}, and what it would refer to is in {}", self.root_name(f, b), self.root_name(f, a)),
                    format!("a pool link refers only to elements of {} (REFERENCES §7.4, L1)", self.root_name(f, b)),
                )
            } else {
                (
                    format!("this link is in an element of {}, and what it would refer to is in {}", self.root_name(f, b), self.root_name(f, a)),
                    "a self-relative link refers only into its own array; use a plain reference, `T&?`, to refer elsewhere (REFERENCES §7.4, L1)".into(),
                )
            };
            errs.push((span, what, help));
        }
    }

    /// The root set of every variable of `f`: the union of everything stored into it (§3.5).
    fn root_sets(&self, f: &Func, statics: &mut HashMap<u32, Roots>) -> HashMap<Var, Roots> {
        let mut rs: HashMap<Var, Roots> = HashMap::new();
        for (i, (_, t)) in f.params.iter().enumerate() {
            if self.holds_refs(t) {
                let whole = Self::whole_ref(t);
                rs.entry(Var::Param(i as u32)).or_default().insert(Root {
                    whole,
                    ..Root::of(Var::Formal(i as u32))
                });
            }
        }
        let evs = events(&f.body);
        // Root sets only grow, and are finite, so this ends; the bound is a guard.
        for round in 0.. {
            if round == LIMIT {
                self.too_complex(f.file, f.span);
                break;
            }
            let mut changed = false;
            let mut add = |rs: &mut HashMap<Var, Roots>,
                           statics: &mut HashMap<u32, Roots>,
                           d: &Root,
                           src: &Roots| match d.var {
                // A static is the whole program's, so it keeps only roots that are too:
                // statics and literals. One of this function's parameters, or locals, means
                // nothing in another function; what a call stores into a static is mapped
                // to the caller's own roots through the summary's `stores` instead.
                Var::Static(id) => {
                    let e = statics.entry(id).or_default();
                    let n = e.len();
                    e.extend(
                        src.iter()
                            .copied()
                            .filter(|r| matches!(r.var, Var::Static(_) | Var::Literal)),
                    );
                    changed |= e.len() != n;
                }
                Var::Literal | Var::Temp(..) => {}
                v => {
                    let e = rs.entry(v).or_default();
                    let n = e.len();
                    e.extend(src.iter().copied());
                    changed |= e.len() != n;
                }
            };
            for ev in &evs {
                match ev {
                    Ev::Store { dst, src, .. } | Ev::Carry { dst, src } => {
                        let s = self.eval(&rs, src);
                        if s.is_empty() {
                            continue;
                        }
                        for d in self.eval(&rs, dst) {
                            add(&mut rs, statics, &d, &s);
                        }
                    }
                    Ev::Call(c) => {
                        for &g in &c.call.callees {
                            for (a, b) in self.summaries[g].stores.clone() {
                                let s = self.map_formal(&rs, &c.call, &a);
                                for d in self.map_formal(&rs, &c.call, &b) {
                                    add(&mut rs, statics, &d, &s);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }
        rs
    }

    fn summarize(&self, f: &Func, rs: &HashMap<Var, Roots>) -> Summary {
        let mut s = Summary::default();
        let keep = |r: &Root| r.var.is_formal() || matches!(r.var, Var::Static(_) | Var::Literal);
        if f.kind == FnKind::DefaultCtor {
            s.results.push(
                (0..f.params.len())
                    .map(|i| Root::of(Var::Formal(i as u32)))
                    .collect(),
            );
            s.stores = (0..f.params.len())
                .map(|i| (Root::of(Var::Formal(i as u32)), Root::of(Var::This)))
                .collect();
            // Its parameters are the fields set by argument, in order (DESIGN §6.2).
            let c = &self.prog.classes[f.class];
            let set = c
                .fields
                .iter()
                .enumerate()
                .filter(|(_, x)| !x.is_static && x.init.is_none() && !starts_empty(&x.ty));
            for (k, (fi, _)) in set.enumerate() {
                match self.link_of_field(f.class, fi) {
                    Some(None) => {
                        s.links.insert((Root::of(Var::Formal(k as u32)), Root::of(Var::This), false));
                    }
                    Some(Some(id)) => {
                        s.links.insert((
                            Root::of(Var::Formal(k as u32)),
                            Root::of(Var::Static(id)),
                            true,
                        ));
                    }
                    None => {}
                }
            }
        }
        let evs = events(&f.body);
        for ev in &evs {
            match ev {
                Ev::Ret { values, .. } => {
                    for (k, v) in values.iter().enumerate() {
                        if s.results.len() <= k {
                            s.results.resize(k + 1, Roots::new());
                        }
                        let roots: Vec<Root> = self.eval(rs, v).into_iter().filter(keep).collect();
                        s.results[k].extend(roots);
                    }
                }
                Ev::Store { dst, src, cell, .. } => {
                    if *cell {
                        continue;
                    }
                    let src = self.eval(rs, src);
                    for d in self.eval(rs, dst) {
                        if d.var.is_formal() || matches!(d.var, Var::Static(_)) {
                            for a in &src {
                                // Into the same object counts too: a view of one field
                                // kept in another ends at a removal from the first.
                                if a.var.is_formal() {
                                    s.stores.insert((*a, d));
                                }
                            }
                        }
                    }
                }
                Ev::Link { dst, src, pool, .. } => {
                    let (_, callers) = Self::link_pairs(&self.eval(rs, src), &self.eval(rs, dst));
                    for (a, b) in callers {
                        s.links.insert((a, b, *pool));
                    }
                }
                Ev::Remove { target, .. } => {
                    s.removes.extend(
                        self.eval(rs, target)
                            .into_iter()
                            .filter(|r| r.var.is_formal() || matches!(r.var, Var::Static(_))),
                    );
                }
                Ev::Grow { target } => {
                    s.grows.extend(
                        self.eval(rs, target)
                            .into_iter()
                            .filter(|r| r.var.is_formal() || matches!(r.var, Var::Static(_))),
                    );
                }
                Ev::Change { target, kind, .. } => {
                    let set = if *kind == Change::Rearrange {
                        &mut s.rearranges
                    } else {
                        &mut s.moves
                    };
                    set.extend(
                        self.eval(rs, target)
                            .into_iter()
                            .filter(|r| r.var.is_formal() || matches!(r.var, Var::Static(_))),
                    );
                }
                Ev::Call(c) => {
                    if let Some(n) = c.call.value
                        && let Some(sm) = self.value_sums.get(&n)
                    {
                        for (from, to) in [
                            (&sm.removes, &mut s.removes),
                            (&sm.moves, &mut s.moves),
                            (&sm.rearranges, &mut s.rearranges),
                            (&sm.grows, &mut s.grows),
                        ] {
                            for r in from {
                                to.extend(self.map_formal(rs, &c.call, r).into_iter().filter(
                                    |r| r.var.is_formal() || matches!(r.var, Var::Static(_)),
                                ));
                            }
                        }
                    }
                    for &g in &c.call.callees {
                        let sm = &self.summaries[g];
                        for (from, to) in [
                            (&sm.removes, &mut s.removes),
                            (&sm.moves, &mut s.moves),
                            (&sm.rearranges, &mut s.rearranges),
                            (&sm.grows, &mut s.grows),
                        ] {
                            for r in from {
                                to.extend(self.map_formal(rs, &c.call, r).into_iter().filter(
                                    |r| r.var.is_formal() || matches!(r.var, Var::Static(_)),
                                ));
                            }
                        }
                        for (a, b) in &self.summaries[g].stores {
                            let src = self.map_formal(rs, &c.call, a);
                            for d in self.map_formal(rs, &c.call, b) {
                                if d.var.is_formal() || matches!(d.var, Var::Static(_)) {
                                    for x in &src {
                                        if x.var.is_formal() {
                                            s.stores.insert((*x, d));
                                        }
                                    }
                                }
                            }
                        }
                        for (a, b, pool) in &self.summaries[g].links {
                            let src = self.map_link(rs, &c.call, a);
                            let dst = self.map_link(rs, &c.call, b);
                            let (_, callers) = Self::link_pairs(&src, &dst);
                            for (x, y) in callers {
                                s.links.insert((x, y, *pool));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if matches!(f.kind, FnKind::Ctor | FnKind::DefaultCtor) {
            // What a constructor builds holds whatever it stored into `this`, and a
            // default one its arguments.
            let mut held: Roots = rs
                .get(&Var::This)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|r| keep(r) && !matches!(r.var, Var::This | Var::ThisHeld))
                .collect();
            if let Some(r) = s.results.first() {
                held.extend(r.iter().copied());
            }
            s.results = vec![held];
        }
        s
    }

    /// Summaries for every function, and root sets for statics, by repeating until nothing changes (§4.3).
    fn solve(&mut self) {
        for round in 0.. {
            if round == LIMIT {
                if let Some(f) = self.funcs.first() {
                    self.too_complex(f.file, f.span);
                }
                break;
            }
            let mut changed = false;
            let mut statics = self.static_rs.clone();
            let mut all_rs = Vec::new();
            for i in 0..self.funcs.len() {
                let rs = self.root_sets(&self.funcs[i], &mut statics);
                let s = self.summarize(&self.funcs[i], &rs);
                if s != self.summaries[i] {
                    self.summaries[i] = s;
                    changed = true;
                }
                all_rs.push(rs);
            }
            let vs = self.value_summaries(&all_rs);
            if vs != self.value_sums {
                self.value_sums = vs;
                changed = true;
            }
            if statics != self.static_rs {
                self.static_rs = statics;
                changed = true;
            }
            if !changed {
                break;
            }
        }
        // Then what each function still uses after a change, which nothing above depends
        // on: through the same walk as rule R, and through what its callees pass on.
        let mut statics = self.static_rs.clone();
        let all_rs: Vec<HashMap<Var, Roots>> = (0..self.funcs.len())
            .map(|i| self.root_sets(&self.funcs[i], &mut statics))
            .collect();
        for round in 0.. {
            if round == LIMIT {
                break;
            }
            let mut changed = false;
            for (i, rs) in all_rs.iter().enumerate() {
                let f = &self.funcs[i];
                let mut ctx = LiveCtx::default();
                let mut scratch = Vec::new();
                let start = Live::new();
                self.live(
                    f,
                    rs,
                    &self.static_rs,
                    &f.body,
                    &start,
                    &mut ctx,
                    true,
                    &mut scratch,
                );
                if ctx.pairs != self.summaries[i].uses_after {
                    self.summaries[i].uses_after = ctx.pairs;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// The summaries a call may run: its callees', and for a call through a function
    /// value, what any lambda with as many parameters does.
    fn sums<'s>(&'s self, c: &CallR) -> Vec<&'s Summary> {
        let mut v: Vec<&Summary> = c.callees.iter().map(|&g| &self.summaries[g]).collect();
        if let Some(n) = c.value
            && let Some(s) = self.value_sums.get(&n)
        {
            v.push(s);
        }
        v
    }

    /// What calls through function values may do: for each lambda not passed straight to a
    /// built-in, the removals, moves and growth its parameters' storage may see, as the
    /// formals of a function with as many parameters (REFERENCES §4.3).
    fn value_summaries(&self, all_rs: &[HashMap<Var, Roots>]) -> HashMap<usize, Summary> {
        let mut out: HashMap<usize, Summary> = HashMap::new();
        for (fid, locals) in self.lambdas.borrow().iter() {
            let (f, rs) = (&self.funcs[*fid], &all_rs[*fid]);
            let s = out.entry(locals.len()).or_default();
            let mut note = |roots: Roots, which: usize| {
                for r in roots {
                    if let Var::Arg(l) = r.var
                        && let Some(pos) = locals.iter().position(|x| *x == l)
                    {
                        let fr = Root {
                            var: Var::Formal(pos as u32),
                            ..r
                        };
                        match which {
                            0 => s.removes.insert(fr),
                            1 => s.moves.insert(fr),
                            2 => s.rearranges.insert(fr),
                            _ => s.grows.insert(fr),
                        };
                    }
                }
            };
            for ev in events(&f.body) {
                match ev {
                    Ev::Remove { target, .. } => note(self.eval(rs, target), 0),
                    Ev::Change { target, kind, .. } => note(
                        self.eval(rs, target),
                        if *kind == Change::Rearrange { 2 } else { 1 },
                    ),
                    Ev::Grow { target } => note(self.eval(rs, target), 3),
                    Ev::Call(c) => {
                        for sm in self.sums(&c.call) {
                            for (k, set) in [&sm.removes, &sm.moves, &sm.rearranges, &sm.grows]
                                .into_iter()
                                .enumerate()
                            {
                                for r in set {
                                    note(self.map_formal(rs, &c.call, r), k);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    // ------------------------------------------------------------ reporting

    /// The analysis didn't settle within `LIMIT` rounds: refused, never accepted unchecked.
    fn too_complex(&self, file: usize, span: Span) {
        self.gave_up
            .borrow_mut()
            .insert((file, span.start, span.end));
    }

    fn error(&mut self, file: usize, span: Span, msg: String, help: String) {
        if self.reported.insert((file, span.start, span.end)) {
            self.diags
                .push((file, Diagnostic::error(span, msg).with_help(help)));
        }
    }

    fn line(&self, file: usize, span: Span) -> usize {
        self.sources
            .get(file)
            .map_or(0, |s| s.line_col(span.start as usize).0)
    }

    fn root_name(&self, f: &Func, r: &Root) -> String {
        let base = match r.var {
            Var::Local(i) => quoted(&f.locals[i as usize].name),
            Var::Param(i) => format!("`{}`", f.params[i as usize].0),
            Var::Formal(i) | Var::FormalHeld(i) => {
                format!("what `{}` refers to", f.params[i as usize].0)
            }
            Var::This | Var::ThisHeld => "`this`".to_string(),
            Var::Static(id) => {
                let (c, i) = self.statics[id as usize];
                format!("`{}`", self.prog.classes[c].fields[i].name)
            }
            Var::Literal => "a string literal".to_string(),
            Var::Temp(..) => "a temporary".to_string(),
            Var::Arg(i) => format!(
                "what the lambda's caller passed for `{}`",
                f.locals[i as usize].name
            ),
        };
        let Some(p) = r.part else { return base };
        let holder = match r.var {
            Var::Local(_) | Var::Param(_) | Var::This | Var::Static(_) => base.clone(),
            Var::Formal(i) => format!("`{}`", f.params[i as usize].0),
            _ => return base,
        };
        let field = &self.prog.classes[p.class].fields[p.field as usize].name;
        let whole = Root { part: None, ..*r };
        if matches!(self.root_ty(f, &whole), Some(Ty::Class(c, _)) if c == p.class) {
            format!("`{}.{}`", holder.trim_matches('`'), field)
        } else {
            // Reached through a value nested in it.
            format!(
                "the `{}` of a `{}` in {}",
                field, self.prog.classes[p.class].name, holder
            )
        }
    }

    /// The type of the storage a root names, if known.
    fn root_ty(&self, f: &Func, r: &Root) -> Option<Ty> {
        let whole = match r.var {
            Var::Local(i) => f.locals[i as usize].ty.unref().clone(),
            Var::Param(i) | Var::Formal(i) => f.params[i as usize].1.unref().clone(),
            Var::This => Ty::Class(f.class, Vec::new()),
            Var::Arg(i) => f.locals[i as usize].ty.unref().clone(),
            Var::Static(id) => {
                let (c, i) = self.statics[id as usize];
                self.prog.classes[c].fields[i].ty.clone()
            }
            _ => return None,
        };
        match r.part {
            None => Some(whole),
            Some(p) => self.prog.classes[p.class]
                .fields
                .get(p.field as usize)
                .map(|f| f.ty.clone()),
        }
    }

    /// Threads (STDLIB §10.3): a new thread has statics of its own, copies of those the
    /// code generator can copy (`Program::thread_copyable`); the others start empty. So
    /// a spawned lambda, and everything it may call, may reach only the copied ones, and of
    /// those that hold views only ones pointing at string literals, which every thread
    /// shares. A view into a static array would point into the spawning thread's copy.
    /// A call through a function value may reach any lambda.
    fn worker_statics(&mut self) {
        let prog = self.prog;
        let facts = self.facts;
        let mut spawns: Vec<(usize, &Expr)> = Vec::new();
        let mut lambdas: Vec<(usize, &Expr)> = Vec::new();
        for c in &prog.classes {
            for m in &c.methods {
                if let Some(b) = &m.decl.body {
                    for s in &b.stmts {
                        visit_stmt(s, &mut |e| {
                            find_spawns(facts, prog, c.file, e, &mut spawns, &mut lambdas)
                        });
                    }
                }
            }
            for k in &c.ctors {
                if let Some(d) = k.decl {
                    for s in &d.body.stmts {
                        visit_stmt(s, &mut |e| {
                            find_spawns(facts, prog, c.file, e, &mut spawns, &mut lambdas)
                        });
                    }
                }
            }
        }
        // The iterators of for-each loops, whose `next()` and `iterator()` they call.
        let mut loops: HashSet<(u32, u32)> = HashSet::new();
        for c in &prog.classes {
            let bodies = c
                .methods
                .iter()
                .filter_map(|m| m.decl.body.as_ref())
                .chain(c.ctors.iter().filter_map(|k| k.decl.map(|d| &d.body)));
            for b in bodies {
                for st in &b.stmts {
                    foreach_iters(st, &mut loops);
                }
            }
        }
        for (file, lam) in spawns {
            let mut seen: HashSet<(ClassId, bool, usize)> = HashSet::new();
            let mut work: Vec<(usize, &Expr)> = vec![(file, lam)];
            let mut stmts: Vec<(usize, &Stmt)> = Vec::new();
            let mut reached: Vec<(u32, usize, Span)> = Vec::new();
            let mut all_lambdas = false;
            loop {
                let mut calls: Vec<(ClassId, bool, usize)> = Vec::new();
                let mut value_call = false;
                let mut note = |file: usize, e: &Expr, reached: &mut Vec<(u32, usize, Span)>| {
                    if let Some(id) = static_named(facts, prog, &self.static_of, e)
                        && !reached.iter().any(|r| r.0 == id)
                    {
                        reached.push((id, file, e.span));
                    }
                    if loops.contains(&key(e.span))
                        && let Some(Ty::Class(c, _)) =
                            facts.types.get(&key(e.span)).map(|t| t.unref())
                    {
                        iter_methods(prog, *c, &mut calls);
                    }
                    if let E::Call { .. } | E::New { .. } = &e.kind
                        && let Some(f) = facts.calls.get(&key(e.span))
                    {
                        match f.origin {
                            Origin::Method(c, i) => callees(prog, c, i, &mut calls),
                            Origin::Ctor(c, i) => calls.push((c, true, i)),
                            Origin::Value => value_call = true,
                            _ => {}
                        }
                    }
                };
                if let Some((f, e)) = work.pop() {
                    visit_expr(e, &mut |x| note(f, x, &mut reached));
                } else if let Some((f, s)) = stmts.pop() {
                    visit_stmt(s, &mut |x| note(f, x, &mut reached));
                } else {
                    break;
                }
                if value_call && !all_lambdas {
                    all_lambdas = true;
                    work.extend(lambdas.iter().copied());
                }
                for (c, ctor, i) in calls {
                    if !seen.insert((c, ctor, i)) {
                        continue;
                    }
                    let ci = &prog.classes[c];
                    if ctor {
                        if let Some(d) = ci.ctors.get(i).and_then(|k| k.decl) {
                            stmts.extend(d.body.stmts.iter().map(|s| (ci.file, s)));
                        }
                        // A new object's fields are initialized as it's built.
                        for f in ci.fields.iter().filter(|f| !f.is_static) {
                            if let Some(init) = f.init {
                                work.push((ci.file, init));
                            }
                        }
                    } else if let Some(b) = ci.methods.get(i).and_then(|m| m.decl.body.as_ref()) {
                        stmts.extend(b.stmts.iter().map(|s| (ci.file, s)));
                    }
                }
            }
            let at = self.line(file, lam.span);
            for (id, file, span) in reached {
                let (c, i) = self.statics[id as usize];
                let f = &prog.classes[c].fields[i];
                let name = &f.name;
                let shown = prog.show(&f.ty);
                if !prog.thread_copyable(&f.ty) {
                    self.error(
                        file,
                        span,
                        format!("the worker spawned on line {} can't use the static `{}`: a new thread doesn't get a copy of a `{}`, so it would start empty there", at, name, shown),
                        "pass the worker what it needs when it's spawned or through a queue, or keep it in a static of numbers, classes of them, or an array of those (STDLIB §10.3)".into(),
                    );
                } else if self.holds_refs(&f.ty)
                    && self
                        .static_rs
                        .get(&id)
                        .is_some_and(|rs| rs.iter().any(|r| r.var != Var::Literal))
                {
                    self.error(
                        file,
                        span,
                        format!("the worker spawned on line {} can't use the static `{}`: it holds views that may point into the spawning thread's memory", at, name),
                        "only views of string literals are shared between threads; copy the text into a capped array (STDLIB §10.3)".into(),
                    );
                }
            }
        }
    }

    fn report(&mut self) {
        let sccs = self.cycles();
        let mut statics = self.static_rs.clone();
        for i in 0..self.funcs.len() {
            let rs = self.root_sets(&self.funcs[i], &mut statics);
            let f = &self.funcs[i];
            let mut errs: Vec<(Span, String, String)> = Vec::new();
            for ev in events(&f.body) {
                match ev {
                    Ev::Store {
                        dst,
                        src,
                        cell,
                        span,
                    } => {
                        let s = self.eval(&rs, src);
                        let d = self.eval(&rs, dst);
                        if !*cell {
                            self.this_escapes(f, &s, *span, &mut errs);
                        }
                        self.rule_s(f, &s, &d, !*cell, *span, None, &mut errs);
                    }
                    Ev::Ret { values, span } => {
                        for v in values {
                            for r in self.eval(&rs, v) {
                                self.returned(f, &r, *span, &mut errs);
                            }
                        }
                    }
                    Ev::Link {
                        dst,
                        src,
                        pool,
                        span,
                    } => {
                        let s = self.eval(&rs, src);
                        let d = self.eval(&rs, dst);
                        self.link_rule(f, &s, &d, *pool, *span, &mut errs);
                    }
                    Ev::Refuse { span, msg, help } => errs.push((*span, msg.clone(), help.clone())),
                    Ev::LambdaRet { value, scope, span } => {
                        for r in self.eval(&rs, value) {
                            match r.var {
                                Var::Temp(TempKind::Text | TempKind::Value, ..) => errs.push((
                                    *span,
                                    "this lambda returns something built only for the statement it's in, which is released before its result is used".into(),
                                    "only a lambda passed straight to a built-in, such as `map` or `fold`, can return one; build it in a variable outside the lambda (DESIGN §9.3)".into(),
                                )),
                                Var::Local(l) if f.locals[l as usize].scope.contains(scope) => {
                                    errs.push((
                                        *span,
                                        format!("this lambda returns a reference into `{}`, which is released when the lambda returns", f.locals[l as usize].name),
                                        "return a copy, or build it in a variable outside the lambda (DESIGN §9.3)".into(),
                                    ))
                                }
                                // Its caller is anywhere the function value goes, which
                                // may keep the result longer than what the lambda captures.
                                Var::Local(_)
                                | Var::Param(_)
                                | Var::Formal(_)
                                | Var::FormalHeld(_)
                                | Var::This
                                | Var::ThisHeld => errs.push((
                                    *span,
                                    format!("this lambda returns a reference into {}, which it captures, and what calls it may keep the result longer", self.root_name(f, &r)),
                                    "return a copy; only a lambda passed straight to a built-in, such as `map`, can return a view of what it captures (DESIGN §9.3)".into(),
                                )),
                                _ => {}
                            }
                        }
                    }
                    Ev::Call(c) => {
                        // A value built in place in an array: its constructor, and what it
                        // calls, mustn't change that array meanwhile (REFERENCES §6, V4).
                        if c.is_new
                            && let Some(into) = &c.call.into
                        {
                            let dest = self.eval(&rs, into);
                            for &g in &c.call.callees {
                                let sm = &self.summaries[g];
                                for (set, verb) in [
                                    (&sm.removes, "removes from"),
                                    (&sm.moves, "can move the elements of"),
                                    (&sm.grows, "adds to"),
                                ] {
                                    let hit = set.iter().any(|r| {
                                        self.map_formal(&rs, &c.call, r).iter().any(|m| {
                                            dest.iter().any(|d| {
                                                m.var == d.var
                                                    && (m.part.is_none()
                                                        || d.part.is_none()
                                                        || m.part == d.part)
                                            })
                                        })
                                    });
                                    if hit {
                                        errs.push((
                                            c.span,
                                            format!("`{}` {} the array this value is being built in, while it's built there", c.name, verb),
                                            "compute what it needs first, or add the value with a separate statement (REFERENCES §6, V4)".into(),
                                        ));
                                        break;
                                    }
                                }
                            }
                        }
                        for &g in &c.call.callees {
                            for (a, b, pool) in &self.summaries[g].links {
                                let s = self.map_link(&rs, &c.call, a);
                                let d = self.map_link(&rs, &c.call, b);
                                self.link_rule(f, &s, &d, *pool, c.span, &mut errs);
                            }
                            for (a, b) in &self.summaries[g].stores {
                                let s = self.map_formal(&rs, &c.call, a);
                                self.this_escapes(f, &s, c.span, &mut errs);
                                if c.is_new && b.var == Var::This {
                                    self.holder_rule(f, &s, c.span, &mut errs);
                                    continue;
                                }
                                let d = self.map_formal(&rs, &c.call, b);
                                self.rule_s(f, &s, &d, true, c.span, Some(&c.name), &mut errs);
                            }
                            if sccs[g].is_some() && sccs[g] == sccs[i] {
                                for &l in &c.in_scope {
                                    let lv = &f.locals[l as usize];
                                    if lv.name.starts_with('`') {
                                        errs.push((
                                            c.span,
                                            format!("{} is still being built during this call, which can recurse back into `{}`", lv.name, f.name),
                                            "it would need a new data stack on every call; build it with a loop into an array passed in by reference from outside the cycle (REFERENCES §8.4)".into(),
                                        ));
                                        continue;
                                    }
                                    let what = if self.prog.variable_size(&lv.ty) {
                                        "a variable-size record"
                                    } else {
                                        "growable"
                                    };
                                    errs.push((
                                        c.span,
                                        format!("`{}` is {} and in scope during this call, which can recurse back into `{}`", lv.name, what, f.name),
                                        format!("{} local would need a new data stack on every call; pass it in by reference from outside the cycle (REFERENCES §8.4)", if what == "growable" { "a growable" } else { "such a" }),
                                    ));
                                }
                                for s in &c.by_value {
                                    errs.push((*s, "a growable value can't be passed by value into a recursive cycle".into(), "pass it by reference, `T&` (REFERENCES §8.4)".into()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Rule R: removals while a reference into the array is still used.
            let mut ctx = LiveCtx::default();
            self.live(
                f,
                &rs,
                &statics,
                &f.body,
                &Live::new(),
                &mut ctx,
                true,
                &mut errs,
            );
            let file = f.file;
            for (span, msg, help) in errs {
                self.error(file, span, msg, help);
            }
        }
    }

    /// Rule S for one store, and §5.3 when it goes into a field or an element.
    #[allow(clippy::too_many_arguments)]
    fn rule_s(
        &self,
        f: &Func,
        src: &Roots,
        dst: &Roots,
        holder: bool,
        span: Span,
        via: Option<&str>,
        errs: &mut Vec<(Span, String, String)>,
    ) {
        let prefix = via.map_or(String::new(), |n| {
            format!(
                "`{}(...)` stores a reference from one argument into another, and ",
                n
            )
        });
        for s in src {
            for d in dst {
                if s.var == d.var {
                    continue;
                }
                match (s.var, d.var) {
                    (Var::Literal | Var::Static(_), _) => {}
                    (_, Var::Temp(..) | Var::Literal) => {}
                    (Var::Temp(kind, ..), _) => {
                        if !self.temps_reported.borrow_mut().insert((f.file, s.var)) {
                            continue;
                        }
                        let (msg, help) = match kind {
                            TempKind::Text => (
                                format!("{}this text is a temporary, built only for this statement, so {} can't keep a view of it", prefix, self.root_name(f, d)),
                                "build it in the variable's own storage instead, as in `u8[.] s = \"${x}\";` (REFERENCES §3.3)".to_string(),
                            ),
                            TempKind::Lambda => (
                                format!("{}a lambda that captures variables can't be stored in {}", prefix, self.root_name(f, d)),
                                "only a lambda passed as an argument can capture; a stored `func` is a plain function pointer (DESIGN §9.3)".to_string(),
                            ),
                            TempKind::Value => (
                                format!("{}this refers into a temporary value, which is released at the end of the statement", prefix),
                                "store the value in a variable first, and refer to that (REFERENCES §3.3)".to_string(),
                            ),
                        };
                        errs.push((span, msg, help));
                    }
                    (Var::Local(a), Var::Local(b)) => {
                        let (la, lb) = (&f.locals[a as usize], &f.locals[b as usize]);
                        if !outlives(la, lb) {
                            let why = if la.scope.len() > lb.scope.len()
                                || !lb.scope.starts_with(&la.scope)
                            {
                                "is in an inner block"
                            } else {
                                "is declared after it"
                            };
                            errs.push((
                                span,
                                format!("{}a reference into {} can't be stored in {}: {} {}, so it is released first", prefix, quoted(&la.name), quoted(&lb.name), quoted(&la.name), why),
                                if la.name.contains(' ') {
                                    "what it gives lasts only as long as the loop; copy it to keep it (REFERENCES §3.3)".to_string()
                                } else {
                                    format!("declare `{}` before `{}`, in the same block or an enclosing one (REFERENCES §3.3)", la.name, lb.name)
                                },
                            ));
                        }
                    }
                    // A lambda's parameter, when the lambda isn't passed straight to a built-in:
                    // only its own locals can keep what it refers to.
                    (Var::Arg(p), _) => {
                        let kept = match d.var {
                            Var::Local(b) => outlives(&f.locals[p as usize], &f.locals[b as usize]),
                            _ => false,
                        };
                        if !kept {
                            errs.push((
                                span,
                                format!("{}the lambda's parameter `{}` refers to what its caller passed, which may not outlive the call, so {} can't keep it", prefix, f.locals[p as usize].name, self.root_name(f, d)),
                                "copy it, or pass the lambda straight to a built-in, which tells what it passes (DESIGN §9.3)".into(),
                            ));
                        }
                    }
                    (Var::Local(_) | Var::Param(_), _) if !matches!(d.var, Var::Local(_)) => {
                        let what = match d.var {
                            Var::Static(_) => "a static, which lives for the whole run".to_string(),
                            Var::Param(_) => format!(
                                "{}, a parameter, which lives for the whole call",
                                self.root_name(f, d)
                            ),
                            _ => format!("{}, which belongs to the caller", self.root_name(f, d)),
                        };
                        errs.push((
                            span,
                            format!("{}a reference into {}, a variable of this function, can't be stored in {}", prefix, self.root_name(f, s), what),
                            "it would outlive what it points into; store a copy instead (REFERENCES §3.3)".into(),
                        ));
                    }
                    _ => {}
                }
            }
        }
        if holder {
            self.holder_rule(f, src, span, errs);
        }
    }

    /// A constructor builds its value where the code generator chooses and copies it to
    /// where it goes, so a reference to `this`, or into its inline storage, can't be
    /// kept: only into a root's growable field, which is on a data stack of its own.
    fn this_escapes(
        &self,
        f: &Func,
        src: &Roots,
        span: Span,
        errs: &mut Vec<(Span, String, String)>,
    ) {
        if f.kind != FnKind::Ctor {
            return;
        }
        let own_stack = |p: Part| {
            let c = &self.prog.classes[p.class];
            c.kind == ClassKind::Root && starts_empty(&c.fields[p.field as usize].ty)
        };
        if src
            .iter()
            .any(|r| r.var == Var::This && !r.part.is_some_and(own_stack))
        {
            errs.push((
                span,
                "a constructor's `this` is copied to where the value goes when it returns, so a reference to it can't be kept".into(),
                "set the reference once the value is in place, as `n.me .= n;` (DESIGN §6.2)".into(),
            ));
        }
    }

    /// §5.3: references into removable arrays stay in locals and parameters.
    fn holder_rule(
        &self,
        f: &Func,
        src: &Roots,
        span: Span,
        errs: &mut Vec<(Span, String, String)>,
    ) {
        for s in src {
            // A reference into a local, a static, or a removable field of anything.
            if s.whole || (!s.var.is_cell() && !matches!(s.var, Var::Static(_)) && s.part.is_none())
            {
                continue;
            }
            if let Some(t) = self.root_ty(f, s)
                && Self::removable_ty(&t)
            {
                let n = self.root_name(f, s);
                let can = if moves(&t) {
                    "whose elements can move"
                } else {
                    "which can remove elements"
                };
                errs.push((
                    span,
                    format!("a reference into {}, {}, can be kept only in a local variable or a parameter", n, can),
                    "a field, an element or a static could keep it past a removal; store an index instead, or make the array grow-only (`T[..>]`) or a `pool` (REFERENCES §5.3)".into(),
                ));
            }
        }
    }

    fn returned(&self, f: &Func, r: &Root, span: Span, errs: &mut Vec<(Span, String, String)>) {
        match r.var {
            Var::Local(_) | Var::Param(_) => errs.push((
                span,
                format!("this returns a reference into {}, which is released when the function returns", self.root_name(f, r)),
                "return a copy, or a reference rooted in what a `&` or slice parameter refers to (REFERENCES §3.3)".into(),
            )),
            Var::Temp(TempKind::Lambda, ..) => errs.push((span, "a lambda that captures variables can't be returned".into(), "only a lambda passed as an argument can capture (DESIGN §9.3)".into())),
            Var::Temp(..) => errs.push((span, "this returns a reference to a temporary, which is released at the end of the statement".into(), "return a value instead (REFERENCES §3.3)".into())),
            _ => {}
        }
    }

    /// Function cycles in the call graph: each function's cycle number, if it is in one.
    fn cycles(&self) -> Vec<Option<usize>> {
        let n = self.funcs.len();
        let mut edges: Vec<Vec<FnId>> = vec![Vec::new(); n];
        for (i, f) in self.funcs.iter().enumerate() {
            for ev in events(&f.body) {
                if let Ev::Call(c) = ev {
                    edges[i].extend(c.call.callees.iter().copied());
                }
            }
        }
        // Tarjan's algorithm.
        struct T<'e> {
            edges: &'e [Vec<FnId>],
            index: Vec<Option<usize>>,
            low: Vec<usize>,
            on: Vec<bool>,
            stack: Vec<usize>,
            next: usize,
            comp: Vec<Option<usize>>,
            ncomp: usize,
        }
        fn visit(t: &mut T, v: usize) {
            t.index[v] = Some(t.next);
            t.low[v] = t.next;
            t.next += 1;
            t.stack.push(v);
            t.on[v] = true;
            for &w in &t.edges[v].clone() {
                match t.index[w] {
                    None => {
                        visit(t, w);
                        t.low[v] = t.low[v].min(t.low[w]);
                    }
                    Some(iw) if t.on[w] => t.low[v] = t.low[v].min(iw),
                    _ => {}
                }
            }
            if Some(t.low[v]) == t.index[v] {
                let mut members = Vec::new();
                while let Some(w) = t.stack.pop() {
                    t.on[w] = false;
                    members.push(w);
                    if w == v {
                        break;
                    }
                }
                let cyclic = members.len() > 1 || t.edges[v].contains(&v);
                if cyclic {
                    for m in members {
                        t.comp[m] = Some(t.ncomp);
                    }
                    t.ncomp += 1;
                }
            }
        }
        let mut t = T {
            edges: &edges,
            index: vec![None; n],
            low: vec![0; n],
            on: vec![false; n],
            stack: Vec::new(),
            next: 0,
            comp: vec![None; n],
            ncomp: 0,
        };
        for v in 0..n {
            if t.index[v].is_none() {
                visit(&mut t, v);
            }
        }
        t.comp
    }

    // ------------------------------------------------------------ rule R: liveness

    /// Walks `nodes` backward from the variables live after them, returning those live
    /// before them. With `report`, checks each removal against what is live after it.
    #[allow(clippy::too_many_arguments)]
    fn live(
        &self,
        f: &Func,
        rs: &HashMap<Var, Roots>,
        statics: &HashMap<u32, Roots>,
        nodes: &[Node],
        after: &Live,
        ctx: &mut LiveCtx,
        report: bool,
        errs: &mut Vec<(Span, String, String)>,
    ) -> Live {
        let mut live = after.clone();
        for n in nodes.iter().rev() {
            match n {
                Node::Ev(ev) => match ev {
                    Ev::Use(v) => {
                        live.insert(*v);
                    }
                    Ev::Def(v) => {
                        live.remove(v);
                    }
                    Ev::Remove { target, span, what } => {
                        if report {
                            let t = self.eval(rs, target);
                            let c = Change::Remove;
                            self.removal(f, rs, statics, &t, &live, ctx, *span, what, c, errs);
                        }
                    }
                    Ev::Change {
                        target,
                        span,
                        what,
                        kind,
                    } => {
                        if report {
                            let t = self.eval(rs, target);
                            self.removal(f, rs, statics, &t, &live, ctx, *span, what, *kind, errs);
                        }
                    }
                    Ev::Call(c) => {
                        let mut with_args = live.clone();
                        with_args.extend(c.uses.iter().copied());
                        if report {
                            self.aliased(f, rs, &c.call, &c.name, c.span, ctx, errs);
                            for sm in self.sums(&c.call) {
                                for (set, kind) in [
                                    (&sm.removes, Change::Remove),
                                    (&sm.moves, Change::Move),
                                    (&sm.rearranges, Change::Rearrange),
                                ] {
                                    for r in set {
                                        let t = self.map_formal(rs, &c.call, r);
                                        let what = format!("`{}(...)`", c.name);
                                        self.removal(
                                            f, rs, statics, &t, &with_args, ctx, c.span, &what,
                                            kind, errs,
                                        );
                                    }
                                }
                            }
                        }
                        live = with_args;
                    }
                    Ev::Store { .. }
                    | Ev::Ret { .. }
                    | Ev::Link { .. }
                    | Ev::LambdaRet { .. }
                    | Ev::Refuse { .. }
                    | Ev::Grow { .. }
                    | Ev::Carry { .. } => {}
                },
                Node::Block { body, defers } => {
                    // A deferred statement runs when the block ends (REFERENCES §9): by
                    // falling off its end, or by a `break` or `continue` out of it, which
                    // goes on after an enclosing loop or at its next pass.
                    let mut leaving = live.clone();
                    if !defers.is_empty() {
                        for l in &ctx.loops {
                            leaving.extend(l.1.iter().copied());
                            leaving.extend(l.2.iter().copied());
                        }
                    }
                    let end = self.live(f, rs, statics, defers, &leaving, ctx, report, errs);
                    // What a jump out of the block reads: its deferred statements' uses.
                    ctx.defers.push(if defers.is_empty() {
                        Live::new()
                    } else {
                        end.clone()
                    });
                    live = self.live(f, rs, statics, body, &end, ctx, report, errs);
                    ctx.defers.pop();
                }
                Node::If(t, e) => {
                    let a = self.live(f, rs, statics, t, &live, ctx, report, errs);
                    let b = self.live(f, rs, statics, e, &live, ctx, report, errs);
                    live = a.union(&b).copied().collect();
                }
                Node::Switch(arms, every) => {
                    let mut out = if *every { Live::new() } else { live.clone() };
                    for a in arms {
                        out.extend(self.live(f, rs, statics, a, &live, ctx, report, errs));
                    }
                    live = out;
                }
                Node::Loop(l) => live = self.live_loop(f, rs, statics, l, &live, ctx, report, errs),
                // Leaving blocks runs their deferred statements first.
                Node::Break(label) => {
                    live = ctx
                        .target(label)
                        .map_or_else(Live::new, |(after, _, depth)| {
                            let mut l = after.clone();
                            l.extend(ctx.defers[depth..].iter().flatten().copied());
                            l
                        })
                }
                Node::Continue(label) => {
                    live = ctx
                        .target(label)
                        .map_or_else(Live::new, |(_, cont, depth)| {
                            let mut l = cont.clone();
                            l.extend(ctx.defers[depth..].iter().flatten().copied());
                            l
                        })
                }
                Node::Return => live = ctx.defers.iter().flatten().copied().collect(),
            }
        }
        live
    }

    /// A loop: finds what is live at its start by going around until nothing changes,
    /// then, reporting, goes through once more in that state (REFERENCES §5.2).
    #[allow(clippy::too_many_arguments)]
    fn live_loop(
        &self,
        f: &Func,
        rs: &HashMap<Var, Roots>,
        statics: &HashMap<u32, Roots>,
        l: &Loop,
        after: &Live,
        ctx: &mut LiveCtx,
        report: bool,
        errs: &mut Vec<(Span, String, String)>,
    ) -> Live {
        let mut head = Live::new();
        let mut scratch = Vec::new();
        // What is live only grows, so this ends; the bound is a guard.
        for round in 0.. {
            if round == LIMIT {
                self.too_complex(f.file, f.span);
                break;
            }
            let start = self.loop_pass(f, rs, statics, l, after, &head, ctx, false, &mut scratch);
            if start == head {
                break;
            }
            head = start;
        }
        if report {
            self.loop_pass(f, rs, statics, l, after, &head, ctx, true, errs);
        }
        head
    }

    #[allow(clippy::too_many_arguments)]
    fn loop_pass(
        &self,
        f: &Func,
        rs: &HashMap<Var, Roots>,
        statics: &HashMap<u32, Roots>,
        l: &Loop,
        after: &Live,
        head: &Live,
        ctx: &mut LiveCtx,
        report: bool,
        errs: &mut Vec<(Span, String, String)>,
    ) -> Live {
        let post_after: Live = if l.exit == Exit::Post {
            after.union(head).copied().collect()
        } else {
            head.clone()
        };
        let post_start = self.live(f, rs, statics, &l.post, &post_after, ctx, report, errs);
        ctx.loops.push((
            l.label.clone(),
            after.clone(),
            post_start.clone(),
            ctx.defers.len(),
        ));
        if let Some((h, s)) = &l.holder {
            ctx.holders.push((self.eval(rs, h), *s));
        }
        let body_start = self.live(f, rs, statics, &l.body, &post_start, ctx, report, errs);
        if l.holder.is_some() {
            ctx.holders.pop();
        }
        ctx.loops.pop();
        let pre_after: Live = if l.exit == Exit::Pre {
            after.union(&body_start).copied().collect()
        } else {
            body_start
        };
        self.live(f, rs, statics, &l.pre, &pre_after, ctx, report, errs)
    }

    /// A call whose callee uses a reference into one argument after removing from
    /// another, where these arguments are the same storage: an error. Where they are the
    /// caller's own arguments, the caller's summary passes the pair on.
    #[allow(clippy::too_many_arguments)]
    fn aliased(
        &self,
        f: &Func,
        rs: &HashMap<Var, Roots>,
        call: &CallR,
        name: &str,
        span: Span,
        ctx: &mut LiveCtx,
        errs: &mut Vec<(Span, String, String)>,
    ) {
        let callers = |r: &Root| r.var.is_formal() || matches!(r.var, Var::Static(_));
        for &g in &call.callees {
            for (u, t, kind) in &self.summaries[g].uses_after {
                let us = self.map_formal(rs, call, u);
                let ts = self.map_formal(rs, call, t);
                let shared = us.iter().find(|x| {
                    !x.whole
                        && ts.iter().any(|y| {
                            x.var == y.var
                                && (x.part.is_none() || y.part.is_none() || x.part == y.part)
                        })
                });
                if let Some(x) = shared {
                    let callee = &self.funcs[g];
                    let verb = match kind {
                        Change::Remove => "removes from",
                        Change::Move => "can move the elements of",
                        Change::Rearrange => "can rearrange",
                    };
                    // A reference parameter is used itself; anything else, a reference
                    // into it.
                    let used = match u.var {
                        Var::Formal(i)
                            if callee.params.get(i as usize).is_some_and(|p| {
                                matches!(p.1, Ty::Ref(_) | Ty::Nullable(_))
                                    && !Self::whole_ref(&p.1)
                            }) =>
                        {
                            self.formal_name(callee, u)
                        }
                        _ => format!("a reference into {}", self.formal_name(callee, u)),
                    };
                    errs.push((
                        span,
                        format!(
                            "`{}(...)` {} {} while it still uses {}, and in this call both are in {}",
                            name,
                            verb,
                            self.formal_name(callee, t),
                            used,
                            self.root_name(f, &Root { part: None, ..*x }),
                        ),
                        format!("pass separate storage, or have `{}` bind the reference again after the change (REFERENCES §4.2)", callee.name),
                    ));
                    return;
                }
                for x in us.iter().filter(|x| callers(x) && !x.whole) {
                    for y in ts.iter().filter(|y| callers(y) && y.var != x.var) {
                        ctx.pairs.insert((*x, Root { whole: false, ..*y }, *kind));
                    }
                }
            }
        }
    }

    /// A formal root of `g`, as its own code names it.
    fn formal_name(&self, g: &Func, r: &Root) -> String {
        match r.var {
            Var::Formal(i) | Var::FormalHeld(i) => g
                .params
                .get(i as usize)
                .map_or("an argument".into(), |p| format!("`{}`", p.0)),
            Var::This | Var::ThisHeld => "`this`".into(),
            Var::Static(id) => {
                let (c, i) = self.statics[id as usize];
                format!("the static `{}`", self.prog.classes[c].fields[i].name)
            }
            _ => "an argument".into(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn removal(
        &self,
        f: &Func,
        rs: &HashMap<Var, Roots>,
        statics: &HashMap<u32, Roots>,
        target: &Roots,
        live: &Live,
        ctx: &mut LiveCtx,
        span: Span,
        what: &str,
        kind: Change,
        errs: &mut Vec<(Span, String, String)>,
    ) {
        let verb = match kind {
            Change::Remove => "removes from",
            Change::Move => "can move the elements of",
            Change::Rearrange => "can rearrange",
        };
        let target: Vec<Root> = target
            .iter()
            .filter(|t| !self.root_ty(f, t).is_some_and(|ty| Self::exempt_ty(&ty)))
            .copied()
            .collect();
        if target.is_empty() {
            return;
        }
        // A reference into one of the caller's arrays, used after this changes another:
        // the caller decides whether they are the same.
        let callers = |r: &Root| r.var.is_formal() || matches!(r.var, Var::Static(_));
        for v in live {
            let Some(set) = rs.get(v) else { continue };
            let ty = match v {
                Var::Local(i) => f.locals[*i as usize].ty.clone(),
                Var::Param(i) => f.params[*i as usize].1.clone(),
                _ => Ty::Unknown,
            };
            if Self::whole_ref(&ty)
                || kind == Change::Rearrange && matches!(ty, Ty::Slice(_) | Ty::View(_))
            {
                continue;
            }
            for u in set.iter().filter(|u| callers(u) && !u.whole) {
                for t in target.iter().filter(|t| callers(t) && t.var != u.var) {
                    ctx.pairs.insert((*u, Root { whole: false, ..*t }, kind));
                }
            }
        }
        let hits = |set: &Roots| {
            target
                .iter()
                .find(|t| set.iter().any(|r| r.overlaps(t)))
                .copied()
        };
        // A for-each over the array keeps a position in it, which a rearrangement keeps.
        for (h, s) in ctx.holders.iter().filter(|_| kind != Change::Rearrange) {
            if let Some(t) = hits(h) {
                let _ = s;
                errs.push((
                    span,
                    format!("{} {} {} inside a for-each over it", what, verb, self.root_name(f, &t)),
                    "the loop keeps its place in it; remove afterwards, or use `retain` or a repeat loop (REFERENCES §9)".into(),
                ));
                return;
            }
        }
        for v in live {
            let Some(set) = rs.get(v) else { continue };
            let ty = match v {
                Var::Local(i) => f.locals[*i as usize].ty.clone(),
                Var::Param(i) => f.params[*i as usize].1.clone(),
                _ => Ty::Unknown,
            };
            if Self::whole_ref(&ty)
                || kind == Change::Rearrange && matches!(ty, Ty::Slice(_) | Ty::View(_))
            {
                continue;
            }
            if let Some(t) = hits(set) {
                let (name, decl) = match v {
                    Var::Local(i) => (
                        f.locals[*i as usize].name.clone(),
                        Some(f.locals[*i as usize].span),
                    ),
                    Var::Param(i) => (f.params[*i as usize].0.clone(), None),
                    _ => continue,
                };
                let bound = decl.map_or(String::new(), |d| {
                    format!(", declared on line {},", self.line(f.file, d))
                });
                let help = match kind {
                    Change::Remove => format!(
                        "`{}`{} is used after this; use it before the removal, or bind it again after (REFERENCES §5.2)",
                        name, bound
                    ),
                    Change::Move => format!(
                        "`{}`{} is used after this, and an insert into a ring or gap buffer can move its elements; use it before, or bind it again after (REFERENCES §5.4)",
                        name, bound
                    ),
                    Change::Rearrange => format!(
                        "`{}`{} is used after this; a slice, sort or comparison makes a ring or gap buffer one block first, which can move its elements; use it before, or bind it again after (REFERENCES §5.4)",
                        name, bound
                    ),
                };
                errs.push((
                    span,
                    format!(
                        "{} {} {} while `{}` may still refer into it",
                        what,
                        verb,
                        self.root_name(f, &t),
                        name
                    ),
                    help,
                ));
                return;
            }
        }
        // References stored in what the caller passed, or in statics, may be used later.
        for (v, set) in rs {
            if v.is_formal()
                && let Some(t) = hits(set)
            {
                let holder = self.root_name(f, &Root::of(*v));
                errs.push((
                    span,
                    format!(
                        "{} {} {}, and a field of {} may hold a reference into it",
                        what,
                        verb,
                        self.root_name(f, &t),
                        holder
                    ),
                    if kind == Change::Remove {
                        "a reference kept in a field can be used after the removal (REFERENCES §5.3)"
                    } else {
                        "a reference kept in a field can be used after this, which can move the element it refers to (REFERENCES §5.4)"
                    }
                    .into(),
                ));
                return;
            }
        }
        for (id, set) in statics {
            if let Some(t) = hits(set) {
                let (c, i) = self.statics[*id as usize];
                errs.push((
                    span,
                    format!(
                        "{} {} {}, and the static `{}` may hold a reference into it",
                        what,
                        verb,
                        self.root_name(f, &t),
                        self.prog.classes[c].fields[i].name
                    ),
                    if kind == Change::Remove {
                        "a reference kept in a static can be used after the removal (REFERENCES §5.3)"
                    } else {
                        "a reference kept in a static can be used after this, which can move the element it refers to (REFERENCES §5.4)"
                    }
                    .into(),
                ));
                return;
            }
        }
    }
}

type Live = BTreeSet<Var>;

#[derive(Default)]
struct LiveCtx {
    /// What `Summary::uses_after` gets: references into the caller's storage used after a
    /// removal from other storage of the caller's.
    pairs: BTreeSet<(Root, Root, Change)>,
    /// Enclosing loops: label, live at their exit, live where `continue` goes, and how
    /// many blocks with deferred statements enclosed the loop.
    loops: Vec<(Option<String>, Live, Live, usize)>,
    /// Live at the end of each enclosing block, where its defers run.
    defers: Vec<Live>,
    /// Enclosing for-each loops: what they loop over.
    holders: Vec<(Roots, Span)>,
}

impl LiveCtx {
    fn target(&self, label: &Option<String>) -> Option<(&Live, &Live, usize)> {
        let l = match label {
            None => self.loops.last(),
            Some(n) => self.loops.iter().rev().find(|l| l.0.as_deref() == Some(n)),
        }?;
        Some((&l.1, &l.2, l.3))
    }
}

/// Whether `e` is `new C(...)`, through any parentheses.
/// Each expression in `e`, `e` first, including those in lambdas' and switch arms' blocks.
fn visit_expr<'e>(e: &'e Expr, f: &mut impl FnMut(&'e Expr)) {
    f(e);
    match &e.kind {
        E::Lambda { body, .. } => {
            if let LambdaBody::Block(b) = body.as_ref() {
                b.stmts.iter().for_each(|s| visit_stmt(s, f));
            }
        }
        E::Switch(sw) => {
            for r in &sw.rules {
                if let SwitchBody::Block(b) = &r.body {
                    b.stmts.iter().for_each(|s| visit_stmt(s, f));
                }
            }
        }
        _ => {}
    }
    for x in crate::check::expr_children(e) {
        visit_expr(x, f);
    }
}

fn visit_stmt<'e>(s: &'e Stmt, f: &mut impl FnMut(&'e Expr)) {
    let (ss, es) = crate::check::stmt_children(s);
    es.into_iter().for_each(|e| visit_expr(e, f));
    ss.into_iter().for_each(|s| visit_stmt(s, f));
}

/// `thread.spawn(lambda)` calls, by their lambda, and every lambda, for the calls through
/// function values a worker may make.
fn find_spawns<'e>(
    facts: &Facts,
    prog: &Program,
    file: usize,
    e: &'e Expr,
    spawns: &mut Vec<(usize, &'e Expr)>,
    lambdas: &mut Vec<(usize, &'e Expr)>,
) {
    if let E::Lambda { .. } = &e.kind {
        lambdas.push((file, e));
    }
    if let E::Call { callee, args } = &e.kind
        && let E::Member { expr, name } = &callee.kind
        && name.name == "spawn"
        && args.len() == 1
        && let Some(Ty::TypeName(t)) = facts.types.get(&key(expr.span))
        && let Ty::Class(c, _) = &**t
        && prog.classes[*c].name == "thread"
        && prog.classes[*c].package == "zink.thread"
    {
        spawns.push((file, &args[0].expr));
    }
}

/// The static an expression names: a bare name, or `Class.name`.
fn static_named(
    facts: &Facts,
    prog: &Program,
    static_of: &HashMap<(ClassId, usize), u32>,
    e: &Expr,
) -> Option<u32> {
    match &e.kind {
        E::Ident(_) => match facts.names.get(&key(e.span)) {
            Some(NameRes::Field(c, i)) => static_of.get(&(*c, *i)).copied(),
            _ => None,
        },
        E::Member { expr, name } => match facts.types.get(&key(expr.span)) {
            Some(Ty::TypeName(t)) => match &**t {
                Ty::Class(c, _) => prog.classes[*c]
                    .fields
                    .iter()
                    .position(|f| f.is_static && f.name == name.name)
                    .and_then(|i| static_of.get(&(*c, i)).copied()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// The iterator expression of every for-each in `s`, including those in lambdas.
fn foreach_iters(s: &Stmt, out: &mut HashSet<(u32, u32)>) {
    if let S::ForEach { iter, .. } = &s.kind {
        out.insert(key(iter.span));
    }
    let (ss, es) = crate::check::stmt_children(s);
    for st in ss {
        foreach_iters(st, out);
    }
    for e in es {
        visit_expr(e, &mut |x| {
            if let E::Lambda { body, .. } = &x.kind
                && let LambdaBody::Block(b) = body.as_ref()
            {
                for st in &b.stmts {
                    foreach_iters(st, out);
                }
            }
        });
    }
}

/// What a for-each over a value of class `c` calls: its `next()`, or its `iterator()`
/// and that result's `next()`.
fn iter_methods(prog: &Program, c: ClassId, out: &mut Vec<(ClassId, bool, usize)>) {
    let find = |c: ClassId, name: &str| {
        prog.classes[c]
            .methods
            .iter()
            .position(|m| m.name == name && m.params.is_empty() && !m.is_static)
    };
    if let Some(i) = find(c, "next") {
        callees(prog, c, i, out);
    } else if let Some(i) = find(c, "iterator") {
        callees(prog, c, i, out);
        if let Ty::Class(ic, _) = prog.classes[c].methods[i].ret.unref()
            && let Some(j) = find(*ic, "next")
        {
            callees(prog, *ic, j, out);
        }
    }
}

/// What a call of method `i` of class `c` may run: the method, or for an interface's,
/// each implementation of it.
fn callees(prog: &Program, c: ClassId, i: usize, out: &mut Vec<(ClassId, bool, usize)>) {
    let ci = &prog.classes[c];
    if ci.kind != ClassKind::Interface {
        out.push((c, false, i));
        return;
    }
    for k in 0..prog.classes.len() {
        if let Some(j) = prog.implementation(k, c, i) {
            out.push((k, false, j));
        }
    }
}

fn is_new(e: &Expr) -> bool {
    match &e.kind {
        E::Paren(x) => is_new(x),
        E::New { .. } => true,
        _ => false,
    }
}

fn outlives(a: &LocalVar, b: &LocalVar) -> bool {
    b.scope.starts_with(&a.scope) && (a.scope.len() < b.scope.len() || a.order <= b.order)
}

fn name_of(n: &Name) -> String {
    match n {
        Name::Ident(i) => i.name.clone(),
        Name::Discard(_) => "_".into(),
    }
}

/// Every event in a flow form, in no particular order.
fn events(nodes: &[Node]) -> Vec<&Ev> {
    let mut out = Vec::new();
    fn go<'n>(nodes: &'n [Node], out: &mut Vec<&'n Ev>) {
        for n in nodes {
            match n {
                Node::Ev(e) => out.push(e),
                Node::Block { body, defers } => {
                    go(body, out);
                    go(defers, out);
                }
                Node::If(a, b) => {
                    go(a, out);
                    go(b, out);
                }
                Node::Switch(arms, _) => {
                    for a in arms {
                        go(a, out);
                    }
                }
                Node::Loop(l) => {
                    go(&l.pre, out);
                    go(&l.body, out);
                    go(&l.post, out);
                }
                _ => {}
            }
        }
    }
    go(nodes, &mut out);
    out
}

// ---------------------------------------------------------------- the builder

/// An expression's storage, if it is a place, and the roots of the references its value holds.
#[derive(Clone)]
struct Val {
    cell: Option<R>,
    refs: R,
    /// For a tuple: each element.
    elems: Option<Vec<Val>>,
}

impl Val {
    fn none() -> Val {
        Val {
            cell: None,
            refs: R::Empty,
            elems: None,
        }
    }

    fn refs(r: R) -> Val {
        Val {
            cell: None,
            refs: r,
            elems: None,
        }
    }

    fn place(cell: R) -> Val {
        Val {
            refs: R::stored(cell.clone()),
            cell: Some(cell),
            elems: None,
        }
    }
}

struct Builder<'x, 'p, 'a> {
    an: &'x Analysis<'p, 'a>,
    has_this: bool,
    locals: Vec<LocalVar>,
    var_of: HashMap<(u32, u32), Var>,
    scopes: Vec<u32>,
    scope_locals: Vec<Vec<u32>>,
    next_scope: u32,
    order: u32,
    out: Vec<Vec<Node>>,
    defers: Vec<Vec<Node>>,
    /// Collecting the variables read while evaluating a call's arguments.
    collect: Vec<Vec<Var>>,
    /// The function's declared result type.
    ret: Ty,
    /// Collecting the values of `yield`s, and the type each switch gives.
    yields: Vec<Vec<R>>,
    yield_tys: Vec<Ty>,
    lambda_depth: usize,
    /// Rearrangements from converting a ring or gap buffer to a slice, which `convert`
    /// finds; emitted before the next event.
    pending: std::cell::RefCell<Vec<(R, Span)>>,
    /// The array a `new` about to be evaluated is built in place in (CallR::into).
    new_into: Option<R>,
    /// Where each parameter is declared, for its type.
    param_spans: Vec<Span>,
    /// The variables a receiver read, for a free function called as a method: uses of
    /// the call, as its arguments' are.
    recv_uses: Vec<Var>,
    /// For a constructor: its class, whose field initializers run first.
    inits: Option<ClassId>,
    /// The function being built.
    fn_id: FnId,
    /// For a lambda about to be walked that is passed straight to a built-in: the
    /// elements it is given, the built-in's name, and its other arguments.
    lambda_elems: Option<(R, String, R)>,
    /// For each lambda being walked, its result type and what its `return`s give.
    lambda_rets: Vec<(Ty, R)>,
    /// What each lambda returns, by span, for the built-ins whose result it is.
    lambda_res: HashMap<(u32, u32), R>,
    /// The expression about to be walked is a lambda passed straight to a built-in,
    /// which may return a temporary: the built-in's result is rooted at it.
    lambda_to_builtin: bool,
}

impl<'x, 'p, 'a> Builder<'x, 'p, 'a> {
    fn new(an: &'x Analysis<'p, 'a>, has_this: bool) -> Self {
        Builder {
            an,
            has_this,
            locals: Vec::new(),
            var_of: HashMap::new(),
            scopes: vec![0],
            scope_locals: vec![Vec::new()],
            next_scope: 1,
            order: 0,
            out: vec![Vec::new()],
            defers: vec![Vec::new()],
            collect: Vec::new(),
            ret: Ty::Void,
            yields: Vec::new(),
            yield_tys: Vec::new(),
            lambda_depth: 0,
            pending: Default::default(),
            new_into: None,
            param_spans: Vec::new(),
            recv_uses: Vec::new(),
            inits: None,
            fn_id: 0,
            lambda_elems: None,
            lambda_rets: Vec::new(),
            lambda_res: HashMap::new(),
            lambda_to_builtin: false,
        }
    }

    fn function(mut self, params: &[ast::Param], body: &ast::Block) -> (Vec<LocalVar>, Vec<Node>) {
        for (i, p) in params.iter().enumerate() {
            if let Name::Ident(id) = &p.name {
                self.var_of.insert(key(id.span), Var::Param(i as u32));
                self.param_spans.push(id.span);
            } else {
                self.param_spans.push(p.span);
            }
        }
        // A constructor's field initializers run before its body, into `this`.
        if let Some(c) = self.inits {
            let prog = self.an.prog;
            for (i, fd) in prog.classes[c].fields.iter().enumerate() {
                let Some(init) = fd.init.filter(|_| !fd.is_static) else {
                    continue;
                };
                let v = self.expr(init);
                let from = self.ty(init);
                let src = self.convert(&v, &from, &fd.ty, init.span);
                let dst = self.field_cell(R::root(Var::This), c, i);
                self.ev(Ev::Store {
                    dst,
                    src,
                    cell: false,
                    span: init.span,
                });
            }
        }
        self.block(body);
        let nodes = self.out.pop().unwrap_or_default();
        (self.locals, nodes)
    }

    /// The static initializers of class `c`, each a store into its static.
    fn statics(mut self, c: ClassId) -> (Vec<LocalVar>, Vec<Node>) {
        let prog = self.an.prog;
        for (i, fd) in prog.classes[c].fields.iter().enumerate() {
            let Some(init) = fd.init.filter(|_| fd.is_static) else {
                continue;
            };
            let Some(id) = self.an.static_of.get(&(c, i)).copied() else {
                continue;
            };
            let v = self.expr(init);
            let from = self.ty(init);
            let src = self.convert(&v, &from, &fd.ty, init.span);
            self.ev(Ev::Store {
                dst: R::root(Var::Static(id)),
                src,
                cell: false,
                span: init.span,
            });
        }
        let nodes = self.out.pop().unwrap_or_default();
        (self.locals, nodes)
    }

    fn ty(&self, e: &Expr) -> Ty {
        self.an
            .facts
            .types
            .get(&key(e.span))
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    fn emit(&mut self, n: Node) {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        for (target, span) in pending {
            self.out
                .last_mut()
                .expect("a node list")
                .push(Node::Ev(Ev::Change {
                    target,
                    span,
                    what: "this slice".into(),
                    kind: Change::Rearrange,
                }));
        }
        if let Node::Ev(Ev::Use(v)) = &n {
            for c in &mut self.collect {
                c.push(*v);
            }
        }
        self.out.last_mut().expect("a node list").push(n);
    }

    fn ev(&mut self, e: Ev) {
        self.emit(Node::Ev(e));
    }

    /// Builds the nodes that `f` emits into a list of their own.
    fn sub(&mut self, f: impl FnOnce(&mut Self)) -> Vec<Node> {
        self.out.push(Vec::new());
        f(self);
        self.out.pop().unwrap_or_default()
    }

    fn push_scope(&mut self) {
        self.scopes.push(self.next_scope);
        self.next_scope += 1;
        self.scope_locals.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.scope_locals.pop();
    }

    fn declare(&mut self, name: &ast::Ident) -> Var {
        let ty = self
            .an
            .facts
            .locals
            .get(&key(name.span))
            .cloned()
            .unwrap_or(Ty::Unknown);
        let i = self.locals.len() as u32;
        self.locals.push(LocalVar {
            name: name.name.clone(),
            ty,
            span: name.span,
            scope: self.scopes.clone(),
            order: self.order,
        });
        self.order += 1;
        self.var_of.insert(key(name.span), Var::Local(i));
        if let Some(s) = self.scope_locals.last_mut() {
            s.push(i);
        }
        Var::Local(i)
    }

    fn var_ty(&self, v: Var) -> Ty {
        match v {
            Var::Local(i) => self.locals[i as usize].ty.clone(),
            _ => Ty::Unknown,
        }
    }

    /// The roots a value gets when it goes where type `to` is expected: a value place
    /// converts to a reference or a slice of itself (REFERENCES §3.2).
    fn convert(&self, v: &Val, from: &Ty, to: &Ty, span: Span) -> R {
        if matches!(to, Ty::Slice(_) | Ty::View(_)) && moves(from) {
            let r = R::Elems(Box::new(v.cell.clone().unwrap_or_else(|| v.refs.clone())));
            self.pending.borrow_mut().push((r, span));
        }
        if !self.an.holds_refs(to) {
            return R::Empty;
        }
        let from_value =
            !Analysis::refish(from) && !matches!(from, Ty::Null | Ty::Unknown | Ty::Func(..));
        if Analysis::refish(to) && from_value {
            let cell = v
                .cell
                .clone()
                .unwrap_or_else(|| R::temp(TempKind::Value, span));
            return if Analysis::whole_ref(to) {
                R::Whole(Box::new(cell))
            } else {
                R::Elems(Box::new(cell))
            };
        }
        // A view of a whole array's elements, from a reference to the array.
        if Analysis::whole_ref(from) && !Analysis::whole_ref(to) {
            return R::Elems(Box::new(v.refs.clone()));
        }
        v.refs.clone()
    }

    /// The storage reached through `e` for its fields or elements.
    fn storage(&self, ty: &Ty, v: &Val, span: Span) -> R {
        if Analysis::refish(ty) {
            v.refs.clone()
        } else if matches!(ty.unref(), Ty::Unknown) {
            v.cell.clone().unwrap_or_else(|| v.refs.clone())
        } else {
            v.cell
                .clone()
                .unwrap_or_else(|| R::temp(TempKind::Value, span))
        }
    }

    fn field_cell(&self, base: R, cls: ClassId, idx: usize) -> R {
        let c = &self.an.prog.classes[cls];
        let ty = &c.fields[idx].ty;
        if c.kind == ClassKind::Root && starts_empty(ty) || Analysis::removable_ty(ty) {
            let part = Part {
                class: cls,
                field: idx as u32,
            };
            R::Part(Box::new(base), part)
        } else {
            base
        }
    }

    /// Replacing the value at `cell`, of type `t`, removes what its own storage held: an
    /// array's, map's or set's elements, a variable-size value's parts, or the elements of
    /// a class value's fields that are storage of their own (REFERENCES §5.2, §6).
    fn replaces(&mut self, cell: R, t: &Ty, span: Span, what: String) {
        let t = t.unref();
        if matches!(t, Ty::Array(..) | Ty::Map(..) | Ty::Set(..)) || self.an.prog.variable_size(t) {
            self.ev(Ev::Remove {
                target: cell,
                span,
                what,
            });
            return;
        }
        let parts = self.an.parts_of(t);
        if parts.is_empty() {
            return;
        }
        let target = parts.into_iter().fold(R::Empty, |r, p| {
            R::union(r, R::Part(Box::new(cell.clone()), p))
        });
        self.ev(Ev::Remove { target, span, what });
    }

    /// A value of this type built by `new` into an array is built in its slot: one with
    /// self-relative links (REFERENCES §7.4), or a variable-size record (§6, V4).
    fn in_place_ty(&self, el: &Ty) -> bool {
        self.an.prog.link_bearing(el) || self.an.prog.variable_size(el)
    }

    /// A variable the code doesn't name, in the current scope.
    fn hidden(&mut self, name: &str, ty: Ty, span: Span) -> Var {
        let i = self.locals.len() as u32;
        self.locals.push(LocalVar {
            name: name.to_string(),
            ty,
            span,
            scope: self.scopes.clone(),
            order: self.order,
        });
        self.order += 1;
        if let Some(s) = self.scope_locals.last_mut() {
            s.push(i);
        }
        Var::Local(i)
    }

    /// The method `name` with no parameters of class `c`, or of an interface's
    /// implementations.
    fn method_callees(&self, c: ClassId, name: &str) -> Vec<FnId> {
        let prog = self.an.prog;
        let Some(i) = prog.classes[c]
            .methods
            .iter()
            .position(|m| m.name == name && m.params.is_empty() && !m.is_static)
        else {
            return Vec::new();
        };
        if prog.classes[c].kind == ClassKind::Interface {
            (0..prog.classes.len())
                .filter_map(|k| {
                    prog.implementation(k, c, i)
                        .and_then(|j| self.an.fn_of.get(&(k, false, j)))
                })
                .copied()
                .collect()
        } else {
            self.an
                .fn_of
                .get(&(c, false, i))
                .copied()
                .into_iter()
                .collect()
        }
    }

    /// A call of method `name`, with no arguments, on the value `recv` names.
    fn method_call(&mut self, c: ClassId, name: &str, recv: R, span: Span) -> Option<Rc<CallR>> {
        let callees = self.method_callees(c, name);
        if callees.is_empty() {
            return None;
        }
        let call = Rc::new(CallR {
            callees,
            recv: Some(recv),
            recv_held: None,
            args: Vec::new(),
            into: None,
            value: None,
        });
        let in_scope = self.resizable_in_scope();
        self.ev(Ev::Call(CallEv {
            call: call.clone(),
            span,
            name: name.into(),
            uses: Vec::new(),
            in_scope,
            by_value: Vec::new(),
            is_new: false,
        }));
        Some(call)
    }

    /// For a for-each over a value of class `c`: the class whose `next()` each pass calls,
    /// and the variable that holds the iterator, which is set here.
    fn iterator_of(&mut self, c: ClassId, it: &Ty, iv: &Val, span: Span) -> Option<(ClassId, Var)> {
        let prog = self.an.prog;
        let find = |c: ClassId, name: &str| {
            prog.classes[c]
                .methods
                .iter()
                .find(|m| m.name == name && m.params.is_empty() && !m.is_static)
        };
        let (cls, src) = if find(c, "next").is_some() {
            (c, iv.refs.clone())
        } else {
            let m = find(c, "iterator")?;
            let Ty::Class(ic, _) = m.ret.unref().clone() else {
                return None;
            };
            let recv = self.storage(it, iv, span);
            let call = self.method_call(c, "iterator", recv, span)?;
            (ic, R::Call(call, 0))
        };
        let var = self.hidden("the for-each's iterator", Ty::Class(cls, Vec::new()), span);
        self.ev(Ev::Store {
            dst: R::root(var),
            src,
            cell: true,
            span,
        });
        self.ev(Ev::Def(var));
        Some((cls, var))
    }

    fn resizable_in_scope(&self) -> Vec<u32> {
        self.scope_locals
            .iter()
            .flatten()
            .copied()
            .filter(|&l| self.an.resizable_ty(&self.locals[l as usize].ty))
            .collect()
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self, b: &ast::Block) {
        self.push_scope();
        self.defers.push(Vec::new());
        let body = self.sub(|me| {
            for s in &b.stmts {
                me.stmt(s, None);
            }
        });
        let defers = self.defers.pop().unwrap_or_default();
        self.pop_scope();
        self.emit(Node::Block { body, defers });
    }

    fn stmt(&mut self, s: &Stmt, label: Option<String>) {
        match &s.kind {
            S::Block(b) => self.block(b),
            S::Local(d) => {
                for v in &d.decls {
                    match &v.init {
                        Some(init) => {
                            let val = self.expr(init);
                            let from = self.ty(init);
                            let var = self.declare(&v.name);
                            let to = self.var_ty(var);
                            let src = self.convert(&val, &from, &to, init.span);
                            self.ev(Ev::Store {
                                dst: R::root(var),
                                src,
                                cell: true,
                                span: init.span,
                            });
                            self.ev(Ev::Def(var));
                        }
                        None => {
                            self.declare(&v.name);
                        }
                    }
                    for n in v.capture.iter().flat_map(|c| c.names()) {
                        if let Name::Ident(c) = n {
                            self.declare(c);
                        }
                    }
                }
            }
            S::If { cond, then, els } => {
                self.push_scope();
                self.expr(cond);
                let a = self.sub(|me| me.stmt(then, None));
                let b = self.sub(|me| {
                    if let Some(e) = els {
                        me.stmt(e, None);
                    }
                });
                self.emit(Node::If(a, b));
                self.pop_scope();
            }
            S::While { cond, body } => {
                let pre = self.sub(|me| {
                    me.expr(cond);
                });
                let body = self.sub(|me| me.stmt(body, None));
                let exit = if is_true(cond) {
                    Exit::Never
                } else {
                    Exit::Pre
                };
                self.emit(Node::Loop(Box::new(Loop {
                    label,
                    pre,
                    body,
                    post: Vec::new(),
                    exit,
                    holder: None,
                })));
            }
            S::DoWhile { body, cond } => {
                let body = self.sub(|me| me.stmt(body, None));
                let post = self.sub(|me| {
                    me.expr(cond);
                });
                let exit = if is_true(cond) {
                    Exit::Never
                } else {
                    Exit::Post
                };
                self.emit(Node::Loop(Box::new(Loop {
                    label,
                    pre: Vec::new(),
                    body,
                    post,
                    exit,
                    holder: None,
                })));
            }
            S::For {
                init,
                cond,
                update,
                body,
            } => {
                self.push_scope();
                let outer = self.sub(|me| {
                    match init {
                        Some(ast::ForInit::Decl(d)) => me.stmt(
                            &Stmt {
                                kind: S::Local(d.clone()),
                                span: s.span,
                            },
                            None,
                        ),
                        Some(ast::ForInit::Exprs(es)) => {
                            for e in es {
                                me.expr(e);
                            }
                        }
                        None => {}
                    }
                    let pre = me.sub(|me| {
                        if let Some(c) = cond {
                            me.expr(c);
                        }
                    });
                    let body = me.sub(|me| me.stmt(body, None));
                    let post = me.sub(|me| {
                        for u in update {
                            me.expr(u);
                        }
                    });
                    let exit = match cond {
                        Some(c) if !is_true(c) => Exit::Pre,
                        _ => Exit::Never,
                    };
                    me.emit(Node::Loop(Box::new(Loop {
                        label,
                        pre,
                        body,
                        post,
                        exit,
                        holder: None,
                    })));
                });
                self.pop_scope();
                self.emit(Node::Block {
                    body: outer,
                    defers: Vec::new(),
                });
            }
            S::ForEach {
                ty,
                name,
                iter,
                body,
            } => {
                let iv = self.expr(iter);
                let it = self.ty(iter);
                let (held, holder) = match it.unref() {
                    Ty::Class(..) | Ty::Unknown => (iv.refs.clone(), iv.refs.clone()),
                    _ => {
                        let st = R::Elems(Box::new(self.storage(&it, &iv, iter.span)));
                        (st.clone(), st)
                    }
                };
                // Over an iterator, or a class with `iterator()`: the loop keeps the
                // iterator in a variable of its own, just outside it, and each pass calls
                // its `next()`, whose result the loop variable is (DESIGN §9.4).
                let next_of = match it.unref() {
                    Ty::Class(c, _) => self.iterator_of(*c, &it, &iv, iter.span),
                    _ => None,
                };
                self.push_scope();
                let by_ref = matches!(ty, LocalType::VarRef)
                    || matches!(ty, LocalType::Type(t) if matches!(t.kind, ast::TypeKind::Ref { .. }));
                let pre = self.sub(|me| {
                    let next = next_of.map(|(cls, at)| {
                        me.ev(Ev::Use(at));
                        me.method_call(cls, "next", R::root(at), iter.span)
                    });
                    if let Name::Ident(n) = name {
                        let var = me.declare(n);
                        let elem = if let Some(Some(c)) = &next {
                            R::Call(c.clone(), 0)
                        } else if by_ref || matches!(it.unref(), Ty::Class(..) | Ty::Unknown) {
                            held.clone()
                        } else {
                            R::stored(held.clone())
                        };
                        let to = me.var_ty(var);
                        let src = if me.an.holds_refs(&to) {
                            elem
                        } else {
                            R::Empty
                        };
                        me.ev(Ev::Store {
                            dst: R::root(var),
                            src,
                            cell: true,
                            span: iter.span,
                        });
                        me.ev(Ev::Def(var));
                    }
                });
                let body = self.sub(|me| me.stmt(body, None));
                self.pop_scope();
                self.emit(Node::Loop(Box::new(Loop {
                    label,
                    pre,
                    body,
                    post: Vec::new(),
                    exit: Exit::Pre,
                    holder: Some((holder, iter.span)),
                })));
            }
            S::Repeat {
                target,
                source,
                capture,
                body,
            } => {
                for n in capture.iter().flat_map(|c| c.names()) {
                    if let Name::Ident(c) = n {
                        self.declare(c);
                    }
                }
                self.push_scope();
                let mut val = Val::none();
                let pre = self.sub(|me| {
                    val = me.expr(source);
                });
                let st = self.ty(source);
                let body = self.sub(|me| {
                    match target {
                        ast::LoopTarget::Single(_, Name::Ident(n)) => {
                            let var = me.declare(n);
                            let to = me.var_ty(var);
                            let src = me.convert(&val, &st, &to, source.span);
                            me.ev(Ev::Store {
                                dst: R::root(var),
                                src,
                                cell: true,
                                span: source.span,
                            });
                            me.ev(Ev::Def(var));
                        }
                        ast::LoopTarget::Tuple(items) => {
                            for (k, item) in items.iter().enumerate() {
                                if let ast::LoopItem::Decl(_, Name::Ident(n)) = item {
                                    let var = me.declare(n);
                                    let r = val
                                        .elems
                                        .as_ref()
                                        .and_then(|e| e.get(k))
                                        .map_or(val.refs.clone(), |v| v.refs.clone());
                                    let to = me.var_ty(var);
                                    let src = if me.an.holds_refs(&to) { r } else { R::Empty };
                                    me.ev(Ev::Store {
                                        dst: R::root(var),
                                        src,
                                        cell: true,
                                        span: source.span,
                                    });
                                    me.ev(Ev::Def(var));
                                }
                            }
                        }
                        _ => {}
                    }
                    me.stmt(body, None);
                });
                self.pop_scope();
                self.emit(Node::Loop(Box::new(Loop {
                    label,
                    pre,
                    body,
                    post: Vec::new(),
                    exit: Exit::Pre,
                    holder: None,
                })));
            }
            S::Switch(sw) => {
                self.switch(sw, None);
            }
            S::Break(l) => self.emit(Node::Break(l.as_ref().map(|i| i.name.clone()))),
            S::Continue(l) => self.emit(Node::Continue(l.as_ref().map(|i| i.name.clone()))),
            S::Return { status, value } => {
                if let Some(ast::StatusWrite::Expr(e)) = status {
                    self.expr(e);
                }
                let mut values = Vec::new();
                if let Some(v) = value {
                    let val = self.expr(v);
                    // A `return` in a lambda's block returns from the lambda.
                    let ret = match self.lambda_rets.last() {
                        Some((t, _)) if self.lambda_depth > 0 => t.clone(),
                        _ => self.fn_ret(),
                    };
                    match (&ret, &val.elems, &v.kind) {
                        (Ty::Tuple(ts), Some(elems), E::Tuple(xs)) => {
                            for ((t, ev), x) in ts.iter().zip(elems).zip(xs) {
                                let from = self.ty(x);
                                values.push(self.convert(ev, &from, t, x.span));
                            }
                        }
                        (Ty::Tuple(ts), Some(elems), _) => {
                            for (_, ev) in ts.iter().zip(elems) {
                                values.push(ev.refs.clone());
                            }
                        }
                        _ => {
                            let from = self.ty(v);
                            values.push(self.convert(&val, &from, &ret, v.span));
                        }
                    }
                }
                if self.lambda_depth == 0 {
                    self.ev(Ev::Ret {
                        values,
                        span: s.span,
                    });
                    self.emit(Node::Return);
                } else if let Some((_, r)) = self.lambda_rets.last_mut() {
                    for v in values {
                        *r = R::union(std::mem::replace(r, R::Empty), v);
                    }
                }
            }
            S::Destructure { targets, value, .. } => {
                let val = self.expr(value);
                for (k, t) in targets.iter().enumerate() {
                    let r = val
                        .elems
                        .as_ref()
                        .and_then(|e| e.get(k))
                        .map_or(val.refs.clone(), |v| v.refs.clone());
                    match t {
                        Target::Decl(_, Name::Ident(n)) => {
                            let var = self.declare(n);
                            let to = self.var_ty(var);
                            let src = if self.an.holds_refs(&to) { r } else { R::Empty };
                            self.ev(Ev::Store {
                                dst: R::root(var),
                                src,
                                cell: true,
                                span: value.span,
                            });
                            self.ev(Ev::Def(var));
                        }
                        Target::Existing(Name::Ident(n)) => {
                            match self.name_var(n.span) {
                                // A reference variable is written through, as `=` does.
                                Some(var)
                                    if self.local_ty(var).is_some_and(|t| {
                                        matches!(t, Ty::Ref(_))
                                            || matches!(&t, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)))
                                    }) =>
                                {
                                    self.ev(Ev::Use(var));
                                    self.ev(Ev::Store {
                                        dst: R::stored(R::root(var)),
                                        src: r,
                                        cell: false,
                                        span: value.span,
                                    });
                                }
                                Some(var) => {
                                    self.ev(Ev::Store {
                                        dst: R::root(var),
                                        src: r,
                                        cell: true,
                                        span: value.span,
                                    });
                                    self.ev(Ev::Def(var));
                                }
                                // A field or a static.
                                None => {
                                    let place = self.ident(&n.name, n.span, true);
                                    if let Some(cell) = place.cell {
                                        self.ev(Ev::Store {
                                            dst: cell,
                                            src: r,
                                            cell: false,
                                            span: value.span,
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            S::Defer(inner) => {
                let nodes = self.sub(|me| me.stmt(inner, None));
                // Deferred statements run last first, so the newest goes first.
                if let Some(d) = self.defers.last_mut() {
                    d.splice(0..0, nodes);
                }
            }
            S::Yield(e) => {
                let v = self.expr(e);
                let want = self.yield_tys.last().cloned().unwrap_or(Ty::Unknown);
                let c = self.convert(&v, &self.ty(e), &want, e.span);
                if let Some(y) = self.yields.last_mut() {
                    y.push(R::union(v.refs, c));
                }
            }
            S::Labeled { label, stmt } => self.stmt(stmt, Some(label.name.clone())),
            S::Expr(e) => {
                self.expr(e);
            }
            S::If_(b) => {
                for st in active(b) {
                    self.stmt(st, None);
                }
            }
            S::Empty => {}
        }
    }

    fn fn_ret(&self) -> Ty {
        self.ret.clone()
    }

    fn name_var(&self, span: Span) -> Option<Var> {
        match self.an.facts.names.get(&key(span))? {
            NameRes::Local(d) => self.var_of.get(d).copied(),
            NameRes::Field(..) | NameRes::CConst(_) => None,
        }
    }

    /// A switch statement, or with the type of its value a switch expression.
    fn switch(&mut self, sw: &ast::Switch, ty: Option<Ty>) -> R {
        self.expr(&sw.scrutinee);
        let is_expr = ty.is_some();
        let every = is_expr || sw.rules.iter().any(|r| r.labels.is_none());
        let mut arms = Vec::new();
        let mut result = R::Empty;
        let want = ty.unwrap_or(Ty::Unknown);
        for rule in &sw.rules {
            let mut r = R::Empty;
            let nodes = self.sub(|me| match &rule.body {
                SwitchBody::Expr(x) => {
                    let v = me.expr(x);
                    // As the switch's type: an array arm of a `string` switch is a view.
                    let c = me.convert(&v, &me.ty(x), &want, x.span);
                    r = R::union(v.refs, c);
                }
                SwitchBody::Block(b) => {
                    me.yields.push(Vec::new());
                    me.yield_tys.push(want.clone());
                    me.block(b);
                    me.yield_tys.pop();
                    for y in me.yields.pop().unwrap_or_default() {
                        r = R::union(r.clone(), y);
                    }
                }
            });
            result = R::union(result, r);
            arms.push(nodes);
        }
        self.emit(Node::Switch(arms, every));
        result
    }

    // ------------------------------------------------------------ expressions

    fn expr(&mut self, e: &Expr) -> Val {
        match &e.kind {
            E::Str(parts) => {
                let mut interp = false;
                for p in parts {
                    if let StrPart::Interp(x) = p {
                        self.expr(x);
                        interp = true;
                    }
                }
                Val::refs(if interp {
                    R::temp(TempKind::Text, e.span)
                } else {
                    R::root(Var::Literal)
                })
            }
            E::Ident(n) => self.ident(n, e.span, false),
            E::This => Val::refs(R::root(Var::This)),
            E::ThisNew(args) => {
                let call = self.user_call(e.span, Some(R::root(Var::This)), args, false, None);
                if let Some(c) = call {
                    self.ev(Ev::Store {
                        dst: R::root(Var::This),
                        src: R::Call(c, 0),
                        cell: false,
                        span: e.span,
                    });
                }
                Val::none()
            }
            E::New { args, .. } => {
                let into = self.new_into.take();
                let call = self.user_call(e.span, None, args, true, into);
                match call {
                    Some(c) => Val::refs(R::Call(c, 0)),
                    None => Val::refs(self.unknown_args(R::Empty, args)),
                }
            }
            E::ArrayLit(items) => {
                let mut r = R::Empty;
                for x in items {
                    let v = self.expr(x);
                    let from = self.ty(x);
                    let to = match self.ty(e) {
                        Ty::Array(el, _) | Ty::Slice(el) => *el,
                        _ => Ty::Unknown,
                    };
                    r = R::union(r, self.convert(&v, &from, &to, x.span));
                }
                Val::refs(r)
            }
            E::Switch(sw) => {
                let r = self.switch(sw, Some(self.ty(e)));
                Val::refs(r)
            }
            E::Paren(x) => self.expr(x),
            E::Tuple(xs) => {
                let elems: Vec<Val> = xs.iter().map(|x| self.expr(x)).collect();
                let refs = elems
                    .iter()
                    .fold(R::Empty, |a, v| R::union(a, v.refs.clone()));
                Val {
                    cell: None,
                    refs,
                    elems: Some(elems),
                }
            }
            E::Lambda { params, body } => {
                let to_builtin = std::mem::take(&mut self.lambda_to_builtin);
                let elems = if to_builtin {
                    self.lambda_elems.take()
                } else {
                    None
                };
                let ret = match self.ty(e) {
                    Ty::Func(_, r) => *r,
                    _ => Ty::Unknown,
                };
                self.lambda_depth += 1;
                self.push_scope();
                let scope = self.scopes.last().copied().unwrap_or(0);
                self.lambda_rets.push((ret.clone(), R::Empty));
                let mut res = R::Empty;
                let mut param_locals = Vec::new();
                let mut acc = None;
                let nodes = self.sub(|me| {
                    for (pos, p) in params.iter().enumerate() {
                        if let Name::Ident(n) = &p.name {
                            let v = me.declare(n);
                            let pt = me.var_ty(v);
                            let Var::Local(l) = v else { continue };
                            param_locals.push(l);
                            // Passed straight to a built-in, a parameter is an element of
                            // what it works on, or `fold`'s running result; otherwise what
                            // the lambda's caller passed.
                            let src = match &elems {
                                Some((_, name, others)) if name == "fold" && pos == 0 => {
                                    acc = Some(v);
                                    others.clone()
                                }
                                Some((el, ..)) if matches!(pt, Ty::Ref(_)) => el.clone(),
                                Some((el, ..)) if me.an.holds_refs(&pt) => R::stored(el.clone()),
                                Some(_) => R::Empty,
                                None if me.an.holds_refs(&pt) => R::Root(Root {
                                    var: Var::Arg(l),
                                    part: None,
                                    whole: Analysis::whole_ref(&pt),
                                }),
                                None => R::Empty,
                            };
                            me.ev(Ev::Store {
                                dst: R::root(v),
                                src,
                                cell: true,
                                span: n.span,
                            });
                            me.ev(Ev::Def(v));
                        }
                    }
                    match body.as_ref() {
                        LambdaBody::Expr(x) => {
                            let v = me.expr(x);
                            let from = me.ty(x);
                            res = me.convert(&v, &from, &ret, x.span);
                        }
                        LambdaBody::Block(b) => me.block(b),
                    }
                    let returned = me.lambda_rets.pop().map_or(R::Empty, |(_, r)| r);
                    res = R::union(std::mem::replace(&mut res, R::Empty), returned);
                    // `fold` passes what one call returns to the next.
                    if let Some(a) = acc {
                        me.ev(Ev::Carry {
                            dst: R::root(a),
                            src: res.clone(),
                        });
                    }
                    // Only a built-in uses what the lambda returns before the end of the
                    // statement, so anywhere else it can't be a temporary (DESIGN §9.3).
                    if !to_builtin && !matches!(res, R::Empty) {
                        me.ev(Ev::LambdaRet {
                            value: res.clone(),
                            scope,
                            span: e.span,
                        });
                    }
                });
                self.lambda_res.insert(key(e.span), res);
                self.pop_scope();
                self.lambda_depth -= 1;
                match elems {
                    // A built-in calls it once for each element, or not at all, while it
                    // works through them, as a for-each does.
                    Some((el, ..)) => self.emit(Node::Loop(Box::new(Loop {
                        label: None,
                        pre: Vec::new(),
                        body: nodes,
                        post: Vec::new(),
                        exit: Exit::Pre,
                        holder: Some((el, e.span)),
                    }))),
                    None => {
                        if !to_builtin {
                            self.an
                                .lambdas
                                .borrow_mut()
                                .push((self.fn_id, param_locals));
                        }
                        self.emit(Node::Block {
                            body: nodes,
                            defers: Vec::new(),
                        })
                    }
                }
                if self.an.facts.capturing.contains(&key(e.span)) {
                    Val::refs(R::temp(TempKind::Lambda, e.span))
                } else {
                    Val::none()
                }
            }
            E::Assign {
                op, target, value, ..
            } => self.assign(*op, target, value, e.span),
            E::StatusAssign { value, .. } => {
                self.expr(value);
                Val::none()
            }
            E::Cond { cond, then, els } => {
                self.expr(cond);
                let mut a = Val::none();
                let mut b = Val::none();
                let ta = self.sub(|me| a = me.expr(then));
                let tb = self.sub(|me| b = me.expr(els));
                self.emit(Node::If(ta, tb));
                let cell = match (a.cell.clone(), b.cell.clone()) {
                    (Some(x), Some(y)) => Some(R::union(x, y)),
                    _ => None,
                };
                // Each arm as the whole expression's type: an array arm of a `string`
                // `?:` is a view of that array.
                let ct = self.ty(e);
                let ra = self.convert(&a, &self.ty(then), &ct, then.span);
                let rb = self.convert(&b, &self.ty(els), &ct, els.span);
                Val {
                    cell,
                    refs: R::union(R::union(a.refs, b.refs), R::union(ra, rb)),
                    elems: None,
                }
            }
            E::Binary { op, lhs, rhs } => match op {
                ast::BinOp::InsertEnd | ast::BinOp::InsertFront | ast::BinOp::InsertCursor => {
                    let lv = self.expr(lhs);
                    let lt = self.ty(lhs);
                    let dst = self.storage(&lt, &lv, lhs.span);
                    if lt.elem().is_some_and(|el| self.in_place_ty(el)) && is_new(rhs) {
                        self.new_into = Some(R::Elems(Box::new(dst.clone())));
                    }
                    let rv = self.expr(rhs);
                    self.new_into = None;
                    self.ev(Ev::Grow {
                        target: R::Elems(Box::new(dst.clone())),
                    });
                    let rt = self.ty(rhs);
                    if moves(&lt) {
                        let sym = match op {
                            ast::BinOp::InsertEnd => "+>",
                            ast::BinOp::InsertFront => "+<",
                            _ => "+^",
                        };
                        self.ev(Ev::Change {
                            target: R::Elems(Box::new(dst.clone())),
                            span: e.span,
                            what: format!("`{}`", sym),
                            kind: Change::Move,
                        });
                    }
                    let elem = lt.elem().cloned().unwrap_or(Ty::Unknown);
                    let src = self.convert(&rv, &rt, &elem, rhs.span);
                    // A link into an array of them: what it refers to must be in the
                    // array the value holding them is in (L1).
                    if self.link_array_of(lhs) {
                        self.ev(Ev::Link {
                            dst: R::container(&dst),
                            src: src.clone(),
                            pool: false,
                            span: rhs.span,
                        });
                    }
                    self.ev(Ev::Store {
                        dst,
                        src,
                        cell: false,
                        span: rhs.span,
                    });
                    lv
                }
                ast::BinOp::And | ast::BinOp::Or => {
                    self.expr(lhs);
                    let r = self.sub(|me| {
                        me.expr(rhs);
                    });
                    self.emit(Node::If(r, Vec::new()));
                    Val::none()
                }
                _ => {
                    let lv = self.expr(lhs);
                    let rv = self.expr(rhs);
                    // Comparing arrays compares slices of them.
                    if matches!(
                        op,
                        ast::BinOp::Eq
                            | ast::BinOp::Ne
                            | ast::BinOp::Lt
                            | ast::BinOp::Gt
                            | ast::BinOp::Le
                            | ast::BinOp::Ge
                    ) {
                        for (x, v) in [(lhs, lv), (rhs, rv)] {
                            let t = self.ty(x);
                            if moves(&t) {
                                let r = R::Elems(Box::new(self.storage(&t, &v, x.span)));
                                self.ev(Ev::Change {
                                    target: r,
                                    span: e.span,
                                    what: "this comparison".into(),
                                    kind: Change::Rearrange,
                                });
                            }
                        }
                    }
                    Val::none()
                }
            },
            E::InstanceOf { expr, bind, .. } => {
                let v = self.expr(expr);
                if let Some(b) = bind {
                    let var = self.declare(b);
                    self.ev(Ev::Store {
                        dst: R::root(var),
                        src: v.refs,
                        cell: true,
                        span: e.span,
                    });
                    self.ev(Ev::Def(var));
                }
                Val::none()
            }
            E::Unary { expr, .. } | E::StatusRead { expr, .. } | E::Cast { expr, .. } => {
                self.expr(expr);
                Val::none()
            }
            E::Postfix { op, expr } => {
                let v = self.expr(expr);
                match op {
                    ast::PostfixOp::Inc | ast::PostfixOp::Dec => Val::none(),
                    _ => {
                        let t = self.ty(expr);
                        let target = self.storage(&t, &v, expr.span);
                        let sym = match op {
                            ast::PostfixOp::RemoveEnd => "->",
                            ast::PostfixOp::RemoveFront => "-<",
                            ast::PostfixOp::RemoveBefore => "-^",
                            _ => "^-",
                        };
                        self.ev(Ev::Remove {
                            target: target.clone(),
                            span: e.span,
                            what: format!("`{}`", sym),
                        });
                        Val::refs(R::stored(target))
                    }
                }
            }
            E::Member { expr, name } => self.member(expr, name),
            E::Call { callee, args } => self.call(e, callee, args),
            E::Index { expr, index, .. } => {
                let v = self.expr(expr);
                self.expr(index);
                let t = self.ty(expr);
                Val::place(R::Elems(Box::new(self.storage(&t, &v, expr.span))))
            }
            E::Slice { expr, lo, hi } => {
                let v = self.expr(expr);
                for b in [lo, hi].into_iter().flatten() {
                    self.expr(&b.expr);
                }
                let t = self.ty(expr);
                let r = R::Elems(Box::new(self.storage(&t, &v, expr.span)));
                if moves(&t) {
                    self.ev(Ev::Change {
                        target: r.clone(),
                        span: e.span,
                        what: "this slice".into(),
                        kind: Change::Rearrange,
                    });
                }
                Val::refs(r)
            }
            _ => Val::none(),
        }
    }

    fn ident(&mut self, n: &str, span: Span, writing: bool) -> Val {
        match self.an.facts.names.get(&key(span)) {
            Some(NameRes::Local(d)) => match self.var_of.get(d).copied() {
                Some(v) => {
                    if !writing {
                        self.ev(Ev::Use(v));
                    }
                    Val::place(R::root(v))
                }
                None => Val::none(),
            },
            Some(NameRes::CConst(_)) => Val::none(),
            Some(NameRes::Field(c, i)) => {
                let f = &self.an.prog.classes[*c].fields[*i];
                if f.is_static {
                    match self.an.static_of.get(&(*c, *i)) {
                        Some(id) => Val::place(R::root(Var::Static(*id))),
                        None => Val::none(),
                    }
                } else if self.has_this {
                    Val::place(self.field_cell(R::root(Var::This), *c, *i))
                } else {
                    Val::none()
                }
            }
            // The standard streams live for the whole program, as static data does.
            None if matches!(n, "stdin" | "stdout" | "stderr") => Val::place(R::root(Var::Literal)),
            None => Val::none(),
        }
    }

    fn member(&mut self, x: &Expr, name: &ast::Ident) -> Val {
        let xt = self.ty(x);
        if let Ty::TypeName(t) = &xt {
            if let Ty::Class(c, _) = &**t
                && let Some(i) = self.an.prog.classes[*c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name)
                && let Some(id) = self.an.static_of.get(&(*c, i))
            {
                return Val::place(R::root(Var::Static(*id)));
            }
            return Val::none();
        }
        let v = self.expr(x);
        let base = self.storage(&xt, &v, x.span);
        match xt.unref() {
            Ty::Class(c, _) => match self.an.prog.classes[*c]
                .fields
                .iter()
                .position(|f| f.name == name.name)
            {
                Some(i) => Val::place(self.field_cell(base, *c, i)),
                None => Val::none(),
            },
            Ty::MapEntry(..) | Ty::Unknown | Ty::Param(_) => Val::place(base),
            _ => Val::none(), // `len`, `cursor`
        }
    }

    fn assign(&mut self, op: ast::AssignOp, target: &Expr, value: &Expr, span: Span) -> Val {
        let vv = self.expr(value);
        let vt = self.ty(value);
        if matches!(target.kind, E::Discard) {
            return Val::none();
        }
        let tt = self.ty(target);
        let compound = !matches!(op, ast::AssignOp::Assign | ast::AssignOp::Rebind);
        // A plain local or parameter: its own storage.
        if let E::Ident(_) = &target.kind
            && let Some(var) = self.name_var(target.span)
        {
            let declared = self.local_ty(var).unwrap_or(tt.clone());
            if compound {
                self.ev(Ev::Use(var));
                return Val::none();
            }
            let is_ref = matches!(declared, Ty::Ref(_))
                || matches!(&declared, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)));
            let binds = self.an.facts.binds.contains(&key(span));
            if op == ast::AssignOp::Assign && is_ref && !binds {
                // `=` writes through a reference, replacing what it refers to.
                self.ev(Ev::Use(var));
                let deref = declared.unref().clone();
                self.replaces(
                    R::stored(R::root(var)),
                    &deref,
                    span,
                    format!("assigning through `{}`", self.var_name(var)),
                );
                let src = self.convert(&vv, &vt, &deref, value.span);
                self.ev(Ev::Store {
                    dst: R::stored(R::root(var)),
                    src,
                    cell: false,
                    span,
                });
                return Val::none();
            }
            // V2: assigning a variable-size value rebuilds it (REFERENCES §6), and any
            // whole value replaces what its parts held.
            if op == ast::AssignOp::Assign {
                let what = format!("assigning `{}`", self.var_name(var));
                self.replaces(R::root(var), &declared, span, what);
            }
            let src = self.convert(&vv, &vt, &declared, value.span);
            self.ev(Ev::Store {
                dst: R::root(var),
                src,
                cell: true,
                span,
            });
            self.ev(Ev::Def(var));
            return Val::none();
        }
        // A field, an element, or what a reference refers to.
        let tv = self.target(target);
        if compound {
            return Val::none();
        }
        if let Some(cell) = tv.cell.clone() {
            let is_ref = matches!(tt, Ty::Ref(_))
                || matches!(&tt, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)));
            if op == ast::AssignOp::Assign {
                // A reference field written through replaces what it refers to.
                let replaced = if is_ref && !self.an.facts.binds.contains(&key(span)) {
                    R::stored(cell.clone())
                } else {
                    cell.clone()
                };
                if !is_ref || !self.an.facts.binds.contains(&key(span)) {
                    self.replaces(replaced, &tt, span, "this assignment".into());
                }
            }
            // `=` on a reference field writes through it, unless it binds it in a constructor.
            let through =
                op == ast::AssignOp::Assign && is_ref && !self.an.facts.binds.contains(&key(span));
            let to = if op == ast::AssignOp::Rebind || (is_ref && !through) {
                tt.clone()
            } else {
                tt.unref().clone()
            };
            let cell = if through { R::stored(cell) } else { cell };
            let dst = if matches!(target.kind, E::Slice { .. }) {
                tv.refs.clone()
            } else {
                cell
            };
            let src = if matches!(target.kind, E::Slice { .. }) {
                R::stored(self.convert(&vv, &vt, &Ty::Slice(Box::new(Ty::Unknown)), value.span))
            } else {
                self.convert(&vv, &vt, &to, value.span)
            };
            // Binding a link: what it refers to must be in its array (L1).
            if !through
                && let Some(pool) = self.link_of(target)
            {
                let link_dst = match pool {
                    Some(id) => R::root(Var::Static(id)),
                    // An element of an array of links: the value holding the array.
                    None if self.link_array_elem(target) => R::container(&dst),
                    None => dst.clone(),
                };
                self.ev(Ev::Link {
                    dst: link_dst,
                    src: src.clone(),
                    pool: pool.is_some(),
                    span,
                });
            }
            self.ev(Ev::Store {
                dst,
                src,
                cell: false,
                span,
            });
        } else if matches!(target.kind, E::Slice { .. }) {
            let src =
                R::stored(self.convert(&vv, &vt, &Ty::Slice(Box::new(Ty::Unknown)), value.span));
            self.ev(Ev::Store {
                dst: tv.refs.clone(),
                src,
                cell: false,
                span,
            });
        }
        Val::none()
    }

    /// When `t` is a field holding a link: `None` for a self-relative one, or the index of
    /// the static array a pool link is measured from (REFERENCES §7.2, §7.3).
    fn link_of(&self, t: &Expr) -> Option<Option<u32>> {
        let (c, i) = match &t.kind {
            E::Paren(x) => return self.link_of(x),
            // An element of an array of links.
            E::Index { expr, .. } => {
                return self
                    .link_array_of(expr)
                    .then_some(None);
            }
            E::Member { expr, name } => {
                let Ty::Class(c, _) = self.ty(expr).unref().clone() else {
                    return None;
                };
                let i = self.an.prog.classes[c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name)?;
                (c, i)
            }
            E::Ident(_) => match self.an.facts.names.get(&key(t.span)) {
                Some(NameRes::Field(c, i)) => (*c, *i),
                _ => return None,
            },
            _ => return None,
        };
        self.an.link_of_field(c, i)
    }

    /// Whether `t` is a field holding an array of self-relative links, or an insertion
    /// chain into one: `n.kids`, `n.kids +> a` (REFERENCES §7.2).
    fn link_array_of(&self, t: &Expr) -> bool {
        let (c, i) = match &t.kind {
            E::Paren(x) => return self.link_array_of(x),
            E::Binary {
                op: ast::BinOp::InsertEnd | ast::BinOp::InsertFront | ast::BinOp::InsertCursor,
                lhs,
                ..
            } => return self.link_array_of(lhs),
            E::Member { expr, name } => {
                let Ty::Class(c, _) = self.ty(expr).unref().clone() else {
                    return false;
                };
                match self.an.prog.classes[c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name)
                {
                    Some(i) => (c, i),
                    None => return false,
                }
            }
            E::Ident(_) => match self.an.facts.names.get(&key(t.span)) {
                Some(NameRes::Field(c, i)) => (*c, *i),
                _ => return false,
            },
            _ => return false,
        };
        self.an.prog.classes[c].fields[i].elem_link.is_some()
    }

    /// Whether `t` is an element of an array of links: `n.kids[i]`.
    fn link_array_elem(&self, t: &Expr) -> bool {
        match &t.kind {
            E::Paren(x) => self.link_array_elem(x),
            E::Index { expr, .. } => self.link_array_of(expr),
            _ => false,
        }
    }

    /// Evaluates an assignment target, which is written rather than read.
    fn target(&mut self, t: &Expr) -> Val {
        match &t.kind {
            E::Ident(n) => self.ident(n, t.span, true),
            E::Paren(x) => self.target(x),
            _ => self.expr(t),
        }
    }

    fn local_ty(&self, v: Var) -> Option<Ty> {
        match v {
            Var::Local(i) => Some(self.locals[i as usize].ty.clone()),
            Var::Param(i) => self
                .param_spans
                .get(i as usize)
                .and_then(|s| self.an.facts.locals.get(&key(*s)))
                .cloned(),
            _ => None,
        }
    }

    fn var_name(&self, v: Var) -> String {
        match v {
            Var::Local(i) => self.locals[i as usize].name.clone(),
            _ => "it".into(),
        }
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[ast::Arg]) -> Val {
        let fact = self.an.facts.calls.get(&key(e.span));
        let mut recv_held = None;
        let (recv, recv_ty): (Option<R>, Ty) = match &callee.kind {
            E::Member { expr: base, .. } => {
                let bt = self.ty(base);
                if matches!(bt, Ty::TypeName(_) | Ty::Namespace(_)) {
                    (None, bt)
                } else {
                    // `r.bump(a)`: a free function given the receiver as its first
                    // argument reads it as it reads the others.
                    let recv_arg = fact.is_some_and(|f| f.recv_arg);
                    if recv_arg {
                        self.collect.push(Vec::new());
                    }
                    let bv = self.expr(base);
                    if recv_arg {
                        self.recv_uses = self.collect.pop().unwrap_or_default();
                    }
                    if bv.cell.is_none()
                        && !Analysis::refish(&bt)
                        && !matches!(bt.unref(), Ty::Unknown)
                    {
                        recv_held = Some(bv.refs.clone());
                    }
                    // A library object the checker doesn't model is a handle: what a call
                    // gives back is rooted at what it holds.
                    let r = if matches!(bt.unref(), Ty::Unknown) {
                        bv.cell.clone().map_or(bv.refs.clone(), R::stored)
                    } else {
                        self.storage(&bt, &bv, base.span)
                    };
                    (Some(r), bt)
                }
            }
            E::Ident(_) => {
                if let Some(v) = self.name_var(callee.span) {
                    self.ev(Ev::Use(v));
                }
                let implicit_this = matches!(fact, Some(CallFact { origin: Origin::Method(c, i), .. }) if !self.an.prog.classes[*c].methods[*i].is_static && self.an.prog.classes[*c].kind != ClassKind::Script);
                (
                    if implicit_this && self.has_this {
                        Some(R::root(Var::This))
                    } else {
                        None
                    },
                    Ty::Unknown,
                )
            }
            E::PrimMember { .. } => (None, Ty::Unknown),
            _ => {
                self.expr(callee);
                (None, Ty::Unknown)
            }
        };
        let Some(fact) = fact else {
            let base = recv.clone().unwrap_or(R::Empty);
            return Val::refs(self.unknown_args(base, args));
        };
        match &fact.origin {
            Origin::Builtin => self.builtin(e, &fact.name, recv, &recv_ty, args, &fact.params),
            // A C function gives back numbers or handles, which hold no references.
            Origin::Extern(_) => {
                for a in args {
                    self.expr(&a.expr);
                }
                Val::none()
            }
            // It may run any lambda with as many parameters: their summaries decide what
            // it does to the arguments, and its result is rooted at them (REFERENCES §4.3).
            Origin::Value => {
                self.collect.push(Vec::new());
                let mut arg_rs = Vec::new();
                for (k, a) in args.iter().enumerate() {
                    let v = self.expr(&a.expr);
                    let at = self.ty(&a.expr);
                    let p = fact.params.get(k).cloned().unwrap_or(Ty::Unknown);
                    arg_rs.push(self.convert(&v, &at, &p, a.expr.span));
                }
                let uses = self.collect.pop().unwrap_or_default();
                let call = Rc::new(CallR {
                    callees: Vec::new(),
                    recv: None,
                    recv_held: None,
                    args: arg_rs,
                    into: None,
                    value: Some(args.len()),
                });
                let in_scope = self.resizable_in_scope();
                self.ev(Ev::Call(CallEv {
                    call: call.clone(),
                    span: e.span,
                    name: fact.name.clone(),
                    uses,
                    in_scope,
                    by_value: Vec::new(),
                    is_new: false,
                }));
                Val::refs(R::Call(call, 0))
            }
            Origin::Method(..) | Origin::Ctor(..) => {
                // `g.split(...)`: a free function given the receiver as a slice, which
                // makes a ring or gap buffer one block.
                if fact.recv_arg
                    && moves(&recv_ty)
                    && matches!(fact.params.first(), Some(Ty::Slice(_) | Ty::View(_)))
                    && let Some(r) = &recv
                {
                    self.ev(Ev::Change {
                        target: R::Elems(Box::new(r.clone())),
                        span: e.span,
                        what: format!("`{}`", fact.name),
                        kind: Change::Rearrange,
                    });
                }
                let ret = self.ty(e);
                match self.user_call_with(e.span, recv, recv_held, args, fact, false, None) {
                    Some(c) => match &ret {
                        Ty::Tuple(ts) => {
                            let elems: Vec<Val> = (0..ts.len())
                                .map(|k| Val::refs(R::Call(c.clone(), k)))
                                .collect();
                            Val {
                                cell: None,
                                refs: R::Call(c, 0),
                                elems: Some(elems),
                            }
                        }
                        t if self.an.holds_refs(t) => Val::refs(R::Call(c, 0)),
                        _ => Val::none(),
                    },
                    None => Val::none(),
                }
            }
        }
    }

    /// A library call the checker doesn't model: its result may be rooted at anything
    /// it was given, and at what that holds.
    fn unknown_args(&mut self, mut r: R, args: &[ast::Arg]) -> R {
        for a in args {
            let v = self.expr(&a.expr);
            let at = self.ty(&a.expr);
            let as_ref = self.convert(&v, &at, &Ty::Ref(Box::new(Ty::Unknown)), a.expr.span);
            r = R::union(r, R::union(v.refs, as_ref));
        }
        r
    }

    /// A `new C(...)` or `this.new(...)`, by its recorded constructor.
    fn user_call(
        &mut self,
        span: Span,
        recv: Option<R>,
        args: &[ast::Arg],
        is_new: bool,
        into: Option<R>,
    ) -> Option<Rc<CallR>> {
        let fact = self.an.facts.calls.get(&key(span))?;
        self.user_call_with(span, recv, None, args, fact, is_new, into)
    }

    #[allow(clippy::too_many_arguments)]
    fn user_call_with(
        &mut self,
        span: Span,
        recv: Option<R>,
        recv_held: Option<R>,
        args: &[ast::Arg],
        fact: &CallFact,
        is_new: bool,
        into: Option<R>,
    ) -> Option<Rc<CallR>> {
        let prog = self.an.prog;
        let callees: Vec<FnId> = match &fact.origin {
            Origin::Method(c, i) => {
                let ci = &prog.classes[*c];
                if ci.kind == ClassKind::Interface {
                    // Every class that implements it (REFERENCES §4.3).
                    let mut v = Vec::new();
                    for k in 0..prog.classes.len() {
                        if let Some(j) = prog.implementation(k, *c, *i)
                            && let Some(f) = self.an.fn_of.get(&(k, false, j))
                        {
                            v.push(*f);
                        }
                    }
                    v
                } else {
                    self.an
                        .fn_of
                        .get(&(*c, false, *i))
                        .copied()
                        .into_iter()
                        .collect()
                }
            }
            Origin::Ctor(c, i) => self
                .an
                .fn_of
                .get(&(*c, true, *i))
                .copied()
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        self.collect.push(Vec::new());
        let shift = usize::from(fact.recv_arg);
        let mut arg_rs = Vec::new();
        let mut by_value = Vec::new();
        let mut recv = recv;
        if fact.recv_arg {
            // `x.helper()`: the receiver is the first argument.
            if let Some(r) = recv.take() {
                let p = fact.params.first().cloned().unwrap_or(Ty::Unknown);
                let r = if Analysis::refish(&p) {
                    r
                } else {
                    R::stored(r)
                };
                arg_rs.push(r);
            }
        }
        for (k, a) in args.iter().enumerate() {
            let v = self.expr(&a.expr);
            let at = self.ty(&a.expr);
            let p = fact.params.get(k + shift).cloned().unwrap_or(Ty::Unknown);
            // The function is compiled again for this layout (DESIGN §4.3), but checked
            // for its own, whose elements stay where they are.
            if moves(&at) && matches!(p.unref(), Ty::Array(..)) && !moves(&p) {
                let prog = self.an.prog;
                self.ev(Ev::Refuse {
                    span: a.expr.span,
                    msg: {
                        let shown = prog.show(at.unref());
                        let want = prog.show(&p);
                        format!("{} `{}` can't be passed to `{}`, which takes {} `{}`: an insert, a sort or a slice can move this one's elements, and the function is checked for an array whose elements stay where they are", crate::check::article(&shown), shown, fact.name, crate::check::article(&want), want)
                    },
                    help: "pass a slice of it, or give the parameter the buffer's own type (DESIGN §4.3, REFERENCES §5.4)".into(),
                });
            }
            if self.an.resizable_ty(&p) {
                by_value.push(a.expr.span);
            }
            arg_rs.push(self.convert(&v, &at, &p, a.expr.span));
        }
        let mut uses = self.collect.pop().unwrap_or_default();
        uses.append(&mut self.recv_uses);
        let call = Rc::new(CallR {
            callees: callees.clone(),
            recv,
            recv_held,
            args: arg_rs,
            into,
            value: None,
        });
        let in_scope = self.resizable_in_scope();
        self.ev(Ev::Call(CallEv {
            call: call.clone(),
            span,
            name: fact.name.clone(),
            uses,
            in_scope,
            by_value,
            is_new: is_new && matches!(fact.origin, Origin::Ctor(..)),
        }));
        Some(call)
    }

    /// The built-in methods and functions: what they store, remove and return (REFERENCES §3.2, §5.1).
    fn builtin(
        &mut self,
        e: &Expr,
        name: &str,
        recv: Option<R>,
        recv_ty: &Ty,
        args: &[ast::Arg],
        params: &[Ty],
    ) -> Val {
        let mut vals = Vec::new();
        // The array `filter` and the like build is growable while their lambda runs, as
        // a growable local would be (§8.4): a call in it can't recurse back here.
        let builds = matches!(name, "filter" | "map" | "sortedIndexes" | "sortedIndexesBy")
            && matches!(recv_ty.unref(), Ty::Array(..) | Ty::Slice(_) | Ty::View(_));
        if builds {
            self.push_scope();
            let i = self.locals.len() as u32;
            self.locals.push(LocalVar {
                name: format!("`{}`'s result", name),
                ty: Ty::Array(Box::new(Ty::i64()), ArrayKind::grow()),
                span: e.span,
                scope: self.scopes.clone(),
                order: self.order,
            });
            self.order += 1;
            if let Some(s) = self.scope_locals.last_mut() {
                s.push(i);
            }
        }
        let in_place = matches!(name, "push" | "pushFront" | "insert")
            && recv_ty
                .unref()
                .elem()
                .is_some_and(|el| self.in_place_ty(el));
        let mut others = R::Empty;
        for a in args {
            if in_place && is_new(&a.expr) {
                self.new_into = recv.clone().map(|r| R::Elems(Box::new(r)));
            }
            self.lambda_to_builtin = lambda_of(&a.expr).is_some();
            if self.lambda_to_builtin {
                self.lambda_elems = Some((
                    R::Elems(Box::new(recv.clone().unwrap_or(R::Empty))),
                    name.to_string(),
                    others.clone(),
                ));
            }
            let v = self.expr(&a.expr);
            self.lambda_elems = None;
            others = R::union(others, v.refs.clone());
            self.lambda_to_builtin = false;
            self.new_into = None;
            vals.push((v, self.ty(&a.expr), a.expr.span));
        }
        if builds {
            self.pop_scope();
        }
        let arg = |k: usize, me: &Self| -> R {
            match vals.get(k) {
                Some((v, t, s)) => {
                    let p = params.get(k).cloned().unwrap_or(Ty::Unknown);
                    me.convert(v, t, &p, *s)
                }
                None => R::Empty,
            }
        };
        // Converting an argument to a slice can make a ring or gap buffer one block, so
        // every argument is converted, whether its roots are used or not.
        for k in 0..vals.len() {
            arg(k, self);
        }
        let all_args = |me: &Self| (0..vals.len()).fold(R::Empty, |r, k| R::union(r, arg(k, me)));
        // What the built-in gives back from its arguments: a lambda's result rather than
        // the lambda, which a built-in only calls.
        let results = |me: &Self| {
            (0..vals.len()).fold(R::Empty, |r, k| {
                let a = match lambda_of(&args[k].expr) {
                    Some(l) => me.lambda_res.get(&key(l.span)).cloned().unwrap_or(R::Empty),
                    None => arg(k, me),
                };
                R::union(r, a)
            })
        };
        let Some(recv) = recv else {
            // A function: `min`, `max` and `clamp` give back one of their arguments.
            return match name {
                "min" | "max" | "clamp" => Val::refs(results(self)),
                "message" | "name" => Val::refs(R::root(Var::Literal)),
                _ => Val::none(),
            };
        };
        let span = e.span;
        let recv = R::Elems(Box::new(recv));
        let grows = match recv_ty.unref() {
            Ty::Array(..) => matches!(
                name,
                "push"
                    | "pushFront"
                    | "insert"
                    | "insertAt"
                    | "pushAll"
                    | "append"
                    | "appendUtf8"
                    | "heapPush"
            ),
            Ty::Map(..) => matches!(name, "put" | "getOrAdd"),
            Ty::Set(_) | Ty::Pool(_) => name == "add",
            _ => false,
        };
        if grows {
            self.ev(Ev::Grow {
                target: recv.clone(),
            });
        }
        let store = |me: &mut Self, src: R| {
            me.ev(Ev::Store {
                dst: recv.clone(),
                src,
                cell: false,
                span,
            })
        };
        let remove = |me: &mut Self| {
            me.ev(Ev::Remove {
                target: recv.clone(),
                span,
                what: format!("`{}`", name),
            })
        };
        let rt = recv_ty.unref();
        if moves(rt) {
            // Inserts can move the elements of a ring or gap buffer; anything that works on
            // it as one slice, a sort or a text method, makes it one block first. Only
            // what goes element by element leaves it as it is.
            let first_string = params.first().is_some_and(|p| p.is_string());
            let kind = match name {
                "push" | "pushFront" | "insert" | "insertAt" | "pushAll" | "heapPush"
                | "append" | "appendUtf8" => Some(Change::Move),
                // A text method takes its receiver as one string.
                _ if crate::builtins::slice_receiver(name, first_string) => Some(Change::Rearrange),
                "pop" | "popFront" | "removeBefore" | "removeAfter" | "removeAt" | "clear"
                | "truncate" | "dedup" | "retain" | "seek" | "seekStart" | "seekFromEnd"
                | "move" | "get" | "getOr" | "fill" | "swap" | "reverse" | "shuffle"
                | "contains" | "indexOf" | "sum" | "min" | "max" | "isSorted" | "isSortedBy"
                | "count" | "all" | "any" | "findIndex" | "findAny" | "filter" | "map" | "fold"
                | "sortedIndexes" | "sortedIndexesBy" => None,
                _ => Some(Change::Rearrange),
            };
            if let Some(kind) = kind {
                self.ev(Ev::Change {
                    target: recv.clone(),
                    span,
                    what: format!("`{}`", name),
                    kind,
                });
            }
        }
        // Moving whole values between slots replaces each one: references into their
        // own storage end, as an assignment ends them.
        if let Some(el) = rt.elem()
            && matches!(
                name,
                "swap"
                    | "sort"
                    | "stableSort"
                    | "sortDescending"
                    | "sortBy"
                    | "stableSortBy"
                    | "sortByDescending"
                    | "radixSort"
                    | "radixSortBy"
                    | "partialSort"
                    | "partialSortBy"
                    | "selectNth"
                    | "selectNthBy"
                    | "reverse"
                    | "shuffle"
                    | "fill"
                    | "heapify"
                    | "heapPush"
                    | "heapPop"
            )
        {
            let el = el.clone();
            self.replaces(recv.clone(), &el, span, format!("`{}`", name));
        }
        match rt {
            Ty::Array(..) | Ty::Slice(_) | Ty::View(_) => match name {
                "push" | "pushFront" | "insert" => {
                    let a = arg(0, self);
                    store(self, a);
                    Val::refs(recv.clone())
                }
                "insertAt" => {
                    let a = arg(1, self);
                    store(self, a);
                    Val::refs(recv.clone())
                }
                "pop" | "popFront" | "removeBefore" | "removeAfter" | "removeAt" | "heapPop" => {
                    remove(self);
                    Val::refs(R::stored(recv.clone()))
                }
                "clear" | "retain" | "dedup" | "truncate" => {
                    remove(self);
                    Val::none()
                }
                "heapPush" | "fill" => {
                    let a = arg(0, self);
                    store(self, a);
                    Val::none()
                }
                "pushAll" => {
                    let a = R::stored(arg(0, self));
                    store(self, a);
                    Val::none()
                }
                "getOr" => Val::refs(R::union(R::stored(recv.clone()), arg(1, self))),
                "selectNth" | "selectNthBy" | "chunks" | "windows" => Val::refs(recv.clone()),
                "min" | "max" | "filter" => Val::refs(R::stored(recv.clone())),
                "map" | "fold" => Val::refs(R::union(R::stored(recv.clone()), results(self))),
                "split" | "splitBy" | "lines" | "words" | "trim" | "trimStart" | "trimEnd"
                | "trimPrefix" | "trimSuffix" | "codePoints" => Val::refs(recv.clone()),
                "splitFirst" | "splitLast" => Val {
                    cell: None,
                    refs: recv.clone(),
                    elems: Some(vec![Val::refs(recv.clone()), Val::refs(recv.clone())]),
                },
                _ => Val::none(),
            },
            Ty::Map(..) => match name {
                "get" => Val::refs(R::stored(recv.clone())),
                "getOr" => Val::refs(R::union(R::stored(recv.clone()), arg(1, self))),
                "getOrAdd" => {
                    let a = R::union(arg(0, self), arg(1, self));
                    store(self, a);
                    Val::refs(recv.clone())
                }
                "put" => {
                    let a = R::union(arg(0, self), arg(1, self));
                    store(self, a);
                    Val::none()
                }
                "remove" | "clear" => {
                    remove(self);
                    Val::none()
                }
                _ => Val::none(),
            },
            Ty::Set(_) => match name {
                "add" => {
                    let a = arg(0, self);
                    store(self, a);
                    Val::none()
                }
                "remove" | "clear" => {
                    remove(self);
                    Val::none()
                }
                _ => Val::none(),
            },
            Ty::Pool(_) => match name {
                "add" => {
                    let a = arg(0, self);
                    store(self, a);
                    Val::none()
                }
                "get" => Val::refs(recv.clone()),
                _ => Val::none(),
            },
            Ty::Unknown | Ty::Class(..) => Val::refs(R::union(recv.clone(), all_args(self))),
            _ => Val::none(),
        }
    }
}

fn is_true(e: &Expr) -> bool {
    match &e.kind {
        E::Bool(true) => true,
        E::Paren(x) => is_true(x),
        _ => false,
    }
}

/// A capped ring or gap buffer whose elements can move (REFERENCES §5.4).
fn moves(t: &Ty) -> bool {
    matches!(t.unref(), Ty::Array(_, k) if k.moves_elements())
}

/// The lambda an argument is, through any parentheses.
fn lambda_of(e: &Expr) -> Option<&Expr> {
    match &e.kind {
        E::Lambda { .. } => Some(e),
        E::Paren(x) => lambda_of(x),
        _ => None,
    }
}

/// Each class type a value of `t` holds by value: itself, or in its elements, keys,
/// values or tuple parts. Behind a reference or a view a value holds references anyway.
fn classes_in(t: &Ty, f: &mut impl FnMut(ClassId, &[Ty])) {
    match t {
        Ty::Class(c, a) => f(*c, a),
        Ty::Array(e, _) | Ty::Set(e) | Ty::Pool(e) => classes_in(e, f),
        Ty::Map(k, v) => {
            classes_in(k, f);
            classes_in(v, f);
        }
        Ty::Tuple(ts) => ts.iter().for_each(|t| classes_in(t, f)),
        _ => {}
    }
}

/// A variable's name for a message: in backticks, unless it is a description of one the
/// code doesn't name.
fn quoted(name: &str) -> String {
    if name.contains(' ') {
        name.to_string()
    } else {
        format!("`{}`", name)
    }
}
