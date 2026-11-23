//! Compact S-expressions for syntax trees, for tests and `zink parse --tree`.
//! `a + b * c` prints as `(+ a (* b c))`.

use crate::ast::*;

pub fn ty(t: &Type) -> String {
    match &t.kind {
        TypeKind::Prim(p) => p.name().to_string(),
        TypeKind::String => "string".to_string(),
        TypeKind::Named { path, args } => {
            if args.is_empty() {
                path.text()
            } else {
                format!(
                    "{}<{}>",
                    path.text(),
                    args.iter().map(ty).collect::<Vec<_>>().join(", ")
                )
            }
        }
        TypeKind::Func { params, result } => {
            let ps = if params.is_empty() {
                "void".to_string()
            } else {
                params.iter().map(ty).collect::<Vec<_>>().join(", ")
            };
            format!("func({} => {})", ps, ret(result))
        }
        TypeKind::Array { elem, slots } => {
            let cap = match &slots.cap {
                None => String::new(),
                Some(Cap::Int(n)) => n.to_string(),
                Some(Cap::Const(q)) => q.text(),
                Some(Cap::Varint) => "*".to_string(),
                Some(Cap::Chunked) => "$".to_string(),
            };
            format!(
                "{}[{}{}]",
                ty(elem),
                slots.chars.iter().collect::<String>(),
                cap
            )
        }
        TypeKind::Slice(e) => format!("{}[:]", ty(e)),
        TypeKind::Ref { inner, link } => match link {
            None => format!("{}&", ty(inner)),
            Some(Link { width, pool: None }) => format!("{}&<{}>", ty(inner), width.name()),
            Some(Link {
                width,
                pool: Some(q),
            }) => format!("{}&<{} in {}>", ty(inner), width.name(), q.text()),
        },
        TypeKind::Nullable(i) => format!("{}?", ty(i)),
    }
}

pub fn ret(r: &ReturnType) -> String {
    match r {
        ReturnType::Void => "void".to_string(),
        ReturnType::Type(t) => ty(t),
        ReturnType::Tuple(ts) => format!("({})", ts.iter().map(ty).collect::<Vec<_>>().join(", ")),
    }
}

pub fn name(n: &Name) -> String {
    match n {
        Name::Ident(i) => i.name.clone(),
        Name::Discard(_) => "_".to_string(),
    }
}

pub fn local_type(t: &LocalType) -> String {
    match t {
        LocalType::Type(t) => ty(t),
        LocalType::Var => "var".to_string(),
        LocalType::VarRef => "var&".to_string(),
    }
}

fn binop(op: BinOp) -> &'static str {
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
        CheckAdd => "+#",
        CheckSub => "-#",
        SatAdd => "+|",
        SatSub => "-|",
        Mul => "*",
        Div => "/",
        Rem => "%",
        CheckMul => "*#",
        SatMul => "*|",
    }
}

fn assign(op: AssignOp) -> &'static str {
    use AssignOp::*;
    match op {
        Assign => "=",
        Rebind => ".=",
        Add => "+=",
        Sub => "-=",
        Mul => "*=",
        Div => "/=",
        Rem => "%=",
        And => "&=",
        Or => "|=",
        Xor => "^=",
        Shl => "<<=",
        Shr => ">>=",
        CheckAdd => "+#=",
        CheckSub => "-#=",
        CheckMul => "*#=",
        SatAdd => "+|=",
        SatSub => "-|=",
        SatMul => "*|=",
    }
}

fn args(a: &[Arg]) -> String {
    a.iter()
        .map(|a| {
            if a.marked {
                format!(" &{}", expr(&a.expr))
            } else {
                format!(" {}", expr(&a.expr))
            }
        })
        .collect()
}

fn bound(b: &Option<Bound>) -> String {
    match b {
        None => "_".to_string(),
        Some(Bound {
            from_end: true,
            expr: e,
        }) => format!("^{}", expr(e)),
        Some(Bound {
            from_end: false,
            expr: e,
        }) => expr(e),
    }
}

