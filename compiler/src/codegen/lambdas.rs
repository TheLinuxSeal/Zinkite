//! Lambdas and function values (DESIGN §9.3). A lambda passed to a built-in is inlined
//! where the built-in uses it, the compile-time value DESIGN describes, with its
//! parameters bound to the elements in place. A lambda that captures, passed to a
//! function of the program's own, makes a copy of that function for it, which inlines
//! the lambda where it calls it and is passed pointers to what the lambda reads. Any
//! other lambda used as a value becomes a C function, reached through a pointer; such a
//! lambda can't capture.

use super::*;
use crate::ast::LambdaBody;
use std::rc::Rc;

/// How many lambdas deep a copy of a function may be made for a lambda that calls
/// another lambda passed to it, as `f(x, (i64 y) => g(y))` does inside a copy for `g`.
/// A function that passes itself a new lambda each time would otherwise never end.
const MAX_BOUND_DEPTH: usize = 8;

/// A lambda that captures, passed for a `func` parameter of a copy of a function made
/// for it (DESIGN §9.3).
pub(super) struct Bound {
    pub lam: Expr,
    /// Where it was written: the class, file and instantiation its body is generated in.
    pub class: ClassId,
    pub file: usize,
    pub subst: Vec<(String, Ty)>,
    pub cargs: Vec<Ty>,
    pub has_this: bool,
    /// The variables it reads from where it was written, as C lvalues in the function
    /// being generated now.
    pub caps: Vec<Cap>,
    /// `this` where it was written, as a C pointer here, and its struct's C type.
    pub this: Option<(String, String)>,
    /// Parameters of where it was written that are bound to lambdas in turn, which it calls.
    pub lams: Vec<((u32, u32), Rc<Bound>)>,
}

#[derive(Clone)]
pub(super) struct Cap {
    pub decl: (u32, u32),
    pub c: String,
    pub ty: Ty,
    pub ctype: String,
}

impl Bound {
    /// What tells copies apart: the lambda, its instantiation, and what it reads.
    fn shape(&self) -> String {
        let caps: Vec<&str> = self.caps.iter().map(|c| c.ctype.as_str()).collect();
        let lams: Vec<String> = self
            .lams
            .iter()
            .map(|(d, b)| format!("{:?}={}", d, b.shape()))
            .collect();
        format!(
            "{:?}{:?}{:?}[{}]{}{{{}}}",
            key(self.lam.span),
            self.subst,
            self.cargs,
            caps.join(","),
            self.this.is_some(),
            lams.join(";")
        )
    }

    fn depth(&self) -> usize {
        1 + self.lams.iter().map(|(_, b)| b.depth()).max().unwrap_or(0)
    }

    /// The C arguments a copy takes for it, in order: each variable's address, then
    /// `this`, then those of the lambdas it calls.
    fn args(&self, out: &mut Vec<String>) {
        out.extend(self.caps.iter().map(|c| format!("&({})", c.c)));
        if let Some((c, _)) = &self.this {
            out.push(c.clone());
        }
        for (_, b) in &self.lams {
            b.args(out);
        }
    }

    /// The same lambda as the copy sees it: each variable through its parameter, in the
    /// order `args` passes them.
    fn in_copy(&self, n: &mut usize, params: &mut Vec<String>) -> Bound {
        let mut next = |ctype: &str, params: &mut Vec<String>| {
            let name = format!("_c{}", *n);
            *n += 1;
            params.push(format!("{} *{}", ctype, name));
            name
        };
        let caps = self
            .caps
            .iter()
            .map(|c| Cap {
                c: format!("(*{})", next(&c.ctype, params)),
                ..c.clone()
            })
            .collect();
        let this = self
            .this
            .as_ref()
            .map(|(_, ct)| (next(ct, params), ct.clone()));
        let lams = self
            .lams
            .iter()
            .map(|(d, b)| (*d, Rc::new(b.in_copy(n, params))))
            .collect();
        Bound {
            lam: self.lam.clone(),
            class: self.class,
            file: self.file,
            subst: self.subst.clone(),
            cargs: self.cargs.clone(),
            has_this: self.has_this,
            caps,
            this,
            lams,
        }
    }
}

/// A copy's parameters bound to lambdas, by position.
pub(super) type BoundParams = Vec<(usize, Rc<Bound>)>;

