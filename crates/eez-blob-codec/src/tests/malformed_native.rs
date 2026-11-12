//! Raw-wire regressions for the bounded source-case map in issue #143.
use crate::{Error, NativeProfileError, decode, decode_native_payload, pack};

fn one_action() -> Vec<u8> {
    // Native outer v0: one ordinary empty block, then an L1 -> rollup 7 call.
    // Construct independently of the stream encoder; these are syntax fixtures,
    // not authenticated execution or settlement evidence.
    let mut wire = vec![0, 2];
    wire.extend_from_slice(&7_u64.to_le_bytes());
    wire.extend_from_slice(&[26, 0, 1, 0, 1]);
    wire.extend_from_slice(&[0; 20]);
    wire.extend_from_slice(&[1, 0, 3]);
    wire.extend_from_slice(&[0; 8]);
    wire.extend_from_slice(&[0, 4]);
    wire.extend_from_slice(&7_u64.to_le_bytes());
    wire.extend_from_slice(&[0; 80]); // addresses, value, gas
    wire.extend_from_slice(&[0, 6, 0, 10, 1]);
    assert_eq!(wire.len(), 141);
    assert_eq!((wire[37], wire[47], wire[137], wire[139]), (3, 4, 6, 10));
    wire
}

#[test]
fn mapped_action_boundaries_fail_at_the_raw_decoder() {
    let canonical = one_action();
    assert!(decode_native_payload(&pack(&canonical).unwrap(), 7).is_ok());
    for (offset, replacement, expected) in [
        (47, 3, Error::NestedTransaction),
        (137, 255, Error::InvalidMessageType(255)),
        (139, 3, Error::TruncatedStream),
    ] {
        let mut wire = canonical.clone();
        wire[offset] = replacement;
        assert_eq!(decode(&wire, &[]), Err(expected), "offset {offset}");
    }
    let mut missing_finish = canonical.clone();
    missing_finish.remove(139);
    assert_eq!(decode(&missing_finish, &[]), Err(Error::UnclosedBracket));

    let mut truncated_tx_data = canonical[..47].to_vec();
    truncated_tx_data[46] = 2;
    assert_eq!(decode(&truncated_tx_data, &[]), Err(Error::TruncatedStream));

    // A second complete ChainOperation is structurally legal in Core, but
    // forbidden by the native carrier profile even between transactions.
    let mut second_operation = canonical.clone();
    second_operation.splice(140..140, canonical[1..37].iter().copied());
    assert!(decode(&second_operation, &[]).is_ok());
    assert_eq!(
        decode_native_payload(&pack(&second_operation).unwrap(), 7),
        Err(NativeProfileError::UnexpectedMessages)
    );
}

#[test]
fn native_wire_truncations_lengths_and_padding_are_bounded() {
    let canonical = one_action();
    for end in 0..canonical.len() {
        assert!(decode(&canonical[..end], &[]).is_err(), "prefix {end}");
    }
    // Each of the four outer bytes fields uses a u32 varint. Non-minimal
    // lengths are allowed by Core; this does not relax inner payload integers.
    for offset in [10, 46, 136, 138] {
        let mut nonminimal = canonical.clone();
        nonminimal.splice(offset..=offset, [canonical[offset] | 128, 0]);
        assert_eq!(decode(&nonminimal, &[]), decode(&canonical, &[]));
        for (prefix, expected) in [
            (&[255, 255, 255, 255, 16][..], Error::VarintOverflow),
            (&[128, 128, 128, 128, 128, 0][..], Error::VarintOverflow),
            (&[255, 255, 255, 255, 15][..], Error::TruncatedStream),
        ] {
            let mut invalid = canonical.clone();
            invalid.splice(offset..=offset, prefix.iter().copied());
            assert_eq!(decode(&invalid, &[]), Err(expected), "length at {offset}");
        }
    }
    let mut padding = canonical;
    padding.extend_from_slice(&[0, 0]);
    assert!(decode(&padding, &[]).is_ok());
    padding.push(2);
    assert_eq!(
        decode(&padding, &[]),
        Err(Error::NonZeroPadding { offset: 143 })
    );
}
