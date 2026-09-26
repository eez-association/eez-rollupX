use alloy_consensus::Header;
use alloy_primitives::{Address, B64, B256, Bloom, Bytes, FixedBytes, U256};
use alloy_rlp::{Decodable, Encodable};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, black_box, criterion_group, criterion_main};
use gnosis_primitives::header::GnosisHeader;
use reth_codecs::Compact;
use reth_db::table::{Compress, Decompress};
use reth_primitives_traits::InMemorySize;

// Configure benchmarks to run faster
fn configure_benchmark_group(group: &mut BenchmarkGroup<WallTime>) {
    group.sample_size(20); // Reduce from 100 to 20 samples
    group.measurement_time(std::time::Duration::from_secs(2)); // 2 seconds instead of 5
}

// ============================================================================
// Test Data Generators
// ============================================================================

fn create_gnosis_post_merge_header() -> GnosisHeader {
    GnosisHeader {
        parent_hash: B256::random(),
        ommers_hash: B256::random(),
        beneficiary: Address::random(),
        state_root: B256::random(),
        transactions_root: B256::random(),
        receipts_root: B256::random(),
        logs_bloom: Bloom::random(),
        difficulty: U256::from(0),
        number: 19_000_000,
        gas_limit: 30_000_000,
        gas_used: 15_000_000,
        timestamp: 1704067200,
        extra_data: Bytes::from_static(b"Gnosis Chain Post-Merge Block"),
        mix_hash: Some(B256::random()),
        nonce: Some(B64::from(0u64)),
        aura_step: None,
        aura_seal: None,
        base_fee_per_gas: Some(7_000_000_000),
        withdrawals_root: Some(B256::random()),
        blob_gas_used: Some(393_216),
        excess_blob_gas: Some(2_621_440),
        parent_beacon_block_root: Some(B256::random()),
        requests_hash: Some(B256::random()),
        block_access_list_hash: None,
        slot_number: None,
    }
}

fn create_gnosis_pre_merge_header() -> GnosisHeader {
    GnosisHeader {
        parent_hash: B256::random(),
        ommers_hash: B256::random(),
        beneficiary: Address::random(),
        state_root: B256::random(),
        transactions_root: B256::random(),
        receipts_root: B256::random(),
        logs_bloom: Bloom::random(),
        difficulty: U256::from(1_000_000),
        number: 18_000_000,
        gas_limit: 17_000_000,
        gas_used: 8_500_000,
        timestamp: 1695067200,
        extra_data: Bytes::from_static(b"Gnosis Chain Aura Block"),
        mix_hash: None,
        nonce: None,
        aura_step: Some(U256::from(1637394693478219_u64)),
        aura_seal: Some(FixedBytes::from([42u8; 65])),
        base_fee_per_gas: Some(5_000_000_000),
        withdrawals_root: None,
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        block_access_list_hash: None,
        slot_number: None,
    }
}

fn create_alloy_header() -> Header {
    Header {
        parent_hash: B256::random(),
        ommers_hash: B256::random(),
        beneficiary: Address::random(),
        state_root: B256::random(),
        transactions_root: B256::random(),
        receipts_root: B256::random(),
        logs_bloom: Bloom::random(),
        difficulty: U256::from(0),
        number: 19_000_000,
        gas_limit: 30_000_000,
        gas_used: 15_000_000,
        timestamp: 1704067200,
        extra_data: Bytes::from_static(b"Standard Ethereum Header"),
        mix_hash: B256::random(),
        nonce: B64::from(0u64),
        base_fee_per_gas: Some(7_000_000_000),
        withdrawals_root: Some(B256::random()),
        blob_gas_used: Some(393_216),
        excess_blob_gas: Some(2_621_440),
        parent_beacon_block_root: Some(B256::random()),
        requests_hash: Some(B256::random()),
        block_access_list_hash: None,
        slot_number: None,
    }
}

// ============================================================================
// RLP Serialization Benchmarks
// ============================================================================

