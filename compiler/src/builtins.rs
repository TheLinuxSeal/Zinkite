//! Signatures of the built-in functions and methods the checker models:
//! `zink.lang`'s functions, `status`, `math`, and the methods on numbers,
//! arrays, strings, `hashmap`, `hashset` and `pool` (STDLIB §2, §3, §7, §8.1).

use crate::token::Prim;
use crate::types::*;

/// A requirement on a type that the checker verifies after choosing an overload.
#[derive(Clone, Debug, PartialEq)]
pub enum Need {
    Bound(Ty, BoundKind),
    /// A radix sort key: a number or a string.
    Radix(Ty),
}

/// Where a signature comes from, so later passes know what a call called.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// Built in: the method is known by its name and its receiver's type.
    Builtin,
    /// `classes[c].methods[i]`
    Method(ClassId, usize),
    /// `classes[c].ctors[i]`
    Ctor(ClassId, usize),
    /// A call through a `func` value.
    Value,
    /// A C function from an `extern import`ed header, by its C name (DESIGN §12.6).
    Extern(String),
}

/// One callable signature: a method, function or constructor.
#[derive(Clone, Debug)]
pub struct Sig {
    pub origin: Origin,
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub tparams: Vec<TParam>,
    /// For a generic call, the type arguments inference chose, in `tparams` order.
    pub targs: Vec<Ty>,
    pub can_fail: bool,
    pub is_mut: bool,
    pub needs: Vec<Need>,
}

impl Sig {
    pub fn new(name: &str, params: Vec<Ty>, ret: Ty) -> Sig {
        Sig {
            origin: Origin::Builtin,
            name: name.to_string(),
            params,
            ret,
            tparams: Vec::new(),
            targs: Vec::new(),
            can_fail: false,
            is_mut: false,
            needs: Vec::new(),
        }
    }

    fn fails(mut self) -> Sig {
        self.can_fail = true;
        self
    }

    fn fails_if(mut self, b: bool) -> Sig {
        self.can_fail = b;
        self
    }

    fn muts(mut self) -> Sig {
        self.is_mut = true;
        self
    }

    fn generic(mut self, tps: Vec<TParam>) -> Sig {
        self.tparams = tps;
        self
    }

    fn need(mut self, n: Need) -> Sig {
        self.needs.push(n);
        self
    }
}

fn tp(name: &str, k: BoundKind) -> TParam {
    TParam {
        name: name.to_string(),
        bounds: vec![Bound::Builtin(k)],
    }
}

fn param(name: &str) -> Ty {
    Ty::Param(name.to_string())
}

fn func(params: Vec<Ty>, ret: Ty) -> Ty {
    Ty::Func(params, Box::new(ret))
}

fn text() -> Ty {
    Ty::Array(Box::new(Ty::u8()), ArrayKind::frozen())
}

fn small_text() -> Ty {
    small_text_of(16)
}

fn small_text_of(size: u64) -> Ty {
    Ty::Array(
        Box::new(Ty::u8()),
        ArrayKind {
            start: '.',
            cursor: '.',
            end: '~',
            cap: CapKind::Capped,
            size,
        },
    )
}

/// What `hex`, `oct` and `bin` give: a capped array as wide as the type's digits
/// (DESIGN §4.13), 32 for a `u128` in hex, 128 in binary.
fn radix_text(me: &Ty, name: &str) -> Ty {
    let bits = match me {
        Ty::Prim(p) => crate::types::bits(*p) as u64,
        _ => 128,
    };
    let per = match name {
        "hex" => 4,
        "oct" => 3,
        _ => 1,
    };
    small_text_of(bits.div_ceil(per))
}

fn grow(e: Ty) -> Ty {
    Ty::Array(Box::new(e), ArrayKind::grow())
}

fn s() -> Ty {
    Ty::string()
}

fn i64t() -> Ty {
    Ty::i64()
}

/// Names that stay `Unknown` when a file imports an unmodelled library package:
/// the lowercase namespaces those packages provide.
pub const LIBRARY_NAMESPACES: &[&str] = &[
    "env", "fs", "path", "process", "term", "ansi", "random", "hash", "hex", "base64", "time",
    "clock",
];