pub fn expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int { value, suffix } => format!("{}{}", value, suffix.map_or("", |p| p.name())),
        ExprKind::Float { text, suffix } => format!("{}{}", text, suffix.map_or("", |p| p.name())),
        ExprKind::Char(c) => format!("'{}'", char::from_u32(*c).unwrap_or('?')),
        ExprKind::Str(parts) => {
            let mut s = String::from("\"");
            for p in parts {
                match p {
                    StrPart::Text(t) => s.push_str(t),
                    StrPart::Interp(e) => s.push_str(&format!("${{{}}}", expr(e))),
                }
            }
            s.push('"');
            s
        }
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Null => "null".to_string(),
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Discard => "_".to_string(),
        ExprKind::This => "this".to_string(),
        ExprKind::ThisNew(a) => format!("(this.new{})", args(a)),
        ExprKind::PrimMember { prim, name } => format!("{}.{}", prim.name(), name.name),
        ExprKind::New { class, args: a } => {
            format!(
                "(new {}{})",
                class.as_ref().map_or("_".to_string(), ty),
                args(a)
            )
        }
        ExprKind::ArrayLit(es) => {
            format!("[{}]", es.iter().map(expr).collect::<Vec<_>>().join(" "))
        }
        ExprKind::Switch(s) => format!("(switch {} {} rules)", expr(&s.scrutinee), s.rules.len()),
        ExprKind::Paren(e) => expr(e),
        ExprKind::Tuple(es) => format!(
            "(tuple {})",
            es.iter().map(expr).collect::<Vec<_>>().join(" ")
        ),
        ExprKind::Lambda { params, body } => {
            let ps = params
                .iter()
                .map(|p| format!("{} {}", ty(&p.ty), name(&p.name)))
                .collect::<Vec<_>>()
                .join(", ");
            let b = match body.as_ref() {
                LambdaBody::Expr(e) => expr(e),
                LambdaBody::Block(b) => format!("{{{} stmts}}", b.stmts.len()),
            };
            format!("(lambda ({}) {})", ps, b)
        }
        ExprKind::Assign {
            op,
            target,
            capture,
            value,
        } => {
            let cap = capture.as_ref().map_or(String::new(), capture_text);
            format!("({}{} {} {})", assign(*op), cap, expr(target), expr(value))
        }
        ExprKind::StatusAssign { capture, value } => {
            let dest = match (&capture.one, &capture.deep, &capture.trail) {
                (Some(n), None, None) => name(n),
                _ => capture_text(capture).trim_start().to_string(),
            };
            format!("(@= {} {})", dest, expr(value))
        }
        ExprKind::Cond { cond, then, els } => {
            format!("(? {} {} {})", expr(cond), expr(then), expr(els))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", binop(*op), expr(lhs), expr(rhs))
        }
        ExprKind::InstanceOf {
            expr: e,
            ty: t,
            bind,
        } => match bind {
            Some(b) => format!("(instanceof {} {} {})", expr(e), ty(t), b.name),
            None => format!("(instanceof {} {})", expr(e), ty(t)),
        },
        ExprKind::Unary { op, expr: e } => {
            let o = match op {
                UnaryOp::Plus => "+u",
                UnaryOp::Neg => "-u",
                UnaryOp::Not => "!",
                UnaryOp::BitNot => "~",
                UnaryOp::PreInc => "++pre",
                UnaryOp::PreDec => "--pre",
            };
            format!("({} {})", o, expr(e))
        }
        ExprKind::StatusRead { expr: e, deep } => {
            format!("({} {})", if *deep { "@@" } else { "@" }, expr(e))
        }
        ExprKind::Cast {
            ty: t,
            mode,
            expr: e,
        } => {
            let target = match t {
                CastTarget::Prim(p) => p.name().to_string(),
                CastTarget::Param(i) => i.name.clone(),
            };
            let m = match mode {
                CastMode::Checked => "",
                CastMode::Wrap => "%",
                CastMode::Saturate => "|",
            };
            format!("(cast {}{} {})", target, m, expr(e))
        }
        ExprKind::Postfix { op, expr: e } => {
            let o = match op {
                PostfixOp::Inc => "post++",
                PostfixOp::Dec => "post--",
                PostfixOp::RemoveEnd => "->",
                PostfixOp::RemoveFront => "-<",
                PostfixOp::RemoveBefore => "-^",
                PostfixOp::RemoveAfter => "^-",
            };
            format!("({} {})", o, expr(e))
        }
        ExprKind::Member { expr: e, name: n } => format!("{}.{}", expr(e), n.name),
        ExprKind::Call { callee, args: a } => format!("(call {}{})", expr(callee), args(a)),
        ExprKind::Index {
            expr: e,
            mode,
            index,
        } => {
            let m = match mode {
                IndexMode::Checked => "index",
                IndexMode::Wrap => "index%",
                IndexMode::Clamp => "index|",
                IndexMode::FromEnd => "index^",
            };
            format!("({} {} {})", m, expr(e), expr(index))
        }
        ExprKind::Slice { expr: e, lo, hi } => {
            format!("(slice {} {} {})", expr(e), bound(lo), bound(hi))
        }
    }
}