fn bench_rlp_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("RLP Encode");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_post_merge).encode(&mut buf);
            black_box(buf);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_pre_merge).encode(&mut buf);
            black_box(buf);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&alloy_header).encode(&mut buf);
            black_box(buf);
        })
    });

    group.finish();
}

fn bench_rlp_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("RLP Decode");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    // Pre-encode the headers
    let mut gnosis_post_buf = Vec::new();
    gnosis_post_merge.encode(&mut gnosis_post_buf);

    let mut gnosis_pre_buf = Vec::new();
    gnosis_pre_merge.encode(&mut gnosis_pre_buf);

    let mut alloy_buf = Vec::new();
    alloy_header.encode(&mut alloy_buf);

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf_slice = &gnosis_post_buf[..];
            let header = GnosisHeader::decode(&mut buf_slice).unwrap();
            black_box(header);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf_slice = &gnosis_pre_buf[..];
            let header = GnosisHeader::decode(&mut buf_slice).unwrap();
            black_box(header);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf_slice = &alloy_buf[..];
            let header = Header::decode(&mut buf_slice).unwrap();
            black_box(header);
        })
    });

    group.finish();
}

fn bench_rlp_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("RLP Roundtrip (Encode + Decode)");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_post_merge).encode(&mut buf);
            let mut buf_slice = &buf[..];
            let decoded = GnosisHeader::decode(&mut buf_slice).unwrap();
            black_box(decoded);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_pre_merge).encode(&mut buf);
            let mut buf_slice = &buf[..];
            let decoded = GnosisHeader::decode(&mut buf_slice).unwrap();
            black_box(decoded);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&alloy_header).encode(&mut buf);
            let mut buf_slice = &buf[..];
            let decoded = Header::decode(&mut buf_slice).unwrap();
            black_box(decoded);
        })
    });

    group.finish();
}

// ============================================================================
// Compact Encoding Benchmarks
// ============================================================================

fn bench_compact_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("Compact Encode");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let len = black_box(&gnosis_post_merge).to_compact(&mut buf);
            black_box((buf, len));
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let len = black_box(&gnosis_pre_merge).to_compact(&mut buf);
            black_box((buf, len));
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let len = black_box(&alloy_header).to_compact(&mut buf);
            black_box((buf, len));
        })
    });

    group.finish();
}

fn bench_compact_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("Compact Decode");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    // Pre-encode the headers
    let mut gnosis_post_buf = Vec::new();
    let gnosis_post_len = gnosis_post_merge.to_compact(&mut gnosis_post_buf);

    let mut gnosis_pre_buf = Vec::new();
    let gnosis_pre_len = gnosis_pre_merge.to_compact(&mut gnosis_pre_buf);

    let mut alloy_buf = Vec::new();
    let alloy_len = alloy_header.to_compact(&mut alloy_buf);

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let (header, _) = GnosisHeader::from_compact(&gnosis_post_buf, gnosis_post_len);
            black_box(header);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let (header, _) = GnosisHeader::from_compact(&gnosis_pre_buf, gnosis_pre_len);
            black_box(header);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let (header, _) = Header::from_compact(&alloy_buf, alloy_len);
            black_box(header);
        })
    });

    group.finish();
}

fn bench_compact_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("Compact Roundtrip (Encode + Decode)");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let len = black_box(&gnosis_post_merge).to_compact(&mut buf);
            let (decoded, _) = GnosisHeader::from_compact(&buf, len);
            black_box(decoded);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let len = black_box(&gnosis_pre_merge).to_compact(&mut buf);
            let (decoded, _) = GnosisHeader::from_compact(&buf, len);
            black_box(decoded);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let len = black_box(&alloy_header).to_compact(&mut buf);
            let (decoded, _) = Header::from_compact(&buf, len);
            black_box(decoded);
        })
    });

    group.finish();
}

// ============================================================================
// Compression Benchmarks
// ============================================================================

fn bench_compress(c: &mut Criterion) {
    let mut group = c.benchmark_group("Compress");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_post_merge).compress_to_buf(&mut buf);
            black_box(buf);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_pre_merge).compress_to_buf(&mut buf);
            black_box(buf);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&alloy_header).compress_to_buf(&mut buf);
            black_box(buf);
        })
    });

    group.finish();
}

