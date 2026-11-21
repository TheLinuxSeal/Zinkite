//! Interfaces (DESIGN §6.3, §12.4). A reference to an interface is two pointers: to the
//! value, and to the method table of its class for that interface. There is one table
//! per (class, interface) pair, in static data, made the first time the pair is needed;
//! each entry is a thunk that takes the value as `void *` and calls the class's own
//! method directly, so a call through an interface is one load and one indirect call.
//! `instanceof` compares the table pointer.

use super::*;

/// The interface a reference type refers to, and whether it may be null: `Shape&` and
/// `Shape&?`.
pub(super) fn iface_target(prog: &Program, t: &Ty) -> Option<(ClassId, Vec<Ty>, bool)> {
    let (inner, nullable) = match t {
        Ty::Nullable(x) => (x.unref(), true),
        Ty::Ref(x) => (&**x, false),
        _ => return None,
    };
    match inner {
        Ty::Class(id, args) if prog.classes[*id].kind == ClassKind::Interface => {
            Some((*id, args.clone(), nullable))
        }
        _ => None,
    }
}

impl<'p, 'a> Gen<'p, 'a> {
    fn iface_key(id: ClassId, args: &[Ty]) -> String {
        format!("{}{:?}", id, args)
    }

    /// The C struct of a reference to interface `id` with `args`, `{void *p; table *t;}`,
    /// and its method table's struct, defined the first time they're asked for.
    pub(super) fn iface_ref(&mut self, id: ClassId, args: &[Ty], span: Span) -> String {
        let k = Self::iface_key(id, args);
        if let Some((n, _)) = self.iface_refs.get(&k) {
            return n.clone();
        }
        if args.iter().any(|a| a.mentions_param()) {
            return self.unsupported(span, "a generic interface outside its instantiations");
        }
        let prog = self.prog;
        let n = self.iface_refs.len();
        let cname = &prog.classes[id].name;
        let name = format!("zi{}_{}", n, cname);
        let table = format!("zv{}_{}", n, cname);
        // In first, so a method that takes the interface itself finds it.
        self.iface_refs.insert(k, (name.clone(), table.clone()));
        let map = self.class_map(id, args);
        let mut slots = String::new();
        for (j, m) in prog.classes[id].methods.iter().enumerate() {
            if m.is_static {
                continue;
            }
            if !m.type_params.is_empty() {
                self.unsupported(m.decl.name.span, "generic methods in an interface");
                continue;
            }
            let ret = m.ret.subst(&map);
            let rc = self.ret_c(&ret, m.can_fail, span);
            let mut ps = vec!["void *".to_string()];
            // Where an array result goes (DESIGN §3.3), before the value.
            if dst_ret(&settle(&ret)) {
                ps.insert(0, "void *".to_string());
            }
            for p in &m.params {
                ps.push(self.cty(&p.subst(&map), span));
            }
            let _ = write!(slots, "{} (*m{})({}); ", rc, j, ps.join(", "));
        }
        if slots.is_empty() {
            // C has no empty structs; an interface with no methods still has tables.
            slots.push_str("char none; ");
        }
        self.def_names.insert(table.clone(), table.clone());
        self.defs.push((table.clone(), slots, Vec::new()));
        self.def_names.insert(name.clone(), name.clone());
        self.defs.push((
            name.clone(),
            format!("void *p; const {} *t; ", table),
            Vec::new(),
        ));
        name
    }

