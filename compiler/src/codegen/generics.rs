//! Generics, monomorphized (DESIGN §10). Each generic function, and each method and
//! constructor of a generic class, is generated once per set of type arguments it's
//! used with, on demand, with every type the checker recorded substituted; each generic
//! class gets a struct per instantiation. Instantiations get internal names, and only
//! what is used is generated.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum InstKind {
    Method(ClassId, usize),
    Ctor(ClassId, usize),
}

pub(super) struct Inst {
    kind: InstKind,
    cargs: Vec<Ty>,
    targs: Vec<Ty>,
    name: String,
}

/// How large type arguments may grow before an instantiation counts as runaway, as
/// `f<T>` calling `f<Pair<T, T>>` would, doubling at every step. Counted in nodes, as
/// such types grow exponentially in depth.
const MAX_SIZE: usize = 256;

/// Whether the types have more than `MAX_SIZE` nodes, counting no further than that.
fn too_big<'t>(ts: impl IntoIterator<Item = &'t Ty>) -> bool {
    fn count(t: &Ty, n: &mut usize) {
        *n += 1;
        if *n > MAX_SIZE {
            return;
        }
        match t {
            Ty::Class(_, args) | Ty::Tuple(args) => args.iter().for_each(|a| count(a, n)),
            Ty::Ref(x)
            | Ty::Nullable(x)
            | Ty::Slice(x)
            | Ty::View(x)
            | Ty::Array(x, _)
            | Ty::Set(x)
            | Ty::Pool(x)
            | Ty::Handle(x) => count(x, n),
            Ty::Map(k, v) | Ty::MapEntry(k, v) => {
                count(k, n);
                count(v, n);
            }
            Ty::Func(ps, r) => {
                ps.iter().for_each(|p| count(p, n));
                count(r, n);
            }
            _ => {}
        }
    }
    let mut n = 0;
    for t in ts {
        count(t, &mut n);
        if n > MAX_SIZE {
            return true;
        }
    }
    false
}

impl<'p, 'a> Gen<'p, 'a> {
    /// `t` with the current instantiation's type arguments in place of its parameters.
    pub(super) fn sb(&self, t: &Ty) -> Ty {
        if self.subst.is_empty() {
            t.clone()
        } else {
            t.subst(&self.subst)
        }
    }

    /// A generic class's type parameters bound to `args`.
    pub(super) fn class_map(&self, id: ClassId, args: &[Ty]) -> Vec<(String, Ty)> {
        self.prog.classes[id]
            .type_params
            .iter()
            .zip(args)
            .map(|(p, a)| (p.name.clone(), a.clone()))
            .collect()
    }

    /// The type of field `i` of a class with type arguments `args`.
    pub(super) fn field_ty(&self, id: ClassId, i: usize, args: &[Ty]) -> Ty {
        let f = &self.prog.classes[id].fields[i];
        if args.is_empty() || self.prog.classes[id].type_params.is_empty() {
            self.sb(&f.ty)
        } else {
            f.ty.subst(&self.class_map(id, args))
        }
    }

