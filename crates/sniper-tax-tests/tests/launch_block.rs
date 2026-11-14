//! The launch block: the tax on buys in the launch's slot and the next goes to the protocol
//! in full; from the slot after, the pool's recorded share applies. What a trader pays and
//! receives is the same either way: only the split of the tax changes.

use sniper_tax_math::{split_tax, LAUNCH_BLOCK_SLOTS};
use sniper_tax_tests::*;

/// A 1 SOL buy by a fresh wallet at the current slot; returns how the pool's tax moved
/// (creator, protocol) and the tokens received.
fn buy(env: &mut Env, pool: &Pool) -> (u64, u64, u64) {
    let buyer = env.buyer(pool, 2 * SOL);
    let before = env.pool_state(pool);
    env.swap(pool, &buyer, Side::Buy, exact_in(SOL, 0))
        .expect("buy");
    let after = env.pool_state(pool);
    (
        after.accrued_tax_quote - before.accrued_tax_quote,
        after.accrued_protocol_quote - before.accrued_protocol_quote,
        env.token_balance(&env.token_account(pool, &buyer.pubkey())),
    )
}

#[test]
fn the_launch_records_its_slot() {
    let mut env = Env::new();
    let pool = env.launched_pool_in_launch_block(PoolOptions::default());
    let state = env.pool_state(&pool);
    assert_eq!(state.launch_slot, env.slot());
    assert_ne!(state.launch_slot, 0);
}

#[test]
fn the_tax_in_the_launch_slot_and_the_next_goes_to_the_protocol() {
    let mut env = Env::new();
    let pool = env.launched_pool_in_launch_block(PoolOptions::default());
    let share = env.pool_state(&pool).protocol_tax_share_bps;
    assert!(share > 0 && share < 10_000);

    // The launch slot: all of the 99% tax to the protocol.
    let (creator, protocol, _) = buy(&mut env, &pool);
    assert_eq!(creator, 0);
    assert_eq!(protocol, 990_000_000);

    // The next slot, at the same time: still the launch block.
    env.advance_slots(1);
    let (creator, protocol, _) = buy(&mut env, &pool);
    assert_eq!(creator, 0);
    assert_eq!(protocol, 990_000_000);

    // The slot after that: the pool's recorded split.
    env.advance_slots(LAUNCH_BLOCK_SLOTS - 1);
    let (creator, protocol, _) = buy(&mut env, &pool);
    let expected = split_tax(990_000_000, share);
    assert_eq!(creator, expected.creator);
    assert_eq!(protocol, expected.protocol);
    assert!(creator > protocol);
    env.assert_solvent(&pool);
}

#[test]
fn a_launch_block_buy_gets_the_same_tokens_as_any_buy_at_that_rate() {
    // Two pools in the same state, one bought into in the launch slot, the other past the
    // launch block at the same time: the trader's side is identical.
    let mut env = Env::new();
    let in_block = env.launched_pool_in_launch_block(PoolOptions::default());
    let (_, _, tokens_in_block) = buy(&mut env, &in_block);

    let mut env = Env::new();
    let after_block = env.launched_pool(PoolOptions::default());
    let (_, _, tokens_after) = buy(&mut env, &after_block);

    assert_eq!(tokens_in_block, tokens_after);
}

#[test]
fn quotes_in_the_launch_block_match_the_swap() {
    let mut env = Env::new();
    let pool = env.launched_pool_in_launch_block(PoolOptions::default());
    let buyer = env.buyer(&pool, 2 * SOL);
    let quote = env
        .quote(
            &pool,
            Side::Buy,
            TradeMode::ExactIn,
            SOL,
            Some(buyer.pubkey()),
        )
        .unwrap();
    env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, quote.amount_out))
        .expect("buy at the quoted output");
    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        quote.amount_out
    );
}

#[test]
fn the_creator_and_sellers_still_pay_nothing_in_the_launch_block() {
    let mut env = Env::new();
    let pool = env.launched_pool_in_launch_block(PoolOptions::default());
    let creator = pool.creator.insecure_clone();
    env.swap(&pool, &creator, Side::Buy, exact_in(SOL, 0))
        .expect("creator buy");
    let state = env.pool_state(&pool);
    assert_eq!(state.accrued_tax_quote + state.accrued_protocol_quote, 0);

    // A sell in the next slot: no tax either.
    env.advance_slots(1);
    let tokens = env.token_balance(&env.token_account(&pool, &creator.pubkey())) / 2;
    env.swap(&pool, &creator, Side::Sell, exact_in(tokens, 1))
        .expect("sell");
    let state = env.pool_state(&pool);
    assert_eq!(state.accrued_tax_quote + state.accrued_protocol_quote, 0);
}

#[test]
fn the_launch_block_tax_is_swept_to_the_treasury() {
    let mut env = Env::new();
    let pool = env.launched_pool_in_launch_block(PoolOptions::default());
    let (_, protocol, _) = buy(&mut env, &pool);
    let treasury = env.treasury;
    let treasury_quote = env.create_ata(&treasury, &pool.quote_mint, &pool.quote_program);
    let ix = pool.sweep_ix(&treasury, &treasury_quote);
    env.send(&[ix], &[]).expect("sweep");
    assert_eq!(env.token_balance(&treasury_quote), protocol);
    assert_eq!(env.pool_state(&pool).accrued_protocol_quote, 0);
    env.assert_solvent(&pool);
}

#[test]
fn a_pool_launched_before_the_launch_block_keeps_its_split() {
    // Pools launched by the previous version have zeros where `launch_slot` now is. Make
    // one by clearing the field, still in the launch slot: its buys split as before.
    let mut env = Env::new();
    let pool = env.launched_pool_in_launch_block(PoolOptions::default());
    let mut state = env.pool_state(&pool);
    let share = state.protocol_tax_share_bps;
    state.launch_slot = 0;
    let mut account = env.svm.get_account(&pool.address).unwrap();
    let mut data = router::account_discriminator("LaunchPool").to_vec();
    data.extend(borsh::to_vec(&state).unwrap());
    assert_eq!(data.len(), account.data.len());
    account.data = data;
    env.svm.set_account(pool.address, account).unwrap();

    let (creator, protocol, _) = buy(&mut env, &pool);
    let expected = split_tax(990_000_000, share);
    assert_eq!(creator, expected.creator);
    assert_eq!(protocol, expected.protocol);
}
