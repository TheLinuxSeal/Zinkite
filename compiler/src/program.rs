//! Every declaration in a program, collected from its files, and the
//! resolution of written types to semantic ones (DESIGN §8.4–§8.5, §10).

use std::collections::HashMap;

use crate::ast::{self, Member, Modifier, TopItem, TypeDeclKind};
use crate::diag::{Diagnostic, Severity};
use crate::source::Span;
use crate::token::Cond;
use crate::types::*;

/// Library packages the checker models. Names imported from any other `zink.*`
/// package get the type `Unknown` until the standard library exists.
pub const MODELLED_PACKAGES: &[&str] = &["zink.lang", "zink.math"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    Root,
    Interface,
    Enum,
    /// The hidden class holding a script file's top-level fields and methods (DESIGN §8.6).
    Script,
}

pub struct FieldInfo<'a> {
    pub name: String,
    pub ty: Ty,
    pub is_static: bool,
    pub is_private: bool,
    /// Neither: package-private (DESIGN §6.1).
    pub is_public: bool,
    pub init: Option<&'a ast::Expr>,
    pub span: Span,
}

pub struct MethodInfo<'a> {
    pub name: String,
    pub type_params: Vec<TParam>,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub is_static: bool,
    pub is_mut: bool,
    pub is_private: bool,
    pub is_public: bool,
    pub decl: &'a ast::MethodDecl,
    /// The body writes a nonzero status somewhere (`return #...`).
    pub can_fail: bool,
    /// A C function declared with `extern`: its C name (DESIGN §12.6).
    pub c_name: Option<String>,
}

pub struct CtorInfo<'a> {
    pub params: Vec<Ty>,
    /// `None` for a default constructor (DESIGN §6.2).
    pub decl: Option<&'a ast::CtorDecl>,
    pub can_fail: bool,
    /// A default constructor is as visible as the least visible field it sets.
    pub is_private: bool,
    pub is_public: bool,
}

pub struct ClassInfo<'a> {
    pub name: String,
    pub kind: ClassKind,
    pub file: usize,
    pub package: String,
    pub span: Span,
    pub type_params: Vec<TParam>,
    pub implements: Vec<Ty>,
    /// `implements ordered`
    pub ordered: bool,
    pub fields: Vec<FieldInfo<'a>>,
    pub methods: Vec<MethodInfo<'a>>,
    pub ctors: Vec<CtorInfo<'a>>,
    /// An enum's constants.
    pub constants: Vec<String>,
}

pub struct FileInfo<'a> {
    pub ast: &'a ast::File,
    pub package: String,
    pub script: Option<ClassId>,
    /// Has top-level fields or methods (DESIGN §8.6). Its classes are visible only inside it.
    pub is_script: bool,
    /// `zink.*` packages it imports that the checker doesn't model.
    pub unmodelled: Vec<String>,
    /// What its `extern import`s declare, and the libraries its `extern link`s name
    /// (DESIGN §12.6).
    pub c: Option<std::rc::Rc<crate::cheader::CScope>>,
    pub links: Vec<String>,
}

pub struct Program<'a> {
    pub files: Vec<FileInfo<'a>>,
    pub classes: Vec<ClassInfo<'a>>,
    pub diags: Vec<(usize, Diagnostic)>,
}

/// What `#if` conditions mean while checking: Linux on x86-64, no build flags.
pub fn cond_true(c: &Cond) -> bool {
    match c {
        Cond::Name(n, None) => n == "release",
        Cond::Name(n, Some(arg)) => {
            (n == "os" && arg == "linux") || (n == "arch" && arg == "x86_64")
        }
        Cond::Not(c) => !cond_true(c),
        Cond::And(a, b) => cond_true(a) && cond_true(b),
        Cond::Or(a, b) => cond_true(a) || cond_true(b),
    }
}

/// The items of an `#if` block that are active.
pub fn active<T>(b: &ast::CondBlock<T>) -> &[T] {
    if cond_true(&b.cond) {
        &b.then
    } else {
        b.els.as_deref().unwrap_or(&[])
    }
}

fn flatten_top<'a>(items: &'a [TopItem], out: &mut Vec<&'a TopItem>) {
    for i in items {
        match i {
            TopItem::If(b) => flatten_top(active(b), out),
            _ => out.push(i),
        }
    }
}

pub fn flatten_members<'a>(items: &'a [Member], out: &mut Vec<&'a Member>) {
    for i in items {
        match i {
            Member::If(b) => flatten_members(active(b), out),
            _ => out.push(i),
        }
    }
}