/// `zink.lang`'s functions.
pub fn global(name: &str) -> Vec<Sig> {
    use BoundKind::*;
    let t = || param("#T");
    match name {
        "print" | "println" | "errprint" | "errprintln" => {
            vec![Sig::new(name, vec![s()], Ty::Void)]
        }
        "min" | "max" => vec![Sig::new(name, vec![t(), t()], t()).generic(vec![tp("#T", Ordered)])],
        // INVALID, and 0, when lo > hi.
        "clamp" => vec![
            Sig::new(name, vec![t(), t(), t()], t())
                .generic(vec![tp("#T", Ordered)])
                .fails(),
        ],
        "abs" => vec![Sig::new(name, vec![t()], t()).generic(vec![tp("#T", Number)])],
        _ => Vec::new(),
    }
}

pub fn status_fn(name: &str) -> Vec<Sig> {
    match name {
        "message" | "name" => vec![Sig::new(name, vec![Ty::u8()], s())],
        _ => Vec::new(),
    }
}

pub fn math_fn(name: &str) -> Vec<Sig> {
    use BoundKind::*;
    let f = || param("#F");
    let t = || param("#T");
    match name {
        "sqrt" | "cbrt" | "exp" | "log" | "log2" | "log10" | "sin" | "cos" | "tan" | "asin"
        | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "floor" | "ceil" | "trunc" | "round" => {
            vec![Sig::new(name, vec![f()], f()).generic(vec![tp("#F", Float)])]
        }
        "pow" | "hypot" | "atan2" => {
            vec![Sig::new(name, vec![f(), f()], f()).generic(vec![tp("#F", Float)])]
        }
        "gcd" | "lcm" => vec![Sig::new(name, vec![t(), t()], t()).generic(vec![tp("#T", Int)])],
        "ipow" => vec![Sig::new(name, vec![t(), i64t()], t()).generic(vec![tp("#T", Int)])],
        "isqrt" => vec![Sig::new(name, vec![t()], t()).generic(vec![tp("#T", Int)])],
        "roundToI64" => vec![Sig::new(name, vec![Ty::Prim(Prim::F64)], i64t()).fails()],
        _ => Vec::new(),
    }
}

pub fn math_const(name: &str) -> Option<Ty> {
    matches!(name, "PI" | "TAU" | "E").then_some(Ty::Prim(Prim::F64))
}

/// What a number type, or a type parameter with number bounds, can do.
#[derive(Clone, Copy)]
pub struct NumCaps {
    pub int: bool,
    pub float: bool,
    pub number: bool,
    pub ordered: bool,
    pub is_u8: bool,
    pub is_u32: bool,
}

impl NumCaps {
    pub fn of_prim(p: Prim) -> NumCaps {
        NumCaps {
            int: p.is_int(),
            float: p.is_float(),
            number: p.is_int() || p.is_float(),
            ordered: true,
            is_u8: p == Prim::U8,
            is_u32: p == Prim::U32,
        }
    }

    pub fn of_param(tp: &TParam) -> NumCaps {
        NumCaps {
            int: tp.has(BoundKind::Int),
            float: tp.has(BoundKind::Float),
            number: tp.has(BoundKind::Number),
            ordered: tp.has(BoundKind::Ordered),
            is_u8: false,
            is_u32: false,
        }
    }
}

