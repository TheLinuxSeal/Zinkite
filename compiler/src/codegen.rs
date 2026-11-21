//! The C backend (DESIGN §12.5, §14.2): the whole program as one C file, with the
//! runtime (`runtime/zink_rt.h`) at its top.
//!
//! First milestone: script files, classes, enums, constructors and methods,
//! statuses, references, strings and interpolation, frozen, growable and capped
//! arrays, slices, every statement, casts and the integer rules, and for-each over
//! arrays and text iterators. Everything else is reported as not supported yet.
//!
//! Layout notes: an array on a data stack is a header `{p, n}` pointing at its
//! elements, which live on a data stack of their own. Each declaration of a
//! growable local gets its own stack; since no recursive call can happen while one
//! is in scope (REFERENCES §8.4), at most one instance of it is live at a time.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::ast::{
    self, AssignOp, BinOp, Expr, ExprKind as E, LocalType, Name, PostfixOp, Stmt, StmtKind as S,
    StrPart, SwitchBody, Target, UnaryOp,
};
use crate::builtins::Origin;
use crate::check::{Facts, NameRes, key};
use crate::diag::Diagnostic;
use crate::program::{ClassKind, MethodInfo, Program, active, starts_empty};
use crate::source::{SourceFile, Span};
use crate::token::Prim;
use crate::types::*;

mod generics;
mod interfaces;
mod lambdas;
mod maps;
mod sorting;

use generics::{Inst, InstKind};
use lambdas::PendingLambda;
use maps::MapOps;
use sorting::Order;

pub const RUNTIME: &str = include_str!("../runtime/zink_rt.h");

/// Generates the C for a program that checked without errors.
pub fn generate(
    prog: &Program,
    facts: &Facts,
    sources: &[SourceFile],
) -> Result<String, Vec<(usize, Diagnostic)>> {
    let mut g = Gen::new(prog, facts, sources);
    g.program();
    if !g.errs.is_empty() {
        return Err(g.errs);
    }
    Ok(g.finish())
}

struct LoopCtx {
    label: Option<String>,
    /// C labels to jump to for `break` and `continue` with this loop's label.
    brk: String,
    cont: String,
    /// How many blocks with defers enclose the loop.
    depth: usize,
}

struct SwitchCtx {
    result: String,
    end: String,
    ty: Ty,
}

struct Gen<'p, 'a> {
    prog: &'p Program<'a>,
    facts: &'p Facts,
    sources: &'p [SourceFile],
    errs: Vec<(usize, Diagnostic)>,
    reported: HashSet<(u32, u32)>,
    /// Struct definitions: name, body, and the structs it holds by value.
    defs: Vec<(String, String, Vec<String>)>,
    def_names: HashMap<String, String>,
    protos: String,
    bodies: String,
    statics_init: String,
    next_site: u32,
    tmp: u32,
    // the function being generated
    file: usize,
    class: ClassId,
    ret: Ty,
    fails: bool,
    is_ctor: bool,
    is_main: bool,
    has_this: bool,
    locals: HashMap<(u32, u32), (String, Ty)>,
    out: String,
    indent: usize,
    defers: Vec<Vec<Stmt>>,
    loops: Vec<LoopCtx>,
    switches: Vec<SwitchCtx>,
    pending_label: Option<String>,
    used_scratch: bool,
    /// The call about to be generated has its array result copied into a variable at once,
    /// so it needn't be kept on the scratch stack (see `call`).
    direct_result: bool,
    /// The type of the value the last `with_status` evaluated.
    last_status_ret: Ty,
    /// Function-pointer typedefs, by signature, and their text.
    fn_types: HashMap<String, String>,
    fn_typedefs: String,
    /// Lambdas used as values, still to generate, and the C names given to them.
    pending_lambdas: Vec<PendingLambda>,
    lambda_names: HashMap<String, String>,
    /// Inlined lambda block bodies being generated: result, end label, result type.
    inline_ret: Vec<(String, String, Ty)>,
    /// Each `hashmap` and `hashset` type's operations, and their C text.
    maps: HashMap<String, MapOps>,
    helpers: String,
    /// The instantiation being generated: its type parameters bound, and the type
    /// arguments of its class (DESIGN §10).
    subst: Vec<(String, Ty)>,
    cur_cargs: Vec<Ty>,
    class_insts: HashMap<String, String>,
    fn_insts: HashMap<String, String>,
    inst_queue: Vec<Inst>,
    /// Interface instantiations: their reference struct and method table struct.
    iface_refs: HashMap<String, (String, String)>,
    /// Method tables, by class and interface.
    vtables: HashMap<String, String>,
}

fn c_string(s: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in s {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => {
                if b == b'?' {
                    out.push_str("\\?");
                } else {
                    out.push(b as char);
                }
            }
            _ => {
                let _ = write!(out, "\\{:03o}", b);
            }
        }
    }
    out.push('"');
    out
}

fn lit(s: &str) -> String {
    format!("zr_lit({}, {})", c_string(s.as_bytes()), s.len())
}

fn prim_c(p: Prim) -> &'static str {
    match p {
        Prim::Bool => "bool",
        Prim::I8 => "int8_t",
        Prim::I16 => "int16_t",
        Prim::I32 => "int32_t",
        Prim::I64 => "int64_t",
        Prim::U8 => "uint8_t",
        Prim::U16 => "uint16_t",
        Prim::U32 => "uint32_t",
        Prim::U64 => "uint64_t",
        Prim::I128 => "zr_i128",
        Prim::U128 => "zr_u128",
        Prim::F32 => "float",
        Prim::F64 => "double",
    }
}

fn unsigned_of(p: Prim) -> &'static str {
    match p {
        Prim::I8 | Prim::U8 => "uint8_t",
        Prim::I16 | Prim::U16 => "uint16_t",
        Prim::I32 | Prim::U32 => "uint32_t",
        Prim::I128 | Prim::U128 => "zr_u128",
        _ => "uint64_t",
    }
}

/// Integer temporaries wide enough for a conversion between `a` and `b`.
fn wide_temps(a: Prim, b: Prim) -> (&'static str, &'static str) {
    if bits(a) == 128 || bits(b) == 128 {
        ("zr_i128", "zr_u128")
    } else {
        ("int64_t", "uint64_t")
    }
}

fn min_max(p: Prim) -> (String, String) {
    let (lo, hi) = match p {
        Prim::I8 => ("INT8_MIN", "INT8_MAX"),
        Prim::I16 => ("INT16_MIN", "INT16_MAX"),
        Prim::I32 => ("INT32_MIN", "INT32_MAX"),
        Prim::I64 => ("INT64_MIN", "INT64_MAX"),
        Prim::U8 => ("0", "UINT8_MAX"),
        Prim::U16 => ("0", "UINT16_MAX"),
        Prim::U32 => ("0", "UINT32_MAX"),
        Prim::U64 => ("0", "UINT64_MAX"),
        Prim::I128 => ("ZR_I128_MIN", "ZR_I128_MAX"),
        Prim::U128 => ("0", "ZR_U128_MAX"),
        Prim::F32 => ("-FLT_MAX", "FLT_MAX"),
        Prim::F64 => ("-DBL_MAX", "DBL_MAX"),
        Prim::Bool => ("0", "1"),
    };
    (lo.to_string(), hi.to_string())
}

/// An array whose header points at elements on a data stack, or a map or set on one.
fn on_stack(t: &Ty) -> bool {
    matches!(t, Ty::Array(_, k) if k.cap != CapKind::Capped)
        || matches!(t, Ty::Map(..) | Ty::Set(_))
}

fn is_ref(t: &Ty) -> bool {
    matches!(t, Ty::Ref(_)) || matches!(t, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)))
}

/// A literal's type once nothing else decides it.
fn settle(t: &Ty) -> Ty {
    match t {
        Ty::IntLit { .. } | Ty::FloatLit => t.default_lit(),
        t => t.clone(),
    }
}

impl<'p, 'a> Gen<'p, 'a> {
    fn new(prog: &'p Program<'a>, facts: &'p Facts, sources: &'p [SourceFile]) -> Self {
        Gen {
            prog,
            facts,
            sources,
            errs: Vec::new(),
            reported: HashSet::new(),
            defs: Vec::new(),
            def_names: HashMap::new(),
            protos: String::new(),
            bodies: String::new(),
            statics_init: String::new(),
            next_site: 2,
            tmp: 0,
            file: 0,
            class: 0,
            ret: Ty::Void,
            fails: false,
            is_ctor: false,
            is_main: false,
            has_this: false,
            locals: HashMap::new(),
            out: String::new(),
            indent: 1,
            defers: Vec::new(),
            loops: Vec::new(),
            switches: Vec::new(),
            pending_label: None,
            used_scratch: false,
            direct_result: false,
            last_status_ret: Ty::Void,
            fn_types: HashMap::new(),
            fn_typedefs: String::new(),
            pending_lambdas: Vec::new(),
            lambda_names: HashMap::new(),
            inline_ret: Vec::new(),
            maps: HashMap::new(),
            helpers: String::new(),
            subst: Vec::new(),
            cur_cargs: Vec::new(),
            class_insts: HashMap::new(),
            fn_insts: HashMap::new(),
            inst_queue: Vec::new(),
            iface_refs: HashMap::new(),
            vtables: HashMap::new(),
        }
    }

    fn unsupported(&mut self, span: Span, what: &str) -> String {
        if self.reported.insert((span.start, span.end)) {
            let d = Diagnostic::error(span, format!("the C backend doesn't support {} yet", what))
                .with_help(
                    "`zink check` accepts it; code generation for it comes in a later milestone",
                );
            self.errs.push((self.file, d));
        }
        "0".into()
    }

    /// `T.parse(s, base)` as C: a result struct of T and the status.
    fn parse_code(&mut self, p: Prim, s: &str, base: &str, span: Span) -> String {
        let code = if p.is_float() {
            format!(
                "({{ zr_res_f64 _r = zr_parse_f64({}); {}; }})",
                s,
                if p == Prim::F32 {
                    "(zr_r_float){(float)_r.v, _r.s}"
                } else {
                    "_r"
                }
            )
        } else if bits(p) == 128 {
            let f = if signed(p) {
                "zr_parse_i128"
            } else {
                "zr_parse_u128"
            };
            format!("{}({}, {})", f, s, base)
        } else if signed(p) {
            let (lo, hi) = min_max(p);
            let rt = self.result_ty(&Ty::Prim(p), span);
            format!(
                "({{ zr_res_i64 _r = zr_parse_i64({}, {}); if (!_r.s && (_r.v < {} || _r.v > {})) {{ _r.v = 0; _r.s = 34; }} ({}){{({})_r.v, _r.s}}; }})",
                s,
                base,
                lo,
                hi,
                rt,
                prim_c(p)
            )
        } else {
            let (_, hi) = min_max(p);
            let rt = self.result_ty(&Ty::Prim(p), span);
            format!(
                "({{ zr_res_u64 _r = zr_parse_u64({}, {}); if (!_r.s && _r.v > {}) {{ _r.v = 0; _r.s = 34; }} ({}){{({})_r.v, _r.s}}; }})",
                s,
                base,
                hi,
                rt,
                prim_c(p)
            )
        };
        if p == Prim::F32 {
            self.result_ty(&Ty::Prim(Prim::F32), span);
        }
        code
    }

    /// An argument as it crosses into C (DESIGN §12.6): text C reads as NUL-terminated
    /// is copied onto the scratch stack with the NUL; any other slice passes its pointer.
    fn c_arg(&mut self, v: String, ty: &Ty, nul: bool) -> String {
        match ty {
            Ty::Slice(e) if nul && **e == Ty::u8() => {
                self.used_scratch = true;
                format!("zr_cstr({})", v)
            }
            Ty::Slice(_) | Ty::View(_) => format!("(({}).p)", v),
            _ => v,
        }
    }

    /// The prototype of an explicit `extern` declaration: the C symbol under the
    /// function's own name, through an asm label, so a header that declares it
    /// differently doesn't clash.
    fn extern_proto(&mut self, id: ClassId, i: usize, m: &MethodInfo, cname: &str) {
        // The runtime's own functions are declared by the runtime.
        if cname.starts_with("zr_") {
            return;
        }
        let span = m.decl.name.span;
        let mut ps = Vec::new();
        for (d, t) in m.decl.params.iter().zip(&m.params) {
            let c = match t {
                Ty::Slice(_) if matches!(d.ty.kind, ast::TypeKind::String) => {
                    "const char *".to_string()
                }
                Ty::Slice(e) => format!("const {} *", self.cty(e, span)),
                Ty::View(e) => format!("{} *", self.cty(e, span)),
                Ty::CHandle(_) => "void *".to_string(),
                t => self.cty(t, span),
            };
            ps.push(c);
        }
        let ret = self.cty(&m.ret, span);
        let ps = if ps.is_empty() {
            "void".to_string()
        } else {
            ps.join(", ")
        };
        let _ = writeln!(
            self.protos,
            "extern {} {}({}) __asm__({});",
            ret,
            self.fn_name(id, i),
            ps,
            c_string(cname.as_bytes())
        );
    }

    /// An error in the program that only instantiating it shows, such as a runaway
    /// instantiation (DESIGN §10).
    fn instance_error(&mut self, span: Span, msg: &str, help: &str) -> String {
        if self.reported.insert((span.start, span.end)) {
            let d = Diagnostic::error(span, msg).with_help(help);
            self.errs.push((self.file, d));
        }
        "0".into()
    }

    fn tmp(&mut self, base: &str) -> String {
        self.tmp += 1;
        format!("_{}{}", base, self.tmp)
    }

    fn loc(&self, span: Span) -> String {
        let src = &self.sources[self.file];
        let (line, _) = src.line_col(span.start as usize);
        format!("{}, {}", c_string(src.name.as_bytes()), line)
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn ty(&self, e: &Expr) -> Ty {
        self.ty_at(e.span)
    }

    /// The type the checker recorded at a span, in the current instantiation.
    fn ty_at(&self, span: Span) -> Ty {
        match self.facts.types.get(&key(span)) {
            Some(t) => self.sb(t),
            None => Ty::Unknown,
        }
    }

    // ------------------------------------------------------------ types

    fn class_name(&self, id: ClassId) -> String {
        format!("zc{}_{}", id, self.prog.classes[id].name)
    }

    fn cty(&mut self, t: &Ty, span: Span) -> String {
        match t {
            Ty::Prim(p) => prim_c(*p).into(),
            Ty::Void => "void".into(),
            Ty::IntLit { is_char: true, .. } => "uint32_t".into(),
            Ty::IntLit { .. } => "int64_t".into(),
            Ty::FloatLit => "double".into(),
            Ty::Slice(_) | Ty::View(_) => "zr_slice".into(),
            Ty::Ref(x) => match &**x {
                // Two pointers: the value and its class's table (DESIGN §6.3).
                Ty::Class(id, args) if self.prog.classes[*id].kind == ClassKind::Interface => {
                    self.iface_ref(*id, args, span)
                }
                _ => format!("{}*", self.cty(x, span)),
            },
            Ty::Nullable(x) => self.cty(x, span),
            Ty::Class(id, args) => {
                let c = &self.prog.classes[*id];
                match c.kind {
                    ClassKind::Enum => "int32_t".into(),
                    ClassKind::Interface => self.unsupported(span, "interfaces"),
                    _ if !args.is_empty() => self.class_inst(*id, args, span),
                    _ => self.class_name(*id),
                }
            }
            Ty::Array(e, k) => {
                if k.cap != CapKind::Capped {
                    if k.cap == CapKind::Chunked {
                        return "zr_chk".into();
                    }
                    if k.has_cursor() {
                        return "zr_cur".into();
                    }
                    if k.push_front() || k.pop_front() {
                        return "zr_deq".into();
                    }
                    return "zr_vec".into();
                }
                if k.size == 0 {
                    return self.unsupported(span, "array capacities given by a constant");
                }
                let ec = self.cty(e, span);
                let c = counter(k.size);
                let (prefix, fields) = match layout(t) {
                    Some(Layout::Ring) => ("zring", format!("{c} h; {c} n;", c = c)),
                    Some(Layout::Gap) => ("zgap", format!("{c} gs; {c} an; {c} cur;", c = c)),
                    _ => ("zcap", format!("{} n;", c)),
                };
                let name = format!("{}_{}_{}", prefix, ec.replace('*', "p"), k.size);
                if !self.def_names.contains_key(&name) {
                    let deps = self.deps(e);
                    self.def_names.insert(name.clone(), name.clone());
                    self.defs.push((
                        name.clone(),
                        format!("{} {} e[{}];", fields, ec, k.size),
                        deps,
                    ));
                }
                name
            }
            Ty::Tuple(ts) => {
                let cs: Vec<String> = ts.iter().map(|t| self.cty(t, span)).collect();
                let name = format!("zt_{}", cs.join("_").replace('*', "p"));
                if !self.def_names.contains_key(&name) {
                    let mut deps = Vec::new();
                    for t in ts {
                        deps.extend(self.deps(t));
                    }
                    let body: String = cs
                        .iter()
                        .enumerate()
                        .map(|(i, c)| format!("{} a{}; ", c, i))
                        .collect();
                    self.def_names.insert(name.clone(), name.clone());
                    self.defs.push((name.clone(), body, deps));
                }
                name
            }
            Ty::Null => "void*".into(),
            Ty::CHandle(_) => "void*".into(),
            Ty::Func(ps, r) => self.fn_type(ps, r, span),
            Ty::Map(..) | Ty::Set(_) => "zr_map".into(),
            Ty::MapEntry(..) => match self.map_ops(t, span) {
                Some(ops) => ops.entry,
                None => "int".into(),
            },
            Ty::Pool(_) | Ty::Handle(_) => self.unsupported(span, "pools"),
            Ty::Param(_) => self.unsupported(span, "generics"),
            _ => self.unsupported(span, "this library type"),
        }
    }

    /// The structs a value of this type holds by value, which must be defined first.
    fn deps(&mut self, t: &Ty) -> Vec<String> {
        match t {
            Ty::Class(id, args)
                if matches!(
                    self.prog.classes[*id].kind,
                    ClassKind::Class | ClassKind::Root
                ) =>
            {
                if args.is_empty() {
                    vec![self.class_name(*id)]
                } else {
                    vec![self.class_inst(*id, args, Span::default())]
                }
            }
            Ty::Array(_, k) if k.cap == CapKind::Capped => {
                let c = self.cty(t, Span::default());
                vec![c]
            }
            Ty::Tuple(_) => {
                let c = self.cty(t, Span::default());
                vec![c]
            }
            // An interface reference is a struct held by value.
            Ty::Ref(_) | Ty::Nullable(_) if interfaces::iface_target(self.prog, t).is_some() => {
                let c = self.cty(t, Span::default());
                vec![c]
            }
            _ => Vec::new(),
        }
    }

    /// `{value, status}` for a function that can fail (DESIGN §12.5).
    fn result_ty(&mut self, t: &Ty, span: Span) -> String {
        match t {
            Ty::Prim(Prim::I64) => return "zr_res_i64".into(),
            Ty::Prim(Prim::U64) => return "zr_res_u64".into(),
            Ty::Prim(Prim::F64) => return "zr_res_f64".into(),
            Ty::Prim(Prim::I128) => return "zr_res_i128".into(),
            Ty::Prim(Prim::U128) => return "zr_res_u128".into(),
            _ => {}
        }
        let c = self.cty(t, span);
        let name = format!("zr_r_{}", c.replace('*', "p"));
        if !self.def_names.contains_key(&name) {
            let deps = self.deps(t);
            self.def_names.insert(name.clone(), name.clone());
            self.defs
                .push((name.clone(), format!("{} v; uint8_t s;", c), deps));
        }
        name
    }

    fn elem_size(&mut self, t: &Ty, span: Span) -> String {
        let c = self.cty(t, span);
        format!("sizeof({})", c)
    }

    // ------------------------------------------------------------ the program

    fn program(&mut self) {
        let prog = self.prog;
        // Struct definitions for every class.
        for (id, c) in prog.classes.iter().enumerate() {
            if !matches!(c.kind, ClassKind::Class | ClassKind::Root) || !c.type_params.is_empty() {
                continue;
            }
            self.file = c.file;
            let (body, deps) = self.struct_body(id, &[]);
            let name = self.class_name(id);
            self.def_names.insert(name.clone(), name.clone());
            self.defs.push((name, body, deps));
        }
        // Statics.
        for (id, c) in prog.classes.iter().enumerate() {
            self.file = c.file;
            for (i, f) in c.fields.iter().enumerate() {
                if !f.is_static {
                    continue;
                }
                let ct = self.cty(&f.ty, f.span);
                let name = format!("zs{}_{}", id, i);
                let _ = writeln!(self.protos, "static {} {};", ct, name);
                if on_stack(&f.ty) {
                    let init = self.stack_init(&f.ty);
                    let _ = writeln!(self.statics_init, "    {} = {};", name, init);
                }
                if let Some(init) = f.init {
                    self.begin_fn(id, Ty::Void, false, false, true);
                    let code = self.store_into(&name, &f.ty, init);
                    let body = std::mem::take(&mut self.out);
                    self.statics_init.push_str(&body);
                    let _ = writeln!(self.statics_init, "    {};", code);
                }
            }
        }
        self.drain_lambdas();
        // Functions.
        for (id, c) in prog.classes.iter().enumerate() {
            if !c.type_params.is_empty() {
                continue;
            }
            self.file = c.file;
            for (i, m) in c.methods.iter().enumerate() {
                if let Some(cn) = &m.c_name {
                    self.extern_proto(id, i, m, cn);
                    continue;
                }
                if m.decl.body.is_none() {
                    continue;
                }
                if !m.type_params.is_empty() {
                    continue;
                }
                self.method(id, i, m);
                self.drain_lambdas();
            }
            for (i, k) in c.ctors.iter().enumerate() {
                if matches!(c.kind, ClassKind::Class | ClassKind::Root) {
                    self.ctor(id, i, k);
                    self.drain_lambdas();
                }
            }
        }
        self.drain_insts();
    }

    fn site(&mut self) -> u32 {
        let s = self.next_site;
        self.next_site += 1;
        s
    }

    fn begin_fn(&mut self, class: ClassId, ret: Ty, fails: bool, has_this: bool, is_ctor: bool) {
        self.class = class;
        self.ret = ret;
        self.fails = fails;
        self.has_this = has_this;
        self.is_ctor = is_ctor;
        self.is_main = false;
        self.locals.clear();
        self.out.clear();
        self.indent = 1;
        self.defers.clear();
        self.loops.clear();
        self.switches.clear();
        self.used_scratch = false;
    }

    fn fn_name(&self, class: ClassId, idx: usize) -> String {
        format!(
            "zm{}_{}_{}",
            class, idx, self.prog.classes[class].methods[idx].name
        )
    }

    fn ctor_name(&self, class: ClassId, idx: usize) -> String {
        format!("zk{}_{}", class, idx)
    }

    fn ret_c(&mut self, ret: &Ty, fails: bool, span: Span) -> String {
        match (fails, ret) {
            (true, Ty::Void) => "uint8_t".into(),
            (true, t) => self.result_ty(t, span),
            (false, t) => self.cty(t, span),
        }
    }

    fn params_c(&mut self, decl: &[ast::Param], tys: &[Ty]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, (p, t)) in decl.iter().zip(tys).enumerate() {
            let ct = self.cty(t, p.ty.span);
            let name = match &p.name {
                Name::Ident(id) => {
                    let n = format!("p_{}_{}", id.name, i);
                    self.locals.insert(key(id.span), (n.clone(), t.clone()));
                    n
                }
                Name::Discard(_) => format!("p__{}", i),
            };
            out.push(format!("{} {}", ct, name));
        }
        out
    }

