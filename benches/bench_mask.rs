// Required for NEON on 32-bit ARM until stable
#![cfg_attr(
    all(feature = "nightly", target_arch = "arm"),
    feature(
        stdarch_arm_neon_intrinsics,
        stdarch_arm_feature_detection,
        arm_target_feature
    )
)]
// Required for VSX until stable
#![cfg_attr(
    all(
        feature = "nightly",
        any(target_arch = "powerpc64", target_arch = "powerpc")
    ),
    feature(
        stdarch_powerpc,
        stdarch_powerpc_feature_detection,
        powerpc_target_feature
    )
)]
// Required for s390x vectors until stable
#![cfg_attr(
    all(feature = "nightly", target_arch = "s390x"),
    feature(stdarch_s390x)
)]
// Required for LASX until stable
#![cfg_attr(
    all(feature = "nightly", target_arch = "loongarch64"),
    feature(stdarch_loongarch)
)]
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
#[path = "../src/mask.rs"]
mod mask;

/// Retains the actual randomness helpers used by the included conditional mask
/// test.
#[cfg(all(test, feature = "client", feature = "fastrand"))]
#[path = "../src/rand.rs"]
pub mod rand;

use mask::frame;

fn mask_benchmark(c: &mut Criterion) {
    // Generate 1 GiB of random data
    let mut data = vec![0; 1024 * 1024 * 1024];
    fastrand::fill(&mut data);
    // Generate a random masking key
    let mut key = [0; 4];
    fastrand::fill(&mut key);

    // We benchmark 2^n bytes of input for n in 0..=30 (2 ** 30 bytes are 1GiB)
    for exp in 0..=30 {
        let bytes = 2usize.pow(exp);
        c.bench_function(&format!("mask {bytes} bytes"), |b| {
            b.iter(|| frame(black_box(&mut key), black_box(&mut data[..bytes])))
        });
    }
}

criterion_group!(benches, mask_benchmark);
criterion_main!(benches);