/// Methods on a value of a number type `me` (DESIGN §4.13, STDLIB §2.4, §3.5, §8.1).
pub fn number_method(me: &Ty, caps: NumCaps, name: &str) -> Vec<Sig> {
    let t = || me.clone();
    let b = Ty::bool;
    let one = |ok: bool, sig: Sig| if ok { vec![sig] } else { Vec::new() };
    match name {
        "hex" | "bin" | "oct" => one(caps.int, Sig::new(name, vec![], radix_text(me, name))),
        "pad" | "zeroPad" => one(caps.number, Sig::new(name, vec![i64t()], text())),
        "fixed" | "sci" => one(caps.float, Sig::new(name, vec![i64t()], text())),
        "utf8" => one(caps.is_u32, Sig::new(name, vec![], small_text()).fails()),
        "isNaN" | "isFinite" => one(caps.float, Sig::new(name, vec![], b())),
        "divOr" => one(caps.int, Sig::new(name, vec![t(), t()], t())),
        "popCount" | "leadingZeros" | "trailingZeros" => {
            one(caps.int, Sig::new(name, vec![], i64t()))
        }
        "rotateLeft" | "rotateRight" => one(caps.int, Sig::new(name, vec![i64t()], t())),
        "reverseBytes" => one(caps.int, Sig::new(name, vec![], t())),
        "isPowerOf2" => one(caps.int, Sig::new(name, vec![], b())),
        "compareTo" => one(caps.ordered, Sig::new(name, vec![t()], Ty::Prim(Prim::I32))),
        "isDigit" | "isHexDigit" | "isAlpha" | "isAlnum" | "isSpace" | "isUpper" | "isLower"
        | "isPrint" => one(caps.is_u8, Sig::new(name, vec![], b())),
        "toUpper" | "toLower" => one(caps.is_u8, Sig::new(name, vec![], Ty::u8())),
        _ => Vec::new(),
    }
}

/// Functions called on a number type itself: `i64.parse(s)`, `u16.from(n)`.
pub fn number_static(me: &Ty, caps: NumCaps, name: &str) -> Vec<Sig> {
    let t = || me.clone();
    match name {
        "parse" => {
            let mut v = vec![Sig::new(name, vec![s()], t()).fails()];
            if caps.int {
                v.push(Sig::new(name, vec![s(), i64t()], t()).fails());
            }
            if caps.number { v } else { Vec::new() }
        }
        "parsePrefix" if caps.number => {
            vec![Sig::new(name, vec![s()], Ty::Tuple(vec![t(), s()])).fails()]
        }
        "from" if caps.number => {
            vec![
                Sig::new(name, vec![param("#S")], t())
                    .fails()
                    .generic(vec![tp("#S", BoundKind::Number)]),
            ]
        }
        _ => Vec::new(),
    }
}

/// Constants on a number type: `u64.MAX`, `f64.NAN`.
pub fn number_const(caps: NumCaps, name: &str) -> bool {
    match name {
        "MIN" | "MAX" => caps.int,
        "NAN" | "INFINITY" | "EPSILON" => caps.float,
        _ => false,
    }
}

/// How an array-like receiver can be changed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SeqKind {
    Array(ArrayKind),
    /// `T[:]`: read-only.
    Slice,
    /// `T[:]&`: elements can be written, the length can't change.
    View,
}