    fn method(&mut self, id: ClassId, idx: usize, m: &MethodInfo) {
        let name = self.fn_name(id, idx);
        self.method_as(id, idx, m, name);
    }

    /// Method `idx` of class `id` as the C function `name`, in the current instantiation.
    fn method_as(&mut self, id: ClassId, _idx: usize, m: &MethodInfo, name: String) {
        let c = &self.prog.classes[id];
        let span = m.decl.name.span;
        let m_ret = self.sb(&m.ret);
        self.begin_fn(id, m_ret.clone(), m.can_fail, !m.is_static, false);
        self.is_main = c.kind == ClassKind::Script && m.name == "main";
        let ret = self.ret_c(&m_ret, m.can_fail, span);
        let mut params = Vec::new();
        if !m.is_static {
            let st = self.cty(&self.this_class(), span);
            params.push(format!("{}* self", st));
        }
        let ptys: Vec<Ty> = m.params.iter().map(|t| self.sb(t)).collect();
        params.extend(self.params_c(&m.decl.params, &ptys));
        let sig = format!(
            "static {} {}({})",
            ret,
            name,
            if params.is_empty() {
                "void".to_string()
            } else {
                params.join(", ")
            }
        );
        let _ = writeln!(self.protos, "{};", sig);
        let body = m.decl.body.as_ref().expect("a body");
        self.block_stmts(&body.stmts);
        // Falling off the end returns status 0, and 0 from `u8 main` (DESIGN §7.2, §8.6).
        if (self.fails && m_ret == Ty::Void) || (self.is_main && m_ret == Ty::u8()) {
            self.line("return 0;");
        }
        let out = std::mem::take(&mut self.out);
        let _ = writeln!(self.bodies, "{} {{\n{}}}\n", sig, out);
        if self.is_main {
            let call = if m.params.is_empty() {
                format!("{}()", name)
            } else {
                format!("{}(zr_args)", name)
            };
            let body = if m.ret == Ty::u8() {
                format!("    int status = {};", call)
            } else {
                format!("    {};\n    int status = 0;", call)
            };
            let _ = writeln!(
                self.bodies,
                "int main(int argc, char **argv) {{\n    zr_start(argc, argv);\n    zr_init_statics();\n{}\n    return zr_finish(status);\n}}\n",
                body
            );
        }
    }

    fn ctor(&mut self, id: ClassId, idx: usize, k: &crate::program::CtorInfo) {
        let name = self.ctor_name(id, idx);
        self.ctor_as(id, idx, k, name);
    }

