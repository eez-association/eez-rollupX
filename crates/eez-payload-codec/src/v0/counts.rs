//! Canonical transaction-count tokens. Empty runs are never expanded here.

use super::Error;

pub(crate) const MAX_TRANSACTIONS: usize = 16_383;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Transactions(usize),
    Empty(u32),
}

impl Token {
    pub(crate) fn encode(self, out: &mut Vec<u8>) {
        match self {
            Self::Transactions(n) => {
                debug_assert!((1..=MAX_TRANSACTIONS).contains(&n));
                if n < 128 {
                    out.push(n as u8);
                } else {
                    out.extend_from_slice(&[0x80 | (n >> 8) as u8, n as u8]);
                }
            }
            Self::Empty(1) => out.push(0),
            Self::Empty(n @ 2..=64) => out.push(0xc0 + (n - 2) as u8),
            Self::Empty(mut n) => {
                debug_assert!(n >= 65);
                out.push(0xff);
                while n >= 128 {
                    out.push((n as u8 & 0x7f) | 0x80);
                    n >>= 7;
                }
                out.push(n as u8);
            }
        }
    }

    pub(crate) const fn len(self) -> usize {
        match self {
            Self::Transactions(n) => {
                if n < 128 {
                    1
                } else {
                    2
                }
            }
            Self::Empty(n) => {
                if n <= 64 {
                    1
                } else {
                    1 + (32 - n.leading_zeros()).div_ceil(7) as usize
                }
            }
        }
    }
}

pub(crate) fn read(input: &mut &[u8], previous_empty: &mut bool) -> Result<Token, Error> {
    let byte = take(input)?;
    let token = match byte {
        0 => Token::Empty(1),
        1..=0x7f => Token::Transactions(byte as usize),
        0x80..=0xbf => {
            let n = (usize::from(byte & 63) << 8) | usize::from(take(input)?);
            if n < 128 {
                return Err(Error::Counts);
            }
            Token::Transactions(n)
        }
        0xc0..=0xfe => Token::Empty(u32::from(byte - 0xc0) + 2),
        0xff => {
            let mut n = 0u32;
            let mut shift = 0;
            loop {
                let b = take(input)?;
                if shift == 28 && b > 15 {
                    return Err(Error::Counts);
                }
                n |= u32::from(b & 127) << shift;
                if b < 128 {
                    if n < 65 || (shift > 0 && b == 0) {
                        return Err(Error::Counts);
                    }
                    break;
                }
                shift += 7;
            }
            Token::Empty(n)
        }
    };
    let empty = matches!(token, Token::Empty(_));
    if empty && *previous_empty {
        return Err(Error::Counts);
    }
    *previous_empty = empty;
    Ok(token)
}

fn take(input: &mut &[u8]) -> Result<u8, Error> {
    let (first, rest) = input.split_first().ok_or(Error::Counts)?;
    *input = rest;
    Ok(*first)
}

/// Constant-size monoid for exact token bytes, including runs crossing a join.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    blocks: usize,
    bytes: usize,
    leading: u32,
    trailing: u32,
}

impl Summary {
    pub(crate) fn block(count: usize) -> Result<Self, Error> {
        if count > MAX_TRANSACTIONS {
            return Err(Error::TransactionCount);
        }
        Ok(Self {
            blocks: 1,
            bytes: if count < 128 { 1 } else { 2 },
            leading: u32::from(count == 0),
            trailing: u32::from(count == 0),
        })
    }

    pub(crate) fn empty(blocks: u32) -> Self {
        if blocks == 0 {
            return Self::default();
        }
        Self {
            blocks: blocks as usize,
            bytes: Token::Empty(blocks).len(),
            leading: blocks,
            trailing: blocks,
        }
    }

    pub(crate) fn encoded_len(&self) -> usize {
        self.bytes
    }