/// Methods on arrays and slices (DESIGN §4.7, §4.9, STDLIB §3, §7.1, §7.2).
/// `Err` says what the receiver lacks for a method that exists.
pub fn seq_method(elem: &Ty, kind: SeqKind, name: &str) -> Result<Vec<Sig>, String> {
    use BoundKind::*;
    let e = || elem.clone();
    let arr = match kind {
        SeqKind::Array(k) => Some(k),
        _ => None,
    };
    let capped = arr.is_some_and(|k| k.cap == CapKind::Capped);
    let less = || func(vec![e(), e()], Ty::bool());
    let pred = || func(vec![e()], Ty::bool());
    let key = || func(vec![e()], param("#K"));
    let kord = || vec![tp("#K", Ordered)];
    let ord = || Need::Bound(e(), Ordered);
    let editable = |what: &str| -> Result<(), String> {
        match kind {
            SeqKind::Slice => Err(format!(
                "an array or a writable view (`T[:]&`), since it {}",
                what
            )),
            _ => Ok(()),
        }
    };
    let cap = |ok: fn(&ArrayKind) -> bool, desc: &str| -> Result<(), String> {
        match arr {
            Some(k) if ok(&k) => Ok(()),
            Some(_) => Err(desc.to_string()),
            None => Err(format!("{}, and a slice can't change its length", desc)),
        }
    };
    const END_IN: &str = "an array that can insert at the end (`>` or `~` as its last slot)";
    const END_OUT: &str = "an array that can remove at the end (`<` or `~` as its last slot)";
    const FRONT_IN: &str = "an array that can insert at the start (`>` or `~` as its first slot)";
    const FRONT_OUT: &str = "an array that can remove at the start (`<` or `~` as its first slot)";
    const CUR_IN: &str = "an array that can insert at a cursor (`>` or `~` as its middle slot)";
    const CUR_OUT: &str = "an array that can remove at a cursor (`<` or `~` as its middle slot)";
    const CURSOR: &str = "a cursor (a middle slot other than `.`)";
    const EDIT: &str = "an array that can insert or remove";

    // Text methods, on anything that converts to a `string`.
    let is_text = *elem == Ty::u8() && arr.is_none_or(|k| k.sliceable());
    if is_text {
        let v = text_method(name);
        if !v.is_empty() {
            return Ok(v);
        }
        if name == "append" {
            cap(|k| k.push_end(), END_IN)?;
            return Ok(vec![
                Sig::new(name, vec![s()], Ty::Void).fails_if(capped).muts(),
            ]);
        }
        if name == "appendUtf8" {
            cap(|k| k.push_end(), END_IN)?;
            return Ok(vec![
                Sig::new(
                    name,
                    vec![Ty::Slice(Box::new(Ty::Prim(Prim::U32))), i64t(), i64t()],
                    Ty::Void,
                )
                .fails_if(capped)
                .muts(),
            ]);
        }
    }
    if elem.is_string() && name == "join" {
        return Ok(vec![Sig::new(name, vec![s()], text())]);
    }
    if *elem == Ty::Prim(Prim::U32) && name == "utf8" {
        return Ok(vec![Sig::new(name, vec![], text()).fails()]);
    }

    let one = |sig: Sig| Ok(vec![sig]);
    match name {
        "push" => {
            cap(|k| k.push_end(), END_IN)?;
            one(Sig::new(name, vec![e()], Ty::Ref(Box::new(e())))
                .fails_if(capped)
                .muts())
        }
        "pushFront" => {
            cap(|k| k.push_front(), FRONT_IN)?;
            one(Sig::new(name, vec![e()], Ty::Ref(Box::new(e())))
                .fails_if(capped)
                .muts())
        }
        "insert" => {
            cap(|k| k.cursor_insert(), CUR_IN)?;
            one(Sig::new(name, vec![e()], Ty::Ref(Box::new(e())))
                .fails_if(capped)
                .muts())
        }
        "pop" => {
            cap(|k| k.pop_end(), END_OUT)?;
            one(Sig::new(name, vec![], e()).fails().muts())
        }
        "popFront" => {
            cap(|k| k.pop_front(), FRONT_OUT)?;
            one(Sig::new(name, vec![], e()).fails().muts())
        }
        "removeBefore" | "removeAfter" => {
            cap(|k| k.cursor_remove(), CUR_OUT)?;
            one(Sig::new(name, vec![], e()).fails().muts())
        }
        "clear" => {
            cap(
                |k| k.is_editable() || k.cap == CapKind::Capped && !k.is_frozen(),
                EDIT,
            )?;
            one(Sig::new(name, vec![], Ty::Void).muts())
        }
        "seek" | "seekFromEnd" => {
            cap(|k| k.has_cursor(), CURSOR)?;
            one(Sig::new(name, vec![i64t()], Ty::Void).fails().muts())
        }
        "seekStart" => {
            cap(|k| k.has_cursor(), CURSOR)?;
            one(Sig::new(name, vec![], Ty::Void).muts())
        }
        "move" => {
            cap(|k| k.has_cursor(), CURSOR)?;
            one(Sig::new(name, vec![i64t()], Ty::Void).muts())
        }
        "insertAt" => {
            cap(|k| k.cursor_insert(), CUR_IN)?;
            one(Sig::new(name, vec![i64t(), e()], Ty::Ref(Box::new(e())))
                .fails()
                .muts())
        }
        "removeAt" => {
            cap(|k| k.cursor_remove(), CUR_OUT)?;
            one(Sig::new(name, vec![i64t()], e()).fails().muts())
        }
        "getOr" => one(Sig::new(name, vec![i64t(), e()], e())),

        // Sorting (STDLIB §7.1).
        "sort" | "stableSort" | "sortDescending" => {
            editable("changes the order of the elements")?;
            let mut v = vec![Sig::new(name, vec![], Ty::Void).need(ord()).muts()];
            if name != "sortDescending" {
                v.push(Sig::new(name, vec![less()], Ty::Void).muts());
            }
            Ok(v)
        }
        "sortBy" | "stableSortBy" | "sortByDescending" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![key()], Ty::Void).generic(kord()).muts())
        }
        "radixSort" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![], Ty::Void)
                .need(Need::Radix(e()))
                .muts())
        }
        "radixSortBy" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![func(vec![e()], param("#K"))], Ty::Void)
                .generic(vec![TParam {
                    name: "#K".into(),
                    bounds: vec![],
                }])
                .need(Need::Radix(param("#K")))
                .muts())
        }
        "partialSort" => {
            editable("changes the order of the elements")?;
            Ok(vec![
                Sig::new(name, vec![i64t()], Ty::Void).need(ord()).muts(),
                Sig::new(name, vec![i64t(), less()], Ty::Void).muts(),
            ])
        }
        "partialSortBy" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![i64t(), key()], Ty::Void)
                .generic(kord())
                .muts())
        }
        "selectNth" => {
            editable("changes the order of the elements")?;
            let r = Ty::Ref(Box::new(e()));
            Ok(vec![
                Sig::new(name, vec![i64t()], r.clone()).need(ord()).muts(),
                Sig::new(name, vec![i64t(), less()], r).muts(),
            ])
        }
        "selectNthBy" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![i64t(), key()], Ty::Ref(Box::new(e())))
                .generic(kord())
                .muts())
        }
        "sortedIndexes" => Ok(vec![
            Sig::new(name, vec![], grow(i64t())).need(ord()),
            Sig::new(name, vec![less()], grow(i64t())),
        ]),
        "sortedIndexesBy" => one(Sig::new(name, vec![key()], grow(i64t())).generic(kord())),
        "isSorted" => Ok(vec![
            Sig::new(name, vec![], Ty::bool()).need(ord()),
            Sig::new(name, vec![less()], Ty::bool()),
        ]),
        "isSortedBy" => one(Sig::new(name, vec![key()], Ty::bool()).generic(kord())),

        // In place.
        "reverse" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![], Ty::Void).muts())
        }
        "shuffle" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![Ty::Unknown], Ty::Void).muts())
        }
        "swap" => {
            editable("changes the order of the elements")?;
            one(Sig::new(name, vec![i64t(), i64t()], Ty::Void).muts())
        }
        "fill" => {
            editable("writes the elements")?;
            one(Sig::new(name, vec![e()], Ty::Void).muts())
        }

        // Searching.
        "indexOf" => one(Sig::new(name, vec![e()], i64t()).fails()),
        "contains" => one(Sig::new(name, vec![e()], Ty::bool())),
        "findIndex" => one(Sig::new(name, vec![pred()], i64t()).fails()),
        "any" | "all" => one(Sig::new(name, vec![pred()], Ty::bool())),
        "count" => one(Sig::new(name, vec![pred()], i64t())),
        "binarySearch" => one(Sig::new(name, vec![e()], i64t()).fails().need(ord())),
        "lowerBound" | "upperBound" => one(Sig::new(name, vec![e()], i64t()).need(ord())),
        "binarySearchBy" => one(Sig::new(name, vec![param("#K"), key()], i64t())
            .fails()
            .generic(kord())),
        "lowerBoundBy" | "upperBoundBy" => {
            one(Sig::new(name, vec![param("#K"), key()], i64t()).generic(kord()))
        }

        // New values.
        "filter" => one(Sig::new(name, vec![pred()], grow(e()))),
        "map" => one(
            Sig::new(name, vec![func(vec![e()], param("#U"))], grow(param("#U"))).generic(vec![
                TParam {
                    name: "#U".into(),
                    bounds: vec![],
                },
            ]),
        ),
        "fold" => one(Sig::new(
            name,
            vec![param("#A"), func(vec![param("#A"), e()], param("#A"))],
            param("#A"),
        )
        .generic(vec![TParam {
            name: "#A".into(),
            bounds: vec![],
        }])),
        "sum" => one(Sig::new(name, vec![], e()).need(Need::Bound(e(), Number))),
        "min" | "max" => one(Sig::new(name, vec![], e()).fails().need(ord())),
        "minIndexBy" | "maxIndexBy" => {
            one(Sig::new(name, vec![key()], i64t()).fails().generic(kord()))
        }

        // Editing.
        "pushAll" => {
            cap(|k| k.push_end(), END_IN)?;
            one(Sig::new(name, vec![Ty::Slice(Box::new(e()))], Ty::Void)
                .fails_if(capped)
                .muts())
        }
        "retain" => {
            cap(|k| k.pop_end(), END_OUT)?;
            one(Sig::new(name, vec![pred()], Ty::Void).muts())
        }
        "dedup" => {
            cap(|k| k.pop_end(), END_OUT)?;
            one(Sig::new(name, vec![], Ty::Void).muts())
        }
        "truncate" => {
            cap(|k| k.pop_end(), END_OUT)?;
            one(Sig::new(name, vec![i64t()], Ty::Void).muts())
        }

        // `chunks` and `windows` are `zink.lang` classes (check.rs, `text_iterator`).

        // Heaps (STDLIB §7.2).
        "heapPush" => {
            cap(
                |k| k.push_end() && k.pop_end(),
                "an array that can insert and remove at the end (`~` as its last slot)",
            )?;
            one(Sig::new(name, vec![e(), less()], Ty::Void)
                .fails_if(capped)
                .muts())
        }
        "heapPop" => {
            cap(
                |k| k.push_end() && k.pop_end(),
                "an array that can insert and remove at the end (`~` as its last slot)",
            )?;
            one(Sig::new(name, vec![less()], e()).fails().muts())
        }
        "heapify" => {
            cap(
                |k| k.push_end() && k.pop_end(),
                "an array that can insert and remove at the end (`~` as its last slot)",
            )?;
            one(Sig::new(name, vec![less()], Ty::Void).muts())
        }
        _ => Ok(Vec::new()),
    }
}