/// Status destinations, each after a space: ` @a @@b @@@t`.
fn capture_text(c: &Capture) -> String {
    let mut out = String::new();
    for (ats, n) in [("@", &c.one), ("@@", &c.deep), ("@@@", &c.trail)] {
        if let Some(n) = n {
            out.push_str(&format!(" {}{}", ats, name(n)));
        }
    }
    out
}

/// The kind of a statement, with its main parts, on one line.
pub fn stmt(s: &Stmt) -> String {
    match &s.kind {
        StmtKind::Block(b) => format!("(block {})", b.stmts.len()),
        StmtKind::Local(d) => {
            let decls = d
                .decls
                .iter()
                .map(|v| {
                    let cap = v.capture.as_ref().map_or(String::new(), capture_text);
                    let init = v
                        .init
                        .as_ref()
                        .map_or(String::new(), |e| format!(" = {}", expr(e)));
                    format!("{}{}{}", v.name.name, cap, init)
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("(local {} {})", local_type(&d.ty), decls)
        }
        StmtKind::If { cond, els, .. } => format!(
            "(if {}{})",
            expr(cond),
            if els.is_some() { " else" } else { "" }
        ),
        StmtKind::While { cond, .. } => format!("(while {})", expr(cond)),
        StmtKind::Repeat {
            target,
            source,
            capture,
            ..
        } => {
            let t = match target {
                LoopTarget::Single(lt, n) => format!("{} {}", local_type(lt), name(n)),
                LoopTarget::Tuple(items) => format!(
                    "({})",
                    items
                        .iter()
                        .map(|i| match i {
                            LoopItem::Decl(lt, n) => format!("{} {}", local_type(lt), name(n)),
                            LoopItem::Discard(_) => "_".to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            let cap = capture.as_ref().map_or(String::new(), capture_text);
            format!("(repeat {} : {}{})", t, expr(source), cap)
        }
        StmtKind::DoWhile { cond, .. } => format!("(do-while {})", expr(cond)),
        StmtKind::For { cond, .. } => {
            format!("(for {})", cond.as_ref().map_or("_".to_string(), expr))
        }
        StmtKind::ForEach {
            ty: t,
            name: n,
            iter,
            ..
        } => format!("(foreach {} {} : {})", local_type(t), name(n), expr(iter)),
        StmtKind::Switch(s) => format!("(switch {} {} rules)", expr(&s.scrutinee), s.rules.len()),
        StmtKind::Break(l) => format!(
            "(break{})",
            l.as_ref().map_or(String::new(), |l| format!(" {}", l.name))
        ),
        StmtKind::Continue(l) => format!(
            "(continue{})",
            l.as_ref().map_or(String::new(), |l| format!(" {}", l.name))
        ),
        StmtKind::Return { status, value } => {
            let st = match status {
                None => String::new(),
                Some(StatusWrite::Int(n)) => format!(" #{}", n),
                Some(StatusWrite::Name(q)) => format!(" #{}", q.text()),
                Some(StatusWrite::Expr(e)) => format!(" #({})", expr(e)),
                Some(StatusWrite::Pass) => " ##".to_string(),
            };
            format!(
                "(return{}{})",
                st,
                value
                    .as_ref()
                    .map_or(String::new(), |e| format!(" {}", expr(e)))
            )
        }
        StmtKind::Destructure {
            targets,
            capture,
            value,
        } => {
            let ts = targets
                .iter()
                .map(|t| match t {
                    Target::Decl(lt, n) => format!("{} {}", local_type(lt), name(n)),
                    Target::Existing(n) => name(n),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let cap = capture.as_ref().map_or(String::new(), capture_text);
            format!("(destructure ({}){} {})", ts, cap, expr(value))
        }
        StmtKind::Defer(s) => format!("(defer {})", stmt(s)),
        StmtKind::Yield(e) => format!("(yield {})", expr(e)),
        StmtKind::Labeled { label, stmt: s } => format!("(label {} {})", label.name, stmt(s)),
        StmtKind::Expr(e) => expr(e),
        StmtKind::If_(b) => format!(
            "(#if {} {})",
            b.then.len(),
            b.els.as_ref().map_or(0, |e| e.len())
        ),
        StmtKind::Empty => "(empty)".to_string(),
    }
}
