use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use wasbox::bench_suite::{benchmark_cases, WasboxHarness};

fn bench_wasbox_invoke(c: &mut Criterion) {
    let mut group = c.benchmark_group("wasbox_invoke");
    for case in benchmark_cases() {
        group.bench_function(case.name, |b| {
            b.iter_batched(
                || {
                    WasboxHarness::new(case)
                        .expect("failed to create harness")
                        .0
                },
                |mut harness| harness.invoke_once().expect("invoke failed"),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_wasbox_instantiate(c: &mut Criterion) {
    let mut group = c.benchmark_group("wasbox_instantiate");
    for case in benchmark_cases() {
        group.bench_function(case.name, |b| {
            b.iter(|| {
                let _ = WasboxHarness::new(case).expect("failed to instantiate");
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_wasbox_invoke, bench_wasbox_instantiate);
criterion_main!(benches);
