//! Copies of a function for the array layouts passed to it (DESIGN §4.3). A parameter
//! typed by its capabilities, as `T[..>]&` is, takes any array that has them, and a plain
//! `T[..>]` any array at all, while each layout is its own C type: an array on a data
//! stack is a `zr_vec`, a capped one an inline struct, a cursor array a `zr_cur`. So a
//! call whose array argument has another C type than its parameter goes to a copy of the
//! function in which the parameter has the argument's type: every name of it there is
//! given that type, so what the function does with it is generated for that layout, and a
//! call passing it on gets a copy in turn.
//!
//! A copy refuses, as not supported yet, what it can't follow the parameter through: a
//! reference bound to it, an assignment of it, a lambda that captures it and isn't
//! inlined, and C code, which fills only an array on a data stack. A push the
//! checker took for one that can't fail can, into a capped array: it fails with FULL, and
//! stops the program there unless the status is read.

use super::*;
use crate::ast::{LambdaBody, StrPart};
use crate::check::NameRes;

/// The parameters a copy gives another layout: each by its index, with its type there.
pub(super) type Over = Vec<(usize, Ty)>;

/// A copy of method `method` of class `class`, waiting to be generated.
pub(super) struct LayoutCopy {
    pub class: ClassId,
    pub method: usize,
    pub name: String,
    pub subst: Vec<(String, Ty)>,
    pub cargs: Vec<Ty>,
    /// Each parameter given another layout, by its index, with its type in the copy.
    pub over: Over,
    /// The call it was made for, where what it can't do is reported when the function is
    /// the library's.
    pub origin: (usize, Span),
}

impl<'p, 'a> Gen<'p, 'a> {
    /// The parameters whose array argument has another C type than the parameter's, with
    /// the type the parameter has in the copy: the argument's, by reference when the
    /// parameter is one. Their indexes count `shift` arguments before `args`.
    pub(super) fn layout_needs(
        &mut self,
        args: &[ast::Arg],
        params: &[Ty],
        shift: usize,
    ) -> Over {
        let mut out = Vec::new();
        for (k, (a, p)) in args.iter().zip(params).enumerate() {
            let (pa, by_ref) = match settle(p) {
                Ty::Ref(x) => (*x, true),
                t => (t, false),
            };
            if !matches!(pa, Ty::Array(..)) {
                continue;
            }
            let at = settle(self.ty(&a.expr).unref());
            if !matches!(at, Ty::Array(..)) {
                continue;
            }
            let span = a.expr.span;
            if self.cty(&pa, span) != self.cty(&at, span) {
                out.push((k + shift, if by_ref { Ty::Ref(Box::new(at)) } else { at }));
            }
        }
        out
    }

    /// The name of the copy of method `i` of class `c` (called as `fname`, in the
    /// instantiation `map`) for the layouts `over`, queued the first time it's asked for.
    pub(super) fn layout_copy(
        &mut self,
        c: ClassId,
        i: usize,
        fname: &str,
        map: &[(String, Ty)],
        over: Over,
        span: Span,
    ) -> String {
        let shapes: Vec<String> = over
            .iter()
            .map(|(k, t)| format!("{}:{}", k, self.cty(t, span)))
            .collect();
        let k = format!("{}|{}", fname, shapes.join("|"));
        if let Some(n) = self.layout_copies.get(&k) {
            return n.clone();
        }
        let name = format!("{}_y{}", fname, self.layout_copies.len());
        self.layout_copies.insert(k, name.clone());
        let cargs = self.prog.classes[c]
            .type_params
            .iter()
            .map(|p| {
                map.iter()
                    .find(|(n, _)| *n == p.name)
                    .map_or(Ty::Unknown, |(_, t)| t.clone())
            })
            .collect();
        self.layout_queue.push(LayoutCopy {
            class: c,
            method: i,
            name: name.clone(),
            subst: map.to_vec(),
            cargs,
            over,
            origin: (self.file, span),
        });
        name
    }

