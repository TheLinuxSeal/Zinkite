//! Lambdas and function values (DESIGN §9.3). A lambda passed to a built-in is inlined
//! where the built-in uses it, the compile-time value DESIGN describes, with its
//! parameters bound to the elements in place. A lambda used as a value becomes a C
//! function, reached through a pointer; such a lambda can't capture.

use super::*;
use crate::ast::LambdaBody;

pub(super) struct PendingLambda {
    pub name: String,
    pub params: Vec<ast::Param>,
    pub body: LambdaBody,
    pub ptys: Vec<Ty>,
    pub ret: Ty,
    pub class: ClassId,
    pub file: usize,
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
    pub(super) fn apply(&mut self, f: &Expr, args: &[String]) -> String {
        let Some((params, body)) = as_lambda(f) else {
            let ft = settle(&self.ty(f));
            let fv = self.expr(f, &ft);
            return format!("({})({})", fv, args.join(", "));
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
        let k = key(e.span);
        if let Some(n) = self.lambda_names.get(&k) {
            return n.clone();
        }
        if self.facts.capturing.contains(&k) {
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
        });
        name
    }

    /// Generates the C functions for lambdas used as values, including any they use.
    pub(super) fn drain_lambdas(&mut self) {
        while let Some(l) = self.pending_lambdas.pop() {
            self.file = l.file;
            self.begin_fn(l.class, l.ret.clone(), false, false, false);
            let span = l.params.first().map_or(Span::default(), |p| p.ty.span);
            let rc = self.cty(&l.ret, span);
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
                    } else {
                        let v = self.expr(x, &l.ret);
                        if self.used_scratch {
                            self.line(&format!(
                                "int64_t _sm = zr_sm(); {} _r = {}; zr_sr(_sm); return _r;",
                                rc, v
                            ));
                        } else {
                            self.line(&format!("return {};", v));
                        }
                    }
                }
                LambdaBody::Block(b) => self.block_stmts(&b.stmts),
            }
            let out = std::mem::take(&mut self.out);
            let _ = writeln!(self.bodies, "{} {{\n{}}}\n", sig, out);
        }
    }

    /// `return` inside an inlined lambda's block: set its result, run its own defers,
    /// and jump to its end. False outside one.
    pub(super) fn inline_return(&mut self, value: Option<&Expr>) -> bool {
        let Some((rv, label, rt)) = self.inline_ret.last().cloned() else {
            return false;
        };
        if let Some(v) = value {
            let c = self.expr(v, &rt);
            self.line(&format!("{} = {};", rv, c));
        }
        self.run_defers(0);
        self.line(&format!("goto {};", label));
        true
    }
}
