//! Threads (STDLIB §10.3): `thread.spawn`, `queue<T>`, and what a new thread gets a copy
//! of. A program that imports zink.thread is compiled with `ZR_THREADS`, which gives each
//! thread its own data stacks, scratch stack, trail and Files (`runtime/zink_rt.h`), and
//! each of the program's statics a copy per thread (`ZR_TLS`).
//!
//! A spawned lambda becomes a C function of its own, `ztN(void *)`, run on the new thread.
//! Its argument is a packet the spawning thread fills: the bytes of each local the lambda
//! captures, which the checker made sure are flat, then the statics a thread copies
//! (`Program::thread_copyable`), an array on a data stack with its elements. The new
//! thread reads its captures out of the packet into locals, and its statics into its own
//! copies, each array onto a data stack of the thread's own.

use super::*;
use crate::ast::LambdaBody;
use lambdas::as_lambda;

/// A spawned lambda waiting to be generated as its thread's function.
pub(super) struct PendingSpawn {
    pub name: String,
    pub body: LambdaBody,
    /// Each capture: its declaration, its type and its C type.
    pub caps: Vec<((u32, u32), Ty, String)>,
    pub class: ClassId,
    pub file: usize,
    pub subst: Vec<(String, Ty)>,
    pub cargs: Vec<Ty>,
}

impl<'p, 'a> Gen<'p, 'a> {
    /// `put`, `get`, `get(timeout)`, `poll`, `close` and `limit`. A queue that is a zero
    /// value, as a failed call gives, fails with INVALID (the runtime checks for NULL).
    pub(super) fn queue_method(
        &mut self,
        base: &Expr,
        m: &str,
        args: &[ast::Arg],
        ret: Ty,
        span: Span,
    ) -> CallOut {
        let bt = self.ty(base);
        let Ty::Queue(el) = settle(bt.unref()) else {
            return CallOut {
                code: self.unsupported(span, "this queue"),
                ret,
                fails: false,
            };
        };
        let el = *el;
        let q = self.expr(base, &Ty::Queue(Box::new(el.clone())));
        let ec = self.cty(&el, span);
        let take = |me: &mut Self, wait: &str| {
            let rt = me.result_ty(&el, span);
            format!(
                "({{ zr_queue *_q = {}; {} _r; _r.s = zr_q_get(_q, &_r.v, {}); _r; }})",
                q, rt, wait
            )
        };
        match m {
            "put" => {
                let x = self.expr(&args[0].expr, &el);
                CallOut {
                    code: format!(
                        "({{ zr_queue *_q = {}; {} _x = {}; zr_q_put(_q, &_x); }})",
                        q, ec, x
                    ),
                    ret: Ty::Void,
                    fails: true,
                }
            }
            "get" if args.is_empty() => CallOut {
                code: take(self, "-1"),
                ret: el,
                fails: true,
            },
            // With a timeout: TIMEOUT when it runs out first, and when it's 0 or less.
            "get" => {
                let dt = settle(&self.ty(&args[0].expr));
                let d = self.expr(&args[0].expr, &dt);
                let rt = self.result_ty(&el, span);
                CallOut {
                    code: format!(
                        "({{ zr_queue *_q = {}; int64_t _ms = ({}).f_nanos / 1000000; {} _r; _r.s = zr_q_get(_q, &_r.v, _ms > 0 ? _ms : 0); if (_r.s == 192) _r.s = 110; _r; }})",
                        q, d, rt
                    ),
                    ret: el,
                    fails: true,
                }
            }
            "poll" => CallOut {
                code: take(self, "0"),
                ret: el,
                fails: true,
            },
            "close" => CallOut {
                code: format!("zr_q_close({})", q),
                ret: Ty::Void,
                fails: false,
            },
            "limit" => {
                let n = self.expr(&args[0].expr, &Ty::i64());
                CallOut {
                    code: format!("zr_q_limit({}, {})", q, n),
                    ret: Ty::Void,
                    fails: true,
                }
            }
            _ => CallOut {
                code: self.unsupported(span, &format!("`{}` on a queue", m)),
                ret,
                fails: false,
            },
        }
    }

