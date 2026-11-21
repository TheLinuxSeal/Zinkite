//! The type checker, first milestone: names, types, literals, the mixed-integer
//! rule, casts, conditions, calls with overloading and generic inference,
//! statuses, loops, `switch`, lambdas, tuples, null narrowing and missing
//! returns (DESIGN §5.1, §6, §7, §9, §10).
//!
//! Not yet: definite assignment, `mut` and read-only checks, root and lifetime
//! rules (REFERENCES.md) and the unused-capability lint.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    self, AssignOp, BinOp, CaseLabel, Expr, ExprKind as E, LambdaBody, LocalType, Name, PostfixOp,
    Stmt, StmtKind as S, StrPart, SwitchBody, Target, UnaryOp,
};
use crate::builtins::{self, Need, NumCaps, Origin, SeqKind, Sig};
use crate::diag::{Diagnostic, Severity};
use crate::flow::Flow;
use crate::program::{
    ClassKind, FieldInfo, Lookup, MethodInfo, Program, Scope, active, starts_empty,
};
use crate::source::{SourceFile, Span};
use crate::token::Prim;
use crate::types::*;

/// How well an argument fits a parameter (DESIGN §6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Exact,
    Widen,
    Other,
}

/// Type-checks a program and, when it has no errors, generates its C.
pub fn check_and_generate(
    files: &[ast::File],
    sources: &[SourceFile],
) -> (Vec<(usize, Diagnostic)>, Option<String>) {
    let mut prog = Program::collect(files);
    let (diags, facts) = {
        let mut c = Checker::new(&prog);
        c.check_all();
        (c.diags, c.facts)
    };
    let mut all = std::mem::take(&mut prog.diags);
    all.extend(diags);
    if all.iter().any(|d| d.1.severity == Severity::Error) {
        return (all, None);
    }
    all.extend(crate::lifetimes::check(&prog, &facts, sources));
    if all.iter().any(|d| d.1.severity == Severity::Error) {
        return (all, None);
    }
    match crate::codegen::generate(&prog, &facts, sources) {
        Ok(c) => (all, Some(c)),
        Err(e) => {
            all.extend(e);
            (all, None)
        }
    }
}

/// Type-checks a whole program, then checks roots and lifetimes if it has no type
/// errors. Returns each diagnostic with the index of its file.
pub fn check(files: &[ast::File], sources: &[SourceFile]) -> Vec<(usize, Diagnostic)> {
    let mut prog = Program::collect(files);
    let (diags, facts) = {
        let mut c = Checker::new(&prog);
        c.check_all();
        (c.diags, c.facts)
    };
    let mut all = std::mem::take(&mut prog.diags);
    all.extend(diags);
    if !all.iter().any(|d| d.1.severity == Severity::Error) {
        all.extend(crate::lifetimes::check(&prog, &facts, sources));
    }
    all
}

/// What the checker worked out, for the passes after it: every expression's type, what
/// each name refers to, which declaration each call chose, and which lambdas capture.
#[derive(Default)]
pub struct Facts {
    pub types: HashMap<(u32, u32), Ty>,
    /// By the span of an identifier expression.
    pub names: HashMap<(u32, u32), NameRes>,
    /// Each local and parameter's type, by the span of its name where it's declared.
    pub locals: HashMap<(u32, u32), Ty>,
    /// By the span of a call, `new` or `this.new` expression.
    pub calls: HashMap<(u32, u32), CallFact>,
    /// Lambdas that capture a variable or `this`, by span.
    pub capturing: HashSet<(u32, u32)>,
    /// Assignments `r = x` that bind a reference not yet bound, rather than write through it.
    pub binds: HashSet<(u32, u32)>,
}

pub enum NameRes {
    /// A local or parameter, by the span of its declared name.
    Local((u32, u32)),
    /// `classes[c].fields[i]`, an instance field or a static.
    Field(ClassId, usize),
}

pub struct CallFact {
    pub origin: Origin,
    pub name: String,
    /// The chosen declaration's parameter types, with type arguments filled in.
    pub params: Vec<Ty>,
    /// A free function called with method syntax: the receiver is the first argument.
    pub recv_arg: bool,
}

pub fn key(s: Span) -> (u32, u32) {
    (s.start, s.end)
}

struct Local {
    name: String,
    /// Where its name is declared.
    decl: Span,
    ty: Ty,
    /// Declared without a value, so definite assignment tracks it (DESIGN §6.2).
    slot: Option<usize>,
    /// A plain parameter: a read-only copy (DESIGN §5.1).
    param: bool,
    /// `fns.len()` where it was declared: a lambda deeper than that captures it.
    depth: usize,
    /// How many loops enclosed its declaration, to tell when an assignment may repeat.
    loop_depth: usize,
    /// A reference bound to something the code can't change: why. Nothing may be written through it.
    ro_bind: Option<String>,
    /// A reference that something was written through, so it can't be bound to read-only data.
    write_use: Option<Span>,
    /// Read by a lambda, and assigned after its initialization; reported once.
    captured: bool,
    reassigned: bool,
    final_reported: bool,
}

/// Why something can't be changed here, and what to do instead.
struct Ro {
    msg: String,
    help: String,
}

fn is_ref_ty(t: &Ty) -> bool {
    matches!(t, Ty::Ref(_)) || matches!(t, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)))
}

/// A field a constructor must assign.
struct CtorField {
    name: String,
    slot: usize,
    /// A non-nullable reference or function, or a value holding one: it must be assigned even on a failing exit.
    no_zero: bool,
}

/// `a`, `a and b`, `a, b and c`.
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