/// Methods on text (STDLIB §3), with the receiver as a `string`.
fn text_method(name: &str) -> Vec<Sig> {
    let pair = || Ty::Tuple(vec![s(), s()]);
    let one = |sig: Sig| vec![sig];
    match name {
        "find" => vec![
            Sig::new(name, vec![s()], i64t()).fails(),
            Sig::new(name, vec![s(), i64t()], i64t()).fails(),
        ],
        "rfind" | "findAny" => one(Sig::new(name, vec![s()], i64t()).fails()),
        // `contains` on bytes is also the array method; both are offered.
        "contains" => vec![
            Sig::new(name, vec![s()], Ty::bool()),
            Sig::new(name, vec![Ty::u8()], Ty::bool()),
        ],
        "startsWith" | "endsWith" | "equalsIgnoreCase" => {
            one(Sig::new(name, vec![s()], Ty::bool()))
        }
        "count" => one(Sig::new(name, vec![s()], i64t())),
        "split" => one(Sig::new(name, vec![s()], grow(s()))),
        // `splitBy`, `lines`, `words` and `codePoints` are `zink.lang` classes (check.rs,
        // `text_iterator`).
        "splitFirst" | "splitLast" => one(Sig::new(name, vec![s()], pair())),
        "trim" | "trimStart" | "trimEnd" => one(Sig::new(name, vec![], s())),
        "trimPrefix" | "trimSuffix" => one(Sig::new(name, vec![s()], s())),
        "compareTo" | "compareNatural" => one(Sig::new(name, vec![s()], Ty::Prim(Prim::I32))),
        "toUpper" | "toLower" => one(Sig::new(name, vec![], text())),
        "replaceAll" | "replaceFirst" => one(Sig::new(name, vec![s(), s()], text())),
        "repeat" | "pad" | "padEnd" => one(Sig::new(name, vec![i64t()], text())),
        "isValidUtf8" => one(Sig::new(name, vec![], Ty::bool())),
        // The bytes as hex (STDLIB §8.4); on an integer, `hex` formats its value.
        "hex" => one(Sig::new(name, vec![], text())),
        "codePointCount" => one(Sig::new(name, vec![], i64t())),
        _ => Vec::new(),
    }
}