    /// The method table of class `cls` (with `cargs`) for interface `id` (with `args`): a
    /// thunk per method, each calling the class's method of that name and signature.
    pub(super) fn vtable(
        &mut self,
        cls: ClassId,
        cargs: &[Ty],
        id: ClassId,
        args: &[Ty],
        span: Span,
    ) -> String {
        let k = format!("{}{:?}/{}", cls, cargs, Self::iface_key(id, args));
        if let Some(n) = self.vtables.get(&k) {
            return n.clone();
        }
        self.iface_ref(id, args, span);
        let table = self.iface_refs[&Self::iface_key(id, args)].1.clone();
        let name = format!("zvt{}", self.vtables.len());
        self.vtables.insert(k, name.clone());
        let prog = self.prog;
        let cls_ty = Ty::Class(cls, cargs.to_vec());
        let cc = self.cty(&cls_ty, span);
        let imap = self.class_map(id, args);
        let cmap = self.class_map(cls, cargs);
        let mut entries = Vec::new();
        for (j, im) in prog.classes[id].methods.iter().enumerate() {
            if im.is_static || !im.type_params.is_empty() {
                continue;
            }
            let want: Vec<Ty> = im.params.iter().map(|p| p.subst(&imap)).collect();
            let ret = im.ret.subst(&imap);
            // The checker matched it: the same name, and the same parameter types once the
            // class's own type parameters are bound.
            let pick = prog.classes[cls].methods.iter().position(|m| {
                m.name == im.name
                    && !m.is_static
                    && m.params.len() == want.len()
                    && m.params
                        .iter()
                        .zip(&want)
                        .all(|(p, w)| p.subst(&cmap) == *w)
            });
            let Some(mi) = pick else {
                self.unsupported(span, "a class's method for its interface");
                continue;
            };
            let (fname, _) = self.method_ref(cls, mi, Some(&cls_ty), &[], span);
            let class_fails = prog.classes[cls].methods[mi].can_fail;
            let rc = self.ret_c(&ret, im.can_fail, span);
            let thunk = format!("{}_m{}", name, j);
            let mut ps = vec!["void *self".to_string()];
            let mut call = vec![format!("({}*)self", cc)];
            if dst_ret(&settle(&ret)) {
                ps.insert(0, "void *_dst".to_string());
                call.insert(0, "_dst".to_string());
            }
            for (n, p) in want.iter().enumerate() {
                let pc = self.cty(p, span);
                ps.push(format!("{} a{}", pc, n));
                call.push(format!("a{}", n));
            }
            let call = format!("{}({})", fname, call.join(", "));
            // An interface method fails when any implementation can (program.rs), so a
            // class's method that can't gets a status of 0 here.
            let body = match (im.can_fail, class_fails, ret == Ty::Void) {
                (false, _, true) => format!("{};", call),
                (true, false, true) => format!("{}; return 0;", call),
                (true, false, false) => format!("{} _r = {{{}, 0}}; return _r;", rc, call),
                _ => format!("return {};", call),
            };
            let sig = format!("static {} {}({})", rc, thunk, ps.join(", "));
            let _ = writeln!(self.protos, "{};", sig);
            let _ = writeln!(self.bodies, "{} {{ {} }}\n", sig, body);
            entries.push(format!(".m{} = {}", j, thunk));
        }
        let _ = writeln!(self.protos, "static const {} {};", table, name);
        let init = if entries.is_empty() {
            "0".to_string()
        } else {
            entries.join(", ")
        };
        let _ = writeln!(
            self.bodies,
            "static const {} {} = {{{}}};\n",
            table, name, init
        );
        name
    }

    /// A pointer to a class value as a reference to interface `id`: the pointer and the
    /// class's table. A pointer that may be null gives a null reference.
    pub(super) fn upcast(
        &mut self,
        ptr: &str,
        cls: (ClassId, &[Ty]),
        id: ClassId,
        args: &[Ty],
        nullable: bool,
        span: Span,
    ) -> String {
        let zi = self.iface_ref(id, args, span);
        let vt = self.vtable(cls.0, cls.1, id, args, span);
        if nullable {
            format!(
                "({{ void *_up = (void *)({}); ({}){{_up, _up ? &{} : NULL}}; }})",
                ptr, zi, vt
            )
        } else {
            format!("(({}){{(void *)({}), &{}}})", zi, ptr, vt)
        }
    }

    /// `code`, of type `from`, as the interface reference `to`, or `None` when `to` isn't
    /// one. Class references and class places get their table; interface references and
    /// `null` pass as they are.
    pub(super) fn iface_conv(
        &mut self,
        code: &str,
        from: &Ty,
        to: &Ty,
        span: Span,
    ) -> Option<String> {
        let (id, args, _) = iface_target(self.prog, to)?;
        if iface_target(self.prog, from).is_some() {
            return Some(code.to_string());
        }
        let zi = self.iface_ref(id, &args, span);
        let out = match from {
            Ty::Null => format!("(({}){{0}})", zi),
            Ty::Ref(x) => match &**x {
                Ty::Class(c, cargs) => self.upcast(code, (*c, cargs), id, &args, false, span),
                _ => return None,
            },
            Ty::Nullable(x) => match x.unref() {
                Ty::Class(c, cargs) => {
                    let ptr = if matches!(**x, Ty::Ref(_)) {
                        code.to_string()
                    } else {
                        format!("&({})", code)
                    };
                    self.upcast(&ptr, (*c, cargs), id, &args, true, span)
                }
                _ => return None,
            },
            // A class value in a place, bound to the reference.
            Ty::Class(c, cargs) => {
                let ptr = format!("&({})", code);
                self.upcast(&ptr, (*c, cargs), id, &args, false, span)
            }
            _ => return None,
        };
        Some(out)
    }