/// A `return #code;` whose code is a constant other than 0.
fn failing_status(st: &ast::StatusWrite) -> bool {
    match st {
        ast::StatusWrite::Int(n) => *n != 0,
        ast::StatusWrite::Name(q) => {
            q.parts.len() == 2 && q.parts[0].name == "status" && q.parts[1].name != "OK"
        }
        ast::StatusWrite::Expr(_) => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FnKind {
    Method,
    Main,
    Ctor,
    Lambda,
    /// A field initializer.
    Init,
}

struct FnCtx {
    kind: FnKind,
    name: String,
    /// A `mut` method, which may change its object (DESIGN §6.1).
    is_mut: bool,
    ret: Ty,
    is_static: bool,
    tparams: Vec<TParam>,
    /// The labels of the enclosing loops, innermost last.
    loops: Vec<Option<String>>,
    /// A block-bodied lambda's result, from its first `return`.
    lambda_ret: Option<Ty>,
    /// Enclosing block arms of switch expressions: the type wanted, and the type yielded.
    yields: Vec<(Option<Ty>, Option<Ty>)>,
}

struct Checker<'p, 'a> {
    prog: &'p Program<'a>,
    diags: Vec<(usize, Diagnostic)>,
    file: usize,
    class: Option<ClassId>,
    locals: Vec<Local>,
    scopes: Vec<Vec<usize>>,
    /// Nullable locals known not to be null here (DESIGN §5.1).
    narrowed: HashSet<usize>,
    fns: Vec<FnCtx>,
    /// Whether the last call or array operator typed can set a nonzero status; `None` when unknown.
    last_can_fail: Option<bool>,
    /// A label waiting for the loop it names.
    pending_label: Option<String>,
    /// Definite assignment: the state here, and how many slots the function has used.
    flow: Flow,
    nslots: usize,
    /// Per enclosing loop, innermost last: its label, and the states its `break`s and `continue`s leave.
    flow_loops: Vec<(Option<String>, Vec<Flow>, Vec<Flow>)>,
    /// Per enclosing block arm of a switch expression: the states at its `yield`s.
    yield_flows: Vec<Vec<Flow>>,
    /// In a constructor: the fields it must assign.
    ctor_fields: Vec<CtorField>,
    /// Set while typing the target of a plain assignment, so the write isn't taken for a read.
    writing: bool,
    /// What later passes need; `facts.types` also serves the read-only checks.
    pub facts: Facts,
    /// Enclosing lambdas: the index of each one's `FnCtx`, and whether it captures.
    lambdas: Vec<(usize, bool)>,
    /// Set just before typing the value that initializes a named local or static, where
    /// a root may be created (DESIGN §6.6).
    root_new_ok: bool,
    /// Whether the last call chosen is a `mut` method.
    last_call_mut: bool,
}

fn is_status_const(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn starts_upper(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
}

/// Hidden type parameter names (`#K`) are shown without the `#`.
fn tp_name(n: &str) -> &str {
    n.trim_start_matches('#')
}

fn plural(n: usize, word: &str) -> String {
    format!("{} {}{}", n, word, if n == 1 { "" } else { "s" })
}

impl<'p, 'a> Checker<'p, 'a> {
    fn new(prog: &'p Program<'a>) -> Self {
        Checker {
            prog,
            diags: Vec::new(),
            file: 0,
            class: None,
            locals: Vec::new(),
            scopes: Vec::new(),
            narrowed: HashSet::new(),
            fns: Vec::new(),
            last_can_fail: None,
            pending_label: None,
            flow: Flow::default(),
            nslots: 0,
            flow_loops: Vec::new(),
            yield_flows: Vec::new(),
            ctor_fields: Vec::new(),
            writing: false,
            facts: Facts::default(),
            lambdas: Vec::new(),
            root_new_ok: false,
            last_call_mut: false,
        }
    }

    // ------------------------------------------------------------ diagnostics

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push((self.file, Diagnostic::error(span, msg)));
    }

    fn err_help(&mut self, span: Span, msg: impl Into<String>, help: impl Into<String>) {
        self.diags
            .push((self.file, Diagnostic::error(span, msg).with_help(help)));
    }

    fn warn_help(&mut self, span: Span, msg: impl Into<String>, help: impl Into<String>) {
        let d = Diagnostic {
            severity: Severity::Warning,
            span,
            message: msg.into(),
            help: Some(help.into()),
        };
        self.diags.push((self.file, d));
    }

    fn show(&self, t: &Ty) -> String {
        self.prog.show(t)
    }

    // ------------------------------------------------------------ scopes

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        if let Some(s) = self.scopes.pop() {
            for id in s {
                self.narrowed.remove(&id);
            }
        }
    }

    fn declare(&mut self, name: &ast::Ident, ty: Ty) -> usize {
        let dup = self
            .scopes
            .last()
            .is_some_and(|s| s.iter().any(|&i| self.locals[i].name == name.name));
        if dup {
            self.err(
                name.span,
                format!("`{}` is already declared in this block", name.name),
            );
        }
        let id = self.locals.len();
        self.facts.locals.insert(key(name.span), ty.clone());
        self.locals.push(Local {
            name: name.name.clone(),
            decl: name.span,
            ty,
            slot: None,
            param: false,
            depth: self.fns.len(),
            loop_depth: self.flow_loops.len(),
            ro_bind: None,
            write_use: None,
            captured: false,
            reassigned: false,
            final_reported: false,
        });
        self.scopes.last_mut().expect("a scope").push(id);
        id
    }

    fn declare_name(&mut self, name: &Name, ty: Ty) {
        if let Name::Ident(id) = name {
            self.declare(id, ty);
        }
    }

    /// A parameter. A plain one is a read-only copy; a `T&` one refers to the caller's value.
    fn declare_param(&mut self, name: &Name, ty: Ty) {
        if let Name::Ident(id) = name {
            let plain = !is_ref_ty(&ty) && !matches!(ty, Ty::View(_));
            let l = self.declare(id, ty);
            self.locals[l].param = plain;
        }
    }

    fn lookup_local(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .copied()
            .find(|&i| self.locals[i].name == name)
    }

    fn begin(&mut self, kind: FnKind, name: &str, ret: Ty, is_static: bool, tparams: Vec<TParam>) {
        if kind != FnKind::Lambda {
            self.flow = Flow::default();
            self.nslots = 0;
            self.flow_loops.clear();
            self.yield_flows.clear();
            self.ctor_fields.clear();
        }
        self.fns.push(FnCtx {
            kind,
            name: name.to_string(),
            is_mut: false,
            ret,
            is_static,
            tparams,
            loops: Vec::new(),
            lambda_ret: None,
            yields: Vec::new(),
        });
        self.push_scope();
    }

    fn end(&mut self) {
        self.pop_scope();
        self.fns.pop();
    }

    fn cur(&self) -> &FnCtx {
        self.fns.last().expect("inside a function")
    }

    fn cur_mut(&mut self) -> &mut FnCtx {
        self.fns.last_mut().expect("inside a function")
    }

    fn fn_static(&self) -> bool {
        self.fns.last().is_none_or(|f| f.is_static)
    }

    fn tparams(&self) -> Vec<TParam> {
        match self.fns.last() {
            Some(f) => f.tparams.clone(),
            None => self
                .class
                .map(|c| self.prog.classes[c].type_params.clone())
                .unwrap_or_default(),
        }
    }

    fn tparam(&self, name: &str) -> Option<TParam> {
        self.tparams().into_iter().rev().find(|t| t.name == name)
    }

    fn unmodelled(&self) -> bool {
        !self.prog.files[self.file].unmodelled.is_empty()
    }

    fn resolve(&mut self, t: &ast::Type) -> Ty {
        let tps = self.tparams();
        let mut d = Vec::new();
        let ty = self.prog.resolve_quiet(
            t,
            &Scope {
                file: self.file,
                tparams: &tps,
            },
            &mut d,
        );
        for x in d {
            self.diags.push((self.file, x));
        }
        self.type_rules(&ty, t.span);
        ty
    }

    /// The type of `this` inside class `id`.
    fn self_type(&self, id: ClassId) -> Ty {
        let c = &self.prog.classes[id];
        Ty::Class(
            id,
            c.type_params
                .iter()
                .map(|p| Ty::Param(p.name.clone()))
                .collect(),
        )
    }

    fn class_subst(&self, id: ClassId, args: &[Ty]) -> Vec<(String, Ty)> {
        self.prog.classes[id]
            .type_params
            .iter()
            .map(|p| p.name.clone())
            .zip(args.iter().cloned())
            .collect()
    }

    // ------------------------------------------------------------ declarations

    fn check_all(&mut self) {
        let prog = self.prog;
        for (id, c) in prog.classes.iter().enumerate() {
            self.file = c.file;
            self.class = Some(id);
            self.check_class_rules(id);
            let ctps = c.type_params.clone();
            for f in &c.fields {
                if let Some(init) = f.init {
                    self.begin(
                        FnKind::Init,
                        &f.name,
                        f.ty.clone(),
                        f.is_static,
                        ctps.clone(),
                    );
                    self.root_new_ok = f.is_static && matches!(init.kind, E::New { .. });
                    let t = self.expr(init, Some(&f.ty));
                    self.root_new_ok = false;
                    self.expect(&t, &f.ty, init);
                    self.end();
                }
            }
            for ctor in &c.ctors {
                let Some(d) = ctor.decl else { continue };
                self.begin(FnKind::Ctor, "new", Ty::Void, false, ctps.clone());
                for f in c.fields.iter().filter(|f| !f.is_static) {
                    let slot = self.new_slot();
                    if f.init.is_some() || starts_empty(&f.ty) {
                        self.flow.preset(slot);
                    }
                    let no_zero = self.no_zero(&f.ty, &mut Vec::new());
                    self.ctor_fields.push(CtorField {
                        name: f.name.clone(),
                        slot,
                        no_zero,
                    });
                }
                for (p, ty) in d.params.iter().zip(&ctor.params) {
                    self.type_rules(ty, p.ty.span);
                    if self.is_root_value(ty) {
                        self.root_by_value(p.ty.span, ty, "passed");
                    }
                    self.declare_param(&p.name, ty.clone());
                }
                self.block(&d.body);
                let end = d.body.span.end as usize;
                self.check_ctor_end(Span::new(end.saturating_sub(1), end), false);
                self.end();
            }
            for m in &c.methods {
                for (p, ty) in m.decl.params.iter().zip(&m.params) {
                    self.type_rules(ty, p.ty.span);
                }
                let Some(body) = &m.decl.body else { continue };
                let mut tps = ctps.clone();
                tps.extend(m.type_params.iter().cloned());
                let is_main = c.kind == ClassKind::Script && m.name == "main";
                if is_main {
                    self.check_main(m);
                }
                let kind = if is_main {
                    FnKind::Main
                } else {
                    FnKind::Method
                };
                for (p, ty) in m.decl.params.iter().zip(&m.params) {
                    if self.is_root_value(ty) {
                        self.root_by_value(p.ty.span, ty, "passed");
                    }
                }
                if self.is_root_value(&m.ret)
                    && let ast::ReturnType::Type(t) = &m.decl.ret
                {
                    self.root_by_value(t.span, &m.ret, "returned");
                }
                if m.is_mut && m.is_static {
                    self.err_help(
                        m.decl.name.span,
                        format!(
                            "`{}` is static, so it has no object for `mut` to change",
                            m.name
                        ),
                        "drop `mut`; static fields can be changed from any function (DESIGN §6.1)",
                    );
                }
                self.begin(kind, &m.name, m.ret.clone(), m.is_static, tps);
                self.cur_mut().is_mut = m.is_mut;
                for (p, ty) in m.decl.params.iter().zip(&m.params) {
                    self.declare_param(&p.name, ty.clone());
                }
                self.block(body);
                if m.ret != Ty::Void && !is_main && completes_all(&body.stmts) {
                    let help = format!(
                        "it returns {}; add a `return`, or call `fail(...)` where the end can't be reached",
                        self.show(&m.ret)
                    );
                    self.err_help(
                        m.decl.name.span,
                        format!("`{}` can reach its end without returning a value", m.name),
                        help,
                    );
                }
                self.end();
            }
        }
    }

    fn check_main(&mut self, m: &MethodInfo) {
        let ok_params = match m.params.as_slice() {
            [] => true,
            [Ty::Array(e, k)] => e.is_string() && k.is_frozen() && k.cap == CapKind::None,
            [Ty::Unknown] => true,
            _ => false,
        };
        if !ok_params {
            self.err(
                m.decl.name.span,
                "`main` takes no parameters, or `string[.] args` (DESIGN §8.6)",
            );
        }
        if !matches!(m.ret, Ty::Void | Ty::Unknown) && m.ret != Ty::u8() {
            self.err(
                m.decl.name.span,
                "`main` returns `void`, or `u8` for the exit status (DESIGN §8.6)",
            );
        }
    }

    fn check_class_rules(&mut self, id: ClassId) {
        let prog = self.prog;
        let c = &prog.classes[id];
        // Interfaces it implements (DESIGN §6.3).
        for iface in &c.implements {
            let Ty::Class(i, iargs) = iface else { continue };
            let ic = &prog.classes[*i];
            let map = self.class_subst(*i, iargs);
            for im in &ic.methods {
                let want: Vec<Ty> = im.params.iter().map(|p| p.subst(&map)).collect();
                let want_ret = im.ret.subst(&map);
                let named: Vec<&MethodInfo> =
                    c.methods.iter().filter(|m| m.name == im.name).collect();
                let sig = self.show_sig(&im.name, &want, &want_ret);
                if named.is_empty() {
                    self.err_help(
                        c.span,
                        format!(
                            "`{}` implements `{}` but has no method `{}`",
                            c.name, ic.name, im.name
                        ),
                        format!("add `{}`", sig),
                    );
                    continue;
                }
                let matching = named.iter().find(|m| {
                    m.params.len() == want.len()
                        && m.params.iter().zip(&want).all(|(a, b)| self.same(a, b))
                        && self.same(&m.ret, &want_ret)
                });
                match matching {
                    None => {
                        let m = named[0];
                        self.err_help(
                            m.decl.name.span,
                            format!(
                                "`{}.{}` doesn't match the method `{}` declares",
                                c.name, im.name, ic.name
                            ),
                            format!("`{}` declares `{}`", ic.name, sig),
                        );
                    }
                    Some(m) if m.is_mut && !im.is_mut => {
                        self.err(m.decl.name.span, format!("`{}` is `mut` here, but not in `{}`, whose callers expect it not to change the value", im.name, ic.name));
                    }
                    _ => {}
                }
            }
        }
        // `implements ordered` (DESIGN §10.1).
        if c.ordered {
            let me = self.self_type(id);
            let ok = c.methods.iter().any(|m| {
                m.name == "compareTo"
                    && m.params.len() == 1
                    && self.same(m.params[0].unref(), &me)
                    && m.ret == Ty::Prim(Prim::I32)
            });
            if !ok {
                self.err_help(c.span, format!("`{}` implements `ordered`, so it needs a `compareTo` method", c.name), format!("add `i32 compareTo({} other)`, returning below 0, 0 or above 0 (DESIGN §10.1)", c.name));
            }
        }
        // Overloads must differ in a parameter's type (DESIGN §6.5).
        for (j, mj) in c.methods.iter().enumerate() {
            if c.methods[..j]
                .iter()
                .any(|mi| mi.name == mj.name && self.same_overload(&mi.params, &mj.params))
            {
                self.err_help(
                    mj.decl.name.span,
                    format!("`{}` is declared twice with the same parameter types", mj.name),
                    "overloads must differ in a parameter's type, not only in `&`, `?`, array slots or the return type (DESIGN §6.5)",
                );
            }
        }
        for (j, cj) in c.ctors.iter().enumerate() {
            let Some(d) = cj.decl else { continue };
            if c.ctors[..j]
                .iter()
                .any(|ci| self.same_overload(&ci.params, &cj.params))
            {
                self.err(
                    d.span,
                    "this constructor has the same parameter types as an earlier one (DESIGN §6.5)",
                );
            }
        }
        // Fields: one growable field per class; roots and cursor arrays (DESIGN §3.4, §6.6).
        let mut growable: Option<&str> = None;
        for f in &c.fields {
            self.type_rules(&f.ty, f.span);
            if f.is_static {
                continue;
            }
            if matches!(&f.ty, Ty::Class(r, _) if prog.classes[*r].kind == ClassKind::Root) {
                self.err_help(
                    f.span,
                    "a root can't be stored by value in a class, a root or an array",
                    "store a reference to it instead (DESIGN §6.6)",
                );
            }
            if c.kind != ClassKind::Class {
                continue;
            }
            if let Ty::Array(_, k) = &f.ty
                && k.has_cursor()
                && k.cap != CapKind::Capped
            {
                self.err_help(
                        f.span,
                        "a cursor array needs two regions, its data and its index, so only a root can hold one",
                        format!("declare `root {}` instead of `class {}` (DESIGN §6.6)", c.name, c.name),
                    );
                continue;
            }
            if starts_empty(&f.ty) {
                match growable {
                    Some(first) => self.err_help(
                        f.span,
                        format!("a class can hold only one growable field, and `{}` is already one", first),
                        format!("declare `root {}`, which gives each growable field its own data stack (DESIGN §6.6)", c.name),
                    ),
                    None => growable = Some(&f.name),
                }
            }
        }
    }

    fn same_overload(&self, a: &[Ty], b: &[Ty]) -> bool {
        fn norm(t: &Ty) -> Ty {
            match t {
                Ty::Ref(x) | Ty::Nullable(x) => norm(x),
                Ty::Array(e, _) => Ty::Array(Box::new(norm(e)), ArrayKind::frozen()),
                Ty::View(e) => Ty::Slice(Box::new(norm(e))),
                t => t.clone(),
            }
        }
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| norm(x) == norm(y))
    }

    fn show_sig(&self, name: &str, params: &[Ty], ret: &Ty) -> String {
        let ps: Vec<String> = params.iter().map(|p| self.show(p)).collect();
        format!("{} {}({})", self.show(ret), name, ps.join(", "))
    }

    /// Rules on a written type: hashable keys, and what can be an element (DESIGN §3.4, §4.14).
    fn type_rules(&mut self, ty: &Ty, span: Span) {
        match ty {
            Ty::Map(k, v) => {
                if !self.hashable(k) {
                    self.err_help(span, format!("a hashmap key must be hashable, and `{}` isn't", self.show(k)), "keys are integers, bool, string, enums, or classes whose fields all are (DESIGN §4.14)");
                }
                if starts_empty(v) {
                    self.err_help(span, "hashmap values can't grow", "use a composite key, or values that are indexes into one shared array (DESIGN §4.14)");
                }
                self.type_rules(k, span);
                self.type_rules(v, span);
            }
            Ty::Set(e) => {
                if !self.hashable(e) {
                    self.err_help(span, format!("a hashset element must be hashable, and `{}` isn't", self.show(e)), "elements are integers, bool, string, enums, or classes whose fields all are (DESIGN §4.14)");
                }
                self.type_rules(e, span);
            }
            Ty::Array(e, _) | Ty::Pool(e) => {
                if starts_empty(e) {
                    self.err_help(
                        span,
                        "an element can't grow: an array of growable arrays isn't allowed",
                        "keep the elements in one shared array and store indexes (DESIGN §3.4)",
                    );
                } else if matches!(&**e, Ty::Class(r, _) if self.prog.classes[*r].kind == ClassKind::Root)
                {
                    self.err_help(span, "a root can't be an element", "a root has its own data stacks, so it can only be a named local or static (DESIGN §6.6)");
                }
                self.type_rules(e, span);
            }
            Ty::Ref(e) | Ty::Nullable(e) | Ty::Slice(e) | Ty::View(e) | Ty::Handle(e) => {
                self.type_rules(e, span)
            }
            Ty::Func(ps, r) => {
                for p in ps {
                    self.type_rules(p, span);
                }
                self.type_rules(r, span);
            }
            Ty::Tuple(ts) => {
                for t in ts {
                    self.type_rules(t, span);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ type relations

    fn same(&self, a: &Ty, b: &Ty) -> bool {
        use Ty::*;
        let all = |xs: &[Ty], ys: &[Ty]| {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| self.same(x, y))
        };
        match (a, b) {
            (Unknown, _) | (_, Unknown) => true,
            (Class(x, xa), Class(y, ya)) => x == y && all(xa, ya),
            (Array(x, k1), Array(y, k2)) => k1 == k2 && self.same(x, y),
            (Slice(x), Slice(y))
            | (View(x), View(y))
            | (Ref(x), Ref(y))
            | (Nullable(x), Nullable(y))
            | (Set(x), Set(y))
            | (Pool(x), Pool(y))
            | (Handle(x), Handle(y))
            | (Iter(x), Iter(y))
            | (TypeName(x), TypeName(y)) => self.same(x, y),
            (Func(p1, r1), Func(p2, r2)) => all(p1, p2) && self.same(r1, r2),
            (Tuple(x), Tuple(y)) => all(x, y),
            (Map(k1, v1), Map(k2, v2)) | (MapEntry(k1, v1), MapEntry(k2, v2)) => {
                self.same(k1, k2) && self.same(v1, v2)
            }
            _ => a == b,
        }
    }

    fn lit_fits_param(&self, value: Option<i128>, is_float: bool, tp: &TParam) -> bool {
        use BoundKind::*;
        if is_float {
            return tp.has(Float);
        }
        let range = if tp.has(Sint) {
            (-128, 127)
        } else if tp.has(Uint) {
            (0, 255)
        } else if tp.has(Float) {
            return true;
        } else if tp.has(Number) {
            (0, 127)
        } else {
            return false;
        };
        value.is_none_or(|v| v >= range.0 && v <= range.1)
    }

    fn lit_rank(value: Option<i128>, is_char: bool, p: Prim) -> Option<Rank> {
        if p.is_float() {
            return Some(Rank::Widen);
        }
        if !p.is_int() {
            return None;
        }
        if let Some(v) = value
            && (!fits(v, p) || (is_char && p == Prim::U8 && v >= 128))
        {
            return None;
        }
        let default = if is_char { Prim::U32 } else { Prim::I64 };
        Some(if p == default {
            Rank::Exact
        } else {
            Rank::Widen
        })
    }

    fn prim_rank(a: Prim, b: Prim) -> Option<Rank> {
        if a == b {
            return Some(Rank::Exact);
        }
        if !lossless(a, b) {
            return None;
        }
        let same_kind =
            (a.is_int() && b.is_int() && signed(a) == signed(b)) || (a.is_float() && b.is_float());
        Some(if same_kind { Rank::Widen } else { Rank::Other })
    }

    /// How well a value of type `from` fits where `to` is expected; `None` when it doesn't.
    fn rank(&self, from: &Ty, to: &Ty) -> Option<Rank> {
        use Ty::*;
        if from.is_unknown() || to.is_unknown() {
            return Some(Rank::Exact);
        }
        match (from, to) {
            (_, Nullable(t)) => match from {
                Null => Some(Rank::Exact),
                Nullable(f) => self.rank(f, t),
                _ => self.rank(from, t),
            },
            (Null, _) => None,
            (Ref(f), Ref(t)) => self.ref_rank(f, t),
            (_, Ref(t)) => self.ref_rank(from, t),
            (Ref(f), _) => self.rank(f, to),
            (IntLit { value, is_char }, Prim(p)) => Self::lit_rank(*value, *is_char, *p),
            (IntLit { value, .. }, Param(n)) => self
                .tparam(n)
                .filter(|tp| self.lit_fits_param(*value, false, tp))
                .map(|_| Rank::Widen),
            (FloatLit, Prim(p)) => match p {
                crate::token::Prim::F64 => Some(Rank::Exact),
                crate::token::Prim::F32 => Some(Rank::Widen),
                _ => None,
            },
            (FloatLit, Param(n)) => self
                .tparam(n)
                .filter(|tp| tp.has(BoundKind::Float))
                .map(|_| Rank::Widen),
            (Prim(a), Prim(b)) => Self::prim_rank(*a, *b),
            (Array(e, k), Slice(t) | View(t)) => {
                (k.sliceable() && self.same(e, t)).then_some(Rank::Other)
            }
            (View(e), Slice(t)) => self.same(e, t).then_some(Rank::Exact),
            (Slice(e) | View(e), Array(t, _)) => self.same(e, t).then_some(Rank::Other),
            (Array(e, k1), Array(t, k2)) => {
                self.same(e, t)
                    .then_some(if k1 == k2 { Rank::Exact } else { Rank::Other })
            }
            (Tuple(a), Tuple(b)) if a.len() == b.len() => {
                let mut worst = Rank::Exact;
                for (x, y) in a.iter().zip(b) {
                    worst = worst.max(self.rank(x, y)?);
                }
                Some(worst)
            }
            _ => self.same(from, to).then_some(Rank::Exact),
        }
    }

    /// How well a place of type `f` can be bound to a reference to `t`.
    fn ref_rank(&self, f: &Ty, t: &Ty) -> Option<Rank> {
        use Ty::*;
        let f = f.unref();
        if self.same(f, t) {
            return Some(Rank::Exact);
        }
        match (f, t) {
            (Array(e, k1), Array(e2, k2)) if self.same(e, e2) && k1.satisfies(k2) => {
                Some(Rank::Widen)
            }
            (Class(..) | Param(_), Class(i, _))
                if self.is_interface(*i) && self.implements(f, t) =>
            {
                Some(Rank::Other)
            }
            _ => None,
        }
    }

    fn is_interface(&self, id: ClassId) -> bool {
        self.prog.classes[id].kind == ClassKind::Interface
    }

    fn implements(&self, t: &Ty, iface: &Ty) -> bool {
        match t.unref() {
            Ty::Class(c, args) => {
                let map = self.class_subst(*c, args);
                self.prog.classes[*c]
                    .implements
                    .iter()
                    .any(|i| self.same(&i.subst(&map), iface))
            }
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| {
                tp.bounds
                    .iter()
                    .any(|b| matches!(b, Bound::Interface(i) if self.same(i, iface)))
            }),
            Ty::Unknown => true,
            _ => false,
        }
    }

    fn satisfies(&self, t: &Ty, b: &Bound) -> bool {
        match b {
            Bound::Builtin(k) => match t.unref() {
                Ty::Prim(p) => k.admits_prim(*p),
                Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(*k)),
                Ty::Unknown => true,
                t => match k {
                    BoundKind::Ordered => self.ordered(t),
                    BoundKind::Hashable => self.hashable(t),
                    _ => false,
                },
            },
            Bound::Interface(i) => self.implements(t, i),
        }
    }

    fn ordered(&self, t: &Ty) -> bool {
        match t.unref() {
            Ty::Prim(_) | Ty::Unknown => true,
            t if t.is_string() => true,
            Ty::Class(id, _) => {
                let c = &self.prog.classes[*id];
                c.kind == ClassKind::Enum || c.ordered
            }
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Ordered)),
            _ => false,
        }
    }

    fn hashable(&self, t: &Ty) -> bool {
        self.hashable_in(t, &mut Vec::new())
    }

    fn hashable_in(&self, t: &Ty, seen: &mut Vec<ClassId>) -> bool {
        match t {
            Ty::Prim(p) => !p.is_float(),
            Ty::Unknown | Ty::Handle(_) => true,
            t if t.is_string() => true,
            Ty::Class(id, args) => {
                let c = &self.prog.classes[*id];
                if c.kind == ClassKind::Enum || seen.contains(id) {
                    return true;
                }
                if c.kind != ClassKind::Class {
                    return false;
                }
                seen.push(*id);
                let map = self.class_subst(*id, args);
                let ok = c
                    .fields
                    .iter()
                    .filter(|f| !f.is_static)
                    .all(|f| self.hashable_in(&f.ty.subst(&map), seen));
                seen.pop();
                ok
            }
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Hashable)),
            _ => false,
        }
    }

    /// Whether an expression names storage a reference can point at.
    fn is_place(&self, e: &Expr) -> bool {
        match &e.kind {
            E::Ident(_) | E::This | E::Member { .. } | E::Index { .. } | E::Slice { .. } => true,
            E::Paren(x) => self.is_place(x),
            _ => false,
        }
    }

    /// Checks that a value of type `from`, computed by `e`, can go where `to` is expected.
    fn expect(&mut self, from: &Ty, to: &Ty, e: &Expr) -> bool {
        let place = self.is_place(e);
        self.expect_at(from, to, e.span, place)
    }

    fn expect_at(&mut self, from: &Ty, to: &Ty, span: Span, place: bool) -> bool {
        use Ty::*;
        if self.rank(from, to).is_some() {
            let binds = matches!(to, Ref(_)) || matches!(to, Nullable(x) if matches!(**x, Ref(_)));
            if binds && !place && !matches!(from, Ref(_) | Nullable(_) | Null | Unknown) {
                self.err_help(span, "a reference needs a variable, field or element to refer to, and this is a temporary value", "store the value in a variable first, or copy it with a plain type (DESIGN §5.1)");
                return false;
            }
            return true;
        }
        let shown_to = self.show(to);
        let shown_from = self.show(from);
        match (from.unref(), to) {
            (IntLit { value: Some(v), is_char: true }, Prim(crate::token::Prim::U8)) if fits(*v, crate::token::Prim::U8) => self.err_help(
                span,
                "this character isn't ASCII, so it can't be a `u8`",
                "in UTF-8 text it is more than one byte, so comparing it with a byte would never match; compare with a string instead (DESIGN §9.1)",
            ),
            (IntLit { value: Some(v), .. }, Prim(p)) if p.is_int() => {
                let help = format!("`({}%) x` wraps and `({}|) x` saturates, if that's what you want (DESIGN §9.1)", p.name(), p.name());
                self.err_help(span, format!("{} doesn't fit in {}", v, p.name()), help)
            }
            (IntLit { .. } | FloatLit, Param(n)) => self.err_help(
                span,
                format!("this literal doesn't fit every type `{}` can be", n),
                "in a generic body, a literal must fit every type the bound allows: 0 to 127 under `number` and `int` (DESIGN §10.1); a cast `(T) x` checks it per instance",
            ),
            (Nullable(inner), _) if self.rank(inner, to).is_some() => {
                self.err_help(span, "this may be null here", "check `x != null` first, in a local variable (DESIGN §5.1)")
            }
            (Null, _) => self.err(span, format!("`null` is only for nullable references (`T&?`), and this is {}", shown_to)),
            (Slice(_), View(_)) => self.err_help(span, "a read-only slice can't become a writable view", "pass the array itself, or a `T[:]&` (DESIGN §4.10)"),
            (Array(_, k), Slice(_) | View(_)) if !k.sliceable() => self.err_help(
                span,
                format!("`{}` doesn't keep its elements in order in one block, so it can't be viewed as a slice", shown_from),
                "slices of cursor, chunked and reversed arrays on data stacks are undecided (DESIGN §17); capped arrays, ring and gap buffers included, can be sliced",
            ),
            (Void, _) => self.err(span, "this returns nothing, so it has no value"),
            (TypeName(_) | Namespace(_), _) => self.err(span, format!("`{}` is a type, not a value", shown_from)),
            (Tuple(_), t) if !matches!(t, Tuple(_)) => {
                self.err_help(span, "a call with several results must be destructured", "write `(T a, T b) = f();`, or `_ = f();` to drop them (DESIGN §9.5)")
            }
            (Prim(a), Prim(b)) if (a.is_int() || a.is_float()) && (b.is_int() || b.is_float()) => {
                let help = if a.is_float() && b.is_int() {
                    format!("`({}) x` truncates toward zero; use `math.round` first to round (DESIGN §9.1)", b.name())
                } else {
                    format!("write `({}) x`, which stops the program if the value doesn't fit (DESIGN §9.1)", b.name())
                };
                self.err_help(span, format!("{} doesn't convert to {} implicitly", a.name(), b.name()), help)
            }
            (Class(c, _), Ref(i)) if matches!(**i, Class(ii, _) if self.is_interface(ii)) => {
                self.err(span, format!("`{}` doesn't implement `{}`", self.prog.classes[*c].name, self.show(i)))
            }
            _ => self.err(span, format!("expected {}, found {}", shown_to, shown_from)),
        }
        false
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self, b: &ast::Block) {
        self.push_scope();
        for s in &b.stmts {
            self.stmt(s);
        }
        self.pop_scope();
    }

    fn take_label(&mut self) -> Option<String> {
        self.pending_label.take()
    }

    /// Checks a loop's body. Returns the states its `break`s and its `continue`s leave.
    fn loop_body(&mut self, label: Option<String>, body: &Stmt) -> (Vec<Flow>, Vec<Flow>) {
        self.cur_mut().loops.push(label.clone());
        self.flow_loops.push((label, Vec::new(), Vec::new()));
        self.stmt(body);
        self.cur_mut().loops.pop();
        let (_, breaks, continues) = self.flow_loops.pop().expect("a loop");
        (breaks, continues)
    }

    /// Sets the state after a loop. `exit` is the state where its condition ends it, if it can;
    /// `back` are the states that go around again, which a later `break` may have passed through.
    fn loop_exit(&mut self, exit: Option<Flow>, breaks: Vec<Flow>, back: Vec<Flow>) {
        let mut out = Flow::join_all(exit.into_iter().chain(breaks));
        for b in &back {
            out.absorb_maybe(b);
        }
        self.flow = out;
    }

    /// Ends the narrowing of locals that `body` rebinds, since a loop may come back to its start.
    fn unnarrow_rebound(&mut self, body: &Stmt) {
        let mut names = Vec::new();
        rebound_names(body, &mut names);
        for n in names {
            if let Some(id) = self.lookup_local(&n) {
                self.narrowed.remove(&id);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        if !matches!(
            s.kind,
            S::While { .. }
                | S::Repeat { .. }
                | S::DoWhile { .. }
                | S::For { .. }
                | S::ForEach { .. }
                | S::Labeled { .. }
        ) {
            self.pending_label = None;
        }
        match &s.kind {
            S::Block(b) => self.block(b),
            S::Local(d) => self.local_decl(d),
            S::If { cond, then, els } => self.if_stmt(cond, then, els.as_deref()),
            S::While { cond, body } => {
                let label = self.take_label();
                self.unnarrow_rebound(body);
                let base = self.narrowed.clone();
                self.cond(cond);
                let after_cond = self.flow.clone();
                let (t, _) = self.facts(cond);
                self.narrowed.extend(t);
                let (breaks, mut back) = self.loop_body(label, body);
                back.push(self.take_flow());
                self.loop_exit((!is_true(cond)).then_some(after_cond), breaks, back);
                self.narrowed = base;
            }
            S::Repeat {
                target,
                source,
                capture,
                body,
            } => {
                let label = self.take_label();
                if let Some(c) = capture {
                    self.declare_name(c, Ty::u8());
                    if !matches!(body.kind, S::Block(_)) {
                        self.err(
                            body.span,
                            "with a status capture, the loop's body must be a block (DESIGN §9.4)",
                        );
                    }
                }
                self.unnarrow_rebound(body);
                let base = self.narrowed.clone();
                if !is_status_source(source) {
                    self.err(source.span, "a repeat loop's source is a call or an array operator such as `todo ->` (DESIGN §9.4)");
                }
                self.last_can_fail = None;
                let t = self.expr_c(source, None, true);
                if self.last_can_fail == Some(false) {
                    self.err_help(
                        source.span,
                        "this never sets a status, so the loop would never end",
                        "use `while (true)` with a `break` (DESIGN §9.4)",
                    );
                }
                let after_source = self.flow.clone();
                self.push_scope();
                self.bind_loop_target(target, &t, source.span);
                let (breaks, mut back) = self.loop_body(label, body);
                back.push(self.take_flow());
                self.loop_exit(Some(after_source), breaks, back);
                self.pop_scope();
                self.narrowed = base;
            }
            S::DoWhile { body, cond } => {
                let label = self.take_label();
                self.unnarrow_rebound(body);
                let base = self.narrowed.clone();
                let (breaks, continues) = self.loop_body(label, body);
                let end = self.take_flow();
                self.flow = Flow::join_all(std::iter::once(end).chain(continues));
                self.cond(cond);
                let after_cond = self.flow.clone();
                self.loop_exit(
                    (!is_true(cond)).then_some(after_cond.clone()),
                    breaks,
                    vec![after_cond],
                );
                self.narrowed = base;
            }
            S::For {
                init,
                cond,
                update,
                body,
            } => {
                let label = self.take_label();
                self.push_scope();
                match init {
                    Some(ast::ForInit::Decl(d)) => self.local_decl(d),
                    Some(ast::ForInit::Exprs(es)) => {
                        for e in es {
                            self.expr_stmt(e);
                        }
                    }
                    None => {}
                }
                self.unnarrow_rebound(body);
                let base = self.narrowed.clone();
                if let Some(c) = cond {
                    self.cond(c);
                    let (t, _) = self.facts(c);
                    self.narrowed.extend(t);
                }
                let after_cond = self.flow.clone();
                let (breaks, continues) = self.loop_body(label, body);
                let end = self.take_flow();
                self.flow = Flow::join_all(std::iter::once(end).chain(continues));
                for u in update {
                    self.expr_stmt(u);
                }
                let back = self.take_flow();
                let exits = cond.as_ref().is_some_and(|c| !is_true(c));
                self.loop_exit(exits.then_some(after_cond), breaks, vec![back]);
                self.narrowed = base;
                self.pop_scope();
            }
            S::ForEach {
                ty,
                name,
                iter,
                body,
            } => {
                let label = self.take_label();
                let it = self.expr(iter, None);
                let (elem, by_ref) = self.iter_elem(&it, iter);
                let after_iter = self.flow.clone();
                self.push_scope();
                self.bind_local_type(ty, name, &elem, by_ref, iter.span);
                if let Name::Ident(n) = name
                    && let Some(id) = self.lookup_local(&n.name)
                    && is_ref_ty(&self.locals[id].ty)
                {
                    self.bind_ref(id, iter);
                }
                self.unnarrow_rebound(body);
                let base = self.narrowed.clone();
                let (breaks, mut back) = self.loop_body(label, body);
                back.push(self.take_flow());
                self.loop_exit(Some(after_iter), breaks, back);
                self.narrowed = base;
                self.pop_scope();
            }
            S::Switch(sw) => {
                self.switch(sw, None, false);
            }
            S::Break(label) => self.jump("break", label.as_ref(), s.span),
            S::Continue(label) => self.jump("continue", label.as_ref(), s.span),
            S::Return { status, value } => {
                self.return_stmt(status.as_ref(), value.as_ref(), s.span)
            }
            S::Destructure {
                targets,
                capture,
                value,
            } => self.destructure(targets, capture.as_ref(), value),
            S::Defer(inner) => {
                if let Some(sp) = jumps_out(inner, 0) {
                    self.err(
                        sp,
                        "a deferred statement can't `return`, `break` or `continue` (DESIGN §6.4)",
                    );
                }
                // It runs when the block ends; what it assigns doesn't count before then.
                let saved = self.flow.clone();
                self.stmt(inner);
                self.flow = saved;
            }
            S::Yield(e) => self.yield_stmt(e, s.span),
            S::Labeled { label, stmt } => {
                if matches!(
                    stmt.kind,
                    S::While { .. }
                        | S::Repeat { .. }
                        | S::DoWhile { .. }
                        | S::For { .. }
                        | S::ForEach { .. }
                ) {
                    self.pending_label = Some(label.name.clone());
                } else {
                    self.err(
                        label.span,
                        "a label names a loop, for `break` and `continue` (GRAMMAR §5)",
                    );
                }
                self.stmt(stmt);
            }
            S::Expr(e) => self.expr_stmt(e),
            S::If_(b) => {
                for st in active(b) {
                    self.stmt(st);
                }
            }
            S::Empty => {}
        }
    }

    fn if_stmt(&mut self, cond: &Expr, then: &Stmt, els: Option<&Stmt>) {
        self.push_scope(); // `instanceof` bindings
        self.cond(cond);
        let after_cond = self.flow.clone();
        let (t, f) = self.facts(cond);
        let saved = self.narrowed.clone();
        self.narrowed.extend(t.iter().copied());
        self.stmt(then);
        let then_flow = std::mem::replace(&mut self.flow, after_cond);
        let after_then = std::mem::replace(&mut self.narrowed, saved.clone());
        self.narrowed.extend(f.iter().copied());
        if let Some(e) = els {
            self.stmt(e);
        }
        let else_flow = self.take_flow();
        self.flow = Flow::join(then_flow, else_flow);
        let after_else = std::mem::replace(&mut self.narrowed, saved);
        let then_exits = !completes(then);
        let else_exits = els.is_some_and(|e| !completes(e));
        self.narrowed = match (then_exits, else_exits) {
            (true, true) => self.narrowed.clone(),
            (true, false) => after_else,
            (false, true) => after_then,
            (false, false) => after_then.intersection(&after_else).copied().collect(),
        };
        // Bindings made by `instanceof` end with the `if`; narrowings of outer locals stay.
        let scope = self.scopes.pop().unwrap_or_default();
        for id in scope {
            self.narrowed.remove(&id);
        }
    }

    fn jump(&mut self, kw: &str, label: Option<&ast::Ident>, span: Span) {
        let loops = &self.cur().loops;
        match label {
            None if loops.is_empty() => {
                let help = if kw == "break" {
                    "a switch never falls through, so its arms need no `break` (DESIGN §9.4)"
                } else {
                    "`continue` goes to the next pass of a loop"
                };
                self.err_help(span, format!("`{}` is only for loops", kw), help);
            }
            Some(l) if !loops.iter().any(|x| x.as_deref() == Some(l.name.as_str())) => {
                self.err(l.span, format!("no enclosing loop is labeled `{}`", l.name));
            }
            _ => {}
        }
        let flow = self.take_flow();
        let target = match label {
            None => self.flow_loops.len().checked_sub(1),
            Some(l) => self
                .flow_loops
                .iter()
                .rposition(|x| x.0.as_deref() == Some(l.name.as_str())),
        };
        if let Some(i) = target {
            if kw == "break" {
                self.flow_loops[i].1.push(flow);
            } else {
                self.flow_loops[i].2.push(flow);
            }
        }
    }

    fn return_stmt(&mut self, status: Option<&ast::StatusWrite>, value: Option<&Expr>, span: Span) {
        let (kind, ret, name) = {
            let c = self.cur();
            (c.kind, c.ret.clone(), c.name.clone())
        };
        if let Some(st) = status {
            self.status_write(st, span);
            if kind == FnKind::Main {
                self.err_help(
                    span,
                    "`main` can't set a status",
                    "return the exit status instead: `return 1;` (DESIGN §8.6)",
                );
            }
        }
        match kind {
            FnKind::Lambda => {
                let t = match value {
                    Some(v) => {
                        let t = self.expr(v, None);
                        self.var_type(&t, v.span)
                    }
                    None => Ty::Void,
                };
                match self.cur().lambda_ret.clone() {
                    None => self.cur_mut().lambda_ret = Some(t),
                    Some(prev) => {
                        if self.rank(&t, &prev).is_none() {
                            self.err(
                                span,
                                format!(
                                    "this lambda returned {} before, and {} here",
                                    self.show(&prev),
                                    self.show(&t)
                                ),
                            );
                        }
                    }
                }
            }
            FnKind::Init => self.err(span, "`return` isn't allowed in a field initializer"),
            FnKind::Ctor => {
                if let Some(v) = value {
                    self.expr(v, None);
                    self.err(v.span, "a constructor returns nothing; `return;` and `return #code;` leave it early (DESIGN §6.2)");
                }
                self.check_ctor_end(span, status.is_some_and(failing_status));
            }
            FnKind::Method | FnKind::Main => match (value, &ret) {
                (None, Ty::Void) => {}
                (Some(v), Ty::Void) => {
                    self.expr(v, None);
                    self.err(
                        v.span,
                        format!("`{}` returns nothing, so `return` takes no value", name),
                    );
                }
                (None, r) => {
                    let msg = format!(
                        "`{}` returns {}, so `return` needs a value, even with a status (DESIGN §7.5)",
                        name,
                        self.show(r)
                    );
                    self.err(span, msg);
                }
                (Some(v), r) => {
                    if matches!(v.kind, E::StatusRead(_)) && *r != Ty::u8() && !r.is_unknown() {
                        self.err_help(
                            v.span,
                            "`@` after `return` reads a status as a value",
                            "to pass a status on, write `return #(err) value;` (DESIGN §7.2)",
                        );
                    }
                    let t = self.expr(v, Some(r));
                    if self.expect(&t, r, v)
                        && (is_ref_ty(r) || matches!(r, Ty::View(_)))
                        && let Some(ro) = self.read_only_returned(v)
                    {
                        self.err_help(
                            v.span,
                            format!("can't return a reference to this: {}", ro.msg),
                            ro.help,
                        );
                    }
                }
            },
        }
        self.flow = Flow::unreachable();
    }

    fn status_write(&mut self, st: &ast::StatusWrite, span: Span) {
        match st {
            ast::StatusWrite::Int(n) => {
                if *n > 255 {
                    self.err(span, "a status is a u8, from 0 to 255 (DESIGN §7.2)");
                }
            }
            ast::StatusWrite::Name(q) => {
                if q.parts.len() == 2 && q.parts[0].name == "status" {
                    if !is_status_const(&q.parts[1].name) {
                        self.err(
                            q.span,
                            format!("`status.{}` isn't a status code", q.parts[1].name),
                        );
                    }
                } else if q.parts.len() == 1 {
                    let t = self.ident(&q.parts[0].name, q.span);
                    if !matches!(t.unref(), Ty::Prim(Prim::U8 | Prim::Bool) | Ty::Unknown) {
                        self.err_help(
                            q.span,
                            format!(
                                "`#` takes a u8 or bool variable, and `{}` is {}",
                                q.parts[0].name,
                                self.show(&t)
                            ),
                            "write `#((u8) n)`, or `#(n != 0)` (DESIGN §7.2)",
                        );
                    }
                }
            }
            ast::StatusWrite::Expr(e) => {
                let t = self.expr(e, Some(&Ty::u8()));
                let ok = match t.unref() {
                    Ty::Prim(Prim::U8 | Prim::Bool) | Ty::Unknown => true,
                    Ty::IntLit { value, .. } => value.is_none_or(|v| (0..=255).contains(&v)),
                    _ => false,
                };
                if !ok {
                    self.err_help(
                        e.span,
                        format!(
                            "`#( ... )` takes a u8 or a bool, and this is {}",
                            self.show(&t)
                        ),
                        "write `#((u8) n)`, or `#(n != 0)` (DESIGN §7.2)",
                    );
                }
            }
        }
    }

    fn yield_stmt(&mut self, e: &Expr, span: Span) {
        let Some((want, _)) = self.cur().yields.last().cloned() else {
            self.expr(e, None);
            self.err(span, "`yield` gives the value of a block arm in a switch expression, and there is none here");
            self.flow = Flow::unreachable();
            return;
        };
        let t = self.expr(e, want.as_ref());
        if let Some(w) = &want {
            self.expect(&t, w, e);
        }
        let got = self.cur().yields.last().and_then(|y| y.1.clone());
        let merged = match got {
            None => t,
            Some(prev) => self.unify(&prev, &t, e.span),
        };
        if let Some(y) = self.cur_mut().yields.last_mut() {
            y.1 = Some(merged);
        }
        let flow = self.take_flow();
        if let Some(y) = self.yield_flows.last_mut() {
            y.push(flow);
        }
    }

    fn expr_stmt(&mut self, e: &Expr) {
        let useful = matches!(
            &e.kind,
            E::Assign { .. }
                | E::StatusAssign { .. }
                | E::Call { .. }
                | E::Postfix { .. }
                | E::ThisNew(_)
                | E::New { .. }
                | E::Unary {
                    op: UnaryOp::PreInc | UnaryOp::PreDec,
                    ..
                }
                | E::Binary {
                    op: BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor,
                    ..
                }
        );
        self.expr(e, None);
        if !useful {
            self.err(
                e.span,
                "this expression's value isn't used, so the statement does nothing",
            );
        }
        if never_returns(e) {
            self.flow = Flow::unreachable();
        }
    }

    fn local_decl(&mut self, d: &ast::LocalDecl) {
        let declared = match &d.ty {
            LocalType::Type(t) => Some(self.resolve(t)),
            _ => None,
        };
        for v in &d.decls {
            let consumed = v.capture.is_some();
            self.root_new_ok = v
                .init
                .as_ref()
                .is_some_and(|i| matches!(i.kind, E::New { .. }));
            let ty = match (&declared, &d.ty, &v.init) {
                (Some(want), _, Some(init)) => {
                    let t = self.expr_c(init, Some(want), consumed);
                    self.expect(&t, want, init);
                    want.clone()
                }
                (Some(want), _, None) => want.clone(),
                (None, LocalType::VarRef, Some(init)) => {
                    let t = self.expr_c(init, None, consumed);
                    match t {
                        Ty::Ref(_) | Ty::Unknown => t,
                        t if self.is_place(init) => Ty::Ref(Box::new(t.unref().clone())),
                        _ => {
                            self.err_help(init.span, "`var&` needs a variable, field or element to refer to, and this is a temporary value", "use `var` to keep a copy (DESIGN §5.1)");
                            Ty::Unknown
                        }
                    }
                }
                (None, _, Some(init)) => {
                    let t = self.expr_c(init, None, consumed);
                    self.var_type(&t, init.span)
                }
                (None, _, None) => {
                    self.err(
                        v.name.span,
                        "`var` needs an initializer to take its type from",
                    );
                    Ty::Unknown
                }
            };
            self.root_new_ok = false;
            if let Some(init) = &v.init
                && self.is_root_value(&ty)
                && !matches!(init.kind, E::New { .. })
            {
                self.root_copy(init.span, &ty);
            }
            let is_view = matches!(ty, Ty::View(_));
            let is_ref = is_ref_ty(&ty);
            let id = self.declare(&v.name, ty);
            if let Some(init) = &v.init {
                if is_ref {
                    self.bind_ref(id, init);
                } else if is_view && declared.is_some() {
                    self.need_writable(init, "can't take a writable view of this");
                }
            }
            if v.init.is_none() && declared.is_some() {
                let slot = self.new_slot();
                self.locals[id].slot = Some(slot);
            }
            if let Some(c) = &v.capture {
                self.declare_name(c, Ty::u8());
            }
        }
    }

    /// The type `var x = e` gives `x`.
    fn var_type(&mut self, t: &Ty, span: Span) -> Ty {
        match t {
            Ty::Ref(x) => self.var_type(x, span),
            Ty::IntLit { .. } | Ty::FloatLit => t.default_lit(),
            Ty::Null => {
                self.err(span, "`null` doesn't say which type it is; write the variable's type, such as `Node&?`");
                Ty::Unknown
            }
            Ty::Void => {
                self.err(span, "this returns nothing, so it has no value");
                Ty::Unknown
            }
            Ty::TypeName(_) | Ty::Namespace(_) => {
                self.err(span, format!("`{}` is a type, not a value", self.show(t)));
                Ty::Unknown
            }
            Ty::Tuple(_) => {
                self.err_help(
                    span,
                    "a call with several results must be destructured",
                    "write `(T a, T b) = f();` (DESIGN §9.5)",
                );
                Ty::Unknown
            }
            t => t.clone(),
        }
    }

    /// Declares a loop or destructuring variable whose value has type `elem`.
    fn bind_local_type(
        &mut self,
        lt: &LocalType,
        name: &Name,
        elem: &Ty,
        by_ref_ok: bool,
        span: Span,
    ) {
        let ty = match lt {
            LocalType::Type(t) => {
                let declared = self.resolve(t);
                match &declared {
                    Ty::Ref(inner) => {
                        if !by_ref_ok {
                            self.err_help(
                                t.span,
                                "these values can't be referred to with `&`",
                                "drop the `&` to work with copies",
                            );
                        } else if self.ref_rank(elem, inner).is_none() && !elem.is_unknown() {
                            self.err(
                                t.span,
                                format!(
                                    "expected a reference to {}, but the values are {}",
                                    self.show(inner),
                                    self.show(elem)
                                ),
                            );
                        }
                    }
                    _ => {
                        if self.rank(elem, &declared).is_none() {
                            self.expect_at(elem, &declared, t.span, true);
                        }
                    }
                }
                declared
            }
            LocalType::Var => self.var_type(elem, span),
            LocalType::VarRef => {
                if !by_ref_ok {
                    self.err_help(
                        span,
                        "these values can't be referred to with `&`",
                        "use `var` to work with copies",
                    );
                }
                Ty::Ref(Box::new(elem.unref().clone()))
            }
        };
        self.declare_name(name, ty);
    }

    fn bind_loop_target(&mut self, target: &ast::LoopTarget, t: &Ty, span: Span) {
        if *t == Ty::Void {
            self.err_help(
                span,
                "this returns nothing, so there is nothing to bind",
                "for a void method, loop on its status: `while (!@r.next(...))` (DESIGN §9.4)",
            );
            return;
        }
        match target {
            ast::LoopTarget::Single(lt, name) => {
                let (elem, by_ref) = match t {
                    Ty::Ref(x) => ((**x).clone(), true),
                    t => (t.clone(), false),
                };
                self.bind_local_type(lt, name, &elem, by_ref, span);
            }
            ast::LoopTarget::Tuple(items) => {
                let elems = match t {
                    Ty::Tuple(ts) if ts.len() == items.len() => ts.clone(),
                    Ty::Tuple(ts) => {
                        self.err(
                            span,
                            format!(
                                "this gives {}, and the loop names {}",
                                plural(ts.len(), "result"),
                                items.len()
                            ),
                        );
                        vec![Ty::Unknown; items.len()]
                    }
                    Ty::Unknown => vec![Ty::Unknown; items.len()],
                    t => {
                        self.err(span, format!("only a call with several results can be destructured, and this gives {}", self.show(t)));
                        vec![Ty::Unknown; items.len()]
                    }
                };
                for (item, e) in items.iter().zip(&elems) {
                    if let ast::LoopItem::Decl(lt, name) = item {
                        self.bind_local_type(lt, name, e, false, span);
                    }
                }
            }
        }
    }

    /// What a for-each over a value of type `it` yields, and whether it can be referred to.
    fn iter_elem(&mut self, it: &Ty, iter: &Expr) -> (Ty, bool) {
        let prog = self.prog;
        match it.unref() {
            Ty::Array(e, _) | Ty::View(e) => ((**e).clone(), true),
            Ty::Slice(e) => ((**e).clone(), false),
            Ty::Map(k, v) => (Ty::MapEntry(k.clone(), v.clone()), true),
            Ty::Set(e) => ((**e).clone(), false),
            Ty::Pool(e) => ((**e).clone(), true),
            Ty::Iter(e) => ((**e).clone(), false),
            Ty::Unknown => (Ty::Unknown, true),
            Ty::Class(id, args) => {
                let c = &prog.classes[*id];
                let map = self.class_subst(*id, args);
                let find = |name: &str| {
                    c.methods
                        .iter()
                        .find(|m| m.name == name && m.params.is_empty() && !m.is_static)
                };
                if let Some(next) = find("next") {
                    if let E::Ident(n) = &iter.kind {
                        self.err_help(
                            iter.span,
                            "a for-each over an iterator variable isn't allowed",
                            format!("use a repeat loop, which names it: `while (T x : {}.next())` (DESIGN §9.4)", n),
                        );
                    }
                    return match next.ret.subst(&map) {
                        Ty::Ref(x) => (*x, true),
                        r => (r, false),
                    };
                }
                if let Some(itm) = find("iterator") {
                    let r = itm.ret.subst(&map);
                    if let Ty::Class(iid, iargs) = r.unref() {
                        let imap = self.class_subst(*iid, iargs);
                        if let Some(next) = prog.classes[*iid]
                            .methods
                            .iter()
                            .find(|m| m.name == "next" && m.params.is_empty())
                        {
                            return match next.ret.subst(&imap) {
                                Ty::Ref(x) => (*x, true),
                                r => (r, false),
                            };
                        }
                    }
                    return (Ty::Unknown, true);
                }
                self.err(
                    iter.span,
                    format!(
                        "`{}` has no `next()` method, so a for-each can't loop over it",
                        c.name
                    ),
                );
                (Ty::Unknown, true)
            }
            Ty::Nullable(_) => {
                self.err_help(
                    iter.span,
                    "this may be null here",
                    "check `x != null` first, in a local variable (DESIGN §5.1)",
                );
                (Ty::Unknown, true)
            }
            t => {
                self.err(iter.span, format!("a for-each loops over an array, a slice, a hashmap, a hashset, a pool or an iterator, not {}", self.show(t)));
                (Ty::Unknown, true)
            }
        }
    }

    fn destructure(&mut self, targets: &[Target], capture: Option<&Name>, value: &Expr) {
        let vt = self.expr_c(value, None, capture.is_some());
        if let Some(c) = capture {
            if targets.iter().any(|t| matches!(t, Target::Decl(..))) {
                self.declare_name(c, Ty::u8());
            } else {
                self.status_dest(c);
            }
        }
        let n = targets.len();
        let elems = match vt.unref() {
            Ty::Tuple(ts) if ts.len() == n => ts.clone(),
            Ty::Tuple(ts) => {
                self.err(
                    value.span,
                    format!(
                        "this gives {}, and there are {} targets",
                        plural(ts.len(), "result"),
                        n
                    ),
                );
                vec![Ty::Unknown; n]
            }
            Ty::Unknown => vec![Ty::Unknown; n],
            t => {
                self.err(value.span, format!("only a call with several results (DESIGN §9.5) can be destructured, and this gives {}", self.show(t)));
                vec![Ty::Unknown; n]
            }
        };
        for (t, et) in targets.iter().zip(&elems) {
            match t {
                Target::Decl(lt, name) => self.bind_local_type(lt, name, et, false, value.span),
                Target::Existing(Name::Discard(_)) => {}
                Target::Existing(Name::Ident(id)) => {
                    let slot = self.name_slot(&id.name);
                    self.writing = true;
                    let lt = self.ident(&id.name, id.span);
                    self.writing = false;
                    self.write_name(&id.name, id.span);
                    if let Some(s) = slot {
                        self.flow.assign(s);
                    }
                    let to = lt.unref().clone();
                    if !matches!(lt, Ty::TypeName(_)) {
                        self.expect_at(et, &to, id.span, true);
                    }
                }
            }
        }
    }

    fn status_dest(&mut self, c: &Name) {
        let Name::Ident(id) = c else { return };
        if self.lookup_local(&id.name).is_some() || self.field_type_quiet(&id.name) {
            self.write_name(&id.name, id.span);
        }
        if let Some(s) = self.name_slot(&id.name) {
            self.flow.assign(s);
        }
        let t = match self.lookup_local(&id.name) {
            Some(l) => {
                let decl = key(self.locals[l].decl);
                self.facts.names.insert(key(id.span), NameRes::Local(decl));
                self.locals[l].ty.clone()
            }
            None => match self.field_type(&id.name, id.span) {
                Some(t) => t,
                None => {
                    self.err_help(
                        id.span,
                        format!("`{}` isn't declared", id.name),
                        "`@err = ...` assigns an existing u8; to declare one, write `T x @err = f();` or `u8 err = @f();` (DESIGN §7.3)",
                    );
                    return;
                }
            },
        };
        if !matches!(t.unref(), Ty::Prim(Prim::U8) | Ty::Unknown) {
            self.err(
                id.span,
                format!(
                    "a status goes into a u8, and `{}` is {}",
                    id.name,
                    self.show(&t)
                ),
            );
        }
    }

    /// Checks a condition: a bool, or an unsigned integer (DESIGN §9.4).
    fn cond(&mut self, e: &Expr) {
        if matches!(e.kind, E::Assign { .. }) {
            self.err_help(
                e.span,
                "an assignment can't be a condition",
                "to compare, write `==` (DESIGN §9.4)",
            );
        }
        let t = self.expr(e, Some(&Ty::bool()));
        match t.unref() {
            Ty::Prim(p) if *p == Prim::Bool || (p.is_int() && !signed(*p)) => {}
            Ty::Unknown => {}
            Ty::Param(n) if self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Uint)) => {}
            Ty::Prim(p) if p.is_int() => self.err_help(
                e.span,
                format!("a signed integer ({}) isn't a condition", p.name()),
                "write `n != 0` (DESIGN §9.4)",
            ),
            Ty::IntLit { .. } => self.err_help(
                e.span,
                "a number isn't a condition",
                "write `true`, or compare it (DESIGN §9.4)",
            ),
            Ty::Nullable(_) | Ty::Ref(_) => self.err_help(
                e.span,
                "a reference isn't a condition",
                "write `x != null` (DESIGN §5.1)",
            ),
            t => self.err(
                e.span,
                format!(
                    "a condition is a bool or an unsigned integer, and this is {}",
                    self.show(t)
                ),
            ),
        }
    }

    /// Locals known to be non-null when `c` is true, and when it is false.
    fn facts(&self, c: &Expr) -> (Vec<usize>, Vec<usize>) {
        match &c.kind {
            E::Paren(x) => self.facts(x),
            E::Unary {
                op: UnaryOp::Not,
                expr,
            } => {
                let (t, f) = self.facts(expr);
                (f, t)
            }
            E::Binary {
                op: op @ (BinOp::Ne | BinOp::Eq),
                lhs,
                rhs,
            } => {
                let id = match (&lhs.kind, &rhs.kind) {
                    (E::Ident(n), E::Null) | (E::Null, E::Ident(n)) => self.lookup_local(n),
                    _ => None,
                };
                match id {
                    Some(id) if matches!(self.locals[id].ty, Ty::Nullable(_)) => {
                        if *op == BinOp::Ne {
                            (vec![id], vec![])
                        } else {
                            (vec![], vec![id])
                        }
                    }
                    _ => (vec![], vec![]),
                }
            }
            E::Binary {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                let (mut t, _) = self.facts(lhs);
                t.extend(self.facts(rhs).0);
                (t, vec![])
            }
            E::Binary {
                op: BinOp::Or,
                lhs,
                rhs,
            } => {
                let (_, mut f) = self.facts(lhs);
                f.extend(self.facts(rhs).1);
                (vec![], f)
            }
            _ => (vec![], vec![]),
        }
    }

    fn switch(&mut self, sw: &ast::Switch, want: Option<&Ty>, is_expr: bool) -> Ty {
        enum K {
            Int(Option<Prim>),
            Str,
            Enum(ClassId),
            Any,
        }
        let st = self.expr(&sw.scrutinee, None).unref().clone();
        let kind = match &st {
            Ty::Prim(p) if p.is_int() => K::Int(Some(*p)),
            Ty::IntLit { .. } => K::Int(None),
            Ty::Class(id, _) if self.prog.classes[*id].kind == ClassKind::Enum => K::Enum(*id),
            Ty::Unknown => K::Any,
            Ty::Slice(e) | Ty::View(e) | Ty::Array(e, _) if **e == Ty::u8() => K::Str,
            t => {
                self.err(
                    sw.scrutinee.span,
                    format!(
                        "a switch works on integers, strings and enums (DESIGN §9.4), not {}",
                        self.show(t)
                    ),
                );
                K::Any
            }
        };
        let mut seen: Vec<String> = Vec::new();
        let mut has_default = false;
        let mut arm_ty: Option<Ty> = None;
        let start = self.flow.clone();
        let mut ends: Vec<Flow> = Vec::new();
        for rule in &sw.rules {
            self.flow = start.clone();
            match &rule.labels {
                None => {
                    if has_default {
                        self.err(rule.span, "this switch already has a `default`");
                    }
                    has_default = true;
                }
                Some(labels) => {
                    for l in labels {
                        let (key, span) = match l {
                            CaseLabel::Int {
                                negative,
                                value,
                                span,
                            } => (
                                format!("{}{}", if *negative { "-" } else { "" }, value),
                                *span,
                            ),
                            CaseLabel::Char(c, span) => ((*c).to_string(), *span),
                            CaseLabel::Str(s, span) => (format!("{:?}", s), *span),
                            CaseLabel::Ident(id) => (id.name.clone(), id.span),
                        };
                        if seen.contains(&key) {
                            self.err(span, "this case is already covered by an earlier label");
                        }
                        seen.push(key);
                        self.case_label(
                            l,
                            &kind_desc(&kind),
                            match &kind {
                                K::Int(p) => Some(Ty::Prim(p.unwrap_or(Prim::I64))),
                                _ => None,
                            },
                            matches!(kind, K::Str),
                            match kind {
                                K::Enum(id) => Some(id),
                                _ => None,
                            },
                            matches!(kind, K::Any),
                        );
                    }
                }
            }
            if is_expr {
                let (t, span) = match &rule.body {
                    SwitchBody::Expr(x) => {
                        let t = self.expr(x, want);
                        if never_returns(x) {
                            self.flow = Flow::unreachable();
                        }
                        (t, x.span)
                    }
                    SwitchBody::Block(b) => {
                        self.cur_mut().yields.push((want.cloned(), None));
                        self.yield_flows.push(Vec::new());
                        self.block(b);
                        let yields = self.yield_flows.pop().unwrap_or_default();
                        let fall = self.take_flow();
                        self.flow = Flow::join_all(yields.into_iter().chain(std::iter::once(fall)));
                        let y = self.cur_mut().yields.pop().and_then(|y| y.1);
                        if completes_all(&b.stmts) {
                            self.err(b.span, "this arm can reach its end without `yield`ing a value (DESIGN §9.4)");
                        }
                        (y.unwrap_or(Ty::Unknown), b.span)
                    }
                };
                if let Some(w) = want {
                    if let SwitchBody::Expr(x) = &rule.body {
                        self.expect(&t, w, x);
                    }
                    let _ = span;
                } else {
                    arm_ty = Some(match arm_ty {
                        None => t,
                        Some(prev) => self.unify(&prev, &t, span),
                    });
                }
            } else {
                match &rule.body {
                    SwitchBody::Expr(x) => self.expr_stmt(x),
                    SwitchBody::Block(b) => self.block(b),
                }
            }
            ends.push(self.take_flow());
        }
        let all_constants = matches!(kind, K::Enum(id) if self.prog.classes[id].constants.iter().all(|c| seen.contains(c)));
        // Without a `default`, a statement switch may run no arm at all.
        let every_path = is_expr || has_default || all_constants;
        self.flow = Flow::join_all(ends.into_iter().chain((!every_path).then_some(start)));
        if !is_expr {
            return Ty::Void;
        }
        let covered = has_default || all_constants || matches!(kind, K::Any);
        if !covered {
            self.err_help(
                sw.span,
                "a switch expression must cover every case",
                "add a `default` arm, or a case for every enum constant (DESIGN §9.4)",
            );
        }
        return want.cloned().or(arm_ty).unwrap_or(Ty::Unknown);

        fn kind_desc(k: &K) -> String {
            match k {
                K::Int(_) => "an integer".into(),
                K::Str => "a string".into(),
                K::Enum(_) => "an enum".into(),
                K::Any => String::new(),
            }
        }
    }

    fn case_label(
        &mut self,
        l: &CaseLabel,
        desc: &str,
        int: Option<Ty>,
        is_str: bool,
        en: Option<ClassId>,
        any: bool,
    ) {
        if any {
            return;
        }
        let mismatch = |me: &mut Self, span: Span, what: &str| {
            me.err(
                span,
                format!("this label is {}, and the switch is on {}", what, desc),
            )
        };
        match l {
            CaseLabel::Int {
                negative,
                value,
                span,
            } => match &int {
                Some(Ty::Prim(p)) => {
                    let v = i128::try_from(*value).unwrap_or(i128::MAX)
                        * if *negative { -1 } else { 1 };
                    if !fits(v, *p) {
                        self.err(
                            *span,
                            format!("{} doesn't fit in {}, the type of the switch", v, p.name()),
                        );
                    }
                }
                _ => mismatch(self, *span, "an integer"),
            },
            CaseLabel::Char(c, span) => match &int {
                Some(Ty::Prim(p)) => {
                    if Self::lit_rank(Some(*c as i128), true, *p).is_none() {
                        self.expect_at(
                            &Ty::IntLit {
                                value: Some(*c as i128),
                                is_char: true,
                            },
                            &Ty::Prim(*p),
                            *span,
                            false,
                        );
                    }
                }
                _ => mismatch(self, *span, "a character"),
            },
            CaseLabel::Str(_, span) => {
                if !is_str {
                    mismatch(self, *span, "a string");
                }
            }
            CaseLabel::Ident(id) => match en {
                Some(e) => {
                    let c = &self.prog.classes[e];
                    if !c.constants.contains(&id.name) {
                        let msg = format!("`{}` isn't a constant of `{}`", id.name, c.name);
                        self.err(id.span, msg);
                    }
                }
                None => {
                    let t = self.ident(&id.name, id.span);
                    if !int.as_ref().is_some_and(|i| self.rank(&t, i).is_some()) && !is_str {
                        mismatch(self, id.span, "a name");
                    }
                }
            },
        }
    }

    // ------------------------------------------------------------ expressions

    fn expr(&mut self, e: &Expr, want: Option<&Ty>) -> Ty {
        self.expr_c(e, want, false)
    }

    /// Types an expression. `consumed` says its status is read, so dropping it draws no warning.
    fn expr_c(&mut self, e: &Expr, want: Option<&Ty>, consumed: bool) -> Ty {
        let t = self.expr_kind(e, want, consumed);
        self.facts.types.insert(key(e.span), t.clone());
        t
    }

    fn expr_kind(&mut self, e: &Expr, want: Option<&Ty>, consumed: bool) -> Ty {
        match &e.kind {
            E::Int { value, suffix } => match suffix {
                Some(p) => {
                    if p.is_int() && !fits(*value as i128, *p) {
                        self.err(e.span, format!("{} doesn't fit in {}", value, p.name()));
                    }
                    Ty::Prim(*p)
                }
                None => Ty::IntLit {
                    value: i128::try_from(*value).ok(),
                    is_char: false,
                },
            },
            E::Float { suffix, .. } => suffix.map_or(Ty::FloatLit, Ty::Prim),
            E::Char(c) => Ty::IntLit {
                value: Some(*c as i128),
                is_char: true,
            },
            E::Str(parts) => {
                for p in parts {
                    if let StrPart::Interp(x) = p {
                        self.interp(x);
                    }
                }
                if let Some(Ty::Array(_, k)) = want.map(|w| w.unref())
                    && k.cap == CapKind::Capped
                    && parts.iter().all(|p| matches!(p, StrPart::Text(_)))
                {
                    let n: usize = parts
                        .iter()
                        .map(|p| match p {
                            StrPart::Text(t) => t.len(),
                            StrPart::Interp(_) => 0,
                        })
                        .sum();
                    self.over_cap(n, k.size, "bytes", e.span);
                }
                Ty::string()
            }
            E::Bool(_) => Ty::bool(),
            E::Null => Ty::Null,
            E::Ident(n) => self.ident(n, e.span),
            E::Discard => {
                self.err(e.span, "`_` can't be read; it only discards (DESIGN §7.3)");
                Ty::Unknown
            }
            E::This => {
                self.this_use(e.span, "`this` can't be used");
                self.this_ty(e.span)
            }
            E::ThisNew(args) => self.this_new(args, e.span, consumed),
            E::PrimMember { prim, name } => self.prim_const(*prim, name),
            E::New { class, args } => self.new_expr(class.as_ref(), args, want, e.span, consumed),
            E::ArrayLit(items) => self.array_lit(items, want, e.span),
            E::Switch(sw) => self.switch(sw, want, true),
            E::Paren(x) => self.expr_c(x, want, consumed),
            E::Tuple(xs) => {
                let wants = match want {
                    Some(Ty::Tuple(ts)) if ts.len() == xs.len() => Some(ts.clone()),
                    _ => None,
                };
                let mut out = Vec::new();
                for (i, x) in xs.iter().enumerate() {
                    let w = wants.as_ref().map(|w| &w[i]);
                    let t = self.expr(x, w);
                    if let Some(w) = w {
                        self.expect(&t, w, x);
                    }
                    out.push(t);
                }
                Ty::Tuple(out)
            }
            E::Lambda { params, body } => self.lambda(params, body, e.span),
            E::Assign {
                op,
                target,
                capture,
                value,
            } => self.assign(*op, target, capture.as_ref(), value, e.span),
            E::StatusAssign { capture, value } => {
                self.status_dest(capture);
                self.expr_c(value, None, true);
                Ty::Void
            }
            E::Cond { cond, then, els } => {
                self.cond(cond);
                let after_cond = self.flow.clone();
                let (t, f) = self.facts(cond);
                let saved = self.narrowed.clone();
                self.narrowed.extend(t);
                let a = self.expr_c(then, want, consumed);
                let then_flow = std::mem::replace(&mut self.flow, after_cond);
                self.narrowed = saved.clone();
                self.narrowed.extend(f);
                let b = self.expr_c(els, want, consumed);
                let else_flow = self.take_flow();
                self.flow = Flow::join(then_flow, else_flow);
                self.narrowed = saved;
                if let Some(w) = want
                    && self.rank(&a, w).is_some()
                    && self.rank(&b, w).is_some()
                {
                    return w.clone();
                }
                self.unify(&a, &b, e.span)
            }
            E::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span, consumed),
            E::InstanceOf { expr, ty, bind } => {
                let t = self.expr(expr, None);
                let iface = match &t {
                    Ty::Ref(i) | Ty::Nullable(i) => match i.unref() {
                        Ty::Class(id, _) if self.is_interface(*id) => Some(i.unref().clone()),
                        _ => None,
                    },
                    _ => None,
                };
                let target = self.resolve(ty);
                match (&iface, &target) {
                    (Some(i), Ty::Class(c, _)) => {
                        if !self.implements(&target, i) {
                            let msg = format!(
                                "`{}` doesn't implement `{}`, so this is never true",
                                self.prog.classes[*c].name,
                                self.show(i)
                            );
                            self.err(ty.span, msg);
                        }
                    }
                    (None, _) if !t.is_unknown() => {
                        self.err_help(expr.span, "`instanceof` tests an interface reference", "there are no subclasses; only an interface reference (`Shape&`) can refer to different classes (DESIGN §6.3)");
                    }
                    _ => {}
                }
                if let Some(b) = bind {
                    self.declare(b, Ty::Ref(Box::new(target)));
                }
                Ty::bool()
            }
            E::Unary { op, expr } => self.unary(*op, expr, want),
            E::StatusRead(x) => {
                if !is_status_source(x) {
                    self.err_help(x.span, "`@` reads the status of a call or an array operator", "for an operator expression, put it in parentheses: `@(a +> x)` (DESIGN §7.3)");
                }
                self.expr_c(x, None, true);
                Ty::u8()
            }
            E::Cast { ty, mode, expr } => self.cast(ty, *mode, expr, e.span),
            E::Postfix { op, expr } => self.postfix(*op, expr, e.span, consumed),
            E::Member { expr, name } => {
                // In a constructor, `this.x` reads one field, which needs only that field assigned.
                if matches!(expr.kind, E::This) && self.in_ctor() {
                    let bt = self.this_ty(expr.span);
                    let writing = std::mem::take(&mut self.writing);
                    if let Some(slot) = self.ctor_field(&name.name)
                        && !writing
                    {
                        self.read_slot(slot, &name.name, name.span, true);
                    }
                    return self.member_of(&bt, expr, name);
                }
                let bt = self.expr(expr, None);
                self.member_of(&bt, expr, name)
            }
            E::Call { callee, args } => self.call(callee, args, e.span, consumed),
            E::Index { expr, mode, index } => {
                let bt = self.expr(expr, None);
                let it = self.expr(index, None);
                self.index_type(&it, index);
                if *mode == ast::IndexMode::FromEnd
                    && matches!(it, Ty::IntLit { value: Some(0), .. })
                {
                    self.err_help(
                        index.span,
                        "`a[^0]` is past the end",
                        "`a[^1]` is the last element (DESIGN §4.10)",
                    );
                }
                match bt.unref() {
                    Ty::Array(el, _) | Ty::Slice(el) | Ty::View(el) => (**el).clone(),
                    Ty::Unknown => Ty::Unknown,
                    Ty::Map(..) => {
                        self.err_help(expr.span, "a hashmap isn't indexed with `[]`", "use `m.get(k)`, or `m.getOrAdd(k, initial)` for a reference (DESIGN §4.14)");
                        Ty::Unknown
                    }
                    Ty::Nullable(_) => {
                        self.err_help(
                            expr.span,
                            "this may be null here",
                            "check `x != null` first, in a local variable (DESIGN §5.1)",
                        );
                        Ty::Unknown
                    }
                    t => {
                        self.err(
                            expr.span,
                            format!(
                                "only arrays and slices are indexed, and this is {}",
                                self.show(t)
                            ),
                        );
                        Ty::Unknown
                    }
                }
            }
            E::Slice { expr, lo, hi } => {
                let bt = self.expr(expr, None);
                for b in [lo, hi].into_iter().flatten() {
                    let t = self.expr(&b.expr, None);
                    self.index_type(&t, &b.expr);
                }
                match bt.unref() {
                    Ty::Array(el, k) => {
                        if !k.sliceable() {
                            self.err_help(expr.span, format!("`{}` doesn't keep its elements in order in one block, so it can't be sliced", self.show(&bt)), "slices of cursor, chunked and reversed arrays on data stacks are undecided (DESIGN §17); capped arrays, ring and gap buffers included, can be sliced");
                        }
                        Ty::View(el.clone())
                    }
                    Ty::View(el) => Ty::View(el.clone()),
                    Ty::Slice(el) => Ty::Slice(el.clone()),
                    Ty::Unknown => Ty::Unknown,
                    t => {
                        self.err(
                            expr.span,
                            format!(
                                "only arrays and slices can be sliced, and this is {}",
                                self.show(t)
                            ),
                        );
                        Ty::Unknown
                    }
                }
            }
        }
    }

    fn index_type(&mut self, t: &Ty, e: &Expr) {
        let ok = match t.unref() {
            Ty::Prim(p) => p.is_int(),
            Ty::IntLit { .. } | Ty::Unknown => true,
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Int)),
            _ => false,
        };
        if !ok {
            self.err(
                e.span,
                format!("an index is an integer, and this is {}", self.show(t)),
            );
        }
    }

    /// A value inside `${...}` (DESIGN §4.11, §4.13).
    fn interp(&mut self, x: &Expr) {
        let t = self.expr_c(x, None, true);
        match t.unref() {
            Ty::Class(..) => self.err_help(
                x.span,
                "a class value can't be printed until default formatting is decided (DESIGN §17)",
                "print its fields instead",
            ),
            Ty::Void => self.err(x.span, "this returns nothing, so there is nothing to print"),
            Ty::Nullable(_) => self.err_help(
                x.span,
                "this may be null here",
                "check `x != null` first, in a local variable (DESIGN §5.1)",
            ),
            Ty::Tuple(_)
            | Ty::Map(..)
            | Ty::Set(_)
            | Ty::Pool(_)
            | Ty::Func(..)
            | Ty::Iter(_)
            | Ty::MapEntry(..)
            | Ty::Handle(_) => self.err(x.span, format!("{} can't be printed", self.show(&t))),
            Ty::TypeName(_) | Ty::Namespace(_) => self.err(
                x.span,
                format!("`{}` is a type, not a value", self.show(&t)),
            ),
            _ => {}
        }
    }

    fn ident(&mut self, name: &str, span: Span) -> Ty {
        let writing = std::mem::take(&mut self.writing);
        if let Some(id) = self.lookup_local(name) {
            self.facts
                .names
                .insert(key(span), NameRes::Local(key(self.locals[id].decl)));
            if self.locals[id].depth < self.fns.len() && !writing {
                self.note_capture(id, span);
            }
            if let Some(slot) = self.locals[id].slot
                && !writing
            {
                self.read_slot(slot, name, span, false);
            }
            let l = &self.locals[id];
            if let Ty::Nullable(inner) = &l.ty
                && self.narrowed.contains(&id)
            {
                return (**inner).clone();
            }
            return l.ty.clone();
        }
        if let Some(t) = self.field_type(name, span) {
            if let Some(slot) = self.ctor_field(name)
                && !writing
            {
                self.read_slot(slot, name, span, true);
            }
            return t;
        }
        if self.tparam(name).is_some() {
            return Ty::TypeName(Box::new(Ty::Param(name.to_string())));
        }
        match self.prog.find_class(self.file, name) {
            Lookup::Found(id) => return Ty::TypeName(Box::new(Ty::Class(id, Vec::new()))),
            Lookup::Unmodelled if starts_upper(name) => return Ty::Unknown,
            _ => {}
        }
        match name {
            "status" => return Ty::Namespace("status"),
            "math" => return Ty::Namespace("math"),
            // `File` handles; `zink.io` isn't modelled yet.
            "stdin" | "stdout" | "stderr" | "env" => return Ty::Unknown,
            _ => {}
        }
        if self.unmodelled() && builtins::LIBRARY_NAMESPACES.contains(&name) {
            return Ty::Unknown;
        }
        if self.function_exists(name) {
            self.err_help(
                span,
                format!("`{}` is a function; call it with `()`", name),
                "method references are undecided (DESIGN §9.3)",
            );
        } else {
            self.err(span, format!("unknown name `{}`", name));
        }
        Ty::Unknown
    }

    fn function_exists(&self, name: &str) -> bool {
        let prog = self.prog;
        self.class
            .is_some_and(|c| prog.classes[c].methods.iter().any(|m| m.name == name))
            || prog.files[self.file]
                .script
                .is_some_and(|s| prog.classes[s].methods.iter().any(|m| m.name == name))
            || !builtins::global(name).is_empty()
    }

    /// A field reached by its bare name: of the current class, or a script file's static.
    fn field_type(&mut self, name: &str, span: Span) -> Option<Ty> {
        let prog = self.prog;
        if let Some(cls) = self.class
            && let Some(i) = prog.classes[cls].fields.iter().position(|f| f.name == name)
        {
            let f = &prog.classes[cls].fields[i];
            self.facts.names.insert(key(span), NameRes::Field(cls, i));
            if !f.is_static {
                self.note_this_capture();
            }
            if !f.is_static && self.fn_static() {
                let fname = self.fns.last().map(|f| f.name.clone()).unwrap_or_default();
                self.err(
                    span,
                    format!(
                        "`{}` is a field of each value, and `{}` is static",
                        name, fname
                    ),
                );
            }
            return Some(f.ty.clone());
        }
        if let Some(script) = prog.files[self.file].script
            && let Some(i) = prog.classes[script]
                .fields
                .iter()
                .position(|f| f.name == name)
        {
            self.facts
                .names
                .insert(key(span), NameRes::Field(script, i));
            return Some(prog.classes[script].fields[i].ty.clone());
        }
        None
    }

    /// A root's value: a record of where each part lives, which can't be copied (DESIGN §6.6).
    fn is_root_value(&self, t: &Ty) -> bool {
        matches!(t, Ty::Class(id, _) if self.prog.classes[*id].kind == ClassKind::Root)
    }

    fn root_copy(&mut self, span: Span, t: &Ty) {
        let name = self.show(t);
        self.err_help(
            span,
            format!("`{}` is a root, which can't be copied", name),
            format!("refer to it instead: `{}& r = ...;` (DESIGN §6.6)", name),
        );
    }

    fn root_by_value(&mut self, span: Span, t: &Ty, how: &str) {
        let name = self.show(t);
        self.err_help(
            span,
            format!("`{}` is a root, so it can't be {} by value", name, how),
            format!("use a reference, `{}&` (DESIGN §6.6)", name),
        );
    }

    /// A lambda uses `this`: every enclosing lambda captures it.
    fn note_this_capture(&mut self) {
        for l in &mut self.lambdas {
            l.1 = true;
        }
    }

    fn this_ty(&mut self, span: Span) -> Ty {
        self.note_this_capture();
        match self.class {
            Some(c) if !self.fn_static() && self.prog.classes[c].kind != ClassKind::Script => {
                Ty::Ref(Box::new(self.self_type(c)))
            }
            _ => {
                self.err(
                    span,
                    "`this` is only in constructors and methods that aren't static",
                );
                Ty::Unknown
            }
        }
    }

    fn prim_const(&mut self, p: Prim, name: &ast::Ident) -> Ty {
        if builtins::number_const(NumCaps::of_prim(p), &name.name) {
            return Ty::Prim(p);
        }
        if !builtins::number_static(&Ty::Prim(p), NumCaps::of_prim(p), &name.name).is_empty() {
            self.err(
                name.span,
                format!("`{}.{}` is a function; call it", p.name(), name.name),
            );
        } else {
            self.err(
                name.span,
                format!("`{}` has no constant `{}`", p.name(), name.name),
            );
        }
        Ty::Unknown
    }

    fn member_of(&mut self, bt: &Ty, base: &Expr, name: &ast::Ident) -> Ty {
        let prog = self.prog;
        let n = name.name.as_str();
        match bt.unref() {
            Ty::Unknown => Ty::Unknown,
            Ty::Nullable(_) => {
                self.err_help(base.span, "this may be null here", "check `x != null` first, in a local variable; narrowing doesn't apply to fields (DESIGN §5.1)");
                Ty::Unknown
            }
            Ty::TypeName(t) => match &**t {
                Ty::Class(id, _) => {
                    let c = &prog.classes[*id];
                    if c.kind == ClassKind::Enum {
                        if c.constants.iter().any(|k| k == n) {
                            return Ty::Class(*id, Vec::new());
                        }
                        self.err(name.span, format!("`{}` has no constant `{}`", c.name, n));
                        return Ty::Unknown;
                    }
                    match c.fields.iter().find(|f| f.name == n) {
                        Some(f) if f.is_static => {
                            self.check_private_field(*id, f, name.span);
                            f.ty.clone()
                        }
                        Some(_) => {
                            self.err(name.span, format!("`{}` is a field of each `{}`, so it needs a value, not the class", n, c.name));
                            Ty::Unknown
                        }
                        None if c.methods.iter().any(|m| m.name == n) => {
                            self.err(
                                name.span,
                                format!("`{}.{}` is a function; call it", c.name, n),
                            );
                            Ty::Unknown
                        }
                        None => {
                            self.err(
                                name.span,
                                format!("`{}` has no static field `{}`", c.name, n),
                            );
                            Ty::Unknown
                        }
                    }
                }
                Ty::Param(p) => {
                    let caps = self.tparam(p).map(|tp| NumCaps::of_param(&tp));
                    if caps.is_some_and(|c| builtins::number_const(c, n)) {
                        return Ty::Param(p.clone());
                    }
                    self.err(
                        name.span,
                        format!(
                            "`{}` has no constant `{}` under its bounds (DESIGN §10.1)",
                            p, n
                        ),
                    );
                    Ty::Unknown
                }
                _ => Ty::Unknown,
            },
            Ty::Namespace("status") => {
                if is_status_const(n) {
                    return Ty::u8();
                }
                if !builtins::status_fn(n).is_empty() {
                    self.err(name.span, format!("`status.{}` is a function; call it", n));
                } else {
                    self.err(name.span, format!("`status.{}` isn't a status code", n));
                }
                Ty::Unknown
            }
            Ty::Namespace("math") => match builtins::math_const(n) {
                Some(t) => t,
                None => {
                    self.err(name.span, format!("`math` has no constant `{}`", n));
                    Ty::Unknown
                }
            },
            Ty::Class(id, args) => {
                let c = &prog.classes[*id];
                let map = self.class_subst(*id, args);
                match c.fields.iter().find(|f| f.name == n) {
                    Some(f) => {
                        self.check_private_field(*id, f, name.span);
                        f.ty.subst(&map)
                    }
                    None if c.methods.iter().any(|m| m.name == n) => {
                        self.err_help(
                            name.span,
                            format!("`{}` is a method of `{}`", n, c.name),
                            format!("call it: `{}()`", n),
                        );
                        Ty::Unknown
                    }
                    None => {
                        self.err(
                            name.span,
                            format!("`{}` has no field `{}`", self.show(bt.unref()), n),
                        );
                        Ty::Unknown
                    }
                }
            }
            Ty::Array(_, k) if n == "cursor" => {
                if !k.has_cursor() {
                    self.err(name.span, format!("`cursor` needs a cursor (a middle slot other than `.`), and `{}` has none", self.show(bt.unref())));
                }
                Ty::i64()
            }
            Ty::Array(..) | Ty::Slice(_) | Ty::View(_) | Ty::Map(..) | Ty::Set(_) | Ty::Pool(_)
                if n == "len" =>
            {
                Ty::i64()
            }
            Ty::MapEntry(k, v) if n == "key" || n == "value" => {
                if n == "key" {
                    (**k).clone()
                } else {
                    (**v).clone()
                }
            }
            t => {
                let has_method = !self.methods_of(t, n).is_empty();
                if has_method {
                    self.err_help(
                        name.span,
                        format!("`{}` is a method", n),
                        format!("call it: `{}()`", n),
                    );
                } else {
                    self.err(name.span, format!("{} has no field `{}`", self.show(t), n));
                }
                Ty::Unknown
            }
        }
    }

    fn check_private_field(&mut self, owner: ClassId, f: &FieldInfo, span: Span) {
        if f.is_private && self.class != Some(owner) {
            let msg = format!(
                "`{}` is private to `{}`",
                f.name, self.prog.classes[owner].name
            );
            self.err(span, msg);
        }
    }

    /// Built-in methods of a value, for messages.
    fn methods_of(&self, t: &Ty, n: &str) -> Vec<Sig> {
        match t {
            Ty::Prim(p) => builtins::number_method(t, NumCaps::of_prim(*p), n),
            Ty::Array(e, k) => builtins::seq_method(e, SeqKind::Array(*k), n).unwrap_or_default(),
            Ty::Slice(e) => builtins::seq_method(e, SeqKind::Slice, n).unwrap_or_default(),
            Ty::View(e) => builtins::seq_method(e, SeqKind::View, n).unwrap_or_default(),
            Ty::Map(k, v) => builtins::map_method(k, v, n),
            Ty::Set(e) => builtins::set_method(e, n),
            Ty::Pool(e) => builtins::pool_method(e, n),
            _ => Vec::new(),
        }
    }

    fn lambda(&mut self, params: &[ast::Param], body: &LambdaBody, lambda_span: Span) -> Ty {
        let ptys: Vec<Ty> = params.iter().map(|p| self.resolve(&p.ty)).collect();
        let tps = self.tparams();
        let is_static = self.fn_static();
        // The body sees what is assigned where the lambda is made; its own assignments,
        // returns and loops don't change the state around it.
        let outer_flow = self.flow.clone();
        let outer_loops = std::mem::take(&mut self.flow_loops);
        let outer_yields = std::mem::take(&mut self.yield_flows);
        self.begin(FnKind::Lambda, "lambda", Ty::Unknown, is_static, tps);
        self.lambdas.push((self.fns.len() - 1, false));
        for (p, t) in params.iter().zip(&ptys) {
            self.declare_param(&p.name, t.clone());
        }
        let ret = match body {
            LambdaBody::Expr(x) => {
                let t = self.expr(x, None);
                match t {
                    Ty::Tuple(ts) => {
                        Ty::Tuple(ts.iter().map(|t| t.unref().default_lit()).collect())
                    }
                    Ty::Void => Ty::Void,
                    t => self.var_type(&t, x.span),
                }
            }
            LambdaBody::Block(b) => {
                for s in &b.stmts {
                    self.stmt(s);
                }
                let r = self.cur().lambda_ret.clone().unwrap_or(Ty::Void);
                if r != Ty::Void && completes_all(&b.stmts) {
                    self.err(
                        b.span,
                        "this lambda can reach its end without returning a value",
                    );
                }
                r
            }
        };
        self.end();
        if let Some((_, true)) = self.lambdas.pop() {
            self.facts.capturing.insert(key(lambda_span));
        }
        self.flow = outer_flow;
        self.flow_loops = outer_loops;
        self.yield_flows = outer_yields;
        Ty::Func(ptys, Box::new(ret))
    }

    fn assign(
        &mut self,
        op: AssignOp,
        target: &Expr,
        capture: Option<&Name>,
        value: &Expr,
        span: Span,
    ) -> Ty {
        let consumed = capture.is_some();
        if let Some(c) = capture {
            self.status_dest(c);
        }
        if matches!(target.kind, E::Discard) {
            if op != AssignOp::Assign {
                self.err(
                    target.span,
                    "`_` can't be read, so it can't take a compound assignment (DESIGN §7.3)",
                );
            }
            self.expr_c(value, None, consumed);
            return Ty::Void;
        }
        let slot = if matches!(op, AssignOp::Assign | AssignOp::Rebind) {
            self.target_slot(target)
        } else {
            None
        };
        self.writing = slot.is_some();
        let tt = self.expr(target, None);
        self.writing = false;
        if let E::Ident(n) = &target.kind
            && self.lookup_local(n).is_some()
        {
            self.note_local_write(n, target.span);
        }
        if !self.is_place(target) && !matches!(tt, Ty::Ref(_) | Ty::Unknown) {
            self.err(
                target.span,
                "only a variable, a field or an element can be assigned",
            );
        }
        if op == AssignOp::Assign && self.is_root_value(tt.unref()) {
            let t = tt.unref().clone();
            self.root_copy(value.span, &t);
        }
        if let E::Member { expr, name } = &target.kind
            && name.name == "key"
            && let Some(Ty::MapEntry(..)) = self.peek_type(expr)
        {
            self.err(name.span, "a hashmap entry's key can't be changed");
        }
        match op {
            AssignOp::Rebind => {
                // Rebinding compares with the declared type, not a narrowed one.
                let tt = match &target.kind {
                    E::Ident(n) => self
                        .lookup_local(n)
                        .map_or(tt, |id| self.locals[id].ty.clone()),
                    _ => tt,
                };
                let ok = matches!(&tt, Ty::Ref(_) | Ty::View(_) | Ty::Unknown)
                    || matches!(&tt, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)));
                if !ok {
                    self.err_help(
                        span,
                        "`.=` rebinds a reference, and this is a value",
                        "use `=` to assign it (DESIGN §5.1)",
                    );
                    self.expr_c(value, None, consumed);
                    return Ty::Void;
                }
                let vt = self.expr_c(value, Some(&tt), consumed);
                if self.expect(&vt, &tt, value) {
                    self.check_rebind(target, value);
                }
                if let E::Ident(n) = &target.kind
                    && let Some(id) = self.lookup_local(n)
                {
                    self.narrowed.remove(&id);
                }
                if let Some(s) = slot {
                    self.flow.assign(s);
                }
                tt
            }
            AssignOp::Assign => {
                // The first assignment to a reference binds it (DESIGN §6.2).
                let is_ref = matches!(&tt, Ty::Ref(_) | Ty::View(_))
                    || matches!(&tt, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)));
                if let Some(s) = slot
                    && is_ref
                    && !self.flow.is_def(s)
                {
                    if self.flow.is_maybe(s) {
                        self.err_help(
                            target.span,
                            "this reference is assigned on some paths to here and not on others, so `=` could bind it or write through it",
                            "assign it on every path first, or rebind it with `.=` (DESIGN §5.1)",
                        );
                    }
                    let vt = self.expr_c(value, Some(&tt), consumed);
                    if self.expect(&vt, &tt, value) {
                        self.check_rebind(target, value);
                    }
                    self.flow.assign(s);
                    self.facts.binds.insert(key(span));
                    return tt;
                }
                self.need_writable(target, "can't assign to this");
                let to = match &tt {
                    Ty::Ref(x) => (**x).clone(),
                    Ty::View(e) => Ty::Slice(e.clone()),
                    Ty::Nullable(x) if matches!(**x, Ty::Ref(_)) => {
                        self.err_help(target.span, "this may be null here, and `=` writes through it", "to rebind it, write `.=`; to write the value it refers to, check `x != null` first (DESIGN §5.1)");
                        self.expr_c(value, None, consumed);
                        return Ty::Unknown;
                    }
                    t => t.clone(),
                };
                let vt = self.expr_c(value, Some(&to), consumed);
                self.expect(&vt, &to, value);
                if let Some(s) = slot {
                    self.flow.assign(s);
                }
                to
            }
            _ => {
                self.need_writable(target, "can't assign to this");
                let to = tt.unref().clone();
                let vt = self.expr_c(value, Some(&to), consumed);
                let bop = compound_op(op);
                let int_only = matches!(
                    op,
                    AssignOp::Rem
                        | AssignOp::WrapAdd
                        | AssignOp::WrapSub
                        | AssignOp::WrapMul
                        | AssignOp::SatAdd
                        | AssignOp::SatSub
                        | AssignOp::SatMul
                );
                let bits = matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Xor);
                let shift = matches!(op, AssignOp::Shl | AssignOp::Shr);
                match to.unref() {
                    Ty::Unknown => {}
                    Ty::Prim(Prim::Bool) if bits => {
                        self.expect(&vt, &to, value);
                    }
                    Ty::Prim(p) if p.is_int() || (p.is_float() && !int_only && !bits && !shift) => {
                        if shift {
                            self.index_type(&vt, value);
                        } else if self.rank(&vt, &to).is_none() {
                            let help = format!("`x {}= y` needs `y` to convert to `x`'s type implicitly; cast it: `({}) y` (DESIGN §9.2)", bop, p.name());
                            self.err_help(value.span, format!("{} doesn't convert to {} implicitly", self.show(&vt), p.name()), help);
                        } else if let (Ty::IntLit { value: Some(0), .. }, AssignOp::Div | AssignOp::Rem) = (&vt, op) {
                            self.err(value.span, "this divides by zero, which stops the program");
                        }
                    }
                    Ty::Param(n) => {
                        let need = if int_only || bits || shift { BoundKind::Int } else { BoundKind::Number };
                        if !self.tparam(n).is_some_and(|tp| tp.has(need)) {
                            self.err(span, format!("`{}=` needs `{}` to be `{}` (DESIGN §10.1)", bop, n, need.name()));
                        } else if !shift {
                            self.expect(&vt, &to, value);
                        }
                    }
                    t if t.is_string() && op == AssignOp::Add => {
                        self.err_help(span, "there's no `+=` on strings", "append to a `u8[..>]` buffer with `b.append(...)`, or build the text with interpolation (DESIGN §4.11)")
                    }
                    t => self.err(span, format!("`{}=` needs a number, and this is {}", bop, self.show(t))),
                }
                to
            }
        }
    }

    /// The type of an expression without reporting anything, for simple cases.
    fn peek_type(&self, e: &Expr) -> Option<Ty> {
        match &e.kind {
            E::Ident(n) => self
                .lookup_local(n)
                .map(|id| self.locals[id].ty.unref().clone()),
            _ => None,
        }
    }

    fn unary(&mut self, op: UnaryOp, x: &Expr, want: Option<&Ty>) -> Ty {
        match op {
            UnaryOp::Not => {
                self.cond(x);
                Ty::bool()
            }
            UnaryOp::Plus | UnaryOp::Neg => {
                let t = self.expr(x, want);
                match t.unref() {
                    Ty::IntLit { value, is_char } => Ty::IntLit {
                        value: if op == UnaryOp::Neg {
                            value.map(|v| -v)
                        } else {
                            *value
                        },
                        is_char: *is_char,
                    },
                    Ty::FloatLit | Ty::Unknown => t.unref().clone(),
                    Ty::Prim(p) if p.is_int() || p.is_float() => Ty::Prim(*p),
                    Ty::Param(n) if self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Number)) => {
                        t.unref().clone()
                    }
                    t => {
                        self.err(
                            x.span,
                            format!("`-` needs a number, and this is {}", self.show(t)),
                        );
                        Ty::Unknown
                    }
                }
            }
            UnaryOp::BitNot => {
                let t = self.expr(x, want);
                if !self.is_int_like(&t) {
                    self.err(
                        x.span,
                        format!("`~` needs an integer, and this is {}", self.show(&t)),
                    );
                }
                t.unref().clone()
            }
            UnaryOp::PreInc | UnaryOp::PreDec => self.step(x),
        }
    }

    /// `x++`, `x--`, `++x`, `--x`.
    fn step(&mut self, x: &Expr) -> Ty {
        let t = self.expr(x, None);
        self.need_writable(x, "can't change this");
        if let E::Ident(n) = &x.kind
            && self.lookup_local(n).is_some()
        {
            self.note_local_write(n, x.span);
        }
        if !self.is_place(x) && !matches!(t, Ty::Ref(_) | Ty::Unknown) {
            self.err(
                x.span,
                "only a variable, a field or an element can be incremented",
            );
        }
        let ok = match t.unref() {
            Ty::Prim(p) => p.is_int() || p.is_float(),
            Ty::Unknown => true,
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Number)),
            _ => false,
        };
        if !ok {
            self.err(
                x.span,
                format!("`++` and `--` need a number, and this is {}", self.show(&t)),
            );
        }
        t.unref().clone()
    }

    fn is_int_like(&self, t: &Ty) -> bool {
        match t.unref() {
            Ty::Prim(p) => p.is_int(),
            Ty::IntLit { .. } | Ty::Unknown => true,
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Int)),
            _ => false,
        }
    }

    fn postfix(&mut self, op: PostfixOp, x: &Expr, span: Span, consumed: bool) -> Ty {
        let (sym, method) = match op {
            PostfixOp::Inc | PostfixOp::Dec => return self.step(x),
            PostfixOp::RemoveEnd => ("->", "pop"),
            PostfixOp::RemoveFront => ("-<", "popFront"),
            PostfixOp::RemoveBefore => ("-^", "removeBefore"),
            PostfixOp::RemoveAfter => ("^-", "removeAfter"),
        };
        let t = self.expr(x, None);
        if matches!(t.unref(), Ty::Array(..)) {
            self.need_writable(
                x,
                &format!("can't use `{}` here, since it removes from the array", sym),
            );
        }
        match t.unref() {
            Ty::Array(e, k) => {
                if let Err(need) = builtins::seq_method(e, SeqKind::Array(*k), method) {
                    self.err(
                        span,
                        format!(
                            "`{}` needs {}, and `{}` can't",
                            sym,
                            need,
                            self.show(t.unref())
                        ),
                    );
                }
                self.note_status(Some(true), consumed, span, sym);
                (**e).clone()
            }
            Ty::Unknown => {
                self.last_can_fail = None;
                Ty::Unknown
            }
            Ty::Slice(_) | Ty::View(_) => {
                self.err(
                    span,
                    format!(
                        "`{}` removes from an array, and a slice can't change its length",
                        sym
                    ),
                );
                Ty::Unknown
            }
            t => {
                self.err(
                    span,
                    format!(
                        "`{}` removes from an array, and this is {}",
                        sym,
                        self.show(t)
                    ),
                );
                Ty::Unknown
            }
        }
    }

    fn cast(&mut self, ty: &ast::CastTarget, mode: ast::CastMode, x: &Expr, span: Span) -> Ty {
        let src = self.expr(x, None);
        let src = src.unref().clone();
        let src_ok = match &src {
            Ty::Prim(p) => p.is_int() || p.is_float(),
            Ty::IntLit { .. } | Ty::FloatLit | Ty::Unknown => true,
            Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Number)),
            _ => false,
        };
        if !src_ok {
            let help = if matches!(src, Ty::Class(..)) {
                "for interface references, use `instanceof` (DESIGN §6.3)"
            } else {
                "for a bool, write `b ? 1 : 0`"
            };
            self.err_help(
                x.span,
                format!(
                    "casts exist only for numbers, and this is {}",
                    self.show(&src)
                ),
                help,
            );
        }
        let src_float = matches!(&src, Ty::FloatLit) || matches!(&src, Ty::Prim(p) if p.is_float());

        match ty {
            ast::CastTarget::Prim(p) => {
                if *p == Prim::Bool {
                    self.err_help(
                        span,
                        "there's no cast to bool",
                        "write `n != 0` (DESIGN §9.1)",
                    );
                    return Ty::bool();
                }
                if p.is_float() && mode != ast::CastMode::Checked {
                    self.err_help(span, "`%` and `|` don't apply to a float target", "converting to a float never stops the program, so write a plain cast (DESIGN §9.1)");
                }
                if p.is_int() && src_float && mode == ast::CastMode::Wrap {
                    self.err_help(
                        span,
                        "a float can't wrap to an integer",
                        "use `|` to saturate, or a checked cast (DESIGN §9.1)",
                    );
                }
                if let (Ty::IntLit { value: Some(v), .. }, ast::CastMode::Checked) = (&src, mode)
                    && p.is_int()
                    && !fits(*v, *p)
                {
                    self.err_help(
                        span,
                        format!("{} doesn't fit in {}", v, p.name()),
                        format!(
                            "`({}%) {}` wraps, and `({}|) {}` saturates (DESIGN §9.1)",
                            p.name(),
                            v,
                            p.name(),
                            v
                        ),
                    );
                }
                Ty::Prim(*p)
            }
            ast::CastTarget::Param(id) => match self.tparam(&id.name) {
                Some(tp) if tp.has(BoundKind::Number) => Ty::Param(id.name.clone()),
                Some(_) => {
                    self.err(
                        id.span,
                        format!(
                            "`({}) x` needs `{}` to be a `number` (DESIGN §10.1)",
                            id.name, id.name
                        ),
                    );
                    Ty::Unknown
                }
                None => {
                    self.err(id.span, format!("unknown type `{}`", id.name));
                    Ty::Unknown
                }
            },
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, span: Span, consumed: bool) -> Ty {
        use BinOp::*;
        match op {
            And | Or => {
                self.cond(l);
                // The right side may not run, so what it assigns counts only as "maybe".
                let after_left = self.flow.clone();
                let saved = self.narrowed.clone();
                let (t, f) = self.facts(l);
                self.narrowed.extend(if op == And { t } else { f });
                self.cond(r);
                self.narrowed = saved;
                let after_right = self.take_flow();
                self.flow = Flow::join(after_left, after_right);
                Ty::bool()
            }
            Eq | Ne | Lt | Gt | Le | Ge => {
                let a = self.expr(l, None);
                let b = self.expr(r, None);
                let ordering = !matches!(op, Eq | Ne);
                if let Err(m) = self.comparable(&a, &b, ordering) {
                    self.err(span, m);
                }
                Ty::bool()
            }
            InsertEnd | InsertFront | InsertCursor => {
                let (sym, method) = match op {
                    InsertEnd => ("+>", "push"),
                    InsertFront => ("+<", "pushFront"),
                    _ => ("+^", "insert"),
                };
                let lt = self.expr_c(l, None, consumed);
                if matches!(lt.unref(), Ty::Array(..)) && !matches!(l.kind, E::Binary { .. }) {
                    self.need_writable(
                        l,
                        &format!("can't use `{}` here, since it inserts into the array", sym),
                    );
                }
                match lt.unref() {
                    Ty::Array(e, k) => {
                        if let Err(need) = builtins::seq_method(e, SeqKind::Array(*k), method) {
                            self.err(
                                span,
                                format!(
                                    "`{}` needs {}, and `{}` can't",
                                    sym,
                                    need,
                                    self.show(lt.unref())
                                ),
                            );
                        }
                        let e = (**e).clone();
                        let rt = self.expr(r, Some(&e));
                        self.expect(&rt, &e, r);
                        self.note_status(Some(k.cap == CapKind::Capped), consumed, span, sym);
                    }
                    Ty::Unknown => {
                        self.expr(r, None);
                    }
                    Ty::Slice(_) | Ty::View(_) => {
                        self.expr(r, None);
                        self.err(
                            span,
                            format!(
                                "`{}` inserts into an array, and a slice can't change its length",
                                sym
                            ),
                        );
                    }
                    t => {
                        self.expr(r, None);
                        self.err(
                            span,
                            format!(
                                "`{}` inserts into an array, and this is {}",
                                sym,
                                self.show(t)
                            ),
                        );
                    }
                }
                lt
            }
            Shl | Shr => {
                let a = self.expr(l, None);
                let b = self.expr(r, None);
                if !self.is_int_like(&a) {
                    self.err(
                        l.span,
                        format!(
                            "a shift needs an integer on its left, and this is {}",
                            self.show(&a)
                        ),
                    );
                }
                if !self.is_int_like(&b) {
                    self.err(
                        r.span,
                        format!("a shift count is an integer, and this is {}", self.show(&b)),
                    );
                }
                match (a.unref(), b.unref()) {
                    (Ty::IntLit { value: Some(x), .. }, Ty::IntLit { value: Some(y), .. }) => {
                        Ty::IntLit {
                            value: fold(op, *x, *y),
                            is_char: false,
                        }
                    }
                    (Ty::IntLit { .. }, _) => Ty::int_lit(),
                    (a, _) => a.clone(),
                }
            }
            BitAnd | BitOr | BitXor => {
                let a = self.expr(l, None);
                let b = self.expr(r, None);
                if a.unref() == &Ty::bool() && b.unref() == &Ty::bool() {
                    return Ty::bool();
                }
                for (t, e) in [(&a, l), (&b, r)] {
                    if !self.is_int_like(t) {
                        self.err(
                            e.span,
                            format!(
                                "`{}` needs integers or two bools, and this is {}",
                                op_text(op),
                                self.show(t)
                            ),
                        );
                        return Ty::Unknown;
                    }
                }
                self.arith(op, &a, &b, span)
            }
            _ => {
                let a = self.expr(l, None);
                let b = self.expr(r, None);
                if matches!(op, Div | Rem) && matches!(b.unref(), Ty::IntLit { value: Some(0), .. })
                {
                    self.err(r.span, "this divides by zero, which stops the program");
                }
                self.arith(op, &a, &b, span)
            }
        }
    }

    /// The type of an arithmetic or bitwise operation on two numbers (DESIGN §9.2).
    fn arith(&mut self, op: BinOp, a: &Ty, b: &Ty, span: Span) -> Ty {
        use Ty::*;
        let (a, b) = (a.unref(), b.unref());
        let sym = op_text(op);
        let int_only = matches!(
            op,
            BinOp::Rem
                | BinOp::WrapAdd
                | BinOp::WrapSub
                | BinOp::WrapMul
                | BinOp::SatAdd
                | BinOp::SatSub
                | BinOp::SatMul
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
        );
        if a.is_string() || b.is_string() {
            self.err_help(
                span,
                format!("there's no `{}` on strings", sym),
                "build text with interpolation: `\"${a}${b}\"` (DESIGN §4.11)",
            );
            return Unknown;
        }
        let result = match (a, b) {
            (Unknown, _) | (_, Unknown) => return Unknown,
            (IntLit { value: Some(x), .. }, IntLit { value: Some(y), .. }) => {
                return IntLit {
                    value: fold(op, *x, *y),
                    is_char: false,
                };
            }
            (IntLit { .. }, IntLit { .. }) => return Ty::int_lit(),
            (FloatLit, FloatLit) | (FloatLit, IntLit { .. }) | (IntLit { .. }, FloatLit) => {
                FloatLit
            }
            (IntLit { value, .. }, Prim(p)) | (Prim(p), IntLit { value, .. })
                if p.is_int() || p.is_float() =>
            {
                if let Some(v) = value
                    && p.is_int()
                    && !fits(*v, *p)
                {
                    self.err(
                        span,
                        format!(
                            "{} doesn't fit in {}, the type of the other operand",
                            v,
                            p.name()
                        ),
                    );
                }
                Prim(*p)
            }
            (FloatLit, Prim(p)) | (Prim(p), FloatLit) if p.is_float() => Prim(*p),
            (FloatLit, Prim(p)) | (Prim(p), FloatLit) if p.is_int() => {
                self.err_help(
                    span,
                    format!("a float literal with an integer ({})", p.name()),
                    "cast the integer: `(f64) n` (DESIGN §9.2)",
                );
                return Unknown;
            }
            (Prim(x), Prim(y)) if (x.is_int() || x.is_float()) && (y.is_int() || y.is_float()) => {
                if x == y {
                    Prim(*x)
                } else if lossless(*x, *y) {
                    Prim(*y)
                } else if lossless(*y, *x) {
                    Prim(*x)
                } else {
                    let help = if x.is_float() || y.is_float() {
                        "cast the integer: `(f64) n` (DESIGN §9.2)".to_string()
                    } else if bits(*x).max(bits(*y)) < 64 {
                        "cast one side, as in `(i64) x` (DESIGN §9.2)".to_string()
                    } else {
                        format!("cast one side, as in `({}) x` (DESIGN §9.2)", x.name())
                    };
                    self.err_help(
                        span,
                        format!("no type holds every {} and every {}", x.name(), y.name()),
                        help,
                    );
                    return Unknown;
                }
            }
            (Param(n), Param(m)) if n == m => Param(n.clone()),
            (Param(n), IntLit { value, .. }) | (IntLit { value, .. }, Param(n)) => {
                if let Some(tp) = self.tparam(n)
                    && !self.lit_fits_param(*value, false, &tp)
                {
                    self.err_help(span, format!("this literal doesn't fit every type `{}` can be", n), "in a generic body, a literal must fit every type the bound allows: 0 to 127 under `number` and `int` (DESIGN §10.1)");
                }
                Param(n.clone())
            }
            (Param(n), FloatLit) | (FloatLit, Param(n)) => {
                if !self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Float)) {
                    self.err(
                        span,
                        format!("a float literal with `{}`, which needn't be a float", n),
                    );
                }
                Param(n.clone())
            }
            _ => {
                self.err(
                    span,
                    format!(
                        "`{}` needs numbers, and these are {} and {}",
                        sym,
                        self.show(a),
                        self.show(b)
                    ),
                );
                return Unknown;
            }
        };
        match &result {
            Prim(p) if p.is_float() && int_only => {
                self.err(
                    span,
                    format!("`{}` needs integers, and these are floats", sym),
                );
                Unknown
            }
            FloatLit if int_only => {
                self.err(span, format!("`{}` needs integers", sym));
                Unknown
            }
            Param(n) => {
                let need = if int_only {
                    BoundKind::Int
                } else {
                    BoundKind::Number
                };
                if !self.tparam(n).is_some_and(|tp| tp.has(need)) {
                    self.err_help(
                        span,
                        format!("`{}` needs `{}` to be `{}`", sym, n, need.name()),
                        format!(
                            "add it to the bound: `<{} implements {}>` (DESIGN §10.1)",
                            n,
                            need.name()
                        ),
                    );
                }
                result
            }
            _ => result,
        }
    }

    /// Whether `==` or `<` can compare values of the two types (DESIGN §9.2, §10.1).
    fn comparable(&self, a: &Ty, b: &Ty, ordering: bool) -> Result<(), String> {
        use Ty::*;
        let what = if ordering { "order" } else { "compare" };
        let bad = || {
            Err(format!(
                "can't {} {} with {}",
                what,
                self.show(a),
                self.show(b)
            ))
        };
        if a.is_unknown() || b.is_unknown() {
            return Ok(());
        }
        // References compare with null before they're dereferenced.
        match (a, b) {
            (Null, Nullable(_)) | (Nullable(_), Null) if !ordering => return Ok(()),
            (Null, Ref(_)) | (Ref(_), Null) if !ordering => {
                return Err("this reference can't be null; only `T&?` can".into());
            }
            (Null, _) | (_, Null) => {
                return Err("only nullable references (`T&?`) can be compared with `null`".into());
            }
            (Nullable(x), Nullable(y)) if !ordering && self.same(x, y) => return Ok(()),
            (Nullable(_), _) | (_, Nullable(_)) => return Err(
                "this may be null here; check `x != null` first, in a local variable (DESIGN §5.1)"
                    .into(),
            ),
            _ => {}
        }
        let (a, b) = (a.unref(), b.unref());
        let seq_elem = |t: &Ty| match t {
            Slice(e) | View(e) => Some((**e).clone()),
            Array(e, k) if k.sliceable() => Some((**e).clone()),
            _ => None,
        };
        let num = |t: &Ty| {
            matches!(t, IntLit { .. } | FloatLit)
                || matches!(t, Prim(p) if p.is_int() || p.is_float())
        };
        match (a, b) {
            (
                IntLit {
                    value: Some(v),
                    is_char,
                },
                Prim(p),
            )
            | (
                Prim(p),
                IntLit {
                    value: Some(v),
                    is_char,
                },
            ) if p.is_int() => {
                if Self::lit_rank(Some(*v), *is_char, *p).is_none() {
                    if *is_char && *p == crate::token::Prim::U8 {
                        return Err("this character isn't ASCII, so it would never equal a byte; compare with a string (DESIGN §9.1)".into());
                    }
                    return Err(format!(
                        "{} doesn't fit in {}, so this comparison is always the same",
                        v,
                        p.name()
                    ));
                }
                Ok(())
            }
            (Prim(x), Prim(y)) if x.is_int() && y.is_int() => Ok(()),
            (FloatLit, Prim(p)) | (Prim(p), FloatLit) if p.is_int() => Err(format!(
                "can't {} a float literal with an integer ({}); cast the integer: `(f64) n`",
                what,
                p.name()
            )),
            (Prim(x), Prim(y)) if num(a) && num(b) => {
                if x == y || lossless(*x, *y) || lossless(*y, *x) {
                    Ok(())
                } else {
                    Err(format!(
                        "no type holds every {} and every {}; cast one side (DESIGN §9.2)",
                        x.name(),
                        y.name()
                    ))
                }
            }
            _ if num(a) && num(b) => Ok(()),
            (Prim(crate::token::Prim::Bool), Prim(crate::token::Prim::Bool)) => Ok(()),
            (Param(n), Param(m)) if n == m => {
                if ordering && !self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Ordered)) {
                    Err(format!(
                        "`{}` isn't `ordered`; add it to the bound: `<{} implements ordered>` (DESIGN §10.1)",
                        n, n
                    ))
                } else {
                    Ok(())
                }
            }
            (Param(n), IntLit { value, .. }) | (IntLit { value, .. }, Param(n)) => {
                match self.tparam(n) {
                    Some(tp) if self.lit_fits_param(*value, false, &tp) => {
                        if ordering && !tp.has(BoundKind::Ordered) {
                            Err(format!("`{}` isn't `ordered` (DESIGN §10.1)", n))
                        } else {
                            Ok(())
                        }
                    }
                    _ => Err(format!(
                        "this literal doesn't fit every type `{}` can be (DESIGN §10.1)",
                        n
                    )),
                }
            }
            (Param(n), FloatLit) | (FloatLit, Param(n)) => {
                if self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Float)) {
                    Ok(())
                } else {
                    bad()
                }
            }
            _ => {
                if let (Some(x), Some(y)) = (seq_elem(a), seq_elem(b)) {
                    if !self.same(&x, &y) {
                        return bad();
                    }
                    if ordering && x != Ty::u8() {
                        return Err(format!(
                            "only strings can be ordered with `<`; these are {} and {}",
                            self.show(a),
                            self.show(b)
                        ));
                    }
                    return Ok(());
                }
                if !self.same(a, b) {
                    return bad();
                }
                if ordering && !self.ordered(a) {
                    if let Class(id, _) = a {
                        let n = &self.prog.classes[*id].name;
                        return Err(format!(
                            "`{}` isn't ordered; give it `i32 compareTo({} other)` and `implements ordered` (DESIGN §10.1)",
                            n, n
                        ));
                    }
                    return bad();
                }
                match a {
                    Func(..) | Map(..) | Set(_) | Pool(_) | Iter(_) | TypeName(_)
                    | Namespace(_) | Tuple(_) | Void => bad(),
                    _ => Ok(()),
                }
            }
        }
    }

    /// The type of `c ? a : b`, or of the arms of a switch expression.
    fn unify(&mut self, a: &Ty, b: &Ty, span: Span) -> Ty {
        use Ty::*;
        match (a, b) {
            (Null, Nullable(_)) | (Null, Ref(_)) => {
                return Nullable(Box::new(b.clone())).flatten_nullable();
            }
            (Nullable(_), Null) | (Ref(_), Null) => {
                return Nullable(Box::new(a.clone())).flatten_nullable();
            }
            _ => {}
        }
        if a.is_unknown() {
            return b.clone();
        }
        if b.is_unknown() || self.same(a, b) {
            return a.clone();
        }
        let (x, y) = (a.unref(), b.unref());
        if self.same(x, y) {
            return x.clone();
        }
        match (x, y) {
            (IntLit { is_char: c1, .. }, IntLit { is_char: c2, .. }) => IntLit {
                value: None,
                is_char: *c1 && *c2,
            },
            (FloatLit, FloatLit | IntLit { .. }) | (IntLit { .. }, FloatLit) => FloatLit,
            (IntLit { .. } | FloatLit, Prim(_) | Param(_)) => y.clone(),
            (Prim(_) | Param(_), IntLit { .. } | FloatLit) => x.clone(),
            (Prim(p), Prim(q)) if lossless(*p, *q) => y.clone(),
            (Prim(p), Prim(q)) if lossless(*q, *p) => x.clone(),
            (Slice(e) | View(e) | Array(e, _), Slice(f) | View(f) | Array(f, _))
                if self.same(e, f) =>
            {
                Slice(e.clone())
            }
            _ => {
                self.err(
                    span,
                    format!(
                        "the two branches have different types: {} and {}",
                        self.show(x),
                        self.show(y)
                    ),
                );
                Unknown
            }
        }
    }

    /// A literal longer than a capped array's capacity doesn't compile, as a number
    /// literal that doesn't fit its type doesn't (DESIGN §9.1).
    fn over_cap(&mut self, n: usize, cap: u64, what: &str, span: Span) {
        if cap > 0 && n as u64 > cap {
            self.err_help(
                span,
                format!(
                    "this literal has {} {}, and the array holds {}",
                    n, what, cap
                ),
                "shorten the literal, or raise the array's cap",
            );
        }
    }

    fn array_lit(&mut self, items: &[Expr], want: Option<&Ty>, span: Span) -> Ty {
        match want.map(|w| w.unref().clone()) {
            Some(t @ (Ty::Array(..) | Ty::Slice(_))) => {
                let e = t.elem().cloned().unwrap_or(Ty::Unknown);
                for x in items {
                    let it = self.expr(x, Some(&e));
                    self.expect(&it, &e, x);
                }
                if let Ty::Array(_, k) = &t
                    && k.cap == CapKind::Capped
                {
                    self.over_cap(items.len(), k.size, "elements", span);
                }
                t
            }
            Some(t @ (Ty::Map(..) | Ty::Set(_) | Ty::Pool(_))) => {
                if let Some(first) = items.first() {
                    self.err_help(first.span, "only `[]` creates a hashmap, a hashset or a pool", "start with `[]` and add the entries (hash map literals are undecided, GRAMMAR §8)");
                }
                t
            }
            Some(Ty::Unknown) => {
                for x in items {
                    self.expr(x, None);
                }
                Ty::Unknown
            }
            _ => {
                if items.is_empty() {
                    self.err_help(
                        span,
                        "`[]` takes its type from where it's used, and nothing says it here",
                        "declare the variable's type: `i64[..>] xs = [];`",
                    );
                    return Ty::Unknown;
                }
                let first = self.expr(&items[0], None);
                let e = self.var_type(&first, items[0].span);
                for x in &items[1..] {
                    let it = self.expr(x, Some(&e));
                    self.expect(&it, &e, x);
                }
                Ty::Array(Box::new(e), ArrayKind::frozen())
            }
        }
    }

    // ------------------------------------------------------------ calls

    fn note_status(&mut self, can_fail: Option<bool>, consumed: bool, span: Span, name: &str) {
        self.last_can_fail = can_fail;
        if can_fail == Some(true) && !consumed {
            self.warn_help(
                span,
                format!("the status of `{}` can be nonzero, and it is dropped", name),
                "capture it with `x @err = ...`, or discard it on purpose with `@_ = ...` (DESIGN §7.3)",
            );
        }
    }

    fn type_args(&mut self, args: &[ast::Arg]) {
        for a in args {
            self.expr(&a.expr, None);
        }
    }

    fn call(&mut self, callee: &Expr, args: &[ast::Arg], span: Span, consumed: bool) -> Ty {
        self.last_can_fail = None;
        match &callee.kind {
            E::Ident(name) => self.bare_call(name, callee.span, args, span, consumed),
            E::Member { expr: base, name } => {
                let bt = self.expr(base, None);
                self.method_call(&bt, base, name, args, span, consumed)
            }
            E::PrimMember { prim, name } => {
                let me = Ty::Prim(*prim);
                let sigs = builtins::number_static(&me, NumCaps::of_prim(*prim), &name.name);
                if sigs.is_empty() {
                    self.type_args(args);
                    self.err(
                        name.span,
                        format!("`{}` has no function `{}`", prim.name(), name.name),
                    );
                    return Ty::Unknown;
                }
                self.resolve_call(
                    &format!("{}.{}", prim.name(), name.name),
                    sigs,
                    None,
                    args,
                    span,
                    consumed,
                )
            }
            _ => {
                let t = self.expr(callee, None);
                self.call_value(&t, "this function", args, span, consumed)
            }
        }
    }

    fn method_sigs(&self, id: ClassId, cargs: &[Ty], ms: &[&MethodInfo]) -> Vec<Sig> {
        let map = self.class_subst(id, cargs);
        ms.iter()
            .map(|m| Sig {
                origin: Origin::Method(
                    id,
                    self.prog.classes[id]
                        .methods
                        .iter()
                        .position(|x| std::ptr::eq(x, *m))
                        .unwrap_or(0),
                ),
                name: m.name.clone(),
                params: m.params.iter().map(|p| p.subst(&map)).collect(),
                ret: m.ret.subst(&map),
                tparams: m.type_params.clone(),
                can_fail: m.can_fail,
                is_mut: m.is_mut,
                needs: Vec::new(),
            })
            .collect()
    }

    fn bare_call(
        &mut self,
        name: &str,
        name_span: Span,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        let prog = self.prog;
        if let Some(id) = self.lookup_local(name) {
            let t = self.locals[id].ty.clone();
            return self.call_value(&t, name, args, span, consumed);
        }
        if let Some(cls) = self.class {
            let c = &prog.classes[cls];
            if c.kind != ClassKind::Script {
                let ms: Vec<&MethodInfo> = c.methods.iter().filter(|m| m.name == name).collect();
                if !ms.is_empty() {
                    let usable: Vec<&MethodInfo> = ms
                        .iter()
                        .copied()
                        .filter(|m| m.is_static || !self.fn_static())
                        .collect();
                    if usable.is_empty() {
                        self.type_args(args);
                        let here = self.fns.last().map(|f| f.name.clone()).unwrap_or_default();
                        self.err(
                            name_span,
                            format!(
                                "`{}` works on a value of `{}`, and `{}` is static",
                                name, c.name, here
                            ),
                        );
                        return Ty::Unknown;
                    }
                    if usable.iter().any(|m| !m.is_static) {
                        self.this_use(name_span, &format!("`{}` can't be called", name));
                    }
                    let sigs = self.method_sigs(cls, &self.self_type(cls).class_args(), &usable);
                    let t = self.resolve_call(name, sigs, None, args, span, consumed);
                    if self.last_call_mut
                        && let Some(ro) = self.this_read_only()
                    {
                        self.err_help(
                            name_span,
                            format!(
                                "can't call `{}` here, since it changes its object: {}",
                                name, ro.msg
                            ),
                            ro.help,
                        );
                    }
                    return t;
                }
                if let Some(f) = c.fields.iter().find(|f| f.name == name) {
                    let t = f.ty.clone();
                    return self.call_value(&t, name, args, span, consumed);
                }
            }
        }
        if let Some(script) = prog.files[self.file].script {
            let ms: Vec<&MethodInfo> = prog.classes[script]
                .methods
                .iter()
                .filter(|m| m.name == name)
                .collect();
            if !ms.is_empty() {
                let sigs = self.method_sigs(script, &[], &ms);
                return self.resolve_call(name, sigs, None, args, span, consumed);
            }
            if let Some(f) = prog.classes[script].fields.iter().find(|f| f.name == name) {
                let t = f.ty.clone();
                return self.call_value(&t, name, args, span, consumed);
            }
        }
        let g = builtins::global(name);
        if !g.is_empty() {
            return self.resolve_call(name, g, None, args, span, consumed);
        }
        self.type_args(args);
        if !self.unmodelled() {
            self.err(name_span, format!("unknown function `{}`", name));
        }
        Ty::Unknown
    }

    fn call_value(
        &mut self,
        t: &Ty,
        name: &str,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        match t.unref() {
            Ty::Func(ps, r) => {
                let mut sig = Sig::new(name, ps.clone(), (**r).clone());
                sig.origin = Origin::Value;
                self.resolve_call(name, vec![sig], None, args, span, consumed)
            }
            Ty::Unknown => {
                self.type_args(args);
                Ty::Unknown
            }
            Ty::Nullable(_) => {
                self.type_args(args);
                self.err_help(
                    span,
                    format!("`{}` may be null here", name),
                    "check `f != null` first, in a local variable (DESIGN §5.1)",
                );
                Ty::Unknown
            }
            other => {
                self.type_args(args);
                self.err(
                    span,
                    format!("`{}` isn't a function; it is {}", name, self.show(other)),
                );
                Ty::Unknown
            }
        }
    }

    fn method_call(
        &mut self,
        bt: &Ty,
        base: &Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        self.last_call_mut = false;
        let t = self.method_call_inner(bt, base, name, args, span, consumed);
        let on_value = !matches!(bt, Ty::TypeName(_) | Ty::Namespace(_));
        if self.last_call_mut && on_value {
            let action = format!(
                "can't call `{}` here, since it changes what it's called on",
                name.name
            );
            self.need_writable(base, &action);
        }
        t
    }

    #[allow(clippy::too_many_arguments)]
    fn method_call_inner(
        &mut self,
        bt: &Ty,
        base: &Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        let prog = self.prog;
        let n = name.name.as_str();
        let t = bt.unref().clone();
        match &t {
            Ty::Unknown => {
                self.type_args(args);
                self.last_can_fail = None;
                Ty::Unknown
            }
            Ty::Nullable(_) => {
                self.type_args(args);
                self.err_help(base.span, "this may be null here", "check `x != null` first, in a local variable; narrowing doesn't apply to fields (DESIGN §5.1)");
                Ty::Unknown
            }
            Ty::TypeName(inner) => match &**inner {
                Ty::Class(id, _) => {
                    let c = &prog.classes[*id];
                    let ms: Vec<&MethodInfo> = c.methods.iter().filter(|m| m.name == n).collect();
                    if ms.is_empty() {
                        self.type_args(args);
                        self.err(name.span, format!("`{}` has no function `{}`", c.name, n));
                        return Ty::Unknown;
                    }
                    let statics: Vec<&MethodInfo> =
                        ms.iter().copied().filter(|m| m.is_static).collect();
                    if statics.is_empty() {
                        self.type_args(args);
                        self.err(
                            name.span,
                            format!(
                                "`{}` isn't static, so it needs a value of `{}`, not the class",
                                n, c.name
                            ),
                        );
                        return Ty::Unknown;
                    }
                    self.check_private_methods(*id, &statics, name.span);
                    let sigs = self.method_sigs(*id, &[], &statics);
                    self.resolve_call(
                        &format!("{}.{}", c.name, n),
                        sigs,
                        None,
                        args,
                        span,
                        consumed,
                    )
                }
                Ty::Param(p) => {
                    let sigs = match self.tparam(p) {
                        Some(tp) => builtins::number_static(
                            &Ty::Param(p.clone()),
                            NumCaps::of_param(&tp),
                            n,
                        ),
                        None => Vec::new(),
                    };
                    if sigs.is_empty() {
                        self.type_args(args);
                        self.err(
                            name.span,
                            format!(
                                "`{}.{}` isn't available under `{}`'s bounds (DESIGN §10.1)",
                                p, n, p
                            ),
                        );
                        return Ty::Unknown;
                    }
                    self.resolve_call(&format!("{}.{}", p, n), sigs, None, args, span, consumed)
                }
                _ => {
                    self.type_args(args);
                    Ty::Unknown
                }
            },
            Ty::Namespace(ns) => {
                let sigs = if *ns == "status" {
                    builtins::status_fn(n)
                } else {
                    builtins::math_fn(n)
                };
                if sigs.is_empty() {
                    self.type_args(args);
                    self.err(name.span, format!("`{}` has no function `{}`", ns, n));
                    return Ty::Unknown;
                }
                self.resolve_call(&format!("{}.{}", ns, n), sigs, None, args, span, consumed)
            }
            Ty::Class(id, cargs) => {
                let c = &prog.classes[*id];
                let ms: Vec<&MethodInfo> = c.methods.iter().filter(|m| m.name == n).collect();
                if !ms.is_empty() {
                    self.check_private_methods(*id, &ms, name.span);
                    let sigs = self.method_sigs(*id, cargs, &ms);
                    return self.resolve_call(n, sigs, None, args, span, consumed);
                }
                if let Some(f) = c.fields.iter().find(|f| f.name == n) {
                    let ft = f.ty.subst(&self.class_subst(*id, cargs));
                    return self.call_value(&ft, n, args, span, consumed);
                }
                self.ufcs(&t, base, name, args, span, consumed)
            }
            Ty::Param(p) => {
                let mut sigs = Vec::new();
                if let Some(tp) = self.tparam(p) {
                    for b in &tp.bounds {
                        if let Bound::Interface(Ty::Class(i, iargs)) = b {
                            let ms: Vec<&MethodInfo> = prog.classes[*i]
                                .methods
                                .iter()
                                .filter(|m| m.name == n)
                                .collect();
                            sigs.extend(self.method_sigs(*i, iargs, &ms));
                        }
                    }
                    sigs.extend(builtins::number_method(&t, NumCaps::of_param(&tp), n));
                }
                if sigs.is_empty() {
                    return self.ufcs(&t, base, name, args, span, consumed);
                }
                self.resolve_call(n, sigs, None, args, span, consumed)
            }
            Ty::Prim(_) | Ty::IntLit { .. } | Ty::FloatLit => {
                let me = t.default_lit();
                let Ty::Prim(p) = me else { unreachable!() };
                let sigs = builtins::number_method(&me, NumCaps::of_prim(p), n);
                if sigs.is_empty() {
                    return self.ufcs(&me, base, name, args, span, consumed);
                }
                self.resolve_call(n, sigs, None, args, span, consumed)
            }
            Ty::Array(e, k) => {
                self.seq_call(&t, e, SeqKind::Array(*k), base, name, args, span, consumed)
            }
            Ty::Slice(e) => self.seq_call(&t, e, SeqKind::Slice, base, name, args, span, consumed),
            Ty::View(e) => self.seq_call(&t, e, SeqKind::View, base, name, args, span, consumed),
            Ty::Map(..) | Ty::Set(_) | Ty::Pool(_) => {
                let sigs = self.methods_of(&t, n);
                if sigs.is_empty() {
                    return self.ufcs(&t, base, name, args, span, consumed);
                }
                self.resolve_call(n, sigs, None, args, span, consumed)
            }
            _ => self.ufcs(&t, base, name, args, span, consumed),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn seq_call(
        &mut self,
        t: &Ty,
        e: &Ty,
        kind: SeqKind,
        base: &Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        match builtins::seq_method(e, kind, &name.name) {
            Err(need) => {
                self.type_args(args);
                self.err(
                    name.span,
                    format!(
                        "`{}` needs {}, and `{}` can't",
                        name.name,
                        need,
                        self.show(t)
                    ),
                );
                Ty::Unknown
            }
            Ok(sigs) if sigs.is_empty() => self.ufcs(t, base, name, args, span, consumed),
            Ok(mut sigs) => {
                // seek(0) and seekFromEnd(0) can't fail.
                if matches!(name.name.as_str(), "seek" | "seekFromEnd")
                    && args.len() == 1
                    && matches!(args[0].expr.kind, E::Int { value: 0, .. })
                {
                    for s in &mut sigs {
                        s.can_fail = false;
                    }
                }
                self.resolve_call(&name.name, sigs, None, args, span, consumed)
            }
        }
    }

    fn check_private_methods(&mut self, owner: ClassId, ms: &[&MethodInfo], span: Span) {
        if self.class != Some(owner) && ms.iter().all(|m| m.is_private) {
            let msg = format!(
                "`{}` is private to `{}`",
                ms[0].name, self.prog.classes[owner].name
            );
            self.err(span, msg);
        }
    }

    /// A free function called with method syntax: `x.helper()` (DESIGN §6.1).
    fn ufcs(
        &mut self,
        recv: &Ty,
        base: &Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        let prog = self.prog;
        let n = name.name.as_str();
        let mut sigs = Vec::new();
        if let Some(script) = prog.files[self.file].script {
            let ms: Vec<&MethodInfo> = prog.classes[script]
                .methods
                .iter()
                .filter(|m| m.name == n && !m.params.is_empty())
                .collect();
            sigs.extend(self.method_sigs(script, &[], &ms));
        }
        if let Some(cls) = self.class
            && prog.classes[cls].kind != ClassKind::Script
        {
            let ms: Vec<&MethodInfo> = prog.classes[cls]
                .methods
                .iter()
                .filter(|m| m.name == n && m.is_static && !m.params.is_empty())
                .collect();
            sigs.extend(self.method_sigs(cls, &[], &ms));
        }
        if !sigs.is_empty() {
            return self.resolve_call(n, sigs, Some((recv.clone(), base)), args, span, consumed);
        }
        self.type_args(args);
        self.err(
            name.span,
            format!("{} has no method `{}`", self.show(recv), n),
        );
        Ty::Unknown
    }

    fn new_expr(
        &mut self,
        class: Option<&ast::Type>,
        args: &[ast::Arg],
        want: Option<&Ty>,
        span: Span,
        consumed: bool,
    ) -> Ty {
        let prog = self.prog;
        let want_class = want.map(|w| w.unref().clone()).map(|w| match w {
            Ty::Nullable(x) => x.unref().clone(),
            w => w,
        });
        let ty = match class {
            Some(t) => {
                // `new Node(...)` for a generic class takes its type arguments from where it goes.
                let bare_generic = match &t.kind {
                    ast::TypeKind::Named { path, args: targs }
                        if targs.is_empty() && path.parts.len() == 1 =>
                    {
                        match prog.find_class(self.file, &path.parts[0].name) {
                            Lookup::Found(id) if !prog.classes[id].type_params.is_empty() => {
                                Some(id)
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                match (bare_generic, &want_class) {
                    (Some(id), Some(Ty::Class(wid, wargs))) if id == *wid => {
                        Ty::Class(id, wargs.clone())
                    }
                    _ => self.resolve(t),
                }
            }
            None => match &want_class {
                Some(t @ Ty::Class(..)) => t.clone(),
                Some(Ty::Unknown) => Ty::Unknown,
                _ => {
                    self.type_args(args);
                    self.err_help(span, "`new(...)` takes its class from where the value goes, and nothing says it here", "write the class: `new Item(...)` (DESIGN §6.1)");
                    return Ty::Unknown;
                }
            },
        };
        match &ty {
            Ty::Class(id, cargs) => {
                let c = &prog.classes[*id];
                match c.kind {
                    ClassKind::Interface => {
                        self.type_args(args);
                        self.err(span, format!("`{}` is an interface, so it can't be constructed; construct a class that implements it", c.name));
                        return Ty::Unknown;
                    }
                    ClassKind::Enum => {
                        self.type_args(args);
                        self.err(
                            span,
                            format!(
                                "the values of `{}` are its constants, such as `{}.{}`",
                                c.name,
                                c.name,
                                c.constants.first().map_or("X", |s| s.as_str())
                            ),
                        );
                        return Ty::Unknown;
                    }
                    _ => {}
                }
                let map = self.class_subst(*id, cargs);
                let sigs: Vec<Sig> = c
                    .ctors
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        let mut s = Sig::new(
                            &format!("new {}", c.name),
                            k.params.iter().map(|p| p.subst(&map)).collect(),
                            Ty::Void,
                        );
                        s.can_fail = k.can_fail;
                        s.origin = Origin::Ctor(*id, i);
                        s
                    })
                    .collect();
                // A root lives only in a named local or static (DESIGN §6.6).
                if c.kind == ClassKind::Root && !std::mem::take(&mut self.root_new_ok) {
                    self.err_help(
                        span,
                        format!("`{}` is a root, so it can't be a temporary", c.name),
                        format!("create it as a named local or static: `{} x = new {}(...);` (DESIGN §6.6)", c.name, c.name),
                    );
                }
                self.resolve_call(&format!("new {}", c.name), sigs, None, args, span, consumed);
                ty
            }
            Ty::Unknown => {
                self.type_args(args);
                Ty::Unknown
            }
            t => {
                self.type_args(args);
                self.err_help(span, format!("`new` constructs classes, and {} isn't one", self.show(t)), "arrays, numbers and other values come from literals and declarations (DESIGN §6.1)");
                Ty::Unknown
            }
        }
    }

    fn this_new(&mut self, args: &[ast::Arg], span: Span, consumed: bool) -> Ty {
        let in_ctor = self.fns.last().is_some_and(|f| f.kind == FnKind::Ctor);
        let Some(cls) = self.class.filter(|_| in_ctor) else {
            self.type_args(args);
            self.err(span, "`this.new(...)` calls another constructor, so it is only in a constructor (DESIGN §6.2)");
            return Ty::Void;
        };
        // It runs at most once on any path, before anything assigns a field (DESIGN §6.2).
        if !self.flow_loops.is_empty() {
            self.err(span, "`this.new(...)` is inside a loop, so it could run more than once; it runs at most once (DESIGN §6.2)");
        } else if self.flow.maybe_delegated() {
            self.err(span, "`this.new(...)` may already have run on a path to here; it runs at most once (DESIGN §6.2)");
        } else {
            let assigned: Vec<String> = self
                .ctor_fields
                .iter()
                .filter(|f| self.flow.is_maybe(f.slot))
                .map(|f| format!("`{}`", f.name))
                .collect();
            if !assigned.is_empty() {
                self.err_help(
                    span,
                    format!("{} {} assigned before `this.new(...)`, which assigns every field", join_names(&assigned), if assigned.len() == 1 { "is" } else { "are" }),
                    "before `this.new(...)`, statements may only compute its arguments (DESIGN §6.2)",
                );
            }
        }
        let c = &self.prog.classes[cls];
        let sigs: Vec<Sig> = c
            .ctors
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let mut s = Sig::new("this.new", k.params.clone(), Ty::Void);
                s.can_fail = k.can_fail;
                s.origin = Origin::Ctor(cls, i);
                s
            })
            .collect();
        self.resolve_call("this.new", sigs, None, args, span, consumed);
        let slots: Vec<usize> = self.ctor_fields.iter().map(|f| f.slot).collect();
        self.flow.delegate(slots);
        Ty::Void
    }

    // ------------------------------------------------------------ definite assignment (DESIGN §6.2)

    fn take_flow(&mut self) -> Flow {
        std::mem::replace(&mut self.flow, Flow::unreachable())
    }

    fn new_slot(&mut self) -> usize {
        self.nslots += 1;
        self.nslots - 1
    }

    /// Whether the innermost function that isn't a lambda is a constructor.
    fn in_ctor(&self) -> bool {
        self.fns
            .iter()
            .rev()
            .find(|f| f.kind != FnKind::Lambda)
            .is_some_and(|f| f.kind == FnKind::Ctor)
    }

    /// The slot of a field the current constructor must assign.
    fn ctor_field(&self, name: &str) -> Option<usize> {
        if !self.in_ctor() {
            return None;
        }
        self.ctor_fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.slot)
    }

    /// The slot a bare name refers to: a local declared without a value, or a field in a constructor.
    fn name_slot(&self, name: &str) -> Option<usize> {
        match self.lookup_local(name) {
            Some(id) => self.locals[id].slot,
            None => self.ctor_field(name),
        }
    }

    /// The slot a plain assignment to `target` fills.
    fn target_slot(&self, target: &Expr) -> Option<usize> {
        match &target.kind {
            E::Ident(n) => self.name_slot(n),
            E::Member { expr, name } if matches!(expr.kind, E::This) => self.ctor_field(&name.name),
            _ => None,
        }
    }

    fn read_slot(&mut self, slot: usize, name: &str, span: Span, field: bool) {
        if self.flow.is_def(slot) {
            return;
        }
        let some = self.flow.is_maybe(slot);
        let msg = if some {
            format!(
                "`{}` may be read before it is assigned: some paths to here don't assign it",
                name
            )
        } else {
            format!("`{}` is read before it is assigned", name)
        };
        let help = if field {
            "in a constructor, a field can't be read until every path to the read has assigned it (DESIGN §6.2)"
        } else if some {
            "assign it on every path before this, as with an `else` (DESIGN §6.2)"
        } else {
            "give it a value where it's declared, or assign it first (DESIGN §6.2)"
        };
        self.err_help(span, msg, help);
    }

    /// The fields not yet assigned on every path; with `failing`, only those with no zero value.
    fn unassigned_fields(&self, failing: bool) -> Vec<String> {
        if self.flow.delegated() {
            return Vec::new();
        }
        self.ctor_fields
            .iter()
            .filter(|f| !self.flow.is_def(f.slot) && (!failing || f.no_zero))
            .map(|f| format!("`{}`", f.name))
            .collect()
    }

    /// A constructor ends here, successfully or with a constant nonzero status.
    fn check_ctor_end(&mut self, span: Span, failing: bool) {
        if !self.in_ctor() || self.cur().kind != FnKind::Ctor {
            return;
        }
        let missing = self.unassigned_fields(failing);
        if missing.is_empty() {
            return;
        }
        let list = join_names(&missing);
        if failing {
            let verb = if missing.len() == 1 { "has" } else { "have" };
            self.err_help(
                span,
                format!("{} {} no zero value, so it must be assigned even when the constructor fails", list, verb),
                "a failing `return #code;` leaves the other fields at their zero values, but a non-nullable reference or function has none (DESIGN §6.2)",
            );
        } else {
            self.err_help(
                span,
                format!("the constructor can end here without assigning {}", list),
                "every field without an initializer is assigned on every path through a constructor, or by `this.new(...)`; only a failing `return #code;` needn't (DESIGN §6.2)",
            );
        }
    }

    /// `this` is used for something other than assigning or reading one field.
    fn this_use(&mut self, span: Span, what: &str) {
        if !self.in_ctor() {
            return;
        }
        let missing = self.unassigned_fields(false);
        if missing.is_empty() {
            return;
        }
        let verb = if missing.len() == 1 {
            "isn't"
        } else {
            "aren't"
        };
        self.err_help(
            span,
            format!("{} until every field is assigned, and {} {} yet", what, join_names(&missing), verb),
            "a method could read a field that isn't set, so until then `this` is only for assigning fields (DESIGN §6.2)",
        );
    }

    /// A non-nullable reference or function, or a value holding one, which has no zero value.
    fn no_zero(&self, t: &Ty, seen: &mut Vec<ClassId>) -> bool {
        match t {
            Ty::Ref(_) | Ty::Func(..) => true,
            Ty::Class(id, args) => {
                let c = &self.prog.classes[*id];
                if !matches!(c.kind, ClassKind::Class | ClassKind::Root) || seen.contains(id) {
                    return false;
                }
                seen.push(*id);
                let map = self.class_subst(*id, args);
                let found = c
                    .fields
                    .iter()
                    .filter(|f| !f.is_static)
                    .any(|f| self.no_zero(&f.ty.subst(&map), seen));
                seen.pop();
                found
            }
            _ => false,
        }
    }

    /// Chooses an overload (DESIGN §6.5), checks the arguments and returns the result type.
    #[allow(clippy::too_many_arguments)]
    fn resolve_call(
        &mut self,
        what: &str,
        cands: Vec<Sig>,
        recv: Option<(Ty, &Expr)>,
        args: &[ast::Arg],
        span: Span,
        consumed: bool,
    ) -> Ty {
        self.last_call_mut = false;
        for a in args {
            if a.marked {
                self.warn_help(a.expr.span, "`&` isn't needed at the call", "the parameter's declaration says whether the argument may change (DESIGN §5.1)");
            }
        }
        let has_recv = recv.is_some();
        let mut tys: Vec<Option<Ty>> = Vec::new();
        let mut exprs: Vec<Option<&Expr>> = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        if let Some((t, e)) = recv {
            tys.push(Some(t));
            exprs.push(Some(e));
            spans.push(e.span);
        }
        for a in args {
            tys.push(if context_typed(&a.expr) {
                None
            } else {
                Some(self.expr(&a.expr, None))
            });
            exprs.push(Some(&a.expr));
            spans.push(a.expr.span);
        }
        let n = tys.len();
        let arity: Vec<&Sig> = cands.iter().filter(|c| c.params.len() == n).collect();
        let mut fits: Vec<(Sig, Vec<Rank>, bool)> = Vec::new();
        let mut first_failure: Option<(Sig, usize)> = None;
        let mut infer_err: Option<String> = None;
        for c in &arity {
            let generic = !c.tparams.is_empty();
            let sig = if generic {
                match self.infer(c, &tys) {
                    Ok(s) => s,
                    Err(m) => {
                        infer_err.get_or_insert(m);
                        continue;
                    }
                }
            } else {
                (*c).clone()
            };
            let mut ranks = Vec::new();
            let mut failed = None;
            for (i, p) in sig.params.iter().enumerate() {
                match &tys[i] {
                    None => ranks.push(Rank::Exact),
                    Some(t) => {
                        let binds = matches!(p, Ty::Ref(_));
                        let place = exprs[i].is_none_or(|e| self.is_place(e))
                            || matches!(t, Ty::Ref(_) | Ty::Unknown);
                        match self.rank(t, p) {
                            Some(r) if !binds || place => ranks.push(r),
                            _ => {
                                failed = Some(i);
                                break;
                            }
                        }
                    }
                }
            }
            match failed {
                None => fits.push((sig, ranks, generic)),
                Some(i) => {
                    if first_failure.is_none() {
                        first_failure = Some((sig, i));
                    }
                }
            }
        }
        let chosen = match fits.len() {
            0 => {
                let shown_n = if has_recv { n - 1 } else { n };
                if arity.is_empty() {
                    let mut counts: Vec<usize> = cands
                        .iter()
                        .map(|c| c.params.len() - usize::from(has_recv))
                        .collect();
                    counts.sort();
                    counts.dedup();
                    let msg = if counts.len() == 1 {
                        format!(
                            "`{}` takes {}, not {}",
                            what,
                            plural(counts[0], "argument"),
                            shown_n
                        )
                    } else {
                        format!("no `{}` takes {}", what, plural(shown_n, "argument"))
                    };
                    self.err(span, msg);
                } else if arity.len() == 1 {
                    if let Some(m) = infer_err {
                        self.err(span, m);
                    } else if let Some((sig, i)) = first_failure {
                        let t = tys[i].clone().unwrap_or(Ty::Unknown);
                        let place = exprs[i].is_none_or(|e| self.is_place(e));
                        self.expect_at(&t, &sig.params[i], spans[i], place);
                    }
                } else {
                    let shown: Vec<String> = tys
                        .iter()
                        .skip(usize::from(has_recv))
                        .map(|t| t.as_ref().map_or("_".into(), |t| self.show(t)))
                        .collect();
                    let list: Vec<String> = arity
                        .iter()
                        .map(|c| self.show_sig(&c.name, &c.params[usize::from(has_recv)..], &c.ret))
                        .collect();
                    self.err_help(
                        span,
                        format!("no `{}` takes ({})", what, shown.join(", ")),
                        format!("the candidates are: {}", list.join("; ")),
                    );
                }
                for (i, t) in tys.iter().enumerate() {
                    if t.is_none()
                        && let Some(e) = exprs[i]
                    {
                        self.expr(e, Some(&Ty::Unknown));
                    }
                }
                self.last_call_mut = false;
                return Ty::Unknown;
            }
            1 => fits.pop().expect("one").0,
            _ => match best(&fits) {
                Some(i) => fits.swap_remove(i).0,
                None => {
                    let list: Vec<String> = fits
                        .iter()
                        .map(|(c, _, _)| {
                            self.show_sig(&c.name, &c.params[usize::from(has_recv)..], &c.ret)
                        })
                        .collect();
                    self.err_help(
                        span,
                        format!("the call to `{}` is ambiguous", what),
                        format!(
                            "it fits {}; cast an argument to choose (DESIGN §6.5)",
                            list.join(" and ")
                        ),
                    );
                    self.last_call_mut = false;
                    return Ty::Unknown;
                }
            },
        };
        for (i, t) in tys.iter().enumerate() {
            if t.is_none()
                && let Some(e) = exprs[i]
            {
                let p = &chosen.params[i];
                let at = self.expr(e, Some(p));
                self.expect(&at, p, e);
            }
        }
        for need in &chosen.needs {
            self.check_need(need, span, &chosen.name);
        }
        // An argument for a `T&` or `T[:]&` parameter may be changed by the callee (DESIGN §5.1).
        for (i, p) in chosen.params.iter().enumerate() {
            if (is_ref_ty(p) || matches!(p, Ty::View(_)))
                && let Some(e) = exprs[i]
            {
                let action = format!("can't pass this to `{}`, which may change it", what);
                self.need_writable(e, &action);
            }
        }
        self.note_status(Some(chosen.can_fail), consumed, span, &chosen.name);
        self.last_call_mut = chosen.is_mut;
        self.facts.calls.insert(
            key(span),
            CallFact {
                origin: chosen.origin.clone(),
                name: chosen.name.clone(),
                params: chosen.params.clone(),
                recv_arg: has_recv,
            },
        );
        chosen.ret
    }

    // ------------------------------------------------------------ read-only checks (DESIGN §5.1, §6.1, §4.10, §9.3)

    /// Why `this` can't be changed here: the innermost method isn't `mut`.
    fn this_read_only(&self) -> Option<Ro> {
        let f = self.fns.iter().rev().find(|f| f.kind != FnKind::Lambda)?;
        match f.kind {
            FnKind::Method | FnKind::Main if !f.is_mut => Some(Ro {
                msg: format!("`{}` isn't `mut`, so it can't change its object", f.name),
                help: format!(
                    "declare it `mut {} {}(...)` if it should change it (DESIGN §6.1)",
                    self.show(&f.ret),
                    f.name
                ),
            }),
            _ => None,
        }
    }

    /// A bare name that is an instance field of the current class.
    fn field_type_quiet(&self, name: &str) -> bool {
        self.class.is_some_and(|c| {
            self.prog.classes[c]
                .fields
                .iter()
                .any(|f| f.name == name && !f.is_static)
        })
    }

    /// Why a write to the variable or field `n` isn't allowed. With `through`, a write to
    /// what a reference refers to; without, to the variable itself, as `.=` does.
    fn read_only_name(&self, n: &str, through: bool) -> Option<Ro> {
        if let Some(id) = self.lookup_local(n) {
            let l = &self.locals[id];
            if through && is_ref_ty(&l.ty) {
                return l.ro_bind.as_ref().map(|why| Ro {
                    msg: format!("`{}` refers to something that can't be changed here", n),
                    help: format!("it was bound to it: {}", why),
                });
            }
            if through && matches!(l.ty, Ty::View(_)) {
                return None;
            }
            if l.depth < self.fns.len() {
                return Some(Ro {
                    msg: format!(
                        "`{}` is captured by this lambda, and captures are read-only",
                        n
                    ),
                    help: "have the lambda return a value instead (DESIGN §9.3)".into(),
                });
            }
            if l.param && through {
                let as_ref = match &l.ty {
                    Ty::Slice(e) => self.show(&Ty::View(e.clone())),
                    t => format!("{}&", self.show(t)),
                };
                return Some(Ro {
                    msg: format!("`{}` is a plain parameter, which is a read-only copy", n),
                    help: format!(
                        "declare it `{} {}` to let the function change the caller's value (DESIGN §5.1)",
                        as_ref, n
                    ),
                });
            }
            return None;
        }
        if self.field_type_quiet(n) {
            return self.this_read_only().map(|ro| Ro {
                msg: format!("`{}` is a field, and {}", n, ro.msg),
                help: ro.help,
            });
        }
        None
    }

    /// Why the storage `e` names can't be changed here, or `None` when it can.
    fn read_only(&self, e: &Expr) -> Option<Ro> {
        match &e.kind {
            E::Paren(x) => self.read_only(x),
            E::Ident(n) => self.read_only_name(n, true),
            E::This => self.this_read_only(),
            E::Member { expr, .. } => self.read_only(expr),
            E::Index { expr, .. } | E::Slice { expr, .. } => {
                if let Some(Ty::Slice(el)) = self
                    .facts
                    .types
                    .get(&(expr.span.start, expr.span.end))
                    .map(|t| t.unref())
                {
                    return Some(Ro {
                        msg: format!(
                            "a `{}` is a read-only view",
                            self.show(&Ty::Slice(el.clone()))
                        ),
                        help: "take a writable view (`T[:]&`) of the array itself (DESIGN §4.10)"
                            .into(),
                    });
                }
                self.read_only(expr)
            }
            E::Str(_) => Some(Ro {
                msg: "a string literal is read-only data".into(),
                help: "copy it into a `u8[..>]` buffer first (DESIGN §4.11)".into(),
            }),
            E::Cond { then, els, .. } => self.read_only(then).or_else(|| self.read_only(els)),
            // A reference a method returns is as changeable as what it was called on.
            E::Call { callee, .. } => match &callee.kind {
                E::Member { expr, .. } => self.read_only(expr),
                E::Ident(n)
                    if self.class.is_some_and(|c| {
                        self.prog.classes[c]
                            .methods
                            .iter()
                            .any(|m| &m.name == n && !m.is_static)
                    }) =>
                {
                    self.this_read_only()
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Like `read_only`, for a reference a function returns: one into its own object
    /// is fine, since the caller's access to the object decides.
    fn read_only_returned(&self, e: &Expr) -> Option<Ro> {
        match &e.kind {
            E::Paren(x) => self.read_only_returned(x),
            E::This => None,
            E::Ident(n) if self.lookup_local(n).is_none() => None,
            E::Member { expr, .. } => self.read_only_returned(expr),
            E::Index { expr, .. } | E::Slice { expr, .. } => {
                if let Some(Ty::Slice(_)) = self
                    .facts
                    .types
                    .get(&(expr.span.start, expr.span.end))
                    .map(|t| t.unref())
                {
                    return self.read_only(e);
                }
                self.read_only_returned(expr)
            }
            E::Cond { then, els, .. } => self
                .read_only_returned(then)
                .or_else(|| self.read_only_returned(els)),
            E::Call { .. } => None,
            _ => self.read_only(e),
        }
    }

    /// The reference local a write to `e` goes through, if any.
    fn ref_root(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            E::Paren(x)
            | E::Member { expr: x, .. }
            | E::Index { expr: x, .. }
            | E::Slice { expr: x, .. } => self.ref_root(x),
            E::Ident(n) => self
                .lookup_local(n)
                .filter(|&id| is_ref_ty(&self.locals[id].ty)),
            _ => None,
        }
    }

    /// Reports `action` when `e` can't be changed here.
    fn need_writable(&mut self, e: &Expr, action: &str) {
        match self.read_only(e) {
            Some(ro) => self.err_help(e.span, format!("{}: {}", action, ro.msg), ro.help),
            None => {
                if let Some(id) = self.ref_root(e)
                    && self.locals[id].write_use.is_none()
                {
                    self.locals[id].write_use = Some(e.span);
                }
            }
        }
    }

    /// A reference local is bound to `source`. Bound to something the code can't change,
    /// it can't be written through.
    fn bind_ref(&mut self, id: usize, source: &Expr) {
        let Some(ro) = self.read_only(source) else {
            return;
        };
        let name = self.locals[id].name.clone();
        if self.locals[id].write_use.is_some() {
            self.err_help(
                source.span,
                format!("this binds `{}` to something that can't be changed here, and `{}` is written through elsewhere", name, name),
                format!("{}; {}", ro.msg, ro.help),
            );
        }
        if self.locals[id].ro_bind.is_none() {
            self.locals[id].ro_bind = Some(ro.msg);
        }
    }

    /// `target .= value`, or the first `=` that binds a reference.
    fn check_rebind(&mut self, target: &Expr, value: &Expr) {
        match &target.kind {
            E::Ident(n) if self.lookup_local(n).is_some() => {
                let id = self.lookup_local(n).expect("a local");
                if let Some(ro) = self.read_only_name(n, false) {
                    self.err_help(
                        target.span,
                        format!("can't rebind `{}`: {}", n, ro.msg),
                        ro.help,
                    );
                } else if matches!(self.locals[id].ty, Ty::View(_)) {
                    self.need_writable(value, "can't make this a writable view");
                } else {
                    self.bind_ref(id, value);
                }
            }
            _ => {
                // A field or an element: the place itself must be changeable, and what it
                // will refer to too, since the reference outlives this statement.
                let place = match &target.kind {
                    E::Ident(n) => self.read_only_name(n, false),
                    _ => self.read_only(target),
                };
                match place {
                    Some(ro) => self.err_help(
                        target.span,
                        format!("can't rebind this: {}", ro.msg),
                        ro.help,
                    ),
                    None => self.need_writable(value, "can't store a reference to this"),
                }
            }
        }
    }

    /// A write to a local or field by its bare name: `@err = ...`, a destructuring target.
    fn write_name(&mut self, n: &str, span: Span) {
        if let Some(ro) = self.read_only_name(n, true) {
            self.err_help(
                span,
                format!("can't assign to `{}`: {}", n, ro.msg),
                ro.help,
            );
        }
        if self.lookup_local(n).is_some() {
            self.note_local_write(n, span);
        }
    }

    /// A lambda reads local `id`, which it captures.
    fn note_capture(&mut self, id: usize, span: Span) {
        let depth = self.locals[id].depth;
        for l in &mut self.lambdas {
            if l.0 >= depth {
                l.1 = true;
            }
        }
        let l = &mut self.locals[id];
        l.captured = true;
        if l.reassigned && !l.final_reported {
            l.final_reported = true;
            let name = l.name.clone();
            self.err_help(
                span,
                format!("this lambda captures `{}`, which is assigned again after it is initialized", name),
                "a captured variable never changes after it's initialized; copy it into a new variable for the lambda (DESIGN §9.3)",
            );
        }
    }

    /// An assignment to local `n` in the function that declared it.
    fn note_local_write(&mut self, n: &str, span: Span) {
        let Some(id) = self.lookup_local(n) else {
            return;
        };
        if self.locals[id].depth != self.fns.len() {
            return; // inside a lambda: rejected as a write to a capture
        }
        let again = match self.locals[id].slot {
            None => true,
            Some(s) => self.flow.is_maybe(s) || self.flow_loops.len() > self.locals[id].loop_depth,
        };
        if !again {
            return;
        }
        let l = &mut self.locals[id];
        l.reassigned = true;
        if l.captured && !l.final_reported {
            l.final_reported = true;
            self.err_help(
                span,
                format!("`{}` is assigned again after a lambda captured it", n),
                "a captured variable never changes after it's initialized; copy it into a new variable for the lambda (DESIGN §9.3)",
            );
        }
    }

    fn check_need(&mut self, need: &Need, span: Span, name: &str) {
        match need {
            Need::Bound(t, k) => {
                if !self.satisfies(t, &Bound::Builtin(*k)) {
                    let help = match k {
                        BoundKind::Ordered => {
                            let e = self.show(t);
                            format!(
                                "pass a comparator, such as `({e} a, {e} b) => ...`, use a `By` form, or give `{e}` a `compareTo` method and `implements ordered` (DESIGN §10.1)"
                            )
                        }
                        _ => "see STDLIB §7.1".to_string(),
                    };
                    self.err_help(
                        span,
                        format!(
                            "`{}` needs elements that are `{}`, and {} isn't",
                            name,
                            k.name(),
                            self.show(t)
                        ),
                        help,
                    );
                }
            }
            Need::Radix(t) => {
                let ok = match t.unref() {
                    Ty::Prim(p) => p.is_int() || p.is_float(),
                    Ty::Unknown => true,
                    Ty::Param(n) => self.tparam(n).is_some_and(|tp| tp.has(BoundKind::Number)),
                    t => t.is_string(),
                };
                if !ok {
                    self.err_help(
                        span,
                        format!(
                            "a radix sort's key is a number or a string, and this is {}",
                            self.show(t)
                        ),
                        "use `sort` or `sortBy` for other keys (STDLIB §7.1)",
                    );
                }
            }
        }
    }

    /// Infers a generic candidate's type arguments from the argument types (DESIGN §10).
    fn infer(&self, c: &Sig, tys: &[Option<Ty>]) -> Result<Sig, String> {
        let names: Vec<&str> = c.tparams.iter().map(|t| t.name.as_str()).collect();
        let mut binds: Vec<(String, Ty, bool)> = Vec::new();
        for (p, a) in c.params.iter().zip(tys) {
            if let Some(a) = a {
                self.unify_infer(p, a, &names, &mut binds)?;
            }
        }
        let mut map = Vec::new();
        for tp in &c.tparams {
            let ty = match binds.iter().find(|b| b.0 == tp.name) {
                Some((_, t, _)) => t.default_lit(),
                None => {
                    return Err(format!(
                        "can't tell what `{}` is from the arguments of `{}`",
                        tp_name(&tp.name),
                        c.name
                    ));
                }
            };
            for b in &tp.bounds {
                if !self.satisfies(&ty, b) {
                    let bn = match b {
                        Bound::Builtin(k) => k.name().to_string(),
                        Bound::Interface(i) => self.show(i),
                    };
                    let what = if tp.name.starts_with('#') {
                        "here".to_string()
                    } else {
                        format!("for `{}`", tp.name)
                    };
                    return Err(format!(
                        "`{}` needs a type that is `{}` {}, and {} isn't",
                        c.name,
                        bn,
                        what,
                        self.show(&ty)
                    ));
                }
            }
            map.push((tp.name.clone(), ty));
        }
        Ok(Sig {
            origin: c.origin.clone(),
            name: c.name.clone(),
            params: c.params.iter().map(|p| p.subst(&map)).collect(),
            ret: c.ret.subst(&map),
            tparams: Vec::new(),
            can_fail: c.can_fail,
            is_mut: c.is_mut,
            needs: c
                .needs
                .iter()
                .map(|n| match n {
                    Need::Bound(t, k) => Need::Bound(t.subst(&map), *k),
                    Need::Radix(t) => Need::Radix(t.subst(&map)),
                })
                .collect(),
        })
    }

    fn unify_infer(
        &self,
        p: &Ty,
        a: &Ty,
        names: &[&str],
        binds: &mut Vec<(String, Ty, bool)>,
    ) -> Result<(), String> {
        use Ty::*;
        let a = a.unref();
        let pair = |me: &Self,
                    ps: &[Ty],
                    xs: &[Ty],
                    binds: &mut Vec<(String, Ty, bool)>|
         -> Result<(), String> {
            for (p, x) in ps.iter().zip(xs) {
                me.unify_infer(p, x, names, binds)?;
            }
            Ok(())
        };
        match (p, a) {
            (_, Unknown) => Ok(()),
            (Param(n), _) if names.contains(&n.as_str()) => {
                let weak = matches!(a, IntLit { .. } | FloatLit);
                match binds.iter_mut().find(|b| &b.0 == n) {
                    None => binds.push((n.clone(), a.clone(), !weak)),
                    Some(b) => {
                        if weak {
                            if !b.2 && matches!(a, FloatLit) {
                                b.1 = FloatLit;
                            }
                        } else if !b.2 {
                            b.1 = a.clone();
                            b.2 = true;
                        } else if !self.same(&b.1, a) {
                            return Err(format!(
                                "`{}` would be both {} and {} here; no conversion is applied to make them agree (DESIGN §10)",
                                tp_name(n),
                                self.show(&b.1),
                                self.show(a)
                            ));
                        }
                    }
                }
                Ok(())
            }
            (Slice(p) | View(p) | Array(p, _), Array(x, _) | Slice(x) | View(x)) => {
                self.unify_infer(p, x, names, binds)
            }
            (Ref(p), _) => self.unify_infer(p, a, names, binds),
            (Nullable(p), Nullable(x)) => self.unify_infer(p, x, names, binds),
            (Nullable(p), _) => self.unify_infer(p, a, names, binds),
            (Class(i, ps), Class(j, xs)) if i == j => pair(self, ps, xs, binds),
            (Func(ps, r), Func(xs, xr)) if ps.len() == xs.len() => {
                pair(self, ps, xs, binds)?;
                self.unify_infer(r, xr, names, binds)
            }
            (Map(k, v), Map(xk, xv)) | (MapEntry(k, v), MapEntry(xk, xv)) => {
                self.unify_infer(k, xk, names, binds)?;
                self.unify_infer(v, xv, names, binds)
            }
            (Set(p), Set(x)) | (Pool(p), Pool(x)) | (Handle(p), Handle(x)) | (Iter(p), Iter(x)) => {
                self.unify_infer(p, x, names, binds)
            }
            (Tuple(ps), Tuple(xs)) if ps.len() == xs.len() => pair(self, ps, xs, binds),
            _ => Ok(()),
        }
    }
}

trait ClassArgs {
    fn class_args(&self) -> Vec<Ty>;
    fn flatten_nullable(self) -> Ty;
}

impl ClassArgs for Ty {
    fn class_args(&self) -> Vec<Ty> {
        match self {
            Ty::Class(_, a) => a.clone(),
            _ => Vec::new(),
        }
    }

    fn flatten_nullable(self) -> Ty {
        match self {
            Ty::Nullable(x) => match *x {
                Ty::Nullable(y) => Ty::Nullable(y),
                y => Ty::Nullable(Box::new(y)),
            },
            t => t,
        }
    }
}

/// The candidate that fits every argument at least as well as every other, and some better (DESIGN §6.5).
fn best(fits: &[(Sig, Vec<Rank>, bool)]) -> Option<usize> {
    (0..fits.len()).find(|&i| {
        (0..fits.len()).all(|j| {
            if i == j {
                return true;
            }
            let (ri, rj) = (&fits[i].1, &fits[j].1);
            let no_worse = ri.iter().zip(rj).all(|(a, b)| a <= b);
            let better = ri.iter().zip(rj).any(|(a, b)| a < b);
            no_worse && (better || (!fits[i].2 && fits[j].2))
        })
    })
}

/// Arguments whose type comes from the parameter: `null`, `[...]`, `new(...)`.
fn context_typed(e: &Expr) -> bool {
    match &e.kind {
        E::Null | E::ArrayLit(_) | E::New { class: None, .. } => true,
        E::Paren(x) => context_typed(x),
        _ => false,
    }
}

/// Expressions whose status `@` can read (DESIGN §7.3).
fn is_status_source(e: &Expr) -> bool {
    match &e.kind {
        E::Call { .. } | E::New { .. } | E::ThisNew(_) => true,
        E::Postfix { op, .. } => !matches!(op, PostfixOp::Inc | PostfixOp::Dec),
        E::Binary { op, .. } => matches!(
            op,
            BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor
        ),
        E::Paren(x) => is_status_source(x),
        _ => false,
    }
}

fn fold(op: BinOp, a: i128, b: i128) -> Option<i128> {
    use BinOp::*;
    match op {
        Add | WrapAdd | SatAdd => a.checked_add(b),
        Sub | WrapSub | SatSub => a.checked_sub(b),
        Mul | WrapMul | SatMul => a.checked_mul(b),
        Div => a.checked_div(b),
        Rem => a.checked_rem(b),
        BitAnd => Some(a & b),
        BitOr => Some(a | b),
        BitXor => Some(a ^ b),
        Shl => u32::try_from(b)
            .ok()
            .filter(|b| *b < 100)
            .and_then(|b| a.checked_mul(1i128 << b)),
        Shr => u32::try_from(b).ok().filter(|b| *b < 127).map(|b| a >> b),
        _ => None,
    }
}

fn op_text(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Or => "||",
        And => "&&",
        BitOr => "|",
        BitXor => "^",
        BitAnd => "&",
        Eq => "==",
        Ne => "!=",
        Lt => "<",
        Gt => ">",
        Le => "<=",
        Ge => ">=",
        InsertEnd => "+>",
        InsertFront => "+<",
        InsertCursor => "+^",
        Shl => "<<",
        Shr => ">>",
        Add => "+",
        Sub => "-",
        WrapAdd => "+%",
        WrapSub => "-%",
        SatAdd => "+|",
        SatSub => "-|",
        Mul => "*",
        Div => "/",
        Rem => "%",
        WrapMul => "*%",
        SatMul => "*|",
    }
}

fn compound_op(op: AssignOp) -> &'static str {
    use AssignOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Rem => "%",
        And => "&",
        Or => "|",
        Xor => "^",
        Shl => "<<",
        Shr => ">>",
        WrapAdd => "+%",
        WrapSub => "-%",
        WrapMul => "*%",
        SatAdd => "+|",
        SatSub => "-|",
        SatMul => "*|",
        Assign | Rebind => "",
    }
}