    pub(crate) fn combine(&self, right: &Self) -> Result<Self, Error> {
        if self.blocks == 0 {
            return Ok(right.clone());
        }
        if right.blocks == 0 {
            return Ok(self.clone());
        }
        let blocks = super::add(self.blocks, right.blocks)?;
        u32::try_from(blocks).map_err(|_| Error::Overflow)?;
        let mut bytes = super::add(self.bytes, right.bytes)?;
        if self.trailing > 0 && right.leading > 0 {
            bytes -= Token::Empty(self.trailing).len() + Token::Empty(right.leading).len();
            bytes += Token::Empty(self.trailing + right.leading).len();
        }
        let leading = self.leading
            + if self.leading as usize == self.blocks {
                right.leading
            } else {
                0
            };
        let trailing = right.trailing
            + if right.trailing as usize == right.blocks {
                self.trailing
            } else {
                0
            };
        Ok(Self {
            blocks,
            bytes,
            leading,
            trailing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn summary(values: &[usize]) -> Summary {
        values.iter().fold(Summary::default(), |sum, n| {
            sum.combine(&Summary::block(*n).unwrap()).unwrap()
        })
    }

    fn assert_size(values: &[usize]) {
        let original = values;
        let mut bytes = Vec::new();
        let mut values = values.iter().peekable();
        while let Some(&n) = values.next() {
            if n == 0 {
                let mut run = 1;
                while values.peek() == Some(&&0) {
                    values.next();
                    run += 1;
                }
                Token::Empty(run).encode(&mut bytes);
            } else {
                Token::Transactions(n).encode(&mut bytes);
            }
        }
        let mut decoded = Vec::new();
        let mut input = bytes.as_slice();
        let mut previous_empty = false;
        while !input.is_empty() {
            match read(&mut input, &mut previous_empty).unwrap() {
                Token::Transactions(n) => decoded.push(n),
                Token::Empty(n) => decoded.extend(std::iter::repeat_n(0, n as usize)),
            }
        }
        assert_eq!(decoded, original);
        assert_eq!(summary(&decoded).encoded_len(), bytes.len());
    }

    #[test]
    fn run_joins_have_exact_sizes() {
        for n in [1, 2, 63, 64, 65, 127, 128, 255, 256, 16_383, 16_384, 32_000] {
            let zeros = vec![0; n];
            let left = summary(&zeros);
            let mut both = zeros.clone();
            both.extend_from_slice(&zeros);
            assert_eq!(left.combine(&left).unwrap(), summary(&both));
            assert_size(&both);
        }
        for n in [0, 1, 55, 56, 127, 128, 255, 256] {
            for txs in [0, 1, 127, 128, 16_383] {
                assert_size(&vec![txs; n]);
            }
        }
    }

    proptest! {
        #[test]
        fn summaries_are_associative_and_exact(
            values in prop::collection::vec(prop_oneof![4 => Just(0usize), 1 => 1usize..=MAX_TRANSACTIONS], 0..300),
            a in 0usize..300, b in 0usize..300,
        ) {
            let a = a.min(values.len()); let b = b.min(values.len()).max(a);
            let left = summary(&values[..a]); let mid = summary(&values[a..b]); let right = summary(&values[b..]);
            prop_assert_eq!(left.combine(&mid).unwrap().combine(&right).unwrap(), left.combine(&mid.combine(&right).unwrap()).unwrap());
            assert_size(&values);
        }
    }

    #[test]
    fn every_positive_count_and_run_boundaries_roundtrip() {
        for token in (1..=MAX_TRANSACTIONS).map(Token::Transactions).chain(
            [
                1,
                2,
                15,
                64,
                65,
                127,
                128,
                600,
                16_383,
                16_384,
                65_535,
                u32::MAX,
            ]
            .map(Token::Empty),
        ) {
            let mut encoded = Vec::new();
            token.encode(&mut encoded);
            assert_eq!(encoded.len(), token.len());
            let mut input = encoded.as_slice();
            assert_eq!(read(&mut input, &mut false).unwrap(), token);
            assert!(input.is_empty());
        }
        for (token, expected) in [
            (Token::Transactions(128), vec![0x80, 0x80]),
            (Token::Transactions(1428), vec![0x85, 0x94]),
            (Token::Transactions(16_383), vec![0xbf, 0xff]),
            (Token::Empty(64), vec![0xfe]),
            (Token::Empty(65), vec![0xff, 0x41]),
            (Token::Empty(600), vec![0xff, 0xd8, 4]),
        ] {
            let mut encoded = Vec::new();
            token.encode(&mut encoded);
            assert_eq!(encoded, expected);
        }
    }

    #[test]
    fn rejects_noncanonical_truncated_overflowing_and_adjacent_runs() {
        for input in [
            &[][..],
            &[0x80],
            &[0x80, 0],
            &[0x80, 127],
            &[0xff],
            &[0xff, 64],
            &[0xff, 0xc1, 0],
            &[0xff, 0x80, 0x80, 0x80, 0x80, 16],
            &[0xff, 0x80, 0x80, 0x80, 0x80, 0x80, 1],
        ] {
            assert!(matches!(read(&mut &*input, &mut false), Err(Error::Counts)));
        }
        for input in [&[0][..], &[0xc0], &[0xff, 65]] {
            assert!(read(&mut &*input, &mut true).is_err());
        }
        assert!(Summary::block(16_384).is_err());
    }
}