    /// Constructor `idx` of class `id` as the C function `name`, in the current
    /// instantiation.
    fn ctor_as(&mut self, id: ClassId, _idx: usize, k: &crate::program::CtorInfo, name: String) {
        let c = &self.prog.classes[id];
        let span = c.span;
        self.begin_fn(id, Ty::Void, k.can_fail, true, true);
        let ret = if k.can_fail { "uint8_t" } else { "void" };
        let st = self.cty(&self.this_class(), span);
        let mut params = vec![format!("{}* self", st)];
        let cargs = self.cur_cargs.clone();
        let fields: Vec<(String, Ty, Option<&Expr>)> = c
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.is_static)
            .map(|(i, f)| (f.name.clone(), self.field_ty(id, i, &cargs), f.init))
            .collect();
        let kparams: Vec<Ty> = k.params.iter().map(|t| self.sb(t)).collect();
        match k.decl {
            Some(d) => params.extend(self.params_c(&d.params, &kparams)),
            None => {
                for (i, t) in kparams.iter().enumerate() {
                    let ct = self.cty(t, span);
                    params.push(format!("{} a{}", ct, i));
                }
            }
        }
        let sig = format!("static {} {}({})", ret, name, params.join(", "));
        let _ = writeln!(self.protos, "{};", sig);
        // Field initializers run first; growable fields start empty, set up by whoever
        // gave the object its storage.
        for (fname, ty, init) in &fields {
            if let Some(init) = init {
                let code = self.store_into(&format!("self->f_{}", fname), ty, init);
                self.line(&format!("{};", code));
            }
        }
        match k.decl {
            Some(d) => {
                self.block_stmts(&d.body.stmts);
                if k.can_fail {
                    self.line("return 0;");
                }
            }
            None => {
                let mut i = 0;
                for (fname, ty, init) in &fields {
                    if init.is_none() && !starts_empty(ty) {
                        self.line(&format!("self->f_{} = a{};", fname, i));
                        i += 1;
                    }
                }
            }
        }
        let out = std::mem::take(&mut self.out);
        let _ = writeln!(self.bodies, "{} {{\n{}}}\n", sig, out);
    }

    fn finish(mut self) -> String {
        let mut out = String::new();
        // C libraries to link, read by the driver (DESIGN §12.6).
        let mut links: Vec<&str> = Vec::new();
        let mut headers: Vec<&str> = Vec::new();
        for f in &self.prog.files {
            for l in &f.links {
                if !links.contains(&l.as_str()) {
                    links.push(l);
                }
            }
            if let Some(c) = &f.c {
                for h in &c.headers {
                    if !headers.contains(&h.as_str()) {
                        headers.push(h);
                    }
                }
            }
        }
        for l in &links {
            let _ = writeln!(out, "{}{} */", crate::driver::LINK_MARK, l);
        }
        out.push_str("/* Generated by zink. */\n");
        out.push_str(RUNTIME);
        out.push_str("\n#include <float.h>\n");
        for h in &headers {
            let _ = writeln!(out, "#include <{}>", h);
        }
        out.push('\n');
        for (name, _, _) in &self.defs {
            let _ = writeln!(out, "typedef struct {} {};", name, name);
        }
        // Function pointers may name the structs before they are defined.
        out.push_str(&self.fn_typedefs);
        // Definitions, each after the structs it holds by value.
        let mut done: HashSet<String> = HashSet::new();
        let defs = std::mem::take(&mut self.defs);
        let index: HashMap<String, usize> = defs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.0.clone(), i))
            .collect();
        fn emit(
            i: usize,
            defs: &[(String, String, Vec<String>)],
            index: &HashMap<String, usize>,
            done: &mut HashSet<String>,
            out: &mut String,
        ) {
            if !done.insert(defs[i].0.clone()) {
                return;
            }
            for d in &defs[i].2 {
                if let Some(&j) = index.get(d) {
                    emit(j, defs, index, done, out);
                }
            }
            let _ = writeln!(out, "struct {} {{ {}}};", defs[i].0, defs[i].1);
        }
        for i in 0..defs.len() {
            emit(i, &defs, &index, &mut done, &mut out);
        }
        out.push('\n');
        out.push_str(&self.protos);
        out.push('\n');
        out.push_str(&self.helpers);
        let _ = writeln!(
            out,
            "\nstatic void zr_init_statics(void) {{\n{}}}\n",
            self.statics_init
        );
        out.push_str(&self.bodies);
        out
    }

    // ------------------------------------------------------------ statements

    fn block_stmts(&mut self, stmts: &[Stmt]) {
        self.defers.push(Vec::new());
        for s in stmts {
            self.stmt(s);
        }
        let ds = self.defers.pop().unwrap_or_default();
        if stmts
            .last()
            .is_none_or(|s| !matches!(s.kind, S::Return { .. } | S::Break(_) | S::Continue(_)))
        {
            for d in ds.iter().rev() {
                self.stmt(d);
            }
        }
    }

    fn block(&mut self, b: &ast::Block) {
        self.line("{");
        self.indent += 1;
        self.block_stmts(&b.stmts);
        self.indent -= 1;
        self.line("}");
    }

    /// A statement as a block of its own, for loop and `if` bodies.
    fn body(&mut self, s: &Stmt) {
        match &s.kind {
            S::Block(b) => self.block(b),
            _ => {
                self.line("{");
                self.indent += 1;
                self.block_stmts(std::slice::from_ref(s));
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn line_directive(&mut self, span: Span) {
        let src = &self.sources[self.file];
        let (line, _) = src.line_col(span.start as usize);
        let d = format!("#line {} {}", line, c_string(src.name.as_bytes()));
        self.out.push_str(&d);
        self.out.push('\n');
    }

    /// Runs the defers of the innermost `n` blocks, innermost first (DESIGN §6.4).
    fn run_defers(&mut self, from: usize) {
        let blocks: Vec<Vec<Stmt>> = self.defers[from..].iter().rev().cloned().collect();
        for b in blocks {
            for d in b.iter().rev() {
                self.stmt(d);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        self.line_directive(s.span);
        let label = self.pending_label.take();
        // Temporaries on the scratch stack live until the end of their statement.
        let simple = matches!(
            s.kind,
            S::Local(_) | S::Expr(_) | S::Destructure { .. } | S::Return { .. }
        );
        let mark = if simple {
            let m = self.tmp("sm");
            self.used_scratch = false;
            let at = self.out.len();
            Some((m, at))
        } else {
            None
        };
        match &s.kind {
            S::Block(b) => self.block(b),
            S::Local(d) => self.local(d),
            S::If { cond, then, els } => {
                let c = self.cond(cond);
                self.line(&format!("if ({})", c));
                self.body(then);
                if let Some(e) = els {
                    self.line("else");
                    self.body(e);
                }
            }
            S::While { cond, body } => {
                let c = self.cond(cond);
                self.loop_(label, &format!("while ({})", c), body, None);
            }
            S::DoWhile { body, cond } => {
                let lp = self.enter_loop(label);
                self.line("do {");
                self.indent += 1;
                self.body(body);
                self.line(&format!("{}: ;", lp.cont));
                self.indent -= 1;
                let c = self.cond(cond);
                self.line(&format!("}} while ({});", c));
                self.leave_loop(lp);
            }
            S::For {
                init,
                cond,
                update,
                body,
            } => {
                self.line("{");
                self.indent += 1;
                match init {
                    Some(ast::ForInit::Decl(d)) => self.local(d),
                    Some(ast::ForInit::Exprs(es)) => {
                        for e in es {
                            let c = self.expr_stmt_code(e);
                            self.line(&format!("{};", c));
                        }
                    }
                    None => {}
                }
                let c = cond.as_ref().map(|c| self.cond(c)).unwrap_or_default();
                let ups: Vec<String> = update.iter().map(|u| self.expr_stmt_code(u)).collect();
                self.loop_(
                    label,
                    &format!("for (; {}; {})", c, ups.join(", ")),
                    body,
                    None,
                );
                self.indent -= 1;
                self.line("}");
            }
            S::ForEach {
                ty,
                name,
                iter,
                body,
            } => self.for_each(label, ty, name, iter, body),
            S::Repeat {
                target,
                source,
                capture,
                body,
            } => self.repeat(label, target, source, capture.as_ref(), body),
            S::Switch(sw) => self.switch_stmt(sw),
            S::Break(l) | S::Continue(l) => {
                let is_break = matches!(s.kind, S::Break(_));
                let i = match l {
                    None => self.loops.len().checked_sub(1),
                    Some(n) => self
                        .loops
                        .iter()
                        .rposition(|x| x.label.as_deref() == Some(&n.name)),
                };
                if let Some(i) = i {
                    let depth = self.loops[i].depth;
                    self.run_defers(depth);
                    let target = if is_break {
                        self.loops[i].brk.clone()
                    } else {
                        self.loops[i].cont.clone()
                    };
                    self.line(&format!("goto {};", target));
                }
            }
            S::Return { status, value } => self.return_(status.as_ref(), value.as_ref(), s.span),
            S::Destructure {
                targets,
                capture,
                value,
            } => self.destructure(targets, capture.as_ref(), value),
            S::Defer(inner) => {
                if let Some(d) = self.defers.last_mut() {
                    d.push((**inner).clone());
                }
            }
            S::Yield(e) => {
                if let Some(sw) = self.switches.last() {
                    let (res, end, ty) = (sw.result.clone(), sw.end.clone(), sw.ty.clone());
                    let v = self.expr(e, &ty);
                    self.line(&format!("{} = {}; goto {};", res, v, end));
                }
            }
            S::Labeled { label, stmt } => {
                self.pending_label = Some(label.name.clone());
                self.stmt(stmt);
            }
            S::Expr(e) => {
                let c = self.expr_stmt_code(e);
                self.line(&format!("{};", c));
            }
            S::If_(b) => {
                for st in active(b) {
                    self.stmt(st);
                }
            }
            S::Empty => {}
        }
        if let Some((m, at)) = mark
            && self.used_scratch
        {
            let start = format!("{}int64_t {} = zr_sm();\n", "    ".repeat(self.indent), m);
            self.out.insert_str(at, &start);
            self.line(&format!("zr_sr({});", m));
            self.used_scratch = false;
        }
    }

    fn enter_loop(&mut self, label: Option<String>) -> LoopCtx {
        let n = self.tmp("L");
        LoopCtx {
            label,
            brk: format!("{}_brk", n),
            cont: format!("{}_cont", n),
            depth: self.defers.len(),
        }
    }

    fn leave_loop(&mut self, lp: LoopCtx) {
        self.line(&format!("{}: ;", lp.brk));
    }

    /// A C loop whose body jumps to `cont` for `continue` and `brk` for `break`.
    fn loop_(
        &mut self,
        label: Option<String>,
        head: &str,
        body: &Stmt,
        before: Option<&dyn Fn(&mut Self)>,
    ) {
        let lp = self.enter_loop(label);
        let (brk, cont) = (lp.brk.clone(), lp.cont.clone());
        self.loops.push(lp);
        self.line(&format!("{} {{", head));
        self.indent += 1;
        if let Some(f) = before {
            f(self);
        }
        self.body(body);
        self.line(&format!("{}: ;", cont));
        self.indent -= 1;
        self.line("}");
        self.loops.pop();
        self.line(&format!("{}: ;", brk));
    }

    fn new_local(&mut self, name: &ast::Ident) -> (String, Ty) {
        let ty = self
            .facts
            .locals
            .get(&key(name.span))
            .map(|t| self.sb(t))
            .unwrap_or(Ty::Unknown);
        let c = self.tmp(&format!("l_{}_", name.name));
        self.locals.insert(key(name.span), (c.clone(), ty.clone()));
        (c, ty)
    }

    fn local(&mut self, d: &ast::LocalDecl) {
        for v in &d.decls {
            let (name, ty) = self.new_local(&v.name);
            let ct = self.cty(&ty, v.name.span);
            if on_stack(&ty) {
                let init = self.stack_init(&ty);
                self.line(&format!("{} {} = {};", ct, name, init));
                match (&v.init, &v.capture) {
                    (Some(init), Some(Name::Ident(c))) => match (&ty, layout(&ty)) {
                        // A failing call's array comes with its status, then is copied in
                        // as store_into copies a slice: `u8[.] p @err = path.resolve(x);`.
                        (Ty::Array(el, _), Some(Layout::Vec)) => {
                            let el = (**el).clone();
                            let (cn, _) = self.new_local(c);
                            self.direct_result = matches!(strip_paren(init).kind, E::Call { .. });
                            let (val, st) =
                                self.with_status(init, &Ty::Slice(Box::new(el.clone())));
                            let sz = self.elem_size(&el, init.span);
                            self.line(&format!("uint8_t {};", cn));
                            self.line(&format!(
                                "({{ {} zr_slice _s = {}; {} = {}; memmove({n}.p, _s.p, (size_t)_s.n * {}); {n}.n = _s.n; }});",
                                st.0,
                                val,
                                cn,
                                st.1,
                                sz,
                                n = name
                            ));
                        }
                        _ => {
                            self.unsupported(init.span, "a status with this kind of array");
                        }
                    },
                    (Some(init), _) => {
                        let code = self.store_into(&name, &ty, init);
                        self.line(&format!("{};", code));
                    }
                    (None, _) => {}
                }
                continue;
            }
            if let Ty::Class(id, _) = &ty
                && self.growable_fields(*id) > 0
            {
                self.line(&format!("{} {};", ct, name));
                self.init_growable(&name, *id);
                match &v.init {
                    Some(Expr {
                        kind: E::New { args, .. },
                        span,
                    }) => {
                        let st = self.construct(&name, args, *span);
                        if let Some(Name::Ident(c)) = &v.capture {
                            let (cn, _) = self.new_local(c);
                            self.line(&format!("uint8_t {} = {};", cn, st));
                        } else {
                            self.line(&format!("{};", st));
                        }
                    }
                    Some(e) => {
                        self.unsupported(e.span, "copying a value with a growable field");
                    }
                    None => {}
                }
                continue;
            }
            match &v.init {
                Some(init) => {
                    if let Some(Name::Ident(c)) = &v.capture {
                        let (cn, _) = self.new_local(c);
                        let (val, st) = self.with_status(init, &ty);
                        self.line(&format!("uint8_t {};", cn));
                        self.line(&format!(
                            "{} {} = ({{ {} __typeof__({}) _v = {}; {} = {}; _v; }});",
                            ct, name, st.0, val, val, cn, st.1
                        ));
                    } else {
                        let code = self.expr(init, &ty);
                        self.line(&format!("{} {} = {};", ct, name, code));
                    }
                }
                None => {
                    let zero = if matches!(ty, Ty::Prim(_)) || is_ref(&ty) {
                        "0".to_string()
                    } else {
                        format!("({}){{0}}", ct)
                    };
                    self.line(&format!("{} {} = {};", ct, name, zero));
                }
            }
        }
    }

    fn growable_fields(&self, id: ClassId) -> usize {
        self.prog.classes[id]
            .fields
            .iter()
            .filter(|f| !f.is_static && starts_empty(&f.ty))
            .count()
    }

    /// An empty array on data stacks of its own: one, or two for a cursor array.
    fn stack_init(&mut self, ty: &Ty) -> String {
        if matches!(ty, Ty::Map(..) | Ty::Set(_)) {
            let a = self.site();
            return format!("zr_map_new(zr_stack({}))", a);
        }
        match layout(ty) {
            Some(Layout::Cur) => {
                let (a, b) = (self.site(), self.site());
                format!("zr_cur_new(zr_stack({}), zr_stack({}))", a, b)
            }
            Some(Layout::Chk) => {
                let (a, b) = (self.site(), self.site());
                format!("zr_chk_new(zr_stack({}), zr_stack({}))", a, b)
            }
            Some(Layout::Deq) => {
                let a = self.site();
                format!("zr_deq_new(zr_stack({}))", a)
            }
            _ => {
                let a = self.site();
                format!("((zr_vec){{zr_stack({}), 0}})", a)
            }
        }
    }

    /// Gives an object's growable fields their data stacks.
    fn init_growable(&mut self, place: &str, id: ClassId) {
        let fields: Vec<(String, Ty, Span)> = self.prog.classes[id]
            .fields
            .iter()
            .filter(|f| !f.is_static && starts_empty(&f.ty))
            .map(|f| (f.name.clone(), f.ty.clone(), f.span))
            .collect();
        for (f, ty, span) in fields {
            if on_stack(&ty) {
                let init = self.stack_init(&ty);
                self.line(&format!("{}.f_{} = {};", place, f, init));
            } else {
                self.unsupported(span, "this kind of growable field");
            }
        }
    }

    /// Calls the constructor a `new` chose on storage `place`; gives its status, or 0.
    fn construct(&mut self, place: &str, args: &[ast::Arg], span: Span) -> String {
        let Some(fact) = self.facts.calls.get(&key(span)) else {
            return self.unsupported(span, "this constructor");
        };
        let Origin::Ctor(c, i) = fact.origin else {
            return self.unsupported(span, "this constructor");
        };
        let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
        let mut cargs = vec![format!("&{}", place)];
        for (a, p) in args.iter().zip(&params) {
            cargs.push(self.expr(&a.expr, p));
        }
        let fails = self.prog.classes[c].ctors[i].can_fail;
        let fname = if self.prog.classes[c].type_params.is_empty() {
            self.ctor_name(c, i)
        } else {
            let targs = match settle(self.ty_at(span).unref()) {
                Ty::Class(_, a) => a,
                _ => Vec::new(),
            };
            self.fn_inst(InstKind::Ctor(c, i), targs, Vec::new(), span)
        };
        let call = format!("{}({})", fname, cargs.join(", "));
        if fails {
            call
        } else {
            format!("({}, (uint8_t)0)", call)
        }
    }

    /// The code that stores `init` into `place`, of type `ty`: arrays on data stacks are
    /// filled in place, text is built in place (DESIGN §3.3, §4.11).
    fn store_into(&mut self, place: &str, ty: &Ty, init: &Expr) -> String {
        // A map or set starts empty on its data stack; `[]` is all that makes one.
        if matches!(ty, Ty::Map(..) | Ty::Set(_)) {
            if matches!(&init.kind, E::ArrayLit(items) if items.is_empty()) {
                return "(void)0".into();
            }
            return self.unsupported(init.span, "copying a map");
        }
        if let Ty::Array(el, _) = ty
            && matches!(layout(ty), Some(Layout::Deq | Layout::Cur | Layout::Chk))
        {
            // Cleared, then filled by pushing each element at the end.
            let el = (**el).clone();
            let ec = self.cty(&el, init.span);
            let arr = Arr {
                a: "_a".into(),
                lay: layout(ty).unwrap_or(Layout::Vec),
                ec: ec.clone(),
                cap: 0,
            };
            let fill = match &init.kind {
                E::ArrayLit(items) => {
                    let mut out = String::new();
                    for x in items {
                        let v = self.expr(x, &el);
                        out.push_str(&format!(
                            "{{ {} _x = {}; {} }} ",
                            ec,
                            v,
                            arr.put(Where::End)
                        ));
                    }
                    out
                }
                _ => {
                    let src = self.expr(init, &Ty::Slice(Box::new(el.clone())));
                    format!(
                        "zr_slice _src = {}; for (int64_t _i = 0; _i < _src.n; _i++) {{ {} _x = (({}*)_src.p)[_i]; {} }}",
                        src,
                        ec,
                        ec,
                        arr.put(Where::End)
                    )
                }
            };
            return format!(
                "({{ __typeof__({p}) *_a = &({p}); {} {} *_e; uint8_t _st = 0; {} (void)_e; (void)_st; }})",
                arr.clear(),
                ec,
                fill,
                p = place
            );
        }
        if let Ty::Array(el, _) = ty
            && on_stack(ty)
        {
            let el = (**el).clone();
            match &init.kind {
                E::ArrayLit(items) => {
                    let ec = self.cty(&el, init.span);
                    let mut parts = vec![format!("{}.n = 0", place)];
                    for x in items {
                        let v = self.expr(x, &el);
                        parts.push(format!("(({}*){}.p)[{}.n++] = {}", ec, place, place, v));
                    }
                    return parts.join(", ");
                }
                _ if el == Ty::u8() && self.is_text(init) => {
                    let (pre, writes) = self.text(init, "_k");
                    return format!(
                        "({{ {}.n = 0; zr_sink _k = zr_into(&{}); {} {} }})",
                        place, place, pre, writes
                    );
                }
                _ => {
                    self.direct_result = matches!(strip_paren(init).kind, E::Call { .. });
                    let src = self.expr(init, &Ty::Slice(Box::new(el.clone())));
                    self.direct_result = false;
                    let sz = self.elem_size(&el, init.span);
                    return format!(
                        "({{ zr_slice _s = {}; memmove({}.p, _s.p, (size_t)_s.n * {}); {}.n = _s.n; }})",
                        src, place, sz, place
                    );
                }
            }
        }
        let v = self.expr(init, ty);
        format!("{} = {}", place, v)
    }

    fn return_(&mut self, status: Option<&ast::StatusWrite>, value: Option<&Expr>, span: Span) {
        if self.inline_return(value) {
            return;
        }
        let st = match status {
            None => "0".to_string(),
            Some(ast::StatusWrite::Int(n)) => n.to_string(),
            Some(ast::StatusWrite::Name(q)) => {
                if q.parts.len() == 2 && q.parts[0].name == "status" {
                    status_code(&q.parts[1].name).to_string()
                } else {
                    let e = Expr {
                        kind: E::Ident(q.parts[0].name.clone()),
                        span: q.span,
                    };
                    self.expr(&e, &Ty::u8())
                }
            }
            Some(ast::StatusWrite::Expr(e)) => {
                let t = self.ty(e);
                let v = self.expr(e, &settle(&t));
                format!("(uint8_t)({})", v)
            }
        };
        let ret = self.ret.clone();
        if self.is_ctor {
            self.run_defers(0);
            if self.fails {
                self.line(&format!("return {};", st));
            } else {
                self.line("return;");
            }
            return;
        }
        let val = value.map(|v| self.expr(v, &ret));
        let rc = self.ret_c(&ret, self.fails, span);
        match (self.fails, val) {
            (true, Some(v)) => {
                let t = self.tmp("ret");
                self.line(&format!("{} {} = {{{}, {}}};", rc, t, v, st));
                self.run_defers(0);
                self.line(&format!("return {};", t));
            }
            (true, None) => {
                let t = self.tmp("ret");
                self.line(&format!("uint8_t {} = {};", t, st));
                self.run_defers(0);
                self.line(&format!("return {};", t));
            }
            (false, Some(v)) => {
                let t = self.tmp("ret");
                self.line(&format!("{} {} = {};", rc, t, v));
                self.run_defers(0);
                self.line(&format!("return {};", t));
            }
            (false, None) => {
                self.run_defers(0);
                if self.is_main && ret == Ty::u8() {
                    self.line("return 0;");
                } else {
                    self.line("return;");
                }
            }
        }
    }

    fn destructure(&mut self, targets: &[Target], capture: Option<&Name>, value: &Expr) {
        let vt = self.ty(value);
        let Ty::Tuple(ts) = settle(&vt) else {
            self.unsupported(value.span, "this destructuring");
            return;
        };
        let t = self.tmp("tup");
        if let Some(c) = capture {
            let (v, st) = self.with_status(value, &vt);
            self.line(&st.0);
            self.line(&format!("__typeof__({}) {} = {};", v, t, v));
            match c {
                Name::Ident(id) => {
                    let code = match self.locals.get(&key(id.span)) {
                        Some((n, _)) => n.clone(),
                        None => {
                            let (n, _) = self.new_local(id);
                            self.line(&format!("uint8_t {};", n));
                            n
                        }
                    };
                    self.line(&format!("{} = {};", code, st.1));
                }
                Name::Discard(_) => {}
            }
        } else {
            let v = self.expr(value, &vt);
            self.line(&format!("__typeof__({}) {} = {};", v, t, v));
        }
        for (i, (tg, ty)) in targets.iter().zip(&ts).enumerate() {
            match tg {
                Target::Decl(_, Name::Ident(n)) => {
                    let (name, lt) = self.new_local(n);
                    let ct = self.cty(&lt, n.span);
                    let conv = self.conv(&format!("{}.a{}", t, i), ty, &lt, value.span);
                    self.line(&format!("{} {} = {};", ct, name, conv));
                }
                Target::Existing(Name::Ident(n)) => {
                    let e = Expr {
                        kind: E::Ident(n.name.clone()),
                        span: n.span,
                    };
                    let (p, pt) = self.place(&e);
                    let conv = self.conv(&format!("{}.a{}", t, i), ty, &pt, value.span);
                    self.line(&format!("{} = {};", p, conv));
                }
                _ => {}
            }
        }
    }

    fn for_each(
        &mut self,
        label: Option<String>,
        ty: &LocalType,
        name: &Name,
        iter: &Expr,
        body: &Stmt,
    ) {
        let it = self.ty(iter);
        let by_ref = matches!(ty, LocalType::VarRef)
            || matches!(ty, LocalType::Type(t) if matches!(t.kind, ast::TypeKind::Ref { .. }));
        self.line("{");
        self.indent += 1;
        if matches!(it.unref(), Ty::Map(..) | Ty::Set(_)) {
            self.map_for_each(label, name, iter, body, by_ref);
            self.indent -= 1;
            self.line("}");
            return;
        }
        if let Ty::Class(..) = settle(it.unref()) {
            self.iter_for_each(label, name, iter, body);
            self.indent -= 1;
            self.line("}");
            return;
        }
        let elem = it.elem().cloned();
        let Some(elem) = elem else {
            self.unsupported(iter.span, "this for-each");
            self.indent -= 1;
            self.line("}");
            return;
        };
        let ec = self.cty(&elem, iter.span);
        let arr = self.tmp("a");
        // What is looped over is found once; its length is read on every step (DESIGN §9.4).
        let lay = layout(&it).unwrap_or(Layout::Vec);
        let ap = self.tmp("ap");
        if matches!(it.unref(), Ty::Slice(_) | Ty::View(_)) {
            let v = self.expr(iter, &it.unref().clone());
            self.line(&format!(
                "zr_slice {} = {}; zr_slice *{} = &{};",
                arr, v, ap, arr
            ));
        } else if self.is_place(iter) || is_ref(&it) {
            let (p, _) = self.place(iter);
            self.line(&format!("__typeof__({}) *{} = &({});", p, ap, p));
        } else {
            let v = self.expr(iter, &it);
            self.line(&format!(
                "__typeof__({}) {} = {}; __typeof__({}) *{} = &{};",
                v, arr, v, arr, ap, arr
            ));
        }
        let looped = Arr {
            a: ap,
            lay,
            ec: ec.clone(),
            cap: cap_of(&it),
        };
        let len = looped.len();
        let i = self.tmp("i");
        let (vname, vty) = match name {
            Name::Ident(n) => self.new_local(n),
            Name::Discard(_) => (
                self.tmp("x"),
                if by_ref {
                    Ty::Ref(Box::new(elem.clone()))
                } else {
                    elem.clone()
                },
            ),
        };
        let vc = self.cty(&vty, iter.span);
        let at = looped.at(&i);
        // A reference to an element that is itself a reference is that reference.
        let bind = if by_ref && !is_ref(&elem) {
            format!("{} {} = &{};", vc, vname, at)
        } else {
            format!("{} {} = {};", vc, vname, at)
        };
        let bind_line = bind.clone();
        self.loop_(
            label,
            &format!("for (int64_t {} = 0; {} < {}; {}++)", i, i, len, i),
            body,
            Some(&move |me: &mut Self| me.line(&bind_line)),
        );
        self.indent -= 1;
        self.line("}");
    }

    fn repeat(
        &mut self,
        label: Option<String>,
        target: &ast::LoopTarget,
        source: &Expr,
        capture: Option<&Name>,
        body: &Stmt,
    ) {
        let why = match capture {
            Some(Name::Ident(c)) => {
                let (n, _) = self.new_local(c);
                self.line(&format!("uint8_t {} = 0;", n));
                Some(n)
            }
            _ => None,
        };
        let st = self.ty(source);
        let lp = self.enter_loop(label);
        let (brk, cont) = (lp.brk.clone(), lp.cont.clone());
        self.loops.push(lp);
        self.line("for (;;) {");
        self.indent += 1;
        let (val, status) = self.with_status(source, &st);
        let st = if st.is_unknown() {
            self.last_status_ret.clone()
        } else {
            st
        };
        self.line(&status.0);
        let r = self.tmp("r");
        if st != Ty::Void {
            self.line(&format!("__typeof__({}) {} = {};", val, r, val));
        }
        let s = self.tmp("s");
        self.line(&format!("uint8_t {} = {};", s, status.1));
        match &why {
            Some(w) => self.line(&format!("if ({}) {{ {} = {}; goto {}; }}", s, w, s, brk)),
            None => self.line(&format!("if ({}) goto {};", s, brk)),
        }
        match target {
            ast::LoopTarget::Single(_, Name::Ident(n)) => {
                let (name, ty) = self.new_local(n);
                let ct = self.cty(&ty, n.span);
                let v = self.conv(&r, &st, &ty, source.span);
                self.line(&format!("{} {} = {};", ct, name, v));
            }
            ast::LoopTarget::Tuple(items) => {
                if let Ty::Tuple(ts) = &st {
                    for (k, (item, t)) in items.iter().zip(ts).enumerate() {
                        if let ast::LoopItem::Decl(_, Name::Ident(n)) = item {
                            let (name, ty) = self.new_local(n);
                            let ct = self.cty(&ty, n.span);
                            let v = self.conv(&format!("{}.a{}", r, k), t, &ty, source.span);
                            self.line(&format!("{} {} = {};", ct, name, v));
                        }
                    }
                }
            }
            _ => {}
        }
        self.body(body);
        self.line(&format!("{}: ;", cont));
        if let Some(w) = &why {
            self.line(&format!("{} = 0;", w));
        }
        self.indent -= 1;
        self.line("}");
        self.loops.pop();
        self.line(&format!("{}: ;", brk));
    }

    fn switch_stmt(&mut self, sw: &ast::Switch) {
        let st = settle(&self.ty(&sw.scrutinee));
        let v = self.expr(&sw.scrutinee, &st);
        let t = self.tmp("sw");
        let ct = self.cty(&st, sw.span);
        self.line("{");
        self.indent += 1;
        self.line(&format!("{} {} = {};", ct, t, v));
        let mut first = true;
        let mut default: Option<&ast::SwitchRule> = None;
        for rule in &sw.rules {
            let Some(labels) = &rule.labels else {
                default = Some(rule);
                continue;
            };
            let conds: Vec<String> = labels.iter().map(|l| self.case_test(&t, &st, l)).collect();
            self.line(&format!(
                "{}if ({})",
                if first { "" } else { "else " },
                conds.join(" || ")
            ));
            first = false;
            self.arm(&rule.body);
        }
        if let Some(d) = default {
            if !first {
                self.line("else");
            }
            self.arm(&d.body);
        }
        self.indent -= 1;
        self.line("}");
    }

    fn arm(&mut self, b: &SwitchBody) {
        match b {
            SwitchBody::Block(b) => self.block(b),
            SwitchBody::Expr(x) => {
                self.line("{");
                self.indent += 1;
                let c = self.expr_stmt_code(x);
                self.line(&format!("{};", c));
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn case_test(&mut self, v: &str, st: &Ty, l: &ast::CaseLabel) -> String {
        match l {
            ast::CaseLabel::Int {
                negative, value, ..
            } => format!("{} == {}{}", v, if *negative { "-" } else { "" }, value),
            ast::CaseLabel::Char(c, _) => format!("{} == {}", v, c),
            ast::CaseLabel::Str(s, _) => format!("zr_eq({}, {}, 1)", v, lit(s)),
            ast::CaseLabel::Ident(id) => match st {
                Ty::Class(c, _) => {
                    let i = self.prog.classes[*c]
                        .constants
                        .iter()
                        .position(|k| *k == id.name)
                        .unwrap_or(0);
                    format!("{} == {}", v, i)
                }
                _ => {
                    let e = Expr {
                        kind: E::Ident(id.name.clone()),
                        span: id.span,
                    };
                    let x = self.expr(&e, st);
                    format!("{} == {}", v, x)
                }
            },
        }
    }

    // ------------------------------------------------------------ expressions

    fn is_place(&self, e: &Expr) -> bool {
        match &e.kind {
            E::Ident(_) => matches!(
                self.facts.names.get(&key(e.span)),
                Some(NameRes::Local(_) | NameRes::Field(..))
            ),
            E::This | E::Member { .. } | E::Index { .. } => true,
            E::Paren(x) => self.is_place(x),
            _ => false,
        }
    }

    /// An lvalue for the storage `e` names, with its type. For a reference, the storage it refers to.
    fn place(&mut self, e: &Expr) -> (String, Ty) {
        let (code, t) = self.rv(e);
        match t {
            Ty::Ref(x) => (format!("(*{})", code), *x),
            Ty::Nullable(x) if matches!(*x, Ty::Ref(_)) => match *x {
                Ty::Ref(y) => (format!("(*{})", code), *y),
                _ => unreachable!(),
            },
            t => (code, t),
        }
    }

    /// The variable itself, for rebinding a reference with `.=`.
    fn cell(&mut self, e: &Expr) -> (String, Ty) {
        match &e.kind {
            E::Paren(x) => self.cell(x),
            E::Ident(_) | E::Member { .. } | E::Index { .. } => self.rv(e),
            _ => {
                let s = self.unsupported(e.span, "this rebinding");
                (s, Ty::Unknown)
            }
        }
    }

    /// A C expression for `e`, converted to type `want`.
    fn expr(&mut self, e: &Expr, want: &Ty) -> String {
        match &e.kind {
            E::Int {
                value,
                suffix: None,
            } => return self.int_lit(*value as i128, want),
            E::Char(c) => return self.int_lit(*c as i128, want),
            E::Float { text, suffix: None } => {
                return match want {
                    Ty::Prim(Prim::F32) => format!("{}f", float_text(text)),
                    _ => float_text(text),
                };
            }
            E::Unary {
                op: UnaryOp::Neg,
                expr,
            } if matches!(expr.kind, E::Int { suffix: None, .. }) => {
                if let E::Int { value, .. } = &expr.kind {
                    return self.int_lit((*value as i128).wrapping_neg(), want);
                }
            }
            E::Null => {
                // A null interface reference is two null pointers.
                if let Some((id, args, _)) = interfaces::iface_target(self.prog, want) {
                    let zi = self.iface_ref(id, &args, e.span);
                    return format!("(({}){{0}})", zi);
                }
                return "NULL".into();
            }
            E::ArrayLit(items) if items.is_empty() => {
                if let Ty::Array(..) = want {
                    let ct = self.cty(want, e.span);
                    return format!("({}){{0}}", ct);
                }
            }
            E::ArrayLit(items) if cap_of(want) > 0 => {
                let Some(el) = want.elem().cloned() else {
                    return self.unsupported(e.span, "this array literal");
                };
                let ec = self.cty(&el, e.span);
                let vals: Vec<String> = items.iter().map(|x| self.expr(x, &el)).collect();
                let fill = self.capped_fill(want, &items.len().to_string(), "_src", e.span);
                return format!(
                    "({{ {} _src[] = {{{}}}; {} _c; }})",
                    ec,
                    vals.join(", "),
                    fill
                );
            }
            E::New { args, .. } if matches!(want, Ty::Class(..)) => {
                let ct = self.cty(want, e.span);
                if let Ty::Class(id, _) = want
                    && self.growable_fields(*id) > 0
                {
                    return self.unsupported(e.span, "a temporary value with a growable field");
                }
                let t = self.tmp("new");
                let st = self.construct(&t, args, e.span);
                return format!("({{ {} {}; {}; {}; }})", ct, t, st, t);
            }
            E::Cond { cond, then, els } => {
                let c = self.cond(cond);
                let a = self.expr(then, want);
                let b = self.expr(els, want);
                return format!("({} ? {} : {})", c, a, b);
            }
            E::Paren(x) => return self.expr(x, want),
            E::Tuple(xs) => {
                if let Ty::Tuple(ts) = want {
                    let ct = self.cty(want, e.span);
                    let parts: Vec<String> =
                        xs.iter().zip(ts).map(|(x, t)| self.expr(x, t)).collect();
                    return format!("(({}){{{}}})", ct, parts.join(", "));
                }
            }
            E::Switch(sw) => return self.switch_expr(sw, want),
            _ => {}
        }
        if self.is_text(e)
            && !matches!(e.kind, E::Str(ref p) if p.iter().all(|x| matches!(x, StrPart::Text(_))))
        {
            // Text built for this statement only, on the scratch stack.
            self.used_scratch = true;
            let (pre, writes) = self.text(e, "_k");
            let s = format!(
                "({{ {} zr_vec _t = zr_scratch_begin(); zr_sink _k = zr_into(&_t); {} zr_scratch_end(_t); }})",
                pre, writes
            );
            if cap_of(want) > 0 {
                return self.conv(&s, &Ty::string(), want, e.span);
            }
            return s;
        }
        if matches!(want, Ty::Slice(_) | Ty::View(_)) && cap_of(&self.ty(e)) > 0 {
            return self.capped_view(e, None, None, e.span);
        }
        let (code, t) = self.rv(e);
        self.conv(&code, &t, want, e.span)
    }

    fn int_lit(&mut self, v: i128, want: &Ty) -> String {
        match want {
            Ty::Prim(p) if p.is_float() => format!("{}.0", v),
            Ty::Prim(Prim::I64) if v == i64::MIN as i128 => "INT64_MIN".into(),
            // C has no 128-bit literals: beyond 64 bits, two halves.
            Ty::Prim(p @ (Prim::I128 | Prim::U128)) => {
                if v > i64::MIN as i128 && v <= i64::MAX as i128 {
                    format!("(({}){}LL)", prim_c(*p), v)
                } else {
                    let b = v as u128;
                    format!(
                        "(({})(((zr_u128)0x{:x}ULL << 64) | 0x{:x}ULL))",
                        prim_c(*p),
                        (b >> 64) as u64,
                        b as u64
                    )
                }
            }
            Ty::Prim(p) if p.is_int() => {
                let suffix = if *p == Prim::U64 {
                    "ULL"
                } else if v > i32::MAX as i128 || v < i32::MIN as i128 {
                    "LL"
                } else {
                    ""
                };
                format!("(({}){}{})", prim_c(*p), v, suffix)
            }
            _ => {
                if v > i64::MAX as i128 {
                    format!("{}ULL", v)
                } else {
                    format!("((int64_t){}LL)", v)
                }
            }
        }
    }

    /// Converts C code of type `from` to type `to` (DESIGN §9.1, §4.10, §5.1).
    /// Statements that declare `_c`, a capped array of type `t`, holding the `n` elements
    /// at `src`. A gap buffer's cursor starts at the front, as when a cursor array is
    /// filled from a literal.
    fn capped_fill(&mut self, t: &Ty, n: &str, src: &str, span: Span) -> String {
        let ct = self.cty(t, span);
        let Some(el) = t.elem().cloned() else {
            return self.unsupported(span, "this array");
        };
        let ec = self.cty(&el, span);
        let (dst, counters) = match layout(t) {
            Some(Layout::Gap) => ("_c.e".to_string(), format!("_c.gs = {}; _c.an = 0;", n)),
            // A ring that changes only at the start sits at the top of its buffer, so
            // it never wraps and its elements never move (REFERENCES §5.4).
            Some(Layout::Ring) if matches!(t.unref(), Ty::Array(_, k) if k.end == '.') => {
                let k = cap_of(t);
                let h = format!("(({k} - (int64_t)({n})) % {k})", k = k, n = n);
                (
                    format!("&_c.e[{}]", h),
                    format!("_c.h = {}; _c.n = {};", h, n),
                )
            }
            Some(Layout::Ring) => ("_c.e".to_string(), format!("_c.h = 0; _c.n = {};", n)),
            _ => ("_c.e".to_string(), format!("_c.n = {};", n)),
        };
        format!(
            "{} _c = {{0}}; memcpy({}, {}, (size_t)({}) * sizeof({})); {}",
            ct, dst, src, n, ec, counters
        )
    }

    fn conv(&mut self, code: &str, from: &Ty, to: &Ty, span: Span) -> String {
        if from == to || to.is_unknown() {
            return code.to_string();
        }
        if let Some(c) = self.iface_conv(code, from, to, span) {
            return c;
        }
        match (from, to) {
            (Ty::Null, _) => "NULL".into(),
            (Ty::Ref(x), Ty::Ref(_)) | (Ty::Ref(x), Ty::Nullable(_)) if !matches!(**x, Ty::Class(c, _) if self.prog.classes[c].kind == ClassKind::Interface) => {
                code.to_string()
            }
            (Ty::Nullable(_), Ty::Nullable(_)) | (Ty::Nullable(_), Ty::Ref(_)) => code.to_string(),
            (Ty::Ref(x), _) => {
                let inner = format!("(*{})", code);
                self.conv(&inner, x, to, span)
            }
            (_, Ty::Ref(x)) | (_, Ty::Nullable(x))
                if !matches!(to, Ty::Nullable(_)) || matches!(**x, Ty::Ref(_)) =>
            {
                // A value place bound to a reference.
                let target = match to {
                    Ty::Nullable(y) => match &**y {
                        Ty::Ref(z) => (**z).clone(),
                        _ => (**y).clone(),
                    },
                    Ty::Ref(y) => (**y).clone(),
                    _ => unreachable!(),
                };
                let inner = self.conv(code, from, &target, span);
                format!("(&{})", inner)
            }
            (Ty::Array(_, k), Ty::Slice(_) | Ty::View(_)) => {
                if k.cap == CapKind::Capped {
                    let (Some(el), Some(lay)) = (from.elem().cloned(), layout(from)) else {
                        return self.unsupported(span, "this slice");
                    };
                    let ca = Arr {
                        a: "_c".into(),
                        lay,
                        ec: self.cty(&el, span),
                        cap: k.size,
                    };
                    format!(
                        "({{ __typeof__({}) *_c = &({}); int64_t _n = {}; (zr_slice){{{}, _n}}; }})",
                        code,
                        code,
                        ca.len(),
                        ca.span("0")
                    )
                } else if k.has_cursor() {
                    self.unsupported(
                        span,
                        "slices of cursor arrays, which are undecided (DESIGN §17)",
                    )
                } else if k.push_front() || k.pop_front() {
                    format!(
                        "({{ __typeof__({}) _vv = {}; (zr_slice){{_vv.p, _vv.n}}; }})",
                        code, code
                    )
                } else {
                    format!("zr_view({})", code)
                }
            }
            (Ty::Slice(_) | Ty::View(_), Ty::Slice(_) | Ty::View(_)) => code.to_string(),
            // Stops the program when it doesn't fit, as a cast does (DESIGN §9.1).
            (Ty::Slice(_) | Ty::View(_), Ty::Array(..)) if cap_of(to) > 0 => {
                let k = cap_of(to);
                let loc = self.loc(span);
                let fill = self.capped_fill(to, "_s.n", "_s.p", span);
                format!(
                    "({{ zr_slice _s = {}; if (_s.n > {}) zr_fail({}, \"the value doesn't fit in an array that holds {}\"); {} _c; }})",
                    code, k, loc, k, fill
                )
            }
            (Ty::Prim(_) | Ty::IntLit { .. } | Ty::FloatLit, Ty::Prim(p)) => {
                format!("(({}){})", prim_c(*p), code)
            }
            (Ty::Class(..), Ty::Class(..))
            | (Ty::Tuple(_), Ty::Tuple(_))
            | (Ty::Array(..), Ty::Array(..)) => code.to_string(),
            _ => {
                let what = format!(
                    "converting {} to {}",
                    self.prog.show(from),
                    self.prog.show(to)
                );
                self.unsupported(span, &what)
            }
        }
    }

    /// A condition as a C truth value; temporaries in it are released at once.
    fn cond(&mut self, e: &Expr) -> String {
        let before = self.used_scratch;
        self.used_scratch = false;
        let t = settle(&self.ty(e));
        let c = self.expr(e, &t);
        let c = if self.used_scratch {
            format!(
                "({{ int64_t _m = zr_sm(); bool _c = ({}); zr_sr(_m); _c; }})",
                c
            )
        } else {
            c
        };
        self.used_scratch = before;
        c
    }

    /// An expression used as a statement: its value and its status may be dropped.
    fn expr_stmt_code(&mut self, e: &Expr) -> String {
        match &e.kind {
            E::Call { .. } | E::New { .. } | E::ThisNew(_) => {
                let c = self.call_any(e);
                format!("(void)({})", c.code)
            }
            E::Binary {
                op: BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor,
                ..
            } => {
                let (code, _) = self.insert_status(e);
                format!("(void)({})", code)
            }
            E::Postfix { op, expr } if !matches!(op, PostfixOp::Inc | PostfixOp::Dec) => {
                let c = self.remove_op(*op, expr, e.span);
                format!("(void)({})", c.code)
            }
            _ => {
                let t = settle(&self.ty(e));
                let code = if t == Ty::Void {
                    self.rv(e).0
                } else {
                    self.expr(e, &t)
                };
                format!("(void)({})", code)
            }
        }
    }

    /// The value and the status of an expression whose status is read (DESIGN §7.3):
    /// statements to run first, and the status expression.
    fn with_status(&mut self, e: &Expr, want: &Ty) -> (String, (String, String)) {
        let e = strip_paren(e);
        let c = self.status_call(e);
        self.last_status_ret = c.ret.clone();
        let r = self.tmp("rs");
        // A call that returns nothing and can't fail has status 0.
        let pre = if !c.fails && c.ret == Ty::Void {
            format!("(void)({});", c.code)
        } else {
            format!("__typeof__({}) {} = {};", c.code, r, c.code)
        };
        let (val, st) = match (c.fails, c.ret == Ty::Void) {
            (true, true) => ("0".to_string(), r.clone()),
            (true, false) => (format!("{}.v", r), format!("{}.s", r)),
            (false, true) => ("0".to_string(), "0".to_string()),
            (false, false) => (r.clone(), "0".to_string()),
        };
        let val = if c.ret == Ty::Void || *want == Ty::Void {
            val
        } else {
            self.conv(&val, &c.ret, want, e.span)
        };
        (val, (pre, st))
    }

    fn status_call(&mut self, e: &Expr) -> CallOut {
        match &e.kind {
            E::Call { .. } | E::New { .. } | E::ThisNew(_) => self.call_any(e),
            E::Postfix { op, expr } if !matches!(op, PostfixOp::Inc | PostfixOp::Dec) => {
                self.remove_op(*op, expr, e.span)
            }
            E::Binary {
                op: BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor,
                ..
            } => {
                let (code, fails) = self.insert_status(e);
                CallOut {
                    code,
                    ret: Ty::Void,
                    fails,
                }
            }
            _ => {
                let t = settle(&self.ty(e));
                let (code, _) = self.rv(e);
                CallOut {
                    code,
                    ret: t,
                    fails: false,
                }
            }
        }
    }

    /// An expression's value in its own type.
    fn rv(&mut self, e: &Expr) -> (String, Ty) {
        let t = settle(&self.ty(e));
        match &e.kind {
            E::Int { value, suffix } => {
                // Unsuffixed, it's an `i64`, or a `u128` when only that holds it.
                let lt = match (suffix, &t) {
                    (Some(p), _) => Ty::Prim(*p),
                    (None, Ty::Prim(Prim::U128)) => t.clone(),
                    (None, _) => Ty::i64(),
                };
                (self.int_lit(*value as i128, &lt), t)
            }
            E::Float { text, suffix } => (
                if *suffix == Some(Prim::F32) {
                    format!("{}f", float_text(text))
                } else {
                    float_text(text)
                },
                t,
            ),
            E::Char(c) => (format!("((uint32_t){})", c), Ty::Prim(Prim::U32)),
            E::Bool(b) => (b.to_string(), Ty::bool()),
            E::Null => ("NULL".into(), Ty::Null),
            E::Str(parts) if parts.iter().all(|p| matches!(p, StrPart::Text(_))) => {
                let s: String = parts
                    .iter()
                    .map(|p| {
                        if let StrPart::Text(t) = p {
                            t.as_str()
                        } else {
                            ""
                        }
                    })
                    .collect();
                (lit(&s), Ty::string())
            }
            E::Str(_) => (self.expr(e, &Ty::string()), Ty::string()),
            E::Ident(n) => self.ident(n, e.span, t),
            E::This => ("self".into(), Ty::Ref(Box::new(self.this_class()))),
            E::Paren(x) => self.rv(x),
            E::Member { expr, name } => self.member(expr, name, e.span, t),
            E::Index { expr, mode, index } => self.index(expr, *mode, index, e.span),
            E::Slice { expr, lo, hi } => self.slice(expr, lo.as_ref(), hi.as_ref(), e.span),
            E::Call { .. } | E::New { .. } | E::ThisNew(_) => {
                let c = self.call_any(e);
                let code = match (c.fails, c.ret == Ty::Void) {
                    (true, false) => format!("({}).v", c.code),
                    _ => c.code,
                };
                (code, c.ret)
            }
            E::Assign {
                op,
                target,
                capture,
                value,
            } => (self.assign(*op, target, capture.as_ref(), value, e.span), t),
            E::StatusAssign { capture, value } => {
                let (_, (pre, st)) = self.with_status(value, &Ty::Void);
                let set = match capture {
                    Name::Ident(id) => {
                        let ce = Expr {
                            kind: E::Ident(id.name.clone()),
                            span: id.span,
                        };
                        let (p, _) = self.place(&ce);
                        format!("{} = {};", p, st)
                    }
                    Name::Discard(_) => format!("(void)({});", st),
                };
                (format!("({{ {} {} }})", pre, set), Ty::Void)
            }
            E::StatusRead(x) => {
                let (_, (pre, st)) = self.with_status(x, &Ty::Void);
                (format!("({{ {} (uint8_t)({}); }})", pre, st), Ty::u8())
            }
            E::Cond { .. } | E::Switch(_) | E::Tuple(_) => {
                let code = self.expr(e, &t);
                (code, t)
            }
            E::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span, t),
            E::Unary { op, expr } => self.unary(*op, expr, e.span, t),
            E::Cast { ty, mode, expr } => self.cast(ty, *mode, expr, e.span),
            E::Postfix { op, expr } => match op {
                PostfixOp::Inc | PostfixOp::Dec => {
                    let (p, pt) = self.place(expr);
                    let ct = self.cty(&pt, e.span);
                    let step = self.arith_code(
                        if *op == PostfixOp::Inc {
                            BinOp::Add
                        } else {
                            BinOp::Sub
                        },
                        "*_p",
                        "1",
                        &pt,
                        e.span,
                    );
                    (
                        format!(
                            "({{ {} *_p = &({}); {} _o = *_p; *_p = {}; _o; }})",
                            ct, p, ct, step
                        ),
                        pt,
                    )
                }
                _ => {
                    let c = self.remove_op(*op, expr, e.span);
                    (format!("({}).v", c.code), c.ret)
                }
            },
            E::PrimMember { prim, name } => {
                let (lo, hi) = min_max(*prim);
                let v = match name.name.as_str() {
                    "MIN" => lo,
                    "MAX" => hi,
                    "NAN" => "NAN".into(),
                    "INFINITY" => "INFINITY".into(),
                    "EPSILON" => {
                        if *prim == Prim::F32 {
                            "FLT_EPSILON".into()
                        } else {
                            "DBL_EPSILON".into()
                        }
                    }
                    _ => self.unsupported(e.span, "this constant"),
                };
                (format!("(({}){})", prim_c(*prim), v), Ty::Prim(*prim))
            }
            E::Lambda { .. } => (self.lambda_value(e), t),
            E::InstanceOf { expr, ty, bind } => (self.instance_of(expr, ty, bind.as_ref()), t),
            E::ArrayLit(_) => (self.unsupported(e.span, "an array literal here"), t),
            E::Discard => ("0".into(), t),
        }
    }

    fn this_class(&self) -> Ty {
        let c = &self.prog.classes[self.class];
        self.sb(&Ty::Class(
            self.class,
            c.type_params
                .iter()
                .map(|p| Ty::Param(p.name.clone()))
                .collect(),
        ))
    }

    fn ident(&mut self, n: &str, span: Span, t: Ty) -> (String, Ty) {
        match self.facts.names.get(&key(span)) {
            Some(NameRes::Local(d)) => match self.locals.get(d) {
                Some((c, lt)) => {
                    // A narrowed nullable reference reads as a plain one.
                    let ty = if let (Ty::Nullable(_), Ty::Ref(_)) = (lt, &t) {
                        t.clone()
                    } else {
                        lt.clone()
                    };
                    (c.clone(), ty)
                }
                None => (
                    self.unsupported(span, &format!("the variable `{}` here", n)),
                    t,
                ),
            },
            Some(NameRes::Field(c, i)) => {
                let fty = self.field_ty(*c, *i, &self.cur_cargs.clone());
                let f = &self.prog.classes[*c].fields[*i];
                let ty = if matches!(fty, Ty::Nullable(_)) && is_ref(&t) {
                    t.clone()
                } else {
                    fty
                };
                if f.is_static {
                    (format!("zs{}_{}", c, i), ty)
                } else {
                    (format!("self->f_{}", f.name), ty)
                }
            }
            // A C macro or enum constant, as a literal of the type it's used as.
            Some(NameRes::CConst(v)) => {
                let lt = match settle(&t) {
                    Ty::Prim(p) => Ty::Prim(p),
                    _ => Ty::i64(),
                };
                (self.int_lit(*v, &lt), lt)
            }
            None => (
                self.unsupported(span, &format!("`{}` (a library name)", n)),
                t,
            ),
        }
    }

    fn member(&mut self, base: &Expr, name: &ast::Ident, span: Span, t: Ty) -> (String, Ty) {
        let bt = self.ty(base);
        if let Ty::TypeName(inner) = &bt {
            // `T.MAX` in an instantiation where `T` is a number type: as `u8.MAX`.
            if let Ty::Prim(p) = **inner {
                let as_prim = Expr {
                    kind: E::PrimMember {
                        prim: p,
                        name: name.clone(),
                    },
                    span,
                };
                return self.rv(&as_prim);
            }
            if let Ty::Class(c, _) = &**inner {
                let ci = &self.prog.classes[*c];
                if ci.kind == ClassKind::Enum {
                    let i = ci
                        .constants
                        .iter()
                        .position(|k| *k == name.name)
                        .unwrap_or(0);
                    return (format!("((int32_t){})", i), t);
                }
                if let Some(i) = ci.fields.iter().position(|f| f.name == name.name) {
                    return (format!("zs{}_{}", c, i), ci.fields[i].ty.clone());
                }
            }
            return (self.unsupported(span, "this name"), t);
        }
        if let Ty::Namespace(ns) = &bt {
            if *ns == "status" {
                return (format!("((uint8_t){})", status_code(&name.name)), Ty::u8());
            }
            if *ns == "math" {
                let v = match name.name.as_str() {
                    "PI" => "3.14159265358979323846",
                    "TAU" => "6.28318530717958647692",
                    _ => "2.71828182845904523536",
                };
                return (v.into(), Ty::Prim(Prim::F64));
            }
        }
        let (b, bty) = self.rv(base);
        let deref = bty.unref().clone();
        let acc = if is_ref(&bty) {
            format!("({})->", b)
        } else {
            format!("({}).", b)
        };
        match &deref {
            Ty::Class(c, cargs) => {
                let f = self.prog.classes[*c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name)
                    .map(|i| self.field_ty(*c, i, cargs));
                match f {
                    Some(ft) => {
                        let ty = if matches!(ft, Ty::Nullable(_)) && is_ref(&t) {
                            t.clone()
                        } else {
                            ft
                        };
                        (format!("{}f_{}", acc, name.name), ty)
                    }
                    None => (self.unsupported(span, "this field"), t),
                }
            }
            // An entry's key is read-only (the checker sees to it); its value is writable.
            Ty::MapEntry(k, _) if name.name == "key" => (format!("{}k", acc), (**k).clone()),
            Ty::MapEntry(_, v) if name.name == "value" => (format!("{}v", acc), (**v).clone()),
            Ty::Map(..) | Ty::Set(_) if name.name == "len" => {
                (format!("((int64_t){}live)", acc), Ty::i64())
            }
            Ty::Array(_, k) if k.cap == CapKind::Chunked && name.name == "cursor" => {
                (format!("{}pos", acc), Ty::i64())
            }
            Ty::Array(_, k)
                if k.cap == CapKind::Capped && k.has_cursor() && name.name == "cursor" =>
            {
                (format!("((int64_t){}cur)", acc), Ty::i64())
            }
            Ty::Array(_, k) if k.cap == CapKind::Capped && k.has_cursor() && name.name == "len" => {
                (format!("((int64_t){a}gs + {a}an)", a = acc), Ty::i64())
            }
            Ty::Array(_, k)
                if k.has_cursor() && k.cap != CapKind::Chunked && name.name == "len" =>
            {
                let h = if is_ref(&bty) {
                    b.clone()
                } else {
                    format!("&({})", b)
                };
                let h = if self.is_place(base) || is_ref(&bty) {
                    h
                } else {
                    format!("({{ __typeof__({}) _h = {}; &_h; }})", b, b)
                };
                (format!("zr_cur_len({})", h), Ty::i64())
            }
            Ty::Array(_, k) if k.has_cursor() && name.name == "cursor" => {
                (format!("{}bn", acc), Ty::i64())
            }
            Ty::Array(..) | Ty::Slice(_) | Ty::View(_) if name.name == "len" => {
                (format!("((int64_t){}n)", acc), Ty::i64())
            }
            _ => (self.unsupported(span, "this field"), t),
        }
    }

    /// Element `i` of an array or slice, as an lvalue.
    fn elem_ptr(&mut self, base: &Expr, idx: &str, span: Span) -> (String, Ty) {
        let bt = self.ty(base);
        let arr = bt.unref().clone();
        let Some(el) = arr.elem().cloned() else {
            return (self.unsupported(span, "this indexing"), Ty::Unknown);
        };
        let ec = self.cty(&el, span);
        let loc = self.loc(span);
        let b = self.tmp("b");
        if let Some(lay) = layout(&arr).filter(|l| l.capped()) {
            let ca = Arr {
                a: b.clone(),
                lay,
                ec: ec.clone(),
                cap: cap_of(&arr),
            };
            let fix = idx.replace("LEN", &ca.len()).replace("LOC", &loc);
            let at = ca.at("_ci");
            if !self.is_place(base) && !is_ref(&bt) {
                let v = self.expr(base, &arr);
                return (
                    format!(
                        "({{ __typeof__({}) {}v = {}; __typeof__({}v) *{} = &{}v; int64_t _ci = {}; {}; }})",
                        v, b, v, b, b, b, fix, at
                    ),
                    el,
                );
            }
            let (p, _) = self.place(base);
            return (
                format!(
                    "(*({{ __typeof__({}) *{} = &({}); int64_t _ci = {}; &{}; }}))",
                    p, b, p, fix, at
                ),
                el,
            );
        }
        let v = self.expr(base, &arr);
        if layout(&arr) == Some(Layout::Chk) {
            // Three loads: the chunk's head, the slot, then the element (DESIGN §4.3).
            let fix = idx.replace("LEN", &format!("{}.n", b)).replace("LOC", &loc);
            return (
                format!(
                    "(*({{ __typeof__({}) {} = {}; int64_t _ci = {}; &(({}*){}.d.data)[zr_chk_at(&{}, _ci)]; }}))",
                    v, b, v, fix, ec, b, b
                ),
                el,
            );
        }
        if layout(&arr) == Some(Layout::Cur) {
            // Two loads: the slot from the index, then the element (DESIGN §4.4).
            let fix = idx
                .replace("LEN", &format!("zr_cur_len(&{})", b))
                .replace("LOC", &loc);
            return (
                format!(
                    "(*({{ __typeof__({}) {} = {}; int64_t _ci = {}; &(({}*){}.d.data)[zr_cur_at(&{}, _ci)]; }}))",
                    v, b, v, fix, ec, b, b
                ),
                el,
            );
        }
        let fix = idx.replace("LEN", &format!("{}.n", b)).replace("LOC", &loc);
        (
            format!(
                "(*({{ __typeof__({}) {} = {}; &(({}*){}.p)[{}]; }}))",
                v, b, v, ec, b, fix
            ),
            el,
        )
    }

    fn index(
        &mut self,
        base: &Expr,
        mode: ast::IndexMode,
        index: &Expr,
        span: Span,
    ) -> (String, Ty) {
        let it = settle(&self.ty(index));
        let i = self.expr(index, &it);
        let i = format!("(int64_t)({})", i);
        let fix = match mode {
            ast::IndexMode::Checked => format!("zr_ix({}, LEN, LOC)", i),
            ast::IndexMode::FromEnd => format!("zr_ix(LEN - {}, LEN, LOC)", i),
            ast::IndexMode::Wrap => format!("zr_ix_wrap({}, LEN, LOC)", i),
            ast::IndexMode::Clamp => format!("zr_ix_clamp({}, LEN, LOC)", i),
        };
        self.elem_ptr(base, &fix, span)
    }

    /// A bound of a slice, given the name of the length it counts back from.
    fn slice_bound(&mut self, b: Option<&ast::Bound>, dflt: &str, len: &str) -> String {
        match b {
            None => dflt.to_string(),
            Some(b) => {
                let t = settle(&self.ty(&b.expr));
                let v = self.expr(&b.expr, &t);
                if b.from_end {
                    format!("({} - (int64_t)({}))", len, v)
                } else {
                    format!("(int64_t)({})", v)
                }
            }
        }
    }

    /// A slice of positions `lo..hi` of a capped array, all of it by default (DESIGN
    /// §4.10). A ring or gap buffer is rearranged first when those positions are split,
    /// which leaves its order and cursor as they were. A temporary is copied to the
    /// scratch stack, so the slice lasts to the end of the statement.
    fn capped_view(
        &mut self,
        e: &Expr,
        lo: Option<&ast::Bound>,
        hi: Option<&ast::Bound>,
        span: Span,
    ) -> String {
        let bt = self.ty(e);
        let arr = bt.unref().clone();
        let (Some(el), Some(lay)) = (arr.elem().cloned(), layout(&arr)) else {
            return self.unsupported(span, "this slice");
        };
        let ec = self.cty(&el, span);
        let ptr = if is_ref(&bt) || self.is_place(e) {
            let (p, _) = self.place(e);
            format!("__typeof__({}) *_c = &({});", p, p)
        } else {
            self.used_scratch = true;
            let v = self.expr(e, &arr);
            format!(
                "__typeof__({}) _cv = {}; __typeof__(_cv) *_c = zr_scratch_copy(&_cv, sizeof _cv);",
                v, v
            )
        };
        let ca = Arr {
            a: "_c".into(),
            lay,
            ec,
            cap: cap_of(&arr),
        };
        let l = self.slice_bound(lo, "0", "_n");
        let h = self.slice_bound(hi, "_n", "_n");
        let loc = self.loc(span);
        format!(
            "({{ {} int64_t _n = {}; int64_t _lo = {}, _hi = {}; zr_range(_lo, _hi, _n, {}); (zr_slice){{{}, _hi - _lo}}; }})",
            ptr,
            ca.len(),
            l,
            h,
            loc,
            ca.span("_lo")
        )
    }

    fn slice(
        &mut self,
        base: &Expr,
        lo: Option<&ast::Bound>,
        hi: Option<&ast::Bound>,
        span: Span,
    ) -> (String, Ty) {
        let bt = self.ty(base);
        let arr = bt.unref().clone();
        let Some(el) = arr.elem().cloned() else {
            return (self.unsupported(span, "this slice"), Ty::Unknown);
        };
        if cap_of(&arr) > 0 {
            return (self.capped_view(base, lo, hi, span), Ty::View(Box::new(el)));
        }
        let ec = self.cty(&el, span);
        let s = self.expr(base, &Ty::Slice(Box::new(el.clone())));
        let l = self.slice_bound(lo, "0", "_s.n");
        let h = self.slice_bound(hi, "_s.n", "_s.n");
        let loc = self.loc(span);
        let res = if matches!(bt.unref(), Ty::Slice(_)) {
            Ty::Slice(Box::new(el))
        } else {
            Ty::View(Box::new(el))
        };
        (
            format!(
                "({{ zr_slice _s = {}; int64_t _lo = {}, _hi = {}; zr_range(_lo, _hi, _s.n, {}); (zr_slice){{({}*)_s.p + _lo, _hi - _lo}}; }})",
                s, l, h, loc, ec
            ),
            res,
        )
    }

    fn assign(
        &mut self,
        op: AssignOp,
        target: &Expr,
        capture: Option<&Name>,
        value: &Expr,
        span: Span,
    ) -> String {
        if matches!(target.kind, E::Discard) {
            let t = settle(&self.ty(value));
            let v = if t == Ty::Void {
                self.rv(value).0
            } else {
                self.expr(value, &t)
            };
            return format!("(void)({})", v);
        }
        if let Some(c) = capture {
            let tt = settle(&self.ty(target));
            let (val, (pre, st)) = self.with_status(value, &tt);
            let (p, _) = self.place(target);
            let set = match c {
                Name::Ident(id) => {
                    let ce = Expr {
                        kind: E::Ident(id.name.clone()),
                        span: id.span,
                    };
                    let (cp, _) = self.place(&ce);
                    format!("{} = {};", cp, st)
                }
                Name::Discard(_) => String::new(),
            };
            return format!("({{ {} {} {} = {}; }})", pre, set, p, val);
        }
        match op {
            AssignOp::Rebind => {
                let (c, ct) = self.cell(target);
                let v = self.expr(value, &ct);
                format!("({} = {})", c, v)
            }
            AssignOp::Assign => {
                if self.facts.binds.contains(&key(span)) {
                    let (c, ct) = self.cell(target);
                    let v = self.expr(value, &ct);
                    return format!("({} = {})", c, v);
                }
                let (p, pt) = self.place(target);
                if on_stack(&pt) {
                    return self.store_into(&p, &pt, value);
                }
                let v = self.expr(value, &pt);
                format!("({} = {})", p, v)
            }
            _ => {
                let (p, pt) = self.place(target);
                let ct = self.cty(&pt, span);
                let bop = match op {
                    AssignOp::Add => BinOp::Add,
                    AssignOp::Sub => BinOp::Sub,
                    AssignOp::Mul => BinOp::Mul,
                    AssignOp::Div => BinOp::Div,
                    AssignOp::Rem => BinOp::Rem,
                    AssignOp::And => BinOp::BitAnd,
                    AssignOp::Or => BinOp::BitOr,
                    AssignOp::Xor => BinOp::BitXor,
                    AssignOp::Shl => BinOp::Shl,
                    AssignOp::Shr => BinOp::Shr,
                    AssignOp::WrapAdd => BinOp::WrapAdd,
                    AssignOp::WrapSub => BinOp::WrapSub,
                    AssignOp::WrapMul => BinOp::WrapMul,
                    AssignOp::SatAdd => BinOp::SatAdd,
                    AssignOp::SatSub => BinOp::SatSub,
                    _ => BinOp::SatMul,
                };
                let vt = if matches!(bop, BinOp::Shl | BinOp::Shr) {
                    settle(&self.ty(value))
                } else {
                    pt.clone()
                };
                let v = self.expr(value, &vt);
                let code = self.arith_code(bop, "*_p", &v, &pt, span);
                format!("({{ {} *_p = &({}); *_p = {}; }})", ct, p, code)
            }
        }
    }

    fn arith_code(&mut self, op: BinOp, a: &str, b: &str, t: &Ty, span: Span) -> String {
        let loc = self.loc(span);
        let Ty::Prim(p) = t else {
            return self.unsupported(span, "arithmetic on this type");
        };
        let c = prim_c(*p);
        if p.is_float() {
            let sym = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                _ => return self.unsupported(span, "this operator on floats"),
            };
            return format!("(({}){} {} ({}){})", c, a, sym, c, b);
        }
        let u = unsigned_of(*p);
        let (lo, hi) = min_max(*p);
        match op {
            BinOp::Add => format!("ZR_ADD({}, {}, {}, {})", c, a, b, loc),
            BinOp::Sub => format!("ZR_SUB({}, {}, {}, {})", c, a, b, loc),
            BinOp::Mul => format!("ZR_MUL({}, {}, {}, {})", c, a, b, loc),
            BinOp::Div => format!("ZR_DIV({}, {}, {}, {})", c, a, b, loc),
            BinOp::Rem => format!("ZR_REM({}, {}, {}, {})", c, a, b, loc),
            BinOp::WrapAdd => format!("(({})(({}){} + ({}){}))", c, u, a, u, b),
            BinOp::WrapSub => format!("(({})(({}){} - ({}){}))", c, u, a, u, b),
            BinOp::WrapMul => format!("(({})(({}){} * ({}){}))", c, u, a, u, b),
            BinOp::SatAdd => format!("ZR_SATADD({}, {}, {}, {}, {})", c, a, b, lo, hi),
            BinOp::SatSub => format!("ZR_SATSUB({}, {}, {}, {}, {})", c, a, b, lo, hi),
            BinOp::SatMul => format!("ZR_SATMUL({}, {}, {}, {}, {})", c, a, b, lo, hi),
            BinOp::BitAnd => format!("(({})({} & {}))", c, a, b),
            BinOp::BitOr => format!("(({})({} | {}))", c, a, b),
            BinOp::BitXor => format!("(({})({} ^ {}))", c, a, b),
            BinOp::Shl => format!("ZR_SHL({}, {}, {}, {})", c, u, a, b),
            BinOp::Shr => format!("ZR_SHR({}, {}, {})", c, a, b),
            _ => self.unsupported(span, "this operator"),
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, span: Span, t: Ty) -> (String, Ty) {
        use BinOp::*;
        match op {
            And | Or => {
                let a = self.cond(l);
                let b = self.cond(r);
                (
                    format!("({} {} {})", a, if op == And { "&&" } else { "||" }, b),
                    Ty::bool(),
                )
            }
            Eq | Ne | Lt | Gt | Le | Ge => (self.compare(op, l, r, span), Ty::bool()),
            InsertEnd | InsertFront | InsertCursor => {
                // As a value, an insertion gives the array, so insertions chain.
                let mut stmts = String::new();
                let mut fails = false;
                let whole = Expr {
                    kind: E::Binary {
                        op,
                        lhs: Box::new(l.clone()),
                        rhs: Box::new(r.clone()),
                    },
                    span,
                };
                self.insert_chain(&whole, &mut stmts, &mut fails);
                let at = self.ty(l).unref().clone();
                (
                    format!("({{ uint8_t _st = 0; {} (void)_st; (*_arr); }})", stmts),
                    at,
                )
            }
            _ => {
                let rt = if t.is_unknown() {
                    settle(&self.ty(l))
                } else {
                    t.clone()
                };
                let a = self.expr(l, &rt);
                let bt = if matches!(op, Shl | Shr) {
                    settle(&self.ty(r))
                } else {
                    rt.clone()
                };
                let b = self.expr(r, &bt);
                let b = if matches!(op, Shl | Shr) {
                    format!("(int64_t)({})", b)
                } else {
                    b
                };
                let code = if rt == Ty::bool() {
                    let sym = match op {
                        BitAnd => "&",
                        BitOr => "|",
                        _ => "^",
                    };
                    format!("((bool)({} {} {}))", a, sym, b)
                } else {
                    self.arith_code(op, &a, &b, &rt, span)
                };
                (code, rt)
            }
        }
    }

    fn compare(&mut self, op: BinOp, l: &Expr, r: &Expr, span: Span) -> String {
        let sym = match op {
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            _ => ">=",
        };
        let lt = self.ty(l);
        let rt = self.ty(r);
        // Null tests compare pointers: an interface reference's is its value's.
        if matches!(l.kind, E::Null) || matches!(r.kind, E::Null) {
            let (a, at) = if matches!(l.kind, E::Null) {
                self.rv(r)
            } else {
                self.rv(l)
            };
            if interfaces::iface_target(self.prog, &at).is_some() {
                return format!("(({}).p {} NULL)", a, sym);
            }
            return format!("({} {} NULL)", a, sym);
        }
        let (lu, ru) = (settle(lt.unref()), settle(rt.unref()));
        let textish = |t: &Ty| matches!(t, Ty::Slice(_) | Ty::View(_) | Ty::Array(..));
        if textish(&lu) && textish(&ru) {
            let el = lu.elem().cloned().unwrap_or(Ty::u8());
            let a = self.expr(l, &Ty::Slice(Box::new(el.clone())));
            let b = self.expr(r, &Ty::Slice(Box::new(el.clone())));
            let sz = self.elem_size(&el, span);
            return match op {
                BinOp::Eq => format!("zr_eq({}, {}, {})", a, b, sz),
                BinOp::Ne => format!("!zr_eq({}, {}, {})", a, b, sz),
                _ => format!("(zr_cmp({}, {}) {} 0)", a, b, sym),
            };
        }
        match (&lu, &ru) {
            (Ty::Prim(a), Ty::Prim(b))
                if a.is_int()
                    && b.is_int()
                    && signed(*a) != signed(*b)
                    && !lossless(*a, *b)
                    && !lossless(*b, *a) =>
            {
                // Exact comparison of a signed and an unsigned value (DESIGN §9.2).
                let x = self.expr(l, &lu);
                let y = self.expr(r, &ru);
                let (f, st, ut) = if bits(*a) == 128 || bits(*b) == 128 {
                    ("zr_cmp_su128", "zr_i128", "zr_u128")
                } else {
                    ("zr_cmp_su", "int64_t", "uint64_t")
                };
                if signed(*a) {
                    format!("({}(({})({}), ({})({})) {} 0)", f, st, x, ut, y, sym)
                } else {
                    format!("(-{}(({})({}), ({})({})) {} 0)", f, st, y, ut, x, sym)
                }
            }
            (Ty::Class(c, _), _) if self.prog.classes[*c].kind != ClassKind::Enum => {
                self.unsupported(span, "comparing class values")
            }
            _ => {
                let common = common_type(&lu, &ru);
                let a = self.expr(l, &common);
                let b = self.expr(r, &common);
                format!("({} {} {})", a, sym, b)
            }
        }
    }

    fn unary(&mut self, op: UnaryOp, x: &Expr, span: Span, t: Ty) -> (String, Ty) {
        match op {
            UnaryOp::Not => {
                let c = self.cond(x);
                (format!("(!{})", c), Ty::bool())
            }
            UnaryOp::Plus => {
                let v = self.expr(x, &t);
                (v, t)
            }
            UnaryOp::Neg => {
                let v = self.expr(x, &t);
                match &t {
                    Ty::Prim(p) if p.is_float() => (format!("(-{})", v), t),
                    Ty::Prim(p) => {
                        let loc = self.loc(span);
                        (format!("ZR_NEG({}, {}, {})", prim_c(*p), v, loc), t)
                    }
                    _ => (self.unsupported(span, "this negation"), t),
                }
            }
            UnaryOp::BitNot => {
                let v = self.expr(x, &t);
                let ct = self.cty(&t, span);
                (format!("(({})~{})", ct, v), t)
            }
            UnaryOp::PreInc | UnaryOp::PreDec => {
                let (p, pt) = self.place(x);
                let ct = self.cty(&pt, span);
                let step = self.arith_code(
                    if op == UnaryOp::PreInc {
                        BinOp::Add
                    } else {
                        BinOp::Sub
                    },
                    "*_p",
                    "1",
                    &pt,
                    span,
                );
                (
                    format!("({{ {} *_p = &({}); *_p = {}; }})", ct, p, step),
                    pt,
                )
            }
        }
    }

    fn cast(
        &mut self,
        ty: &ast::CastTarget,
        mode: ast::CastMode,
        x: &Expr,
        span: Span,
    ) -> (String, Ty) {
        let to = match ty {
            ast::CastTarget::Prim(p) => *p,
            ast::CastTarget::Param(id) => match self.sb(&Ty::Param(id.name.clone())) {
                Ty::Prim(p) => p,
                _ => {
                    return (
                        self.unsupported(span, "casts to a type parameter"),
                        Ty::Unknown,
                    );
                }
            },
        };
        let to = &to;
        let from = settle(self.ty(x).unref());
        let v = self.expr(x, &from);
        let tc = prim_c(*to);
        let loc = self.loc(span);
        let out = Ty::Prim(*to);
        if to.is_float() {
            return (format!("(({}){})", tc, v), out);
        }
        let (lo, hi) = min_max(*to);
        let Ty::Prim(fp) = from else {
            return (format!("(({}){})", tc, v), out);
        };
        if fp.is_float() {
            let code = match mode {
                ast::CastMode::Saturate => format!(
                    "({{ double _v = (double)({}); isnan(_v) ? ({})0 : _v <= (double){} ? ({}){} : _v >= (double){} ? ({}){} : ({})_v; }})",
                    v, tc, lo, tc, lo, hi, tc, hi, tc
                ),
                _ => format!(
                    "({{ double _v = (double)({}); if (!(_v > (double){} - 1.0 && _v < (double){} + 1.0)) zr_fail({}, \"the value doesn't fit in {}\"); ({})_v; }})",
                    v,
                    lo,
                    hi,
                    loc,
                    to.name(),
                    tc
                ),
            };
            return (code, out);
        }
        let (st, ut) = wide_temps(fp, *to);
        let code = match mode {
            ast::CastMode::Wrap => format!("(({})({})({}))", tc, unsigned_of(*to), v),
            ast::CastMode::Saturate => {
                if signed(fp) {
                    format!(
                        "({{ {st} _v = ({st})({}); _v < ({st}){} ? ({}){} : ({ut})_v > ({ut}){} && _v > 0 ? ({}){} : ({})_v; }})",
                        v,
                        lo,
                        tc,
                        lo,
                        hi,
                        tc,
                        hi,
                        tc,
                        st = st,
                        ut = ut
                    )
                } else {
                    format!(
                        "({{ {ut} _v = ({ut})({}); _v > ({ut}){} ? ({}){} : ({})_v; }})",
                        v,
                        hi,
                        tc,
                        hi,
                        tc,
                        ut = ut
                    )
                }
            }
            ast::CastMode::Checked => {
                if lossless(fp, *to) {
                    format!("(({}){})", tc, v)
                } else if signed(fp) {
                    format!(
                        "({{ {st} _v = ({st})({}); if (_v < ({st}){} || (_v > 0 && ({ut})_v > ({ut}){})) zr_fail({}, \"the value doesn't fit in {}\"); ({})_v; }})",
                        v,
                        lo,
                        hi,
                        loc,
                        to.name(),
                        tc,
                        st = st,
                        ut = ut
                    )
                } else {
                    format!(
                        "({{ {ut} _v = ({ut})({}); if (_v > ({ut}){}) zr_fail({}, \"the value doesn't fit in {}\"); ({})_v; }})",
                        v,
                        hi,
                        loc,
                        to.name(),
                        tc,
                        ut = ut
                    )
                }
            }
        };
        (code, out)
    }

    fn switch_expr(&mut self, sw: &ast::Switch, want: &Ty) -> String {
        let r = self.tmp("swr");
        let end = format!("{}_end", r);
        let ct = self.cty(want, sw.span);
        // The arms become statements inside a statement expression.
        let saved = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        self.indent = 0;
        self.switches.push(SwitchCtx {
            result: r.clone(),
            end: end.clone(),
            ty: want.clone(),
        });
        let st = settle(&self.ty(&sw.scrutinee));
        let v = self.expr(&sw.scrutinee, &st);
        let t = self.tmp("sw");
        let sct = self.cty(&st, sw.span);
        self.line(&format!("{} {} = {};", sct, t, v));
        let mut default = None;
        for rule in &sw.rules {
            let Some(labels) = &rule.labels else {
                default = Some(rule);
                continue;
            };
            let conds: Vec<String> = labels.iter().map(|l| self.case_test(&t, &st, l)).collect();
            self.line(&format!("if ({}) {{", conds.join(" || ")));
            self.switch_arm(&rule.body, &r, &end, want);
            self.line("}");
        }
        if let Some(d) = default {
            self.switch_arm(&d.body, &r, &end, want);
        } else {
            self.line("zr_fail(\"zink\", 0, \"no switch case matched\");");
        }
        self.switches.pop();
        let body = std::mem::replace(&mut self.out, saved);
        self.indent = saved_indent;
        // A `#line` directive inside must start a line of its own.
        format!("({{ {} {};\n{}{}: ; {}; }})", ct, r, body, end, r)
    }

    fn switch_arm(&mut self, b: &SwitchBody, r: &str, end: &str, want: &Ty) {
        match b {
            SwitchBody::Expr(x) => {
                let v = self.expr(x, want);
                self.line(&format!("{} = {}; goto {};", r, v, end));
            }
            SwitchBody::Block(b) => self.block(b),
        }
    }

    // ------------------------------------------------------------ text (DESIGN §4.11, §4.13)

    /// Whether `e` produces text that can be written straight into a sink.
    fn is_text(&self, e: &Expr) -> bool {
        match &e.kind {
            E::Str(parts) => parts.iter().any(|p| matches!(p, StrPart::Interp(_))),
            E::Call { callee, .. } => match &callee.kind {
                E::Member { expr, name } => {
                    let bt = settle(&self.ty(expr));
                    matches!(
                        name.name.as_str(),
                        "pad" | "zeroPad" | "hex" | "bin" | "oct" | "fixed" | "utf8"
                    ) && matches!(bt.unref(), Ty::Prim(_))
                        || matches!(name.name.as_str(), "toUpper" | "toLower" | "repeat")
                            && matches!(bt.unref(), Ty::Slice(_) | Ty::View(_) | Ty::Array(..))
                }
                _ => false,
            },
            E::Paren(x) => self.is_text(x),
            _ => false,
        }
    }

    /// Code that writes the text of `e` to the sink named `k`: statements that evaluate
    /// its parts first, then the writes.
    fn text(&mut self, e: &Expr, k: &str) -> (String, String) {
        let mut pre = String::new();
        let mut writes = String::new();
        self.text_parts(e, k, &mut pre, &mut writes);
        (pre, writes)
    }

    fn text_parts(&mut self, e: &Expr, k: &str, pre: &mut String, writes: &mut String) {
        match &e.kind {
            E::Paren(x) => self.text_parts(x, k, pre, writes),
            E::Str(parts) => {
                for p in parts {
                    match p {
                        StrPart::Text(t) => {
                            let _ = write!(writes, "zr_w_str({}, {}); ", k, lit(t));
                        }
                        StrPart::Interp(x) => self.value_text(x, k, pre, writes),
                    }
                }
            }
            E::Call { callee, args } if self.is_text(e) => {
                let E::Member { expr: base, name } = &callee.kind else {
                    return;
                };
                let bt = settle(self.ty(base).unref());
                let v = self.tmp("v");
                let bc = self.expr(base, &bt);
                let _ = write!(pre, "__typeof__({}) {} = {}; ", bc, v, bc);
                let arg = |me: &mut Self, pre: &mut String| -> String {
                    match args.first() {
                        Some(a) => {
                            let a = me.expr(&a.expr, &Ty::i64());
                            let t = me.tmp("w");
                            let _ = write!(pre, "int64_t {} = {}; ", t, a);
                            t
                        }
                        None => "0".into(),
                    }
                };
                let p = match &bt {
                    Ty::Prim(p) => Some(*p),
                    _ => None,
                };
                match (name.name.as_str(), p) {
                    ("pad" | "zeroPad", Some(p)) => {
                        let w = arg(self, pre);
                        let fill = if name.name == "pad" { "' '" } else { "'0'" };
                        let (f, cast) = if p.is_float() {
                            ("zr_w_f64_pad", "double")
                        } else if bits(p) == 128 {
                            if signed(p) {
                                ("zr_w_i128_pad", "zr_i128")
                            } else {
                                ("zr_w_u128_pad", "zr_u128")
                            }
                        } else if signed(p) {
                            ("zr_w_i64_pad", "int64_t")
                        } else {
                            ("zr_w_u64_pad", "uint64_t")
                        };
                        let _ = write!(writes, "{}({}, ({}){}, {}, {}); ", f, k, cast, v, w, fill);
                    }
                    ("hex" | "bin" | "oct", Some(p)) => {
                        let shift = match name.name.as_str() {
                            "hex" => 4,
                            "bin" => 1,
                            _ => 3,
                        };
                        if bits(p) == 128 {
                            let _ = write!(
                                writes,
                                "zr_w_radix128({}, (zr_u128)({}), {}); ",
                                k, v, shift
                            );
                        } else {
                            let _ = write!(
                                writes,
                                "zr_w_radix({}, (uint64_t)({}){}, {}, {}); ",
                                k,
                                unsigned_of(p),
                                v,
                                bits(p),
                                shift
                            );
                        }
                    }
                    ("fixed", Some(_)) => {
                        let d = arg(self, pre);
                        let _ = write!(writes, "zr_w_fixed({}, (double){}, {}); ", k, v, d);
                    }
                    ("utf8", Some(_)) => {
                        let _ = write!(writes, "zr_w_utf8({}, (uint32_t){}); ", k, v);
                    }
                    ("toUpper" | "toLower", None) => {
                        let s = self.conv(&v, &bt, &Ty::string(), e.span);
                        let (a, b) = if name.name == "toUpper" {
                            ("'a'", "'z'")
                        } else {
                            ("'A'", "'Z'")
                        };
                        let _ = write!(
                            writes,
                            "{{ zr_slice _s = {}; for (int64_t _j = 0; _j < _s.n; _j++) {{ uint8_t _c = ((uint8_t*)_s.p)[_j]; zr_w_byte({}, _c >= {} && _c <= {} ? (uint8_t)(_c ^ 32) : _c); }} }} ",
                            s, k, a, b
                        );
                    }
                    ("repeat", None) => {
                        let s = self.conv(&v, &bt, &Ty::string(), e.span);
                        let n = arg(self, pre);
                        let _ = write!(
                            writes,
                            "for (int64_t _j = 0; _j < {}; _j++) zr_w_str({}, {}); ",
                            n, k, s
                        );
                    }
                    _ => {
                        let u = self.unsupported(e.span, "this text method");
                        let _ = write!(writes, "(void){}; ", u);
                    }
                }
            }
            _ => self.value_text(e, k, pre, writes),
        }
    }

    /// Writes one value as `${...}` prints it (DESIGN §4.13).
    fn value_text(&mut self, x: &Expr, k: &str, pre: &mut String, writes: &mut String) {
        if self.is_text(x) {
            self.text_parts(x, k, pre, writes);
            return;
        }
        let t = settle(self.ty(x).unref());
        let v = self.tmp("p");
        let code = self.expr(x, &t);
        let _ = write!(pre, "__typeof__({}) {} = {}; ", code, v, code);
        match &t {
            Ty::Slice(_) | Ty::View(_) if t.elem() == Some(&Ty::u8()) => {
                let _ = write!(writes, "zr_w_str({}, {}); ", k, v);
            }
            Ty::Prim(Prim::Bool) => {
                let _ = write!(writes, "zr_w_bool({}, {}); ", k, v);
            }
            Ty::Prim(p) if p.is_float() => {
                let _ = write!(writes, "zr_w_f64({}, (double){}); ", k, v);
            }
            Ty::Prim(Prim::I128) => {
                let _ = write!(writes, "zr_w_i128({}, {}); ", k, v);
            }
            Ty::Prim(Prim::U128) => {
                let _ = write!(writes, "zr_w_u128({}, {}); ", k, v);
            }
            Ty::Prim(p) if signed(*p) => {
                let _ = write!(writes, "zr_w_i64({}, (int64_t){}); ", k, v);
            }
            Ty::Prim(_) => {
                let _ = write!(writes, "zr_w_u64({}, (uint64_t){}); ", k, v);
            }
            Ty::Array(e, _) | Ty::Slice(e) | Ty::View(e) => {
                let el = (**e).clone();
                let ec = self.cty(&el, x.span);
                let lay = layout(&t).unwrap_or(Layout::Slice);
                let looped = Arr {
                    a: format!("(&{})", v),
                    lay,
                    ec: ec.clone(),
                    cap: cap_of(&t),
                };
                let (len, at) = (looped.len(), looped.at("_j"));
                if el == Ty::u8() {
                    // A u8 array prints as its text (DESIGN §4.13).
                    let _ = write!(
                        writes,
                        "for (int64_t _j = 0; _j < {}; _j++) zr_w_byte({}, {}); ",
                        len, k, at
                    );
                    return;
                }
                // Any other array prints as a list: [104, 105].
                let one = match &el {
                    Ty::Prim(p) if p.is_float() => format!("zr_w_f64({}, (double)_e)", k),
                    Ty::Prim(Prim::Bool) => format!("zr_w_bool({}, _e)", k),
                    Ty::Prim(Prim::I128) => format!("zr_w_i128({}, _e)", k),
                    Ty::Prim(Prim::U128) => format!("zr_w_u128({}, _e)", k),
                    Ty::Prim(p) if signed(*p) => format!("zr_w_i64({}, (int64_t)_e)", k),
                    Ty::Prim(_) => format!("zr_w_u64({}, (uint64_t)_e)", k),
                    Ty::Slice(_) => format!("zr_w_str({}, _e)", k),
                    _ => self.unsupported(x.span, "printing this array"),
                };
                let _ = write!(
                    writes,
                    "{{ zr_w_byte({k}, '['); for (int64_t _j = 0; _j < {len}; _j++) {{ {ec} _e = {at}; if (_j) zr_w_str({k}, zr_lit(\", \", 2)); {one}; }} zr_w_byte({k}, ']'); }} ",
                    k = k,
                    len = len,
                    ec = ec,
                    at = at,
                    one = one
                );
            }
            _ => {
                let u = self.unsupported(x.span, "printing this value");
                let _ = write!(writes, "(void){}; ", u);
            }
        }
    }

    // ------------------------------------------------------------ calls

    fn call_any(&mut self, e: &Expr) -> CallOut {
        match &e.kind {
            E::Call { callee, args } => self.call(e, callee, args),
            E::New { args, .. } => {
                let t = settle(&self.ty(e));
                let ct = self.cty(&t, e.span);
                if let Ty::Class(id, _) = &t
                    && self.growable_fields(*id) > 0
                {
                    let u = self.unsupported(e.span, "a temporary value with a growable field");
                    return CallOut {
                        code: u,
                        ret: t,
                        fails: false,
                    };
                }
                let fails = matches!(self.facts.calls.get(&key(e.span)), Some(f) if matches!(f.origin, Origin::Ctor(c, i) if self.prog.classes[c].ctors[i].can_fail));
                let tmp = self.tmp("new");
                let st = self.construct(&tmp, args, e.span);
                let rt = if fails {
                    self.result_ty(&t, e.span)
                } else {
                    String::new()
                };
                let code = if fails {
                    format!(
                        "({{ {} {}; uint8_t _s = {}; ({}){{{}, _s}}; }})",
                        ct, tmp, st, rt, tmp
                    )
                } else {
                    format!("({{ {} {}; {}; {}; }})", ct, tmp, st, tmp)
                };
                CallOut {
                    code,
                    ret: t,
                    fails,
                }
            }
            E::ThisNew(args) => {
                let st = self.construct("(*self)", args, e.span);
                let fails = matches!(self.facts.calls.get(&key(e.span)), Some(f) if matches!(f.origin, Origin::Ctor(c, i) if self.prog.classes[c].ctors[i].can_fail));
                CallOut {
                    code: st,
                    ret: Ty::Void,
                    fails,
                }
            }
            _ => {
                let (code, t) = self.rv(e);
                CallOut {
                    code,
                    ret: t,
                    fails: false,
                }
            }
        }
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[ast::Arg]) -> CallOut {
        // Taken first, so the calls in the arguments don't see it.
        let direct = std::mem::take(&mut self.direct_result);
        let ret = settle(&self.ty(e));
        // `stdin.readAll(buf)` and `stdin.nextLine(buf)`: the first of the library (STDLIB §4.1),
        // known by name until the checker models `File`.
        if let E::Member { expr: base, name } = &callee.kind
            && matches!(&base.kind, E::Ident(n) if n == "stdin")
            && !self.facts.names.contains_key(&key(base.span))
            && args.len() == 1
            && matches!(name.name.as_str(), "readAll" | "nextLine")
            && layout(&self.ty(&args[0].expr)) == Some(Layout::Vec)
        {
            let (p, _) = self.place(&args[0].expr);
            if name.name == "readAll" {
                return CallOut {
                    code: format!("zr_read_all(0, &({}))", p),
                    ret: Ty::Void,
                    fails: true,
                };
            }
            let rt = self.result_ty(&Ty::string(), e.span);
            return CallOut {
                code: format!(
                    "({{ zr_slice _l; uint8_t _s = zr_next_line(0, &({}), &_l); ({}){{_l, _s}}; }})",
                    p, rt
                ),
                ret: Ty::string(),
                fails: true,
            };
        }
        let Some(fact) = self.facts.calls.get(&key(e.span)) else {
            let u = self.unsupported(e.span, "calls into library packages that aren't written");
            return CallOut {
                code: u,
                ret,
                fails: false,
            };
        };
        match &fact.origin {
            Origin::Method(c, i) => {
                let (mut c, mut i) = (*c, *i);
                // In an instantiation, a type parameter bounded by an interface is a
                // known class, so its method is called directly, with no dispatch.
                if self.prog.classes[c].kind == ClassKind::Interface
                    && let E::Member { expr: base, .. } = &callee.kind
                    && let Ty::Class(k, _) = settle(self.ty(base).unref())
                    && self.prog.classes[k].kind != ClassKind::Interface
                {
                    let im = &self.prog.classes[c].methods[i];
                    let same: Vec<usize> = self.prog.classes[k]
                        .methods
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.name == im.name && m.params.len() == im.params.len())
                        .map(|(j, _)| j)
                        .collect();
                    let pick = same
                        .iter()
                        .copied()
                        .find(|&j| self.prog.classes[k].methods[j].params == im.params)
                        .or_else(|| same.first().copied());
                    if let Some(j) = pick {
                        (c, i) = (k, j);
                    }
                }
                let m = &self.prog.classes[c].methods[i];
                if self.prog.classes[c].kind == ClassKind::Interface {
                    let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                    if let E::Member { expr: base, .. } = &callee.kind {
                        return self.iface_call(base, c, i, args, &params, e.span);
                    }
                    let u = self.unsupported(e.span, "this call through an interface");
                    return CallOut {
                        code: u,
                        ret,
                        fails: false,
                    };
                }
                let fails = m.can_fail;
                let is_static = m.is_static;
                let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                let recv_arg = fact.recv_arg;
                // A generic class's type arguments come from the receiver's type.
                let recv = match &callee.kind {
                    E::Member { expr: base, .. } if !recv_arg => Some(self.ty(base)),
                    _ => None,
                };
                let targs = fact.targs.clone();
                let (fname, map) = self.method_ref(c, i, recv.as_ref(), &targs, e.span);
                let mut cargs = Vec::new();
                let mut rest: &[Ty] = &params;
                if let E::Member { expr: base, .. } = &callee.kind {
                    let bt = self.ty(base);
                    if recv_arg {
                        cargs.push(self.expr(base, &params[0]));
                        rest = &params[1..];
                    } else if !is_static {
                        cargs.push(self.receiver(base, &bt));
                    }
                } else if !is_static {
                    cargs.push("self".into());
                }
                let runtime_c = m.c_name.as_deref().filter(|n| n.starts_with("zr_"));
                if runtime_c.is_some() {
                    // The runtime's own C takes Zinkite's representations as they are.
                    for (a, p) in args.iter().zip(rest) {
                        cargs.push(self.expr(&a.expr, p));
                    }
                } else if m.c_name.is_some() {
                    // An explicit `extern`: each argument as its declaration says it
                    // crosses; `string` copied with a NUL, a slice as its pointer.
                    for (k, (a, p)) in args.iter().zip(rest).enumerate() {
                        let v = self.expr(&a.expr, p);
                        let nul = m
                            .decl
                            .params
                            .get(k)
                            .is_some_and(|d| matches!(d.ty.kind, ast::TypeKind::String));
                        cargs.push(self.c_arg(v, p, nul));
                    }
                } else {
                    for (a, p) in args.iter().zip(rest) {
                        cargs.push(self.expr(&a.expr, p));
                    }
                }
                let fname = runtime_c.map_or(fname, str::to_string);
                let mut code = format!("{}({})", fname, cargs.join(", "));
                let r = if map.is_empty() {
                    self.sb(&m.ret)
                } else {
                    m.ret.subst(&map)
                };
                let r = settle(&r);
                // A returned array points into the callee's data stack, which its next call
                // reuses. Unless it goes straight into a variable, it is copied to the
                // scratch stack, where a temporary lasts to the end of its statement:
                // `path.relative(path.absolute(a), path.absolute(b))` (DESIGN §3.3).
                if !direct
                    && m.c_name.is_none()
                    && layout(&r) == Some(Layout::Vec)
                    && let Ty::Array(el, _) = &r
                {
                    let sz = self.elem_size(&el.clone(), e.span);
                    self.used_scratch = true;
                    code = if fails {
                        format!(
                            "({{ __typeof__({c}) _rv = {c}; _rv.v = zr_vec_scratch(_rv.v, {sz}); _rv; }})",
                            c = code,
                            sz = sz
                        )
                    } else {
                        format!("zr_vec_scratch({}, {})", code, sz)
                    };
                }
                CallOut {
                    code,
                    ret: r,
                    fails,
                }
            }
            // A function from a C header: `string`s go as NUL-terminated copies.
            Origin::Extern(cname) => {
                let cname = cname.clone();
                let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                let mut cargs = Vec::new();
                for (a, p) in args.iter().zip(&params) {
                    let v = self.expr(&a.expr, p);
                    cargs.push(self.c_arg(v, p, true));
                }
                CallOut {
                    code: format!("{}({})", cname, cargs.join(", ")),
                    ret,
                    fails: false,
                }
            }
            Origin::Value => {
                let ft = settle(&self.ty(callee));
                let f = self.expr(callee, &ft);
                let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                let cargs: Vec<String> = args
                    .iter()
                    .zip(&params)
                    .map(|(a, p)| self.expr(&a.expr, p))
                    .collect();
                CallOut {
                    code: format!("({})({})", f, cargs.join(", ")),
                    ret,
                    fails: false,
                }
            }
            Origin::Builtin => {
                let name = fact.name.clone();
                let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                self.builtin(e, callee, &name, args, &params, ret)
            }
            _ => {
                let u = self.unsupported(e.span, "this call");
                CallOut {
                    code: u,
                    ret,
                    fails: false,
                }
            }
        }
    }

    /// A pointer to the object a method is called on.
    fn receiver(&mut self, base: &Expr, bt: &Ty) -> String {
        if is_ref(bt) {
            return self.rv(base).0;
        }
        if self.is_place(base) {
            let (p, _) = self.place(base);
            return format!("&({})", p);
        }
        let v = self.expr(base, bt);
        format!(
            "({{ __typeof__({}) *_o = &(__typeof__({})){{0}}; *_o = {}; _o; }})",
            v, v, v
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn builtin(
        &mut self,
        e: &Expr,
        callee: &Expr,
        name: &str,
        args: &[ast::Arg],
        params: &[Ty],
        ret: Ty,
    ) -> CallOut {
        let span = e.span;
        let loc = self.loc(span);
        let arg = |me: &mut Self, k: usize| -> String {
            let p = params
                .get(k)
                .cloned()
                .unwrap_or_else(|| settle(&me.ty(&args[k].expr)));
            me.expr(&args[k].expr, &p)
        };
        let ok = |code: String, ret: Ty| CallOut {
            code,
            ret,
            fails: false,
        };
        let fail = |code: String, ret: Ty| CallOut {
            code,
            ret,
            fails: true,
        };
        match &callee.kind {
            // print, println, errprint, errprintln, exit, fail, min, max, abs, clamp.
            E::Ident(f) => {
                return match f.as_str() {
                    "print" | "println" | "errprint" | "errprintln" => {
                        let fd = if f.starts_with("err") { 2 } else { 1 };
                        let k = format!("zr_fd({})", fd);
                        let (pre, writes) = if self.is_text(&args[0].expr) {
                            self.text(&args[0].expr, &k)
                        } else {
                            let s = self.expr(&args[0].expr, &Ty::string());
                            (String::new(), format!("zr_w_str({}, {}); ", k, s))
                        };
                        let nl = if f.ends_with("ln") {
                            format!("zr_end_line({});", fd)
                        } else {
                            String::new()
                        };
                        ok(format!("({{ {} {} {} }})", pre, writes, nl), Ty::Void)
                    }
                    "exit" => ok(format!("zr_exit({})", arg(self, 0)), Ty::Void),
                    "fail" => ok(format!("zr_fail_text({}, {})", loc, arg(self, 0)), Ty::Void),
                    "min" | "max" | "clamp" | "abs" => {
                        let t = params.first().cloned().unwrap_or(ret.clone());
                        let vs: Vec<String> = (0..args.len()).map(|k| arg(self, k)).collect();
                        let ct = self.cty(&t, span);
                        let code = match f.as_str() {
                            "min" => format!(
                                "({{ {} _a = {}, _b = {}; _b < _a ? _b : _a; }})",
                                ct, vs[0], vs[1]
                            ),
                            "max" => format!(
                                "({{ {} _a = {}, _b = {}; _a < _b ? _b : _a; }})",
                                ct, vs[0], vs[1]
                            ),
                            "clamp" => format!(
                                "({{ {} _x = {}, _lo = {}, _hi = {}; _x < _lo ? _lo : _hi < _x ? _hi : _x; }})",
                                ct, vs[0], vs[1], vs[2]
                            ),
                            _ => {
                                let neg = self.arith_code(BinOp::Sub, "0", "_a", &t, span);
                                format!("({{ {} _a = {}; _a < 0 ? {} : _a; }})", ct, vs[0], neg)
                            }
                        };
                        if !matches!(t, Ty::Prim(_)) && f != "min" && f != "max" {
                            let u = self.unsupported(span, "this function on non-numbers");
                            return ok(u, ret);
                        }
                        ok(code, t)
                    }
                    _ => ok(self.unsupported(span, &format!("`{}`", f)), ret),
                };
            }
            E::PrimMember { prim, name: m } => {
                return match m.name.as_str() {
                    "parse" => {
                        let s = arg(self, 0);
                        let base = if args.len() > 1 {
                            arg(self, 1)
                        } else {
                            "10".into()
                        };
                        let p = *prim;
                        let code = self.parse_code(p, &s, &base, span);
                        fail(code, Ty::Prim(p))
                    }
                    // The number at the start of s, and the rest of s (STDLIB §2.4).
                    "parsePrefix" => {
                        let s = arg(self, 0);
                        let p = *prim;
                        let tt = Ty::Tuple(vec![Ty::Prim(p), Ty::string()]);
                        let rt = self.result_ty(&tt, span);
                        let q = self.parse_code(p, "((zr_slice){_ps.p, _k})", "10", span);
                        fail(
                            format!(
                                "({{ zr_slice _ps = {}; int64_t _k = zr_num_prefix(_ps, {}); {} _r = {{0}}; if (_k == 0) {{ _r.s = 22; }} else {{ __typeof__({}) _q = {}; _r.v.a0 = _q.v; _r.s = _q.s; }} _r.v.a1 = (zr_slice){{(uint8_t *)_ps.p + _k, _ps.n - _k}}; _r; }})",
                                s,
                                i32::from(p.is_float()),
                                rt,
                                q,
                                q
                            ),
                            tt,
                        )
                    }
                    // A checked conversion that reports RANGE, and gives 0, where a cast
                    // would stop the program (STDLIB §2).
                    "from" if prim.is_int() => {
                        let to = *prim;
                        let ft = settle(self.ty(&args[0].expr).unref());
                        let x = self.expr(&args[0].expr, &ft);
                        let tc = prim_c(to);
                        let rt = self.result_ty(&Ty::Prim(to), span);
                        let (lo, hi) = min_max(to);
                        let code = match ft {
                            Ty::Prim(fp) if lossless(fp, to) => {
                                format!("(({}){{({})({}), 0}})", rt, tc, x)
                            }
                            Ty::Prim(fp) if fp.is_float() => format!(
                                "({{ double _v = (double)({}); {} _r = {{0, 34}}; if (_v > (double){} - 1.0 && _v < (double){} + 1.0) {{ _r.v = ({})_v; _r.s = 0; }} _r; }})",
                                x, rt, lo, hi, tc
                            ),
                            Ty::Prim(fp) if signed(fp) => {
                                let (st, ut) = wide_temps(fp, to);
                                format!(
                                    "({{ {st} _v = ({st})({}); {} _r = {{0, 34}}; if (!(_v < ({st}){} || (_v > 0 && ({ut})_v > ({ut}){}))) {{ _r.v = ({})_v; _r.s = 0; }} _r; }})",
                                    x,
                                    rt,
                                    lo,
                                    hi,
                                    tc,
                                    st = st,
                                    ut = ut
                                )
                            }
                            Ty::Prim(fp) => {
                                let (_, ut) = wide_temps(fp, to);
                                format!(
                                    "({{ {ut} _v = ({ut})({}); {} _r = {{0, 34}}; if (_v <= ({ut}){}) {{ _r.v = ({})_v; _r.s = 0; }} _r; }})",
                                    x,
                                    rt,
                                    hi,
                                    tc,
                                    ut = ut
                                )
                            }
                            _ => {
                                return ok(self.unsupported(span, "`from` on this value"), ret);
                            }
                        };
                        fail(code, Ty::Prim(to))
                    }
                    _ => ok(
                        self.unsupported(span, &format!("`{}.{}`", prim.name(), m.name)),
                        ret,
                    ),
                };
            }
            _ => {}
        }
        let E::Member {
            expr: base,
            name: mname,
        } = &callee.kind
        else {
            return ok(self.unsupported(span, "this call"), ret);
        };
        let bt = self.ty(base);
        // `T.parse(s)` in an instantiation where `T` is a number type: as `u8.parse(s)`.
        if let Ty::TypeName(inner) = &bt
            && let Ty::Prim(p) = **inner
        {
            let as_prim = Expr {
                kind: E::PrimMember {
                    prim: p,
                    name: mname.clone(),
                },
                span: callee.span,
            };
            return self.builtin(e, &as_prim, name, args, params, ret);
        }
        if let Ty::Namespace(ns) = &bt {
            if *ns == "extern" && name == "errno" {
                return ok("((uint8_t)errno)".into(), Ty::u8());
            }
            if *ns == "status" && (name == "message" || name == "name") {
                return ok(format!("zr_status_message({})", arg(self, 0)), Ty::string());
            }
            if *ns == "math" {
                let fname = match name {
                    "roundToI64" => return ok(self.unsupported(span, "`math.roundToI64`"), ret),
                    n => n.to_string(),
                };
                let vs: Vec<String> = (0..args.len()).map(|k| arg(self, k)).collect();
                let f32 = ret == Ty::Prim(Prim::F32);
                let cname = if f32 { format!("{}f", fname) } else { fname };
                let cname = match cname.as_str() {
                    "gcd" | "lcm" | "ipow" | "isqrt" | "gcdf" | "lcmf" => {
                        return ok(self.unsupported(span, "this `math` function"), ret);
                    }
                    _ => cname,
                };
                return ok(format!("{}({})", cname, vs.join(", ")), ret);
            }
        }
        let recv_t = settle(bt.unref());
        let m = mname.name.as_str();
        // Numbers.
        if let Ty::Prim(p) = recv_t {
            let v = self.expr(base, &Ty::Prim(p));
            return match m {
                "isNaN" => ok(format!("isnan({})", v), Ty::bool()),
                "isFinite" => ok(format!("isfinite({})", v), Ty::bool()),
                "compareTo" => {
                    let o = arg(self, 0);
                    ok(
                        format!(
                            "({{ __typeof__({}) _a = {}, _b = {}; (int32_t)(_a < _b ? -1 : _a > _b ? 1 : 0); }})",
                            v, v, o
                        ),
                        Ty::Prim(Prim::I32),
                    )
                }
                "isDigit" => ok(
                    format!("({{ uint8_t _c = {}; _c >= '0' && _c <= '9'; }})", v),
                    Ty::bool(),
                ),
                "isAlpha" => ok(
                    format!("({{ uint8_t _c = {} | 32; _c >= 'a' && _c <= 'z'; }})", v),
                    Ty::bool(),
                ),
                "isSpace" => ok(format!("zr_space({})", v), Ty::bool()),
                "isUpper" => ok(
                    format!("({{ uint8_t _c = {}; _c >= 'A' && _c <= 'Z'; }})", v),
                    Ty::bool(),
                ),
                "isLower" => ok(
                    format!("({{ uint8_t _c = {}; _c >= 'a' && _c <= 'z'; }})", v),
                    Ty::bool(),
                ),
                "toUpper" => ok(
                    format!(
                        "({{ uint8_t _c = {}; (uint8_t)(_c >= 'a' && _c <= 'z' ? _c ^ 32 : _c); }})",
                        v
                    ),
                    Ty::u8(),
                ),
                "toLower" => ok(
                    format!(
                        "({{ uint8_t _c = {}; (uint8_t)(_c >= 'A' && _c <= 'Z' ? _c ^ 32 : _c); }})",
                        v
                    ),
                    Ty::u8(),
                ),
                "divOr" => {
                    let d = arg(self, 0);
                    let f = arg(self, 1);
                    let div = self.arith_code(BinOp::Div, "_x", "_d", &Ty::Prim(p), span);
                    ok(
                        format!(
                            "({{ {} _x = {}, _d = {}, _f = {}; _d == 0 ? _f : {}; }})",
                            prim_c(p),
                            v,
                            d,
                            f,
                            div
                        ),
                        Ty::Prim(p),
                    )
                }
                "popCount" if bits(p) == 128 => ok(
                    format!(
                        "({{ zr_u128 _x = (zr_u128)({}); (int64_t)(__builtin_popcountll((uint64_t)_x) + __builtin_popcountll((uint64_t)(_x >> 64))); }})",
                        v
                    ),
                    Ty::i64(),
                ),
                "popCount" => ok(
                    format!(
                        "(int64_t)__builtin_popcountll((uint64_t)({})({}))",
                        unsigned_of(p),
                        v
                    ),
                    Ty::i64(),
                ),
                _ => ok(self.unsupported(span, &format!("`{}` on numbers", m)), ret),
            };
        }
        if matches!(recv_t, Ty::Map(..) | Ty::Set(_)) {
            return self.map_method(base, m, args, ret, span);
        }
        let Some(el) = recv_t.elem().cloned() else {
            return ok(
                self.unsupported(span, &format!("`{}` on {}", m, self.prog.show(&recv_t))),
                ret,
            );
        };
        let is_u8 = el == Ty::u8();
        // Text methods take their receiver as a string (STDLIB §3).
        if is_u8 {
            let first_string = params.first().is_some_and(|p| p.is_string());
            if crate::builtins::slice_receiver(m, first_string)
                && !matches!(m, "lines" | "words" | "splitBy" | "codePoints")
            {
                let s = self.expr(base, &Ty::string());
                let a0 = if args.is_empty() {
                    String::new()
                } else {
                    arg(self, 0)
                };
                return match m {
                    "trim" => ok(format!("zr_trim({}, 1, 1)", s), Ty::string()),
                    "trimStart" => ok(format!("zr_trim({}, 1, 0)", s), Ty::string()),
                    "trimEnd" => ok(format!("zr_trim({}, 0, 1)", s), Ty::string()),
                    "startsWith" => ok(format!("zr_starts({}, {})", s, a0), Ty::bool()),
                    "endsWith" => ok(format!("zr_ends({}, {})", s, a0), Ty::bool()),
                    "contains" => ok(format!("(zr_find_from({}, {}, 0) >= 0)", s, a0), Ty::bool()),
                    "find" => {
                        let from = if args.len() > 1 {
                            arg(self, 1)
                        } else {
                            "0".into()
                        };
                        fail(format!("zr_find({}, {}, {})", s, a0, from), Ty::i64())
                    }
                    "rfind" => fail(format!("zr_rfind({}, {})", s, a0), Ty::i64()),
                    "count" => ok(format!("zr_count({}, {})", s, a0), Ty::i64()),
                    "compareTo" => ok(format!("zr_cmp({}, {})", s, a0), Ty::Prim(Prim::I32)),
                    "compareNatural" => ok(
                        format!("zr_cmp_natural({}, {})", s, a0),
                        Ty::Prim(Prim::I32),
                    ),
                    "splitFirst" | "splitLast" => {
                        let tt = Ty::Tuple(vec![Ty::string(), Ty::string()]);
                        let tc = self.cty(&tt, span);
                        ok(
                            format!(
                                "({{ zr_pair _p = zr_split_first({}, {}, {}); ({}){{_p.a, _p.b}}; }})",
                                s,
                                a0,
                                i32::from(m == "splitLast"),
                                tc
                            ),
                            tt,
                        )
                    }
                    "trimPrefix" => ok(
                        format!(
                            "({{ zr_slice _s = {}, _x = {}; zr_starts(_s, _x) ? (zr_slice){{(uint8_t*)_s.p + _x.n, _s.n - _x.n}} : _s; }})",
                            s, a0
                        ),
                        Ty::string(),
                    ),
                    "trimSuffix" => ok(
                        format!(
                            "({{ zr_slice _s = {}, _x = {}; zr_ends(_s, _x) ? (zr_slice){{_s.p, _s.n - _x.n}} : _s; }})",
                            s, a0
                        ),
                        Ty::string(),
                    ),
                    _ => ok(self.unsupported(span, &format!("`{}`", m)), ret),
                };
            }
        }
        // Array methods (DESIGN §4.7, §4.9, STDLIB §7.1).
        let Some(lay) = layout(&recv_t) else {
            return ok(
                self.unsupported(span, &format!("`{}` on {}", m, self.prog.show(&recv_t))),
                ret,
            );
        };
        let ec = self.cty(&el, span);
        let cap = match &recv_t {
            Ty::Array(_, k) => k.size,
            _ => 0,
        };
        let a = self.tmp("a");
        let ptr = if is_ref(&bt) || self.is_place(base) {
            let (p, _) = self.place(base);
            format!("__typeof__({}) *{} = &({});", p, a, p)
        } else {
            let v = self.expr(base, &recv_t);
            format!(
                "__typeof__({}) {}v = {}; __typeof__({}v) *{} = &{}v;",
                v, a, v, a, a, a
            )
        };
        let arr = Arr {
            a: a.clone(),
            lay,
            ec: ec.clone(),
            cap,
        };
        let len = arr.len();
        let res = |me: &mut Self| me.result_ty(&el, span);
        let put = |me: &mut Self, at: Where, x: String| -> CallOut {
            let body = arr.put(at);
            if lay.capped() {
                let rt = me.result_ty(&Ty::Ref(Box::new(el.clone())), span);
                return CallOut {
                    code: format!(
                        "({{ {} uint8_t _st = 0; {} _x = {}; {} *_e; {} ({}){{_e, _st}}; }})",
                        ptr, ec, x, ec, body, rt
                    ),
                    ret: Ty::Ref(Box::new(el.clone())),
                    fails: true,
                };
            }
            CallOut {
                code: format!(
                    "({{ {} uint8_t _st = 0; {} _x = {}; {} *_e; {} (void)_st; _e; }})",
                    ptr, ec, x, ec, body
                ),
                ret: Ty::Ref(Box::new(el.clone())),
                fails: false,
            }
        };
        let take = |me: &mut Self, at: Where| -> CallOut {
            let rt = me.result_ty(&el, span);
            CallOut {
                code: format!(
                    "({{ {} {} _v = {{0}}; uint8_t _s = 192; {} ({}){{_v, _s}}; }})",
                    ptr,
                    ec,
                    arr.take(at),
                    rt
                ),
                ret: el.clone(),
                fails: true,
            }
        };
        match m {
            "push" => {
                let x = arg(self, 0);
                put(self, Where::End, x)
            }
            "pushFront" => {
                let x = arg(self, 0);
                put(self, Where::Front, x)
            }
            "insert" => {
                let x = arg(self, 0);
                put(self, Where::Cursor, x)
            }
            "pop" => take(self, Where::End),
            "popFront" => take(self, Where::Front),
            "removeBefore" => take(self, Where::Before),
            "removeAfter" => take(self, Where::After),
            "clear" => ok(format!("({{ {} {} }})", ptr, arr.clear()), Ty::Void),
            "seek" => fail(
                format!("({{ {} {}; }})", ptr, arr.seek(&arg(self, 0))),
                Ty::Void,
            ),
            "seekFromEnd" => {
                let k = arg(self, 0);
                let fails = !matches!(args[0].expr.kind, E::Int { value: 0, .. });
                CallOut {
                    code: format!(
                        "({{ {} {}; }})",
                        ptr,
                        arr.seek(&format!("{} - (int64_t)({})", len, k))
                    ),
                    ret: Ty::Void,
                    fails,
                }
            }
            "seekStart" => ok(
                format!("({{ {} (void){}; }})", ptr, arr.seek("0")),
                Ty::Void,
            ),
            "move" => ok(
                format!("({{ {} {}; }})", ptr, arr.mv(&arg(self, 0))),
                Ty::Void,
            ),
            "insertAt" => {
                let i = arg(self, 0);
                let x = arg(self, 1);
                let rt = self.result_ty(&Ty::Ref(Box::new(el.clone())), span);
                fail(
                    format!(
                        "({{ {} {} _x = {}; static {} _none; {} *_e = &_none; uint8_t _st = {}; if (!_st) {{ {} }} ({}){{_e, _st}}; }})",
                        ptr,
                        ec,
                        x,
                        ec,
                        ec,
                        arr.seek(&i),
                        arr.put(Where::Cursor),
                        rt
                    ),
                    Ty::Ref(Box::new(el)),
                )
            }
            "removeAt" => {
                let i = arg(self, 0);
                let rt = res(self);
                fail(
                    format!(
                        "({{ {} {} _v = {{0}}; uint8_t _s = {}; if (!_s) {{ _s = 192; {} }} ({}){{_v, _s}}; }})",
                        ptr,
                        ec,
                        arr.seek(&i),
                        arr.take(Where::After),
                        rt
                    ),
                    el,
                )
            }
            "truncate" => {
                let n = arg(self, 0);
                let body = if lay == Layout::Cur {
                    format!(
                        "while (zr_cur_len({a}) > _n) zr_data_release(&{a}->d, zr_cur_take_back({a}), sizeof({ec}));",
                        a = a,
                        ec = ec
                    )
                } else if lay == Layout::Chk {
                    format!(
                        "while ({a}->n > _n) zr_data_release(&{a}->d, zr_chk_take({a}, 0), sizeof({ec}));",
                        a = a,
                        ec = ec
                    )
                } else if lay == Layout::Gap {
                    // The kept elements after the gap move up to the end of the buffer.
                    format!(
                        "if (_n < (int64_t){a}->gs + {a}->an) {{ if (_n <= {a}->gs) {{ {a}->gs = _n; {a}->an = 0; }} else {{ int64_t _keep = _n - {a}->gs; memmove(&{a}->e[{k} - _keep], &{a}->e[{k} - {a}->an], (size_t)_keep * sizeof({ec})); {a}->an = _keep; }} if ({a}->cur > _n) {a}->cur = _n; }}",
                        a = a,
                        k = cap,
                        ec = ec
                    )
                } else {
                    format!("if (_n < {a}->n) {a}->n = _n;", a = a)
                };
                ok(
                    format!(
                        "({{ {} int64_t _n = {}; if (_n < 0) _n = 0; {} }})",
                        ptr, n, body
                    ),
                    Ty::Void,
                )
            }
            "reverse" => {
                let code = if let Some((setup, finish)) = arr.gather() {
                    // The index is reordered; the elements stay where they are (STDLIB §7.1).
                    if lay == Layout::Chk {
                        self.used_scratch = true;
                    }
                    format!(
                        "({{ {} {} for (int64_t _i = 0, _j = _n - 1; _i < _j; _i++, _j--) {{ uint32_t _t = _d[_i]; _d[_i] = _d[_j]; _d[_j] = _t; }} {} }})",
                        ptr, setup, finish
                    )
                } else {
                    format!(
                        "({{ {} for (int64_t _i = 0, _j = {} - 1; _i < _j; _i++, _j--) {{ {} _t = {}; {} = {}; {} = _t; }} }})",
                        ptr,
                        len,
                        ec,
                        arr.at("_i"),
                        arr.at("_i"),
                        arr.at("_j"),
                        arr.at("_j")
                    )
                };
                ok(code, Ty::Void)
            }
            "swap" => {
                let i = arg(self, 0);
                let j = arg(self, 1);
                let body = if lay == Layout::Cur {
                    format!(
                        "uint32_t *_pi = _i < {a}->bn ? &{a}->bp[_i] : &{a}->ap[_i - {a}->bn], *_pj = _j < {a}->bn ? &{a}->bp[_j] : &{a}->ap[_j - {a}->bn]; uint32_t _t = *_pi; *_pi = *_pj; *_pj = _t;",
                        a = a
                    )
                } else if lay == Layout::Chk {
                    format!(
                        "uint32_t *_pi = zr_chk_entry({a}, _i), *_pj = zr_chk_entry({a}, _j); uint32_t _t = *_pi; *_pi = *_pj; *_pj = _t;",
                        a = a
                    )
                } else {
                    format!(
                        "{} _t = {}; {} = {}; {} = _t;",
                        ec,
                        arr.at("_i"),
                        arr.at("_i"),
                        arr.at("_j"),
                        arr.at("_j")
                    )
                };
                ok(
                    format!(
                        "({{ {} int64_t _i = zr_ix({}, {}, {}), _j = zr_ix({}, {}, {}); {} }})",
                        ptr, i, len, loc, j, len, loc, body
                    ),
                    Ty::Void,
                )
            }
            "fill" => {
                let x = arg(self, 0);
                ok(
                    format!(
                        "({{ {} {} _x = {}; for (int64_t _i = 0; _i < {}; _i++) {} = _x; }})",
                        ptr,
                        ec,
                        x,
                        len,
                        arr.at("_i")
                    ),
                    Ty::Void,
                )
            }
            "getOr" => {
                let i = arg(self, 0);
                let f = arg(self, 1);
                ok(
                    format!(
                        "({{ {} int64_t _i = {}; _i >= 0 && _i < {} ? {} : {}; }})",
                        ptr,
                        i,
                        len,
                        arr.at("_i"),
                        f
                    ),
                    el,
                )
            }
            "contains" | "indexOf" if matches!(el, Ty::Prim(_)) => {
                let x = arg(self, 0);
                if m == "contains" {
                    ok(
                        format!(
                            "({{ {} {} _x = {}; bool _f = false; for (int64_t _i = 0; _i < {}; _i++) if ({} == _x) {{ _f = true; break; }} _f; }})",
                            ptr,
                            ec,
                            x,
                            len,
                            arr.at("_i")
                        ),
                        Ty::bool(),
                    )
                } else {
                    fail(
                        format!(
                            "({{ {} {} _x = {}; zr_res_i64 _r = {{0, 195}}; for (int64_t _i = 0; _i < {}; _i++) if ({} == _x) {{ _r.v = _i; _r.s = 0; break; }} _r; }})",
                            ptr,
                            ec,
                            x,
                            len,
                            arr.at("_i")
                        ),
                        Ty::i64(),
                    )
                }
            }
            "sum" if matches!(el, Ty::Prim(_)) => {
                let add = self.arith_code(BinOp::Add, "_t", &arr.at("_i"), &el, span);
                ok(
                    format!(
                        "({{ {} {} _t = 0; for (int64_t _i = 0; _i < {}; _i++) _t = {}; _t; }})",
                        ptr, ec, len, add
                    ),
                    el,
                )
            }
            "min" | "max" if matches!(el, Ty::Prim(_)) => {
                let rt = res(self);
                let cmp = if m == "min" { "<" } else { ">" };
                fail(
                    format!(
                        "({{ {} {} _r = {{0, 192}}; if ({}) {{ _r.v = {}; _r.s = 0; for (int64_t _i = 1; _i < {}; _i++) if ({} {} _r.v) _r.v = {}; }} _r; }})",
                        ptr,
                        rt,
                        len,
                        arr.at("0"),
                        len,
                        arr.at("_i"),
                        cmp,
                        arr.at("_i")
                    ),
                    el,
                )
            }
            "sort" | "stableSort" | "sortDescending" | "sortBy" | "stableSortBy"
            | "sortByDescending" | "radixSort" | "radixSortBy" | "partialSort"
            | "partialSortBy" => {
                let partial = m.starts_with("partial");
                let fa = if partial { args.get(1) } else { args.first() };
                let desc = m.contains("Descending");
                let order = match fa {
                    None => Order::Natural,
                    Some(f) if m.contains("By") => {
                        let (_, kt) = self.fn_sig(&f.expr);
                        Order::Key(&f.expr, kt)
                    }
                    Some(f) => Order::Less(&f.expr),
                };
                let (setup, item, value, finish) = self.reorder_view(&arr, &len);
                let body = if partial {
                    let top = arg(self, 0);
                    self.partial_sort_code(&item, &value, &top, &order, desc, &el, span)
                } else {
                    self.merge_sort_code(&item, &value, "_n", &order, desc, &el, span)
                };
                ok(
                    format!("({{ {} {} {} {} }})", ptr, setup, body, finish),
                    Ty::Void,
                )
            }
            // Puts at k what a sort would, nothing larger before and nothing smaller
            // after, and gives a reference to it (STDLIB §7.1).
            "selectNth" | "selectNthBy" => {
                let order = match args.get(1) {
                    None => Order::Natural,
                    Some(f) if m == "selectNthBy" => {
                        let (_, kt) = self.fn_sig(&f.expr);
                        Order::Key(&f.expr, kt)
                    }
                    Some(f) => Order::Less(&f.expr),
                };
                let k = arg(self, 0);
                let (setup, item, value, finish) = self.reorder_view(&arr, &len);
                let body = self.select_code(&item, &value, &k, &order, &el, span, &loc);
                let at = if value.is_empty() {
                    "_d[_kk]".to_string()
                } else {
                    format!("{}[_d[_kk]]", value)
                };
                ok(
                    format!(
                        "({{ {} {} {} {} *_res = &{}; {} _res; }})",
                        ptr, setup, body, ec, at, finish
                    ),
                    Ty::Ref(Box::new(el.clone())),
                )
            }
            "isSorted" | "isSortedBy" => {
                let order = match args.first() {
                    None => Order::Natural,
                    Some(f) if m == "isSortedBy" => {
                        let (_, kt) = self.fn_sig(&f.expr);
                        Order::Key(&f.expr, kt)
                    }
                    Some(f) => Order::Less(&f.expr),
                };
                let less =
                    self.order_less(&order, false, &arr.at("_i"), &arr.at("(_i - 1)"), &el, span);
                ok(
                    format!(
                        "({{ {} bool _ok = true; for (int64_t _i = 1; _i < {}; _i++) if ({}) {{ _ok = false; break; }} _ok; }})",
                        ptr, len, less
                    ),
                    Ty::bool(),
                )
            }
            "findIndex" if args.len() == 1 => {
                let pred = self.apply(&args[0].expr, &[arr.at("_i")]);
                let rt = self.result_ty(&Ty::i64(), span);
                fail(
                    format!(
                        "({{ {} {} _r = {{0, {}}}; for (int64_t _i = 0; _i < {}; _i++) if ({}) {{ _r.v = _i; _r.s = 0; break; }} _r; }})",
                        ptr,
                        rt,
                        status_code("MISSING"),
                        len,
                        pred
                    ),
                    Ty::i64(),
                )
            }
            "any" | "all" | "count" if args.len() == 1 => {
                let pred = self.apply(&args[0].expr, &[arr.at("_i")]);
                let (init, step, done, t) = match m {
                    "any" => ("false", "{ _r = true; break; }", "", Ty::bool()),
                    "all" => ("true", "", "{ _r = false; break; }", Ty::bool()),
                    _ => ("0", "_r++;", "", Ty::i64()),
                };
                let ct = self.cty(&t, span);
                let test = if done.is_empty() {
                    format!("if ({}) {}", pred, step)
                } else {
                    format!("if (!{}) {}", pred, done)
                };
                ok(
                    format!(
                        "({{ {} {} _r = {}; for (int64_t _i = 0; _i < {}; _i++) {} _r; }})",
                        ptr, ct, init, len, test
                    ),
                    t,
                )
            }
            // Kept elements move down in place, in order.
            "retain" | "dedup"
                if matches!(lay, Layout::Vec | Layout::Deq | Layout::Capped)
                    && (args.len() == 1) == (m == "retain") =>
            {
                let (aw, ai) = (arr.at("_w"), arr.at("_i"));
                let (start, keep) = if m == "retain" {
                    ("0", self.apply(&args[0].expr, &[arr.at("_i")]))
                } else {
                    let eq = self.natural_eq(&arr.at("_i"), &arr.at("(_w - 1)"), &el, span);
                    ("1", format!("!{}", eq))
                };
                ok(
                    format!(
                        "({{ {ptr} int64_t _n = {len}, _w = _n < {start} ? _n : {start}; for (int64_t _i = _w; _i < _n; _i++) if ({keep}) {{ if (_w != _i) {aw} = {ai}; _w++; }} {a}->n = _w; }})",
                        ptr = ptr,
                        len = len,
                        start = start,
                        keep = keep,
                        aw = aw,
                        ai = ai,
                        a = a
                    ),
                    Ty::Void,
                )
            }
            "fold" if args.len() == 2 => {
                let acc_t = ret.clone();
                let ac = self.cty(&acc_t, span);
                let init = self.expr(&args[0].expr, &acc_t);
                let step = self.apply(&args[1].expr, &["_acc".to_string(), arr.at("_i")]);
                ok(
                    format!(
                        "({{ {} {} _acc = {}; for (int64_t _i = 0; _i < {}; _i++) {{ {} _nx = {}; _acc = _nx; }} _acc; }})",
                        ptr, ac, init, len, ac, step
                    ),
                    acc_t,
                )
            }
            "append" if is_u8 => {
                if lay.capped() || matches!(lay, Layout::Cur | Layout::Chk) {
                    return ok(
                        self.unsupported(span, "`append` to a capped or cursor array"),
                        Ty::Void,
                    );
                }
                let k = format!("zr_into((zr_vec*){})", a);
                let (pre, writes) = if self.is_text(&args[0].expr) {
                    self.text(&args[0].expr, &k)
                } else {
                    let s = arg(self, 0);
                    (String::new(), format!("zr_w_str({}, {}); ", k, s))
                };
                ok(format!("({{ {} {} {} }})", ptr, pre, writes), Ty::Void)
            }
            _ => {
                let what = format!("`{}` on arrays", m);
                ok(self.unsupported(span, &what), ret)
            }
        }
    }

    /// `a +> x` and its chains: the code, which gives the status, and whether it can fail.
    /// A chain keeps the first failure (DESIGN §7.4).
    fn insert_status(&mut self, e: &Expr) -> (String, bool) {
        let mut stmts = String::new();
        let mut fails = false;
        self.insert_chain(e, &mut stmts, &mut fails);
        (format!("({{ uint8_t _st = 0; {} _st; }})", stmts), fails)
    }

    /// Appends the statements of an insertion chain; leaves `_arr` pointing at the array.
    fn insert_chain(&mut self, e: &Expr, stmts: &mut String, fails: &mut bool) {
        let E::Binary { op, lhs, rhs } = &e.kind else {
            return;
        };
        let at = match op {
            BinOp::InsertEnd => Where::End,
            BinOp::InsertFront => Where::Front,
            _ => Where::Cursor,
        };
        if let E::Binary {
            op: BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor,
            ..
        } = &lhs.kind
        {
            self.insert_chain(lhs, stmts, fails);
        } else {
            let (p, _) = self.place(lhs);
            stmts.push_str(&format!("__typeof__({}) *_arr = &({}); ", p, p));
        }
        let arr_t = self.ty(lhs).unref().clone();
        let (Some(el), Some(lay)) = (arr_t.elem().cloned(), layout(&arr_t)) else {
            let u = self.unsupported(e.span, "this insertion");
            stmts.push_str(&format!("(void){}; ", u));
            return;
        };
        let ec = self.cty(&el, e.span);
        let cap = match &arr_t {
            Ty::Array(_, k) => k.size,
            _ => 0,
        };
        if lay.capped() {
            *fails = true;
        }
        let x = self.expr(rhs, &el);
        let arr = Arr {
            a: "_arr".into(),
            lay,
            ec: ec.clone(),
            cap,
        };
        stmts.push_str(&format!(
            "{{ {} _x = {}; {} *_e; {} (void)_e; }} ",
            ec,
            x,
            ec,
            arr.put(at)
        ));
    }

    fn remove_op(&mut self, op: PostfixOp, x: &Expr, span: Span) -> CallOut {
        let t = self.ty(x);
        let arr_t = t.unref().clone();
        let (Some(el), Some(lay)) = (arr_t.elem().cloned(), layout(&arr_t)) else {
            let u = self.unsupported(span, "this removal");
            return CallOut {
                code: u,
                ret: Ty::Unknown,
                fails: false,
            };
        };
        let at = match op {
            PostfixOp::RemoveEnd => Where::End,
            PostfixOp::RemoveFront => Where::Front,
            PostfixOp::RemoveBefore => Where::Before,
            _ => Where::After,
        };
        let ec = self.cty(&el, span);
        let rt = self.result_ty(&el, span);
        let (p, _) = self.place(x);
        let arr = Arr {
            a: "_a".into(),
            lay,
            ec: ec.clone(),
            cap: cap_of(&arr_t),
        };
        let code = format!(
            "({{ __typeof__({}) *_a = &({}); {} _v = {{0}}; uint8_t _s = 192; {} ({}){{_v, _s}}; }})",
            p,
            p,
            ec,
            arr.take(at),
            rt
        );
        CallOut {
            code,
            ret: el,
            fails: true,
        }
    }
}

/// How an array's storage is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    /// `zr_vec`: on a data stack, changing only at its end.
    Vec,
    /// `zr_deq`: on a data stack, changing at its start too.
    Deq,
    /// `zr_cur`: a cursor array: data and index on two data stacks.
    Cur,
    /// `zr_chk`: a cursor array with a chunked index (`$`).
    Chk,
    /// Inline in its parent: `{n, e[k]}`.
    Capped,
    /// Capped, with a start slot: an inline ring buffer `{h, n, e[k]}`.
    Ring,
    /// Capped, with a cursor: an inline gap buffer `{gs, an, e[k]}`, the elements before
    /// the cursor at the start of `e` and those after it at the end.
    Gap,
    /// A slice or a view: `zr_slice`.
    Slice,
}

fn layout(t: &Ty) -> Option<Layout> {
    match t.unref() {
        Ty::Array(_, k) if k.cap == CapKind::Capped && k.has_cursor() => Some(Layout::Gap),
        Ty::Array(_, k) if k.cap == CapKind::Capped && (k.push_front() || k.pop_front()) => {
            Some(Layout::Ring)
        }
        Ty::Array(_, k) if k.cap == CapKind::Capped => Some(Layout::Capped),
        Ty::Array(_, k) if k.cap == CapKind::Chunked => Some(Layout::Chk),
        Ty::Array(_, k) if k.has_cursor() => Some(Layout::Cur),
        Ty::Array(_, k) if k.push_front() || k.pop_front() => Some(Layout::Deq),
        Ty::Array(..) => Some(Layout::Vec),
        Ty::Slice(_) | Ty::View(_) => Some(Layout::Slice),
        _ => None,
    }
}

impl Layout {
    /// Stored inline, with a capacity: full arrays fail with FULL (DESIGN §4.7).
    fn capped(self) -> bool {
        matches!(self, Layout::Capped | Layout::Ring | Layout::Gap)
    }
}

/// The capacity of a capped array type; 0 for others.
fn cap_of(t: &Ty) -> u64 {
    match t.unref() {
        Ty::Array(_, k) if k.cap == CapKind::Capped => k.size,
        _ => 0,
    }
}

/// The narrowest counter that holds a capped array's length (DESIGN §11).
fn counter(k: u64) -> &'static str {
    if k <= u8::MAX as u64 {
        "uint8_t"
    } else if k <= u16::MAX as u64 {
        "uint16_t"
    } else if k <= u32::MAX as u64 {
        "uint32_t"
    } else {
        "int64_t"
    }
}

#[derive(Clone, Copy)]
enum Where {
    End,
    Front,
    Cursor,
    Before,
    After,
}

/// An array reached through the pointer `a`, for generating its operations.
struct Arr {
    a: String,
    lay: Layout,
    ec: String,
    cap: u64,
}

impl Arr {
    fn len(&self) -> String {
        match self.lay {
            Layout::Cur => format!("zr_cur_len({})", self.a),
            Layout::Gap => format!("((int64_t){a}->gs + {a}->an)", a = self.a),
            _ => format!("{}->n", self.a),
        }
    }

    /// The cursor's position.
    fn cursor(&self) -> String {
        match self.lay {
            Layout::Chk => format!("{}->pos", self.a),
            Layout::Gap => format!("((int64_t){}->cur)", self.a),
            _ => format!("{}->bn", self.a),
        }
    }

    /// A relative move of the cursor, clamped at the edges.
    fn mv(&self, n: &str) -> String {
        match self.lay {
            Layout::Gap => {
                let to = format!(
                    "({{ int64_t _to = {} + (int64_t)({}), _l = {}; _to < 0 ? 0 : _to > _l ? _l : _to; }})",
                    self.cursor(),
                    n,
                    self.len()
                );
                format!("(void){}", self.seek(&to))
            }
            _ => format!("zr_{}_move({}, {})", self.kind(), self.a, n),
        }
    }

    /// The runtime's name for this kind of cursor array.
    fn kind(&self) -> &'static str {
        if self.lay == Layout::Chk {
            "chk"
        } else {
            "cur"
        }
    }

    fn seek(&self, i: &str) -> String {
        if self.lay == Layout::Gap {
            // Free: the gap follows only when the next edit at the cursor needs it.
            return format!(
                "({{ int64_t _ix = (int64_t)({i}); uint8_t _sk = 196; if (_ix >= 0 && _ix <= {len}) {{ {a}->cur = _ix; _sk = 0; }} _sk; }})",
                a = self.a,
                i = i,
                len = self.len()
            );
        }
        format!("zr_{}_seek({}, {})", self.kind(), self.a, i)
    }

    /// A statement that moves a gap buffer's gap to position `i`.
    fn gap_to(&self, i: &str) -> String {
        format!(
            "{{ int64_t _gs = {a}->gs, _an = {a}->an; zr_gap_move((char *){a}->e, sizeof({ec}), {k}, &_gs, &_an, {i}); {a}->gs = _gs; {a}->an = _an; }}",
            a = self.a,
            ec = self.ec,
            k = self.cap,
            i = i
        )
    }

    /// Before an edit at a gap buffer's cursor: the gap moves there.
    fn gap_to_cursor(&self) -> String {
        format!(
            "if ({a}->gs != {a}->cur) {}",
            self.gap_to(&format!("{}->cur", self.a)),
            a = self.a
        )
    }

    /// A pointer to position `lo` of a capped array, for a slice: a ring or gap buffer
    /// whose elements are split is made one block first (DESIGN §4.10).
    fn span(&self, lo: &str) -> String {
        let (a, ec, k) = (&self.a, &self.ec, self.cap);
        match self.lay {
            Layout::Ring => format!(
                "({{ int64_t _h = {a}->h; char *_p = zr_ring_span((char *){a}->e, sizeof({ec}), {k}, &_h, {a}->n, {lo}); {a}->h = _h; _p; }})",
                a = a,
                ec = ec,
                k = k,
                lo = lo
            ),
            Layout::Gap => format!(
                "({{ int64_t _gs = {a}->gs, _an = {a}->an; char *_p = zr_gap_span((char *){a}->e, sizeof({ec}), {k}, &_gs, &_an, {lo}); {a}->gs = _gs; {a}->an = _an; _p; }})",
                a = a,
                ec = ec,
                k = k,
                lo = lo
            ),
            _ => format!("(&{}->e[{}])", a, lo),
        }
    }

    /// For reordering a cursor array: statements that lay its order out as `_n` entries
    /// at `_d`, and that put it back afterwards. `None` for other arrays.
    fn gather(&self) -> Option<(String, String)> {
        let a = &self.a;
        match self.lay {
            Layout::Cur => Some((
                format!(
                    "int64_t _at = zr_cur_gather({a}); int64_t _n = {a}->bn; uint32_t *_d = {a}->bp;",
                    a = a
                ),
                format!("zr_cur_seek({}, _at);", a),
            )),
            Layout::Chk => Some((
                format!(
                    "int64_t _n = {a}->n; uint32_t *_d = zr_chk_flatten({a});",
                    a = a
                ),
                format!("zr_chk_unflatten({}, _d);", a),
            )),
            _ => None,
        }
    }

    /// The element at logical position `i`, as an lvalue.
    fn at(&self, i: &str) -> String {
        match self.lay {
            Layout::Capped => format!("{}->e[{}]", self.a, i),
            Layout::Ring => format!(
                "{a}->e[({a}->h + (int64_t)({i})) % {k}]",
                a = self.a,
                i = i,
                k = self.cap
            ),
            Layout::Gap => format!(
                "{a}->e[({i}) < {a}->gs ? ({i}) : {k} - {a}->an + (({i}) - {a}->gs)]",
                a = self.a,
                i = i,
                k = self.cap
            ),
            Layout::Chk => format!(
                "(({}*){}->d.data)[zr_chk_at({}, {})]",
                self.ec, self.a, self.a, i
            ),
            Layout::Cur => format!(
                "(({}*){}->d.data)[zr_cur_at({}, {})]",
                self.ec, self.a, self.a, i
            ),
            _ => format!("(({}*){}->p)[{}]", self.ec, self.a, i),
        }
    }

    /// Statements that store `_x` as a new element and point `_e` at it. A full capped
    /// array stores nothing, sets `_st` to FULL, and points `_e` at its last element (DESIGN §4.7).
    fn put(&self, at: Where) -> String {
        let (a, ec) = (&self.a, &self.ec);
        match (self.lay, at) {
            (Layout::Capped, _) => format!(
                "if ({a}->n < {cap}) {{ _e = &{a}->e[{a}->n++]; *_e = _x; }} else {{ _e = &{a}->e[{a}->n - 1]; if (!_st) _st = 193; }}",
                a = a,
                cap = self.cap
            ),
            (Layout::Ring | Layout::Gap, w) => {
                let k = self.cap;
                let body = match (self.lay, w) {
                    (Layout::Ring, Where::Front) => format!(
                        "{a}->h = ({a}->h + {k} - 1) % {k}; {a}->n++; _e = &{a}->e[{a}->h];",
                        a = a,
                        k = k
                    ),
                    (Layout::Ring, _) => format!(
                        "_e = &{a}->e[({a}->h + {a}->n) % {k}]; {a}->n++;",
                        a = a,
                        k = k
                    ),
                    (_, Where::Front) => format!(
                        "memmove(&{a}->e[1], &{a}->e[0], (size_t){a}->gs * sizeof({ec})); {a}->gs++; {a}->cur++; _e = &{a}->e[0];",
                        a = a,
                        ec = ec
                    ),
                    (_, Where::End) => format!(
                        "memmove(&{a}->e[{k} - {a}->an - 1], &{a}->e[{k} - {a}->an], (size_t){a}->an * sizeof({ec})); {a}->an++; _e = &{a}->e[{k} - 1];",
                        a = a,
                        k = k,
                        ec = ec
                    ),
                    _ => format!(
                        "{} _e = &{a}->e[{a}->gs++]; {a}->cur++;",
                        self.gap_to_cursor(),
                        a = a
                    ),
                };
                let last = self.at(&format!("({} - 1)", self.len()));
                format!(
                    "if ({len} < {k}) {{ {body} *_e = _x; }} else {{ _e = &{last}; if (!_st) _st = 193; }}",
                    len = self.len(),
                    k = k,
                    body = body,
                    last = last
                )
            }
            (Layout::Vec | Layout::Deq, Where::End) => {
                format!("_e = &(({ec}*){a}->p)[{a}->n++]; *_e = _x;", a = a, ec = ec)
            }
            (Layout::Deq, Where::Front) => format!(
                "{a}->p = ({ec}*){a}->p - 1; {a}->n++; _e = ({ec}*){a}->p; *_e = _x;",
                a = a,
                ec = ec
            ),
            (Layout::Chk, w) => {
                let wh = match w {
                    Where::End => 0,
                    Where::Front => 1,
                    _ => 2,
                };
                format!(
                    "{{ uint32_t _sl = zr_chk_put({a}, {wh}, sizeof({ec})); _e = &(({ec}*){a}->d.data)[_sl]; *_e = _x; }}",
                    a = a,
                    wh = wh,
                    ec = ec
                )
            }
            (Layout::Cur, w) => {
                let f = match w {
                    Where::Front => "zr_cur_push_front",
                    Where::Cursor => "zr_cur_insert",
                    _ => "zr_cur_push",
                };
                format!(
                    "{{ uint32_t _sl = {f}({a}, sizeof({ec})); _e = &(({ec}*){a}->d.data)[_sl]; *_e = _x; }}",
                    f = f,
                    a = a,
                    ec = ec
                )
            }
            _ => "_e = NULL;".into(),
        }
    }

    /// Statements that take out an element into `_v`, setting `_s` to 0 when there was one.
    fn take(&self, at: Where) -> String {
        let (a, ec) = (&self.a, &self.ec);
        match (self.lay, at) {
            (Layout::Capped, _) => format!(
                "if ({a}->n > 0) {{ _v = {a}->e[--{a}->n]; _s = 0; }}",
                a = a
            ),
            (Layout::Ring, Where::Front) => format!(
                "if ({a}->n > 0) {{ _v = {a}->e[{a}->h]; {a}->h = ({a}->h + 1) % {k}; {a}->n--; _s = 0; }}",
                a = a,
                k = self.cap
            ),
            (Layout::Ring, _) => format!(
                "if ({a}->n > 0) {{ _v = {a}->e[({a}->h + {a}->n - 1) % {k}]; {a}->n--; _s = 0; }}",
                a = a,
                k = self.cap
            ),
            (Layout::Gap, w) => {
                let k = self.cap;
                let len = self.len();
                let before = format!("_v = {a}->e[--{a}->gs]; _s = 0;", a = a);
                let after = format!(
                    "_v = {a}->e[{k} - {a}->an]; {a}->an--; _s = 0;",
                    a = a,
                    k = k
                );
                let front = format!(
                    "_v = {a}->e[0]; memmove(&{a}->e[0], &{a}->e[1], (size_t)({a}->gs - 1) * sizeof({ec})); {a}->gs--; _s = 0;",
                    a = a,
                    ec = ec
                );
                let back = format!(
                    "_v = {a}->e[{k} - 1]; memmove(&{a}->e[{k} - {a}->an + 1], &{a}->e[{k} - {a}->an], (size_t)({a}->an - 1) * sizeof({ec})); {a}->an--; _s = 0;",
                    a = a,
                    k = k,
                    ec = ec
                );
                let g = self.gap_to_cursor();
                match w {
                    Where::Before => format!(
                        "if ({a}->cur) {{ {g} {b} {a}->cur--; }}",
                        a = a,
                        g = g,
                        b = before
                    ),
                    Where::After => format!(
                        "if ({a}->cur < {len}) {{ {g} {b} }}",
                        a = a,
                        len = len,
                        g = g,
                        b = after
                    ),
                    // The cursor stays between the same elements.
                    Where::Front => format!(
                        "if ({len}) {{ if ({a}->cur) {a}->cur--; if ({a}->gs) {{ {f} }} else {{ {b} }} }}",
                        a = a,
                        len = len,
                        f = front,
                        b = after
                    ),
                    _ => format!(
                        "if ({len}) {{ if ({a}->cur == {len}) {a}->cur--; if ({a}->an) {{ {k2} }} else {{ {b} }} }}",
                        a = a,
                        len = len,
                        k2 = back,
                        b = before
                    ),
                }
            }
            (Layout::Vec, _) => format!(
                "if ({a}->n > 0) {{ _v = (({ec}*){a}->p)[--{a}->n]; _s = 0; }}",
                a = a,
                ec = ec
            ),
            (Layout::Deq, Where::Front) => format!(
                "if ({a}->n > 0) {{ _v = (({ec}*){a}->p)[0]; {a}->p = ({ec}*){a}->p + 1; {a}->n--; _s = 0; zr_deq_settle({a}); }}",
                a = a,
                ec = ec
            ),
            (Layout::Deq, _) => format!(
                "if ({a}->n > 0) {{ _v = (({ec}*){a}->p)[--{a}->n]; _s = 0; zr_deq_settle({a}); }}",
                a = a,
                ec = ec
            ),
            (Layout::Cur, w) => {
                let f = match w {
                    Where::Front => "zr_cur_take_front",
                    Where::Before => "zr_cur_take_before",
                    Where::After => "zr_cur_take_after",
                    _ => "zr_cur_take_back",
                };
                format!(
                    "{{ uint32_t _sl = {f}({a}); if (_sl != UINT32_MAX) {{ _v = (({ec}*){a}->d.data)[_sl]; zr_data_release(&{a}->d, _sl, sizeof({ec})); _s = 0; }} }}",
                    f = f,
                    a = a,
                    ec = ec
                )
            }
            (Layout::Chk, w) => {
                let wh = match w {
                    Where::End => 0,
                    Where::Front => 1,
                    Where::Before => 2,
                    _ => 3,
                };
                format!(
                    "{{ uint32_t _sl = zr_chk_take({a}, {wh}); if (_sl != UINT32_MAX) {{ _v = (({ec}*){a}->d.data)[_sl]; zr_data_release(&{a}->d, _sl, sizeof({ec})); _s = 0; }} }}",
                    a = a,
                    wh = wh,
                    ec = ec
                )
            }
            (Layout::Slice, _) => String::new(),
        }
    }

    fn clear(&self) -> String {
        match self.lay {
            Layout::Cur => format!("zr_cur_clear({});", self.a),
            Layout::Chk => format!("zr_chk_clear({});", self.a),
            Layout::Ring => format!("{a}->n = 0; {a}->h = 0;", a = self.a),
            Layout::Gap => format!("{a}->gs = 0; {a}->an = 0; {a}->cur = 0;", a = self.a),
            Layout::Deq => format!("{a}->n = 0; {a}->p = {a}->mid;", a = self.a),
            _ => format!("{}->n = 0;", self.a),
        }
    }
}

struct CallOut {
    code: String,
    ret: Ty,
    fails: bool,
}

fn strip_paren(e: &Expr) -> &Expr {
    match &e.kind {
        E::Paren(x) => strip_paren(x),
        _ => e,
    }
}

fn float_text(t: &str) -> String {
    let t: String = t.chars().filter(|c| *c != '_').collect();
    if t.contains('.') || t.contains('e') || t.contains('E') {
        t
    } else {
        format!("{}.0", t)
    }
}

/// The type two compared numbers are both converted to.
fn common_type(a: &Ty, b: &Ty) -> Ty {
    match (a, b) {
        (Ty::Prim(x), Ty::Prim(y)) => {
            if x == y || lossless(*y, *x) {
                a.clone()
            } else if lossless(*x, *y) {
                b.clone()
            } else if x.is_float() || y.is_float() {
                Ty::Prim(Prim::F64)
            } else {
                Ty::i64()
            }
        }
        (Ty::Prim(_), _) => a.clone(),
        (_, Ty::Prim(_)) => b.clone(),
        _ => a.clone(),
    }
}

/// The number of a named status code (DESIGN §7.7).
pub fn status_code(name: &str) -> u8 {
    match name {
        "OK" => 0,
        "FAILED" => 1,
        "NOT_FOUND" | "ENOENT" => 2,
        "INTERRUPTED" | "EINTR" => 4,
        "IO" | "EIO" => 5,
        "BAD_HANDLE" | "EBADF" => 9,
        "AGAIN" | "EAGAIN" => 11,
        "NO_MEMORY" | "ENOMEM" => 12,
        "PERMISSION" | "EACCES" => 13,
        "BUSY" | "EBUSY" => 16,
        "EXISTS" | "EEXIST" => 17,
        "CROSS_DEVICE" | "EXDEV" => 18,
        "NOT_DIRECTORY" | "ENOTDIR" => 20,
        "IS_DIRECTORY" | "EISDIR" => 21,
        "INVALID" | "EINVAL" => 22,
        "NOT_TERMINAL" | "ENOTTY" => 25,
        "NO_SPACE" | "ENOSPC" => 28,
        "BROKEN_PIPE" | "EPIPE" => 32,
        "RANGE" | "ERANGE" => 34,
        "NOT_EMPTY" | "ENOTEMPTY" => 39,
        "UNSUPPORTED" | "EOPNOTSUPP" => 95,
        "TIMEOUT" | "ETIMEDOUT" => 110,
        "EMPTY" => 192,
        "FULL" => 193,
        "END" => 194,
        "MISSING" => 195,
        "POSITION" => 196,
        "TYPE" => 197,
        "GPU" => 198,
        "OUT_OF_DATE" => 199,
        _ => 1,
    }
}
