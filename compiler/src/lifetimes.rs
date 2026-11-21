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
//! Not yet: rules V (variable-size values) and L (links).

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

    /// Whether a reference rooted at `self` may point into the elements of `other`.
    fn overlaps(&self, other: &Root) -> bool {
        !self.whole
            && self.var == other.var
            && (self.part.is_none() || other.part.is_none() || self.part == other.part)
    }
}

type Roots = BTreeSet<Root>;

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
    args: Vec<R>,
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
    name: String,
    kind: FnKind,
    params: Vec<(String, Ty)>,
    locals: Vec<LocalVar>,
    body: Vec<Node>,
}

/// What an event does to references into an array, for rule R.
#[derive(Clone, Copy, PartialEq, Debug)]
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
}

// ---------------------------------------------------------------- entry

/// Runs the checks on a program that type-checked without errors.
pub fn check(prog: &Program, facts: &Facts, sources: &[SourceFile]) -> Vec<(usize, Diagnostic)> {
    let mut an = Analysis::new(prog, facts, sources);
    an.build();
    an.solve();
    an.report();
    an.diags
}

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
    holds_memo: std::cell::RefCell<HashMap<ClassId, bool>>,
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
            holds_memo: Default::default(),
        }
    }

    // ------------------------------------------------------------ types

    /// Whether a value of this type holds references, so it has roots.
    fn holds_refs(&self, t: &Ty) -> bool {
        match t {
            Ty::Ref(_)
            | Ty::Nullable(_)
            | Ty::Slice(_)
            | Ty::View(_)
            | Ty::Func(..)
            | Ty::Iter(_)
            | Ty::MapEntry(..) => true,
            Ty::Param(_) | Ty::Unknown => true,
            Ty::Array(e, _) | Ty::Set(e) | Ty::Pool(e) => self.holds_refs(e),
            Ty::Map(k, v) => self.holds_refs(k) || self.holds_refs(v),
            Ty::Tuple(ts) => ts.iter().any(|t| self.holds_refs(t)),
            Ty::Class(id, args) => {
                let c = &self.prog.classes[*id];
                if c.kind == ClassKind::Enum || c.kind == ClassKind::Script {
                    return false;
                }
                if args.is_empty() {
                    if let Some(b) = self.holds_memo.borrow().get(id) {
                        return *b;
                    }
                    self.holds_memo.borrow_mut().insert(*id, false); // a cycle holds nothing new
                }
                let map: Vec<(String, Ty)> = c
                    .type_params
                    .iter()
                    .map(|p| p.name.clone())
                    .zip(args.iter().cloned())
                    .collect();
                let b = c.kind == ClassKind::Interface
                    || c.fields
                        .iter()
                        .filter(|f| !f.is_static)
                        .any(|f| self.holds_refs(&f.ty.subst(&map)));
                if args.is_empty() {
                    self.holds_memo.borrow_mut().insert(*id, b);
                }
                b
            }
            _ => false,
        }
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

    /// Exempt from rule R: pools (REFERENCES §5.4).
    fn exempt_ty(t: &Ty) -> bool {
        matches!(t.unref(), Ty::Pool(_))
    }

    /// Resizable or variable-size: needs a data stack of its own (§8.1, §8.4).
    fn resizable_ty(&self, t: &Ty) -> bool {
        match t {
            Ty::Array(_, k) => k.cap == CapKind::None,
            Ty::Map(..) | Ty::Set(_) | Ty::Pool(_) => true,
            Ty::Class(id, _) => self.prog.classes[*id].kind == ClassKind::Root,
            _ => false,
        }
    }

    // ------------------------------------------------------------ building the flow form

    fn build(&mut self) {
        let prog = self.prog;
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
                let id = self.funcs.len();
                self.fn_of.insert((c, false, i), id);
                let mut b = Builder::new(self, !m.is_static);
                b.ret = m.ret.clone();
                let f = b.function(&m.decl.params, body);
                self.funcs.push(Func {
                    class: c,
                    file: ci.file,
                    name: m.name.clone(),
                    kind: FnKind::Method,
                    params,
                    locals: f.0,
                    body: f.1,
                });
            }
            for (i, k) in ci.ctors.iter().enumerate() {
                let id = self.funcs.len();
                self.fn_of.insert((c, true, i), id);
                match k.decl {
                    Some(d) => {
                        let params = d
                            .params
                            .iter()
                            .zip(&k.params)
                            .map(|(p, t)| (name_of(&p.name), t.clone()))
                            .collect();
                        let f = Builder::new(self, true).function(&d.params, &d.body);
                        self.funcs.push(Func {
                            class: c,
                            file: ci.file,
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
                        self.funcs.push(Func {
                            class: c,
                            file: ci.file,
                            name: format!("new {}", ci.name),
                            kind: FnKind::DefaultCtor,
                            params,
                            locals: Vec::new(),
                            body: Vec::new(),
                        });
                    }
                }
            }
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
            Var::ThisHeld => c
                .recv
                .as_ref()
                .map(|r| self.eval(rs, r))
                .unwrap_or_default()
                .iter()
                .flat_map(|x| self.stored(rs, x))
                .collect(),
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
        for _ in 0..64 {
            let mut changed = false;
            let mut add = |rs: &mut HashMap<Var, Roots>,
                           statics: &mut HashMap<u32, Roots>,
                           d: &Root,
                           src: &Roots| match d.var {
                Var::Static(id) => {
                    let e = statics.entry(id).or_default();
                    let n = e.len();
                    e.extend(src.iter().copied());
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
                    Ev::Store { dst, src, .. } => {
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
            return s;
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
                                if a.var.is_formal() && a.var != d.var {
                                    s.stores.insert((*a, d));
                                }
                            }
                        }
                    }
                }
                Ev::Remove { target, .. } => {
                    s.removes.extend(
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
                    for &g in &c.call.callees {
                        let sm = &self.summaries[g];
                        for (from, to) in [
                            (&sm.removes, &mut s.removes),
                            (&sm.moves, &mut s.moves),
                            (&sm.rearranges, &mut s.rearranges),
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
                                        if x.var.is_formal() && x.var != d.var {
                                            s.stores.insert((*x, d));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if f.kind == FnKind::Ctor {
            // What a constructor builds holds whatever it stored into `this`.
            let held: Roots = rs
                .get(&Var::This)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|r| keep(r) && !matches!(r.var, Var::This | Var::ThisHeld))
                .collect();
            s.results = vec![held];
        }
        s
    }

    /// Summaries for every function, and root sets for statics, by repeating until nothing changes (§4.3).
    fn solve(&mut self) {
        for _ in 0..100 {
            let mut changed = false;
            let mut statics = self.static_rs.clone();
            for i in 0..self.funcs.len() {
                let rs = self.root_sets(&self.funcs[i], &mut statics);
                let s = self.summarize(&self.funcs[i], &rs);
                if s != self.summaries[i] {
                    self.summaries[i] = s;
                    changed = true;
                }
            }
            if statics != self.static_rs {
                self.static_rs = statics;
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }

    // ------------------------------------------------------------ reporting

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
            Var::Local(i) => format!("`{}`", f.locals[i as usize].name),
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
                        self.rule_s(f, &s, &d, !*cell, *span, None, &mut errs);
                    }
                    Ev::Ret { values, span } => {
                        for v in values {
                            for r in self.eval(&rs, v) {
                                self.returned(f, &r, *span, &mut errs);
                            }
                        }
                    }
                    Ev::Call(c) => {
                        for &g in &c.call.callees {
                            for (a, b) in &self.summaries[g].stores {
                                let s = self.map_formal(&rs, &c.call, a);
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
                                    errs.push((
                                        c.span,
                                        format!("`{}` is growable and in scope during this call, which can recurse back into `{}`", lv.name, f.name),
                                        "a growable local would need a new data stack on every call; pass it in by reference from outside the cycle (REFERENCES §8.4)".into(),
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
                                format!("{}a reference into `{}` can't be stored in `{}`: `{}` {}, so it is released first", prefix, la.name, lb.name, la.name, why),
                                format!("declare `{}` before `{}`, in the same block or an enclosing one (REFERENCES §3.3)", la.name, lb.name),
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
                            for &g in &c.call.callees {
                                let sm = &self.summaries[g];
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
                    Ev::Store { .. } | Ev::Ret { .. } => {}
                },
                Node::Block { body, defers } => {
                    // A deferred statement runs when the block ends (REFERENCES §9).
                    let end = self.live(f, rs, statics, defers, &live, ctx, report, errs);
                    ctx.defers.push(end.clone());
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
                Node::Break(label) => {
                    live = ctx.target(label).map_or_else(Live::new, |t| t.0.clone())
                }
                Node::Continue(label) => {
                    live = ctx.target(label).map_or_else(Live::new, |t| t.1.clone())
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
        for _ in 0..50 {
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
        ctx.loops
            .push((l.label.clone(), after.clone(), post_start.clone()));
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

    #[allow(clippy::too_many_arguments)]
    fn removal(
        &self,
        f: &Func,
        rs: &HashMap<Var, Roots>,
        statics: &HashMap<u32, Roots>,
        target: &Roots,
        live: &Live,
        ctx: &LiveCtx,
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
    /// Enclosing loops: label, live at their exit, live where `continue` goes.
    loops: Vec<(Option<String>, Live, Live)>,
    /// Live at the end of each enclosing block, where its defers run.
    defers: Vec<Live>,
    /// Enclosing for-each loops: what they loop over.
    holders: Vec<(Roots, Span)>,
}

impl LiveCtx {
    fn target(&self, label: &Option<String>) -> Option<(&Live, &Live)> {
        let l = match label {
            None => self.loops.last(),
            Some(n) => self.loops.iter().rev().find(|l| l.0.as_deref() == Some(n)),
        }?;
        Some((&l.1, &l.2))
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
    /// Collecting the values of `yield`s.
    yields: Vec<Vec<R>>,
    lambda_depth: usize,
    /// Rearrangements from converting a ring or gap buffer to a slice, which `convert`
    /// finds; emitted before the next event.
    pending: std::cell::RefCell<Vec<(R, Span)>>,
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
            lambda_depth: 0,
            pending: Default::default(),
        }
    }

    fn function(mut self, params: &[ast::Param], body: &ast::Block) -> (Vec<LocalVar>, Vec<Node>) {
        for (i, p) in params.iter().enumerate() {
            if let Name::Ident(id) = &p.name {
                self.var_of.insert(key(id.span), Var::Param(i as u32));
            }
        }
        self.block(body);
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
                    if let Some(Name::Ident(c)) = &v.capture {
                        self.declare(c);
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
                    Ty::Iter(_) | Ty::Class(..) | Ty::Unknown => (iv.refs.clone(), iv.refs.clone()),
                    _ => {
                        let st = R::Elems(Box::new(self.storage(&it, &iv, iter.span)));
                        (st.clone(), st)
                    }
                };
                self.push_scope();
                let by_ref = matches!(ty, LocalType::VarRef)
                    || matches!(ty, LocalType::Type(t) if matches!(t.kind, ast::TypeKind::Ref { .. }));
                let pre = self.sub(|me| {
                    if let Name::Ident(n) = name {
                        let var = me.declare(n);
                        let elem = if by_ref
                            || matches!(it.unref(), Ty::Iter(_) | Ty::Class(..) | Ty::Unknown)
                        {
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
                if let Some(Name::Ident(c)) = capture {
                    self.declare(c);
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
                self.switch(sw, false);
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
                    let ret = self.fn_ret();
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
                            if let Some(var) = self.name_var(n.span) {
                                self.ev(Ev::Store {
                                    dst: R::root(var),
                                    src: r,
                                    cell: true,
                                    span: value.span,
                                });
                                self.ev(Ev::Def(var));
                            }
                        }
                        _ => {}
                    }
                }
            }
            S::Defer(inner) => {
                let nodes = self.sub(|me| me.stmt(inner, None));
                if let Some(d) = self.defers.last_mut() {
                    d.extend(nodes);
                }
            }
            S::Yield(e) => {
                let v = self.expr(e);
                if let Some(y) = self.yields.last_mut() {
                    y.push(v.refs);
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

    fn switch(&mut self, sw: &ast::Switch, is_expr: bool) -> R {
        self.expr(&sw.scrutinee);
        let every = is_expr || sw.rules.iter().any(|r| r.labels.is_none());
        let mut arms = Vec::new();
        let mut result = R::Empty;
        for rule in &sw.rules {
            let mut r = R::Empty;
            let nodes = self.sub(|me| match &rule.body {
                SwitchBody::Expr(x) => r = me.expr(x).refs,
                SwitchBody::Block(b) => {
                    me.yields.push(Vec::new());
                    me.block(b);
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
                let call = self.user_call(e.span, Some(R::root(Var::This)), args, false);
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
            E::New { args, .. } => match self.user_call(e.span, None, args, true) {
                Some(c) => Val::refs(R::Call(c, 0)),
                None => Val::refs(self.unknown_args(R::Empty, args)),
            },
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
                let r = self.switch(sw, true);
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
                self.lambda_depth += 1;
                self.push_scope();
                let nodes = self.sub(|me| {
                    for p in params {
                        if let Name::Ident(n) = &p.name {
                            let v = me.declare(n);
                            me.ev(Ev::Def(v));
                        }
                    }
                    match body.as_ref() {
                        LambdaBody::Expr(x) => {
                            me.expr(x);
                        }
                        LambdaBody::Block(b) => me.block(b),
                    }
                });
                self.pop_scope();
                self.lambda_depth -= 1;
                self.emit(Node::Block {
                    body: nodes,
                    defers: Vec::new(),
                });
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
                Val {
                    cell,
                    refs: R::union(a.refs, b.refs),
                    elems: None,
                }
            }
            E::Binary { op, lhs, rhs } => match op {
                ast::BinOp::InsertEnd | ast::BinOp::InsertFront | ast::BinOp::InsertCursor => {
                    let lv = self.expr(lhs);
                    let lt = self.ty(lhs);
                    let rv = self.expr(rhs);
                    let rt = self.ty(rhs);
                    let dst = self.storage(&lt, &lv, lhs.span);
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
            E::Unary { expr, .. } | E::StatusRead(expr) | E::Cast { expr, .. } => {
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

    fn ident(&mut self, _n: &str, span: Span, writing: bool) -> Val {
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
                // `=` writes through a reference.
                self.ev(Ev::Use(var));
                let deref = declared.unref().clone();
                let src = self.convert(&vv, &vt, &deref, value.span);
                self.ev(Ev::Store {
                    dst: R::stored(R::root(var)),
                    src,
                    cell: false,
                    span,
                });
                return Val::none();
            }
            if op == ast::AssignOp::Assign
                && matches!(declared, Ty::Array(..) | Ty::Map(..) | Ty::Set(..))
            {
                self.ev(Ev::Remove {
                    target: R::root(var),
                    span,
                    what: format!("assigning `{}`", self.var_name(var)),
                });
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
            if op == ast::AssignOp::Assign
                && matches!(tt.unref(), Ty::Array(..) | Ty::Map(..) | Ty::Set(..))
            {
                self.ev(Ev::Remove {
                    target: cell.clone(),
                    span,
                    what: "this assignment".into(),
                });
            }
            let is_ref = matches!(tt, Ty::Ref(_))
                || matches!(&tt, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)));
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
        let (recv, recv_ty): (Option<R>, Ty) = match &callee.kind {
            E::Member { expr: base, .. } => {
                let bt = self.ty(base);
                if matches!(bt, Ty::TypeName(_) | Ty::Namespace(_)) {
                    (None, bt)
                } else {
                    let bv = self.expr(base);
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
            Origin::Value => {
                let mut r = R::Empty;
                for a in args {
                    r = R::union(r, self.expr(&a.expr).refs);
                }
                Val::refs(r)
            }
            Origin::Method(..) | Origin::Ctor(..) => {
                let ret = self.ty(e);
                match self.user_call_with(e.span, recv, args, fact, false) {
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
    ) -> Option<Rc<CallR>> {
        let fact = self.an.facts.calls.get(&key(span))?;
        self.user_call_with(span, recv, args, fact, is_new)
    }

    fn user_call_with(
        &mut self,
        span: Span,
        recv: Option<R>,
        args: &[ast::Arg],
        fact: &CallFact,
        is_new: bool,
    ) -> Option<Rc<CallR>> {
        let prog = self.an.prog;
        let callees: Vec<FnId> = match &fact.origin {
            Origin::Method(c, i) => {
                let ci = &prog.classes[*c];
                if ci.kind == ClassKind::Interface {
                    // Every class that implements it (REFERENCES §4.3).
                    let m = &ci.methods[*i];
                    let mut v = Vec::new();
                    for (k, kc) in prog.classes.iter().enumerate() {
                        if kc
                            .implements
                            .iter()
                            .any(|t| matches!(t, Ty::Class(x, _) if x == c))
                            && let Some(j) = kc
                                .methods
                                .iter()
                                .position(|x| x.name == m.name && x.params.len() == m.params.len())
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
            if self.an.resizable_ty(&p) {
                by_value.push(a.expr.span);
            }
            arg_rs.push(self.convert(&v, &at, &p, a.expr.span));
        }
        let uses = self.collect.pop().unwrap_or_default();
        let call = Rc::new(CallR {
            callees: callees.clone(),
            recv,
            args: arg_rs,
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
        for a in args {
            let v = self.expr(&a.expr);
            vals.push((v, self.ty(&a.expr), a.expr.span));
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
        let all_args = |me: &Self| (0..vals.len()).fold(R::Empty, |r, k| R::union(r, arg(k, me)));
        let Some(recv) = recv else {
            // A function: `min`, `max` and `clamp` give back one of their arguments.
            return match name {
                "min" | "max" | "clamp" => Val::refs(all_args(self)),
                "message" | "name" => Val::refs(R::root(Var::Literal)),
                _ => Val::none(),
            };
        };
        let span = e.span;
        let recv = R::Elems(Box::new(recv));
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
            let first_string = params.first().is_some_and(|p| p.is_string());
            let kind = match name {
                "push" | "pushFront" | "insert" | "insertAt" | "pushAll" => Some(Change::Move),
                "sort" | "stableSort" | "sortDescending" | "sortBy" | "stableSortBy" => {
                    Some(Change::Rearrange)
                }
                _ if crate::builtins::slice_receiver(name, first_string) => Some(Change::Rearrange),
                _ => None,
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
                "map" | "fold" => Val::refs(R::union(R::stored(recv.clone()), all_args(self))),
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