fn bench_decompress(c: &mut Criterion) {
    let mut group = c.benchmark_group("Decompress");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    // Pre-compress the headers
    let mut gnosis_post_buf = Vec::new();
    gnosis_post_merge.compress_to_buf(&mut gnosis_post_buf);

    let mut gnosis_pre_buf = Vec::new();
    gnosis_pre_merge.compress_to_buf(&mut gnosis_pre_buf);

    let mut alloy_buf = Vec::new();
    alloy_header.compress_to_buf(&mut alloy_buf);

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let header = GnosisHeader::decompress(&gnosis_post_buf).unwrap();
            black_box(header);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let header = GnosisHeader::decompress(&gnosis_pre_buf).unwrap();
            black_box(header);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let header = Header::decompress(&alloy_buf).unwrap();
            black_box(header);
        })
    });

    group.finish();
}

// Isolates the cost of the v0.1.x-compat wrapper added in v0.2.4. The
// pre-fix decoder was effectively `CompactHeader::from_compact` (the
// auto-derived path); the post-fix decoder wraps it in
// `catch_unwind` + a re-encode/byte-compare for semantic validation.
// Both bench fns decode the same canonical v0.2.x bytes, so the delta
// between them is the overhead the fix imposes on the happy path.
fn bench_decode_overhead_of_compat_wrapper(c: &mut Criterion) {
    use reth_codecs::Compact as _;

    let mut group = c.benchmark_group("Decode (pre-fix vs post-fix)");
    configure_benchmark_group(&mut group);

    // Use the public alloy header type as a stand-in for "pre-fix": its
    // CompactHeader is the same shape, and `Header::from_compact` is what
    // GnosisHeader::from_compact looked like before v0.2.4.
    let alloy_header = create_alloy_header();
    let mut alloy_buf = Vec::new();
    let alloy_len = alloy_header.to_compact(&mut alloy_buf);

    let gnosis_post = create_gnosis_post_merge_header();
    let mut gnosis_buf = Vec::new();
    let gnosis_len = gnosis_post.to_compact(&mut gnosis_buf);

    group.bench_function(
        "PRE-FIX baseline: alloy_consensus::Header::from_compact",
        |b| {
            b.iter(|| {
                let (h, _) = Header::from_compact(&alloy_buf, alloy_len);
                black_box(h);
            })
        },
    );

    group.bench_function("POST-FIX: GnosisHeader::from_compact", |b| {
        b.iter(|| {
            let (h, _) = GnosisHeader::from_compact(&gnosis_buf, gnosis_len);
            black_box(h);
        })
    });

    group.finish();
}

fn bench_compression_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("Compression Roundtrip (Compress + Decompress)");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_post_merge).compress_to_buf(&mut buf);
            let decoded = GnosisHeader::decompress(&buf).unwrap();
            black_box(decoded);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&gnosis_pre_merge).compress_to_buf(&mut buf);
            let decoded = GnosisHeader::decompress(&buf).unwrap();
            black_box(decoded);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            black_box(&alloy_header).compress_to_buf(&mut buf);
            let decoded = Header::decompress(&buf).unwrap();
            black_box(decoded);
        })
    });

    group.finish();
}

// ============================================================================
// Hash Calculation Benchmarks
// ============================================================================

fn bench_hash_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Hash Calculation");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let hash = black_box(&gnosis_post_merge).hash_slow();
            black_box(hash);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let hash = black_box(&gnosis_pre_merge).hash_slow();
            black_box(hash);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let hash = black_box(&alloy_header).hash_slow();
            black_box(hash);
        })
    });

    group.finish();
}

// ============================================================================
// Conversion Benchmarks
// ============================================================================