    /// A call through an interface reference: the function from its table, given the
    /// value's pointer, and for an array result, where to build it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn iface_call(
        &mut self,
        base: &Expr,
        id: ClassId,
        i: usize,
        args: &[ast::Arg],
        params: &[Ty],
        direct: Option<String>,
        span: Span,
    ) -> CallOut {
        let bt = settle(&self.ty(base));
        let iargs = match iface_target(self.prog, &bt) {
            Some((_, a, _)) => a,
            None => match bt.unref() {
                Ty::Class(_, a) => a.clone(),
                _ => Vec::new(),
            },
        };
        let zi = self.iface_ref(id, &iargs, span);
        let recv = self.expr(base, &bt);
        let r = self.tmp("ir");
        let mut cargs = vec![format!("{}.p", r)];
        for (a, p) in args.iter().zip(params) {
            cargs.push(self.expr(&a.expr, p));
        }
        let m = &self.prog.classes[id].methods[i];
        let map = self.class_map(id, &iargs);
        let ret = settle(&m.ret.subst(&map));
        let fails = m.can_fail;
        let into = dst_ret(&ret);
        if into {
            cargs.insert(0, direct.clone().unwrap_or_else(|| "NULL".into()));
        }
        let mut code = format!(
            "({{ {} {} = {}; {}.t->m{}({}); }})",
            zi,
            r,
            recv,
            r,
            i,
            cargs.join(", ")
        );
        if into {
            code = self.array_result(code, &ret, fails, &direct, span);
        }
        CallOut { code, ret, fails }
    }

    /// `x instanceof C c`: whether the reference's table is `C`'s for that interface,
    /// binding `c` to the value when it is. `c` is declared before the statement, since
    /// the test is inside an expression.
    pub(super) fn instance_of(
        &mut self,
        x: &Expr,
        ty: &ast::Type,
        bind: Option<&ast::Ident>,
    ) -> String {
        let xt = settle(&self.ty(x));
        let Some((id, args, _)) = iface_target(self.prog, &xt) else {
            return self.unsupported(x.span, "`instanceof` on this");
        };
        let target = settle(&self.ty_at(ty.span));
        let Ty::Class(c, cargs) = &target else {
            return self.unsupported(ty.span, "`instanceof` with this type");
        };
        let zi = self.iface_ref(id, &args, x.span);
        let vt = self.vtable(*c, cargs, id, &args, ty.span);
        let v = self.expr(x, &xt);
        let r = self.tmp("io");
        match bind {
            Some(b) => {
                let (name, bt) = self.new_local(b);
                let ct = self.cty(&bt, b.span);
                self.line(&format!("{} {} = NULL;", ct, name));
                format!(
                    "({{ {} {} = {}; bool _is = {}.t == &{}; if (_is) {} = ({}){}.p; _is; }})",
                    zi, r, v, r, vt, name, ct, r
                )
            }
            None => format!("({{ {} {} = {}; {}.t == &{}; }})", zi, r, v, r, vt),
        }
    }

    /// `next()` of an iterator in `it` (a C place of type `t`): a direct call for a class,
    /// through the table for an interface reference. Its C code, what it returns, and
    /// whether it can fail.
    fn next_call(&mut self, it: &str, t: &Ty, span: Span) -> Option<(String, Ty, bool)> {
        let prog = self.prog;
        if let Some((id, args, _)) = iface_target(prog, t) {
            let j = prog.classes[id]
                .methods
                .iter()
                .position(|m| m.name == "next" && m.params.is_empty() && !m.is_static)?;
            let m = &prog.classes[id].methods[j];
            let map = self.class_map(id, &args);
            let ret = settle(&m.ret.subst(&map));
            let dst = if dst_ret(&ret) { "NULL, " } else { "" };
            return Some((
                format!("{}.t->m{}({}{}.p)", it, j, dst, it),
                ret,
                m.can_fail,
            ));
        }
        let Ty::Class(c, cargs) = settle(t.unref()) else {
            return None;
        };
        let j = prog.classes[c]
            .methods
            .iter()
            .position(|m| m.name == "next" && m.params.is_empty() && !m.is_static)?;
        let cls = Ty::Class(c, cargs.clone());
        let (fname, _) = self.method_ref(c, j, Some(&cls), &[], span);
        let m = &prog.classes[c].methods[j];
        let map = self.class_map(c, &cargs);
        let ret = settle(&m.ret.subst(&map));
        // A value lasts until the next call, so an array stays on the callee's stack.
        let dst = if dst_ret(&ret) { "NULL, " } else { "" };
        Some((format!("{}({}&{})", fname, dst, it), ret, m.can_fail))
    }

    /// A for-each over an iterator: a repeat loop on a hidden local, `var it = e;` then
    /// `while (T x : it.next())` (DESIGN §9.4). A class with `iterator()` gives its
    /// iterator first.
    pub(super) fn iter_for_each(
        &mut self,
        label: Option<String>,
        name: &Name,
        iter: &Expr,
        body: &Stmt,
    ) {
        let prog = self.prog;
        let span = iter.span;
        let mut t = settle(&self.ty(iter));
        let v = self.expr(iter, &t);
        let it = self.tmp("it");
        // The iterator itself, or an interface reference to one, as a value of its own.
        let mut place = match &t {
            Ty::Ref(x) if iface_target(prog, &t).is_none() => {
                t = (**x).clone();
                format!("(*({}))", v)
            }
            _ => v,
        };
        let has_next = |t: &Ty| match settle(t.unref()) {
            Ty::Class(c, _) => prog.classes[c]
                .methods
                .iter()
                .any(|m| m.name == "next" && m.params.is_empty() && !m.is_static),
            _ => false,
        };
        if !has_next(&t) {
            // An iterable: its `iterator()` gives the iterator.
            let Ty::Class(c, cargs) = settle(t.unref()) else {
                self.unsupported(span, "this for-each");
                return;
            };
            let Some(j) = prog.classes[c]
                .methods
                .iter()
                .position(|m| m.name == "iterator" && m.params.is_empty() && !m.is_static)
            else {
                self.unsupported(span, "this for-each");
                return;
            };
            let cls = Ty::Class(c, cargs.clone());
            let ct = self.cty(&cls, span);
            let src = self.tmp("src");
            self.line(&format!("{} {} = {};", ct, src, place));
            let (fname, _) = self.method_ref(c, j, Some(&cls), &[], span);
            let r = prog.classes[c].methods[j]
                .ret
                .subst(&self.class_map(c, &cargs));
            place = format!("{}(&{})", fname, src);
            t = settle(&r);
            if let Ty::Ref(x) = &t
                && iface_target(prog, &t).is_none()
            {
                place = format!("(*({}))", place);
                t = (**x).clone();
            }
        }
        let ct = self.cty(&t, span);
        self.line(&format!("{} {} = {};", ct, it, place));
        let Some((call, ret, fails)) = self.next_call(&it, &t, span) else {
            self.unsupported(span, "this for-each");
            return;
        };
        let lp = self.enter_loop(label);
        let (brk, cont) = (lp.brk.clone(), lp.cont.clone());
        self.loops.push(lp);
        self.line("for (;;) {");
        self.indent += 1;
        let r = self.tmp("r");
        self.line(&format!("__typeof__({}) {} = {};", call, r, call));
        let val = if fails {
            self.line(&format!("if ({}.s) goto {};", r, brk));
            format!("{}.v", r)
        } else {
            r.clone()
        };
        if let Name::Ident(n) = name {
            let (vname, vty) = self.new_local(n);
            let vc = self.cty(&vty, n.span);
            let cv = self.conv(&val, &ret, &vty, span);
            self.line(&format!("{} {} = {};", vc, vname, cv));
        }
        self.body(body);
        self.line(&format!("{}: ;", cont));
        self.indent -= 1;
        self.line("}");
        self.loops.pop();
        self.line(&format!("{}: ;", brk));
    }
}