/// A copy of a function to generate for the lambdas bound to its parameters.
pub(super) struct BoundCopy {
    pub class: ClassId,
    pub method: usize,
    pub name: String,
    pub subst: Vec<(String, Ty)>,
    pub cargs: Vec<Ty>,
    pub bound: BoundParams,
    /// The C parameters that come after the function's own.
    pub params: Vec<String>,
}

/// The local declarations a lambda's body names from outside it, in order, once each.
fn outer_names(facts: &Facts, lam: &Expr) -> Vec<(u32, u32)> {
    fn expr(facts: &Facts, e: &Expr, lam: Span, out: &mut Vec<(u32, u32)>) {
        if let E::Ident(_) = &e.kind
            && let Some(NameRes::Local(d)) = facts.names.get(&key(e.span))
            && !(lam.start <= d.0 && d.1 <= lam.end)
            && !out.contains(d)
        {
            out.push(*d);
        }
        match &e.kind {
            E::Lambda { body, .. } => {
                if let LambdaBody::Block(b) = body.as_ref() {
                    b.stmts.iter().for_each(|s| stmt(facts, s, lam, out));
                }
            }
            E::Switch(sw) => {
                for r in &sw.rules {
                    if let SwitchBody::Block(b) = &r.body {
                        b.stmts.iter().for_each(|s| stmt(facts, s, lam, out));
                    }
                }
            }
            _ => {}
        }
        for x in crate::check::expr_children(e) {
            expr(facts, x, lam, out);
        }
    }
    fn stmt(facts: &Facts, s: &Stmt, lam: Span, out: &mut Vec<(u32, u32)>) {
        let (ss, es) = crate::check::stmt_children(s);
        es.into_iter().for_each(|e| expr(facts, e, lam, out));
        ss.into_iter().for_each(|s| stmt(facts, s, lam, out));
    }
    let mut out = Vec::new();
    expr(facts, lam, lam.span, &mut out);
    out
}

/// Where a status that an inlined lambda returns goes (DESIGN §9.3): to the `@` reading
/// the call it's passed to, or, with none, to a stop at that call. `action` has `%%ST%%`
/// where the status goes.
pub(super) struct LambdaFail {
    pub action: String,
    pub used: bool,
}

pub(super) struct PendingLambda {
    pub name: String,
    pub params: Vec<ast::Param>,
    pub body: LambdaBody,
    pub ptys: Vec<Ty>,
    pub ret: Ty,
    pub class: ClassId,
    pub file: usize,
    /// The instantiation it was made in (DESIGN §10).
    pub subst: Vec<(String, Ty)>,
    pub cargs: Vec<Ty>,
}

/// A lambda's parameters and body, through any parentheses.
pub(super) fn as_lambda(e: &Expr) -> Option<(&[ast::Param], &LambdaBody)> {
    match &e.kind {
        E::Lambda { params, body } => Some((params, body)),
        E::Paren(x) => as_lambda(x),
        _ => None,
    }
}

impl<'p, 'a> Gen<'p, 'a> {
    /// The C type of a function value: a pointer typedef, one per signature.
    pub(super) fn fn_type(&mut self, ps: &[Ty], r: &Ty, span: Span) -> String {
        let rc = self.cty(r, span);
        let pcs: Vec<String> = ps.iter().map(|p| self.cty(p, span)).collect();
        let sig = format!("{}({})", rc, pcs.join(","));
        if let Some(n) = self.fn_types.get(&sig) {
            return n.clone();
        }
        // Every function value can fail (DESIGN §9.3): it returns its value and a status.
        let rc = self.ret_c_of(r, true, span);
        let name = format!("zfn{}", self.fn_types.len());
        let params = if pcs.is_empty() {
            "void".to_string()
        } else {
            pcs.join(", ")
        };
        let _ = writeln!(self.fn_typedefs, "typedef {} (*{})({});", rc, name, params);
        self.fn_types.insert(sig, name.clone());
        name
    }

    /// The parameter types and result of a function-typed expression.
    pub(super) fn fn_sig(&self, f: &Expr) -> (Vec<Ty>, Ty) {
        match self.ty(f).unref() {
            Ty::Func(ps, r) => (ps.clone(), (**r).clone()),
            _ => (Vec::new(), Ty::Unknown),
        }
    }