    /// `thread.spawn(() => ...)`: the packet of captures and statics, and the start of the
    /// thread. AGAIN, and a Worker that is a zero value, when the system can't start one.
    pub(super) fn spawn_code(&mut self, lam: &Expr, ret: Ty, span: Span) -> CallOut {
        let l = strip_paren(lam);
        let Some((_, body)) = as_lambda(l) else {
            return CallOut {
                code: self.unsupported(span, "`thread.spawn` without a lambda"),
                ret,
                fails: false,
            };
        };
        let mut caps = Vec::new();
        let mut put = String::new();
        let mut size = String::from("0");
        for d in lambdas::outer_names(self.facts, l) {
            let Some((c, ty)) = self.locals.get(&d).cloned() else {
                return CallOut {
                    code: self
                        .unsupported(span, "a spawned lambda that uses a lambda from outside it"),
                    ret,
                    fails: false,
                };
            };
            let ct = self.cty(&ty, span);
            let _ = write!(size, " + sizeof({})", ct);
            let _ = write!(
                put,
                "{{ {ct} _v = {c}; memcpy(_pk + _o, &_v, sizeof _v); _o += sizeof _v; }} ",
                ct = ct,
                c = c
            );
            caps.push((d, ty, ct));
        }
        let name = format!("zt{}", self.spawn_n);
        self.spawn_n += 1;
        self.pending_spawns.push(PendingSpawn {
            name: name.clone(),
            body: body.clone(),
            caps,
            class: self.class,
            file: self.file,
            subst: self.subst.clone(),
            cargs: self.cur_cargs.clone(),
        });
        let rt = self.result_ty(&ret, span);
        CallOut {
            code: format!(
                "({{ size_t _o = 0; char *_pk = (char *)malloc({size} + zr_statics_size()); if (!_pk) zr_fail(\"zink runtime\", 0, \"out of memory for a worker\"); {put}zr_statics_save(_pk + _o); int64_t _id = zr_thread_spawn({name}, _pk); {rt} _r; _r.v.f_id = _id; _r.s = _id ? 0 : zr_errno(); _r; }})",
                size = size,
                put = put,
                name = name,
                rt = rt
            ),
            ret,
            fails: true,
        }
    }

    /// Generates each spawned lambda's thread function: its captures read out of the
    /// packet into locals, the statics into the thread's own copies, then its body.
    pub(super) fn drain_spawns(&mut self) {
        while let Some(s) = self.pending_spawns.pop() {
            self.file = s.file;
            let saved_subst = std::mem::replace(&mut self.subst, s.subst.clone());
            let saved_cargs = std::mem::replace(&mut self.cur_cargs, s.cargs.clone());
            self.begin_fn(s.class, Ty::Void, true, false, false);
            self.label_fn("spawned lambda");
            let sig = format!("static uint8_t {}(void *_pkv)", s.name);
            let _ = writeln!(self.protos, "{};", sig);
            self.line("char *_pk = (char *)_pkv; size_t _o = 0; (void)_pk; (void)_o;");
            for (i, (d, ty, ct)) in s.caps.iter().enumerate() {
                let c = format!("_wc{}", i);
                self.line(&format!(
                    "{ct} {c}; memcpy(&{c}, _pk + _o, sizeof {c}); _o += sizeof {c};",
                    ct = ct,
                    c = c
                ));
                self.locals.insert(*d, (c, ty.clone()));
            }
            self.line("zr_statics_load(_pk + _o);");
            match &s.body {
                LambdaBody::Expr(x) => {
                    let c = self.expr_stmt_code(x);
                    self.line(&format!("{};", c));
                }
                LambdaBody::Block(b) => self.block_stmts(&b.stmts),
            }
            self.line("return 0;");
            let out = self.take_body();
            let _ = writeln!(self.bodies, "{} {{\n{}}}\n", sig, out);
            self.subst = saved_subst;
            self.cur_cargs = saved_cargs;
        }
    }

    /// `zr_statics_size`, `zr_statics_save` and `zr_statics_load`: the statics a new
    /// thread gets a copy of, as bytes in its packet.
    pub(super) fn statics_copy_fns(&self) -> String {
        let mut size = String::new();
        let mut save = String::new();
        let mut load = String::new();
        for (name, arr) in &self.thread_statics {
            match arr {
                // An array on a data stack: its header, then its elements, which the new
                // thread puts on a data stack of its own.
                Some((init, ec)) => {
                    let elems = format!("(size_t){}.n * sizeof({})", name, ec);
                    let _ = writeln!(size, "    n += sizeof {} + {};", name, elems);
                    let _ = writeln!(
                        save,
                        "    memcpy(p, &{n}, sizeof {n}); p += sizeof {n}; memcpy(p, {n}.p, {e}); p += {e};",
                        n = name,
                        e = elems
                    );
                    let _ = writeln!(
                        load,
                        "    {{ int64_t k; memcpy(&k, p + offsetof(zr_vec, n), sizeof k); p += sizeof {n}; {n} = {init}; {n}.n = k; memcpy({n}.p, p, {e}); p += {e}; }}",
                        n = name,
                        init = init,
                        e = elems
                    );
                }
                None => {
                    let _ = writeln!(size, "    n += sizeof {};", name);
                    let _ = writeln!(
                        save,
                        "    memcpy(p, &{n}, sizeof {n}); p += sizeof {n};",
                        n = name
                    );
                    let _ = writeln!(
                        load,
                        "    memcpy(&{n}, p, sizeof {n}); p += sizeof {n};",
                        n = name
                    );
                }
            }
        }
        for (name, init) in &self.fresh_statics {
            let _ = writeln!(load, "    {} = {};", name, init);
        }
        format!(
            "static size_t zr_statics_size(void) {{\n    size_t n = 0;\n{}    return n;\n}}\n\nstatic void zr_statics_save(char *p) {{\n{}    (void)p;\n}}\n\nstatic void zr_statics_load(char *p) {{\n{}    (void)p;\n}}\n",
            size, save, load
        )
    }
}
