//! Loop versioning (DESIGN §14.2). A `for` loop that counts by a fixed step, and that
//! indexes arrays at its counter plus terms the loop doesn't change, is generated twice.
//! Before it, one test compares the first and the last index each such access can reach
//! with the length of its array, which the loop can't change either. When every one
//! fits, a copy of the loop without those index checks runs; otherwise the checked loop
//! runs, and fails at the same iteration it always did. So the checks cost one test per
//! loop instead of one per access, and the copy without them can be vectorized.
//!
//! Only innermost loops are versioned, so a body is generated at most twice. Everything
//! is decided from the syntax and the checker's facts, conservatively: a variable passed
//! anywhere that could change it, a call that could reach a static or an array through
//! a reference, or a shape this doesn't follow, and the loop is generated once, as before.

use super::*;
use crate::ast::{CastMode, CastTarget, ForInit, IndexMode};
use crate::types::lossless;

/// A variable the analysis follows: a local or parameter, by where its name is declared,
/// or a static field.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Var {
    Local((u32, u32)),
    Static(ClassId, usize),
}

/// What a loop's body does that bears on what stays fixed while it runs; or, `deep`,
/// what the whole program does to its variables.
#[derive(Default)]
struct Scan<'e> {
    /// Following every function's whole body, loops and lambdas too, for `aliased` only.
    deep: bool,
    /// Variables written, or handed to something that could write or rebind them.
    exposed: HashSet<Var>,
    /// Variables handed to something that could keep a reference to them: a reference
    /// declared or rebound to them, a reference parameter, a field, an array.
    aliased: HashSet<Var>,
    /// Locals declared as arrays that own their elements, not references to them.
    owned: HashSet<(u32, u32)>,
    /// In the deep scan, whether the function being scanned returns a reference, so
    /// what it returns could be written through.
    ret_ref: bool,
    /// Locals declared inside the body, new each iteration.
    declared: HashSet<(u32, u32)>,
    /// A call, `new` or method call that could change a static, or an array through a
    /// reference.
    opaque: bool,
    /// An insertion, a removal, or an assignment of a whole array or object, any of which
    /// could change the length of an array reached through a reference.
    resizes: bool,
    /// Something the analysis doesn't follow: a nested loop, a lambda, `defer`, `yield`,
    /// or a status capture.
    bail: bool,
    /// The checked index expressions on a variable: the whole `a[i]`, `a`, and `i`.
    indexes: Vec<(&'e Expr, &'e Expr, &'e Expr)>,
    /// The body's top-level declarations of one integer with a value, in order.
    top_locals: Vec<(&'e ast::Ident, &'e Expr)>,
}

/// How the loop counts: its counter, the conjunct of its condition that bounds it, and
/// its step.
struct Shape<'e> {
    counter: (u32, u32),
    cty: Prim,
    up: bool,
    bound: &'e Expr,
    inclusive: bool,
    /// `None` for `++` and `--`.
    step: Option<&'e Expr>,
}

/// An index as the counter plus a sum of terms the loop doesn't change: `(negated, term)`.
type Terms<'e> = Vec<(bool, &'e Expr)>;

pub(super) struct Plan {
    /// The C condition under which the copy without checks runs.
    pub check: String,
    /// The index expressions that copy doesn't check, by span.
    pub spans: HashSet<(u32, u32)>,
}

fn strip(e: &Expr) -> &Expr {
    match &e.kind {
        E::Paren(x) => strip(x),
        _ => e,
    }
}

/// An integer type of at most 64 bits, whose every value an `__int128` holds.
fn small_int(p: Prim) -> bool {
    p.is_int() && bits(p) <= 64
}

fn prim_range(p: Prim) -> (i128, i128) {
    match p {
        Prim::I8 => (i8::MIN as i128, i8::MAX as i128),
        Prim::I16 => (i16::MIN as i128, i16::MAX as i128),
        Prim::I32 => (i32::MIN as i128, i32::MAX as i128),
        Prim::U8 => (0, u8::MAX as i128),
        Prim::U16 => (0, u16::MAX as i128),
        Prim::U32 => (0, u32::MAX as i128),
        Prim::U64 => (0, u64::MAX as i128),
        _ => (i64::MIN as i128, i64::MAX as i128),
    }
}