    /// A class's fields, largest first, then by name (DESIGN §11), as a struct body and
    /// the structs it holds by value.
    pub(super) fn struct_body(&mut self, id: ClassId, args: &[Ty]) -> (String, Vec<String>) {
        let c = &self.prog.classes[id];
        let mut fields: Vec<(usize, String, Ty, Span)> = c
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.is_static)
            .map(|(i, f)| (i, f.name.clone(), Ty::Void, f.span))
            .collect();
        for f in &mut fields {
            f.2 = self.field_ty(id, f.0, args);
        }
        // A variable-size record's struct is its fixed-size part (records.rs).
        if self.vs(&Ty::Class(id, args.to_vec())) {
            fields.retain(|f| !self.vs(&f.2));
        }
        let prog = self.prog;
        // A link, self-relative or pool, is its offset (REFERENCES §7.2, §7.3).
        let link = |i: usize| {
            c.fields[i]
                .link
                .or(c.fields[i].pool_link.as_ref().map(|p| p.0))
        };
        let size = |i: usize, t: &Ty| -> u32 {
            if let Some(w) = link(i) {
                return bits(w) / 8;
            }
            match t {
                Ty::Prim(p) => bits(*p) / 8,
                Ty::Ref(_) | Ty::Nullable(_) => 8,
                Ty::Class(c, _) if prog.classes[*c].kind == ClassKind::Enum => {
                    enum_bytes(prog.classes[*c].constants.len())
                }
                // A packed value's alignment is 1, so it goes after every wider field.
                Ty::Class(c, _) if prog.classes[*c].packed => 1,
                _ => 16,
            }
        };
        fields.sort_by(|a, b| size(b.0, &b.2).cmp(&size(a.0, &a.2)).then(a.1.cmp(&b.1)));
        let mut body = String::new();
        let mut deps = Vec::new();
        for (i, name, ty, span) in &fields {
            // A self-relative link is its offset (REFERENCES §7.2); what it refers to
            // needn't be defined first.
            if let Some(w) = link(*i) {
                let _ = write!(body, "{} f_{}; ", prim_c(w), name);
                continue;
            }
            // An array of them is a capped array of offsets, each from its own slot.
            if let (Some(w), Ty::Array(_, k)) = (c.fields[*i].elem_link, ty) {
                let ct = self.cty(&Ty::Array(Box::new(Ty::Prim(w)), *k), *span);
                let _ = write!(body, "{} f_{}; ", ct, name);
                continue;
            }
            let ct = self.cty(ty, *span);
            deps.extend(self.deps(ty));
            let _ = write!(body, "{} f_{}; ", ct, name);
        }
        if fields.is_empty() {
            body.push_str("char unused; ");
        }
        (body, deps)
    }

    /// The struct of a generic class with these type arguments, defined the first time.
    /// It's named before its fields are laid out, so a type that refers to itself, as
    /// `Node<K>` does through `Node<K>&? next`, finds it.
    pub(super) fn class_inst(&mut self, id: ClassId, args: &[Ty], span: Span) -> String {
        if too_big(args) {
            return self.instance_error(
                span,
                "this call instantiates generic code with ever larger type arguments",
                "each instantiation asks for one with bigger types, so there would be no end to them; DESIGN §10 rejects this",
            );
        }
        if args.iter().any(|a| a.mentions_param()) {
            return self.unsupported(span, "a generic type outside its instantiations");
        }
        let k = format!("{}{:?}", id, args);
        if let Some(n) = self.class_insts.get(&k) {
            return n.clone();
        }
        let name = format!(
            "zc{}g{}_{}",
            id,
            self.class_insts.len(),
            self.prog.classes[id].name
        );
        self.class_insts.insert(k, name.clone());
        let (body, deps) = self.struct_body(id, args);
        if self.vs(&Ty::Class(id, args.to_vec())) {
            self.packed.insert(name.clone());
        }
        self.def_names.insert(name.clone(), name.clone());
        self.defs.push((name.clone(), body, deps));
        name
    }

    /// The C name of an instantiation of a generic method or constructor, queued to be
    /// generated the first time it's asked for.
    pub(super) fn fn_inst(
        &mut self,
        kind: InstKind,
        cargs: Vec<Ty>,
        targs: Vec<Ty>,
        span: Span,
    ) -> String {
        if too_big(cargs.iter().chain(&targs)) {
            return self.instance_error(
                span,
                "this call instantiates generic code with ever larger type arguments",
                "each instantiation asks for one with bigger types, so there would be no end to them; DESIGN §10 rejects this",
            );
        }
        let k = format!("{:?}{:?}{:?}", kind, cargs, targs);
        if let Some(n) = self.fn_insts.get(&k) {
            return n.clone();
        }
        if cargs.iter().chain(&targs).any(|a| a.mentions_param()) {
            return self.unsupported(span, "a generic call outside its instantiations");
        }
        let base = match kind {
            InstKind::Method(c, i) => self.fn_name(c, i),
            InstKind::Ctor(c, i) => self.ctor_name(c, i),
        };
        let name = format!("{}_g{}", base, self.fn_insts.len());
        self.fn_insts.insert(k, name.clone());
        self.inst_queue.push(Inst {
            kind,
            cargs,
            targs,
            name: name.clone(),
        });
        name
    }

    /// Generates queued instantiations, and any they ask for in turn.
    pub(super) fn drain_insts(&mut self) {
        while let Some(inst) = self.inst_queue.pop() {
            let prog = self.prog;
            let (c, method) = match inst.kind {
                InstKind::Method(c, i) => (c, Some(i)),
                InstKind::Ctor(c, _) => (c, None),
            };
            let mut map = self.class_map(c, &inst.cargs);
            if let Some(i) = method {
                let m = &prog.classes[c].methods[i];
                map.extend(
                    m.type_params
                        .iter()
                        .zip(&inst.targs)
                        .map(|(p, a)| (p.name.clone(), a.clone())),
                );
            }
            let saved_subst = std::mem::replace(&mut self.subst, map);
            let saved_cargs = std::mem::replace(&mut self.cur_cargs, inst.cargs.clone());
            self.file = prog.classes[c].file;
            match inst.kind {
                InstKind::Method(c, i) => {
                    self.method_as(c, i, &prog.classes[c].methods[i], inst.name.clone())
                }
                InstKind::Ctor(c, i) => {
                    self.ctor_as(c, i, &prog.classes[c].ctors[i], inst.name.clone())
                }
            }
            self.drain_lambdas();
            self.subst = saved_subst;
            self.cur_cargs = saved_cargs;
        }
    }

    /// The C name for calling method `i` of class `c`: the plain one, or an
    /// instantiation's when the class or the method is generic.
    pub(super) fn method_ref(
        &mut self,
        c: ClassId,
        i: usize,
        recv: Option<&Ty>,
        targs: &[Ty],
        span: Span,
    ) -> (String, Vec<(String, Ty)>) {
        let cls = &self.prog.classes[c];
        let m = &cls.methods[i];
        if cls.type_params.is_empty() && m.type_params.is_empty() {
            self.want_lib(c, i, false);
            return (self.fn_name(c, i), Vec::new());
        }
        let cargs = if cls.type_params.is_empty() {
            Vec::new()
        } else {
            match recv.map(|t| settle(t.unref())) {
                Some(Ty::Class(rc, args)) if rc == c => args,
                Some(_) => Vec::new(),
                // A bare call from another method of the same class.
                None if c == self.class => self.cur_cargs.clone(),
                None => Vec::new(),
            }
        };
        let targs: Vec<Ty> = targs.iter().map(|t| self.sb(t)).collect();
        let mut map = self.class_map(c, &cargs);
        map.extend(
            m.type_params
                .iter()
                .zip(&targs)
                .map(|(p, a)| (p.name.clone(), a.clone())),
        );
        let name = self.fn_inst(InstKind::Method(c, i), cargs, targs, span);
        (name, map)
    }
}
