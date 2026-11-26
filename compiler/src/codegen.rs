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
    self, AssignOp, BinOp, Capture, Expr, ExprKind as E, LocalType, Name, PostfixOp, Stmt,
    StmtKind as S, StrPart, SwitchBody, Target, UnaryOp,
};
use crate::builtins::Origin;
use crate::check::{Facts, NameRes, key};
use crate::diag::Diagnostic;
use crate::program::{ClassKind, ConstVal, MethodInfo, Program, active, starts_empty};
use crate::source::{SourceFile, Span};
use crate::token::Prim;
use crate::types::*;

mod generics;
mod interfaces;
mod lambdas;
mod layouts;
mod maps;
mod pools;
mod records;
mod sorting;
mod threads;
mod versions;

use generics::{Inst, InstKind};
use lambdas::{Bound, BoundCopy, BoundParams, PendingLambda};
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

/// The status variable and label of an expression `@` reads, and whether anything in it
/// can fail and jump there.
struct Catch {
    cs: String,
    cl: String,
    used: bool,
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
    /// Each open block's defers, with the catch record each has while an `@@` is active.
    defers: Vec<Vec<(Stmt, String)>>,
    loops: Vec<LoopCtx>,
    switches: Vec<SwitchCtx>,
    pending_label: Option<String>,
    used_scratch: bool,
    /// Inside an expression whose status `@` reads: where a failure goes (DESIGN §7.6).
    catching: Option<Catch>,
    /// The function being generated, as its trail entries name it (DESIGN §7.6).
    fn_label: String,
    /// `zr_site` records, one per function and line, and their C names.
    sites: String,
    site_names: HashMap<(String, usize, usize), String>,
    /// The function reads a status, so it keeps `_zg`, the trail of the last failure it
    /// read, for passing that failure on.
    uses_zg: bool,
    /// The function's status variables, by name, and how many enclosing `if`s test one:
    /// a failure returned there passes on the one it read (DESIGN §7.6).
    status_vars: HashSet<String>,
    /// Variables a reference could reach, which a loop can't count on staying fixed
    /// (`versions.rs`).
    aliased: HashSet<versions::Var>,
    /// Locals declared as arrays of their own, which only their own code can resize.
    owned_arrays: HashSet<(u32, u32)>,
    /// Index expressions generated without their check, by span: inside the copy of a
    /// versioned loop whose test showed them in range (DESIGN §14.2).
    unchecked_ix: HashSet<(u32, u32)>,
    /// The C name of the function being generated when its base case is copied to its
    /// calls to itself (DESIGN §14.2), and whether a call was sent to that copy.
    base_name: Option<String>,
    base_used: bool,
    /// While that copy is generated: the `return switch` it ends with, by span, and the
    /// call to the whole function that its arms that call something become.
    base_switch: Option<((u32, u32), String)>,
    /// Arrays of links, by span, that the operation being generated reaches as the
    /// offsets they are: every other use of one is reported (REFERENCES §7.5).
    link_ok: HashSet<(u32, u32)>,
    status_tests: usize,
    /// For the call about to be generated, when its array result goes straight into a
    /// variable: where to build it (DESIGN §3.3), as C, or `NULL` for the callee's own
    /// data stack. `None` makes the result a temporary, kept on the scratch stack.
    direct_result: Option<String>,
    /// The declaration about to be generated is of a new variable, so a call's array
    /// result can be built in its storage: nothing else can be reading it.
    fresh_dst: bool,
    /// The current function returns an array of the plain layout, built where its
    /// caller's `_dst` says (DESIGN §3.3).
    dst_fn: bool,
    /// `direct_result` for the built-in call about to be generated, such as `filter`.
    builtin_into: Option<String>,
    /// The local array the current function returns by name, built in `_dst`.
    result_local: Option<(u32, u32)>,
    /// The type of the value the last `with_status` evaluated.
    last_status_ret: Ty,
    /// Function-pointer typedefs, by signature, and their text.
    fn_types: HashMap<String, String>,
    fn_typedefs: String,
    /// Lambdas used as values, still to generate, and the C names given to them.
    pending_lambdas: Vec<PendingLambda>,
    /// The program imports zink.thread, so it's compiled with `ZR_THREADS` (STDLIB §10.3).
    threads: bool,
    /// The statics a new thread gets a copy of: each one's C name, and for an array on a
    /// data stack, the code that gives it a stack and its element's C type.
    thread_statics: Vec<(String, Option<(String, String)>)>,
    /// The statics on data stacks a new thread doesn't get a copy of, which start empty
    /// there: each one's C name and the code that gives it a stack. The lifetimes pass
    /// keeps workers from reaching them; this keeps one that did safe.
    fresh_statics: Vec<(String, String)>,
    pending_spawns: Vec<threads::PendingSpawn>,
    spawn_n: usize,
    lambda_names: HashMap<String, String>,
    /// In a copy of a function made for lambdas, its parameters bound to them, by
    /// declaration (DESIGN §9.3).
    lam_binds: HashMap<(u32, u32), std::rc::Rc<Bound>>,
    /// The copies made so far, by function and lambdas, and those still to generate.
    bound_copies: HashMap<String, String>,
    copy_queue: Vec<BoundCopy>,
    /// For the copy `method_version` is about to generate: its bound parameters and the
    /// C parameters it takes for them.
    copy_for: Option<(BoundParams, Vec<String>)>,
    /// Copies of functions for the array layouts passed to them (`layouts.rs`): by the
    /// function and the layouts, the copy's name; and those waiting to be generated.
    layout_copies: HashMap<String, String>,
    layout_queue: Vec<layouts::LayoutCopy>,
    /// For the copy being generated: the parameters it changes, and the call it was made
    /// for.
    layout_for: Option<(layouts::Over, (usize, Span))>,
    /// In such a copy: each name of a changed parameter, with its type there; the pushes
    /// into one, which can fail; and the call it was made for.
    layout_over: HashMap<(u32, u32), Ty>,
    extra_unchecked: HashMap<(u32, u32), String>,
    layout_origin: Option<(usize, Span)>,
    /// The next expression is an argument for a method's plain parameter, a read-only
    /// copy for the call: one with a growable field may share its storage.
    borrow_arg: bool,
    /// Inlined lambda block bodies being generated: result, end label, result type.
    inline_ret: Vec<(String, String, Ty)>,
    /// Where a status that the lambda being inlined returns goes (DESIGN §9.3).
    lambda_fail: Vec<lambdas::LambdaFail>,
    /// The built-in calls being generated, innermost last, which their lambdas' failures
    /// are failures of.
    call_names: Vec<(String, Span)>,
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
    /// The standard library's methods and constructors are generated only once something
    /// calls them: `(class, index, is a constructor)`.
    lib_queue: Vec<(ClassId, usize, bool)>,
    lib_done: HashSet<(ClassId, usize, bool)>,
    /// The size functions of variable-size types (records.rs), by type.
    vs_fns: HashMap<String, String>,
    /// Structs laid out packed: the fixed part of a variable-size record, and `packed`
    /// classes (DESIGN §6.1, §11).
    packed: HashSet<String>,
    /// In the constructor of a variable-size record: its type, and the first of its
    /// variable-size fields not written yet (records.rs).
    rec_ctor: Option<(Ty, usize)>,
    /// Where a constructor called while building a record puts its status.
    build_status: Option<String>,
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

/// The bytes a value of an enum with `n` constants takes: the narrowest unsigned integer
/// that holds every constant's index (DESIGN §11), so one of up to 256 constants is a byte.
fn enum_bytes(n: usize) -> u32 {
    if n <= 1 << 8 {
        1
    } else if n <= 1 << 16 {
        2
    } else {
        4
    }
}

/// The C type of an enum with `n` constants (`enum_bytes`).
fn enum_c(n: usize) -> &'static str {
    match enum_bytes(n) {
        1 => "uint8_t",
        2 => "uint16_t",
        _ => "uint32_t",
    }
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
        || matches!(t, Ty::Map(..) | Ty::Set(_) | Ty::Pool(_))
}

fn is_ref(t: &Ty) -> bool {
    matches!(t, Ty::Ref(_)) || matches!(t, Ty::Nullable(x) if matches!(**x, Ty::Ref(_)))
}

