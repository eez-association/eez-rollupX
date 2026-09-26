# Benchmark Coverage Summary

All benchmarks now compare `GnosisHeader` against `alloy_consensus::Header` as the baseline.

## Benchmark Groups

### 1. RLP Serialization Benchmarks
**What**: Network transmission encoding format

| Benchmark | GnosisHeader (Post-Merge) | GnosisHeader (Pre-Merge) | alloy::Header |
|-----------|---------------------------|--------------------------|---------------|
| RLP Encode | ✅ | ✅ | ✅ |
| RLP Decode | ✅ | ✅ | ✅ |
| RLP Roundtrip | ✅ | ✅ | ✅ |

### 2. Compact Encoding Benchmarks
**What**: Database storage format (Reth-specific)

| Benchmark | GnosisHeader (Post-Merge) | GnosisHeader (Pre-Merge) | alloy::Header |
|-----------|---------------------------|--------------------------|---------------|
| Compact Encode | ✅ | ✅ | ✅ |
| Compact Decode | ✅ | ✅ | ✅ |
| Compact Roundtrip | ✅ | ✅ | ✅ |

### 3. Compression Benchmarks
**What**: Database compression operations

| Benchmark | GnosisHeader (Post-Merge) | GnosisHeader (Pre-Merge) | alloy::Header |
|-----------|---------------------------|--------------------------|---------------|
| Compress | ✅ | ✅ | ✅ |
| Decompress | ✅ | ✅ | ✅ |
| Compression Roundtrip | ✅ | ✅ | ✅ |

### 4. Hash Calculation Benchmarks
**What**: Block hash computation (keccak256)

| Benchmark | GnosisHeader (Post-Merge) | GnosisHeader (Pre-Merge) | alloy::Header |
|-----------|---------------------------|--------------------------|---------------|
| Hash Calculation | ✅ | ✅ | ✅ |

### 5. Conversion Benchmarks
**What**: Converting between header types

| Benchmark | Description |
|-----------|-------------|
| GnosisHeader → alloy::Header | ✅ |
| alloy::Header → GnosisHeader | ✅ |

### 6. Memory Size Benchmarks
**What**: In-memory size calculation

| Benchmark | GnosisHeader (Post-Merge) | GnosisHeader (Pre-Merge) | alloy::Header |
|-----------|---------------------------|--------------------------|---------------|
| Memory Size | ✅ | ✅ | ✅ |

### 7. Encoded Size Comparison
**What**: Informational - prints actual sizes

- RLP encoding sizes (all 3 types)
- Compact encoding sizes (GnosisHeader only)
- Compression sizes (GnosisHeader only)

## Expected Performance Characteristics

### Post-Merge Headers
Since post-merge `GnosisHeader` uses the same fields as `alloy::Header`, performance should be nearly identical:
- **Target**: ≤110% of alloy::Header for all operations
- **Why**: Small overhead from conditional logic and conversions

### Pre-Merge Headers
Pre-merge headers include Aura-specific fields and require conditional decoding:
- **Target**: ≤125% of alloy::Header for encode/decode operations
- **Why**: Additional fields and peeking logic during decode

### Conversions
Conversions between types should be very fast:
- **Target**: <1µs per conversion
- **Why**: Simple field mapping, no complex computation

## Running Specific Comparisons

```bash
# Compare RLP performance
cargo bench --features bench "RLP"

# Compare compression performance
cargo bench --features bench "Compress"

# Compare hash calculation
cargo bench --features bench "Hash Calculation"

# Compare all encoding methods
cargo bench --features bench "Encode"

# Compare all decoding methods
cargo bench --features bench "Decode"
```

## Interpreting Results

### Good Performance Indicators
- Post-merge within 5-10% of alloy::Header
- Pre-merge within 15-25% of alloy::Header
- Consistent measurements (narrow confidence intervals)

### Performance Issues (Investigate If)
- Post-merge >20% slower than alloy::Header
- Pre-merge >50% slower than alloy::Header
- High variance between runs (>10% CV)
- Memory size significantly larger than expected

## Continuous Monitoring

To track performance over time:

```bash
# Baseline before changes
cargo bench --features bench -- --save-baseline main

# After optimization
cargo bench --features bench -- --baseline main
```

This will show percentage changes from your baseline for all benchmarks.
