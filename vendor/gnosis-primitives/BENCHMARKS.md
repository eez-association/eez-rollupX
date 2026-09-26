# Performance Benchmarks

This document explains how to run and interpret performance benchmarks for `GnosisHeader` compared to `alloy_consensus::Header`.

## Running Benchmarks

### Prerequisites

The benchmark dependencies are gated behind the `bench` feature to avoid bloating downstream projects. To run benchmarks, you need to enable this feature:

```bash
cargo bench --features bench
```

### Quick Start

```bash
# Run all benchmarks
cargo bench --features bench

# Run specific benchmark group
cargo bench --features bench rlp_benches
cargo bench --features bench compact_benches
cargo bench --features bench compression_benches
cargo bench --features bench misc_benches

# Run specific benchmark
cargo bench --features bench "RLP Encode"
cargo bench --features bench "GnosisHeader (Post-Merge)"
```

### Viewing Results

Criterion generates detailed HTML reports in `target/criterion/`. Open `target/criterion/report/index.html` in your browser for interactive visualizations.

```bash
# Open benchmark results (macOS)
open target/criterion/report/index.html

# Open benchmark results (Linux)
xdg-open target/criterion/report/index.html
```

## Benchmark Categories

### 1. RLP Serialization/Deserialization

Tests the performance of RLP encoding and decoding, which is critical for network transmission and block storage.

**Benchmarks:**
- `RLP Encode` - Encoding headers to RLP format
- `RLP Decode` - Decoding headers from RLP format
- `RLP Roundtrip` - Combined encode + decode operation

**Compared implementations:**
- `GnosisHeader (Post-Merge)` - Gnosis headers with standard consensus fields
- `GnosisHeader (Pre-Merge)` - Gnosis headers with Aura consensus fields
- `alloy_consensus::Header` - Standard Ethereum headers

### 2. Compact Encoding/Decoding

Tests the compact encoding format used for efficient database storage (Reth-specific).

**Benchmarks:**
- `Compact Encode` - Encoding to compact format
- `Compact Decode` - Decoding from compact format
- `Compact Roundtrip` - Combined encode + decode operation

**Compared implementations:**
- `GnosisHeader (Post-Merge)`
- `GnosisHeader (Pre-Merge)`

### 3. Compression/Decompression

Tests database compression operations for header storage.

**Benchmarks:**
- `Compress` - Compressing headers for database storage
- `Decompress` - Decompressing headers from database
- `Compression Roundtrip` - Combined compress + decompress operation

**Compared implementations:**
- `GnosisHeader (Post-Merge)`
- `GnosisHeader (Pre-Merge)`

### 4. Miscellaneous Operations

Additional operations that impact overall performance.

**Benchmarks:**
- `Hash Calculation` - Computing block hash (keccak256)
- `Conversions` - Converting between GnosisHeader and alloy::Header
- `Memory Size Calculation` - Computing in-memory size
- `Encoded Size Comparison` - Comparing encoded sizes (informational)

## Understanding Results

### Interpreting Criterion Output

Criterion provides several metrics for each benchmark:

```
RLP Encode/GnosisHeader (Post-Merge)
                        time:   [1.2345 µs 1.2456 µs 1.2567 µs]
                        change: [-5.1234% -3.4567% -1.2345%] (p = 0.02 < 0.05)
                        Performance has improved.
```

**Key metrics:**
- **time**: The median execution time with confidence interval [lower bound, median, upper bound]
- **change**: Performance change from previous run (if available)
- **p-value**: Statistical significance (p < 0.05 indicates significant change)

### Performance Indicators

Look for these indicators in the results:

#### Good Performance
- **Similar times** between GnosisHeader and alloy::Header (within 5-10%)
- **Stable measurements** (narrow confidence intervals)
- **No significant overhead** from conditional logic

#### Performance Issues
- **>20% slower** than alloy::Header warrants investigation
- **Wide confidence intervals** suggest inconsistent performance
- **Pre-merge consistently slower** may indicate decoding overhead

### Encoded Size Comparison

The benchmark prints size comparisons:

```
=== Encoded Size Comparison ===
RLP Encoding:
  GnosisHeader (Post-Merge): 432 bytes
  GnosisHeader (Pre-Merge):  445 bytes
  alloy::Header:             428 bytes

Compact Encoding:
  GnosisHeader (Post-Merge): 387 bytes
  GnosisHeader (Pre-Merge):  401 bytes

Compression:
  GnosisHeader (Post-Merge): 298 bytes
  GnosisHeader (Pre-Merge):  312 bytes
===============================
```

**What to look for:**
- Post-merge should be similar to alloy::Header (±10 bytes)
- Pre-merge will be slightly larger due to Aura fields
- Compact encoding should be smaller than RLP
- Compression should achieve good reduction ratios

## Optimizing Performance

### If RLP encoding is slow:
1. Check `header_payload_length()` calculation efficiency
2. Review conditional logic in `encode()` method
3. Consider caching payload length if computed multiple times

