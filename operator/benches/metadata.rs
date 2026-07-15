use criterion::{black_box, criterion_group, criterion_main, Criterion, async_executor::FuturesExecutor};
use chimera_operator::token::metadata::fetch_metadata_from_rpc;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

async fn bench_fetch_metadata(c: &mut Criterion) {
    let token_mint = Pubkey::from_str("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263").unwrap();
    
    c.bench_function("fetch_metadata_from_rpc", |b| {
        b.to_async(FuturesExecutor).iter(|| {
            fetch_metadata_from_rpc(black_box(token_mint))
        })
    });
}

criterion_group!(benches, bench_fetch_metadata);
criterion_main!(benches);