fn flatten_iface<'a>(items: &'a [ast::InterfaceMember], out: &mut Vec<&'a ast::MethodDecl>) {
    for i in items {
        match i {
            ast::InterfaceMember::Method(m) => out.push(m),
            ast::InterfaceMember::If(b) => flatten_iface(active(b), out),
        }
    }
}

/// Whether a body writes a nonzero status: `return #...` other than `#0`.
fn writes_status(b: &ast::Block) -> bool {
    fn stmt(s: &ast::Stmt) -> bool {
        use ast::StmtKind::*;
        match &s.kind {
            Return {
                status: Some(st), ..
            } => !matches!(st, ast::StatusWrite::Int(0)),
            Block(b) => writes_status(b),
            If { then, els, .. } => stmt(then) || els.as_ref().is_some_and(|e| stmt(e)),
            While { body, .. }
            | Repeat { body, .. }
            | DoWhile { body, .. }
            | For { body, .. }
            | ForEach { body, .. } => stmt(body),
            Switch(sw) => sw
                .rules
                .iter()
                .any(|r| matches!(&r.body, ast::SwitchBody::Block(b) if writes_status(b))),
            Labeled { stmt: s, .. } => stmt(s),
            If_(b) => active(b).iter().any(stmt),
            _ => false,
        }
    }
    b.stmts.iter().any(stmt)
}

/// Where a type name is looked up from.
pub struct Scope<'s> {
    pub file: usize,
    pub tparams: &'s [TParam],
}

pub enum Lookup {
    Found(ClassId),
    /// From an imported `zink.*` package the checker doesn't model.
    Unmodelled,
    NotFound,
}

impl<'a> Program<'a> {
    /// What `@@@t` gives: `Failure[..~32]`, from `zink.lang` (DESIGN §7.3).
    pub fn trail_ty(&self) -> Ty {
        match self
            .classes
            .iter()
            .position(|c| c.name == "Failure" && c.package == "zink.lang")
        {
            Some(id) => Ty::Array(
                Box::new(Ty::Class(id, Vec::new())),
                ArrayKind {
                    start: '.',
                    cursor: '.',
                    end: '~',
                    cap: CapKind::Capped,
                    size: 32,
                },
            ),
            None => Ty::Unknown,
        }
    }