// ---------------------------------------------------------------- flow

fn is_true(e: &Expr) -> bool {
    match &e.kind {
        E::Bool(true) => true,
        E::Paren(x) => is_true(x),
        _ => false,
    }
}

fn never_returns(e: &Expr) -> bool {
    match &e.kind {
        E::Call { callee, .. } => matches!(&callee.kind, E::Ident(n) if builtins::never_returns(n)),
        _ => false,
    }
}

/// Whether running the statements can reach their end.
fn completes_all(ss: &[Stmt]) -> bool {
    ss.iter().all(completes)
}

fn completes(s: &Stmt) -> bool {
    match &s.kind {
        S::Return { .. } | S::Break(_) | S::Continue(_) | S::Yield(_) => false,
        S::Block(b) => completes_all(&b.stmts),
        S::If {
            then, els: Some(e), ..
        } => completes(then) || completes(e),
        S::While { .. } | S::For { .. } | S::DoWhile { .. } => loop_completes(s, None),
        S::Labeled { label, stmt } => loop_completes(stmt, Some(&label.name)),
        S::Switch(sw) => {
            let has_default = sw.rules.iter().any(|r| r.labels.is_none());
            !has_default
                || sw.rules.iter().any(|r| match &r.body {
                    SwitchBody::Expr(x) => !never_returns(x),
                    SwitchBody::Block(b) => completes_all(&b.stmts),
                })
        }
        S::Expr(e) => !never_returns(e),
        S::If_(b) => completes_all(active(b)),
        _ => true,
    }
}

