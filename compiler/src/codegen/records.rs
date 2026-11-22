//! Variable-size records (DESIGN §3.2, §4.6, §11; REFERENCES §6). A class with a
//! frozen-array field (`u8[.*] sku`, `Item[.] items`) or a variable-size class field is
//! stored inline as one run of bytes:
//! - its fixed-size fields, as a packed C struct (DESIGN §11 allows the unaligned reads);
//! - then each variable-size field in declaration order: an array as its count (a varint
//!   for `T[.*]`, 8 bytes for `T[.]`) and its elements, a record as its bytes.
//!
//! A variable of such a class is a pointer to its record, on a data stack of its own; a
//! record is built where it goes, field by field, nested array literals included (§3.3).
//! An array of variable-size values is `zr_rvec`, which keeps its byte length beside its
//! count, so a record is built in place at its end and a for-each walks the records.

use super::*;

impl<'p, 'a> Gen<'p, 'a> {
    pub(super) fn vs(&self, t: &Ty) -> bool {
        self.prog.variable_size(t)
    }

    /// A variable-size class value: `Order`, not `Order&`.
    pub(super) fn vs_class(&self, t: &Ty) -> bool {
        matches!(t, Ty::Class(..)) && self.vs(t)
    }

    /// An array of variable-size values, kept as `zr_rvec`.
    pub(super) fn vs_elems(&self, t: &Ty) -> bool {
        matches!(t.unref(), Ty::Array(e, _) if self.vs(e))
    }