/// An `__int128` constant, for a value of a type of at most 64 bits.
fn wide_lit(v: i128) -> String {
    if v == i64::MIN as i128 {
        "((__int128)(-9223372036854775807LL - 1))".to_string()
    } else if v < 0 {
        format!("((__int128)({}LL))", v)
    } else {
        format!("((__int128){}ULL)", v)
    }
}

fn wide(c: &str) -> String {
    format!("((__int128)({}))", c)
}

/// The expression of a body that is one expression statement, in braces or not.
fn only_expr(s: &Stmt) -> Option<&Expr> {
    match &s.kind {
        S::Expr(e) => Some(e),
        S::Block(b) => match b.stmts.as_slice() {
            [one] => only_expr(one),
            _ => None,
        },
        _ => None,
    }
}

/// The conjuncts of a condition: `a && b && c` as `[a, b, c]`.
fn conjuncts<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match &strip(e).kind {
        E::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => {
            conjuncts(lhs, out);
            conjuncts(rhs, out);
        }
        _ => out.push(strip(e)),
    }
}

impl<'p, 'a> Gen<'p, 'a> {
    /// The variables anywhere in the program that a reference could reach, since a
    /// reference taken before a loop could change them inside it, out of the loop's
    /// sight; and the locals declared as arrays of their own.
    pub(super) fn alias_scan(&self) -> (HashSet<Var>, HashSet<(u32, u32)>) {
        let mut sc = Scan {
            deep: true,
            ..Scan::default()
        };
        for c in &self.prog.classes {
            for m in &c.methods {
                if let Some(b) = &m.decl.body {
                    sc.ret_ref = is_ref(&m.ret);
                    for s in &b.stmts {
                        self.scan_stmt(s, false, &mut sc);
                    }
                }
            }
            for k in &c.ctors {
                sc.ret_ref = false;
                if let Some(d) = k.decl {
                    for s in &d.body.stmts {
                        self.scan_stmt(s, false, &mut sc);
                    }
                }
            }
            for f in &c.fields {
                if let Some(e) = f.init {
                    self.scan_expr(e, true, &mut sc);
                }
            }
        }
        (sc.aliased, sc.owned)
    }

    /// The test and the unchecked accesses of a loop that can be versioned. Called after
    /// its initializer is generated, so the counter has its C name.
    pub(super) fn version_plan(
        &mut self,
        init: &Option<ForInit>,
        cond: &Option<Expr>,
        update: &[Expr],
        body: &Stmt,
    ) -> Option<Plan> {
        let shape = self.loop_shape(init, cond.as_ref(), update)?;
        let mut sc = Scan::default();
        // The condition and the step run in the loop too; their own checks stay.
        if let Some(c) = cond {
            let mut cs = Scan::default();
            self.scan_expr(c, true, &mut cs);
            sc.exposed.extend(cs.exposed);
            sc.opaque |= cs.opaque;
            sc.resizes |= cs.resizes;
            sc.bail |= cs.bail;
        }
        if let Some(s) = shape.step {
            let mut cs = Scan::default();
            self.scan_expr(s, true, &mut cs);
            sc.exposed.extend(cs.exposed);
            sc.opaque |= cs.opaque;
            sc.resizes |= cs.resizes;
            sc.bail |= cs.bail;
        }
        match &body.kind {
            S::Block(b) => {
                for s in &b.stmts {
                    self.scan_stmt(s, true, &mut sc);
                }
            }
            _ => self.scan_stmt(body, false, &mut sc),
        }
        if sc.bail || sc.exposed.contains(&Var::Local(shape.counter)) {
            return None;
        }
        // A body that only copies one element to another: without its checks, the C
        // compiler makes the loop a call to `memmove`, which loses to the loop on the
        // short copies such loops usually make, as fannkuch's rotation does.
        if let Some(E::Assign {
            op: AssignOp::Assign,
            target,
            value,
            ..
        }) = only_expr(body).map(|e| &e.kind)
            && matches!(strip(target).kind, E::Index { .. })
            && matches!(strip(value).kind, E::Index { .. })
        {
            return None;
        }
        if !self.invariant(shape.bound, &shape, &sc)
            || shape.step.is_some_and(|s| !self.invariant(s, &shape, &sc))
        {
            return None;
        }
        // Locals declared at the top of the body as the counter plus fixed terms, as
        // `i64 i = y * W + x;`, each with its type.
        let mut derived: HashMap<(u32, u32), (Prim, Terms)> = HashMap::new();
        for (name, value) in &sc.top_locals {
            let d = key(name.span);
            if sc.exposed.contains(&Var::Local(d)) {
                continue;
            }
            let Some(Ty::Prim(p)) = self.facts.locals.get(&d).map(|t| self.sb(t)) else {
                continue;
            };
            if !small_int(p) {
                continue;
            }
            if let Some(terms) = self.affine(value, p, &shape, &derived, &sc) {
                derived.insert(d, (p, terms));
            }
        }
        let mut accesses = Vec::new();
        for (ix, base, index) in &sc.indexes {
            if !self.stable_base(base, &sc) {
                continue;
            }
            let Ty::Prim(p) = settle(&self.ty(index)) else {
                continue;
            };
            if !small_int(p) {
                continue;
            }
            if let Some(terms) = self.affine(index, p, &shape, &derived, &sc) {
                accesses.push((*ix, *base, p, terms));
            }
        }
        if accesses.is_empty() {
            return None;
        }
        let check = self.version_check(&shape, &accesses)?;
        Some(Plan {
            check,
            spans: accesses.iter().map(|(ix, ..)| key(ix.span)).collect(),
        })
    }