fn loop_completes(s: &Stmt, label: Option<&str>) -> bool {
    match &s.kind {
        S::While { cond, body } => !is_true(cond) || breaks_out(body, label, 0),
        S::For { cond, body, .. } => {
            !cond.as_ref().is_none_or(is_true) || breaks_out(body, label, 0)
        }
        S::DoWhile { body, cond } => {
            if is_true(cond) {
                breaks_out(body, label, 0)
            } else {
                completes(body) || breaks_out(body, label, 0)
            }
        }
        _ => completes(s),
    }
}

/// Whether `s` has a `break` that leaves the loop it is the body of.
fn breaks_out(s: &Stmt, label: Option<&str>, depth: usize) -> bool {
    match &s.kind {
        S::Break(None) => depth == 0,
        S::Break(Some(l)) => label == Some(l.name.as_str()),
        S::While { body, .. }
        | S::For { body, .. }
        | S::DoWhile { body, .. }
        | S::ForEach { body, .. }
        | S::Repeat { body, .. } => breaks_out(body, label, depth + 1),
        _ => stmt_children(s)
            .0
            .into_iter()
            .any(|c| breaks_out(c, label, depth)),
    }
}

/// A `return`, or a `break` or `continue` that leaves `s`.
fn jumps_out(s: &Stmt, depth: usize) -> Option<Span> {
    match &s.kind {
        S::Return { .. } => Some(s.span),
        S::Break(_) | S::Continue(_) if depth == 0 => Some(s.span),
        S::While { body, .. }
        | S::For { body, .. }
        | S::DoWhile { body, .. }
        | S::ForEach { body, .. }
        | S::Repeat { body, .. } => jumps_out(body, depth + 1),
        _ => stmt_children(s)
            .0
            .into_iter()
            .find_map(|c| jumps_out(c, depth)),
    }
}