/// Methods on `hashmap<K, V>` (DESIGN §4.14, STDLIB §7.3).
pub fn map_method(k: &Ty, v: &Ty, name: &str) -> Vec<Sig> {
    let (k, v) = (k.clone(), v.clone());
    match name {
        "get" => vec![Sig::new(name, vec![k], v).fails()],
        "getOrAdd" => vec![Sig::new(name, vec![k, v.clone()], Ty::Ref(Box::new(v))).muts()],
        "getOr" => vec![Sig::new(name, vec![k, v.clone()], v)],
        "put" => vec![Sig::new(name, vec![k, v], Ty::Void).muts()],
        "remove" => vec![Sig::new(name, vec![k], Ty::Void).fails().muts()],
        "has" => vec![Sig::new(name, vec![k], Ty::bool())],
        "clear" => vec![Sig::new(name, vec![], Ty::Void).muts()],
        _ => Vec::new(),
    }
}

/// Methods on `hashset<T>` (STDLIB §7.4).
pub fn set_method(e: &Ty, name: &str) -> Vec<Sig> {
    let e = e.clone();
    match name {
        "add" => vec![Sig::new(name, vec![e], Ty::bool()).muts()],
        "has" => vec![Sig::new(name, vec![e], Ty::bool())],
        "remove" => vec![Sig::new(name, vec![e], Ty::Void).fails().muts()],
        "clear" => vec![Sig::new(name, vec![], Ty::Void).muts()],
        _ => Vec::new(),
    }
}

