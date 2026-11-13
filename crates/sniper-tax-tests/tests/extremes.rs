//! Extreme values through the real router: a supply near the u64 limit, dust at 99% tax,
//! and trades that try to empty the pool.

use sniper_tax_tests::*;

/// Buys `amount_in` and checks the trader received exactly the quote.
fn buy_as_quoted(env: &mut Env, pool: &Pool, trader: &Keypair, amount_in: u64) -> u64 {
    let quote = env
        .quote(
            pool,
            Side::Buy,
            TradeMode::ExactIn,
            amount_in,
            Some(trader.pubkey()),
        )
        .unwrap();
    let account = env.token_account(pool, &trader.pubkey());
    let before = env.token_balance(&account);
    env.swap(
        pool,
        trader,
        Side::Buy,
        exact_in(amount_in, quote.amount_out),
    )
    .expect("buy");
    assert_eq!(env.token_balance(&account) - before, quote.amount_out);
    quote.amount_out
}

#[test]
fn a_supply_near_the_u64_limit_trades_without_overflow() {
    let mut env = Env::new();
    // About 9.2e18 raw units in the pool and 1.0e19 minted in total.
    let pool = env.launched_pool(PoolOptions {
        token_amount: u64::MAX / 2,
        quote_amount: 1_000 * SOL,
        ..PoolOptions::default()
    });
    env.assert_solvent(&pool);

    // A taxed whale at launch, then a bigger one after the window.
    let whale = env.buyer(&pool, 3_000 * SOL);
    let taxed = buy_as_quoted(&mut env, &pool, &whale, 500 * SOL);
    env.set_time(LAUNCH_TS + 1_000);
    let untaxed = buy_as_quoted(&mut env, &pool, &whale, 2_000 * SOL);
    assert!(untaxed > taxed);
    env.assert_solvent(&pool);

    // Selling it all back (bar Pipeline's fee) still fits.
    let tokens = env.token_balance(&env.token_account(&pool, &whale.pubkey()));
    let selling = tokens / 100 * 99;
    let quote = env
        .quote(&pool, Side::Sell, TradeMode::ExactIn, selling, None)
        .unwrap();
    env.swap(
        &pool,
        &whale,
        Side::Sell,
        exact_in(selling, quote.amount_out),
    )
    .expect("sell");
    let state = env.pool_state(&pool);
    assert!(state.reserve_quote > 0 && state.reserve_token > 0);
    env.assert_solvent(&pool);
}

#[test]
fn dust_at_99_percent_is_all_tax() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let buyer = env.buyer(&pool, SOL);
    let quote_account = env.quote_account(&pool, &buyer.pubkey());

    // One lamport: all of it is tax, nothing is bought, and the router still takes its
    // fee, which rounds up to one lamport.
    let before = env.pool_state(&pool);
    env.swap(&pool, &buyer, Side::Buy, exact_in(1, 0))
        .expect("dust buy");
    let after = env.pool_state(&pool);
    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        0
    );
    assert_eq!(after.accrued_tax_quote - before.accrued_tax_quote, 1);
    assert_eq!(after.accrued_protocol_quote, before.accrued_protocol_quote);
    assert_eq!(after.reserve_quote, before.reserve_quote);
    assert_eq!(SOL - env.token_balance(&quote_account), 2);

    // A hundred lamports: 99 tax, one lamport trades.
    env.swap(&pool, &buyer, Side::Buy, exact_in(100, 0))
        .expect("small buy");
    let later = env.pool_state(&pool);
    assert_eq!(later.reserve_quote, after.reserve_quote + 1);
    assert_eq!(
        (later.accrued_tax_quote + later.accrued_protocol_quote)
            - (after.accrued_tax_quote + after.accrued_protocol_quote),
        99
    );
    env.assert_solvent(&pool);
}

#[test]
fn the_pool_can_never_be_emptied() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    env.set_time(LAUNCH_TS + 1_000);
    let whale = env.buyer(&pool, 5_000 * SOL);
    let reserve_token = env.pool_state(&pool).reserve_token;

    // Asking for every token, or all but one, can't be priced or traded.
    for want in [reserve_token, reserve_token - 1] {
        assert!(env
            .quote(&pool, Side::Buy, TradeMode::ExactOut, want, None)
            .is_err());
        assert!(env
            .swap(&pool, &whale, Side::Buy, exact_out(4_000 * SOL, want))
            .is_err());
    }

    // A buy 800 times the pool's SOL takes almost every token, but never the last one.
    buy_as_quoted(&mut env, &pool, &whale, 4_000 * SOL);
    let state = env.pool_state(&pool);
    assert!(state.reserve_token > 0);
    assert!(state.reserve_token < reserve_token / 500);
    env.assert_solvent(&pool);

    // The other way: selling floods the pool with tokens but never drains its SOL.
    let tokens = env.token_balance(&env.token_account(&pool, &whale.pubkey()));
    let selling = tokens / 100 * 99;
    env.swap(&pool, &whale, Side::Sell, exact_in(selling, 1))
        .expect("sell");
    assert!(env.pool_state(&pool).reserve_quote > 0);
    env.assert_solvent(&pool);
}

#[test]
fn exact_out_at_the_edge_refunds_exactly() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let buyer = env.buyer(&pool, 100 * SOL);
    let account = env.quote_account(&pool, &buyer.pubkey());

    // At 99% tax, one raw token unit still costs at least 100 lamports in total.
    let quote = env
        .quote(&pool, Side::Buy, TradeMode::ExactOut, 1, None)
        .unwrap();
    assert!(quote.amount_in >= 100);
    // Offer far more than needed: everything above the quote comes back.
    env.swap(&pool, &buyer, Side::Buy, exact_out(50 * SOL, 1))
        .expect("buy one unit");
    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        1
    );
    assert_eq!(
        100 * SOL - env.token_balance(&account),
        quote.amount_in + ceil_bps(quote.amount_in, 25)
    );
    env.assert_solvent(&pool);
}