/// The statements and expressions directly inside a statement.
fn stmt_children(s: &Stmt) -> (Vec<&Stmt>, Vec<&Expr>) {
    match &s.kind {
        S::Block(b) => (b.stmts.iter().collect(), vec![]),
        S::Local(d) => (
            vec![],
            d.decls.iter().filter_map(|v| v.init.as_ref()).collect(),
        ),
        S::If { cond, then, els } => {
            let mut ss = vec![then.as_ref()];
            ss.extend(els.as_deref());
            (ss, vec![cond])
        }
        S::While { cond, body } | S::DoWhile { body, cond } => (vec![body], vec![cond]),
        S::Repeat { source, body, .. } => (vec![body], vec![source]),
        S::For {
            init,
            cond,
            update,
            body,
        } => {
            let mut es: Vec<&Expr> = Vec::new();
            match init {
                Some(ast::ForInit::Decl(d)) => {
                    es.extend(d.decls.iter().filter_map(|v| v.init.as_ref()))
                }
                Some(ast::ForInit::Exprs(xs)) => es.extend(xs.iter()),
                None => {}
            }
            es.extend(cond.iter());
            es.extend(update.iter());
            (vec![body], es)
        }
        S::ForEach { iter, body, .. } => (vec![body], vec![iter]),
        S::Switch(sw) => {
            let mut ss = Vec::new();
            let mut es = vec![sw.scrutinee.as_ref()];
            for r in &sw.rules {
                match &r.body {
                    SwitchBody::Expr(x) => es.push(x),
                    SwitchBody::Block(b) => ss.extend(b.stmts.iter()),
                }
            }
            (ss, es)
        }
        S::Return { status, value } => {
            let mut es: Vec<&Expr> = value.iter().collect();
            if let Some(ast::StatusWrite::Expr(e)) = status {
                es.push(e);
            }
            (vec![], es)
        }
        S::Destructure { value, .. } => (vec![], vec![value]),
        S::Defer(x) => (vec![x], vec![]),
        S::Yield(e) | S::Expr(e) => (vec![], vec![e]),
        S::Labeled { stmt, .. } => (vec![stmt], vec![]),
        S::If_(b) => (active(b).iter().collect(), vec![]),
        S::Break(_) | S::Continue(_) | S::Empty => (vec![], vec![]),
    }
}