/// Methods on `pool<T>` (STDLIB §7.5).
pub fn pool_method(e: &Ty, name: &str) -> Vec<Sig> {
    let h = Ty::Handle(Box::new(e.clone()));
    match name {
        "add" => vec![Sig::new(name, vec![e.clone()], h).muts()],
        "get" => vec![Sig::new(name, vec![h], Ty::Ref(Box::new(e.clone()))).fails()],
        "free" => vec![Sig::new(name, vec![h], Ty::Void).fails().muts()],
        "clear" => vec![Sig::new(name, vec![], Ty::Void).muts()],
        _ => Vec::new(),
    }
}

/// Methods on a `u8` array that take it as a `string`, converting it to a slice: a capped
/// ring or gap buffer is made one block for them (DESIGN §4.10). The code generator and
/// the lifetimes pass both follow this list.
pub fn slice_receiver(name: &str, first_param_is_string: bool) -> bool {
    matches!(
        name,
        "trim"
            | "isValidUtf8"
            | "trimStart"
            | "trimEnd"
            | "startsWith"
            | "endsWith"
            | "find"
            | "rfind"
            | "count"
            | "splitFirst"
            | "splitLast"
            | "compareTo"
            | "compareNatural"
            | "trimPrefix"
            | "trimSuffix"
            | "equalsIgnoreCase"
            | "lines"
            | "words"
            | "splitBy"
            | "codePoints"
    ) || (name == "contains" && first_param_is_string)
}

/// The named status codes (DESIGN §7.7, STDLIB §1.4), with their `errno` aliases; the
/// runtime's `zr_status_name` is the same list.
pub fn status_code(name: &str) -> Option<u8> {
    Some(match name {
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
        "DOMAIN" | "EDOM" => 33,
        "RANGE" | "ERANGE" => 34,
        "NAME_TOO_LONG" | "ENAMETOOLONG" => 36,
        "NOT_EMPTY" | "ENOTEMPTY" => 39,
        "LOOP" | "ELOOP" => 40,
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
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    /// The runtime names each code as the compiler does, for the message when nothing
    /// reads a failure.
    #[test]
    fn the_runtime_names_statuses_as_the_compiler_does() {
        let rt = include_str!("../runtime/zink_rt.h");
        let start = rt
            .find("static const char *zr_status_name(uint8_t s) {")
            .unwrap();
        let body = &rt[start..start + rt[start..].find("\n}\n").unwrap()];
        let mut n = 0;
        for line in body.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("case ") else {
                continue;
            };
            let (code, name) = rest.split_once(": return \"").unwrap();
            let name = name.trim_end_matches("\";");
            assert_eq!(super::status_code(name), code.parse().ok(), "{}", name);
            n += 1;
        }
        assert_eq!(n, 33);
    }
}