fn bench_conversions(c: &mut Criterion) {
    let mut group = c.benchmark_group("Conversions");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader -> alloy::Header", |b| {
        b.iter(|| {
            let converted: Header = black_box(gnosis_post_merge.clone()).into();
            black_box(converted);
        })
    });

    group.bench_function("alloy::Header -> GnosisHeader", |b| {
        b.iter(|| {
            let converted: GnosisHeader = black_box(alloy_header.clone()).into();
            black_box(converted);
        })
    });

    group.finish();
}

// ============================================================================
// Memory Size Benchmarks
// ============================================================================

fn bench_memory_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("Memory Size Calculation");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    group.bench_function("GnosisHeader (Post-Merge)", |b| {
        b.iter(|| {
            let size = black_box(&gnosis_post_merge).size();
            black_box(size);
        })
    });

    group.bench_function("GnosisHeader (Pre-Merge)", |b| {
        b.iter(|| {
            let size = black_box(&gnosis_pre_merge).size();
            black_box(size);
        })
    });

    group.bench_function("alloy_consensus::Header", |b| {
        b.iter(|| {
            let size = black_box(&alloy_header).size();
            black_box(size);
        })
    });

    group.finish();
}

// ============================================================================
// Encoded Size Comparison
// ============================================================================

fn bench_encoded_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("Encoded Size Comparison");
    configure_benchmark_group(&mut group);

    let gnosis_post_merge = create_gnosis_post_merge_header();
    let gnosis_pre_merge = create_gnosis_pre_merge_header();
    let alloy_header = create_alloy_header();

    // RLP sizes
    let mut gnosis_post_rlp = Vec::new();
    gnosis_post_merge.encode(&mut gnosis_post_rlp);

    let mut gnosis_pre_rlp = Vec::new();
    gnosis_pre_merge.encode(&mut gnosis_pre_rlp);

    let mut alloy_rlp = Vec::new();
    alloy_header.encode(&mut alloy_rlp);

    // Compact sizes
    let mut gnosis_post_compact = Vec::new();
    gnosis_post_merge.to_compact(&mut gnosis_post_compact);

    let mut gnosis_pre_compact = Vec::new();
    gnosis_pre_merge.to_compact(&mut gnosis_pre_compact);

    // Compression sizes
    let mut gnosis_post_compressed = Vec::new();
    gnosis_post_merge.compress_to_buf(&mut gnosis_post_compressed);

    let mut gnosis_pre_compressed = Vec::new();
    gnosis_pre_merge.compress_to_buf(&mut gnosis_pre_compressed);

    println!("\n=== Encoded Size Comparison ===");
    println!("RLP Encoding:");
    println!(
        "  GnosisHeader (Post-Merge): {} bytes",
        gnosis_post_rlp.len()
    );
    println!(
        "  GnosisHeader (Pre-Merge):  {} bytes",
        gnosis_pre_rlp.len()
    );
    println!("  alloy::Header:             {} bytes", alloy_rlp.len());
    println!("\nCompact Encoding:");
    println!(
        "  GnosisHeader (Post-Merge): {} bytes",
        gnosis_post_compact.len()
    );
    println!(
        "  GnosisHeader (Pre-Merge):  {} bytes",
        gnosis_pre_compact.len()
    );
    println!("\nCompression:");
    println!(
        "  GnosisHeader (Post-Merge): {} bytes",
        gnosis_post_compressed.len()
    );
    println!(
        "  GnosisHeader (Pre-Merge):  {} bytes",
        gnosis_pre_compressed.len()
    );
    println!("===============================\n");

    group.finish();
}

criterion_group!(
    rlp_benches,
    bench_rlp_encode,
    bench_rlp_decode,
    bench_rlp_roundtrip
);

criterion_group!(
    compact_benches,
    bench_compact_encode,
    bench_compact_decode,
    bench_compact_roundtrip,
    bench_decode_overhead_of_compat_wrapper
);

criterion_group!(
    compression_benches,
    bench_compress,
    bench_decompress,
    bench_compression_roundtrip
);

criterion_group!(
    misc_benches,
    bench_hash_calculation,
    bench_conversions,
    bench_memory_size,
    bench_encoded_sizes
);

criterion_main!(
    rlp_benches,
    compact_benches,
    compression_benches,
    misc_benches
);