    /// A call of the function `f` on `args`, C lvalues of its parameter types. A lambda
    /// is inlined, each parameter bound to its argument in place, so nothing is copied;
    /// any other function value is called through its pointer.
    /// A status it returns is the status of the call it's passed to (DESIGN §9.3): the
    /// `@` reading that call gets it, or the program stops there.
    pub(super) fn apply(&mut self, f: &Expr, args: &[String]) -> String {
        let mut saved = self.catching.take();
        let action = match &saved {
            Some(k) => format!("{{ {} = %%ST%%; goto {}; }}", k.cs, k.cl),
            None => {
                let (name, span) = self
                    .call_names
                    .last()
                    .cloned()
                    .unwrap_or(("lambda".to_string(), f.span));
                let loc = self.loc(span);
                format!("zr_unread({}, %%ST%%, {})", loc, c_string(name.as_bytes()))
            }
        };
        self.lambda_fail.push(LambdaFail { action, used: false });
        let code = self.apply_inner(f, args);
        let used = self.lambda_fail.pop().is_some_and(|l| l.used);
        if used && let Some(k) = &mut saved {
            k.used = true;
        }
        self.catching = saved;
        code
    }

    /// When the status `st`, a C name, is nonzero, what the innermost `apply` said to do.
    pub(super) fn lambda_failed(&mut self, st: &str) -> String {
        match self.lambda_fail.last_mut() {
            Some(l) => {
                l.used = true;
                format!("if ({}) {}", st, l.action.replace("%%ST%%", st))
            }
            None => String::new(),
        }
    }

    fn apply_inner(&mut self, f: &Expr, args: &[String]) -> String {
        if let Some(b) = self.bound_of(f) {
            return self.inline_bound(&b, args);
        }
        if as_lambda(f).is_none() {
            // A function value, called through its pointer.
            let ft = settle(&self.ty(f));
            let fv = self.expr(f, &ft);
            let (_, ret) = self.fn_sig(f);
            let t = self.tmp("fr");
            let call = format!("({})({})", fv, args.join(", "));
            if ret == Ty::Void {
                let fail = self.lambda_failed(&t);
                return format!("({{ uint8_t {} = {}; {}; }})", t, call, fail);
            }
            let fail = self.lambda_failed(&format!("{}.s", t));
            return format!("({{ __auto_type {t} = {}; {}; {t}.v; }})", call, fail, t = t);
        }
        self.inline_lambda(f, args, "")
    }

    /// The lambda `f` inlined on `args`, with `after` (C declarations) after its
    /// parameters are bound.
    fn inline_lambda(&mut self, f: &Expr, args: &[String], after: &str) -> String {
        let Some((params, body)) = as_lambda(f) else {
            return self.unsupported(f.span, "this function value");
        };
        let (ptys, ret) = self.fn_sig(f);
        let mut binds = String::new();
        for ((p, t), a) in params.iter().zip(&ptys).zip(args) {
            if let Name::Ident(id) = &p.name {
                let v = self.tmp("lp");
                let _ = write!(binds, "__typeof__({a}) *{v} = &({a}); ", a = a, v = v);
                self.locals
                    .insert(key(id.span), (format!("(*{})", v), t.clone()));
            }
        }
        binds.push_str(after);
        match body {
            LambdaBody::Expr(x) => {
                if ret == Ty::Void {
                    let c = self.expr_stmt_code(x);
                    format!("({{ {} {}; }})", binds, c)
                } else {
                    let v = self.expr(x, &ret);
                    format!("({{ {} {}; }})", binds, v)
                }
            }
            LambdaBody::Block(b) => self.inline_block(&binds, b, &ret),
        }
    }

    /// A lambda's block body inlined as a statement expression: its `return`s set the
    /// result and jump to its end (`return_`).
    fn inline_block(&mut self, binds: &str, b: &ast::Block, ret: &Ty) -> String {
        let label = self.tmp("lend");
        let rv = self.tmp("lr");
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        let saved_defers = std::mem::take(&mut self.defers);
        let saved_loops = std::mem::take(&mut self.loops);
        let saved_switches = std::mem::take(&mut self.switches);
        let saved_scratch = self.used_scratch;
        self.indent = 1;
        self.inline_ret
            .push((rv.clone(), label.clone(), ret.clone()));
        self.block_stmts(&b.stmts);
        self.inline_ret.pop();
        let body = std::mem::replace(&mut self.out, saved_out);
        self.indent = saved_indent;
        self.defers = saved_defers;
        self.loops = saved_loops;
        self.switches = saved_switches;
        self.used_scratch = saved_scratch;
        let (decl, value) = if *ret == Ty::Void {
            (String::new(), String::new())
        } else {
            let rc = self.cty(ret, b.span);
            (format!("{} {} = {{0}}; ", rc, rv), format!("{}; ", rv))
        };
        format!(
            "({{ __label__ {l}; {d}{b}\n{body}{l}: ; {v}}})",
            l = label,
            d = decl,
            b = binds,
            body = body,
            v = value
        )
    }