    pub fn collect(files: &'a [ast::File]) -> Program<'a> {
        let mut prog = Program {
            files: Vec::new(),
            classes: Vec::new(),
            diags: Vec::new(),
        };
        // Pass 1: every type declaration, with its name, kind and type parameters' names.
        let mut decls: Vec<(ClassId, &'a ast::TypeDecl)> = Vec::new();
        let mut c_cache: HashMap<Vec<String>, std::rc::Rc<crate::cheader::CScope>> = HashMap::new();
        for (fi, f) in files.iter().enumerate() {
            let package = f.package.as_ref().map(|p| p.text()).unwrap_or_default();
            let mut unmodelled: Vec<String> = Vec::new();
            let mut first_span = None;
            for imp in &f.imports {
                let pkg = match imp.kind {
                    ast::ImportKind::Wildcard => imp.path.text(),
                    _ => imp.path.parts[..imp.path.parts.len().saturating_sub(1)]
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join("."),
                };
                if pkg.starts_with("zink.")
                    && !MODELLED_PACKAGES.contains(&pkg.as_str())
                    && !crate::stdlib::has_package(&pkg)
                    && !unmodelled.contains(&pkg)
                {
                    first_span.get_or_insert(imp.span);
                    unmodelled.push(pkg);
                }
            }
            if let Some(span) = first_span {
                let names: Vec<String> = unmodelled.iter().map(|p| format!("`{}`", p)).collect();
                let (list, verb) = match names.len() {
                    1 => (names[0].clone(), "isn't"),
                    n => (
                        format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
                        "aren't",
                    ),
                };
                let d = Diagnostic {
                    severity: Severity::Warning,
                    span,
                    message: format!(
                        "{} {} written yet, so the names this file uses from {} aren't type-checked",
                        list,
                        verb,
                        if names.len() == 1 { "it" } else { "them" }
                    ),
                    help: None,
                };
                prog.diags.push((fi, d));
            }
            let mut items = Vec::new();
            flatten_top(&f.items, &mut items);
            let is_script = items
                .iter()
                .any(|i| matches!(i, TopItem::Field(_) | TopItem::Method(_)));
            // C headers: read once for each list of them.
            let headers: Vec<String> = f
                .externs
                .iter()
                .filter(|e| e.kind == ast::ExternKind::Import)
                .map(|e| e.text.clone())
                .collect();
            let links: Vec<String> = f
                .externs
                .iter()
                .filter(|e| e.kind == ast::ExternKind::Link)
                .map(|e| e.text.clone())
                .collect();
            let c = if headers.is_empty() {
                None
            } else if let Some(s) = c_cache.get(&headers) {
                Some(s.clone())
            } else {
                match crate::cheader::read(&headers) {
                    Ok(s) => {
                        let s = std::rc::Rc::new(s);
                        c_cache.insert(headers.clone(), s.clone());
                        Some(s)
                    }
                    Err(msg) => {
                        let span = f
                            .externs
                            .iter()
                            .find(|e| e.kind == ast::ExternKind::Import)
                            .map_or(Span::default(), |e| e.span);
                        prog.diags.push((
                            fi,
                            Diagnostic::error(span, format!("can't read the C headers: {}", msg)),
                        ));
                        None
                    }
                }
            };
            prog.files.push(FileInfo {
                ast: f,
                package: package.clone(),
                script: None,
                is_script,
                unmodelled,
                c,
                links,
            });
            for item in items {
                match item {
                    TopItem::Type(t) => {
                        let (name, kind, span, tps) = match &t.kind {
                            TypeDeclKind::Class(c) => {
                                (&c.name, ClassKind::Class, t.span, &c.type_params[..])
                            }
                            TypeDeclKind::Root(c) => {
                                (&c.name, ClassKind::Root, t.span, &c.type_params[..])
                            }
                            TypeDeclKind::Interface(i) => {
                                (&i.name, ClassKind::Interface, t.span, &i.type_params[..])
                            }
                            TypeDeclKind::Enum(e) => (&e.name, ClassKind::Enum, t.span, &[][..]),
                        };
                        let clash = |c: &ClassInfo| {
                            c.name == name.name
                                && c.package == package
                                && (c.file == fi || (!is_script && !prog.files[c.file].is_script))
                        };
                        if let Some(prev) = prog.classes.iter().find(|c| clash(c)) {
                            let msg = format!(
                                "`{}` is already declared{}",
                                name.name,
                                if prev.file == fi {
                                    " in this file"
                                } else {
                                    " in this package"
                                }
                            );
                            prog.diags.push((fi, Diagnostic::error(name.span, msg)));
                            continue;
                        }
                        if BoundKind::from_name(&name.name).is_some()
                            || ["hashmap", "hashset", "pool"].contains(&name.name.as_str())
                        {
                            prog.diags.push((fi, Diagnostic::error(name.span, format!("`{}` is a built-in name and can't be declared (GRAMMAR §1.3)", name.name))));
                        }
                        let type_params = tps
                            .iter()
                            .map(|tp| TParam {
                                name: tp.name.name.clone(),
                                bounds: Vec::new(),
                            })
                            .collect();
                        prog.classes.push(ClassInfo {
                            name: name.name.clone(),
                            kind,
                            file: fi,
                            package: package.clone(),
                            span,
                            type_params,
                            implements: Vec::new(),
                            ordered: false,
                            fields: Vec::new(),
                            methods: Vec::new(),
                            ctors: Vec::new(),
                            constants: Vec::new(),
                        });
                        decls.push((prog.classes.len() - 1, t));
                    }
                    TopItem::Field(_) | TopItem::Method(_) => {
                        if prog.files[fi].script.is_none() {
                            prog.classes.push(ClassInfo {
                                name: format!("<script {}>", fi),
                                kind: ClassKind::Script,
                                file: fi,
                                package: package.clone(),
                                span: Span::default(),
                                type_params: Vec::new(),
                                implements: Vec::new(),
                                ordered: false,
                                fields: Vec::new(),
                                methods: Vec::new(),
                                ctors: Vec::new(),
                                constants: Vec::new(),
                            });
                            prog.files[fi].script = Some(prog.classes.len() - 1);
                        }
                    }
                    TopItem::If(_) => {}
                }
            }
        }

        // Pass 2: bounds, implements, and every member's signature.
        for (id, t) in decls {
            prog.fill_type_decl(id, t);
        }
        for fi in 0..files.len() {
            let Some(script) = prog.files[fi].script else {
                continue;
            };
            let mut items = Vec::new();
            flatten_top(&files[fi].items, &mut items);
            for item in items {
                match item {
                    TopItem::Field(f) => prog.add_field(script, f, true),
                    TopItem::Method(m) => prog.add_method(script, m, true),
                    _ => {}
                }
            }
        }
        prog.interface_failures();
        prog
    }

    /// An interface method has no body to write a status, so it can fail when any class
    /// that implements it has a method of that name that can (DESIGN §6.3, §7.3): the
    /// whole program is known, so `Shape.area()` warns about no dropped status, and a
    /// repeat loop over `Iterator.next()` ends.
    fn interface_failures(&mut self) {
        let mut fails: Vec<(ClassId, usize)> = Vec::new();
        for c in &self.classes {
            for imp in &c.implements {
                let Ty::Class(i, _) = imp else { continue };
                for (j, im) in self.classes[*i].methods.iter().enumerate() {
                    if c.methods.iter().any(|m| m.name == im.name && m.can_fail) {
                        fails.push((*i, j));
                    }
                }
            }
        }
        for (i, j) in fails {
            self.classes[i].methods[j].can_fail = true;
        }
    }

    fn error(&mut self, file: usize, span: Span, msg: impl Into<String>) {
        self.diags.push((file, Diagnostic::error(span, msg)));
    }

    fn resolve_bounds(
        &mut self,
        id: ClassId,
        tps: &[ast::TypeParam],
        outer: &[TParam],
    ) -> Vec<TParam> {
        let file = self.classes[id].file;
        let mut out: Vec<TParam> = tps
            .iter()
            .map(|tp| TParam {
                name: tp.name.name.clone(),
                bounds: Vec::new(),
            })
            .collect();
        for (i, tp) in tps.iter().enumerate() {
            let mut scope_params = outer.to_vec();
            scope_params.extend(out.iter().cloned());
            for b in &tp.bounds {
                let bound = self.resolve_bound(b, file, &scope_params);
                if let Some(bound) = bound {
                    out[i].bounds.push(bound);
                }
            }
        }
        out
    }

    fn resolve_bound(&mut self, b: &ast::Type, file: usize, tparams: &[TParam]) -> Option<Bound> {
        if let ast::TypeKind::Named { path, args } = &b.kind
            && path.parts.len() == 1
            && args.is_empty()
            && let Some(k) = BoundKind::from_name(&path.parts[0].name)
        {
            return Some(Bound::Builtin(k));
        }
        let ty = self.resolve_interface(b, &Scope { file, tparams });
        match &ty {
            Ty::Class(c, _) if self.classes[*c].kind == ClassKind::Interface => {
                Some(Bound::Interface(ty))
            }
            Ty::Unknown => None,
            _ => {
                self.error(file, b.span, "a bound must be an interface or a built-in bound such as `ordered` (DESIGN §10.1)");
                None
            }
        }
    }

    fn fill_type_decl(&mut self, id: ClassId, t: &'a ast::TypeDecl) {
        let file = self.classes[id].file;
        match &t.kind {
            TypeDeclKind::Class(c) | TypeDeclKind::Root(c) => {
                let tps = self.resolve_bounds(id, &c.type_params, &[]);
                self.classes[id].type_params = tps.clone();
                for imp in &c.implements {
                    if let ast::TypeKind::Named { path, args } = &imp.kind
                        && path.parts.len() == 1
                        && args.is_empty()
                    {
                        match BoundKind::from_name(&path.parts[0].name) {
                            Some(BoundKind::Ordered) => {
                                self.classes[id].ordered = true;
                                continue;
                            }
                            Some(k) => {
                                let msg = format!(
                                    "a class can't implement `{}`; only `ordered` is implemented by declaring `compareTo` (DESIGN §10.1)",
                                    k.name()
                                );
                                self.error(file, imp.span, msg);
                                continue;
                            }
                            None => {}
                        }
                    }
                    let ty = self.resolve_interface(
                        imp,
                        &Scope {
                            file,
                            tparams: &tps,
                        },
                    );
                    match &ty {
                        Ty::Class(i, _) if self.classes[*i].kind == ClassKind::Interface => {
                            self.classes[id].implements.push(ty)
                        }
                        Ty::Unknown => {}
                        _ => self.error(
                            file,
                            imp.span,
                            "a class can only implement interfaces (DESIGN §6.3)",
                        ),
                    }
                }
                let mut members = Vec::new();
                flatten_members(&c.members, &mut members);
                for m in members {
                    match m {
                        Member::Field(f) => self.add_field(id, f, false),
                        Member::Method(m) => self.add_method(id, m, false),
                        Member::Ctor(ctor) => {
                            let params = ctor
                                .params
                                .iter()
                                .map(|p| self.resolve_param(&p.ty, file, &tps))
                                .collect();
                            let can_fail = writes_status(&ctor.body);
                            self.classes[id].ctors.push(CtorInfo {
                                params,
                                decl: Some(ctor),
                                can_fail,
                                is_private: ctor.modifiers.contains(&Modifier::Private),
                                is_public: ctor.modifiers.contains(&Modifier::Public),
                            });
                        }
                        Member::If(_) => {}
                    }
                }
                if self.classes[id].ctors.is_empty() {
                    // The default constructor: every field without an initializer that isn't
                    // an editable array, a hashmap, a hashset or a pool (DESIGN §6.2).
                    let set: Vec<&FieldInfo> = self.classes[id]
                        .fields
                        .iter()
                        .filter(|f| !f.is_static && f.init.is_none() && !starts_empty(&f.ty))
                        .collect();
                    let params = set.iter().map(|f| f.ty.clone()).collect();
                    let is_private = set.iter().any(|f| f.is_private);
                    let is_public = set.iter().all(|f| f.is_public);
                    self.classes[id].ctors.push(CtorInfo {
                        params,
                        decl: None,
                        can_fail: false,
                        is_private,
                        is_public,
                    });
                }
            }
            TypeDeclKind::Interface(i) => {
                let tps = self.resolve_bounds(id, &i.type_params, &[]);
                self.classes[id].type_params = tps;
                let mut methods = Vec::new();
                flatten_iface(&i.members, &mut methods);
                for m in methods {
                    self.add_method(id, m, false);
                }
            }
            TypeDeclKind::Enum(e) => {
                let mut seen = Vec::new();
                for c in &e.constants {
                    if seen.contains(&c.name) {
                        self.error(
                            file,
                            c.span,
                            format!("`{}` is already a constant of this enum", c.name),
                        );
                    }
                    seen.push(c.name.clone());
                }
                self.classes[id].constants = seen;
            }
        }
    }

    /// A parameter's type, which may be a reference or a writable view.
    fn resolve_param(&mut self, t: &ast::Type, file: usize, tparams: &[TParam]) -> Ty {
        self.resolve(t, &Scope { file, tparams })
    }

    fn add_field(&mut self, id: ClassId, f: &'a ast::FieldDecl, script: bool) {
        let file = self.classes[id].file;
        let tps = self.classes[id].type_params.clone();
        let ty = self.resolve(
            &f.ty,
            &Scope {
                file,
                tparams: &tps,
            },
        );
        let is_static = script || f.modifiers.contains(&Modifier::Static);
        for v in &f.vars {
            if self.classes[id]
                .fields
                .iter()
                .any(|x| x.name == v.name.name)
            {
                self.error(
                    file,
                    v.name.span,
                    format!("`{}` is already a field of this class", v.name.name),
                );
                continue;
            }
            let is_private = f.modifiers.contains(&Modifier::Private);
            // A script's top-level fields are reached by bare name, inside its file.
            let is_public = script || f.modifiers.contains(&Modifier::Public);
            self.classes[id].fields.push(FieldInfo {
                name: v.name.name.clone(),
                ty: ty.clone(),
                is_static,
                is_private,
                is_public,
                init: v.init.as_ref(),
                span: v.name.span,
            });
        }
    }

    fn add_method(&mut self, id: ClassId, m: &'a ast::MethodDecl, script: bool) {
        let file = self.classes[id].file;
        let class_tps = self.classes[id].type_params.clone();
        let mtps = self.resolve_bounds(id, &m.type_params, &class_tps);
        let mut all = class_tps.clone();
        all.extend(mtps.iter().cloned());
        let params: Vec<Ty> = m
            .params
            .iter()
            .map(|p| self.resolve_param(&p.ty, file, &all))
            .collect();
        let ret = match &m.ret {
            ast::ReturnType::Void => Ty::Void,
            ast::ReturnType::Type(t) => self.resolve(
                t,
                &Scope {
                    file,
                    tparams: &all,
                },
            ),
            ast::ReturnType::Tuple(ts) => Ty::Tuple(
                ts.iter()
                    .map(|t| {
                        self.resolve(
                            t,
                            &Scope {
                                file,
                                tparams: &all,
                            },
                        )
                    })
                    .collect(),
            ),
        };
        let can_fail = m.body.as_ref().is_some_and(writes_status);
        let is_static = script || m.modifiers.contains(&Modifier::Static);
        let is_mut = m.modifiers.contains(&Modifier::Mut);
        if m.name.name == "delete" {
            self.error(
                file,
                m.name.span,
                "there are no destructors; clean up with `defer` (DESIGN §6.4)",
            );
        }
        let is_private = m.modifiers.contains(&Modifier::Private);
        // An interface's methods are as visible as the interface.
        let is_public = script
            || self.classes[id].kind == ClassKind::Interface
            || m.modifiers.contains(&Modifier::Public);
        let c_name = m.modifiers.iter().find_map(|md| match md {
            Modifier::Extern(n) => Some(n.clone().unwrap_or_else(|| m.name.name.clone())),
            _ => None,
        });
        self.classes[id].methods.push(MethodInfo {
            name: m.name.name.clone(),
            type_params: mtps,
            params,
            ret,
            is_static,
            is_mut,
            is_private,
            is_public,
            decl: m,
            can_fail,
            c_name,
        });
    }

    /// Finds a class by its simple name, as seen from a file.
    pub fn find_class(&self, file: usize, name: &str) -> Lookup {
        let f = &self.files[file];
        let visible = |c: &ClassInfo| c.file == file || !self.files[c.file].is_script;
        if let Some(i) = self.classes.iter().position(|c| {
            c.name == name && c.package == f.package && c.kind != ClassKind::Script && visible(c)
        }) {
            return Lookup::Found(i);
        }
        for imp in &f.ast.imports {
            let parts = &imp.path.parts;
            match &imp.kind {
                ast::ImportKind::Single | ast::ImportKind::Alias(_) => {
                    let local = match &imp.kind {
                        ast::ImportKind::Alias(a) => &a.name,
                        _ => &parts.last().unwrap().name,
                    };
                    if local == name {
                        let pkg = parts[..parts.len() - 1]
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(".");
                        let real = &parts.last().unwrap().name;
                        if let Some(i) = self
                            .classes
                            .iter()
                            .position(|c| &c.name == real && c.package == pkg)
                        {
                            return Lookup::Found(i);
                        }
                        if f.unmodelled.contains(&pkg) {
                            return Lookup::Unmodelled;
                        }
                    }
                }
                ast::ImportKind::Wildcard => {
                    let pkg = imp.path.text();
                    if let Some(i) = self
                        .classes
                        .iter()
                        .position(|c| c.name == name && c.package == pkg)
                    {
                        return Lookup::Found(i);
                    }
                }
            }
        }
        // `zink.lang` is imported into every file, after its own imports.
        if let Some(i) = self
            .classes
            .iter()
            .position(|c| c.name == name && c.package == "zink.lang")
        {
            return Lookup::Found(i);
        }
        if !f.unmodelled.is_empty() {
            return Lookup::Unmodelled;
        }
        Lookup::NotFound
    }

    /// Resolves a type that names an interface, after `implements`.
    fn resolve_interface(&mut self, t: &ast::Type, scope: &Scope) -> Ty {
        let mut diags = Vec::new();
        let ty = match &t.kind {
            ast::TypeKind::Named { .. } => self.resolve_named(t, scope, &mut diags, true),
            _ => self.resolve_quiet(t, scope, &mut diags),
        };
        for d in diags {
            self.diags.push((scope.file, d));
        }
        ty
    }

    /// Resolves a written type.
    pub fn resolve(&mut self, t: &ast::Type, scope: &Scope) -> Ty {
        let mut diags = Vec::new();
        let ty = self.resolve_quiet(t, scope, &mut diags);
        for d in diags {
            self.diags.push((scope.file, d));
        }
        ty
    }

    /// Resolves a written type, putting any errors in `diags`.
    pub fn resolve_quiet(&self, t: &ast::Type, scope: &Scope, diags: &mut Vec<Diagnostic>) -> Ty {
        use ast::TypeKind as K;
        match &t.kind {
            K::Prim(p) => Ty::Prim(*p),
            K::String => Ty::string(),
            K::Func { params, result } => {
                let ps = params
                    .iter()
                    .map(|p| self.resolve_quiet(p, scope, diags))
                    .collect();
                let r = match result.as_ref() {
                    ast::ReturnType::Void => Ty::Void,
                    ast::ReturnType::Type(t) => self.resolve_quiet(t, scope, diags),
                    ast::ReturnType::Tuple(ts) => Ty::Tuple(
                        ts.iter()
                            .map(|t| self.resolve_quiet(t, scope, diags))
                            .collect(),
                    ),
                };
                Ty::Func(ps, Box::new(r))
            }
            K::Array { elem, slots } => {
                let e = self.resolve_quiet(elem, scope, diags);
                let (start, cursor, end) = match slots.chars.as_slice() {
                    [c] => (*c, *c, *c),
                    [a, b, c] => (*a, *b, *c),
                    _ => ('.', '.', '.'),
                };
                let cap = match &slots.cap {
                    None => CapKind::None,
                    Some(ast::Cap::Int(_)) | Some(ast::Cap::Const(_)) => CapKind::Capped,
                    Some(ast::Cap::Varint) => CapKind::Varint,
                    Some(ast::Cap::Chunked) => CapKind::Chunked,
                };
                let size = match &slots.cap {
                    Some(ast::Cap::Int(n)) => *n,
                    _ => 0,
                };
                let kind = ArrayKind {
                    start,
                    cursor,
                    end,
                    cap,
                    size,
                };
                if cap == CapKind::Varint && !kind.is_frozen() {
                    diags.push(Diagnostic::error(t.span, "`*` is allowed only on frozen arrays, whose slots are all `.` (DESIGN §4.6)"));
                }
                if cap == CapKind::Chunked && !kind.has_cursor() {
                    diags.push(Diagnostic::error(
                        t.span,
                        "`$` needs a cursor slot other than `.` (DESIGN §4.1)",
                    ));
                }
                if let Ty::Class(c, _) = &e
                    && self.classes[*c].kind == ClassKind::Interface
                {
                    diags.push(Diagnostic::error(elem.span, "an interface isn't a value type; store references (`Shape&[..>]`) (DESIGN §6.3)"));
                }
                Ty::Array(Box::new(e), kind)
            }
            K::Slice(e) => Ty::Slice(Box::new(self.resolve_quiet(e, scope, diags))),
            K::Ref { inner, .. } => {
                let i = if let K::Named { .. } = &inner.kind {
                    self.resolve_named(inner, scope, diags, true)
                } else {
                    self.resolve_quiet(inner, scope, diags)
                };
                match i {
                    Ty::Slice(e) => Ty::View(e),
                    Ty::Ref(_) => {
                        diags.push(Diagnostic::error(
                            t.span,
                            "a reference can't refer to a reference",
                        ));
                        i
                    }
                    i => Ty::Ref(Box::new(i)),
                }
            }
            K::Nullable(i) => Ty::Nullable(Box::new(self.resolve_quiet(i, scope, diags))),
            K::Named { .. } => self.resolve_named(t, scope, diags, false),
        }
    }

    fn resolve_named(
        &self,
        t: &ast::Type,
        scope: &Scope,
        diags: &mut Vec<Diagnostic>,
        under_ref: bool,
    ) -> Ty {
        let ast::TypeKind::Named { path, args } = &t.kind else {
            unreachable!()
        };
        let args: Vec<Ty> = args
            .iter()
            .map(|a| self.resolve_quiet(a, scope, diags))
            .collect();
        let arity = |n: usize, diags: &mut Vec<Diagnostic>| {
            if args.len() != n {
                diags.push(Diagnostic::error(
                    t.span,
                    format!(
                        "`{}` takes {} type argument{}",
                        path.text(),
                        n,
                        if n == 1 { "" } else { "s" }
                    ),
                ));
                false
            } else {
                true
            }
        };
        if path.parts.len() == 1 {
            let name = path.parts[0].name.as_str();
            if scope.tparams.iter().any(|p| p.name == name) {
                return Ty::Param(name.to_string());
            }
            match name {
                "hashmap" if arity(2, diags) => {
                    return Ty::Map(Box::new(args[0].clone()), Box::new(args[1].clone()));
                }
                "hashset" if arity(1, diags) => return Ty::Set(Box::new(args[0].clone())),
                "pool" if arity(1, diags) => return Ty::Pool(Box::new(args[0].clone())),
                "Handle" if arity(1, diags) => return Ty::Handle(Box::new(args[0].clone())),
                "hashmap" | "hashset" | "pool" | "Handle" => return Ty::Unknown,
                _ => {}
            }
            if let Some(k) = BoundKind::from_name(name) {
                let help = match k {
                    BoundKind::Int => "for a type, write `i64` or `i32`",
                    BoundKind::Float => "for a type, write `f64`",
                    _ => "bounds appear only after `implements` in type parameters",
                };
                diags.push(
                    Diagnostic::error(
                        t.span,
                        format!("`{}` is a generic bound, not a type (DESIGN §10.1)", name),
                    )
                    .with_help(help),
                );
                return Ty::Unknown;
            }
            return match self.find_class(scope.file, name) {
                Lookup::Found(id) => self.class_type(id, args, t.span, under_ref, diags),
                Lookup::Unmodelled => Ty::Unknown,
                // A C struct type from an `extern import`, used through pointers.
                Lookup::NotFound
                    if self.files[scope.file]
                        .c
                        .as_ref()
                        .is_some_and(|c| c.handles.contains(name)) =>
                {
                    Ty::CHandle(name.to_string())
                }
                Lookup::NotFound => {
                    diags.push(Diagnostic::error(
                        t.span,
                        format!("unknown type `{}`", name),
                    ));
                    Ty::Unknown
                }
            };
        }
        let pkg = path.parts[..path.parts.len() - 1]
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let name = &path.parts.last().unwrap().name;
        if let Some(id) = self
            .classes
            .iter()
            .position(|c| &c.name == name && c.package == pkg)
        {
            return self.class_type(id, args, t.span, under_ref, diags);
        }
        if pkg.starts_with("zink.") && !MODELLED_PACKAGES.contains(&pkg.as_str()) {
            return Ty::Unknown;
        }
        diags.push(Diagnostic::error(
            t.span,
            format!("unknown type `{}`", path.text()),
        ));
        Ty::Unknown
    }

    fn class_type(
        &self,
        id: ClassId,
        args: Vec<Ty>,
        span: Span,
        under_ref: bool,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let c = &self.classes[id];
        if c.type_params.len() != args.len() {
            let n = c.type_params.len();
            diags.push(Diagnostic::error(
                span,
                format!(
                    "`{}` takes {} type argument{}, not {}",
                    c.name,
                    n,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            return Ty::Unknown;
        }
        if c.kind == ClassKind::Interface && !under_ref {
            diags.push(
                Diagnostic::error(
                    span,
                    format!("`{}` is an interface, which isn't a value type", c.name),
                )
                .with_help(format!("refer to one with `{}&` (DESIGN §6.3)", c.name)),
            );
        }
        Ty::Class(id, args)
    }

    /// A type's name for messages.
    /// A value whose size isn't fixed by its type (REFERENCES §6): a frozen array with a
    /// runtime length, `T[.]` or `T[.*]`, or a class with a field that is one. Such a value
    /// is stored inline where it lives, as one run of bytes (DESIGN §3.2).
    pub fn variable_size(&self, t: &Ty) -> bool {
        self.variable_size_in(t, 0)
    }

    fn variable_size_in(&self, t: &Ty, depth: u32) -> bool {
        // A class can't hold itself by value; the limit only stops a runaway.
        if depth > 32 {
            return false;
        }
        match t {
            Ty::Array(_, k) => k.is_frozen() && matches!(k.cap, CapKind::None | CapKind::Varint),
            Ty::Class(id, args) => {
                let c = &self.classes[*id];
                if !matches!(c.kind, ClassKind::Class) {
                    return false;
                }
                let map: Vec<(String, Ty)> = c
                    .type_params
                    .iter()
                    .zip(args)
                    .map(|(p, a)| (p.name.clone(), a.clone()))
                    .collect();
                c.fields
                    .iter()
                    .filter(|f| !f.is_static)
                    .any(|f| self.variable_size_in(&f.ty.subst(&map), depth + 1))
            }
            _ => false,
        }
    }

    pub fn show(&self, t: &Ty) -> String {
        match t {
            Ty::Prim(p) => p.name().to_string(),
            Ty::CHandle(n) => n.clone(),
            Ty::Void => "void".into(),
            Ty::Null => "null".into(),
            Ty::IntLit { is_char: true, .. } => "a character literal".into(),
            Ty::IntLit { .. } => "an integer literal".into(),
            Ty::FloatLit => "a float literal".into(),
            Ty::Class(id, args) => {
                let c = &self.classes[*id];
                if args.is_empty() {
                    c.name.clone()
                } else {
                    format!(
                        "{}<{}>",
                        c.name,
                        args.iter()
                            .map(|a| self.show(a))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            Ty::Param(n) => n.clone(),
            Ty::Array(e, k) => {
                let cap = match k.cap {
                    CapKind::None => "",
                    CapKind::Capped => "k",
                    CapKind::Varint => "*",
                    CapKind::Chunked => "$",
                };
                format!("{}[{}{}]", self.show(e), k.slots(), cap)
            }
            Ty::Slice(e) if **e == Ty::u8() => "string".into(),
            Ty::Slice(e) => format!("{}[:]", self.show(e)),
            Ty::View(e) => format!("{}[:]&", self.show(e)),
            Ty::Ref(e) => format!("{}&", self.show(e)),
            Ty::Nullable(e) => format!("{}?", self.show(e)),
            Ty::Func(ps, r) => {
                let ps = if ps.is_empty() {
                    "void".to_string()
                } else {
                    ps.iter()
                        .map(|p| self.show(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                format!("func({} => {})", ps, self.show(r))
            }
            Ty::Tuple(ts) => format!(
                "({})",
                ts.iter()
                    .map(|t| self.show(t))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ty::Map(k, v) => format!("hashmap<{}, {}>", self.show(k), self.show(v)),
            Ty::Set(e) => format!("hashset<{}>", self.show(e)),
            Ty::Pool(e) => format!("pool<{}>", self.show(e)),
            Ty::Handle(e) => format!("Handle<{}>", self.show(e)),
            Ty::MapEntry(k, v) => {
                format!("an entry of hashmap<{}, {}>", self.show(k), self.show(v))
            }
            Ty::TypeName(t) => format!("the type {}", self.show(t)),
            Ty::Namespace(n) => (*n).to_string(),
            Ty::Unknown => "an unknown type".into(),
        }
    }
}

/// Fields that start empty and so take no constructor parameter (DESIGN §6.2).
pub fn starts_empty(t: &Ty) -> bool {
    match t {
        Ty::Array(_, k) => k.is_editable(),
        Ty::Map(..) | Ty::Set(_) | Ty::Pool(_) => true,
        _ => false,
    }
}