/// The expressions directly inside an expression (not inside block-bodied lambdas).
fn expr_children(e: &Expr) -> Vec<&Expr> {
    fn args(a: &[ast::Arg]) -> Vec<&Expr> {
        a.iter().map(|a| &a.expr).collect()
    }
    match &e.kind {
        E::Str(parts) => parts
            .iter()
            .filter_map(|p| match p {
                StrPart::Interp(x) => Some(x),
                StrPart::Text(_) => None,
            })
            .collect(),
        E::ThisNew(a) | E::New { args: a, .. } => args(a),
        E::ArrayLit(xs) | E::Tuple(xs) => xs.iter().collect(),
        E::Switch(sw) => {
            let mut v = vec![sw.scrutinee.as_ref()];
            for r in &sw.rules {
                if let SwitchBody::Expr(x) = &r.body {
                    v.push(x);
                }
            }
            v
        }
        E::Paren(x)
        | E::StatusRead(x)
        | E::Unary { expr: x, .. }
        | E::Cast { expr: x, .. }
        | E::Postfix { expr: x, .. }
        | E::Member { expr: x, .. } => vec![x],
        E::InstanceOf { expr, .. } => vec![expr],
        E::Lambda { body, .. } => match body.as_ref() {
            LambdaBody::Expr(x) => vec![x],
            LambdaBody::Block(_) => vec![],
        },
        E::Assign { target, value, .. } => vec![target, value],
        E::StatusAssign { value, .. } => vec![value],
        E::Cond { cond, then, els } => vec![cond, then, els],
        E::Binary { lhs, rhs, .. } => vec![lhs, rhs],
        E::Call { callee, args: a } => {
            let mut v = vec![callee.as_ref()];
            v.extend(args(a));
            v
        }
        E::Index { expr, index, .. } => vec![expr, index],
        E::Slice { expr, lo, hi } => {
            let mut v = vec![expr.as_ref()];
            v.extend(lo.iter().map(|b| b.expr.as_ref()));
            v.extend(hi.iter().map(|b| b.expr.as_ref()));
            v
        }
        _ => vec![],
    }
}

fn rebound_names(s: &Stmt, out: &mut Vec<String>) {
    let (ss, es) = stmt_children(s);
    for e in es {
        rebound_in_expr(e, out);
    }
    for c in ss {
        rebound_names(c, out);
    }
}

fn rebound_in_expr(e: &Expr, out: &mut Vec<String>) {
    if let E::Assign {
        op: AssignOp::Rebind,
        target,
        ..
    } = &e.kind
        && let E::Ident(n) = &target.kind
    {
        out.push(n.clone());
    }
    for c in expr_children(e) {
        rebound_in_expr(c, out);
    }
}