    /// A lambda used as a value: the name of the C function it becomes, generated after
    /// the current function (`drain_lambdas`).
    pub(super) fn lambda_value(&mut self, e: &Expr) -> String {
        let saved = self.catching.take();
        let code = self.lambda_value_inner(e);
        self.catching = saved;
        code
    }

    fn lambda_value_inner(&mut self, e: &Expr) -> String {
        // One C function per lambda and instantiation of the generic code it's in.
        let k = format!("{:?}{:?}", key(e.span), self.subst);
        if let Some(n) = self.lambda_names.get(&k) {
            return n.clone();
        }
        if self.facts.capturing.contains(&key(e.span)) {
            return self.unsupported(
                e.span,
                "a lambda that captures, passed to a method called through an interface or to a constructor",
            );
        }
        let Some((params, body)) = as_lambda(e) else {
            return self.unsupported(e.span, "this function value");
        };
        let (ptys, ret) = self.fn_sig(e);
        let name = format!("zl{}", self.lambda_names.len());
        self.lambda_names.insert(k, name.clone());
        self.pending_lambdas.push(PendingLambda {
            name: name.clone(),
            params: params.to_vec(),
            body: body.clone(),
            ptys,
            ret,
            class: self.class,
            file: self.file,
            subst: self.subst.clone(),
            cargs: self.cur_cargs.clone(),
        });
        name
    }

    /// Generates the C functions for lambdas used as values, including any they use.
    pub(super) fn drain_lambdas(&mut self) {
        while let Some(l) = self.pending_lambdas.pop() {
            self.file = l.file;
            let saved_subst = std::mem::replace(&mut self.subst, l.subst.clone());
            let saved_cargs = std::mem::replace(&mut self.cur_cargs, l.cargs.clone());
            self.begin_fn(l.class, l.ret.clone(), true, false, false);
            self.label_fn("lambda");
            let span = l.params.first().map_or(Span::default(), |p| p.ty.span);
            let rc = self.ret_c_of(&l.ret, true, span);
            let ps = self.params_c(&l.params, &l.ptys);
            let sig = format!(
                "static {} {}({})",
                rc,
                l.name,
                if ps.is_empty() {
                    "void".to_string()
                } else {
                    ps.join(", ")
                }
            );
            let _ = writeln!(self.protos, "{};", sig);
            match &l.body {
                LambdaBody::Expr(x) => {
                    self.used_scratch = false;
                    if l.ret == Ty::Void {
                        let c = self.expr_stmt_code(x);
                        self.line(&format!("{};", c));
                        self.line("return 0;");
                    } else {
                        let v = self.expr(x, &l.ret);
                        if self.used_scratch {
                            self.line(&format!(
                                "int64_t _sm = zr_sm(); {} _r = {{{}, 0}}; zr_sr(_sm); return _r;",
                                rc, v
                            ));
                        } else {
                            self.line(&format!("return ({}){{{}, 0}};", rc, v));
                        }
                    }
                }
                LambdaBody::Block(b) => {
                    self.block_stmts(&b.stmts);
                    if l.ret == Ty::Void {
                        self.line("return 0;");
                    }
                }
            }
            let out = self.take_body();
            let _ = writeln!(self.bodies, "{} {{\n{}}}\n", sig, out);
            self.subst = saved_subst;
            self.cur_cargs = saved_cargs;
        }
    }

