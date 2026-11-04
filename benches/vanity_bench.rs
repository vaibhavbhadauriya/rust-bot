//! Vanity Miner Microsecond Criterion Benchmark
//! Measures single-threaded and multi-threaded hash rate (hashes/sec) for CREATE2 vanity salt mining.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use alloy_primitives::{Address, keccak256};
use flash_executor::vanity::{
    compute_create2_address, has_leading_zero_bytes, VanityMiner, CREATE2_PREIMAGE_LEN, CREATE2_PREFIX,
};

fn bench_create2_single_hash(c: &mut Criterion) {
    let deployer = Address::from([0x11; 20]);
    let salt = [0x22; 32];
    let init_code_hash = [0x33; 32];

    c.bench_function("create2_single_address_derivation", |b| {
        b.iter(|| {
            compute_create2_address(
                black_box(deployer),
                black_box(salt),
                black_box(init_code_hash),
            )
        })
    });
}

fn bench_raw_keccak_inner_loop(c: &mut Criterion) {
    let mut group = c.benchmark_group("vanity_inner_loop");
    group.throughput(Throughput::Elements(10_000));

    let mut buffer = [0u8; CREATE2_PREIMAGE_LEN];
    buffer[0] = CREATE2_PREFIX;
    buffer[1..21].copy_from_slice(&[0x11; 20]);
    buffer[53..85].copy_from_slice(&[0x33; 32]);

    group.bench_function("10k_keccak_in_place_nonce_increment", |b| {
        b.iter(|| {
            let mut local_nonce = 0u64;
            for _ in 0..10_000 {
                buffer[45..53].copy_from_slice(&local_nonce.to_le_bytes());
                let hash = keccak256(buffer);
                let _ = black_box(has_leading_zero_bytes(&hash.0, 8));
                local_nonce = local_nonce.wrapping_add(1);
            }
        })
    });

    group.finish();
}

fn bench_vanity_miner_multithreaded(c: &mut Criterion) {
    let mut group = c.benchmark_group("vanity_miner_multithreaded");

    let deployer = Address::from([0xAA; 20]);
    let init_code_hash = [0xBB; 32];

    for threads in [1, 2, 4].iter() {
        group.bench_with_input(
            BenchmarkId::new("mine_target_2_zero_bytes", threads),
            threads,
            |b, &t| {
                b.iter(|| {
                    let miner = VanityMiner::new(deployer, init_code_hash)
                        .with_target_zero_bytes(2)
                        .with_threads(t)
                        .with_batch_size(1024);
                    let res = miner.mine().expect("Mine failed");
                    black_box(res);
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_create2_single_hash,
    bench_raw_keccak_inner_loop,
    bench_vanity_miner_multithreaded
);
criterion_main!(benches);
