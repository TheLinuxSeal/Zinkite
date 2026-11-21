//! `hashmap` and `hashset` (DESIGN §4.14, STDLIB §7.3, §7.4). The header, the index and
//! the hashes are in the runtime (`zr_map`); each key and value type gets an entry struct
//! and its find, add, remove and rebuild functions, generated once.

use super::*;

#[derive(Clone)]
pub(super) struct MapOps {
    pub entry: String,
    pub find: String,
    pub add: String,
    pub del: String,
    pub key: Ty,
    pub val: Option<Ty>,
    /// The hash of the C expression `KEY`.
    pub hash: String,
}

impl<'p, 'a> Gen<'p, 'a> {
    /// How a key type is hashed and compared, and whether its hash is worth storing.
    fn key_code(&mut self, k: &Ty) -> Option<(String, String, bool)> {
        match k.unref() {
            Ty::Prim(p) if p.is_int() || *p == Prim::Bool => Some((
                if bits(*p) == 128 {
                    "zr_hash_u128((zr_u128)(KEY))".into()
                } else {
                    "zr_hash_u64((uint64_t)(KEY))".into()
                },
                "((A) == (B))".into(),
                false,
            )),
            Ty::Class(id, _) if self.prog.classes[*id].kind == ClassKind::Enum => Some((
                "zr_hash_u64((uint64_t)(KEY))".into(),
                "((A) == (B))".into(),
                false,
            )),
            // A string's bytes are hashed; its hash is kept, so growing rehashes nothing.
            Ty::Slice(e) | Ty::View(e) if **e == Ty::u8() => Some((
                "zr_hash_bytes((KEY).p, (KEY).n)".into(),
                "zr_eq(A, B, 1)".into(),
                true,
            )),
            _ => None,
        }
    }