    /// A class's variable-size fields, in declaration order: (index, name, type).
    pub(super) fn rec_vars(&self, id: ClassId, args: &[Ty]) -> Vec<(usize, String, Ty)> {
        self.prog.classes[id]
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.is_static)
            .map(|(i, f)| (i, f.name.clone(), self.field_ty(id, i, args)))
            .filter(|(_, _, t)| self.vs(t))
            .collect()
    }

    /// The C function giving the size in bytes of a variable-size value of type `t` that
    /// starts at a pointer, generated the first time.
    pub(super) fn size_fn(&mut self, t: &Ty, span: Span) -> String {
        let k = format!("{:?}", t);
        if let Some(n) = self.vs_fns.get(&k) {
            return n.clone();
        }
        let n = format!("zsz{}", self.vs_fns.len());
        self.vs_fns.insert(k, n.clone());
        let _ = writeln!(self.protos, "static int64_t {}(const uint8_t *p);", n);
        let body = match t {
            Ty::Class(id, args) => {
                let s = self.cty(t, span);
                let mut b = format!("const uint8_t *q = p + sizeof({}); ", s);
                for (_, _, ft) in self.rec_vars(*id, args) {
                    let f = self.size_fn(&ft, span);
                    let _ = write!(b, "q += {}(q); ", f);
                }
                b.push_str("return q - p;");
                b
            }
            Ty::Array(el, kind) => {
                let elems = self.elems_size("p + _h", "_c", el, span);
                format!("{} return _h + {};", header_read("p", kind), elems)
            }
            _ => "return 0;".into(),
        };
        let _ = writeln!(
            self.bodies,
            "static int64_t {}(const uint8_t *p) {{ {} }}\n",
            n, body
        );
        n
    }

    /// The size of `c` elements of `el` starting at `e`.
    fn elems_size(&mut self, e: &str, c: &str, el: &Ty, span: Span) -> String {
        if self.vs(el) {
            let f = self.size_fn(el, span);
            format!("zr_walk({}, (uint64_t)({}), {})", e, c, f)
        } else {
            let ec = self.cty(el, span);
            format!("((int64_t)({}) * (int64_t)sizeof({}))", c, ec)
        }
    }

    /// Where variable-size field `k` of the record of class `t` at `p` starts.
    fn var_field_at(&mut self, p: &str, t: &Ty, k: usize, span: Span) -> String {
        let Ty::Class(id, args) = t else {
            return self.unsupported(span, "this record");
        };
        let s = self.cty(t, span);
        let vars = self.rec_vars(*id, args);
        let mut q = format!("((const uint8_t *)({}) + sizeof({}))", p, s);
        for (_, _, ft) in vars.iter().take(k) {
            let f = self.size_fn(ft, span);
            q = format!("({{ const uint8_t *_fq = {}; _fq + {}(_fq); }})", q, f);
        }
        q
    }

    /// Reading variable-size field `name` of the record of class `t` at pointer `p`: an
    /// array as a view of its elements, a record as a place.
    pub(super) fn var_field(
        &mut self,
        p: &str,
        t: &Ty,
        name: &str,
        span: Span,
    ) -> Option<(String, Ty)> {
        let Ty::Class(id, args) = t else { return None };
        let vars = self.rec_vars(*id, args);
        let k = vars.iter().position(|(_, n, _)| n == name)?;
        let ft = vars[k].2.clone();
        let q = self.var_field_at(p, t, k, span);
        let code = match &ft {
            Ty::Array(el, kind) if self.vs(el) => {
                let elems = self.elems_size("_e", "_c", el, span);
                format!(
                    "({{ const uint8_t *_q = {}; {} const uint8_t *_e = _q + _h; (zr_rvec){{(void *)_e, (int64_t)_c, {}}}; }})",
                    q,
                    header_read("_q", kind),
                    elems
                )
            }
            Ty::Array(_, kind) => format!(
                "({{ const uint8_t *_q = {}; {} (zr_vec){{(void *)(_q + _h), (int64_t)_c}}; }})",
                q,
                header_read("_q", kind)
            ),
            _ => {
                let s = self.cty(&ft, span);
                format!("(*({} *){})", s, q)
            }
        };
        Some((code, ft))
    }

    /// Statements writing `e`, of variable-size type `t`, at the `uint8_t *` C variable
    /// `cur`, and moving `cur` past it.
    pub(super) fn build(&mut self, cur: &str, e: &Expr, t: &Ty) -> String {
        let e = strip_paren(e);
        match t {
            Ty::Class(id, cargs) => {
                if let E::New { args, .. } = &e.kind {
                    return self.build_new(cur, *id, cargs, args, e.span);
                }
                // A call builds its record at `cur` (DESIGN §3.3); any other record is
                // copied there.
                if matches!(e.kind, E::Call { .. }) {
                    self.direct_result = Some(cur.to_string());
                }
                let v = self.expr(e, t);
                self.direct_result = None;
                let f = self.size_fn(t, e.span);
                format!(
                    "{{ const uint8_t *_rs = (const uint8_t *)&({}); int64_t _rz = {}(_rs); if (_rs != {c}) memmove({c}, _rs, (size_t)_rz); {c} += _rz; }} ",
                    v,
                    f,
                    c = cur
                )
            }
            Ty::Array(el, kind) => {
                let el = (**el).clone();
                let hdr = |n: &str| header_write(cur, n, kind);
                if let E::ArrayLit(items) = &e.kind {
                    let mut out = hdr(&items.len().to_string());
                    for x in items {
                        if self.vs(&el) {
                            out.push_str(&self.build(cur, x, &el));
                        } else {
                            let ec = self.cty(&el, x.span);
                            let v = self.expr(x, &el);
                            let _ = write!(
                                out,
                                "{{ {} _x = {}; memcpy({c}, &_x, sizeof _x); {c} += sizeof _x; }} ",
                                ec,
                                v,
                                c = cur
                            );
                        }
                    }
                    return out;
                }
                if self.vs(&el) {
                    // Another array of variable-size values: its bytes.
                    let st = settle(&self.ty(e));
                    let v = self.expr(e, &st);
                    return format!(
                        "{{ zr_rvec _v = {}; {} memmove({c}, _v.p, (size_t)_v.bytes); {c} += _v.bytes; }} ",
                        v,
                        hdr("_v.n"),
                        c = cur
                    );
                }
                let ec = self.cty(&el, e.span);
                let v = self.expr(e, &Ty::Slice(Box::new(el.clone())));
                format!(
                    "{{ zr_slice _s = {}; {} memmove({c}, _s.p, (size_t)_s.n * sizeof({ec})); {c} += (size_t)_s.n * sizeof({ec}); }} ",
                    v,
                    hdr("_s.n"),
                    c = cur,
                    ec = ec
                )
            }
            _ => self.unsupported(e.span, "building this value") + ";",
        }
    }

    /// `new C(...)` with the default constructor: the fixed-size fields into the packed
    /// struct at the start, then each variable-size field after it, in declaration order.
    fn build_new(
        &mut self,
        cur: &str,
        id: ClassId,
        cargs: &[Ty],
        args: &[ast::Arg],
        span: Span,
    ) -> String {
        let prog = self.prog;
        let status = self.build_status.take();
        let Some(fact) = self.facts.calls.get(&key(span)) else {
            return self.unsupported(span, "this constructor") + ";";
        };
        let Origin::Ctor(_, ki) = fact.origin else {
            return self.unsupported(span, "this constructor") + ";";
        };
        // A constructor of its own: its function writes the record at `cur`.
        if prog.classes[id].ctors[ki].decl.is_some() {
            let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
            let fname = self.ctor_fn(id, ki, cargs, span);
            let mut cs = vec![cur.to_string(), "&_kl".to_string()];
            for (a, p) in args.iter().zip(&params) {
                cs.push(self.arg_code(&a.expr, p));
            }
            let sink = match status {
                Some(v) => format!("{} = _ks; ", v),
                None => "(void)_ks; ".into(),
            };
            return format!(
                "{{ int64_t _kl; uint8_t _ks = {}({}); {} += _kl; {}}} ",
                fname,
                cs.join(", "),
                cur,
                sink
            );
        }
        // A default constructor can't fail; the status variable starts at 0.
        drop(status);
        let t = Ty::Class(id, cargs.to_vec());
        let s = self.cty(&t, span);
        let b = self.tmp("rb");
        let mut out = format!("{{ uint8_t *{} = {}; {} += sizeof({}); ", b, cur, cur, s);
        let mut next = args.iter();
        for (i, f) in prog.classes[id].fields.iter().enumerate() {
            if f.is_static {
                continue;
            }
            let ft = self.field_ty(id, i, cargs);
            let src: &Expr = match f.init {
                Some(init) => init,
                None if starts_empty(&ft) => {
                    out.push_str(
                        &self.unsupported(span, "a growable field in a variable-size class"),
                    );
                    out.push_str("; ");
                    continue;
                }
                None => match next.next() {
                    Some(a) => &a.expr,
                    None => continue,
                },
            };
            if self.vs(&ft) {
                out.push_str(&self.build(cur, src, &ft));
            } else {
                let v = self.expr(src, &ft);
                let _ = write!(out, "(({} *){})->f_{} = {}; ", s, b, f.name, v);
            }
        }
        out.push_str("} ");
        out
    }

    /// The C name of constructor `i` of class `c`, generated when it's asked for.
    pub(super) fn ctor_fn(&mut self, c: ClassId, i: usize, cargs: &[Ty], span: Span) -> String {
        if self.prog.classes[c].type_params.is_empty() {
            self.want_lib(c, i, true);
            self.ctor_name(c, i)
        } else {
            self.fn_inst(
                generics::InstKind::Ctor(c, i),
                cargs.to_vec(),
                Vec::new(),
                span,
            )
        }
    }

    /// A constructor of a variable-size record, as a C function writing the record at
    /// `_dst` and its length to `*_len`. The checker made it assign the variable-size fields
    /// in order, so each is written at `_rc`, the end of what's written so far.
    pub(super) fn record_ctor(&mut self, id: ClassId, k: &crate::program::CtorInfo, name: String) {
        let Some(d) = k.decl else { return };
        let prog = self.prog;
        let span = prog.classes[id].span;
        let t = Ty::Class(id, self.cur_cargs.clone());
        self.begin_fn(id, Ty::Void, k.can_fail, true, true);
        self.rec_ctor = Some((t.clone(), 0));
        let s = self.cty(&t, span);
        let kparams: Vec<Ty> = k.params.iter().map(|x| self.sb(x)).collect();
        let mut params = vec!["uint8_t *_dst".to_string(), "int64_t *_len".to_string()];
        params.extend(self.params_c(&d.params, &kparams));
        let sig = format!("static uint8_t {}({})", name, params.join(", "));
        let _ = writeln!(self.protos, "{};", sig);
        self.line(&format!(
            "{s} *self = ({s} *)_dst; uint8_t *_rc = _dst + sizeof({s}); (void)self;",
            s = s
        ));
        // Fixed-size fields' initializers first; variable-size ones in their place.
        let cargs = self.cur_cargs.clone();
        for (i, f) in prog.classes[id].fields.iter().enumerate() {
            let ft = self.field_ty(id, i, &cargs);
            if f.is_static || self.vs(&ft) {
                continue;
            }
            if let Some(init) = f.init {
                let v = self.expr(init, &ft);
                self.line(&format!("self->f_{} = {};", f.name, v));
            }
        }
        self.label_fn("new");
        self.block_stmts(&d.body.stmts);
        let end = self.record_ctor_end("0");
        self.line(&end);
        let out = self.take_body();
        let _ = writeln!(self.bodies, "{} {{\n{}}}\n", sig, out);
        self.rec_ctor = None;
    }

    /// The end of a record's constructor with status `st`: the rest of its variable-size
    /// fields from their initializers, or on failure the zero record (DESIGN §7.5).
    pub(super) fn record_ctor_end(&mut self, st: &str) -> String {
        let Some((t, next)) = self.rec_ctor.clone() else {
            return String::new();
        };
        let rest = self.record_rest(&t, next);
        if st == "0" {
            return format!("{{ {} *_len = _rc - _dst; return 0; }}", rest);
        }
        let zero = self.zero_build("_rc", &t);
        format!(
            "{{ uint8_t _st = {}; if (_st) {{ _rc = _dst; {} }} else {{ {} }} *_len = _rc - _dst; return _st; }}",
            st, zero, rest
        )
    }

    /// Writes variable-size fields from `from` up to `to` from their initializers: the
    /// ones a constructor skips are those it doesn't assign.
    fn record_inits(&mut self, t: &Ty, from: usize, to: usize) -> String {
        let Ty::Class(id, args) = t else {
            return String::new();
        };
        let prog = self.prog;
        let vars = self.rec_vars(*id, args);
        let mut out = String::new();
        for (i, _, ft) in vars.iter().take(to).skip(from) {
            if let Some(init) = prog.classes[*id].fields[*i].init {
                out.push_str(&self.build("_rc", init, ft));
            }
        }
        out
    }

    fn record_rest(&mut self, t: &Ty, next: usize) -> String {
        let n = match t {
            Ty::Class(id, args) => self.rec_vars(*id, args).len(),
            _ => 0,
        };
        self.record_inits(t, next, n)
    }

    /// `this.f = value` for variable-size field `f` in its record's constructor: written
    /// at the end of the record so far, after any fields before it that the constructor
    /// leaves to their initializers.
    pub(super) fn record_field_write(&mut self, target: &Expr, value: &Expr) -> Option<String> {
        let (t, next) = self.rec_ctor.clone()?;
        let name = match &target.kind {
            E::Member { expr, name } if matches!(expr.kind, E::This) => name.name.clone(),
            E::Ident(n)
                if matches!(
                    self.facts.names.get(&key(target.span)),
                    Some(NameRes::Field(..))
                ) =>
            {
                n.clone()
            }
            _ => return None,
        };
        let Ty::Class(id, args) = &t else { return None };
        let vars = self.rec_vars(*id, args);
        let k = vars.iter().position(|(_, n, _)| *n == name)?;
        let ft = vars[k].2.clone();
        let before = self.record_inits(&t, next, k);
        let b = self.build("_rc", value, &ft);
        self.rec_ctor = Some((t, k + 1));
        Some(format!("({{ {} {} }})", before, b))
    }

    /// `this.new(...)` in a record's constructor: the other constructor writes the whole
    /// record at `_dst`.
    pub(super) fn record_this_new(&mut self, args: &[ast::Arg], span: Span) -> String {
        let Some((t, _)) = self.rec_ctor.clone() else {
            return self.unsupported(span, "this.new here");
        };
        let Ty::Class(id, cargs) = &t else {
            return self.unsupported(span, "this.new here");
        };
        let (Some(fact), n) = (
            self.facts.calls.get(&key(span)),
            self.rec_vars(*id, cargs).len(),
        ) else {
            return self.unsupported(span, "this.new here");
        };
        let Origin::Ctor(_, ki) = fact.origin else {
            return self.unsupported(span, "this.new here");
        };
        let params: Vec<Ty> = fact.params.iter().map(|x| self.sb(x)).collect();
        let fname = self.ctor_fn(*id, ki, cargs, span);
        let mut cs = vec!["_dst".to_string(), "&_kl".to_string()];
        for (a, p) in args.iter().zip(&params) {
            cs.push(self.arg_code(&a.expr, p));
        }
        self.rec_ctor = Some((t.clone(), n));
        format!(
            "({{ int64_t _kl; uint8_t _ks = {}({}); _rc = _dst + _kl; _ks; }})",
            fname,
            cs.join(", ")
        )
    }

    /// Statements writing the zero value of variable-size type `t` at `cur`: zeroed
    /// fixed-size fields and empty arrays.
    pub(super) fn zero_build(&mut self, cur: &str, t: &Ty) -> String {
        match t {
            Ty::Class(id, args) => {
                let s = self.cty(t, Span::default());
                let mut out = format!(
                    "memset({c}, 0, sizeof({s})); {c} += sizeof({s}); ",
                    c = cur,
                    s = s
                );
                for (_, _, ft) in self.rec_vars(*id, args) {
                    out.push_str(&self.zero_build(cur, &ft));
                }
                out
            }
            Ty::Array(_, kind) => header_write(cur, "0", kind),
            _ => String::new(),
        }
    }

    /// Whether building `e` could run code that grows an array: a call that isn't a
    /// constructor or a method on text or a number. Building one of those into an array
    /// is done aside, then moved to the array's end (REFERENCES §6, V4).
    pub(super) fn calls_out(&self, e: &Expr) -> bool {
        match &e.kind {
            E::Call { callee, .. } => {
                let builtin_on_value = matches!(
                    self.facts.calls.get(&key(e.span)).map(|f| &f.origin),
                    Some(Origin::Builtin)
                ) && match &callee.kind {
                    E::Member { expr, .. } => !matches!(
                        settle(self.ty(expr).unref()),
                        Ty::Array(..) | Ty::Map(..) | Ty::Set(_) | Ty::Pool(_)
                    ),
                    _ => true,
                };
                if !builtin_on_value {
                    return true;
                }
            }
            E::Lambda { .. } | E::Assign { .. } | E::StatusAssign { .. } => return true,
            E::Binary {
                op: BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor,
                ..
            }
            | E::Postfix { .. } => return true,
            _ => {}
        }
        crate::check::expr_children(e)
            .into_iter()
            .any(|x| self.calls_out(x))
    }

    /// A record built where `arr`'s next element goes, or aside first when building it
    /// could grow the array. Gives a pointer to the new element.
    pub(super) fn rvec_push(&mut self, arr: &str, x: &Expr, el: &Ty, span: Span) -> String {
        let cur = self.tmp("cur");
        let b = self.build(&cur, x, el);
        let s = self.elem_cty(el, span);
        if self.calls_out(x) {
            let site = self.site();
            format!(
                "({{ uint8_t *_t = (uint8_t *)zr_stack({}), *{c} = _t; {} zr_rvec *_a = &({}); uint8_t *_at = (uint8_t *)_a->p + _a->bytes; memmove(_at, _t, (size_t)({c} - _t)); _a->bytes += {c} - _t; _a->n++; ({} *)_at; }})",
                site,
                b,
                arr,
                s,
                c = cur
            )
        } else {
            format!(
                "({{ zr_rvec *_a = &({}); uint8_t *_at = (uint8_t *)_a->p + _a->bytes, *{c} = _at; {} _a->bytes = {c} - (uint8_t *)_a->p; _a->n++; ({} *)_at; }})",
                arr,
                b,
                s,
                c = cur
            )
        }
    }

    /// The C type a pointer to an element of an array of variable-size values points to.
    fn elem_cty(&mut self, el: &Ty, span: Span) -> String {
        if matches!(el, Ty::Class(..)) {
            self.cty(el, span)
        } else {
            "uint8_t".into()
        }
    }

    /// Storing `init` into the `zr_rvec` at `place`: a literal built in place, another
    /// array's bytes copied.
    pub(super) fn rvec_store(&mut self, place: &str, init: &Expr, el: &Ty) -> String {
        let cur = self.tmp("cur");
        if let E::ArrayLit(items) = &strip_paren(init).kind {
            let mut out = format!(
                "({{ zr_rvec *_a = &({}); uint8_t *{c} = (uint8_t *)_a->p; _a->n = 0; ",
                place,
                c = cur
            );
            for x in items {
                let b = self.build(&cur, x, el);
                let _ = write!(out, "{} _a->n++; ", b);
            }
            let _ = write!(out, "_a->bytes = {} - (uint8_t *)_a->p; }})", cur);
            return out;
        }
        let st = settle(&self.ty(init));
        let v = self.expr(init, &st);
        format!(
            "({{ zr_rvec _v = {}; zr_rvec *_a = &({}); if (_v.p != _a->p) memmove(_a->p, _v.p, (size_t)_v.bytes); _a->n = _v.n; _a->bytes = _v.bytes; }})",
            v, place
        )
    }

    /// A for-each over an array of variable-size values, walking from one to the next.
    pub(super) fn rvec_for_each(
        &mut self,
        label: Option<String>,
        name: &Name,
        iter: &Expr,
        body: &Stmt,
    ) {
        let it = settle(&self.ty(iter));
        let Ty::Array(el, _) = it.unref().clone() else {
            self.unsupported(iter.span, "this for-each");
            return;
        };
        let v = self.expr(iter, &it.unref().clone());
        let a = self.tmp("ra");
        let p = self.tmp("rp");
        let i = self.tmp("i");
        let f = self.size_fn(&el, iter.span);
        self.line(&format!(
            "zr_rvec {} = {}; uint8_t *{} = (uint8_t *){}.p;",
            a, v, p, a
        ));
        let bind = match name {
            Name::Ident(n) => {
                let (vname, vty) = self.new_local(n);
                if matches!(*el, Ty::Class(..)) {
                    let s = self.cty(&el, iter.span);
                    // A reference to the record; by value, the checker asks for one.
                    if is_ref(&vty) {
                        format!("{} *{} = ({} *){};", s, vname, s, p)
                    } else {
                        self.locals
                            .insert(key(n.span), (format!("(*{})", vname), vty));
                        format!("{} *{} = ({} *){};", s, vname, s, p)
                    }
                } else {
                    // An array element: a view of it.
                    let Ty::Array(_, kind) = &*el else {
                        self.unsupported(iter.span, "this for-each");
                        return;
                    };
                    format!(
                        "zr_vec {} = ({{ const uint8_t *_q = {}; {} (zr_vec){{(void *)(_q + _h), (int64_t)_c}}; }});",
                        vname,
                        p,
                        header_read("_q", kind)
                    )
                }
            }
            Name::Discard(_) => String::new(),
        };
        self.loop_(
            label,
            &format!(
                "for (int64_t {i} = 0; {i} < {a}.n; {i}++, {p} += {f}({p}))",
                i = i,
                a = a,
                p = p,
                f = f
            ),
            body,
            Some(&move |me: &mut Self| me.line(&bind)),
        );
    }

    /// `push` and `clear`, which is all an array of variable-size values has (REFERENCES
    /// §6, V3); its `len` is the `n` every array has.
    pub(super) fn rvec_method(
        &mut self,
        base: &Expr,
        m: &str,
        args: &[ast::Arg],
        ret: Ty,
        span: Span,
    ) -> CallOut {
        let bt = self.ty(base);
        let Ty::Array(el, _) = settle(bt.unref()) else {
            return CallOut {
                code: self.unsupported(span, "this array"),
                ret,
                fails: false,
            };
        };
        if !(is_ref(&bt) || self.is_place(base)) {
            return CallOut {
                code: self.unsupported(span, "a method of a temporary array"),
                ret,
                fails: false,
            };
        }
        let (p, _) = self.place(base);
        match m {
            "push" => {
                let code = self.rvec_push(&p, &args[0].expr, &el, span);
                CallOut {
                    code,
                    ret: Ty::Ref(el),
                    fails: false,
                }
            }
            "clear" => CallOut {
                code: format!("({{ zr_rvec *_a = &({}); _a->n = 0; _a->bytes = 0; }})", p),
                ret: Ty::Void,
                fails: false,
            },
            _ => CallOut {
                code: self.unsupported(
                    span,
                    &format!("`{}` on an array of variable-size values", m),
                ),
                ret,
                fails: false,
            },
        }
    }

    /// A record of class `t` as a temporary, built on a data stack of its own: a pointer
    /// to it, for a parameter.
    pub(super) fn vs_temp(&mut self, e: &Expr, t: &Ty) -> String {
        let site = self.site();
        let cur = self.tmp("cur");
        let b = self.build(&cur, e, t);
        let s = self.cty(t, e.span);
        format!(
            "({{ uint8_t *_t = (uint8_t *)zr_stack({}), *{} = _t; {} ({} *)_t; }})",
            site, cur, b, s
        )
    }

    /// An argument for a parameter of type `p`: a variable-size record goes by pointer,
    /// since a plain parameter is a read-only copy (DESIGN §5.1).
    pub(super) fn arg_code(&mut self, e: &Expr, p: &Ty) -> String {
        if self.vs_class(p) {
            if matches!(strip_paren(e).kind, E::New { .. } | E::Call { .. }) {
                return self.vs_temp(e, p);
            }
            let v = self.expr(e, p);
            return format!("(&({}))", v);
        }
        self.expr(e, p)
    }

    /// A result built where the caller says (DESIGN §3.3): an array of the plain layout,
    /// or a variable-size record.
    pub(super) fn returns_into(&self, t: &Ty) -> bool {
        dst_ret(t) || self.vs_class(t)
    }

    /// A function's C return type: a record comes back as a pointer to it.
    pub(super) fn ret_c_of(&mut self, t: &Ty, fails: bool, span: Span) -> String {
        if self.vs_class(t) {
            self.ret_c(&Ty::Ref(Box::new(t.clone())), fails, span)
        } else {
            self.ret_c(t, fails, span)
        }
    }

    /// `return v;` in a function returning a record: it goes in the caller's `_dst`, or
    /// on this function's own stack when there's none. The local the function returns is
    /// there already; anything else is built there, or aside first when that local is in
    /// use, since `v` could be reading it. Gives the pointer to return.
    pub(super) fn record_return(&mut self, v: &Expr, t: &Ty, span: Span) -> String {
        let s = self.cty(t, span);
        let returns_local = matches!(&strip_paren(v).kind, E::Ident(_))
            && matches!(self.facts.names.get(&key(strip_paren(v).span)), Some(NameRes::Local(d)) if Some(*d) == self.result_local);
        if returns_local {
            let x = self.expr(v, t);
            return format!("(&({}))", x);
        }
        let site = self.site();
        let cur = self.tmp("cur");
        let r = self.tmp("rd");
        if self.result_local.is_some() {
            let b = self.build(&cur, v, t);
            self.line(&format!(
                "uint8_t *_t{r} = (uint8_t *)zr_stack({site}), *{c} = _t{r}; {b} uint8_t *{r} = _dst ? (uint8_t *)_dst : _t{r}; if ({r} != _t{r}) memmove({r}, _t{r}, (size_t)({c} - _t{r}));",
                r = r,
                site = site,
                c = cur,
                b = b
            ));
        } else {
            let b = self.build(&cur, v, t);
            self.line(&format!(
                "uint8_t *{r} = _dst ? (uint8_t *)_dst : (uint8_t *)zr_stack({site}), *{c} = {r}; {b}",
                r = r,
                site = site,
                c = cur,
                b = b
            ));
        }
        format!("(({} *){})", s, r)
    }

    /// A parameter's C type: a variable-size record's is a pointer.
    pub(super) fn param_cty(&mut self, t: &Ty, span: Span) -> String {
        if self.vs_class(t) {
            format!("{} *", self.cty(t, span))
        } else {
            self.cty(t, span)
        }
    }

    /// `x = value` for a variable-size record: built aside, since `value` may read `x`,
    /// then moved into `x`'s storage (REFERENCES §6, V2).
    pub(super) fn vs_assign(&mut self, place: &str, value: &Expr, t: &Ty) -> String {
        let site = self.site();
        let cur = self.tmp("cur");
        let b = self.build(&cur, value, t);
        format!(
            "({{ uint8_t *_t = (uint8_t *)zr_stack({}), *{c} = _t; {} memmove(&({}), _t, (size_t)({c} - _t)); }})",
            site,
            b,
            place,
            c = cur
        )
    }
}

/// Reads an array field's count into `_c` and its header's length into `_h`.
fn header_read(q: &str, kind: &ArrayKind) -> String {
    if kind.cap == CapKind::Varint {
        format!("uint64_t _c; int _h = zr_uv_get({}, &_c);", q)
    } else {
        format!("int64_t _c; memcpy(&_c, {}, 8); int _h = 8;", q)
    }
}

/// Writes an array field's count `n` at `cur`, moving past it.
fn header_write(cur: &str, n: &str, kind: &ArrayKind) -> String {
    if kind.cap == CapKind::Varint {
        format!("{c} += zr_uv_put({c}, (uint64_t)({n})); ", c = cur, n = n)
    } else {
        format!(
            "{{ int64_t _n8 = {n}; memcpy({c}, &_n8, 8); {c} += 8; }} ",
            c = cur,
            n = n
        )
    }
}
