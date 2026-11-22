//! `pool<T>` and `Handle<T>` (DESIGN §3.5, STDLIB §7.5). The pool's header and its free
//! list are in the runtime (`zr_pool`); each element type gets a slot struct,
//! `{gen, next, value}`, so the value is reached with a plain C member access.

use super::*;

impl<'p, 'a> Gen<'p, 'a> {
    /// The slot struct for a pool of `el`, defined the first time it's asked for.
    pub(super) fn pool_slot(&mut self, el: &Ty, span: Span) -> String {
        let ec = self.cty(el, span);
        let name = format!("zp_{}", ec.replace('*', "p"));
        if !self.def_names.contains_key(&name) {
            let deps = self.deps(el);
            self.def_names.insert(name.clone(), name.clone());
            self.defs.push((
                name.clone(),
                format!("uint32_t g; uint32_t next; {} v; ", ec),
                deps,
            ));
        }
        name
    }

    /// `add`, `get`, `free` and `clear`.
    pub(super) fn pool_method(
        &mut self,
        base: &Expr,
        m: &str,
        args: &[ast::Arg],
        ret: Ty,
        span: Span,
    ) -> CallOut {
        let bt = self.ty(base);
        let Ty::Pool(el) = settle(bt.unref()) else {
            return CallOut {
                code: self.unsupported(span, "this pool"),
                ret,
                fails: false,
            };
        };
        if !(is_ref(&bt) || self.is_place(base)) {
            return CallOut {
                code: self.unsupported(span, "a method of a temporary pool"),
                ret,
                fails: false,
            };
        }
        let el = *el;
        let slot = self.pool_slot(&el, span);
        let (p, _) = self.place(base);
        let ptr = format!("zr_pool *_p = &({});", p);
        let at = |i: &str| format!("(&(({} *)_p->v.p)[{}])", slot, i);
        let handle = |me: &mut Self| me.expr(&args[0].expr, &Ty::Handle(Box::new(el.clone())));
        match m {
            "add" => {
                // The value first: it may read the pool.
                let ec = self.cty(&el, span);
                let x = self.expr(&args[0].expr, &el);
                CallOut {
                    code: format!(
                        "({{ {} {} _x = {}; uint32_t _i = zr_pool_take(_p, sizeof({})); {} *_s = {}; _s->v = _x; (zr_handle){{_i, _s->g}}; }})",
                        ptr,
                        ec,
                        x,
                        slot,
                        slot,
                        at("_i")
                    ),
                    ret: Ty::Handle(Box::new(el)),
                    fails: false,
                }
            }
            // A reference to the slot even when it's been freed, since a slot always holds a
            // T; for a slot the pool never had, to a zero value of its own.
            "get" => {
                let h = handle(self);
                let r = Ty::Ref(Box::new(el.clone()));
                let rt = self.result_ty(&r, span);
                let ec = self.cty(&el, span);
                CallOut {
                    code: format!(
                        "({{ {} zr_handle _h = {}; uint8_t _st = zr_pool_check(_p, _h, sizeof({})); ({}){{_st == 22 ? ({} *)zr_zero_ref(sizeof({})) : &{}->v, _st}}; }})",
                        ptr,
                        h,
                        slot,
                        rt,
                        ec,
                        ec,
                        at("_h.i")
                    ),
                    ret: r,
                    fails: true,
                }
            }
            "free" => {
                let h = handle(self);
                CallOut {
                    code: format!(
                        "({{ {} zr_handle _h = {}; uint8_t _st = zr_pool_check(_p, _h, sizeof({s})); if (!_st) zr_pool_release(_p, _h.i, sizeof({s})); _st; }})",
                        ptr,
                        h,
                        s = slot
                    ),
                    ret: Ty::Void,
                    fails: true,
                }
            }
            "clear" => CallOut {
                code: format!("({{ {} zr_pool_clear(_p, sizeof({})); }})", ptr, slot),
                ret: Ty::Void,
                fails: false,
            },
            _ => CallOut {
                code: self.unsupported(span, &format!("`{}` on a pool", m)),
                ret,
                fails: false,
            },
        }
    }

    /// A for-each over the live slots, in slot order. Slots added during the loop aren't
    /// visited; one freed during it is skipped if the loop hasn't reached it yet.
    pub(super) fn pool_for_each(
        &mut self,
        label: Option<String>,
        name: &Name,
        iter: &Expr,
        body: &Stmt,
        by_ref: bool,
    ) {
        let it = self.ty(iter);
        let Ty::Pool(el) = settle(it.unref()) else {
            self.unsupported(iter.span, "this for-each");
            return;
        };
        if !(is_ref(&it) || self.is_place(iter)) {
            self.unsupported(iter.span, "a for-each over a temporary pool");
            return;
        }
        let slot = self.pool_slot(&el, iter.span);
        let (p, _) = self.place(iter);
        let pp = self.tmp("pp");
        let n = self.tmp("n");
        let i = self.tmp("i");
        let sp = self.tmp("sp");
        self.line(&format!(
            "zr_pool *{} = &({}); int64_t {} = {}->v.n;",
            pp, p, n, pp
        ));
        let (vname, vty) = match name {
            Name::Ident(id) => self.new_local(id),
            Name::Discard(_) => (self.tmp("x"), (*el).clone()),
        };
        let vc = self.cty(&vty, iter.span);
        let bind = format!(
            "{} *{} = &(({} *){}->v.p)[{}]; if (!({}->g & 1)) continue; {} {} = {}{}->v;",
            slot,
            sp,
            slot,
            pp,
            i,
            sp,
            vc,
            vname,
            if by_ref { "&" } else { "" },
            sp
        );
        self.loop_(
            label,
            &format!("for (int64_t {} = 0; {} < {}; {}++)", i, i, n, i),
            body,
            Some(&move |me: &mut Self| me.line(&bind)),
        );
    }
}
