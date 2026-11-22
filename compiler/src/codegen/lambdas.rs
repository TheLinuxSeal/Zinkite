//! Lambdas and function values (DESIGN §9.3). A lambda passed to a built-in is inlined
//! where the built-in uses it, the compile-time value DESIGN describes, with its
//! parameters bound to the elements in place. A lambda used as a value becomes a C
//! function, reached through a pointer; such a lambda can't capture.

use super::*;
use crate::ast::LambdaBody;

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
        let Some((params, body)) = as_lambda(f) else {
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
                "a lambda that captures, passed to a function of your own",
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
}