    /// Generates the queued copies, and any they ask for.
    pub(super) fn drain_layouts(&mut self) {
        while let Some(copy) = self.layout_queue.pop() {
            let prog = self.prog;
            let saved_subst = std::mem::replace(&mut self.subst, copy.subst.clone());
            let saved_cargs = std::mem::replace(&mut self.cur_cargs, copy.cargs.clone());
            self.file = prog.classes[copy.class].file;
            let m = &prog.classes[copy.class].methods[copy.method];
            self.layout_for = Some((copy.over, copy.origin));
            self.method_version(copy.class, m, &copy.name, None, false);
            self.layout_for = None;
            self.drain_lambdas();
            self.subst = saved_subst;
            self.cur_cargs = saved_cargs;
        }
    }

    /// In a copy, before its body is generated: each name of a parameter given another
    /// layout takes the parameter's type there, and a push into it is checked; a use the
    /// copy can't follow it through is reported.
    pub(super) fn layout_body(&mut self, m: &MethodInfo, over: &[(usize, Ty)], body: &ast::Block) {
        let mut params: HashMap<(u32, u32), Ty> = HashMap::new();
        for (k, t) in over {
            if let Some(ast::Param {
                name: Name::Ident(id),
                ..
            }) = m.decl.params.get(*k)
            {
                params.insert(key(id.span), t.clone());
            }
        }
        let mut scan = Scan {
            params,
            over: Vec::new(),
            pushes: Vec::new(),
            bad: Vec::new(),
        };
        for s in &body.stmts {
            scan.stmt(self.facts, s, false);
        }
        for (span, t) in scan.over {
            self.layout_over.insert(span, t);
        }
        for (span, name) in scan.pushes {
            self.extra_unchecked.insert(span, name);
        }
        let (file, origin) = self.layout_origin.unwrap_or((self.file, Span::default()));
        let (line, _) = self.sources[file].line_col(origin.start as usize);
        for (span, name, t) in scan.bad {
            let what = format!(
                "this use of `{}`, which the call on line {} gives a `{}`",
                name,
                line,
                self.prog.show(t.unref())
            );
            self.layout_unsupported(span, &what, None);
        }
    }

    /// What a copy can't do, reported where the copy was asked for when the function is
    /// the library's, since the program can't change that code.
    pub(super) fn layout_unsupported(
        &mut self,
        span: Span,
        what: &str,
        lib: Option<(String, String)>,
    ) -> String {
        if let Some((file, origin)) = self.layout_origin
            && crate::stdlib::reserved(&self.prog.files[self.file].package)
        {
            let (msg, help) = lib.unwrap_or_else(|| {
                (
                    "this call can't take an array of this layout yet: the library's code uses it in a way a copy for that layout can't follow".to_string(),
                    "pass an array on a data stack, such as a `T[..>]` (DESIGN §4.3)".to_string(),
                )
            });
            if self.reported.insert((origin.start, origin.end)) {
                self.errs
                    .push((file, Diagnostic::error(origin, msg).with_help(&help)));
            }
            return "0".into();
        }
        self.unsupported(span, what)
    }
}

/// The names of the parameters a copy changes, and how they're used.
struct Scan {
    params: HashMap<(u32, u32), Ty>,
    /// Each name of one, and the type it has in the copy.
    over: Vec<((u32, u32), Ty)>,
    /// Pushes into one, which can fail with FULL in the copy: by span, with the name to
    /// report.
    pushes: Vec<((u32, u32), String)>,
    /// Uses the copy can't follow: where, the parameter's name, and its type there.
    bad: Vec<(Span, String, Ty)>,
}

impl Scan {
    /// The parameter an expression names, if it's one the copy changes.
    fn param(&self, facts: &crate::check::Facts, e: &Expr) -> Option<Ty> {
        let E::Ident(_) = &e.kind else { return None };
        match facts.names.get(&key(e.span)) {
            Some(NameRes::Local(d)) => self.params.get(d).cloned(),
            _ => None,
        }
    }