    /// `return` inside an inlined lambda's block: set its result, run its own defers,
    /// and jump to its end. False outside one.
    /// A status it returns, `return #st v;` or `return ## f();`, is the status of the call
    /// the lambda is passed to (`apply`), with an entry in the trail.
    pub(super) fn inline_return(
        &mut self,
        status: Option<&ast::StatusWrite>,
        value: Option<&Expr>,
        span: Span,
    ) -> bool {
        let Some((rv, label, rt)) = self.inline_ret.last().cloned() else {
            return false;
        };
        let pass = matches!(status, Some(ast::StatusWrite::Pass));
        let mut st = "0".to_string();
        if let Some(v) = value {
            if pass {
                let (val, pre, one, _) = self.with_status_in(v, &rt, false);
                self.line(&pre);
                if rt != Ty::Void {
                    self.line(&format!("{} = {};", rv, val));
                }
                st = one;
            } else {
                let c = self.expr(v, &rt);
                self.line(&format!("{} = {};", rv, c));
            }
        }
        if !pass {
            st = self.status_text(status);
        }
        self.run_defers(0);
        if st != "0" {
            let t = self.tmp("ls");
            self.line(&format!("uint8_t {} = {};", t, st));
            let passes_on = self.passes_on(status);
            let label_was = self.fn_label.clone();
            if !label_was.is_empty() {
                self.fn_label = "lambda".to_string();
            }
            self.set_at(&format!("{} != 0", t), &t, passes_on, span);
            self.fn_label = label_was;
            let fail = self.lambda_failed(&t);
            self.line(&format!("{};", fail));
        }
        self.line(&format!("goto {};", label));
        true
    }

    /// The lambda a parameter of this copy of a function is bound to, when `e` names one.
    pub(super) fn bound_of(&self, e: &Expr) -> Option<Rc<Bound>> {
        let e = strip_paren(e);
        let E::Ident(_) = &e.kind else {
            return None;
        };
        match self.facts.names.get(&key(e.span)) {
            Some(NameRes::Local(d)) => self.lam_binds.get(d).cloned(),
            _ => None,
        }
    }

    /// What a `func` argument of a call to a function of the program's own binds: a lambda
    /// that captures, or a parameter bound to one already, passed on.
    pub(super) fn bound_arg(&mut self, e: &Expr) -> Option<Rc<Bound>> {
        if let Some(b) = self.bound_of(e) {
            return Some(b);
        }
        let l = strip_paren(e);
        if as_lambda(l).is_none() || !self.facts.capturing.contains(&key(l.span)) {
            return None;
        }
        let mut caps = Vec::new();
        let mut lams = Vec::new();
        for d in outer_names(self.facts, l) {
            if let Some(b) = self.lam_binds.get(&d) {
                lams.push((d, b.clone()));
            } else if let Some((c, ty)) = self.locals.get(&d).cloned() {
                let ctype = self.cty(&ty, l.span);
                caps.push(Cap {
                    decl: d,
                    c,
                    ty,
                    ctype,
                });
            } else {
                return None;
            }
        }
        let this = self.has_this.then(|| {
            let t = self.this_class();
            ("self".to_string(), self.cty(&t, l.span))
        });
        Some(Rc::new(Bound {
            lam: l.clone(),
            class: self.class,
            file: self.file,
            subst: self.subst.clone(),
            cargs: self.cur_cargs.clone(),
            has_this: self.has_this,
            caps,
            this,
            lams,
        }))
    }

    /// The copy of method `i` of class `c` (called as `fname`, in the instantiation `map`)
    /// for the lambdas `bound` to its parameters, and the C arguments to add for them.
    pub(super) fn bound_copy(
        &mut self,
        c: ClassId,
        i: usize,
        fname: &str,
        map: &[(String, Ty)],
        bound: &[(usize, Rc<Bound>)],
        span: Span,
    ) -> (String, Vec<String>) {
        let mut args = Vec::new();
        for (_, b) in bound {
            b.args(&mut args);
        }
        if bound.iter().any(|(_, b)| b.depth() > MAX_BOUND_DEPTH) {
            let u = self.unsupported(
                span,
                "recursion that wraps a lambda in a new one at every call",
            );
            return (u, args);
        }
        let shapes: Vec<String> = bound
            .iter()
            .map(|(k, b)| format!("{}:{}", k, b.shape()))
            .collect();
        let k = format!("{}|{}", fname, shapes.join("|"));
        if let Some(n) = self.bound_copies.get(&k) {
            return (n.clone(), args);
        }
        let name = format!("{}_l{}", fname, self.bound_copies.len());
        self.bound_copies.insert(k, name.clone());
        let mut n = 0;
        let mut params = Vec::new();
        let bound = bound
            .iter()
            .map(|(k, b)| (*k, Rc::new(b.in_copy(&mut n, &mut params))))
            .collect();
        let cargs = self.prog.classes[c]
            .type_params
            .iter()
            .map(|p| {
                map.iter()
                    .find(|(n, _)| *n == p.name)
                    .map_or(Ty::Unknown, |(_, t)| t.clone())
            })
            .collect();
        self.copy_queue.push(BoundCopy {
            class: c,
            method: i,
            name: name.clone(),
            subst: map.to_vec(),
            cargs,
            bound,
            params,
        });
        (name, args)
    }

