//! Allocation-free preflight over Alloy's generated token types. This admits
//! only contiguous canonical tails before the ordinary protocol decoder runs.
//! Shared/overlapping tails can otherwise amplify nested Vec allocations despite
//! a bounded RPC response. Rejected layouts remain unverified method evidence.
use alloy_sol_types::{
    SolCall,
    abi::token::{DynSeqToken, FixedSeqToken, PackedSeqToken, WordToken},
};
use eez_protocol::abi::postAndVerifyBatchCall;

struct Budget<'a> {
    data: &'a [u8],
    fuel: usize,
}
impl Budget<'_> {
    fn enter(&mut self) -> Option<()> {
        self.fuel = self.fuel.checked_sub(1)?;
        Some(())
    }
    fn end(&self, start: usize, len: usize) -> Option<usize> {
        let end = start.checked_add(len)?;
        self.data.get(start..end)?;
        Some(end)
    }
    fn word(&self, start: usize) -> Option<usize> {
        self.data
            .get(start..self.end(start, 32)?)?
            .iter()
            .try_fold(0usize, |n, b| {
                n.checked_mul(256)?.checked_add(usize::from(*b))
            })
    }
    fn slot<T: Shape>(&mut self, base: usize, head: &mut usize, tail: &mut usize) -> Option<()> {
        if T::DYNAMIC {
            if self.word(*head)? != tail.checked_sub(base)? {
                return None;
            }
            *tail = T::body(self, *tail)?;
        } else {
            let end = T::body(self, *head)?;
            if end != self.end(*head, T::HEAD.checked_mul(32)?)? {
                return None;
            }
        }
        *head = self.end(*head, T::HEAD.checked_mul(32)?)?;
        Some(())
    }
    fn repeated<T: Shape>(&mut self, base: usize, count: usize) -> Option<usize> {
        let mut head = base;
        let mut tail = self.end(base, count.checked_mul(T::HEAD)?.checked_mul(32)?)?;
        // Fuel also bounds hypothetical zero-sized static elements.
        if count > self.fuel {
            return None;
        }
        for _ in 0..count {
            self.slot::<T>(base, &mut head, &mut tail)?;
        }
        Some(tail)
    }
}

trait Shape {
    const DYNAMIC: bool;
    const HEAD: usize;
    fn body(budget: &mut Budget<'_>, start: usize) -> Option<usize>;
}
impl Shape for WordToken {
    const DYNAMIC: bool = false;
    const HEAD: usize = 1;
    fn body(budget: &mut Budget<'_>, start: usize) -> Option<usize> {
        budget.enter()?;
        budget.end(start, 32)
    }
}
impl Shape for PackedSeqToken<'_> {
    const DYNAMIC: bool = true;
    const HEAD: usize = 1;
    fn body(budget: &mut Budget<'_>, start: usize) -> Option<usize> {
        budget.enter()?;
        let len = budget.word(start)?;
        budget.end(
            start.checked_add(32)?,
            len.checked_add(31)?.checked_div(32)?.checked_mul(32)?,
        )
    }
}
impl<T: Shape> Shape for DynSeqToken<T> {
    const DYNAMIC: bool = true;
    const HEAD: usize = 1;
    fn body(budget: &mut Budget<'_>, start: usize) -> Option<usize> {
        budget.enter()?;
        let count = budget.word(start)?;
        budget.repeated::<T>(start.checked_add(32)?, count)
    }
}
impl<T: Shape, const N: usize> Shape for FixedSeqToken<T, N> {
    const DYNAMIC: bool = T::DYNAMIC;
    const HEAD: usize = if Self::DYNAMIC { 1 } else { N * T::HEAD };
    fn body(budget: &mut Budget<'_>, start: usize) -> Option<usize> {
        budget.enter()?;
        budget.repeated::<T>(start, N)
    }
}
macro_rules! tuple {
    ($($t:ident),+) => {
        impl<$($t:Shape),+> Shape for ($($t,)+) {
            const DYNAMIC: bool = false $(|| $t::DYNAMIC)+;
            const HEAD: usize = if Self::DYNAMIC {1} else {0 $(+ $t::HEAD)+};
            fn body(budget: &mut Budget<'_>, start: usize) -> Option<usize> {
                budget.enter()?;
                let mut head = start;
                let mut tail = budget.end(start,(0usize $(+ $t::HEAD)+).checked_mul(32)?)?;
                $(budget.slot::<$t>(start,&mut head,&mut tail)?;)+
                Some(tail)
            }
        }
    };
}
tuple!(A);
tuple!(A, B);
tuple!(A, B, C);
tuple!(A, B, C, D);
tuple!(A, B, C, D, E);
tuple!(A, B, C, D, E, F);
tuple!(A, B, C, D, E, F, G);
tuple!(A, B, C, D, E, F, G, H);
tuple!(A, B, C, D, E, F, G, H, I);
tuple!(A, B, C, D, E, F, G, H, I, J);
tuple!(A, B, C, D, E, F, G, H, I, J, K);
tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M);

pub(super) fn admits(arguments: &[u8]) -> bool {
    if arguments.len() > 8 * 1024 * 1024 {
        return false;
    }
    let mut budget = Budget {
        data: arguments,
        fuel: arguments.len().div_ceil(32).saturating_mul(4),
    };
    <postAndVerifyBatchCall as SolCall>::Token::body(&mut budget, 0) == Some(arguments.len())
}