    /// `e`, which may name the parameter when `ok`: where its uses are generated from its
    /// type. `inlined` is false inside a lambda that isn't inlined where it's written.
    fn expr(&mut self, facts: &crate::check::Facts, e: &Expr, ok: bool, inlined: bool) {
        if let Some(t) = self.param(facts, e) {
            // The checker typed a name of a reference parameter as the reference.
            let typed = match facts.types.get(&key(e.span)) {
                Some(Ty::Ref(_)) | None => t.clone(),
                Some(_) => t.unref().clone(),
            };
            self.over.push((key(e.span), typed));
            if !ok || !inlined {
                let name = match &e.kind {
                    E::Ident(n) => n.clone(),
                    _ => String::new(),
                };
                self.bad.push((e.span, name, t));
            }
            return;
        }
        let named = |s: &Self, x: &Expr| s.param(facts, x).is_some();
        match &e.kind {
            E::Member { expr, .. } => self.expr(facts, expr, true, inlined),
            E::Index { expr, index, .. } => {
                self.expr(facts, expr, true, inlined);
                self.expr(facts, index, false, inlined);
            }
            E::Slice { expr, lo, hi } => {
                self.expr(facts, expr, true, inlined);
                for b in [lo, hi].into_iter().flatten() {
                    self.expr(facts, &b.expr, false, inlined);
                }
            }
            E::Call { callee, args } => {
                if let E::Member { expr, name } = &callee.kind
                    && named(self, expr)
                    && matches!(
                        name.name.as_str(),
                        "push" | "pushAll" | "append" | "pushFront" | "insert"
                    )
                {
                    self.pushes.push((key(e.span), name.name.clone()));
                }
                self.expr(facts, callee, false, inlined);
                // A lambda given to a built-in is inlined where it's written; one given to
                // a function of the program's isn't, so it can't see the parameter's type.
                let builtin = matches!(
                    facts.calls.get(&key(e.span)).map(|f| &f.origin),
                    Some(crate::builtins::Origin::Builtin)
                );
                for a in args {
                    let lam = matches!(strip_paren(&a.expr).kind, E::Lambda { .. });
                    self.expr(facts, &a.expr, true, inlined && (builtin || !lam));
                }
            }
            E::Binary { op, lhs, rhs } => {
                let insert = matches!(
                    op,
                    BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor
                );
                if insert && named(self, lhs) {
                    let n = match op {
                        BinOp::InsertEnd => "+>",
                        BinOp::InsertFront => "+<",
                        _ => "+^",
                    };
                    self.pushes.push((key(e.span), n.to_string()));
                }
                let cmp = matches!(op, BinOp::Eq | BinOp::Ne);
                self.expr(facts, lhs, insert || cmp, inlined);
                self.expr(facts, rhs, cmp, inlined);
            }
            E::Postfix { op, expr } => {
                let remove = !matches!(op, PostfixOp::Inc | PostfixOp::Dec);
                self.expr(facts, expr, remove, inlined);
            }
            E::Str(parts) => {
                for p in parts {
                    if let StrPart::Interp(x) = p {
                        self.expr(facts, x, true, inlined);
                    }
                }
            }
            E::Paren(x) => self.expr(facts, x, ok, inlined),
            E::Lambda { body, .. } => match body.as_ref() {
                LambdaBody::Expr(x) => self.expr(facts, x, false, inlined),
                LambdaBody::Block(b) => {
                    for s in &b.stmts {
                        self.stmt(facts, s, !inlined);
                    }
                }
            },
            E::Switch(sw) => {
                self.expr(facts, &sw.scrutinee, false, inlined);
                for r in &sw.rules {
                    match &r.body {
                        ast::SwitchBody::Expr(x) => self.expr(facts, x, false, inlined),
                        ast::SwitchBody::Block(b) => {
                            for s in &b.stmts {
                                self.stmt(facts, s, !inlined);
                            }
                        }
                    }
                }
            }
            _ => {
                for x in crate::check::expr_children(e) {
                    self.expr(facts, x, false, inlined);
                }
            }
        }
    }

    fn stmt(&mut self, facts: &crate::check::Facts, s: &Stmt, in_lambda: bool) {
        let inlined = !in_lambda;
        if let S::ForEach { iter, body, .. } = &s.kind {
            self.expr(facts, iter, true, inlined);
            self.stmt(facts, body, in_lambda);
            return;
        }
        if let S::Expr(x) = &s.kind {
            self.expr(facts, x, false, inlined);
            return;
        }
        let (ss, es) = crate::check::stmt_children(s);
        for e in es {
            self.expr(facts, e, false, inlined);
        }
        for s in ss {
            self.stmt(facts, s, in_lambda);
        }
    }
}
