//! The state definite assignment tracks at one point of a function (DESIGN §6.2):
//! which variables and fields are assigned on every path to it, which on some
//! path, and whether `this.new(...)` has run.
//!
//! Each tracked variable or field has a slot number. A local declared with an
//! initializer needs none, since it is assigned from the start.

#[derive(Clone, Debug, Default)]
pub struct Flow {
    /// Assigned on every path to here.
    def: Vec<bool>,
    /// Assigned by the code on some path to here (initializers don't count).
    maybe: Vec<bool>,
    /// No path reaches here: after a `return`, `break`, `continue` or `fail(...)`.
    pub dead: bool,
    /// `this.new(...)` has run on every path to here.
    delegated: bool,
    /// `this.new(...)` has run on some path to here.
    maybe_delegated: bool,
}

fn get(v: &[bool], i: usize) -> bool {
    v.get(i).copied().unwrap_or(false)
}

fn put(v: &mut Vec<bool>, i: usize, b: bool) {
    if v.len() <= i {
        v.resize(i + 1, false);
    }
    v[i] = b;
}

impl Flow {
    /// The state where nothing reaches.
    pub fn unreachable() -> Flow {
        Flow {
            dead: true,
            ..Flow::default()
        }
    }

    /// Assigned on every path. Anything holds where nothing reaches.
    pub fn is_def(&self, slot: usize) -> bool {
        self.dead || get(&self.def, slot)
    }

    /// Assigned by the code on some path.
    pub fn is_maybe(&self, slot: usize) -> bool {
        !self.dead && get(&self.maybe, slot)
    }

    pub fn delegated(&self) -> bool {
        self.dead || self.delegated
    }

    pub fn maybe_delegated(&self) -> bool {
        !self.dead && self.maybe_delegated
    }

    /// An assignment by the code.
    pub fn assign(&mut self, slot: usize) {
        put(&mut self.def, slot, true);
        put(&mut self.maybe, slot, true);
    }

    /// Assigned from the start, by an initializer or by starting empty.
    pub fn preset(&mut self, slot: usize) {
        put(&mut self.def, slot, true);
    }

    /// `this.new(...)` ran here, assigning every field.
    pub fn delegate(&mut self, slots: impl IntoIterator<Item = usize>) {
        for s in slots {
            self.assign(s);
        }
        self.delegated = true;
        self.maybe_delegated = true;
    }

    /// The state where two paths meet.
    pub fn join(a: Flow, b: Flow) -> Flow {
        if a.dead {
            return b;
        }
        if b.dead {
            return a;
        }
        let n = a
            .def
            .len()
            .max(b.def.len())
            .max(a.maybe.len())
            .max(b.maybe.len());
        Flow {
            def: (0..n).map(|i| get(&a.def, i) && get(&b.def, i)).collect(),
            maybe: (0..n)
                .map(|i| get(&a.maybe, i) || get(&b.maybe, i))
                .collect(),
            dead: false,
            delegated: a.delegated && b.delegated,
            maybe_delegated: a.maybe_delegated || b.maybe_delegated,
        }
    }

    /// The state where several paths meet; unreachable when there are none.
    pub fn join_all(flows: impl IntoIterator<Item = Flow>) -> Flow {
        flows.into_iter().fold(Flow::unreachable(), Flow::join)
    }

    /// Adds what `other` may have assigned, as when a loop goes around again
    /// before leaving: the leaving path may have passed through `other`.
    pub fn absorb_maybe(&mut self, other: &Flow) {
        if self.dead || other.dead {
            return;
        }
        for (i, b) in other.maybe.iter().enumerate() {
            if *b {
                put(&mut self.maybe, i, true);
            }
        }
        self.maybe_delegated |= other.maybe_delegated;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_keeps_what_both_paths_assign() {
        let mut a = Flow::default();
        let mut b = Flow::default();
        a.assign(0);
        a.assign(1);
        b.assign(1);
        let j = Flow::join(a, b);
        assert!(!j.is_def(0) && j.is_maybe(0));
        assert!(j.is_def(1) && j.is_maybe(1));
        assert!(!j.is_def(2) && !j.is_maybe(2));
    }

    #[test]
    fn unreachable_paths_are_ignored() {
        let mut a = Flow::default();
        a.assign(0);
        let j = Flow::join(a.clone(), Flow::unreachable());
        assert!(j.is_def(0));
        assert!(Flow::join_all([]).dead);
        assert!(Flow::unreachable().is_def(7));
    }
}