    /// The generated operations for a `hashmap` or `hashset` type; `None`, reported,
    /// when its key type isn't supported yet.
    pub(super) fn map_ops(&mut self, t: &Ty, span: Span) -> Option<MapOps> {
        let (k, v) = match t.unref() {
            Ty::Map(k, v) | Ty::MapEntry(k, v) => ((**k).clone(), Some((**v).clone())),
            Ty::Set(k) => ((**k).clone(), None),
            _ => return None,
        };
        let kc = self.cty(&k, span);
        let vc = v.as_ref().map(|v| self.cty(v, span));
        let id = format!("{}|{}", kc, vc.as_deref().unwrap_or("-"));
        if let Some(m) = self.maps.get(&id) {
            return Some(m.clone());
        }
        let Some((hash, eq, store_h)) = self.key_code(&k) else {
            let what = format!("`{}` as a key", self.prog.show(&k));
            self.unsupported(span, &what);
            return None;
        };
        let n = self.maps.len();
        let entry = format!("zme{}", n);
        let mut body = format!("{} k; ", kc);
        if let Some(vc) = &vc {
            let _ = write!(body, "{} v; ", vc);
        }
        if store_h {
            body.push_str("uint64_t h; ");
        }
        body.push_str("uint8_t live; ");
        let mut deps = self.deps(&k);
        if let Some(v) = &v {
            deps.extend(self.deps(v));
        }
        self.def_names.insert(entry.clone(), entry.clone());
        self.defs.push((entry.clone(), body, deps));
        let ops = MapOps {
            entry: entry.clone(),
            find: format!("zmf{}", n),
            add: format!("zma{}", n),
            del: format!("zmd{}", n),
            key: k,
            val: v,
            hash: hash.clone(),
        };
        let eh = if store_h {
            "e[i].h".to_string()
        } else {
            hash.replace("KEY", "e[i].k")
        };
        let same = eq.replace('A', "e[s - 1].k").replace('B', "key");
        let same = if store_h {
            format!("e[s - 1].h == h && {}", same)
        } else {
            same
        };
        let set_h = if store_h { "e[i].h = h; " } else { "" };
        let _ = write!(
            self.helpers,
            r#"static void zmr{n}(zr_map *m, int64_t cap) {{
    zr_map_size(m, cap);
    {E} *e = ({E} *)m->e;
    for (int64_t i = 0; i < m->n; i++) {{
        if (!e[i].live) continue;
        uint64_t h = {eh};
        int64_t j = (int64_t)(h & (uint64_t)m->mask);
        while (zr_ix_get(m, j)) j = (j + 1) & m->mask;
        zr_ix_set(m, j, (uint64_t)i + 1);
    }}
}}
static ZR_UNUSED int64_t zmf{n}(zr_map *m, {K} key, uint64_t h, int64_t *at) {{
    *at = -1;
    if (!m->cap) return -1;
    {E} *e = ({E} *)m->e;
    uint64_t dead = zr_ix_dead(m);
    for (int64_t j = (int64_t)(h & (uint64_t)m->mask);; j = (j + 1) & m->mask) {{
        uint64_t s = zr_ix_get(m, j);
        if (!s) return -1;
        if (s != dead && {same}) {{ *at = j; return (int64_t)s - 1; }}
    }}
}}
static ZR_UNUSED int64_t zma{n}(zr_map *m, {K} key, uint64_t h) {{
    if (m->n >= m->cap) zmr{n}(m, m->cap ? m->cap * 2 : 8);
    {E} *e = ({E} *)m->e;
    int64_t i = m->n++;
    memset(&e[i], 0, sizeof e[i]);
    e[i].k = key; {set_h}e[i].live = 1;
    uint64_t dead = zr_ix_dead(m);
    int64_t j = (int64_t)(h & (uint64_t)m->mask);
    for (;;) {{ uint64_t s = zr_ix_get(m, j); if (!s || s == dead) break; j = (j + 1) & m->mask; }}
    zr_ix_set(m, j, (uint64_t)i + 1);
    m->live++;
    return i;
}}
/* Removing marks the entry dead; once more than half are, the entries are compacted in
 * order, which a removal may do, as references don't survive one (rule R). */
static ZR_UNUSED uint8_t zmd{n}(zr_map *m, {K} key, uint64_t h) {{
    int64_t j, i = zmf{n}(m, key, h, &j);
    if (i < 0) return {missing};
    zr_ix_set(m, j, zr_ix_dead(m));
    {E} *e = ({E} *)m->e;
    e[i].live = 0;
    m->live--;
    if (m->live < m->n / 2) {{
        int64_t w = 0;
        for (int64_t r = 0; r < m->n; r++) if (e[r].live) e[w++] = e[r];
        m->n = w;
        zmr{n}(m, m->cap);
    }}
    return 0;
}}

"#,
            n = n,
            E = entry,
            K = kc,
            eh = eh,
            same = same,
            set_h = set_h,
            missing = status_code("MISSING"),
        );
        self.maps.insert(id, ops.clone());
        Some(ops)
    }

    /// A method of a `hashmap` or `hashset`.
    pub(super) fn map_method(
        &mut self,
        base: &Expr,
        m: &str,
        args: &[ast::Arg],
        ret: Ty,
        span: Span,
    ) -> CallOut {
        let bt = self.ty(base);
        let mt = settle(bt.unref());
        let out = |code: String, ret: Ty, fails: bool| CallOut { code, ret, fails };
        let Some(ops) = self.map_ops(&mt, span) else {
            return out("0".into(), ret, false);
        };
        if !(is_ref(&bt) || self.is_place(base)) {
            return out(
                self.unsupported(span, "a method of a temporary map"),
                ret,
                false,
            );
        }
        let (p, _) = self.place(base);
        let a = self.tmp("m");
        let ptr = format!("zr_map *{} = &({});", a, p);
        if m == "clear" {
            return out(
                format!("({{ {} zr_map_clear({}); }})", ptr, a),
                Ty::Void,
                false,
            );
        }
        let kc = self.cty(&ops.key, span);
        let kv = self.expr(&args[0].expr, &ops.key);
        let e = format!("(({}*){}->e)", ops.entry, a);
        let mut pre = format!("{} {} _k = {}; ", ptr, kc, kv);
        let vc = ops.val.as_ref().map(|v| self.cty(v, span));
        // The other argument, evaluated before the map is looked at.
        if let (Some(v), Some(vc), Some(x)) = (&ops.val, &vc, args.get(1)) {
            let xv = self.expr(&x.expr, v);
            let _ = write!(pre, "{} _v = {}; ", vc, xv);
        }
        let _ = write!(
            pre,
            "uint64_t _h = {}; int64_t _j, _i = {}({}, _k, _h, &_j); (void)_j; ",
            ops.hash.replace("KEY", "_k"),
            ops.find,
            a
        );
        let add = format!("{}({}, _k, _h)", ops.add, a);
        let missing = status_code("MISSING");
        let val = ops.val.clone().unwrap_or(Ty::Void);
        match m {
            "put" => out(
                format!(
                    "({{ {} if (_i < 0) _i = {}; {}[_i].v = _v; }})",
                    pre, add, e
                ),
                Ty::Void,
                false,
            ),
            "getOrAdd" => out(
                format!(
                    "({{ {} if (_i < 0) {{ _i = {}; {}[_i].v = _v; }} &{}[_i].v; }})",
                    pre, add, e, e
                ),
                Ty::Ref(Box::new(val)),
                false,
            ),
            "get" => {
                let rt = self.result_ty(&val, span);
                out(
                    format!(
                        "({{ {} {} _r = {{0}}; if (_i < 0) _r.s = {}; else _r.v = {}[_i].v; _r; }})",
                        pre, rt, missing, e
                    ),
                    val,
                    true,
                )
            }
            "getOr" => out(
                format!("({{ {} _i < 0 ? _v : {}[_i].v; }})", pre, e),
                val,
                false,
            ),
            "has" => out(format!("({{ {} _i >= 0; }})", pre), Ty::bool(), false),
            "add" => out(
                format!(
                    "({{ {} bool _new = _i < 0; if (_new) {}; _new; }})",
                    pre, add
                ),
                Ty::bool(),
                false,
            ),
            "remove" => out(
                format!("({{ {} (void)_i; {}({}, _k, _h); }})", pre, ops.del, a),
                Ty::Void,
                true,
            ),
            _ => out(
                self.unsupported(span, &format!("`{}` on {}", m, self.prog.show(&mt))),
                ret,
                false,
            ),
        }
    }

    /// A for-each over a map's entries or a set's elements, in insertion order, skipping
    /// removed ones. By reference, the loop variable points at the entry, so writing
    /// `e.value` changes the map.
    pub(super) fn map_for_each(
        &mut self,
        label: Option<String>,
        name: &Name,
        iter: &Expr,
        body: &Stmt,
        by_ref: bool,
    ) {
        let it = self.ty(iter);
        let mt = settle(it.unref());
        let Some(ops) = self.map_ops(&mt, iter.span) else {
            return;
        };
        if !(is_ref(&it) || self.is_place(iter)) {
            self.unsupported(iter.span, "a for-each over a temporary map");
            return;
        }
        let (p, _) = self.place(iter);
        let mp = self.tmp("mp");
        self.line(&format!("zr_map *{} = &({});", mp, p));
        let i = self.tmp("i");
        let ep = self.tmp("ep");
        let (vname, _) = match name {
            Name::Ident(n) => self.new_local(n),
            Name::Discard(_) => (self.tmp("x"), Ty::Unknown),
        };
        let is_set = ops.val.is_none();
        let bind = match (is_set, by_ref) {
            (true, true) => format!("{} *{} = &{}->k;", self.cty(&ops.key, iter.span), vname, ep),
            (true, false) => format!("{} {} = {}->k;", self.cty(&ops.key, iter.span), vname, ep),
            (false, true) => format!("{} *{} = {};", ops.entry, vname, ep),
            (false, false) => format!("{} {} = *{};", ops.entry, vname, ep),
        };
        let before = format!(
            "{e} *{ep} = &(({e} *){mp}->e)[{i}]; if (!{ep}->live) continue; {bind}",
            e = ops.entry,
            ep = ep,
            mp = mp,
            i = i,
            bind = bind
        );
        self.loop_(
            label,
            &format!(
                "for (int64_t {i} = 0; {i} < {mp}->n; {i}++)",
                i = i,
                mp = mp
            ),
            body,
            Some(&move |me: &mut Self| me.line(&before)),
        );
    }
}
