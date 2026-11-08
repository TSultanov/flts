#[path = "../../../library/benches/support/allocations.rs"]
mod allocations;
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
fn pending_queue(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let library = runtime
        .block_on(library::library::Library::open(
            directory.path().to_path_buf(),
        ))
        .unwrap();
    let text = (0..5000)
        .map(|i| format!("paragraph {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let id = runtime
        .block_on(library.create_book_plain("Queue benchmark", &text, &isolang::Language::Eng))
        .unwrap();
    let book = runtime.block_on(library.get_book(&id)).unwrap();
    let snapshot = book.snapshot();
    black_box(allocations::measure("fill pending queue 1024 jobs", || {
        app_lib::benchmark_pending_jobs(&book, &snapshot, 1024)
    }));
    c.bench_function("fill pending translation queue (1024 jobs)", |b| {
        b.iter(|| black_box(app_lib::benchmark_pending_jobs(&book, &snapshot, 1024)))
    });
    runtime.block_on(library.close()).unwrap();
}
criterion_group!(benches, pending_queue);
criterion_main!(benches);