    /// Generates the queued copies of functions for lambdas, and any they ask for.
    pub(super) fn drain_copies(&mut self) {
        while let Some(copy) = self.copy_queue.pop() {
            let prog = self.prog;
            let saved_subst = std::mem::replace(&mut self.subst, copy.subst.clone());
            let saved_cargs = std::mem::replace(&mut self.cur_cargs, copy.cargs.clone());
            self.file = prog.classes[copy.class].file;
            let m = &prog.classes[copy.class].methods[copy.method];
            self.copy_for = Some((copy.bound, copy.params));
            self.method_version(copy.class, m, &copy.name, None, false);
            self.copy_for = None;
            self.drain_lambdas();
            self.subst = saved_subst;
            self.cur_cargs = saved_cargs;
        }
    }

    /// A call of the lambda `b` is bound to, on `args`: its value and status, as a call
    /// through a `func` value gives them (DESIGN §9.3).
    pub(super) fn call_bound(&mut self, b: &Rc<Bound>, args: &[String], ret: &Ty, span: Span) -> String {
        let r = self.tmp("br");
        let l = self.tmp("bend");
        let void = *ret == Ty::Void;
        let (decl, action) = if void {
            (
                format!("uint8_t {} = 0;", r),
                format!("{{ {} = %%ST%%; goto {}; }}", r, l),
            )
        } else {
            let rc = self.ret_c_of(ret, true, span);
            (
                format!("{} {} = {{0}};", rc, r),
                format!("{{ {}.s = %%ST%%; goto {}; }}", r, l),
            )
        };
        // The arguments are values, which the inlined lambda binds its parameters to.
        let mut decl = decl;
        let mut vals = Vec::new();
        for a in args {
            let v = self.tmp("ba");
            let _ = write!(decl, " __auto_type {} = {};", v, a);
            vals.push(v);
        }
        let saved = self.catching.take();
        self.lambda_fail.push(LambdaFail { action, used: false });
        let code = self.inline_bound(b, &vals);
        self.lambda_fail.pop();
        self.catching = saved;
        if void {
            format!("({{ __label__ {l}; {d} {c}; {l}: ; {r}; }})", l = l, d = decl, c = code, r = r)
        } else {
            format!("({{ __label__ {l}; {d} {r}.v = {c}; {l}: ; {r}; }})", l = l, d = decl, c = code, r = r)
        }
    }

    /// The lambda `b` is bound to, inlined on `args`: generated where it was written,
    /// reading what it captured through this copy's parameters, and `this` as `self`.
    fn inline_bound(&mut self, b: &Rc<Bound>, args: &[String]) -> String {
        let saved = (
            self.class,
            self.file,
            std::mem::replace(&mut self.subst, b.subst.clone()),
            std::mem::replace(&mut self.cur_cargs, b.cargs.clone()),
            self.has_this,
        );
        self.class = b.class;
        self.file = b.file;
        self.has_this = b.has_this;
        let old_locals: Vec<_> = b
            .caps
            .iter()
            .map(|c| (c.decl, self.locals.insert(c.decl, (c.c.clone(), c.ty.clone()))))
            .collect();
        // Its names mean what they meant where it was written: a variable it read isn't
        // bound to a lambda of the function it's inlined in.
        let mut old_binds: Vec<_> = b
            .caps
            .iter()
            .map(|c| (c.decl, self.lam_binds.remove(&c.decl)))
            .collect();
        old_binds.extend(
            b.lams
                .iter()
                .map(|(d, x)| (*d, self.lam_binds.insert(*d, x.clone()))),
        );
        let shadow = b
            .this
            .as_ref()
            .map_or(String::new(), |(c, ct)| format!("{} *self = {}; ", ct, c));
        let code = self.inline_lambda(&b.lam, args, &shadow);
        for (d, old) in old_binds.into_iter().rev() {
            match old {
                Some(x) => self.lam_binds.insert(d, x),
                None => self.lam_binds.remove(&d),
            };
        }
        for (d, old) in old_locals.into_iter().rev() {
            match old {
                Some(x) => self.locals.insert(d, x),
                None => self.locals.remove(&d),
            };
        }
        self.class = saved.0;
        self.file = saved.1;
        self.subst = saved.2;
        self.cur_cargs = saved.3;
        self.has_this = saved.4;
        code
    }
}