### If RLP decoding is slow:
1. Review the peeking logic for consensus detection (line 889)
2. Check if buffer operations are efficient
3. Consider optimizing the conditional decoding paths

### If compact encoding is slow:
1. Review the `CompactHeader` struct transformations
2. Check if Option handling is efficient
3. Look for unnecessary allocations

### If hash calculation is slow:
1. This should match alloy::Header performance
2. Any difference suggests encoding overhead
3. Review the `hash_slow()` implementation

## Comparing Changes

### Baseline Benchmarks

Before making changes, establish a baseline:

```bash
# Run benchmarks and save baseline
cargo bench --features bench -- --save-baseline before-optimization
```

### After Optimization

Compare against the baseline:

```bash
# Run benchmarks and compare
cargo bench --features bench -- --baseline before-optimization
```

Criterion will show percentage changes from the baseline.

### Multiple Baselines

Track performance across multiple changes:

```bash
# Save different baselines
cargo bench --features bench -- --save-baseline v0.0.5
cargo bench --features bench -- --save-baseline after-rlp-optimization
cargo bench --features bench -- --save-baseline after-compact-optimization

# Compare against specific baseline
cargo bench --features bench -- --baseline v0.0.5
```

## Continuous Integration

### Adding to CI

Add benchmark checks to your CI pipeline:

```yaml
# .github/workflows/benchmarks.yml
name: Benchmarks

on:
  pull_request:
    branches: [main]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Run benchmarks
        run: cargo bench --features bench -- --output-format bencher | tee output.txt

      - name: Store benchmark result
        uses: benchmark-action/github-action-benchmark@v1
        with:
          tool: 'cargo'
          output-file-path: output.txt
          github-token: ${{ secrets.GITHUB_TOKEN }}
          auto-push: true
```

## Performance Goals

### Target Metrics

Based on the architecture, these are reasonable performance targets:

| Operation | Target | Notes |
|-----------|--------|-------|
| RLP Encode (Post-Merge) | ≤110% of alloy::Header | Small overhead acceptable |
| RLP Decode (Post-Merge) | ≤115% of alloy::Header | Peeking adds slight overhead |
| RLP Encode (Pre-Merge) | ≤120% of alloy::Header | Aura fields add complexity |
| RLP Decode (Pre-Merge) | ≤125% of alloy::Header | Conditional logic overhead |
| Compact Encode | ≤105% of RLP encode | Efficient transformation |
| Compact Decode | ≤105% of RLP decode | Efficient transformation |
| Hash Calculation | ≤105% of alloy::Header | Encoding overhead only |
| Conversions | <1µs | Should be trivial |

### Red Flags

Investigate if you see:
- **>150% overhead** on any operation
- **Increasing times** across benchmark runs (memory leak?)
- **High variance** in measurements (>10% coefficient of variation)
- **Pre-merge slower than 2x post-merge** (inefficient conditional logic)

## Advanced Usage

### Profiling Specific Functions

To profile a specific slow function:

```bash
# Install cargo-flamegraph
cargo install flamegraph

# Profile with flamegraph
cargo flamegraph --bench header_performance --features bench -- --bench
```

### Custom Benchmark Scenarios

Add custom scenarios to `benches/header_performance.rs`:

```rust
fn bench_custom_scenario(c: &mut Criterion) {
    let mut group = c.benchmark_group("Custom Scenario");

    // Your custom benchmark logic here

    group.finish();
}

criterion_group!(custom_benches, bench_custom_scenario);
criterion_main!(rlp_benches, compact_benches, custom_benches);
```

### Sampling Configuration

Adjust sampling for faster/more accurate benchmarks:

```rust
// In benches/header_performance.rs
use criterion::{Criterion, BenchmarkId, SamplingMode};

fn bench_with_custom_config(c: &mut Criterion) {
    let mut group = c.benchmark_group("Custom Config");

    // Fast benchmarks (fewer samples)
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(10);

    // Or more accurate (more samples)
    group.sample_size(1000);

    // Your benchmarks here

    group.finish();
}
```

## Troubleshooting

### Benchmarks Won't Compile

**Issue:** `error: cannot find criterion in this scope`

**Solution:** Make sure you're using the `--features bench` flag:
```bash
cargo bench --features bench
```

### Inconsistent Results

**Issue:** Benchmark times vary significantly between runs

**Solutions:**
1. Close other applications to reduce system noise
2. Disable CPU frequency scaling: `sudo cpupower frequency-set --governor performance`
3. Increase sample size in benchmark configuration
4. Run benchmarks multiple times and compare

### No HTML Reports

**Issue:** HTML reports not generated

**Solution:** Criterion HTML reports are enabled by default. Check `target/criterion/` directory. If missing, ensure `html_reports` feature is enabled in Cargo.toml (already configured).

## Resources

- [Criterion.rs Documentation](https://bheisler.github.io/criterion.rs/book/)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)
- [Benchmarking Best Practices](https://easyperf.net/blog/)
