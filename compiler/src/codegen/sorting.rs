//! Sorting and the other array methods that take a function (STDLIB §7.1): a
//! comparison, a key or a predicate, inlined where it's used.

use super::*;

/// How elements are ordered: as they compare (`<`, or text order for strings), by a
/// `less(a, b)` function, or by a key function; each possibly reversed.
pub(super) enum Order<'e> {
    Natural,
    Less(&'e Expr),
    Key(&'e Expr, Ty),
}

impl<'p, 'a> Gen<'p, 'a> {
    /// Whether `x` goes before `y`, two C lvalues of type `el`.
    pub(super) fn order_less(
        &mut self,
        o: &Order,
        desc: bool,
        x: &str,
        y: &str,
        el: &Ty,
        span: Span,
    ) -> String {
        let (x, y) = if desc { (y, x) } else { (x, y) };
        match o {
            Order::Natural => self.natural_less(x, y, el, span),
            Order::Less(f) => self.apply(f, &[x.to_string(), y.to_string()]),
            Order::Key(f, kt) => {
                let kc = self.cty(kt, span);
                let kx = self.apply(f, &[x.to_string()]);
                let ky = self.apply(f, &[y.to_string()]);
                let less = self.natural_less("_ka", "_kb", kt, span);
                format!(
                    "({{ {kc} _ka = {kx}; {kc} _kb = {ky}; {less}; }})",
                    kc = kc,
                    kx = kx,
                    ky = ky,
                    less = less
                )
            }
        }
    }

    fn natural_less(&mut self, x: &str, y: &str, t: &Ty, span: Span) -> String {
        match settle(t.unref()) {
            Ty::Prim(_) => format!("(({}) < ({}))", x, y),
            Ty::Class(c, _) if self.prog.classes[c].kind == ClassKind::Enum => {
                format!("(({}) < ({}))", x, y)
            }
            t if t.is_string() => format!("(zr_cmp({}, {}) < 0)", x, y),
            _ => self.unsupported(span, "ordering these values without a comparison"),
        }
    }

    /// Whether two C lvalues of type `t` are equal, as `==` compares them.
    pub(super) fn natural_eq(&mut self, x: &str, y: &str, t: &Ty, span: Span) -> String {
        match settle(t.unref()) {
            Ty::Prim(_) => format!("(({}) == ({}))", x, y),
            Ty::Class(c, _) if self.prog.classes[c].kind == ClassKind::Enum => {
                format!("(({}) == ({}))", x, y)
            }
            t if t.is_string() => format!("zr_eq({}, {}, 1)", x, y),
            _ => self.unsupported(span, "comparing these values"),
        }
    }

    /// For reordering an array in place: statements that lay out its `_n` elements at
    /// `_d`, the type of an item of `_d`, how an item names its element (empty when it is
    /// one), and statements that put the array back. A cursor array sorts its index, so
    /// its elements stay where they are.
    pub(super) fn reorder_view(
        &mut self,
        arr: &Arr,
        len: &str,
    ) -> (String, String, String, String) {
        let (a, ec) = (arr.a.clone(), arr.ec.clone());
        match arr.lay {
            Layout::Cur | Layout::Chk => {
                if arr.lay == Layout::Chk {
                    self.used_scratch = true;
                }
                let (setup, finish) = arr.gather().unwrap_or_default();
                (
                    setup,
                    "uint32_t".into(),
                    format!("(({}*){}->d.data)", ec, a),
                    finish,
                )
            }
            Layout::Fixed => (
                format!("int64_t _n = {k}; {ec} *_d = {a}->e;", k = arr.cap, a = a, ec = ec),
                ec.clone(),
                String::new(),
                String::new(),
            ),
            Layout::Capped => (
                format!("int64_t _n = {a}->n; {ec} *_d = {a}->e;", a = a, ec = ec),
                ec,
                String::new(),
                String::new(),
            ),
            // Made one block first, as for a slice, so slices taken since the last change
            // still show the same positions (DESIGN §4.10).
            Layout::Ring | Layout::Gap => (
                format!(
                    "int64_t _n = {}; {ec} *_d = ({ec} *){};",
                    len,
                    arr.span("0"),
                    ec = ec
                ),
                ec,
                String::new(),
                String::new(),
            ),
            _ => (
                format!(
                    "int64_t _n = {a}->n; {ec} *_d = ({ec}*){a}->p;",
                    a = a,
                    ec = ec
                ),
                ec,
                String::new(),
                String::new(),
            ),
        }
    }

    /// A stable bottom-up merge sort of `_d[0..n)`, through a buffer on the scratch stack.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge_sort_code(
        &mut self,
        item: &str,
        value: &str,
        n: &str,
        o: &Order,
        desc: bool,
        el: &Ty,
        span: Span,
    ) -> String {
        let get = |x: &str| {
            if value.is_empty() {
                x.to_string()
            } else {
                format!("{}[{}]", value, x)
            }
        };
        self.merge_sort_by(item, &get, n, o, desc, el, span)
    }

    /// The same, where `get` gives the element an entry of `_d` stands for.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge_sort_by(
        &mut self,
        item: &str,
        get: &dyn Fn(&str) -> String,
        n: &str,
        o: &Order,
        desc: bool,
        el: &Ty,
        span: Span,
    ) -> String {
        self.used_scratch = true;
        let less = self.order_less(o, desc, &get("_d[_j]"), &get("_d[_i]"), el, span);
        format!(
            "{{ {item} *_tmp = ({item} *)zr_scratch_alloc((size_t)({n}) * sizeof({item})); for (int64_t _w = 1; _w < {n}; _w *= 2) {{ for (int64_t _lo = 0; _lo < {n}; _lo += 2 * _w) {{ int64_t _mid = _lo + _w < {n} ? _lo + _w : {n}, _hi = _lo + 2 * _w < {n} ? _lo + 2 * _w : {n}, _i = _lo, _j = _mid, _k = _lo; while (_i < _mid && _j < _hi) {{ if ({less}) _tmp[_k++] = _d[_j++]; else _tmp[_k++] = _d[_i++]; }} while (_i < _mid) _tmp[_k++] = _d[_i++]; while (_j < _hi) _tmp[_k++] = _d[_j++]; }} memcpy(_d, _tmp, (size_t)({n}) * sizeof *_d); }} }}",
            item = item,
            n = n,
            less = less
        )
    }

    /// Quickselect of `_d[0.._n)` for position `k`, with a median-of-three pivot. A
    /// budget of rounds keeps the worst case at O(n log n): when it runs out, a partial sort
    /// of the first k + 1 finishes the job.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_code(
        &mut self,
        item: &str,
        value: &str,
        k: &str,
        o: &Order,
        el: &Ty,
        span: Span,
        loc: &str,
    ) -> String {
        let get = |x: &str| {
            if value.is_empty() {
                x.to_string()
            } else {
                format!("{}[{}]", value, x)
            }
        };
        let swap = |a: &str, b: &str| {
            format!(
                "{{ {item} _t = _d[{a}]; _d[{a}] = _d[{b}]; _d[{b}] = _t; }}",
                item = item,
                a = a,
                b = b
            )
        };
        let less = |me: &mut Self, a: &str, b: &str| {
            me.order_less(
                o,
                false,
                &get(&format!("_d[{}]", a)),
                &get(&format!("_d[{}]", b)),
                el,
                span,
            )
        };
        let m_lo = less(self, "_mid", "_lo");
        let h_lo = less(self, "_hi", "_lo");
        let m_hi = less(self, "_mid", "_hi");
        let i_hi = less(self, "_i", "_hi");
        let rest = self.partial_sort_code(item, value, "_kk + 1", o, false, el, span);
        format!(
            "int64_t _kk = {k}; if (_kk < 0 || _kk >= _n) {{ {bad}; }} int64_t _lo = 0, _hi = _n - 1; int _budget = 16; for (int64_t _q = _n; _q > 1; _q >>= 1) _budget += 2; while (_lo < _hi) {{ if (--_budget < 0) {{ {{ {rest} }} break; }} int64_t _mid = _lo + (_hi - _lo) / 2; if ({m_lo}) {s_ml} if ({h_lo}) {s_hl} if ({m_hi}) {s_mh} int64_t _st = _lo; for (int64_t _i = _lo; _i < _hi; _i++) if ({i_hi}) {{ {s_is} _st++; }} {s_sh} if (_st == _kk) break; if (_kk < _st) _hi = _st - 1; else _lo = _st + 1; }}",
            k = k,
            bad = self.on_fail(
                34,
                format!(
                    "zr_stop({}, 34, \"selectNth: the position is out of range\")",
                    loc
                )
            ),
            rest = rest,
            m_lo = m_lo,
            h_lo = h_lo,
            m_hi = m_hi,
            i_hi = i_hi,
            s_ml = swap("_mid", "_lo"),
            s_hl = swap("_hi", "_lo"),
            s_mh = swap("_mid", "_hi"),
            s_is = swap("_i", "_st"),
            s_sh = swap("_st", "_hi"),
        )
    }

    /// The smallest `top` elements of `_d[0.._n)`, in order, at its start: a max-heap of
    /// the first `top` keeps the smallest seen, O(n log top), then they are sorted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn partial_sort_code(
        &mut self,
        item: &str,
        value: &str,
        top: &str,
        o: &Order,
        desc: bool,
        el: &Ty,
        span: Span,
    ) -> String {
        let get = |x: &str| {
            if value.is_empty() {
                x.to_string()
            } else {
                format!("{}[{}]", value, x)
            }
        };
        let swap = |a: &str, b: &str| {
            format!(
                "{{ {item} _t = _d[{a}]; _d[{a}] = _d[{b}]; _d[{b}] = _t; }}",
                item = item,
                a = a,
                b = b
            )
        };
        let sift = |me: &mut Self, start: &str| {
            let child = me.order_less(o, desc, &get("_d[_c]"), &get("_d[_c + 1]"), el, span);
            let parent = me.order_less(o, desc, &get("_d[_p]"), &get("_d[_c]"), el, span);
            format!(
                "{{ int64_t _p = {start}; for (;;) {{ int64_t _c = 2 * _p + 1; if (_c >= _top) break; if (_c + 1 < _top && {child}) _c++; if (!{parent}) break; {sw} _p = _c; }} }}",
                start = start,
                child = child,
                parent = parent,
                sw = swap("_p", "_c")
            )
        };
        let heapify = sift(self, "_s");
        let resift = sift(self, "0");
        let smaller = self.order_less(o, desc, &get("_d[_r]"), &get("_d[0]"), el, span);
        let sort = self.merge_sort_code(item, value, "_top", o, desc, el, span);
        format!(
            "int64_t _top = {top}; if (_top < 0) _top = 0; if (_top > _n) _top = _n; if (_top > 0 && _top < _n) {{ for (int64_t _s = _top / 2 - 1; _s >= 0; _s--) {heapify} for (int64_t _r = _top; _r < _n; _r++) if ({smaller}) {{ {sw} {resift} }} }} {sort}",
            top = top,
            heapify = heapify,
            smaller = smaller,
            sw = swap("0", "_r"),
            resift = resift,
            sort = sort
        )
    }
}