/// The type a `switch` compares its value as: an array of text as a view of it, which
/// the cases' strings are.
fn switch_ty(t: Ty) -> Ty {
    match t {
        Ty::Array(e, _) => Ty::Slice(e),
        t => t,
    }
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
            catching: None,
            fn_label: String::new(),
            sites: String::new(),
            site_names: HashMap::new(),
            uses_zg: false,
            status_vars: HashSet::new(),
            aliased: HashSet::new(),
            owned_arrays: HashSet::new(),
            unchecked_ix: HashSet::new(),
            base_name: None,
            base_used: false,
            base_switch: None,
            link_ok: HashSet::new(),
            status_tests: 0,
            direct_result: None,
            fresh_dst: false,
            dst_fn: false,
            builtin_into: None,
            result_local: None,
            last_status_ret: Ty::Void,
            fn_types: HashMap::new(),
            fn_typedefs: String::new(),
            pending_lambdas: Vec::new(),
            threads: prog.classes.iter().any(|c| c.package == "zink.thread"),
            thread_statics: Vec::new(),
            fresh_statics: Vec::new(),
            pending_spawns: Vec::new(),
            spawn_n: 0,
            lambda_names: HashMap::new(),
            lam_binds: HashMap::new(),
            bound_copies: HashMap::new(),
            copy_queue: Vec::new(),
            copy_for: None,
            layout_copies: HashMap::new(),
            layout_queue: Vec::new(),
            layout_for: None,
            layout_over: HashMap::new(),
            extra_unchecked: HashMap::new(),
            layout_origin: None,
            borrow_arg: false,
            inline_ret: Vec::new(),
            lambda_fail: Vec::new(),
            call_names: Vec::new(),
            maps: HashMap::new(),
            helpers: String::new(),
            subst: Vec::new(),
            cur_cargs: Vec::new(),
            class_insts: HashMap::new(),
            fn_insts: HashMap::new(),
            inst_queue: Vec::new(),
            iface_refs: HashMap::new(),
            vtables: HashMap::new(),
            lib_queue: Vec::new(),
            lib_done: HashSet::new(),
            vs_fns: HashMap::new(),
            packed: HashSet::new(),
            rec_ctor: None,
            build_status: None,
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

    /// A call's array result. One built where it goes needs nothing more. A temporary was
    /// built on the callee's own data stack, which its next call reuses, so it is copied to
    /// the scratch stack, where it lasts to the end of its statement:
    /// `path.relative(path.absolute(a), path.absolute(b))`.
    pub(super) fn array_result(
        &mut self,
        code: String,
        r: &Ty,
        fails: bool,
        direct: &Option<String>,
        span: Span,
    ) -> String {
        if direct.is_some() {
            return code;
        }
        let el = r.elem().cloned().unwrap_or(Ty::u8());
        self.used_scratch = true;
        // An array of variable-size records is copied as its bytes.
        let vs = self.vs_elems(r);
        let sz = self.elem_size(&el, span);
        let copy = |v: &str| -> String {
            if vs {
                format!("zr_rvec_scratch({})", v)
            } else {
                format!("zr_vec_scratch({}, {})", v, sz)
            }
        };
        if fails {
            format!(
                "({{ __typeof__({c}) _rv = {c}; _rv.v = {copy}; _rv; }})",
                c = code,
                copy = copy("_rv.v")
            )
        } else {
            copy(&code)
        }
    }

    /// The local array a function returns by name, to build in its caller's `_dst`: the
    /// first one a `return` names, if it isn't a parameter. Only one, since two arrays
    /// can't share the space.
    fn result_local_of(&self, stmts: &[Stmt], params: &[ast::Param]) -> Option<(u32, u32)> {
        fn returns<'e>(s: &'e Stmt, out: &mut Vec<&'e Expr>) {
            match &s.kind {
                S::Return { value: Some(v), .. } => out.push(v),
                S::Block(b) => b.stmts.iter().for_each(|s| returns(s, out)),
                S::If { then, els, .. } => {
                    returns(then, out);
                    if let Some(e) = els {
                        returns(e, out);
                    }
                }
                S::While { body, .. }
                | S::Repeat { body, .. }
                | S::DoWhile { body, .. }
                | S::For { body, .. }
                | S::ForEach { body, .. } => returns(body, out),
                S::Switch(sw) => {
                    for r in &sw.rules {
                        if let ast::SwitchBody::Block(b) = &r.body {
                            b.stmts.iter().for_each(|s| returns(s, out));
                        }
                    }
                }
                S::Labeled { stmt, .. } => returns(stmt, out),
                S::If_(b) => active(b).iter().for_each(|s| returns(s, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        stmts.iter().for_each(|s| returns(s, &mut found));
        let params: Vec<(u32, u32)> = params
            .iter()
            .filter_map(|p| match &p.name {
                Name::Ident(id) => Some(key(id.span)),
                Name::Discard(_) => None,
            })
            .collect();
        found.into_iter().find_map(|v| {
            let v = strip_paren(v);
            let E::Ident(_) = &v.kind else {
                return None;
            };
            match self.facts.names.get(&key(v.span)) {
                Some(NameRes::Local(d)) if !params.contains(d) => {
                    let t = self.facts.locals.get(d).map(|t| self.sb(t))?;
                    (layout(&t) == Some(Layout::Vec) || self.vs_class(&t)).then_some(*d)
                }
                _ => None,
            }
        })
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

    /// A function of the program's returns a nonzero status: an entry in the trail, only
    /// on the failing path, so a success costs nothing (DESIGN §7.6). `passes_on`: the
    /// status is one the function read, or the return is in a branch that tests one, so it
    /// continues that failure's trail.
    fn set_at(&mut self, nonzero: &str, st: &str, passes_on: bool, span: Span) {
        if st == "0" || self.fn_label.is_empty() {
            return;
        }
        let loc = self.loc(span);
        let g = if passes_on {
            self.uses_zg = true;
            "_zg"
        } else {
            "0"
        };
        let code = format!("zr_trail_ret({}, {}, {});", loc, st, g);
        if st.parse::<u8>().is_ok() {
            self.line(&code);
        } else {
            self.line(&format!("if ({}) {}", nonzero, code));
        }
    }

    /// A returned value as C, and for `return ## e;` (`pass`), its status into `st`, from
    /// statements that compute both first (DESIGN §7.2).
    fn ret_value(&mut self, v: &Expr, want: &Ty, pass: bool, st: &mut String) -> String {
        if !pass {
            return self.expr(v, want);
        }
        let (val, pre, one, _) = self.with_status_in(v, want, false);
        self.line(&pre);
        let t = self.tmp("pv");
        self.line(&format!("__typeof__({}) {} = {};", val, t, val));
        *st = one;
        t
    }

    /// Whether `e` reads one of the function's status variables.
    fn reads_status_var(&self, e: &Expr) -> bool {
        match &e.kind {
            E::Ident(n) => self.status_vars.contains(n),
            _ => crate::check::expr_children(e)
                .into_iter()
                .any(|x| self.reads_status_var(x)),
        }
    }

    /// What a failed check does (DESIGN §7.6): inside an expression `@` reads, it sets the
    /// status and jumps to where that's read; elsewhere `stop` runs, which ends the program.
    fn on_fail(&mut self, code: u8, stop: String) -> String {
        match &mut self.catching {
            Some(k) => {
                k.used = true;
                format!("{} = {}; goto {}", k.cs, code, k.cl)
            }
            None => stop,
        }
    }

    /// The `zr_site` of a line in the function being generated, as C: where a failure
    /// there is reported, and how its trail entry names it (DESIGN §7.6).
    fn loc(&mut self, span: Span) -> String {
        let src = &self.sources[self.file];
        let (line, _) = src.line_col(span.start as usize);
        // The library's lines never enter the trail, so theirs are built where they fail.
        if self.fn_label.is_empty() {
            return format!(
                "(&(zr_site){{NULL, {}, {}}})",
                c_string(src.name.as_bytes()),
                line
            );
        }
        let k = (self.fn_label.clone(), self.file, line);
        if let Some(n) = self.site_names.get(&k) {
            return format!("&{}", n);
        }
        let n = format!("_zs{}", self.site_names.len());
        let _ = writeln!(
            self.sites,
            "static const zr_site {} = {{{}, {}, {}}};",
            n,
            c_string(self.fn_label.as_bytes()),
            c_string(src.name.as_bytes()),
            line
        );
        self.site_names.insert(k, n.clone());
        format!("&{}", n)
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
        if let Some(t) = self.layout_over.get(&key(span)) {
            return t.clone();
        }
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
                    ClassKind::Enum => enum_c(c.constants.len()).into(),
                    ClassKind::Interface => self.unsupported(span, "interfaces"),
                    _ if !args.is_empty() => self.class_inst(*id, args, span),
                    _ => self.class_name(*id),
                }
            }
            Ty::Array(e, k) => {
                if self.vs(e) {
                    if k.cap == CapKind::Capped || k.has_cursor() || k.push_front() || k.pop_front()
                    {
                        return self
                            .unsupported(span, "this kind of array of variable-size values");
                    }
                    return "zr_rvec".into();
                }
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
                    Some(Layout::Fixed) => ("zfix", String::new()),
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
            // Variable-size records in tuples, maps, sets and pools aren't supported yet:
            // each is one run of bytes of its own length, which these keep at a fixed size.
            Ty::Tuple(ts) if ts.iter().any(|t| self.vs_class(t)) => {
                self.unsupported(span, "a variable-size record in a tuple")
            }
            Ty::Map(k, v) if self.vs_class(k) || self.vs_class(v) => {
                self.unsupported(span, "variable-size records in a hash map")
            }
            Ty::Set(e) | Ty::Pool(e) if self.vs_class(e) => {
                self.unsupported(span, "variable-size records in a set or a pool")
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
            Ty::Pool(_) => "zr_pool".into(),
            Ty::Queue(_) => "zr_queue *".into(),
            Ty::Handle(_) => "zr_handle".into(),
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
        if !self.one_main() {
            return;
        }
        (self.aliased, self.owned_arrays) = self.alias_scan();
        // Struct definitions for every class.
        for (id, c) in prog.classes.iter().enumerate() {
            if !matches!(c.kind, ClassKind::Class | ClassKind::Root) || !c.type_params.is_empty() {
                continue;
            }
            self.file = c.file;
            let (body, deps) = self.struct_body(id, &[]);
            let name = self.class_name(id);
            // A variable-size record's fixed part, and a `packed` class (DESIGN §6.1).
            if self.vs(&Ty::Class(id, Vec::new())) || c.packed {
                self.packed.insert(name.clone());
            }
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
                // A constant has its value from the start, the same in every thread, so the
                // C compiler can fold it (DESIGN §6.1).
                if f.is_final
                    && let Ok(Some(v @ (ConstVal::Int(_) | ConstVal::Bool(_)))) =
                        prog.const_value(id, &f.name)
                {
                    let lit = match v {
                        ConstVal::Int(x) => self.int_lit(x, &f.ty),
                        _ => (v == ConstVal::Bool(true)).to_string(),
                    };
                    let _ = writeln!(self.protos, "static {} {} = {};", ct, name, lit);
                    continue;
                }
                // Each thread has its own (STDLIB §10.3).
                let tls = if self.threads { "ZR_TLS " } else { "" };
                let _ = writeln!(self.protos, "static {}{} {};", tls, ct, name);
                let mut stack = None;
                if on_stack(&f.ty) {
                    let init = self.stack_init(&f.ty);
                    let _ = writeln!(self.statics_init, "    {} = {};", name, init);
                    stack = Some(init);
                }
                if self.threads && prog.thread_copyable(&f.ty) {
                    let arr = match (stack, f.ty.elem()) {
                        (Some(init), Some(el)) => {
                            let el = el.clone();
                            Some((init, self.cty(&el, f.span)))
                        }
                        _ => None,
                    };
                    self.thread_statics.push((name.clone(), arr));
                } else if self.threads
                    && let Some(init) = stack
                {
                    self.fresh_statics.push((name.clone(), init));
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
        // Functions: the program's all, the library's when something calls them.
        for (id, c) in prog.classes.iter().enumerate() {
            if !c.type_params.is_empty() {
                continue;
            }
            self.file = c.file;
            let lib = crate::stdlib::reserved(&c.package);
            for (i, m) in c.methods.iter().enumerate() {
                if let Some(cn) = &m.c_name {
                    self.extern_proto(id, i, m, cn);
                    continue;
                }
                if m.decl.body.is_none() || !m.type_params.is_empty() || lib {
                    continue;
                }
                self.method(id, i, m);
                self.drain_lambdas();
            }
            let record = self.vs(&Ty::Class(id, Vec::new()));
            for (i, k) in c.ctors.iter().enumerate() {
                if matches!(c.kind, ClassKind::Class | ClassKind::Root)
                    && !lib
                    && (!record || k.decl.is_some())
                {
                    self.ctor(id, i, k);
                    self.drain_lambdas();
                }
            }
        }
        loop {
            self.drain_lib();
            self.drain_insts();
            self.drain_copies();
            self.drain_layouts();
            if self.lib_queue.is_empty()
                && self.inst_queue.is_empty()
                && self.copy_queue.is_empty()
                && self.layout_queue.is_empty()
            {
                break;
            }
        }
    }

    /// Asks for a library method or constructor, which is generated after the function
    /// being generated now.
    fn want_lib(&mut self, c: ClassId, i: usize, ctor: bool) {
        let cls = &self.prog.classes[c];
        if !crate::stdlib::reserved(&cls.package) || !cls.type_params.is_empty() {
            return;
        }
        if !ctor && (cls.methods[i].c_name.is_some() || cls.methods[i].decl.body.is_none()) {
            return;
        }
        if self.lib_done.insert((c, i, ctor)) {
            self.lib_queue.push((c, i, ctor));
        }
    }

    fn drain_lib(&mut self) {
        while let Some((c, i, ctor)) = self.lib_queue.pop() {
            let prog = self.prog;
            self.file = prog.classes[c].file;
            if ctor {
                self.ctor(c, i, &prog.classes[c].ctors[i]);
            } else {
                self.method(c, i, &prog.classes[c].methods[i]);
            }
            self.drain_lambdas();
        }
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
        self.lam_binds.clear();
        self.layout_over.clear();
        self.extra_unchecked.clear();
        self.layout_origin = None;
        self.out.clear();
        self.indent = 1;
        self.defers.clear();
        self.loops.clear();
        self.switches.clear();
        self.used_scratch = false;
        self.dst_fn = false;
        self.result_local = None;
        self.uses_zg = false;
        self.status_vars.clear();
        self.status_tests = 0;
        let cls = &self.prog.classes[class];
        self.fn_label = if crate::stdlib::reserved(&cls.package) {
            String::new()
        } else {
            cls.name.clone()
        };
    }

    /// Names the function being generated for its trail entries: a method as
    /// `Class.method`, and a script's function by its name.
    fn label_fn(&mut self, name: &str) {
        if self.fn_label.is_empty() {
            return;
        }
        let cls = &self.prog.classes[self.class];
        self.fn_label = if cls.kind == ClassKind::Script {
            name.to_string()
        } else {
            format!("{}.{}", cls.name, name)
        };
    }

    /// The function's body: what was generated, after `_zg` if it reads a status.
    fn take_body(&mut self) -> String {
        let out = std::mem::take(&mut self.out);
        if self.uses_zg {
            format!("    uint64_t _zg = 0;\n{}", out)
        } else {
            out
        }
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
            let ct = self.param_cty(t, p.ty.span);
            let name = match &p.name {
                Name::Ident(id) => {
                    let n = format!("p_{}_{}", id.name, i);
                    // A record parameter is a pointer, read as the record.
                    let shown = if self.vs_class(t) {
                        format!("(*{})", n)
                    } else {
                        n.clone()
                    };
                    self.locals.insert(key(id.span), (shown, t.clone()));
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
    /// A `simd` one is three C functions: its AVX-512 and AVX2 versions, then the
    /// baseline, which calls the widest the CPU has (DESIGN §12.8).
    fn method_as(&mut self, id: ClassId, _idx: usize, m: &MethodInfo, name: String) {
        let is_main = self.prog.classes[id].kind == ClassKind::Script && m.name == "main";
        if m.decl.modifiers.contains(&ast::Modifier::Simd) && !is_main {
            for level in [2, 1] {
                self.method_version(id, m, &format!("{}_simd{}", name, level), Some(level), false);
            }
            self.method_version(id, m, &name, None, true);
        } else {
            self.method_version(id, m, &name, None, false);
        }
    }

    /// One C function for a method: built for the vector unit `level` (`ZR_SIMD1`,
    /// `ZR_SIMD2`) or the baseline, and with `dispatch`, first calling the version for the
    /// widest unit the CPU has.
    fn method_version(
        &mut self,
        id: ClassId,
        m: &MethodInfo,
        name: &str,
        level: Option<u8>,
        dispatch: bool,
    ) {
        let c = &self.prog.classes[id];
        let span = m.decl.name.span;
        let m_ret = self.sb(&m.ret);
        self.begin_fn(id, m_ret.clone(), m.can_fail, !m.is_static, false);
        self.is_main = c.kind == ClassKind::Script && m.name == "main";
        let ret = self.ret_c_of(&m_ret, m.can_fail, span);
        let mut params = Vec::new();
        // An array or record result is built where the caller says (DESIGN §3.3).
        if self.returns_into(&m_ret) {
            self.dst_fn = true;
            params.push("void *_dst".to_string());
            if let Some(body) = &m.decl.body {
                self.result_local = self.result_local_of(&body.stmts, &m.decl.params);
            }
        }
        if !m.is_static {
            let st = self.cty(&self.this_class(), span);
            params.push(format!("{}* self", st));
        }
        let mut ptys: Vec<Ty> = m.params.iter().map(|t| self.sb(t)).collect();
        // A copy for the array layouts it's given: those parameters have the arguments'
        // types (DESIGN §4.3, `layouts.rs`).
        let layout = self.layout_for.take();
        if let Some((over, _)) = &layout {
            for (k, t) in over {
                if let Some(p) = ptys.get_mut(*k) {
                    *p = t.clone();
                }
            }
        }
        params.extend(self.params_c(&m.decl.params, &ptys));
        if let Some((over, origin)) = &layout
            && let Some(body) = &m.decl.body
        {
            self.layout_origin = Some(*origin);
            self.layout_body(m, over, body);
        }
        // A copy for lambdas: its bound parameters aren't passed, and what the lambdas
        // read is (DESIGN §9.3).
        let copy = self.copy_for.take();
        let is_copy = copy.is_some() || layout.is_some();
        if let Some((bound, extra)) = copy {
            let off = params.len() - m.decl.params.len();
            for (k, b) in bound.iter().rev() {
                params.remove(off + k);
                if let Name::Ident(id) = &m.decl.params[*k].name {
                    self.locals.remove(&key(id.span));
                    self.lam_binds.insert(key(id.span), b.clone());
                }
            }
            params.extend(extra);
        }
        let small = !self.is_main
            && level.is_none()
            && !dispatch
            && m.decl.body.as_ref().is_some_and(|b| small_fn(&b.stmts));
        let body = m.decl.body.as_ref().expect("a body");
        // Its calls to itself go to a copy of its base case, when it starts with one and
        // calls itself at least twice. With one call, as in a walk down a list, the copy
        // saves only the last call, and it keeps the C compiler from making the
        // recursion a loop: sexp's walk took 3% longer with it.
        let base = (!self.is_main
            && level.is_none()
            && !dispatch
            && !is_copy
            && !m.can_fail
            && !self.dst_fn
            && self.self_calls(&body.stmts, m) >= 2)
            .then(|| self.base_case(&body.stmts))
            .flatten();
        self.base_name = base.as_ref().map(|_| name.to_string());
        self.label_fn(&m.name);
        self.block_stmts(&body.stmts);
        self.base_name = None;
        let based = std::mem::take(&mut self.base_used);
        // Falling off the end returns status 0, and 0 from `u8 main` (DESIGN §7.2, §8.6).
        if (self.fails && m_ret == Ty::Void) || (self.is_main && m_ret == Ty::u8()) {
            self.line("return 0;");
        }
        let mut out = self.take_body();
        let c_params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        // A function whose base case its calls copy isn't inlined itself, as measured.
        let sig = format!(
            "static {}{} {}({})",
            if small && !based { "inline " } else { "" },
            ret,
            name,
            c_params
        );
        // A wider version exists only where the runtime builds them.
        let (open, attr, close) = match level {
            Some(l) => ("#ifdef ZR_SIMD\n", format!("ZR_SIMD{} ", l), "#endif\n"),
            None => ("", String::new(), ""),
        };
        let _ = write!(self.protos, "{}{}{};\n{}", open, attr, sig, close);
        if dispatch {
            // The C parameters' names, passed on as they came.
            let args: Vec<&str> = params
                .iter()
                .map(|p| p.rsplit([' ', '*']).next().unwrap_or(""))
                .collect();
            let call = |l: u8| {
                let c = format!("{}_simd{}({})", name, l, args.join(", "));
                if ret == "void" {
                    format!("{}; return;", c)
                } else {
                    format!("return {};", c)
                }
            };
            out = format!(
                "#ifdef ZR_SIMD\n    switch (zr_simd_level()) {{\n    case 2: {}\n    case 1: {}\n    default: break;\n    }}\n#endif\n{}",
                call(2),
                call(1),
                out
            );
        }
        let _ = write!(self.bodies, "{}{}{} {{\n{}}}\n\n{}", open, attr, sig, out, close);
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
        if based && let Some(base) = base {
            self.base_copy(id, m, name, &ret, &params, &c_params, &ptys, &body.stmts, base);
        }
    }

    /// The base case of a function that calls itself, which those calls copy (DESIGN
    /// §14.2): how many statements its body starts with that only declare locals and
    /// then return a value computed without calling anything when a test holds, ending
    /// at the first such test; or ending with a `return switch` some of whose cases
    /// call nothing. `None` when the body doesn't start that way. Only one test: each
    /// further one costs every call that isn't a leaf, and the function computes the
    /// locals before it again.
    fn base_case(&self, stmts: &[Stmt]) -> Option<BaseCase> {
        // The size of what's copied so far, as `small_fn` counts, and of the copy found.
        let mut size = 0;
        let mut found = None;
        for (i, s) in stmts.iter().enumerate() {
            let mut n = 0;
            match &s.kind {
                S::Local(d) if self.cheap_locals(d, &mut n) => size += n,
                S::If {
                    cond,
                    then,
                    els: None,
                } if pure_expr(cond, &mut n) && leaf_return(then, &mut n) => {
                    size += n;
                    found = Some((
                        size,
                        BaseCase {
                            len: i + 1,
                            switch: None,
                        },
                    ));
                    break;
                }
                S::Return {
                    status: None,
                    value: Some(v),
                } => {
                    if let E::Switch(sw) = &v.kind
                        && pure_expr(&sw.scrutinee, &mut n)
                    {
                        let leaves = sw
                            .rules
                            .iter()
                            .filter(|r| matches!(&r.body, SwitchBody::Expr(x) if pure_expr(x, &mut n)))
                            .count();
                        if leaves > 0 && leaves < sw.rules.len() {
                            found = Some((
                                size + n,
                                BaseCase {
                                    len: i + 1,
                                    switch: Some(sw.span),
                                },
                            ));
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
        // Copied to every call, so small.
        found.filter(|(size, _)| *size <= 48).map(|(_, b)| b)
    }

    /// How many calls in `stmts` call `m`, outside lambdas.
    fn self_calls(&self, stmts: &[Stmt], m: &MethodInfo) -> usize {
        fn expr(g: &Gen, e: &Expr, m: &MethodInfo, n: &mut usize) {
            if let E::Call { .. } = &e.kind
                && let Some(f) = g.facts.calls.get(&key(e.span))
                && let Origin::Method(c, i) = f.origin
                && std::ptr::eq(&g.prog.classes[c].methods[i], m)
            {
                *n += 1;
            }
            if let E::Lambda { .. } = &e.kind {
                return;
            }
            if let E::Switch(sw) = &e.kind {
                for r in &sw.rules {
                    if let SwitchBody::Block(b) = &r.body {
                        b.stmts.iter().for_each(|s| stmt(g, s, m, n));
                    }
                }
            }
            for x in crate::check::expr_children(e) {
                expr(g, x, m, n);
            }
        }
        fn stmt(g: &Gen, s: &Stmt, m: &MethodInfo, n: &mut usize) {
            let (ss, es) = crate::check::stmt_children(s);
            es.into_iter().for_each(|e| expr(g, e, m, n));
            ss.into_iter().for_each(|s| stmt(g, s, m, n));
        }
        let mut n = 0;
        stmts.iter().for_each(|s| stmt(self, s, m, &mut n));
        n
    }

    /// Whether a declaration declares only numbers, enums and references, each from an
    /// expression that calls nothing: copied to a call, it costs a few loads.
    fn cheap_locals(&self, d: &ast::LocalDecl, size: &mut usize) -> bool {
        d.decls.iter().all(|v| {
            let cheap = match self.facts.locals.get(&key(v.name.span)) {
                Some(Ty::Prim(_) | Ty::Ref(_) | Ty::Nullable(_) | Ty::Slice(_) | Ty::View(_)) => {
                    true
                }
                Some(Ty::Class(c, _)) => self.prog.classes[*c].kind == ClassKind::Enum,
                _ => false,
            };
            cheap && v.capture.is_none() && v.init.as_ref().is_some_and(|e| pure_expr(e, size))
        })
    }

    /// The copy of a function's base case, `name_base`, that its calls to itself call:
    /// the statements `base` counts, then a call to the whole function. Inlined always,
    /// so a call whose test holds computes its result where it is, and only the others
    /// make a call. The function computes its first statements again, which only read.
    #[allow(clippy::too_many_arguments)]
    fn base_copy(
        &mut self,
        id: ClassId,
        m: &MethodInfo,
        name: &str,
        ret: &str,
        params: &[String],
        c_params: &str,
        ptys: &[Ty],
        stmts: &[Stmt],
        base: BaseCase,
    ) {
        let m_ret = self.sb(&m.ret);
        self.begin_fn(id, m_ret, false, !m.is_static, false);
        let _ = self.params_c(&m.decl.params, ptys);
        self.label_fn(&m.name);
        let args: Vec<&str> = params
            .iter()
            .map(|p| p.rsplit([' ', '*']).next().unwrap_or(""))
            .collect();
        let call = format!("{}({})", name, args.join(", "));
        let (copied, last) = match base.switch {
            Some(_) => (&stmts[..base.len - 1], Some(&stmts[base.len - 1])),
            None => (&stmts[..base.len], None),
        };
        for s in copied {
            self.stmt(s);
        }
        match (last, base.switch) {
            (Some(s), Some(sw)) => {
                self.base_switch = Some((key(sw), call));
                self.stmt(s);
                self.base_switch = None;
            }
            _ if ret == "void" => self.line(&format!("{}; return;", call)),
            _ => self.line(&format!("return {};", call)),
        }
        let out = self.take_body();
        let sig = format!(
            "static inline __attribute__((always_inline)) {} {}_base({})",
            ret, name, c_params
        );
        let _ = writeln!(self.protos, "{};", sig);
        let _ = write!(self.bodies, "{} {{\n{}}}\n\n", sig, out);
    }

    fn ctor(&mut self, id: ClassId, idx: usize, k: &crate::program::CtorInfo) {
        let name = self.ctor_name(id, idx);
        self.ctor_as(id, idx, k, name);
    }

    /// Constructor `idx` of class `id` as the C function `name`, in the current
    /// instantiation.
    fn ctor_as(&mut self, id: ClassId, _idx: usize, k: &crate::program::CtorInfo, name: String) {
        if self.vs(&Ty::Class(id, self.cur_cargs.clone())) {
            self.record_ctor(id, k, name);
            return;
        }
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
        let small = k.decl.is_none_or(|d| small_fn(&d.body.stmts));
        let sig = format!(
            "static {}{} {}({})",
            if small { "inline " } else { "" },
            ret,
            name,
            params.join(", ")
        );
        let _ = writeln!(self.protos, "{};", sig);
        // Field initializers run first; growable fields start empty, set up by whoever
        // gave the object its storage.
        for (fname, ty, init) in &fields {
            if let Some(init) = init {
                let place = format!("self->f_{}", fname);
                let code = match self.field_link(id, fname) {
                    Some((kind, nullable)) => {
                        let v = self.expr(init, ty);
                        self.link_store(&place, &kind, nullable, &v, init.span)
                    }
                    None if self.has_link_array(id, fname) => {
                        let v = self.expr(init, ty);
                        self.link_array_store(id, fname, &place, &v, init.span)
                    }
                    None => self.store_into(&place, ty, init),
                };
                self.line(&format!("{};", code));
            }
        }
        match k.decl {
            Some(d) => {
                self.label_fn("new");
                self.block_stmts(&d.body.stmts);
                if k.can_fail {
                    self.line("return 0;");
                }
            }
            None => {
                let mut i = 0;
                for (fname, ty, init) in &fields {
                    if init.is_none() && !starts_empty(ty) {
                        let place = format!("self->f_{}", fname);
                        let arg = format!("a{}", i);
                        let code = match self.field_link(id, fname) {
                            Some((kind, nullable)) => {
                                self.link_store(&place, &kind, nullable, &arg, span)
                            }
                            None if self.has_link_array(id, fname) => {
                                self.link_array_store(id, fname, &place, &arg, span)
                            }
                            None => format!("{} = {}", place, arg),
                        };
                        self.line(&format!("{};", code));
                        i += 1;
                    }
                }
            }
        }
        let out = self.take_body();
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
        if self.threads {
            links.push("pthread");
        }
        for l in &links {
            let _ = writeln!(out, "{}{} */", crate::driver::LINK_MARK, l);
        }
        out.push_str("/* Generated by zink. */\n");
        if self.threads {
            out.push_str("#define ZR_THREADS 1\n");
        }
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
        let packed = std::mem::take(&mut self.packed);
        let index: HashMap<String, usize> = defs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.0.clone(), i))
            .collect();
        fn emit(
            i: usize,
            defs: &[(String, String, Vec<String>)],
            index: &HashMap<String, usize>,
            packed: &HashSet<String>,
            done: &mut HashSet<String>,
            out: &mut String,
        ) {
            if !done.insert(defs[i].0.clone()) {
                return;
            }
            for d in &defs[i].2 {
                if let Some(&j) = index.get(d) {
                    emit(j, defs, index, packed, done, out);
                }
            }
            let attr = if packed.contains(&defs[i].0) {
                "__attribute__((packed)) "
            } else {
                ""
            };
            let _ = writeln!(out, "struct {}{} {{ {}}};", attr, defs[i].0, defs[i].1);
        }
        for i in 0..defs.len() {
            emit(i, &defs, &index, &packed, &mut done, &mut out);
        }
        out.push('\n');
        out.push_str(&self.sites);
        out.push_str(&self.protos);
        out.push('\n');
        out.push_str(&self.helpers);
        let _ = writeln!(
            out,
            "\nstatic void zr_init_statics(void) {{\n    uint64_t _zg = 0; (void)_zg;\n{}}}\n",
            self.statics_init
        );
        if self.threads {
            out.push_str(&self.statics_copy_fns());
            out.push('\n');
        }
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
            for (d, k) in ds.iter().rev() {
                self.line(&format!("if ({k}_on) zr_catch_leave(&{k});", k = k));
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
        let blocks: Vec<Vec<(Stmt, String)>> = self.defers[from..].iter().rev().cloned().collect();
        for b in blocks {
            for (d, k) in b.iter().rev() {
                self.line(&format!("if ({k}_on) zr_catch_leave(&{k});", k = k));
                self.stmt(d);
            }
        }
    }

    /// A `defer` while an `@@` is active (DESIGN §6.4, §7.6): a catch record whose landing
    /// runs the deferred statement when a failure nothing read abandons this function, and
    /// goes on to the next record. The variables it uses are kept in memory, so it sees
    /// their values as they are when the jump comes.
    fn defer_record(&mut self, d: &Stmt) -> String {
        let k = self.tmp("kd");
        let mut names = Vec::new();
        stmt_idents(d, &mut names);
        let mut keep = String::new();
        for span in names {
            if let Some(NameRes::Local(decl)) = self.facts.names.get(&key(span))
                && let Some((c, _)) = self.locals.get(decl)
                && is_c_name(c)
            {
                keep.push_str(&format!(
                    "__asm__ volatile(\"\" : : \"r\"(&{}) : \"memory\"); ",
                    c
                ));
            }
        }
        self.line(&format!("zr_catch {k}; int {k}_on = 0;", k = k));
        self.line(&format!("if (zr_catch_top) {{ {}", keep));
        self.indent += 1;
        self.line(&format!("if (setjmp({}.jb)) {{", k));
        self.indent += 1;
        self.stmt(d);
        self.line(&format!("zr_abandon_on({}.st);", k));
        self.indent -= 1;
        self.line("}");
        self.line(&format!("zr_catch_enter(&{k}); {k}_on = 1;", k = k));
        self.indent -= 1;
        self.line("}");
        k
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
                let tests = self.reads_status_var(cond) as usize;
                self.status_tests += tests;
                self.line(&format!("if ({})", c));
                self.body(then);
                if let Some(e) = els {
                    self.line("else");
                    self.body(e);
                }
                self.status_tests -= tests;
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
                // A loop whose indexes move with its counter runs without their checks
                // when a test before it shows them all in range (DESIGN §14.2).
                match self.version_plan(init, cond, update, body) {
                    Some(plan) => {
                        self.line(&format!(
                            "if ({}) {{ /* in range: the loop without its index checks */",
                            plan.check
                        ));
                        self.indent += 1;
                        let outer = std::mem::replace(&mut self.unchecked_ix, plan.spans);
                        self.for_loop(label.clone(), cond.as_ref(), update, body);
                        self.unchecked_ix = outer;
                        self.indent -= 1;
                        self.line("} else {");
                        self.indent += 1;
                        self.for_loop(label, cond.as_ref(), update, body);
                        self.indent -= 1;
                        self.line("}");
                    }
                    None => self.for_loop(label, cond.as_ref(), update, body),
                }
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
                if !self.defers.is_empty() {
                    let k = self.defer_record(inner);
                    if let Some(d) = self.defers.last_mut() {
                        d.push(((**inner).clone(), k));
                    }
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

    /// A `for` loop's head and body, after its initializer.
    fn for_loop(
        &mut self,
        label: Option<String>,
        cond: Option<&Expr>,
        update: &[Expr],
        body: &Stmt,
    ) {
        let c = cond.map(|c| self.cond(c)).unwrap_or_default();
        let ups: Vec<String> = update.iter().map(|u| self.expr_stmt_code(u)).collect();
        self.loop_(
            label,
            &format!("for (; {}; {})", c, ups.join(", ")),
            body,
            None,
        );
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
            // A variable-size record: a pointer to its bytes, on a data stack of its own,
            // built there (DESIGN §3.2, §3.3). The name reads as the record itself.
            if self.vs_class(&ty) {
                let site = self.site();
                // The record the function returns starts in its caller's storage.
                let at = if self.result_local == Some(key(v.name.span)) {
                    format!("(_dst ? _dst : zr_stack({}))", site)
                } else {
                    format!("zr_stack({})", site)
                };
                self.line(&format!("{} *{} = ({} *){};", ct, name, ct, at));
                self.locals
                    .insert(key(v.name.span), (format!("(*{})", name), ty.clone()));
                // A failing constructor's or function's status goes to the capture.
                if let Some(c) = &v.capture
                    && c.catches_inside()
                {
                    self.unsupported(v.name.span, "`@@` on a record built in place");
                }
                let status = match v.capture.as_ref().and_then(|c| c.one.as_ref()) {
                    Some(Name::Ident(c)) => {
                        let (cn, _) = self.new_local(c);
                        self.status_vars.insert(c.name.clone());
                        self.line(&format!("uint8_t {} = 0;", cn));
                        Some(cn)
                    }
                    _ => None,
                };
                if let Some(init) = &v.init {
                    let cur = self.tmp("cur");
                    self.build_status = status;
                    let b = self.build(&cur, init, &ty);
                    self.build_status = None;
                    self.line(&format!(
                        "{{ uint8_t *{} = (uint8_t *){}; {}}}",
                        cur, name, b
                    ));
                }
                continue;
            }
            if on_stack(&ty) {
                // The array the function returns starts in its caller's storage.
                let init = if self.result_local == Some(key(v.name.span))
                    && layout(&ty) == Some(Layout::Vec)
                {
                    let s = self.site();
                    if self.vs_elems(&ty) {
                        format!("((zr_rvec){{_dst ? _dst : zr_stack({}), 0, 0}})", s)
                    } else {
                        format!("((zr_vec){{_dst ? _dst : zr_stack({}), 0}})", s)
                    }
                } else {
                    self.stack_init(&ty)
                };
                self.line(&format!("{} {} = {};", ct, name, init));
                let named = v.capture.as_ref().is_some_and(|c| {
                    c.catches_inside() || c.names().any(|n| matches!(n, Name::Ident(_)))
                });
                match (&v.init, &v.capture) {
                    (Some(init), Some(c)) if named => match (&ty, layout(&ty)) {
                        // A failing call's array comes with its status, then is copied in
                        // as store_into copies a slice: `u8[.] p @err = path.resolve(x);`.
                        (Ty::Array(el, _), Some(Layout::Vec)) => {
                            let el = (**el).clone();
                            let places = self.capture_places(c, true);
                            self.direct_result = matches!(strip_paren(init).kind, E::Call { .. })
                                .then(|| format!("{}.p", name));
                            let (val, pre, one, deep) = self.with_status_in(
                                init,
                                &Ty::Slice(Box::new(el.clone())),
                                c.catches_inside(),
                            );
                            self.direct_result = None;
                            let sz = self.elem_size(&el, init.span);
                            let set = self.capture_set(&places, &one, &deep, init.span);
                            self.line(&format!(
                                "({{ {} zr_slice _s = {}; {} if (_s.p != {n}.p) memmove({n}.p, _s.p, (size_t)_s.n * {}); {n}.n = _s.n; }});",
                                pre,
                                val,
                                set,
                                sz,
                                n = name
                            ));
                        }
                        _ => {
                            self.unsupported(init.span, "a status with this kind of array");
                        }
                    },
                    (Some(init), _) => {
                        self.fresh_dst = true;
                        let code = self.store_into(&name, &ty, init);
                        self.fresh_dst = false;
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
                        if let Some(c) = &v.capture
                            && c.catches_inside()
                        {
                            self.unsupported(*span, "`@@` on a value with a growable field");
                        }
                        let st = self.construct(&name, args, *span);
                        if let Some(Name::Ident(c)) = v.capture.as_ref().and_then(|c| c.one.as_ref()) {
                            let (cn, _) = self.new_local(c);
                            self.status_vars.insert(c.name.clone());
                            self.line(&format!("uint8_t {} = {};", cn, st));
                            self.uses_zg = true;
                            self.line(&format!("if ({}) _zg = zr_trail_claim();", cn));
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
                    if let Some(c) = &v.capture {
                        let places = self.capture_places(c, true);
                        let (val, pre, one, deep) =
                            self.with_status_in(init, &ty, c.catches_inside());
                        let set = self.capture_set(&places, &one, &deep, init.span);
                        self.line(&format!(
                            "{} {} = ({{ {} __typeof__({}) _v = {}; {} _v; }});",
                            ct, name, pre, val, val, set
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

    /// Whether a value of this type is copied whole by a C assignment, as an element a
    /// built-in such as `filter` copies: not an array or map, a variable-size record, a
    /// class with a growable field, or a value with links.
    fn plain_elem(&self, t: &Ty) -> bool {
        let t = settle(t);
        if on_stack(&t) || self.vs(&t) || self.prog.link_bearing(&t) || t == Ty::Void {
            return false;
        }
        match &t {
            Ty::Class(id, _) => self.growable_fields(*id) == 0,
            Ty::Unknown => false,
            _ => true,
        }
    }

    fn growable_fields(&self, id: ClassId) -> usize {
        self.prog.classes[id]
            .fields
            .iter()
            .filter(|f| !f.is_static && starts_empty(&f.ty))
            .count()
    }

    /// Statements that make the value at `place`, of type `t`, its zero value again: zero
    /// bytes, except that a growable field, whose storage whoever made the value set up,
    /// is emptied, keeping its storage.
    fn reset_value(&mut self, place: &str, t: &Ty, span: Span) -> String {
        if let Ty::Class(id, args) = t
            && self.prog.classes[*id].kind != ClassKind::Enum
            && self.holds_growable(t)
        {
            let mut out = String::new();
            for (i, f) in self.prog.classes[*id].fields.iter().enumerate() {
                if f.is_static {
                    continue;
                }
                let ft = self.field_ty(*id, i, args);
                let fp = format!("{}.f_{}", place, f.name);
                out.push_str(&self.reset_value(&fp, &ft, span));
            }
            return out;
        }
        if starts_empty(t) {
            return match t {
                Ty::Map(..) | Ty::Set(_) => format!("zr_map_clear(&{}); ", place),
                Ty::Pool(el) => {
                    let slot = self.pool_slot(el, span);
                    format!("zr_pool_clear(&{}, sizeof({})); ", place, slot)
                }
                _ if self.vs_elems(t) => format!("{p}.n = 0; {p}.bytes = 0; ", p = place),
                _ => {
                    let el = t.elem().cloned().unwrap_or(Ty::u8());
                    let arr = Arr {
                        a: format!("(&{})", place),
                        lay: layout(t).unwrap_or(Layout::Vec),
                        ec: self.cty(&el, span),
                        cap: cap_of(t),
                    };
                    format!("{} ", arr.clear())
                }
            };
        }
        format!("memset(&{p}, 0, sizeof {p}); ", p = place)
    }

    /// Whether a value of `t` holds a growable field, itself or in a field it holds by value.
    fn holds_growable(&self, t: &Ty) -> bool {
        let Ty::Class(id, args) = t else { return false };
        (0..self.prog.classes[*id].fields.len()).any(|i| {
            let f = &self.prog.classes[*id].fields[i];
            if f.is_static {
                return false;
            }
            let ft = self.field_ty(*id, i, args);
            starts_empty(&ft) || (matches!(ft, Ty::Class(..)) && self.holds_growable(&ft))
        })
    }

    /// Whether the program has the one `main` it starts from: a top-level function of one
    /// of its files (DESIGN §6.1). It's reported when it has none, or more than one.
    fn one_main(&mut self) -> bool {
        let prog = self.prog;
        let mut mains = Vec::new();
        let mut in_class = None;
        for c in &prog.classes {
            if crate::stdlib::reserved(&prog.files[c.file].package) {
                continue;
            }
            for m in c.methods.iter().filter(|m| m.name == "main") {
                if c.kind == ClassKind::Script {
                    mains.push((c.file, m.decl.name.span));
                } else if in_class.is_none() {
                    in_class = Some((c.file, m.decl.name.span, c.name.clone()));
                }
            }
        }
        let help = "write `void main() { ... }` outside any class (DESIGN §6.1)";
        match mains.as_slice() {
            [_] => true,
            [] => {
                let d = match &in_class {
                    Some((file, span, class)) => (
                        *file,
                        Diagnostic::error(
                            *span,
                            format!("this program has no `main`: `{}.main` is a method of a class, not where a program starts", class),
                        )
                        .with_help(help),
                    ),
                    None => (
                        0,
                        Diagnostic::error(Span::default(), "this program has no `main` to start from")
                            .with_help(help),
                    ),
                };
                self.errs.push(d);
                false
            }
            [(f0, s0), rest @ ..] => {
                let (line, _) = self.sources[*f0].line_col(s0.start as usize);
                let first = format!("{}:{}", self.sources[*f0].name, line);
                for (f, s) in rest {
                    self.errs.push((
                        *f,
                        Diagnostic::error(
                            *s,
                            "a program starts from one `main`, and this is another",
                        )
                        .with_help(format!(
                            "the first is at {}; give this one another name",
                            first
                        )),
                    ));
                }
                false
            }
        }
    }

    /// An empty array on data stacks of its own: one, or two for a cursor array.
    fn stack_init(&mut self, ty: &Ty) -> String {
        if matches!(ty, Ty::Map(..) | Ty::Set(_)) {
            let a = self.site();
            return format!("zr_map_new(zr_stack({}))", a);
        }
        if matches!(ty, Ty::Pool(_)) {
            let a = self.site();
            return format!("zr_pool_new(zr_stack({}))", a);
        }
        if self.vs_elems(ty) {
            let a = self.site();
            return format!("((zr_rvec){{zr_stack({}), 0, 0}})", a);
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
        let Some((cargs, call)) = self.ctor_args(args, span) else {
            return self.unsupported(span, "this constructor");
        };
        call(self, &format!("&{}", place), &cargs)
    }

    /// The C for a constructor's arguments, in order, and what makes the call from them
    /// and the address of the object to build.
    #[allow(clippy::type_complexity)]
    fn ctor_args(
        &mut self,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<(Vec<String>, Box<dyn Fn(&mut Self, &str, &[String]) -> String>)> {
        let fact = self.facts.calls.get(&key(span))?;
        let Origin::Ctor(c, i) = fact.origin else {
            return None;
        };
        let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
        let mut cargs = Vec::new();
        for (a, p) in args.iter().zip(&params) {
            cargs.push(self.arg_code(&a.expr, p));
        }
        let call = move |me: &mut Self, at: &str, cargs: &[String]| me.ctor_call(c, i, at, cargs, span);
        Some((cargs, Box::new(call)))
    }

    /// A call to constructor `i` of class `c`, building at the address `at`.
    fn ctor_call(&mut self, c: ClassId, i: usize, at: &str, args: &[String], span: Span) -> String {
        let mut cargs = vec![at.to_string()];
        cargs.extend(args.iter().cloned());
        let fails = self.prog.classes[c].ctors[i].can_fail;
        let fname = if self.prog.classes[c].type_params.is_empty() {
            self.want_lib(c, i, true);
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
        let fresh = std::mem::take(&mut self.fresh_dst);
        if let Ty::Array(el, _) = ty
            && self.vs(el)
        {
            let el = (**el).clone();
            return self.rvec_store(place, init, &el);
        }
        // A map, set or pool starts empty on its data stack; `[]` is all that makes one.
        if matches!(ty, Ty::Map(..) | Ty::Set(_) | Ty::Pool(_)) {
            if matches!(&init.kind, E::ArrayLit(items) if items.is_empty()) {
                return "(void)0".into();
            }
            return self.unsupported(init.span, "copying a map, a set or a pool");
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
                    // A call builds its array in a new variable's storage (DESIGN §3.3).
                    // An existing one may be what the arguments read, so the call builds
                    // on its own stack and the result is copied.
                    self.direct_result =
                        matches!(strip_paren(init).kind, E::Call { .. }).then(|| {
                            if fresh {
                                format!("{}.p", place)
                            } else {
                                "NULL".to_string()
                            }
                        });
                    let src = self.expr(init, &Ty::Slice(Box::new(el.clone())));
                    self.direct_result = None;
                    let sz = self.elem_size(&el, init.span);
                    return format!(
                        "({{ zr_slice _s = {}; if (_s.p != {p}.p) memmove({p}.p, _s.p, (size_t)_s.n * {}); {p}.n = _s.n; }})",
                        src,
                        sz,
                        p = place
                    );
                }
            }
        }
        let v = self.expr(init, ty);
        format!("{} = {}", place, v)
    }

    /// The C status a `return` writes; "0" for none, and for `##`, whose status comes from
    /// its value.
    fn status_text(&mut self, status: Option<&ast::StatusWrite>) -> String {
        match status {
            None => "0".to_string(),
            Some(ast::StatusWrite::Int(n)) => n.to_string(),
            Some(ast::StatusWrite::Name(q)) => {
                if q.parts.len() == 2 && q.parts[0].name == "status" {
                    crate::builtins::status_code(&q.parts[1].name)
                        .unwrap_or(1)
                        .to_string()
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
            Some(ast::StatusWrite::Pass) => "0".to_string(),
        }
    }

    /// Whether a failing `return` passes on a failure the function read, and so continues
    /// its trail (DESIGN §7.6).
    fn passes_on(&self, status: Option<&ast::StatusWrite>) -> bool {
        matches!(status, Some(ast::StatusWrite::Pass))
            || self.status_tests > 0
            || match status {
                Some(ast::StatusWrite::Name(q)) => {
                    q.parts.len() == 1 && self.status_vars.contains(&q.parts[0].name)
                }
                Some(ast::StatusWrite::Expr(e)) => self.reads_status_var(e),
                _ => false,
            }
    }

    fn return_(&mut self, status: Option<&ast::StatusWrite>, value: Option<&Expr>, span: Span) {
        if self.inline_return(status, value, span) {
            return;
        }
        let st = self.status_text(status);
        let pass = matches!(status, Some(ast::StatusWrite::Pass));
        let passes_on = self.passes_on(status);
        let ret = self.ret.clone();
        // `return ## f();` in a function or constructor that returns nothing: the call, for
        // its status (DESIGN §7.2).
        let (st, value) = match value {
            Some(v) if pass && (self.is_ctor || ret == Ty::Void) => {
                let (_, pre, one, _) = self.with_status_in(v, &Ty::Void, false);
                self.line(&pre);
                let t = self.tmp("ps");
                self.line(&format!("uint8_t {} = {};", t, one));
                (t, None)
            }
            _ => (st, value),
        };
        let mut st = st;
        if self.is_ctor {
            self.run_defers(0);
            if self.rec_ctor.is_some() {
                let t = self.tmp("ret");
                self.line(&format!("uint8_t {} = {};", t, st));
                self.set_at(&format!("{} != 0", t), &st, passes_on, span);
                let st = if st == "0" { st } else { t };
                let end = self.record_ctor_end(&st);
                self.line(&end);
                return;
            }
            if self.fails {
                let t = self.tmp("ret");
                self.line(&format!("uint8_t {} = {};", t, st));
                self.set_at(&format!("{} != 0", t), &st, passes_on, span);
                // A constructor that fails leaves its class's zero value: the fields it
                // set are reset (DESIGN §6.2).
                if st != "0" {
                    let this = self.this_class();
                    let reset = self.reset_value("(*self)", &this, span);
                    self.line(&format!("if ({}) {{ {}}}", t, reset));
                }
                self.line(&format!("return {};", t));
            } else {
                self.line("return;");
            }
            return;
        }
        let val = value.map(|v| {
            if !self.dst_fn {
                return self.ret_value(v, &ret, pass, &mut st);
            }
            if self.vs_class(&ret) {
                if pass {
                    self.unsupported(v.span, "`return ##` of a variable-size record");
                }
                return self.record_return(v, &ret, span);
            }
            // Into the caller's destination, when it gave one: the result local is already
            // there, and a call is passed it, unless the result local is in use, which the
            // call's arguments could be reading.
            let is_call = matches!(strip_paren(v).kind, E::Call { .. });
            if is_call && self.result_local.is_none() {
                self.direct_result = Some("_dst".into());
            }
            let el = ret.elem().cloned().unwrap_or(Ty::u8());
            // A slice or a literal is copied in too; with no destination, it is returned as
            // it is, since a caller without one copies the result at once.
            let code = if matches!(settle(&self.ty(v)).unref(), Ty::Array(..)) {
                self.ret_value(v, &ret, pass, &mut st)
            } else {
                let s = self.ret_value(v, &Ty::Slice(Box::new(el.clone())), pass, &mut st);
                format!("({{ zr_slice _rs = {}; (zr_vec){{_rs.p, _rs.n}}; }})", s)
            };
            self.direct_result = None;
            // An array of variable-size records is its bytes.
            let bytes = if self.vs_elems(&ret) {
                "(size_t)_rv.bytes".to_string()
            } else {
                format!("(size_t)_rv.n * {}", self.elem_size(&el, span))
            };
            format!(
                "({{ __typeof__({c}) _rv = {c}; if (_dst && _dst != _rv.p) {{ memmove(_dst, _rv.p, {b}); _rv.p = _dst; }} _rv; }})",
                c = code,
                b = bytes
            )
        });
        let rc = self.ret_c_of(&ret, self.fails, span);
        match (self.fails, val) {
            (true, Some(v)) => {
                let t = self.tmp("ret");
                self.line(&format!("{} {} = {{{}, {}}};", rc, t, v, st));
                self.run_defers(0);
                self.set_at(&format!("{}.s != 0", t), &st, passes_on, span);
                self.line(&format!("return {};", t));
            }
            (true, None) => {
                let t = self.tmp("ret");
                self.line(&format!("uint8_t {} = {};", t, st));
                self.run_defers(0);
                self.set_at(&format!("{} != 0", t), &st, passes_on, span);
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

    fn destructure(&mut self, targets: &[Target], capture: Option<&Capture>, value: &Expr) {
        let vt = self.ty(value);
        let Ty::Tuple(ts) = settle(&vt) else {
            self.unsupported(value.span, "this destructuring");
            return;
        };
        let t = self.tmp("tup");
        if let Some(c) = capture {
            let declare = targets.iter().any(|t| matches!(t, Target::Decl(..)));
            let places = self.capture_places(c, declare);
            let (v, pre, one, deep) = self.with_status_in(value, &vt, c.catches_inside());
            self.line(&pre);
            self.line(&format!("__typeof__({}) {} = {};", v, t, v));
            let set = self.capture_set(&places, &one, &deep, value.span);
            self.line(&set);
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
        if matches!(it.unref(), Ty::Pool(_)) {
            self.pool_for_each(label, name, iter, body, by_ref);
            self.indent -= 1;
            self.line("}");
            return;
        }
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
        if self.vs_elems(&it) {
            self.rvec_for_each(label, name, iter, body);
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
        let link = self.link_array(iter);
        if link.is_some() {
            self.link_ok.insert(key(strip_paren(iter).span));
        }
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
        let mut at = looped.at(&i);
        // An array of links gives the references its offsets hold.
        if let Some((_, nullable, target)) = &link {
            at = self.link_follow(&at, *nullable, target, iter.span);
        }
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
        capture: Option<&Capture>,
        body: &Stmt,
    ) {
        let places = match capture {
            Some(c) => self.capture_places(c, true),
            None => [None, None, None],
        };
        let inside = capture.is_some_and(|c| c.catches_inside());
        let st = self.ty(source);
        let lp = self.enter_loop(label);
        let (brk, cont) = (lp.brk.clone(), lp.cont.clone());
        self.loops.push(lp);
        self.line("for (;;) {");
        self.indent += 1;
        // Each pass's value is gone when the next call runs (REFERENCES §9), so an array
        // result can stay on the callee's stack rather than pile up on the scratch stack.
        if matches!(strip_paren(source).kind, E::Call { .. }) {
            self.direct_result = Some("NULL".into());
        }
        let (val, pre, one, deep) = self.with_status_in(source, &st, inside);
        self.direct_result = None;
        let st = if st.is_unknown() {
            self.last_status_ret.clone()
        } else {
            st
        };
        self.line(&pre);
        let r = self.tmp("r");
        if st != Ty::Void {
            self.line(&format!("__typeof__({}) {} = {};", val, r, val));
        }
        let s = self.tmp("s");
        self.line(&format!("uint8_t {} = {};", s, deep));
        let set = self.capture_set(&places, &one, &deep, source.span);
        self.line(&format!("if ({}) {{ {} goto {}; }}", s, set, brk));
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
        for (k, p) in places.iter().enumerate() {
            if let Some(p) = p {
                if k == 2 {
                    self.line(&format!("({}).n = 0;", p));
                } else {
                    self.line(&format!("{} = 0;", p));
                }
            }
        }
        self.indent -= 1;
        self.line("}");
        self.loops.pop();
        self.line(&format!("{}: ;", brk));
    }

    fn switch_stmt(&mut self, sw: &ast::Switch) {
        let st = switch_ty(settle(self.ty(&sw.scrutinee).unref()));
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
            ast::CaseLabel::Ident(id) | ast::CaseLabel::Static { name: id, .. } => match st {
                Ty::Class(c, _) => {
                    let i = self.prog.classes[*c]
                        .constants
                        .iter()
                        .position(|k| *k == id.name)
                        .unwrap_or(0);
                    format!("{} == {}", v, i)
                }
                _ => match self.constant_at(id.span) {
                    Some(ConstVal::Str(s)) => format!("zr_eq({}, {}, 1)", v, lit(&s)),
                    Some(ConstVal::Int(x)) => format!("{} == {}", v, self.int_lit(x, st)),
                    _ => self.unsupported(id.span, "a case label that isn't a constant"),
                },
            },
        }
    }

    /// The constant the name at `span` reads, if it reads one (DESIGN §6.1).
    pub(super) fn constant_at(&self, span: Span) -> Option<ConstVal> {
        match self.facts.names.get(&key(span))? {
            NameRes::Field(c, i) => {
                let f = &self.prog.classes[*c].fields[*i];
                if !(f.is_static && f.is_final) {
                    return None;
                }
                self.prog.const_value(*c, &f.name).ok().flatten()
            }
            _ => None,
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
        // A value with a growable field can't be copied yet: the copy would share the
        // field's storage with the original, as `logs +> a` once did. A method's plain
        // parameter may share it, since it's a read-only copy for the call.
        let borrow = std::mem::take(&mut self.borrow_arg);
        if !borrow
            && matches!(
                strip_paren(e).kind,
                E::Ident(_) | E::Member { .. } | E::Index { .. }
            )
            && !is_ref(want)
            && self.holds_growable(&settle(want))
        {
            return self.unsupported(e.span, "copying a value with a growable field");
        }
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
                if let Ty::Queue(el) = want {
                    let ec = self.cty(el, e.span);
                    return format!("zr_q_new(sizeof({}))", ec);
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
                // A constructor that can fail: a failure nothing reads stops the program,
                // as a function's does (DESIGN §7.6).
                if matches!(self.facts.calls.get(&key(e.span)), Some(f) if matches!(f.origin, Origin::Ctor(c, i) if self.prog.classes[c].ctors[i].can_fail))
                {
                    let c = self.call_any(e);
                    return if c.fails {
                        format!("({}).v", c.code)
                    } else {
                        c.code
                    };
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
            // A fixed array's length is its type's.
            Some(Layout::Fixed) => ("_c.e".to_string(), String::new()),
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
            // A fixed array takes exactly its length.
            (Ty::Slice(_) | Ty::View(_), Ty::Array(..)) if cap_of(to) > 0 => {
                let k = cap_of(to);
                let loc = self.loc(span);
                let fill = self.capped_fill(to, "_s.n", "_s.p", span);
                let (test, msg) = if layout(to) == Some(Layout::Fixed) {
                    ("!=", format!("the value isn't the {} elements the array holds", k))
                } else {
                    (">", format!("the value doesn't fit in an array that holds {}", k))
                };
                let bad = self.on_fail(34, format!("zr_stop({}, 34, \"{}\")", loc, msg));
                format!(
                    "({{ zr_slice _s = {}; if (_s.n {} {}) {{ {}; }} {} _c; }})",
                    code, test, k, bad, fill
                )
            }
            (Ty::Prim(_) | Ty::IntLit { .. } | Ty::FloatLit, Ty::Prim(p)) => {
                format!("(({}){})", prim_c(*p), code)
            }
            // A fixed or capped array copied from an array of another layout takes its
            // elements through a slice, so it stops the program when they don't fit.
            (Ty::Array(fe, _), Ty::Array(..))
                if cap_of(to) > 0 && self.cty(from, span) != self.cty(to, span) =>
            {
                let st = Ty::Slice(fe.clone());
                let s = self.conv(code, from, &st, span);
                self.conv(&s, &st, to, span)
            }
            (Ty::Array(..), Ty::Array(..)) if self.cty(from, span) != self.cty(to, span) => {
                let what = format!(
                    "copying a {} into a {}",
                    self.prog.show(from),
                    self.prog.show(to)
                );
                self.unsupported(span, &what)
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
                let (code, fails) = self.insert_status(e);
                let c = CallOut {
                    code,
                    ret: Ty::Void,
                    fails,
                };
                format!("(void)({})", self.check_unread(c, e.span).code)
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

    /// The value and the status of an expression whose status is read (DESIGN §7.3): the
    /// value, the statements to run first, the status `@` reads, and the status `@@` reads.
    /// Everything that can fail inside `e` is caught (DESIGN §7.6): a call nothing else
    /// reads, an index, a slice, arithmetic, a cast. The first failure skips the rest, gives
    /// its status, and leaves the zero value; a reference then refers to a zero value of
    /// its own. Lambdas inside are functions of their own, and aren't part of it.
    /// With `inside`, for `@@` and `@@@`, a failure nothing read inside the calls is caught
    /// too: it jumps back to a `setjmp` here, abandoning them. `@` then reads ABANDONED,
    /// and `@@` the failure that abandoned them.
    fn with_status_in(
        &mut self,
        e: &Expr,
        want: &Ty,
        inside: bool,
    ) -> (String, String, String, String) {
        let e = strip_paren(e);
        let cs = self.tmp("cs");
        let cl = self.tmp("cl");
        // A record built in place has no zero value to leave, so its checks still stop.
        let et = settle(&self.ty(e));
        let catchable = !self.vs_class(&et);
        if inside && !catchable {
            self.unsupported(e.span, "`@@` on a record built in place");
        }
        let saved = if catchable {
            self.catching.replace(Catch {
                cs: cs.clone(),
                cl: cl.clone(),
                used: false,
            })
        } else {
            self.catching.take()
        };
        // A reference to an element is taken where it is, not copied.
        let by_ref = is_ref(want) && !is_status_source(e);
        let mut c = self.status_call(e);
        if by_ref {
            c.code = format!("(&({}))", c.code);
            c.ret = Ty::Ref(Box::new(c.ret.clone()));
        }
        let caught =
            std::mem::replace(&mut self.catching, saved).is_some_and(|k| k.used) || inside;
        self.last_status_ret = c.ret.clone();
        let r = self.tmp("rs");
        let void_ok = !c.fails && c.ret == Ty::Void;
        // `@@`: a record on the catch stack, which a failure nothing read jumps back to.
        let ab = self.tmp("ab");
        let (enter, leave) = if inside {
            (
                format!(
                    "zr_catch _k; zr_catch_enter(&_k); if (setjmp(_k.jb)) {{ {ab} = _k.st; goto {cl}; }}",
                    ab = ab,
                    cl = cl
                ),
                "zr_catch_leave(&_k);".to_string(),
            )
        } else {
            (String::new(), String::new())
        };
        let ab_decl = if inside {
            format!("uint8_t {} = 0; ", ab)
        } else {
            String::new()
        };
        // A call that returns nothing and can't fail has status 0.
        let pre = if caught && void_ok {
            format!(
                "{ab_decl}uint8_t {cs} = 0; ({{ __label__ {cl}; {enter} (void)({}); {cl}: {leave} }});",
                c.code,
                cs = cs,
                cl = cl,
                ab_decl = ab_decl,
                enter = enter,
                leave = leave
            )
        } else if caught {
            let cd = self.tmp("cd");
            // On failure, the value is zero, and a reference refers to a zero value.
            let zero = match (c.fails, c.ret.clone()) {
                (false, Ty::Ref(_)) => "_cx = zr_zero_ref(sizeof *_cx);".to_string(),
                (true, Ty::Ref(_)) => {
                    "memset(&_cx, 0, sizeof _cx); _cx.v = zr_zero_ref(sizeof *_cx.v);".to_string()
                }
                _ => "memset(&_cx, 0, sizeof _cx);".to_string(),
            };
            format!(
                "{ab_decl}uint8_t {cs} = 0; __auto_type {r} = ({{ __label__ {cl}, {cd}; {enter} __auto_type _cx = {}; {leave} goto {cd}; {cl}: {leave} {zero} {cd}: ; _cx; }});",
                c.code,
                cs = cs,
                cl = cl,
                cd = cd,
                r = r,
                zero = zero,
                ab_decl = ab_decl,
                enter = enter,
                leave = leave
            )
        } else if void_ok {
            format!("(void)({});", c.code)
        } else {
            format!("__auto_type {} = {};", r, c.code)
        };
        let (val, st) = match (c.fails, c.ret == Ty::Void) {
            (true, true) => ("0".to_string(), r.clone()),
            (true, false) if self.vs_class(&c.ret) => (format!("(*{}.v)", r), format!("{}.s", r)),
            (true, false) => (format!("{}.v", r), format!("{}.s", r)),
            (false, true) => ("0".to_string(), "0".to_string()),
            (false, false) => (r.clone(), "0".to_string()),
        };
        let st = match (caught, st.as_str()) {
            (false, _) => st,
            (true, "0") => cs,
            (true, _) => format!("({cs} ? {cs} : {st})", cs = cs, st = st),
        };
        let (one, deep) = if inside {
            (
                format!("({ab} ? 200 : {st})", ab = ab, st = st),
                format!("({ab} ? {ab} : {st})", ab = ab, st = st),
            )
        } else {
            (st.clone(), st)
        };
        // The reader claims the failure's trail, to pass it on (DESIGN §7.6).
        let pre = if deep == "0" {
            pre
        } else {
            self.uses_zg = true;
            format!("{} if ({}) _zg = zr_trail_claim();", pre, deep)
        };

        let val = if c.ret == Ty::Void || *want == Ty::Void {
            val
        } else {
            self.conv(&val, &c.ret, want, e.span)
        };
        (val, pre, one, deep)
    }

    /// The C places of a capture's destinations (DESIGN §7.3), each declared as a new
    /// variable when `declare`.
    fn capture_places(&mut self, c: &Capture, declare: bool) -> [Option<String>; 3] {
        let mut out = [None, None, None];
        for (k, n) in [&c.one, &c.deep, &c.trail].into_iter().enumerate() {
            let Some(Name::Ident(id)) = n else { continue };
            if k < 2 {
                self.status_vars.insert(id.name.clone());
            }
            let place = if declare {
                let (cn, ty) = self.new_local(id);
                if k == 2 {
                    let ct = self.cty(&ty, id.span);
                    self.line(&format!("{} {} = {{0}};", ct, cn));
                } else {
                    self.line(&format!("uint8_t {} = 0;", cn));
                }
                cn
            } else {
                let ce = Expr {
                    kind: E::Ident(id.name.clone()),
                    span: id.span,
                };
                self.place(&ce).0
            };
            out[k] = Some(place);
        }
        out
    }

    /// Sets a capture's destinations, once the statements `with_status_in` gives have
    /// run: `one` and `deep` are the statuses `@` and `@@` read, and `span` the line
    /// that reads them, where a trail with nothing in it yet starts.
    fn capture_set(&mut self, places: &[Option<String>; 3], one: &str, deep: &str, span: Span) -> String {
        let mut out = String::new();
        if let Some(p) = &places[0] {
            out.push_str(&format!("{} = {}; ", p, one));
        }
        if let Some(p) = &places[1] {
            out.push_str(&format!("{} = {}; ", p, deep));
        }
        if let Some(p) = &places[2] {
            self.uses_zg = true;
            let loc = self.loc(span);
            out.push_str(&format!(
                "{{ int64_t _tn = zr_trail_take({loc}, {deep}, &_zg); for (int64_t _tk = 0; _tk < _tn; _tk++) {{ zr_trail_entry *_te = zr_trail_item(_tk); __auto_type _tf = &({p}).e[_tk]; _tf->f_function = zr_ctext(_te->site->fn); _tf->f_file = zr_ctext(_te->site->file); _tf->f_line = _te->site->line; _tf->f_code = _te->code; _tf->f_unread = _te->unread; }} ({p}).n = _tn; }} ",
                loc = loc,
                deep = deep,
                p = p
            ));
        }
        out
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
                    (true, false) if self.vs_class(&c.ret) => format!("(*({}).v)", c.code),
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
                let places = self.capture_places(capture, false);
                let (_, pre, one, deep) =
                    self.with_status_in(value, &Ty::Void, capture.catches_inside());
                let set = self.capture_set(&places, &one, &deep, value.span);
                (format!("({{ {} {} (void)0; }})", pre, set), Ty::Void)
            }
            E::StatusRead { expr: x, deep } => {
                let (_, pre, one, all) = self.with_status_in(x, &Ty::Void, *deep);
                let st = if *deep { all } else { one };
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
            // A literal used as a temporary, as for a slice parameter: its elements on the
            // scratch stack, to the end of the statement.
            E::ArrayLit(items)
                if matches!(layout(&t), Some(Layout::Vec | Layout::Slice))
                    && !self.vs_elems(&t) =>
            {
                let el = t.elem().cloned().unwrap_or(Ty::Unknown);
                let ec = self.cty(&el, e.span);
                let mut code = format!(
                    "({{ {ec} *_al = ({ec} *)zr_scratch_alloc({n} * sizeof({ec})); ",
                    ec = ec,
                    n = items.len().max(1)
                );
                for (i, x) in items.iter().enumerate() {
                    let v = self.expr(x, &el);
                    let _ = write!(code, "_al[{}] = {}; ", i, v);
                }
                let hdr = if matches!(t, Ty::Slice(_) | Ty::View(_)) {
                    "zr_slice"
                } else {
                    "zr_vec"
                };
                let _ = write!(code, "({}){{_al, {}}}; }})", hdr, items.len());
                self.used_scratch = true;
                (code, t)
            }
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
                None if self.lam_binds.contains_key(d) => (
                    self.unsupported(
                        span,
                        &format!("using `{}` as a value when a lambda that captures is passed for it", n),
                    ),
                    t,
                ),
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
                    // A variable-size field of this record, after the ones before it.
                    let this = Ty::Class(*c, self.cur_cargs.clone());
                    if self.vs_class(&this)
                        && let Some(r) = self.var_field("self", &this, &f.name, span)
                    {
                        return r;
                    }
                    let raw = format!("self->f_{}", f.name);
                    let (c, i) = (*c, *i);
                    match self.link_read(c, i, &raw, span) {
                        Some(code) => (code, ty),
                        None => (raw, ty),
                    }
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
            // The standard streams: Files for descriptors 0, 1 and 2, whose handles have
            // generation 0 (STDLIB §2.1).
            None if matches!(n, "stdin" | "stdout" | "stderr")
                && matches!(settle(&t), Ty::Class(..)) =>
            {
                let ty = settle(&t);
                let ct = self.cty(&ty, span);
                let fd = match n {
                    "stdin" => 0,
                    "stdout" => 1,
                    _ => 2,
                };
                (format!("(({}){{.f_h = {}}})", ct, fd), ty)
            }
            None => (
                self.unsupported(span, &format!("`{}` (a library name)", n)),
                t,
            ),
        }
    }

    fn member(&mut self, base: &Expr, name: &ast::Ident, span: Span, t: Ty) -> (String, Ty) {
        // The length of an array of links, whose offsets aren't read.
        if name.name == "len" && self.link_array(base).is_some() {
            let p = self.link_array_place(base);
            return (format!("((int64_t)({}).n)", p), Ty::i64());
        }
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
                    return (format!("(({}){})", enum_c(ci.constants.len()), i), t);
                }
                if let Some(i) = ci.fields.iter().position(|f| f.name == name.name) {
                    return (format!("zs{}_{}", c, i), ci.fields[i].ty.clone());
                }
            }
            return (self.unsupported(span, "this name"), t);
        }
        if let Ty::Namespace(ns) = &bt {
            if *ns == "status" {
                return (
                    format!(
                        "((uint8_t){})",
                        crate::builtins::status_code(&name.name).unwrap_or(1)
                    ),
                    Ty::u8(),
                );
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
        if let Ty::Queue(_) = deref
            && name.name == "len"
        {
            let q = if is_ref(&bty) { format!("(*{})", b) } else { b };
            return (format!("zr_q_len({})", q), Ty::i64());
        }
        let acc = if is_ref(&bty) {
            format!("({})->", b)
        } else {
            format!("({}).", b)
        };
        // A variable-size field of a record: found by walking the ones before it.
        if self.vs_class(&deref) {
            let ptr = if is_ref(&bty) {
                b.clone()
            } else {
                format!("&({})", b)
            };
            if let Some(r) = self.var_field(&ptr, &deref, &name.name, span) {
                return r;
            }
        }
        match &deref {
            Ty::Class(c, cargs) => {
                let idx = self.prog.classes[*c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name);
                match idx {
                    Some(i) => {
                        let ft = self.field_ty(*c, i, cargs);
                        let ty = if matches!(ft, Ty::Nullable(_)) && is_ref(&t) {
                            t.clone()
                        } else {
                            ft
                        };
                        let raw = format!("{}f_{}", acc, name.name);
                        match self.link_read(*c, i, &raw, span) {
                            Some(code) => (code, ty),
                            None => (raw, ty),
                        }
                    }
                    None => (self.unsupported(span, "this field"), t),
                }
            }
            // An entry's key is read-only (the checker sees to it); its value is writable.
            Ty::MapEntry(k, _) if name.name == "key" => (format!("{}k", acc), (**k).clone()),
            Ty::MapEntry(_, v) if name.name == "value" => (format!("{}v", acc), (**v).clone()),
            Ty::Map(..) | Ty::Set(_) | Ty::Pool(_) if name.name == "len" => {
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
            Ty::Array(_, k) if layout(&bty) == Some(Layout::Fixed) && name.name == "len" => {
                (format!("((int64_t){})", k.size), Ty::i64())
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
        // An element of an array of links is an offset from its slot, followed.
        if let Some((_, nullable, target)) = self.link_array(base) {
            self.link_ok.insert(key(strip_paren(base).span));
            let (slot, _) = self.index_slot(base, mode, index, span);
            let el = self.ty(base).unref().elem().cloned().unwrap_or(Ty::Unknown);
            return (self.link_follow(&slot, nullable, &target, span), el);
        }
        self.index_slot(base, mode, index, span)
    }

    /// The slot of an element of an array of links, `n.kids[i]`, as a C lvalue.
    fn link_slot(&mut self, e: &Expr) -> Option<(String, Ty)> {
        let E::Index { expr, mode, index } = &strip_paren(e).kind else {
            return None;
        };
        self.link_array(expr)?;
        self.link_ok.insert(key(strip_paren(expr).span));
        Some(self.index_slot(expr, *mode, index, e.span))
    }

    /// The element of `base` at `index`, as a C lvalue.
    fn index_slot(
        &mut self,
        base: &Expr,
        mode: ast::IndexMode,
        index: &Expr,
        span: Span,
    ) -> (String, Ty) {
        // An index held in a reference, as `src[pos]` with an `i64& pos`, is its value.
        let it = settle(self.ty(index).unref());
        let i = self.expr(index, &it);
        let i = format!("(int64_t)({})", i);
        let oob = self.on_fail(34, "zr_ix_fail(LOC, _zxi, _zxn)".to_string());
        let empty = self.on_fail(
            34,
            "zr_stop(LOC, 34, \"index into an empty array\")".to_string(),
        );
        let fix = match mode {
            // In a versioned loop's copy, shown in range before the loop (DESIGN §14.2).
            ast::IndexMode::Checked if self.unchecked_ix.contains(&key(span)) => i,
            ast::IndexMode::Checked => format!("ZR_IX({}, LEN, {})", i, oob),
            ast::IndexMode::FromEnd => format!("ZR_IX(LEN - {}, LEN, {})", i, oob),
            ast::IndexMode::Wrap => format!("ZR_IX_WRAP({}, LEN, {})", i, empty),
            ast::IndexMode::Clamp => format!("ZR_IX_CLAMP({}, LEN, {})", i, empty),
        };
        self.elem_ptr(base, &fix, span)
    }

    /// A bound of a slice, given the name of the length it counts back from.
    fn slice_bound(&mut self, b: Option<&ast::Bound>, dflt: &str, len: &str) -> String {
        match b {
            None => dflt.to_string(),
            Some(b) => {
                let t = settle(self.ty(&b.expr).unref());
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
        let bad = self.on_fail(34, format!("zr_range_fail({}, _lo, _hi, _n)", loc));
        format!(
            "({{ {} int64_t _n = {}; int64_t _lo = {}, _hi = {}; if (_lo < 0 || _hi > _n || _lo > _hi) {{ {}; }} (zr_slice){{{}, _hi - _lo}}; }})",
            ptr,
            ca.len(),
            l,
            h,
            bad,
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
                "({{ zr_slice _s = {}; int64_t _lo = {}, _hi = {}; if (_lo < 0 || _hi > _s.n || _lo > _hi) {{ {}; }} (zr_slice){{({}*)_s.p + _lo, _hi - _lo}}; }})",
                s,
                l,
                h,
                self.on_fail(34, format!("zr_range_fail({}, _lo, _hi, _s.n)", loc)),
                ec
            ),
            res,
        )
    }

    fn assign(
        &mut self,
        op: AssignOp,
        target: &Expr,
        capture: Option<&Capture>,
        value: &Expr,
        span: Span,
    ) -> String {
        if let Some(c) = capture
            && matches!(target.kind, E::Discard)
        {
            let places = self.capture_places(c, false);
            let (_, pre, one, deep) = self.with_status_in(value, &Ty::Void, c.catches_inside());
            let set = self.capture_set(&places, &one, &deep, value.span);
            return format!("({{ {} {} }})", pre, set);
        }
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
            let places = self.capture_places(c, false);
            let (val, pre, one, deep) = self.with_status_in(value, &tt, c.catches_inside());
            let (p, _) = self.place(target);
            let set = self.capture_set(&places, &one, &deep, value.span);
            return format!("({{ {} {} {} = {}; }})", pre, set, p, val);
        }
        if op == AssignOp::Assign
            && let Some(code) = self.record_field_write(target, value)
        {
            return code;
        }
        // Binding a link stores the offset to what it will refer to (REFERENCES §7.2).
        let binds = op == AssignOp::Rebind
            || (op == AssignOp::Assign && self.facts.binds.contains(&key(span)));
        if binds && let Some((raw, kind, nullable, ft)) = self.link_place(target) {
            let v = self.expr(value, &ft);
            return self.link_store(&raw, &kind, nullable, &v, span);
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
                // `=` on a slice or a writable view copies into what it views; the lengths
                // must match (DESIGN §4.10).
                let tt = settle(&self.ty(target));
                if matches!(strip_paren(target).kind, E::Slice { .. })
                    || matches!(tt, Ty::View(_))
                {
                    return self.copy_into_view(target, value, span);
                }
                let (p, pt) = self.place(target);
                if self.vs_class(&pt) {
                    return self.vs_assign(&p, value, &pt);
                }
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
                    AssignOp::CheckAdd => BinOp::CheckAdd,
                    AssignOp::CheckSub => BinOp::CheckSub,
                    AssignOp::CheckMul => BinOp::CheckMul,
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

    /// `v[i:j] = x` or `view = x`: the elements of `x` copied into what the left side
    /// views, which fails with RANGE when the lengths differ (DESIGN §4.10). The two may
    /// overlap, as `a[1:] = a[:n - 1]` does.
    fn copy_into_view(&mut self, target: &Expr, value: &Expr, span: Span) -> String {
        let tt = settle(&self.ty(target));
        let Some(el) = tt.unref().elem().cloned() else {
            return self.unsupported(span, "copying into this");
        };
        let slice = Ty::Slice(Box::new(el.clone()));
        let dst = self.expr(target, &slice);
        let src = self.expr(value, &slice);
        let sz = self.elem_size(&el, span);
        let loc = self.loc(span);
        let bad = self.on_fail(
            34,
            format!(
                "zr_stop({}, 34, \"the lengths differ, so the copy doesn't fit the view\")",
                loc
            ),
        );
        format!(
            "({{ zr_slice _d = {}, _s = {}; if (_d.n != _s.n) {{ {}; }} memmove(_d.p, _s.p, (size_t)_d.n * {}); }})",
            dst, src, bad, sz
        )
    }

    fn arith_code(&mut self, op: BinOp, a: &str, b: &str, t: &Ty, span: Span) -> String {
        let loc = self.loc(span);
        let Ty::Prim(p) = t else {
            return self.unsupported(span, "arithmetic on this type");
        };
        let c = prim_c(*p);
        if p.is_float() {
            // The checked forms, used inside the library's own code, are the plain ones:
            // floats don't overflow into a status.
            let sym = match op {
                BinOp::Add | BinOp::CheckAdd => "+",
                BinOp::Sub | BinOp::CheckSub => "-",
                BinOp::Mul | BinOp::CheckMul => "*",
                BinOp::Div => "/",
                _ => return self.unsupported(span, "this operator on floats"),
            };
            return format!("(({}){} {} ({}){})", c, a, sym, c, b);
        }
        let u = unsigned_of(*p);
        let (lo, hi) = min_max(*p);
        // `+`, `-` and `*` wrap; `+#`, `-#` and `*#` fail with RANGE on overflow; `/` and
        // `%` fail with DOMAIN on a zero divisor (DESIGN §9.2).
        let over = if matches!(op, BinOp::CheckAdd | BinOp::CheckSub | BinOp::CheckMul) {
            self.on_fail(34, format!("zr_stop({}, 34, \"integer overflow\")", loc))
        } else {
            String::new()
        };
        let zero = if matches!(op, BinOp::Div | BinOp::Rem) {
            self.on_fail(33, format!("zr_stop({}, 33, \"division by zero\")", loc))
        } else {
            String::new()
        };
        match op {
            BinOp::Add => format!("(({})(({}){} + ({}){}))", c, u, a, u, b),
            BinOp::Sub => format!("(({})(({}){} - ({}){}))", c, u, a, u, b),
            BinOp::Mul => format!("(({})(({}){} * ({}){}))", c, u, a, u, b),
            BinOp::Div => format!("ZR_DIV({}, {}, {}, {}, {})", c, u, a, b, zero),
            BinOp::Rem => format!("ZR_REM({}, {}, {}, {})", c, a, b, zero),
            BinOp::CheckAdd => format!("ZR_ADD({}, {}, {}, {})", c, a, b, over),
            BinOp::CheckSub => format!("ZR_SUB({}, {}, {}, {})", c, a, b, over),
            BinOp::CheckMul => format!("ZR_MUL({}, {}, {}, {})", c, a, b, over),
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
                    settle(self.ty(l).unref())
                } else {
                    t.clone()
                };
                let a = self.expr(l, &rt);
                let bt = if matches!(op, Shl | Shr) {
                    settle(self.ty(r).unref())
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
        // An unsuffixed literal takes the other side's integer type, as in arithmetic:
        // `span <= 18446744073709551615` with a u128 `span`.
        let (lu, ru) = match (lt.unref(), rt.unref()) {
            (Ty::IntLit { .. }, Ty::Prim(p)) if p.is_int() => (Ty::Prim(*p), Ty::Prim(*p)),
            (Ty::Prim(p), Ty::IntLit { .. }) if p.is_int() => (Ty::Prim(*p), Ty::Prim(*p)),
            _ => (settle(lt.unref()), settle(rt.unref())),
        };
        // Handles are equal when they name the same slot in the same generation.
        if matches!(lu, Ty::Handle(_)) {
            let a = self.expr(l, &lu);
            let b = self.expr(r, &ru);
            let eq = format!(
                "({{ zr_handle _ha = {}, _hb = {}; _ha.i == _hb.i && _ha.g == _hb.g; }})",
                a, b
            );
            return if op == BinOp::Ne {
                format!("(!{})", eq)
            } else {
                eq
            };
        }
        let textish = |t: &Ty| matches!(t, Ty::Slice(_) | Ty::View(_) | Ty::Array(..));
        if textish(&lu) && textish(&ru) {
            let el = lu.elem().cloned().unwrap_or(Ty::u8());
            let a = self.expr(l, &Ty::Slice(Box::new(el.clone())));
            let b = self.expr(r, &Ty::Slice(Box::new(el.clone())));
            // Elements whose bytes aren't their value, as text views and floats, are
            // compared one by one with `==`.
            if matches!(op, BinOp::Eq | BinOp::Ne) && !self.bytes_are_value(&el) {
                let ec = self.cty(&el, span);
                let Some(each) = self.elem_eq("_xa", "_xb", &el, span) else {
                    return self.unsupported(
                        span,
                        &format!("comparing arrays of {}", self.prog.show(&el)),
                    );
                };
                let eq = format!(
                    "({{ zr_slice _ea = {}, _eb = {}; bool _r = _ea.n == _eb.n; for (int64_t _i = 0; _r && _i < _ea.n; _i++) {{ {ec} _xa = (({ec} *)_ea.p)[_i], _xb = (({ec} *)_eb.p)[_i]; _r = {}; }} _r; }})",
                    a,
                    b,
                    each,
                    ec = ec
                );
                return if op == BinOp::Ne {
                    format!("(!{})", eq)
                } else {
                    eq
                };
            }
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

    /// Whether two values of this type are equal exactly when their bytes are: integers,
    /// `bool` and enums. Not floats (`-0.0 == 0.0`, and a NaN equals nothing), views (equal
    /// text in two places) or classes (padding).
    fn bytes_are_value(&self, t: &Ty) -> bool {
        match t {
            Ty::Prim(p) => !p.is_float(),
            Ty::Class(c, _) => self.prog.classes[*c].kind == ClassKind::Enum,
            _ => false,
        }
    }

    /// `==` on two elements of type `el`, held in the C variables `a` and `b`, or None when
    /// that comparison isn't generated.
    fn elem_eq(&mut self, a: &str, b: &str, el: &Ty, span: Span) -> Option<String> {
        match el {
            t if self.bytes_are_value(t) => Some(format!("{} == {}", a, b)),
            Ty::Prim(p) if p.is_float() => Some(format!("{} == {}", a, b)),
            Ty::Slice(e) | Ty::View(e) => {
                let e = (**e).clone();
                if self.bytes_are_value(&e) {
                    let sz = self.elem_size(&e, span);
                    Some(format!("zr_eq({}, {}, {})", a, b, sz))
                } else {
                    None
                }
            }
            _ => None,
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
                    // Negation wraps, as `0 - x` does: `-i64.MIN` is `i64.MIN`.
                    Ty::Prim(p) => (
                        format!("(({})(({})0 - ({}){}))", prim_c(*p), unsigned_of(*p), unsigned_of(*p), v),
                        t,
                    ),
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
        let mut from = settle(self.ty(x).unref());
        // A literal too big for i64, as in `(i128) -100000000000000000000000`, is converted
        // from the 128-bit type that holds it.
        let lit = match &strip_paren(x).kind {
            E::Int {
                value,
                suffix: None,
            } => Some(*value as i128),
            E::Unary {
                op: UnaryOp::Neg,
                expr,
            } => match &strip_paren(expr).kind {
                E::Int {
                    value,
                    suffix: None,
                } => Some((*value as i128).wrapping_neg()),
                _ => None,
            },
            _ => None,
        };
        if let Some(n) = lit
            && (n > i64::MAX as i128 || n < i64::MIN as i128)
        {
            from = Ty::Prim(if n < 0 { Prim::I128 } else { Prim::U128 });
        }
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
                    "({{ double _v = (double)({}); if (!(_v > (double){} - 1.0 && _v < (double){} + 1.0)) {{ {}; }} ({})_v; }})",
                    v,
                    lo,
                    hi,
                    self.on_fail(
                        34,
                        format!(
                            "zr_stop({}, 34, \"the value doesn't fit in {}\")",
                            loc,
                            to.name()
                        )
                    ),
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
                        "({{ {st} _v = ({st})({}); if (_v < ({st}){} || (_v > 0 && ({ut})_v > ({ut}){})) {{ {}; }} ({})_v; }})",
                        v,
                        lo,
                        hi,
                        self.on_fail(
                            34,
                            format!(
                                "zr_stop({}, 34, \"the value doesn't fit in {}\")",
                                loc,
                                to.name()
                            )
                        ),
                        tc,
                        st = st,
                        ut = ut
                    )
                } else {
                    format!(
                        "({{ {ut} _v = ({ut})({}); if (_v > ({ut}){}) {{ {}; }} ({})_v; }})",
                        v,
                        hi,
                        self.on_fail(
                            34,
                            format!(
                                "zr_stop({}, 34, \"the value doesn't fit in {}\")",
                                loc,
                                to.name()
                            )
                        ),
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
        let st = switch_ty(settle(self.ty(&sw.scrutinee).unref()));
        let v = self.expr(&sw.scrutinee, &st);
        let t = self.tmp("sw");
        let sct = self.cty(&st, sw.span);
        self.line(&format!("{} {} = {};", sct, t, v));
        // In the copy of a function's base case that ends with this switch, a case that
        // calls something calls the whole function instead (DESIGN §14.2).
        let whole = match &self.base_switch {
            Some((s, _)) if *s == key(sw.span) => self.base_switch.take().map(|(_, call)| call),
            _ => None,
        };
        let arm = |me: &mut Self, body: &SwitchBody| match (&whole, body) {
            (Some(_), SwitchBody::Expr(x)) if pure_expr(x, &mut 0) => {
                me.switch_arm(body, &r, &end, want)
            }
            (Some(call), _) => me.line(&format!("{} = {}; goto {};", r, call, end)),
            (None, _) => me.switch_arm(body, &r, &end, want),
        };
        let mut default = None;
        for rule in &sw.rules {
            let Some(labels) = &rule.labels else {
                default = Some(rule);
                continue;
            };
            let conds: Vec<String> = labels.iter().map(|l| self.case_test(&t, &st, l)).collect();
            self.line(&format!("if ({}) {{", conds.join(" || ")));
            arm(self, &rule.body);
            self.line("}");
        }
        if let Some(d) = default {
            arm(self, &d.body);
        } else {
            let loc = self.loc(sw.span);
            let none = self.on_fail(
                22,
                format!("zr_stop({}, 22, \"no switch case matched\")", loc),
            );
            self.line(&format!("{};", none));
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
                        || matches!(name.name.as_str(), "toUpper" | "toLower" | "hex")
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
                        let (f, cast) = if p == Prim::F32 {
                            ("zr_w_f32_pad", "float")
                        } else if p.is_float() {
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
                    // Bytes as hex (STDLIB §8.4): two lowercase digits each.
                    ("hex", None) => {
                        let s = self.conv(&v, &bt, &Ty::string(), e.span);
                        let _ = write!(
                            writes,
                            "{{ zr_slice _s = {}; for (int64_t _j = 0; _j < _s.n; _j++) {{ uint8_t _c = ((uint8_t*)_s.p)[_j]; zr_w_byte({k}, (uint8_t)\"0123456789abcdef\"[_c >> 4]); zr_w_byte({k}, (uint8_t)\"0123456789abcdef\"[_c & 15]); }} }} ",
                            s,
                            k = k
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
            Ty::Prim(Prim::F32) => {
                let _ = write!(writes, "zr_w_f32({}, {}); ", k, v);
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
                    Ty::Prim(Prim::F32) => format!("zr_w_f32({}, _e)", k),
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

    /// A call whose status nothing reads stops the program when it fails (DESIGN §7.6):
    /// the check is wrapped around it, keeping its result's type.
    fn call_any(&mut self, e: &Expr) -> CallOut {
        let c = self.call_any_inner(e);
        self.check_unread(c, e.span)
    }

    fn check_unread(&mut self, mut c: CallOut, span: Span) -> CallOut {
        let Some(name) = self
            .facts
            .unchecked
            .get(&key(span))
            .or_else(|| self.extra_unchecked.get(&key(span)))
            .cloned()
        else {
            return c;
        };
        if !c.fails {
            return c;
        }
        if let Some(k) = &mut self.catching {
            k.used = true;
            let (cs, cl) = (k.cs.clone(), k.cl.clone());
            let u = self.tmp("u");
            let st = if c.ret == Ty::Void {
                u.clone()
            } else {
                format!("{}.s", u)
            };
            c.code = format!(
                "({{ __auto_type {u} = {c}; if ({st}) {{ {cs} = {st}; goto {cl}; }} {u}; }})",
                u = u,
                c = c.code,
                st = st,
                cs = cs,
                cl = cl
            );
            return c;
        }
        let u = self.tmp("u");
        let st = if c.ret == Ty::Void {
            u.clone()
        } else {
            format!("{}.s", u)
        };
        c.code = format!(
            "({{ __auto_type {u} = {c}; if ({st}) zr_unread({loc}, {st}, {name}); {u}; }})",
            c = c.code,
            u = u,
            st = st,
            loc = self.loc(span),
            name = c_string(name.as_bytes()),
        );
        c
    }

    fn call_any_inner(&mut self, e: &Expr) -> CallOut {
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
            E::ThisNew(args) if self.rec_ctor.is_some() => {
                let code = self.record_this_new(args, e.span);
                CallOut {
                    code,
                    ret: Ty::Void,
                    fails: true,
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
        let direct = self.direct_result.take();
        let ret = settle(&self.ty(e));
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
                        return self.iface_call(base, c, i, args, &params, direct, e.span);
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
                let (mut fname, map) = self.method_ref(c, i, recv.as_ref(), &targs, e.span);
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
                // C fills only the array it was declared with: an array on a data stack.
                if m.c_name.is_some()
                    && let Some((k, t)) = self.layout_needs(args, rest, 0).first().cloned()
                {
                    let (given, takes) = (
                        self.prog.show(t.unref()),
                        self.prog.show(settle(&rest[k]).unref()),
                    );
                    let what = format!(
                        "passing a `{}` to `{}`, whose C takes a `{}`",
                        given, m.name, takes
                    );
                    let lib = (
                        format!(
                            "this call can't take a `{}` yet: the library's C fills only a `{}`",
                            given, takes
                        ),
                        format!(
                            "pass a `{}` and copy what you need from it (DESIGN §4.3)",
                            takes
                        ),
                    );
                    let u = self.layout_unsupported(args[k].expr.span, &what, Some(lib));
                    {
                        return CallOut {
                            code: u,
                            ret,
                            fails: false,
                        };
                    }
                }
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
                    // A lambda that captures goes to a copy of the function made for it
                    // (DESIGN §9.3), as does one this copy was given, passed on. An array
                    // of another layout than its parameter's goes to a copy for that
                    // layout (DESIGN §4.3).
                    let shift = usize::from(recv_arg);
                    let over = self.layout_needs(args, rest, shift);
                    let mut bound = Vec::new();
                    for (k, (a, p)) in args.iter().zip(rest).enumerate() {
                        if matches!(settle(p), Ty::Func(..))
                            && let Some(b) = self.bound_arg(&a.expr)
                        {
                            bound.push((k + shift, b));
                            continue;
                        }
                        let p = over
                            .iter()
                            .find(|(j, _)| *j == k + shift)
                            .map_or(p, |(_, t)| t);
                        self.borrow_arg = true;
                        cargs.push(self.arg_code(&a.expr, p));
                        self.borrow_arg = false;
                    }
                    if !bound.is_empty() && !over.is_empty() {
                        let u = self.unsupported(
                            e.span,
                            "a call passing both a lambda that captures and an array of another layout than its parameter's",
                        );
                        return CallOut {
                            code: u,
                            ret,
                            fails: false,
                        };
                    }
                    if !bound.is_empty() {
                        let (name, extra) = self.bound_copy(c, i, &fname, &map, &bound, e.span);
                        fname = name;
                        cargs.extend(extra);
                    }
                    if !over.is_empty() {
                        fname = self.layout_copy(c, i, &fname, &map, over, e.span);
                    }
                }
                // A call to itself copies its base case first (DESIGN §14.2).
                if self.base_name.as_deref() == Some(fname.as_str()) {
                    self.base_used = true;
                    fname.push_str("_base");
                }
                let fname = runtime_c.map_or(fname, str::to_string);
                let r = if map.is_empty() {
                    self.sb(&m.ret)
                } else {
                    m.ret.subst(&map)
                };
                let r = settle(&r);
                // Where to build an array result (DESIGN §3.3).
                let into = m.c_name.is_none() && self.returns_into(&r);
                if into {
                    cargs.insert(0, direct.clone().unwrap_or_else(|| "NULL".into()));
                }
                let mut code = format!("{}({})", fname, cargs.join(", "));
                if into && self.vs_class(&r) {
                    // A pointer to the record, read as the record itself.
                    if !fails {
                        code = format!("(*({}))", code);
                    }
                } else if into {
                    code = self.array_result(code, &r, fails, &direct, e.span);
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
            // Every function value can fail (DESIGN §9.3).
            Origin::Value => {
                // A parameter bound to a lambda: the lambda, inlined.
                if let Some(b) = self.bound_of(callee) {
                    let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                    let cargs: Vec<String> = args
                        .iter()
                        .zip(&params)
                        .map(|(a, p)| self.expr(&a.expr, p))
                        .collect();
                    let code = self.call_bound(&b, &cargs, &ret, e.span);
                    return CallOut {
                        code,
                        ret,
                        fails: true,
                    };
                }
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
                    fails: true,
                }
            }
            Origin::Builtin => {
                let name = fact.name.clone();
                let params: Vec<Ty> = fact.params.iter().map(|t| self.sb(t)).collect();
                self.builtin_into = direct;
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

    /// A call of a built-in, which a lambda passed to it fails as (DESIGN §9.3).
    fn builtin(
        &mut self,
        e: &Expr,
        callee: &Expr,
        name: &str,
        args: &[ast::Arg],
        params: &[Ty],
        ret: Ty,
    ) -> CallOut {
        self.call_names.push((name.to_string(), e.span));
        // Taken first, so the calls in the receiver and the arguments don't see it.
        let into = self.builtin_into.take();
        let c = self.builtin_inner(e, callee, name, args, params, ret, into);
        self.call_names.pop();
        c
    }

    #[allow(clippy::too_many_arguments)]
    fn builtin_inner(
        &mut self,
        e: &Expr,
        callee: &Expr,
        name: &str,
        args: &[ast::Arg],
        params: &[Ty],
        ret: Ty,
        into: Option<String>,
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
        // `thread.spawn(() => ...)` (STDLIB §10.3).
        if let E::Member {
            expr: recv,
            name: m,
        } = &callee.kind
            && m.name == "spawn"
            && matches!(self.ty(recv), Ty::TypeName(_))
            && self.threads
        {
            return self.spawn_code(&args[0].expr, ret, span);
        }
        match &callee.kind {
            // print, println, errprint, errprintln, min, max, abs, clamp.
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
                        // With threads, one thread's line is written whole (STDLIB §10.3);
                        // its parts are worked out first, so nothing waits holding the lock.
                        if self.threads {
                            return ok(
                                format!(
                                    "({{ {} ZR_OUT_LOCK(); {} {} ZR_OUT_UNLOCK(); }})",
                                    pre, writes, nl
                                ),
                                Ty::Void,
                            );
                        }
                        ok(format!("({{ {} {} {} }})", pre, writes, nl), Ty::Void)
                    }
                    "min" | "max" | "clamp" | "abs" => {
                        let t = params.first().cloned().unwrap_or(ret.clone());
                        let vs: Vec<String> = (0..args.len()).map(|k| arg(self, k)).collect();
                        let ct = self.cty(&t, span);
                        // In their natural order, so a NaN counts as the largest float
                        // (STDLIB §2.4).
                        let code = match f.as_str() {
                            "min" => {
                                let less = self.natural_less("_b", "_a", &t, span);
                                format!(
                                    "({{ {} _a = {}, _b = {}; {} ? _b : _a; }})",
                                    ct, vs[0], vs[1], less
                                )
                            }
                            "max" => {
                                let less = self.natural_less("_a", "_b", &t, span);
                                format!(
                                    "({{ {} _a = {}, _b = {}; {} ? _b : _a; }})",
                                    ct, vs[0], vs[1], less
                                )
                            }
                            // INVALID, and 0, when the range is empty.
                            "clamp" => {
                                let rt = self.result_ty(&t, span);
                                format!(
                                    "({{ {ct} _x = {}, _lo = {}, _hi = {}; {rt} _r = {{0, 0}}; if (_hi < _lo) _r.s = 22; else _r.v = _x < _lo ? _lo : _hi < _x ? _hi : _x; _r; }})",
                                    vs[0],
                                    vs[1],
                                    vs[2],
                                    ct = ct,
                                    rt = rt
                                )
                            }
                            _ => {
                                // abs(MIN) has no positive, so it stops (STDLIB §2.4).
                                let neg = self.arith_code(BinOp::CheckSub, "0", "_a", &t, span);
                                format!("({{ {} _a = {}; _a < 0 ? {} : _a; }})", ct, vs[0], neg)
                            }
                        };
                        if !matches!(t, Ty::Prim(_)) && f != "min" && f != "max" {
                            let u = self.unsupported(span, "this function on non-numbers");
                            return ok(u, ret);
                        }
                        if f == "clamp" {
                            return fail(code, t);
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
                return ok("zr_errno_status(errno)".into(), Ty::u8());
            }
            if *ns == "status" && name == "message" {
                return ok(format!("zr_status_message({})", arg(self, 0)), Ty::string());
            }
            if *ns == "status" && name == "name" {
                return ok(format!("zr_status_name_text({})", arg(self, 0)), Ty::string());
            }
            if *ns == "math" {
                let fname = match name {
                    // Half away from zero; RANGE for NaN, infinities and what i64 can't hold.
                    "roundToI64" => {
                        let x = arg(self, 0);
                        return CallOut {
                            code: format!(
                                "({{ double _x = round((double)({})); zr_res_i64 _r = {{0, 0}}; if (_x >= -9223372036854775808.0 && _x < 9223372036854775808.0) _r.v = (int64_t)_x; else _r.s = 34; _r; }})",
                                x
                            ),
                            ret: Ty::i64(),
                            fails: true,
                        };
                    }
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
                // Bit operations (STDLIB §8.1), on the value's bits as unsigned.
                "leadingZeros" | "trailingZeros" => {
                    let w = bits(p);
                    let u = unsigned_of(p);
                    let code = match (m, w) {
                        ("leadingZeros", 128) => format!(
                            "({{ zr_u128 _x = (zr_u128)({}); uint64_t _h = (uint64_t)(_x >> 64), _l = (uint64_t)_x; (int64_t)(_h ? __builtin_clzll(_h) : _l ? 64 + __builtin_clzll(_l) : 128); }})",
                            v
                        ),
                        ("leadingZeros", _) => format!(
                            "({{ {u} _x = ({u})({v}); (int64_t)(_x ? __builtin_clzll((uint64_t)_x) - {d} : {w}); }})",
                            u = u,
                            v = v,
                            d = 64 - w,
                            w = w
                        ),
                        (_, 128) => format!(
                            "({{ zr_u128 _x = (zr_u128)({}); uint64_t _h = (uint64_t)(_x >> 64), _l = (uint64_t)_x; (int64_t)(_l ? __builtin_ctzll(_l) : _h ? 64 + __builtin_ctzll(_h) : 128); }})",
                            v
                        ),
                        _ => format!(
                            "({{ {u} _x = ({u})({v}); (int64_t)(_x ? __builtin_ctzll((uint64_t)_x) : {w}); }})",
                            u = u,
                            v = v,
                            w = w
                        ),
                    };
                    ok(code, Ty::i64())
                }
                "rotateLeft" | "rotateRight" => {
                    let w = bits(p);
                    let u = unsigned_of(p);
                    let n = arg(self, 0);
                    let (a, b) = if m == "rotateLeft" {
                        ("<<", ">>")
                    } else {
                        (">>", "<<")
                    };
                    ok(
                        format!(
                            "({{ {u} _x = ({u})({v}); unsigned _n = (unsigned)((((int64_t)({n})) % {w} + {w}) % {w}); ({t})(_n ? ({u})((_x {a} _n) | (_x {b} ({w} - _n))) : _x); }})",
                            u = u,
                            v = v,
                            n = n,
                            w = w,
                            a = a,
                            b = b,
                            t = prim_c(p)
                        ),
                        Ty::Prim(p),
                    )
                }
                "reverseBytes" => {
                    let code = match bits(p) {
                        8 => format!("(({})({}))", prim_c(p), v),
                        16 => format!("(({})__builtin_bswap16((uint16_t)({})))", prim_c(p), v),
                        32 => format!("(({})__builtin_bswap32((uint32_t)({})))", prim_c(p), v),
                        64 => format!("(({})__builtin_bswap64((uint64_t)({})))", prim_c(p), v),
                        _ => format!(
                            "({{ zr_u128 _x = (zr_u128)({}); ({})(((zr_u128)__builtin_bswap64((uint64_t)_x) << 64) | __builtin_bswap64((uint64_t)(_x >> 64))); }})",
                            v,
                            prim_c(p)
                        ),
                    };
                    ok(code, Ty::Prim(p))
                }
                "isPowerOf2" => {
                    let u = unsigned_of(p);
                    ok(
                        format!(
                            "({{ {t} _v = ({v}); {u} _x = ({u})_v; (bool)(_v > 0 && !(_x & (_x - 1))); }})",
                            t = prim_c(p),
                            v = v,
                            u = u
                        ),
                        Ty::bool(),
                    )
                }
                _ => ok(self.unsupported(span, &format!("`{}` on numbers", m)), ret),
            };
        }
        if matches!(recv_t, Ty::Map(..) | Ty::Set(_)) {
            return self.map_method(base, m, args, ret, span);
        }
        if matches!(recv_t, Ty::Pool(_)) {
            return self.pool_method(base, m, args, ret, span);
        }
        if matches!(recv_t, Ty::Queue(_)) {
            return self.queue_method(base, m, args, ret, span);
        }
        if self.vs_elems(&recv_t) {
            return self.rvec_method(base, m, args, ret, span);
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
                    "isValidUtf8" => ok(format!("zr_valid_utf8({})", s), Ty::bool()),
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
        // Emptying an array of links reads none of its offsets.
        if m == "clear" && self.link_array(base).is_some() {
            self.link_ok.insert(key(strip_paren(base).span));
        }
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
            "push" | "pushFront" | "insert" => {
                let at = match m {
                    "push" => Where::End,
                    "pushFront" => Where::Front,
                    _ => Where::Cursor,
                };
                if let Some(build) = self.build_linked(&arr, &el, &args[0].expr, at, span) {
                    return CallOut {
                        code: format!("({{ {} {} *_e; {} _e; }})", ptr, ec, build),
                        ret: Ty::Ref(Box::new(el.clone())),
                        fails: false,
                    };
                }
                let x = arg(self, 0);
                put(self, at, x)
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
                        "({{ {} int64_t _i = ZR_IX({}, {}, {}), _j = ZR_IX({}, {}, {}); {} }})",
                        ptr,
                        i,
                        len,
                        self.on_fail(34, format!("zr_ix_fail({}, _zxi, _zxn)", loc)),
                        j,
                        len,
                        self.on_fail(34, format!("zr_ix_fail({}, _zxi, _zxn)", loc)),
                        body
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
                // A sum that overflows stops the program (STDLIB §7.1).
                let add = self.arith_code(BinOp::CheckAdd, "_t", &arr.at("_i"), &el, span);
                ok(
                    format!(
                        "({{ {} {} _t = 0; for (int64_t _i = 0; _i < {}; _i++) _t = {}; _t; }})",
                        ptr, ec, len, add
                    ),
                    el,
                )
            }
            "min" | "max" if matches!(el, Ty::Prim(_)) || el.is_string() => {
                let rt = res(self);
                // In their natural order, which puts a NaN last (STDLIB §7.1).
                let at = arr.at("_i");
                let less = if m == "min" {
                    self.natural_less(&at, "_r.v", &el, span)
                } else {
                    self.natural_less("_r.v", &at, &el, span)
                };
                fail(
                    format!(
                        "({{ {} {} _r = {{0, 192}}; if ({}) {{ _r.v = {}; _r.s = 0; for (int64_t _i = 1; _i < {}; _i++) if ({}) _r.v = {}; }} _r; }})",
                        ptr,
                        rt,
                        len,
                        arr.at("0"),
                        len,
                        less,
                        at
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
                        crate::builtins::status_code("MISSING").unwrap_or(195),
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
            // A new array, built where the result goes, as a function's is (DESIGN §3.3):
            // in a new variable's storage, or on a data stack of its own, copied to the
            // scratch stack when it's a temporary.
            "filter" | "map" | "sortedIndexes" | "sortedIndexesBy" => {
                let Some(rel) = ret.elem().cloned() else {
                    return ok(self.unsupported(span, &format!("`{}` here", m)), ret);
                };
                if !self.plain_elem(&rel) || !self.plain_elem(&el) {
                    let what = format!("`{}` on an array of {}", m, self.prog.show(if self.plain_elem(&el) { &rel } else { &el }));
                    return ok(self.unsupported(span, &what), ret);
                }
                let rc = self.cty(&rel, span);
                let site = self.site();
                let at = match &into {
                    Some(p) => format!("((void *)({p}) ? (void *)({p}) : zr_stack({}))", site, p = p),
                    None => format!("zr_stack({})", site),
                };
                let r = self.tmp("nr");
                let body = match m {
                    "filter" => {
                        let keep = self.apply(&args[0].expr, &[arr.at("_i")]);
                        format!(
                            "for (int64_t _i = 0; _i < {len}; _i++) if ({keep}) (({rc}*){r}.p)[{r}.n++] = {x};",
                            len = len,
                            keep = keep,
                            rc = rc,
                            r = r,
                            x = arr.at("_i")
                        )
                    }
                    "map" => {
                        let v = self.apply(&args[0].expr, &[arr.at("_i")]);
                        format!(
                            "for (int64_t _i = 0; _i < {len}; _i++) {{ {rc} _mv = {v}; (({rc}*){r}.p)[{r}.n++] = _mv; }}",
                            len = len,
                            v = v,
                            rc = rc,
                            r = r
                        )
                    }
                    // A stable merge sort of the indexes (STDLIB §7.1).
                    _ => {
                        let order = match args.first() {
                            None => Order::Natural,
                            Some(f) if m == "sortedIndexesBy" => {
                                let (_, kt) = self.fn_sig(&f.expr);
                                Order::Key(&f.expr, kt)
                            }
                            Some(f) => Order::Less(&f.expr),
                        };
                        let get = |x: &str| arr.at(x);
                        let sort = self.merge_sort_by("int64_t", &get, "_n", &order, false, &el, span);
                        format!(
                            "{{ int64_t _n = {len}, *_d = (int64_t *){r}.p; for (int64_t _i = 0; _i < _n; _i++) _d[_i] = _i; {r}.n = _n; {sort} }}",
                            len = len,
                            r = r,
                            sort = sort
                        )
                    }
                };
                let code = format!(
                    "({{ {ptr} zr_vec {r} = {{{at}, 0}}; {body} {r}; }})",
                    ptr = ptr,
                    r = r,
                    at = at,
                    body = body
                );
                let code = self.array_result(code, &ret, false, &into, span);
                ok(code, ret)
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
            // All of a slice's elements at the end (STDLIB §7.1): one copy on a data stack,
            // all or nothing on a capped array, which fails with FULL without room for
            // them all.
            "pushAll" => {
                let src = arg(self, 0);
                if lay == Layout::Vec {
                    return ok(
                        format!(
                            "({{ {ptr} zr_slice _s = {src}; memmove(({ec}*){a}->p + {a}->n, _s.p, (size_t)_s.n * sizeof({ec})); {a}->n += _s.n; }})",
                            ptr = ptr,
                            src = src,
                            ec = ec,
                            a = a
                        ),
                        Ty::Void,
                    );
                }
                let put = arr.put(Where::End);
                let each = format!(
                    "for (int64_t _i = 0; _i < _s.n; _i++) {{ {ec} _x = (({ec}*)_s.p)[_i]; {ec} *_e; {put} (void)_e; }}",
                    ec = ec,
                    put = put
                );
                if lay.capped() {
                    return fail(
                        format!(
                            "({{ {} zr_slice _s = {}; uint8_t _st = 0; if ({} + _s.n > {}) _st = 193; else {{ {} }} _st; }})",
                            ptr, src, len, cap, each
                        ),
                        Ty::Void,
                    );
                }
                ok(
                    format!("({{ {} zr_slice _s = {}; uint8_t _st = 0; {} (void)_st; }})", ptr, src, each),
                    Ty::Void,
                )
            }
            // On a capped array, all of the text or none of it (STDLIB §3.6): it's built at
            // the top of the scratch stack, after its parts are worked out, then copied in,
            // or FULL without room for it.
            "append" if is_u8 && lay.capped() => {
                let k = "zr_into(&_t)";
                let (pre, writes) = if self.is_text(&args[0].expr) {
                    self.text(&args[0].expr, k)
                } else {
                    let s = arg(self, 0);
                    (
                        format!("zr_slice _s0 = {};", s),
                        format!("zr_w_str({}, _s0); ", k),
                    )
                };
                let put = arr.put(Where::End);
                fail(
                    format!(
                        "({{ {ptr} {pre} zr_sm(); zr_vec _t = {{zr_scratch_top, 0}}; {writes} uint8_t _st = 0; if ({len} + _t.n > {cap}) _st = 193; else {{ for (int64_t _i = 0; _i < _t.n; _i++) {{ uint8_t _x = ((uint8_t *)_t.p)[_i]; uint8_t *_e; {put} (void)_e; }} }} _st; }})",
                        ptr = ptr,
                        pre = pre,
                        writes = writes,
                        len = len,
                        cap = cap,
                        put = put
                    ),
                    Ty::Void,
                )
            }
            "append" if is_u8 => {
                if matches!(lay, Layout::Cur | Layout::Chk) {
                    return ok(
                        self.unsupported(span, "`append` to a cursor array"),
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
            let p = if self.link_array(lhs).is_some() {
                self.link_array_place(lhs)
            } else {
                self.place(lhs).0
            };
            stmts.push_str(&format!("__typeof__({}) *_arr = &({}); ", p, p));
        }
        // The array the chain inserts into, which may hold links.
        let mut root = lhs.as_ref();
        while let E::Binary {
            op: BinOp::InsertEnd | BinOp::InsertFront | BinOp::InsertCursor,
            lhs,
            ..
        } = &strip_paren(root).kind
        {
            root = lhs;
        }
        let link = self.link_array(root);
        let arr_t = self.ty(lhs).unref().clone();
        if let Ty::Array(el, _) = &arr_t
            && self.vs(el)
        {
            let el = (**el).clone();
            let push = self.rvec_push("(*_arr)", rhs, &el, e.span);
            stmts.push_str(&format!("(void){}; ", push));
            return;
        }
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
        let arr = Arr {
            a: "_arr".into(),
            lay,
            ec: ec.clone(),
            cap,
        };
        if let Some(build) = self.build_linked(&arr, &el, rhs, at, e.span) {
            stmts.push_str(&format!("{{ {} *_e; {} (void)_e; }} ", ec, build));
            return;
        }
        // A link is stored as its offset from the slot it goes in (REFERENCES §7.2).
        if let Some((w, nullable, _)) = link {
            if lay != Layout::Capped || at != Where::End {
                let u = self.unsupported(e.span, "this insertion into an array of links");
                stmts.push_str(&format!("(void){}; ", u));
                return;
            }
            let x = self.expr(rhs, &el);
            let set = self.link_store("_arr->e[_arr->n]", &LinkKind::Rel(w), nullable, "_x", rhs.span);
            stmts.push_str(&format!(
                "{{ {} _x = {}; if (_arr->n < {}) {{ {}; _arr->n++; }} else if (!_st) _st = 193; }} ",
                ec, x, cap, set
            ));
            return;
        }
        let x = self.expr(rhs, &el);
        stmts.push_str(&format!(
            "{{ {} _x = {}; {} *_e; {} (void)_e; }} ",
            ec,
            x,
            ec,
            arr.put(at)
        ));
    }

    /// What field `i` of class `c` is, when it is a link (REFERENCES §7.2, §7.3).
    pub(super) fn link_kind(&self, c: ClassId, i: usize) -> Option<LinkKind> {
        let f = &self.prog.classes[c].fields[i];
        if let Some(w) = f.link {
            return Some(LinkKind::Rel(w));
        }
        let (w, _, _) = f.pool_link.as_ref()?;
        let (k, j) = self.facts.pool_arrays.get(&(c, i))?;
        Some(LinkKind::Pool(*w, format!("zs{}_{}.p", k, j)))
    }

    /// Field `name` of class `c`, and whether it's nullable, when it is a link.
    fn field_link(&self, c: ClassId, name: &str) -> Option<(LinkKind, bool)> {
        let i = self.prog.classes[c].fields.iter().position(|f| f.name == name)?;
        let kind = self.link_kind(c, i)?;
        let nullable = matches!(self.prog.classes[c].fields[i].ty, Ty::Nullable(_));
        Some((kind, nullable))
    }

    /// Following field `i` of class `c`, the C lvalue `raw`, when it is a link: the
    /// reference it holds.
    fn link_read(&mut self, c: ClassId, i: usize, raw: &str, span: Span) -> Option<String> {
        // An array of links is offsets, which only the operations that convert them read.
        if self.prog.classes[c].fields[i].elem_link.is_some() {
            if self.link_ok.remove(&key(span)) {
                return Some(raw.to_string());
            }
            return Some(self.unsupported(
                span,
                "this use of an array of links (so far it takes `+>`, `[i]`, `for`, `.len` and `clear`)",
            ));
        }
        let kind = self.link_kind(c, i)?;
        let f = &self.prog.classes[c].fields[i];
        let (target, nullable) = match &f.ty {
            Ty::Nullable(x) => (x.unref().clone(), true),
            t => (t.unref().clone(), false),
        };
        let tc = self.cty(&target, span);
        Some(match (kind, nullable) {
            (LinkKind::Rel(_), true) => format!("ZR_LINK_OR_NULL({}, {})", tc, raw),
            (LinkKind::Rel(_), false) => format!("ZR_LINK({}, {})", tc, raw),
            (LinkKind::Pool(_, base), true) => {
                format!("ZR_PLINK_OR_NULL({}, {}, {})", tc, raw, base)
            }
            (LinkKind::Pool(_, base), false) => format!("ZR_PLINK({}, {}, {})", tc, raw, base),
        })
    }

    /// When `e` is a field holding an array of self-relative links (REFERENCES §7.2):
    /// the width of their offsets, whether they are nullable, and what they refer to.
    fn link_array(&self, e: &Expr) -> Option<(Prim, bool, Ty)> {
        let e = strip_paren(e);
        let (c, i) = match &e.kind {
            E::Member { expr: base, name } => {
                let Ty::Class(c, _) = settle(self.ty(base).unref()) else {
                    return None;
                };
                let i = self.prog.classes[c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name)?;
                (c, i)
            }
            E::Ident(_) => match self.facts.names.get(&key(e.span)) {
                Some(NameRes::Field(c, i)) => (*c, *i),
                _ => return None,
            },
            _ => return None,
        };
        let f = &self.prog.classes[c].fields[i];
        let w = f.elem_link?;
        let Ty::Array(el, _) = &f.ty else {
            return None;
        };
        Some(match &**el {
            Ty::Nullable(x) => (w, true, x.unref().clone()),
            t => (w, false, t.unref().clone()),
        })
    }

    /// The C lvalue of an array of links, for an operation that converts its offsets.
    fn link_array_place(&mut self, e: &Expr) -> String {
        self.link_ok.insert(key(strip_paren(e).span));
        let (p, _) = self.place(e);
        p
    }

    /// Whether field `name` of class `c` is an array of links.
    fn has_link_array(&self, c: ClassId, name: &str) -> bool {
        self.prog.classes[c]
            .fields
            .iter()
            .any(|f| f.name == name && f.elem_link.is_some())
    }

    /// Stores `v`, a C array of references, in field `name` of class `c`, an array of
    /// links at the C lvalue `place`: each as its offset from the slot it goes in, so
    /// in the value's own slot in its array (REFERENCES §7.2).
    fn link_array_store(&mut self, c: ClassId, name: &str, place: &str, v: &str, span: Span) -> String {
        let f = self.prog.classes[c]
            .fields
            .iter()
            .find(|f| f.name == name)
            .expect("the field");
        let w = f.elem_link.expect("an array of links");
        let nullable = matches!(&f.ty, Ty::Array(el, _) if matches!(**el, Ty::Nullable(_)));
        let set = self.link_store(
            &format!("{}.e[_li]", place),
            &LinkKind::Rel(w),
            nullable,
            "_lv.e[_li]",
            span,
        );
        format!(
            "({{ __typeof__({v}) _lv = {v}; for (int64_t _li = 0; _li < _lv.n; _li++) {set}; {p}.n = _lv.n; }})",
            v = v,
            set = set,
            p = place
        )
    }

    /// Following the offset at the C lvalue `slot` of an array of links.
    fn link_follow(&mut self, slot: &str, nullable: bool, target: &Ty, span: Span) -> String {
        let tc = self.cty(target, span);
        if nullable {
            format!("ZR_LINK_OR_NULL({}, {})", tc, slot)
        } else {
            format!("ZR_LINK({}, {})", tc, slot)
        }
    }

    /// When `e` is a field holding a link: the field as a C lvalue, what kind of link,
    /// whether it's nullable, and its type.
    fn link_place(&mut self, e: &Expr) -> Option<(String, LinkKind, bool, Ty)> {
        let e = strip_paren(e);
        match &e.kind {
            // An element of an array of links: its slot.
            E::Index { expr: base, .. } => {
                let (w, nullable, target) = self.link_array(base)?;
                let (slot, _) = self.link_slot(e)?;
                let ty = if nullable {
                    Ty::Nullable(Box::new(Ty::Ref(Box::new(target))))
                } else {
                    Ty::Ref(Box::new(target))
                };
                Some((slot, LinkKind::Rel(w), nullable, ty))
            }
            E::Member { expr: base, name } => {
                let bt = self.ty(base);
                let Ty::Class(c, cargs) = settle(bt.unref()) else {
                    return None;
                };
                let i = self.prog.classes[c]
                    .fields
                    .iter()
                    .position(|f| f.name == name.name)?;
                let w = self.link_kind(c, i)?;
                let ft = self.field_ty(c, i, &cargs);
                let nullable = matches!(ft, Ty::Nullable(_));
                let (b, bty) = self.rv(base);
                let acc = if is_ref(&bty) {
                    format!("({})->", b)
                } else {
                    format!("({}).", b)
                };
                Some((format!("{}f_{}", acc, name.name), w, nullable, ft))
            }
            E::Ident(_) => match self.facts.names.get(&key(e.span)) {
                Some(NameRes::Field(c, i)) => {
                    let (c, i) = (*c, *i);
                    let f = &self.prog.classes[c].fields[i];
                    if f.is_static {
                        return None;
                    }
                    let name = f.name.clone();
                    let w = self.link_kind(c, i)?;
                    let ft = self.field_ty(c, i, &self.cur_cargs.clone());
                    let nullable = matches!(ft, Ty::Nullable(_));
                    Some((format!("self->f_{}", name), w, nullable, ft))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Stores a link to `target`, a C pointer, in the field `raw`: its offset from the
    /// field, or from the start of its pool, which stops the program when it doesn't fit
    /// (REFERENCES §7.4, L4).
    pub(super) fn link_store(
        &mut self,
        raw: &str,
        kind: &LinkKind,
        nullable: bool,
        target: &str,
        span: Span,
    ) -> String {
        let loc = self.loc(span);
        match kind {
            LinkKind::Rel(w) => {
                let fail = self.on_fail(
                    34,
                    format!(
                        "zr_stop({}, 34, \"this link's target is too far away for its {}-byte offset\")",
                        loc,
                        bits(*w) / 8
                    ),
                );
                format!(
                    "ZR_LINK_SET({}, {}, {}, {})",
                    raw,
                    target,
                    if nullable { 1 } else { 0 },
                    fail
                )
            }
            LinkKind::Pool(w, base) => {
                let fail = self.on_fail(
                    34,
                    format!(
                        "zr_stop({}, 34, \"this pool link's target is too far into its array for its {}-byte offset\")",
                        loc,
                        bits(*w) / 8
                    ),
                );
                format!("ZR_PLINK_SET({}, {}, {}, {})", raw, target, base, fail)
            }
        }
    }

    /// A value with self-relative links, inserted into an array: it is built in its slot,
    /// since its links are measured from where it sits (REFERENCES §7.4, L2). Statements
    /// that leave `_e` pointing at it; `None` when the element type has no links.
    fn build_linked(
        &mut self,
        arr: &Arr,
        el: &Ty,
        value: &Expr,
        at: Where,
        span: Span,
    ) -> Option<String> {
        if !self.prog.link_bearing(el) {
            return None;
        }
        let v = strip_paren(value);
        let E::New { args, .. } = &v.kind else {
            return Some(self.unsupported(span, "copying a value with links") + ";");
        };
        if arr.lay != Layout::Vec || at != Where::End {
            return Some(
                self.unsupported(span, "values with links in this kind of array") + ";",
            );
        }
        if let Some(f) = self.facts.calls.get(&key(v.span))
            && let Origin::Ctor(c, i) = f.origin
            && self.prog.classes[c].ctors[i].can_fail
        {
            return Some(
                self.unsupported(span, "a constructor that can fail, for a value with links")
                    + ";",
            );
        }
        // The arguments come first, as for any call, and then the slot: an argument
        // that reads the array's length, or pushes into it, sees it as it was.
        let Some((cargs, call)) = self.ctor_args(args, v.span) else {
            return Some(self.unsupported(span, "this constructor") + ";");
        };
        let mut pre = String::new();
        let mut names = Vec::new();
        for a in cargs {
            let t = self.tmp("ca");
            let _ = write!(pre, "__auto_type {} = {}; ", t, a);
            names.push(t);
        }
        let st = call(self, "&(*_e)", &names);
        Some(format!(
            "{pre}_e = &(({ec}*){a}->p)[{a}->n++]; (void){st};",
            pre = pre,
            ec = arr.ec,
            a = arr.a,
            st = st
        ))
    }

    fn remove_op(&mut self, op: PostfixOp, x: &Expr, span: Span) -> CallOut {
        let c = self.remove_op_inner(op, x, span);
        self.check_unread(c, span)
    }

    fn remove_op_inner(&mut self, op: PostfixOp, x: &Expr, span: Span) -> CallOut {
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
    /// Inline and fixed, `T[.k]`: always exactly `k` elements, so just `{e[k]}`.
    Fixed,
    /// Capped, with a start slot: an inline ring buffer `{h, n, e[k]}`.
    Ring,
    /// Capped, with a cursor: an inline gap buffer `{gs, an, e[k]}`, the elements before
    /// the cursor at the start of `e` and those after it at the end.
    Gap,
    /// A slice or a view: `zr_slice`.
    Slice,
}

/// An array a function returns into its caller's storage (DESIGN §3.3): one on a data
/// stack with the plain `{p, n}` layout.
pub(super) fn dst_ret(t: &Ty) -> bool {
    matches!(t, Ty::Array(..)) && layout(t) == Some(Layout::Vec)
}

fn layout(t: &Ty) -> Option<Layout> {
    match t.unref() {
        Ty::Array(_, k) if k.cap == CapKind::Capped && k.has_cursor() => Some(Layout::Gap),
        Ty::Array(_, k) if k.cap == CapKind::Capped && (k.push_front() || k.pop_front()) => {
            Some(Layout::Ring)
        }
        Ty::Array(_, k) if k.cap == CapKind::Capped && k.is_frozen() => Some(Layout::Fixed),
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
        matches!(self, Layout::Capped | Layout::Fixed | Layout::Ring | Layout::Gap)
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

/// A link field (REFERENCES §7.2, §7.3).
#[derive(Clone, Debug)]
pub(super) enum LinkKind {
    /// Self-relative: the signed offset from the field, of this width.
    Rel(Prim),
    /// A pool link: the unsigned offset plus one from the start of a static array, of this
    /// width, and the C expression for that start.
    Pool(Prim, String),
}

#[derive(Clone, Copy, PartialEq)]
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
            Layout::Fixed => format!("((int64_t){})", self.cap),
            // A capped array's counter is as narrow as its cap allows, and unsigned, so
            // `len - 1` on an empty one would wrap.
            _ => format!("((int64_t){}->n)", self.a),
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
            Layout::Capped | Layout::Fixed => format!("{}->e[{}]", self.a, i),
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
            // Nothing removes from a fixed array (the checker sees to it).
            (Layout::Fixed, _) => String::new(),
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
                "if ({a}->n > 0) {{ _v = (({ec}*){a}->p)[0]; {a}->p = ({ec}*){a}->p + 1; {a}->n--; _s = 0; zr_deq_settle({a}, sizeof({ec})); }}",
                a = a,
                ec = ec
            ),
            (Layout::Deq, _) => format!(
                "if ({a}->n > 0) {{ _v = (({ec}*){a}->p)[--{a}->n]; _s = 0; zr_deq_settle({a}, sizeof({ec})); }}",
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

/// A call, or an array operator, whose status `@` reads: its value is its own result, where
/// another expression's is a place it names.
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

/// The spans of the names a statement reads, for `defer_record`.
fn stmt_idents(s: &Stmt, out: &mut Vec<Span>) {
    fn expr(e: &Expr, out: &mut Vec<Span>) {
        if let E::Ident(_) = &e.kind {
            out.push(e.span);
        }
        for x in crate::check::expr_children(e) {
            expr(x, out);
        }
    }
    match &s.kind {
        S::Expr(e) => expr(e, out),
        S::Local(d) => d.decls.iter().filter_map(|v| v.init.as_ref()).for_each(|e| expr(e, out)),
        S::Block(b) => b.stmts.iter().for_each(|x| stmt_idents(x, out)),
        S::If { cond, then, els } => {
            expr(cond, out);
            stmt_idents(then, out);
            if let Some(e) = els {
                stmt_idents(e, out);
            }
        }
        S::Destructure { value, .. } => expr(value, out),
        _ => {}
    }
}

/// A function's base case, which its calls to itself copy: its first `len` statements,
/// the last of them a `return switch` when `switch` gives that switch's span.
struct BaseCase {
    len: usize,
    switch: Option<Span>,
}

/// Whether evaluating `e` only reads and computes: no call, assignment, insertion,
/// removal, allocation or status, so computing it again where the function is called
/// changes nothing a program can see. It may still stop the program, as an index out
/// of range does, at the same place with the same message. Adds its size to `size`
/// when it is.
fn pure_expr(e: &Expr, size: &mut usize) -> bool {
    fn walk(e: &Expr, n: &mut usize) -> bool {
        *n += 1;
        let ok = match &e.kind {
            E::Int { .. }
            | E::Float { .. }
            | E::Char(_)
            | E::Bool(_)
            | E::Null
            | E::Ident(_)
            | E::This
            | E::PrimMember { .. } => true,
            E::Str(parts) => parts.iter().all(|p| matches!(p, StrPart::Text(_))),
            E::Binary { op, .. } => !matches!(
                op,
                ast::BinOp::InsertEnd | ast::BinOp::InsertFront | ast::BinOp::InsertCursor
            ),
            E::Unary { op, .. } => !matches!(op, ast::UnaryOp::PreInc | ast::UnaryOp::PreDec),
            E::InstanceOf { bind, .. } => bind.is_none(),
            E::Paren(_) | E::Cond { .. } | E::Cast { .. } | E::Member { .. } | E::Index { .. } => {
                true
            }
            _ => false,
        };
        ok && crate::check::expr_children(e).into_iter().all(|x| walk(x, n))
    }
    let mut n = 0;
    let ok = walk(e, &mut n);
    if ok {
        *size += n;
    }
    ok
}

/// Whether a statement only returns, a value computed as `pure_expr` allows or none.
fn leaf_return(s: &Stmt, size: &mut usize) -> bool {
    match &s.kind {
        S::Return {
            status: None,
            value,
        } => {
            *size += 1;
            value.as_ref().is_none_or(|v| pure_expr(v, size))
        }
        S::Block(b) => matches!(b.stmts.as_slice(), [one] if leaf_return(one, size)),
        _ => false,
    }
}

/// Whether C code is a plain variable name, whose address can be taken.
/// Whether a function is small enough to ask the C compiler to inline it: a few
/// straight-line statements and few operations, measured before its checks are added.
/// Each checked operation adds a branch and a call to the C, so without the hint a
/// one-line function such as spectralnorm's `a(i, j)` looks too big to inline, and stays
/// a call in the loop that uses it.
fn small_fn(stmts: &[Stmt]) -> bool {
    fn count(e: &Expr) -> usize {
        1 + crate::check::expr_children(e).into_iter().map(count).sum::<usize>()
    }
    fn stmt(s: &Stmt, n: &mut usize) -> bool {
        match &s.kind {
            S::Expr(e) | S::Yield(e) => {
                *n += count(e);
                true
            }
            S::Return { value, .. } => {
                *n += 1 + value.as_ref().map_or(0, count);
                true
            }
            S::Local(d) => {
                for v in &d.decls {
                    *n += 1 + v.init.as_ref().map_or(0, count);
                }
                true
            }
            S::If { cond, then, els } => {
                *n += count(cond);
                stmt(then, n) && els.as_ref().is_none_or(|e| stmt(e, n))
            }
            S::Block(b) => b.stmts.iter().all(|x| stmt(x, n)),
            S::Empty => true,
            // Loops, switches, defers and the rest.
            _ => false,
        }
    }
    let mut n = 0;
    stmts.len() <= 6 && stmts.iter().all(|s| stmt(s, &mut n)) && n <= 48
}

fn is_c_name(c: &str) -> bool {
    !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}