    /// A loop that declares one integer counter, steps it by `++`, `--`, `+=` or `-=`,
    /// and has a conjunct comparing it with a bound in the direction it moves.
    fn loop_shape<'e>(
        &self,
        init: &'e Option<ForInit>,
        cond: Option<&'e Expr>,
        update: &'e [Expr],
    ) -> Option<Shape<'e>> {
        let Some(ForInit::Decl(d)) = init else {
            return None;
        };
        let [v] = d.decls.as_slice() else {
            return None;
        };
        if v.capture.is_some() || v.init.is_none() {
            return None;
        }
        let counter = key(v.name.span);
        let Ty::Prim(cty) = self.sb(self.facts.locals.get(&counter)?) else {
            return None;
        };
        if !small_int(cty) {
            return None;
        }
        let is_counter = |e: &Expr| self.var_of(e) == Some(Var::Local(counter));
        let [u] = update else {
            return None;
        };
        let (up, step) = match &u.kind {
            E::Postfix {
                op: PostfixOp::Inc,
                expr,
            }
            | E::Unary {
                op: UnaryOp::PreInc,
                expr,
            } if is_counter(expr) => (true, None),
            E::Postfix {
                op: PostfixOp::Dec,
                expr,
            }
            | E::Unary {
                op: UnaryOp::PreDec,
                expr,
            } if is_counter(expr) => (false, None),
            E::Assign {
                op: AssignOp::Add,
                target,
                capture: None,
                value,
            } if is_counter(target) => (true, Some(&**value)),
            E::Assign {
                op: AssignOp::Sub,
                target,
                capture: None,
                value,
            } if is_counter(target) => (false, Some(&**value)),
            _ => return None,
        };
        if let Some(s) = step
            && !matches!(settle(&self.ty(s)), Ty::Prim(p) if small_int(p))
        {
            return None;
        }
        let mut cs = Vec::new();
        conjuncts(cond?, &mut cs);
        for c in cs {
            let E::Binary { op, lhs, rhs } = &c.kind else {
                continue;
            };
            let (op, other) = if is_counter(lhs) {
                (*op, &**rhs)
            } else if is_counter(rhs) {
                let flipped = match op {
                    BinOp::Lt => BinOp::Gt,
                    BinOp::Gt => BinOp::Lt,
                    BinOp::Le => BinOp::Ge,
                    BinOp::Ge => BinOp::Le,
                    _ => continue,
                };
                (flipped, &**lhs)
            } else {
                continue;
            };
            let (dir, inclusive) = match op {
                BinOp::Lt => (true, false),
                BinOp::Le => (true, true),
                BinOp::Gt => (false, false),
                BinOp::Ge => (false, true),
                _ => continue,
            };
            if dir != up || !matches!(settle(&self.ty(other)), Ty::Prim(p) if small_int(p)) {
                continue;
            }
            return Some(Shape {
                counter,
                cty,
                up,
                bound: other,
                inclusive,
                step,
            });
        }
        None
    }

    fn var_of(&self, e: &Expr) -> Option<Var> {
        let e = strip(e);
        let E::Ident(_) = &e.kind else {
            return None;
        };
        match self.facts.names.get(&key(e.span))? {
            NameRes::Local(d) => Some(Var::Local(*d)),
            NameRes::Field(c, i) if self.prog.classes[*c].fields[*i].is_static => {
                Some(Var::Static(*c, *i))
            }
            _ => None,
        }
    }

    /// Whether assigning to `t` copies a plain value, which can't change the length of
    /// any array.
    fn plain_target(&self, t: &Expr) -> bool {
        match settle(&self.ty(t)).unref() {
            Ty::Prim(_) => true,
            Ty::Class(c, _) => self.prog.classes[*c].kind == ClassKind::Enum,
            _ => false,
        }
    }

    fn scan_stmt<'e>(&self, s: &'e Stmt, top: bool, sc: &mut Scan<'e>) {
        match &s.kind {
            S::Block(b) => {
                for s in &b.stmts {
                    self.scan_stmt(s, false, sc);
                }
            }
            S::Local(d) => self.scan_decl(d, top, sc),
            S::If { cond, then, els } => {
                self.scan_expr(cond, true, sc);
                self.scan_stmt(then, false, sc);
                if let Some(e) = els {
                    self.scan_stmt(e, false, sc);
                }
            }
            S::Switch(sw) => self.scan_switch(sw, true, sc),
            S::Return { status, value } => {
                if let Some(ast::StatusWrite::Expr(e)) = status {
                    self.scan_expr(e, true, sc);
                }
                if let Some(v) = value {
                    let read = if sc.deep { !sc.ret_ref } else { !is_ref(&self.ret) };
                    self.scan_expr(v, read, sc);
                }
            }
            S::Destructure {
                targets,
                capture,
                value,
            } => {
                if capture.is_some() && !sc.deep {
                    sc.bail = true;
                }
                for t in targets {
                    match t {
                        Target::Decl(_, Name::Ident(id)) => {
                            sc.declared.insert(key(id.span));
                        }
                        Target::Existing(Name::Ident(id)) => {
                            match self.facts.names.get(&key(id.span)) {
                                Some(NameRes::Local(d)) => {
                                    sc.exposed.insert(Var::Local(*d));
                                }
                                Some(NameRes::Field(c, i)) => {
                                    sc.exposed.insert(Var::Static(*c, *i));
                                }
                                _ if !sc.deep => sc.bail = true,
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                self.scan_expr(value, false, sc);
            }
            S::Labeled { stmt, .. } => self.scan_stmt(stmt, false, sc),
            S::Expr(e) => self.scan_expr(e, true, sc),
            S::If_(b) => {
                for s in active(b) {
                    self.scan_stmt(s, false, sc);
                }
            }
            S::Break(_) | S::Continue(_) | S::Empty => {}
            _ if !sc.deep => sc.bail = true,
            S::While { cond, body } | S::DoWhile { body, cond } => {
                self.scan_expr(cond, true, sc);
                self.scan_stmt(body, false, sc);
            }
            S::For {
                init,
                cond,
                update,
                body,
            } => {
                match init {
                    Some(ForInit::Decl(d)) => self.scan_decl(d, false, sc),
                    Some(ForInit::Exprs(es)) => {
                        for e in es {
                            self.scan_expr(e, true, sc);
                        }
                    }
                    None => {}
                }
                if let Some(c) = cond {
                    self.scan_expr(c, true, sc);
                }
                for u in update {
                    self.scan_expr(u, true, sc);
                }
                self.scan_stmt(body, false, sc);
            }
            S::ForEach { iter, body, .. } => {
                self.scan_expr(iter, true, sc);
                self.scan_stmt(body, false, sc);
            }
            S::Repeat { source, body, .. } => {
                self.scan_expr(source, true, sc);
                self.scan_stmt(body, false, sc);
            }
            S::Defer(x) => self.scan_stmt(x, false, sc),
            S::Yield(e) => self.scan_expr(e, false, sc),
        }
    }

    fn scan_decl<'e>(&self, d: &'e ast::LocalDecl, top: bool, sc: &mut Scan<'e>) {
        for v in &d.decls {
            let dk = key(v.name.span);
            sc.declared.insert(dk);
            if v.capture.is_some() && !sc.deep {
                sc.bail = true;
            }
            let ty = self.facts.locals.get(&dk).map(|t| self.sb(t));
            if matches!(ty, Some(Ty::Array(..))) {
                sc.owned.insert(dk);
            }
            if let Some(init) = &v.init {
                // A reference to a variable could write it later.
                let read = !ty.as_ref().is_some_and(is_ref);
                self.scan_expr(init, read, sc);
                if top
                    && d.decls.len() == 1
                    && v.capture.is_none()
                    && matches!(ty, Some(Ty::Prim(p)) if small_int(p))
                {
                    sc.top_locals.push((&v.name, init));
                }
            }
        }
    }

    fn scan_switch<'e>(&self, sw: &'e ast::Switch, read: bool, sc: &mut Scan<'e>) {
        self.scan_expr(&sw.scrutinee, true, sc);
        for r in &sw.rules {
            match &r.body {
                SwitchBody::Expr(e) => self.scan_expr(e, read, sc),
                SwitchBody::Block(b) => {
                    for s in &b.stmts {
                        self.scan_stmt(s, false, sc);
                    }
                }
            }
        }
    }

    /// A target written: a variable, an element, a field, or `_`.
    fn scan_target<'e>(&self, t: &'e Expr, sc: &mut Scan<'e>) {
        let t = strip(t);
        match &t.kind {
            E::Ident(_) => {
                if let Some(v) = self.var_of(t) {
                    sc.exposed.insert(v);
                }
            }
            E::Index { .. } | E::Member { .. } => self.scan_expr(t, true, sc),
            E::Discard => {}
            _ => self.scan_expr(t, false, sc),
        }
    }

    /// `read`: `e`'s value is only copied out here, so nothing can write a variable
    /// through it.
    fn scan_expr<'e>(&self, e: &'e Expr, read: bool, sc: &mut Scan<'e>) {
        match &e.kind {
            E::Int { .. }
            | E::Float { .. }
            | E::Char(_)
            | E::Bool(_)
            | E::Null
            | E::Discard
            | E::This
            | E::PrimMember { .. } => {}
            E::Str(parts) => {
                for p in parts {
                    if let StrPart::Interp(x) = p {
                        self.scan_expr(x, true, sc);
                    }
                }
            }
            E::Ident(_) => {
                if !read && let Some(v) = self.var_of(e) {
                    sc.exposed.insert(v);
                    sc.aliased.insert(v);
                }
            }
            E::New { args, .. } | E::ThisNew(args) => {
                sc.opaque = true;
                for a in args {
                    self.scan_expr(&a.expr, false, sc);
                }
            }
            E::ArrayLit(xs) | E::Tuple(xs) => {
                for x in xs {
                    self.scan_expr(x, false, sc);
                }
            }
            E::Switch(sw) => self.scan_switch(sw, read, sc),
            E::Paren(x) | E::StatusRead { expr: x, .. } => self.scan_expr(x, read, sc),
            E::Lambda { body, .. } if sc.deep => {
                // What a lambda returns could be a reference.
                let outer = std::mem::replace(&mut sc.ret_ref, true);
                match &**body {
                    ast::LambdaBody::Expr(x) => self.scan_expr(x, false, sc),
                    ast::LambdaBody::Block(b) => {
                        for s in &b.stmts {
                            self.scan_stmt(s, false, sc);
                        }
                    }
                }
                sc.ret_ref = outer;
            }
            E::StatusAssign { value, .. } if sc.deep => self.scan_expr(value, true, sc),
            E::Lambda { .. } | E::StatusAssign { .. } => sc.bail = true,
            E::Assign {
                op,
                target,
                capture,
                value,
            } => {
                if capture.is_some() && !sc.deep {
                    sc.bail = true;
                }
                // Binding a reference to the value; rebinding a view copies the view.
                let rebind = (*op == AssignOp::Rebind || self.facts.binds.contains(&key(e.span)))
                    && is_ref(&self.ty(target));
                self.scan_target(target, sc);
                if !self.plain_target(target) {
                    sc.resizes = true;
                }
                self.scan_expr(value, !rebind, sc);
            }
            E::Cond { cond, then, els } => {
                self.scan_expr(cond, true, sc);
                self.scan_expr(then, read, sc);
                self.scan_expr(els, read, sc);
            }
            E::Binary { op, lhs, rhs } => {
                if matches!(
                    op,
                    BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor
                ) {
                    // The array changes; nothing keeps a reference to its variable. What
                    // goes in could be kept, as an element that is a reference.
                    sc.resizes = true;
                    self.scan_target(lhs, sc);
                    self.scan_expr(rhs, false, sc);
                } else {
                    self.scan_expr(lhs, true, sc);
                    self.scan_expr(rhs, true, sc);
                }
            }
            E::InstanceOf { expr, .. } => self.scan_expr(expr, false, sc),
            E::Unary { op, expr } => match op {
                UnaryOp::PreInc | UnaryOp::PreDec => self.scan_target(expr, sc),
                _ => self.scan_expr(expr, true, sc),
            },
            E::Postfix { op, expr } => match op {
                PostfixOp::Inc | PostfixOp::Dec => self.scan_target(expr, sc),
                _ => {
                    sc.resizes = true;
                    self.scan_target(expr, sc);
                }
            },
            E::Cast { expr, .. } => self.scan_expr(expr, true, sc),
            E::Member { expr, .. } => self.scan_expr(expr, read, sc),
            E::Call { callee, args } => {
                let fact = self.facts.calls.get(&key(e.span));
                let builtin = fact.is_some_and(|f| matches!(f.origin, Origin::Builtin));
                // The built-in functions that only compute from their arguments.
                let pure = builtin
                    && matches!(&strip(callee).kind, E::Ident(n)
                        if matches!(n.as_str(), "min" | "max" | "abs" | "clamp"));
                if !pure {
                    sc.opaque = true;
                }
                let shift = usize::from(fact.is_some_and(|f| f.recv_arg));
                match &strip(callee).kind {
                    E::Ident(_) => {}
                    E::Member { expr: recv, .. } => {
                        let by_value = fact.is_some_and(|f| {
                            f.recv_arg && matches!(f.params.first(), Some(Ty::Prim(_)))
                        });
                        if by_value {
                            self.scan_expr(recv, true, sc);
                        } else if builtin {
                            // A built-in method can change its receiver, as `push` does,
                            // but keeps no reference to the variable.
                            self.scan_target(recv, sc);
                        } else {
                            self.scan_expr(recv, false, sc);
                        }
                    }
                    _ => self.scan_expr(callee, true, sc),
                }
                for (k, a) in args.iter().enumerate() {
                    // A number passed by value is a copy, and so is a view.
                    let by_value = !a.marked
                        && fact.is_some_and(|f| {
                            matches!(
                                f.params.get(k + shift),
                                Some(Ty::Prim(_) | Ty::Slice(_) | Ty::View(_))
                            )
                        });
                    self.scan_expr(&a.expr, pure || by_value, sc);
                }
            }
            E::Index { expr, mode, index } => {
                if *mode == IndexMode::Checked && self.var_of(expr).is_some() {
                    sc.indexes.push((e, expr, index));
                }
                // An element read or written leaves the array's length and binding.
                self.scan_expr(expr, true, sc);
                self.scan_expr(index, true, sc);
            }
            E::Slice { expr, lo, hi } => {
                self.scan_expr(expr, true, sc);
                for b in [lo, hi].into_iter().flatten() {
                    self.scan_expr(&b.expr, true, sc);
                }
            }
        }
    }

    /// Whether `e` has the same value on every iteration, and computing it once more,
    /// before the loop, can't fail or change anything.
    fn invariant(&self, e: &Expr, shape: &Shape, sc: &Scan) -> bool {
        let int = |x: &Expr| matches!(settle(&self.ty(x)), Ty::Prim(p) if small_int(p));
        match &e.kind {
            E::Int { .. } | E::Char(_) | E::PrimMember { .. } => int(e),
            E::Paren(x) => self.invariant(x, shape, sc),
            E::Ident(_) => match self.facts.names.get(&key(e.span)) {
                Some(NameRes::Local(d)) => {
                    *d != shape.counter
                        && !sc.declared.contains(d)
                        && !sc.exposed.contains(&Var::Local(*d))
                        && !self.aliased.contains(&Var::Local(*d))
                        && int(e)
                }
                Some(NameRes::Field(c, i)) => {
                    self.prog.classes[*c].fields[*i].is_static
                        && !sc.opaque
                        && !sc.exposed.contains(&Var::Static(*c, *i))
                        && !self.aliased.contains(&Var::Static(*c, *i))
                        && int(e)
                }
                Some(NameRes::CConst(_)) => int(e),
                None => false,
            },
            // Integer operators that wrap or never fail (DESIGN §9.2).
            E::Binary {
                op:
                    BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::BitXor
                    | BinOp::Shl
                    | BinOp::Shr,
                lhs,
                rhs,
            } => int(e) && self.invariant(lhs, shape, sc) && self.invariant(rhs, shape, sc),
            E::Unary {
                op: UnaryOp::Neg | UnaryOp::Plus | UnaryOp::BitNot,
                expr,
            } => int(e) && self.invariant(expr, shape, sc),
            E::Member { expr, name } if name.name == "len" => self.stable_base(expr, sc),
            // A cast that can't fail: one that wraps, saturates or widens.
            E::Cast {
                ty: CastTarget::Prim(to),
                mode,
                expr,
            } => {
                let widens = matches!(settle(&self.ty(expr)), Ty::Prim(from) if lossless(from, *to));
                small_int(*to) && (*mode != CastMode::Checked || widens) && self.invariant(expr, shape, sc)
            }
            _ => false,
        }
    }

    /// Whether an array variable keeps its length and binding while the loop runs.
    fn stable_base(&self, b: &Expr, sc: &Scan) -> bool {
        let Some(v) = self.var_of(b) else {
            return false;
        };
        if matches!(v, Var::Static(..)) && sc.opaque {
            return false;
        }
        let exposed = sc.exposed.contains(&v)
            || self.aliased.contains(&v)
            || matches!(v, Var::Local(d) if sc.declared.contains(&d));
        let t = self.ty(b);
        let arr = t.unref();
        if let Ty::Array(_, k) = arr
            && k.cap == CapKind::Varint
        {
            return false;
        }
        match layout(arr) {
            // Always exactly `k` elements.
            Some(Layout::Fixed) => true,
            // A view keeps its own length; only rebinding the variable changes it.
            Some(Layout::Slice) => !exposed,
            // The length is the array's own. Only the loop can change an array of a
            // local's own that no reference reaches; a call could change a static, and a
            // call or a write through a reference could change an array a parameter or
            // a reference names.
            Some(Layout::Vec | Layout::Deq | Layout::Capped) => {
                !exposed
                    && match v {
                        Var::Static(..) => true,
                        Var::Local(d) if self.owned_arrays.contains(&d) => true,
                        Var::Local(_) => !sc.opaque && !sc.resizes,
                    }
            }
            _ => false,
        }
    }

    /// An index as the counter plus fixed terms, with every `+` and `-` between the
    /// counter and the whole in type `p`, so its value wraps only where the whole would.
    fn affine<'e>(
        &self,
        e: &'e Expr,
        p: Prim,
        shape: &Shape,
        derived: &HashMap<(u32, u32), (Prim, Terms<'e>)>,
        sc: &Scan,
    ) -> Option<Terms<'e>> {
        match &e.kind {
            E::Paren(x) => self.affine(x, p, shape, derived, sc),
            E::Ident(_) => match self.facts.names.get(&key(e.span))? {
                NameRes::Local(d) if *d == shape.counter => Some(Vec::new()),
                NameRes::Local(d) => match derived.get(d) {
                    Some((dp, terms)) if *dp == p => Some(terms.clone()),
                    _ => None,
                },
                _ => None,
            },
            E::Binary { op, lhs, rhs } if matches!(op, BinOp::Add | BinOp::Sub) => {
                if settle(&self.ty(e)) != Ty::Prim(p) {
                    return None;
                }
                if let Some(mut t) = self.affine(lhs, p, shape, derived, sc) {
                    if !self.invariant(rhs, shape, sc) {
                        return None;
                    }
                    t.push((*op == BinOp::Sub, &**rhs));
                    return Some(t);
                }
                if *op == BinOp::Add {
                    let mut t = self.affine(rhs, p, shape, derived, sc)?;
                    if !self.invariant(lhs, shape, sc) {
                        return None;
                    }
                    t.push((false, &**lhs));
                    return Some(t);
                }
                None
            }
            _ => None,
        }
    }

    /// The length of a stable array variable, in C.
    fn stable_len(&mut self, b: &Expr) -> String {
        let t = self.ty(b);
        let arr = t.unref().clone();
        match layout(&arr) {
            Some(Layout::Fixed) => format!("((int64_t){})", cap_of(&arr)),
            Some(Layout::Capped) => {
                let (p, _) = self.place(b);
                format!("((int64_t)(&({}))->n)", p)
            }
            _ => {
                let v = self.expr(b, &arr);
                format!("(({}).n)", v)
            }
        }
    }

    /// The test before the loop: the counter can't wrap, and every access's first and
    /// last index are in range. Computed in 128 bits, which hold every value involved.
    fn version_check(
        &mut self,
        shape: &Shape,
        accesses: &[(&Expr, &Expr, Prim, Terms)],
    ) -> Option<String> {
        let counter = wide(&self.locals.get(&shape.counter)?.0.clone());
        let bt = settle(&self.ty(shape.bound));
        let bound = self.expr(shape.bound, &bt);
        let bound = wide(&bound);
        let step = match shape.step {
            Some(s) => {
                let st = settle(&self.ty(s));
                let c = self.expr(s, &st);
                wide(&c)
            }
            None => wide_lit(1),
        };
        // The lowest and the highest value the counter has where the body runs.
        let (first, last) = match (shape.up, shape.inclusive) {
            (true, false) => (counter, format!("({} - 1)", bound)),
            (true, true) => (counter, bound),
            (false, false) => (format!("({} + 1)", bound), counter),
            (false, true) => (bound, counter),
        };
        let (cmin, cmax) = prim_range(shape.cty);
        let mut parts = Vec::new();
        if shape.step.is_some() {
            parts.push(format!("{} >= 1", step));
        }
        if shape.up {
            parts.push(format!("{} + {} <= {}", last, step, wide_lit(cmax)));
        } else {
            parts.push(format!("{} - {} >= {}", first, step, wide_lit(cmin)));
        }
        for (_, base, p, terms) in accesses {
            let mut k = String::new();
            for (neg, t) in terms {
                let tt = settle(&self.ty(t));
                let c = self.expr(t, &tt);
                let _ = write!(k, " {} {}", if *neg { '-' } else { '+' }, wide(&c));
            }
            let len = self.stable_len(base);
            parts.push(format!("{}{} >= 0", first, k));
            parts.push(format!("{}{} < {}", last, k, wide(&len)));
            if bits(*p) < 64 {
                parts.push(format!("{}{} <= {}", last, k, wide_lit(prim_range(*p).1)));
            }
        }
        // Accesses at the same offset into arrays of one length test the same things.
        let mut seen = HashSet::new();
        parts.retain(|x| seen.insert(x.clone()));
        Some(parts.join(" && "))
    }
}
