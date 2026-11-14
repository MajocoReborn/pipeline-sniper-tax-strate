//! Token-2022 transfer hooks on the quote mint: every transfer the program makes must
//! forward the hook's extra accounts. The test hook counts the transfers of its mint in
//! its one extra account, so each step checks exactly how many hooked transfers ran.
//!
//! A launch token may not have a hook (finding H2), so the quote mint carries it here,
//! through every path: `launch`, a buy's refund, a sell's output, `claim_tax` and
//! `sweep_protocol_fees`. A hooked launch token is refused in `findings.rs`.

use sniper_tax_tests::*;

/// Sends `ix` with `hooks` appended as remaining accounts.
fn with_hooks(mut ix: Instruction, hooks: &[AccountMeta]) -> Instruction {
    ix.accounts.extend_from_slice(hooks);
    ix
}

#[test]
fn a_hooked_quote_mint_works_for_every_transfer() {
    let mut env = Env::new();
    let hook = env.load_test_hook();
    let (pool, created) = env.create_pool(PoolOptions {
        quote: QuoteMint::New(MintSpec::token_2022(9).with_transfer_hook(hook)),
        ..PoolOptions::default()
    });
    created.expect("create_market");
    let hooks = test_hook::accounts(&pool.quote_mint);
    let creator = pool.creator.insecure_clone();
    let launch = pool.launch_ix(
        &env.treasury,
        pool.options.token_amount,
        pool.options.quote_amount,
    );

    // Without the hook's accounts the quote transfer can't run.
    assert!(env
        .send(std::slice::from_ref(&launch), &[&creator])
        .is_err());
    env.send(&[with_hooks(launch, &hooks)], &[&creator])
        .expect("launch");
    assert_eq!(env.hook_count(&pool.quote_mint), 1);
    env.assert_solvent(&pool);
    // Past the launch block, so the tax is split between the creator and the protocol.
    env.skip_launch_block();

    // A taxed exact-out buy: the router sends the maximum in, the program refunds the
    // rest, the router takes its fee. Three hooked transfers.
    let buyer = env.buyer(&pool, 50 * SOL);
    let want = 1_000_000_000;
    let quote = env
        .quote(&pool, Side::Buy, TradeMode::ExactOut, want, None)
        .unwrap();
    let ix = pool.swap_ix_with_hooks(
        &buyer.pubkey(),
        Side::Buy,
        &exact_out(quote.amount_in * 2, want),
        vec![],
        hooks.clone(),
    );
    env.send(&[ix], &[&buyer]).expect("buy");
    assert_eq!(env.hook_count(&pool.quote_mint), 4);
    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        want
    );
    let state = env.pool_state(&pool);
    assert!(state.accrued_tax_quote > 0 && state.accrued_protocol_quote > 0);

    // A sell pays out the hooked quote mint: its hook accounts go after the output vault.
    // The router takes its fee in the launch token, so one hooked transfer.
    let quote_account = env.quote_account(&pool, &buyer.pubkey());
    let selling = want / 4;
    let quote = env
        .quote(&pool, Side::Sell, TradeMode::ExactIn, selling, None)
        .unwrap();
    let args = exact_in(selling, quote.amount_out);
    let before = env.pool_state(&pool);
    let bare = pool.swap_ix(&buyer.pubkey(), Side::Sell, &args);
    assert!(env.send(&[bare], &[&buyer]).is_err());
    assert_eq!(env.pool_state(&pool), before);
    let received = env.token_balance(&quote_account);
    let ix = pool.swap_ix_with_hooks(&buyer.pubkey(), Side::Sell, &args, hooks.clone(), vec![]);
    env.send(&[ix], &[&buyer]).expect("sell");
    assert_eq!(
        env.token_balance(&quote_account) - received,
        quote.amount_out
    );
    assert_eq!(env.hook_count(&pool.quote_mint), 5);

    // An exact-out sell: the unused tokens are refunded in the launch token (no hook),
    // the quote is paid out through the hook.
    // The buyer holds few tokens, so a small amount of quote.
    let want_quote = 1_000;
    let quote = env
        .quote(&pool, Side::Sell, TradeMode::ExactOut, want_quote, None)
        .unwrap();
    let received = env.token_balance(&quote_account);
    let ix = pool.swap_ix_with_hooks(
        &buyer.pubkey(),
        Side::Sell,
        &exact_out(quote.amount_in * 2, want_quote),
        hooks.clone(),
        vec![],
    );
    env.send(&[ix], &[&buyer]).expect("exact-out sell");
    assert_eq!(env.token_balance(&quote_account) - received, want_quote);
    assert_eq!(env.hook_count(&pool.quote_mint), 6);
    env.assert_solvent(&pool);

    // The creator's claim needs the hook's accounts too.
    let claim = pool.claim_tax_ix(&pool.creator_quote());
    assert!(env.send(std::slice::from_ref(&claim), &[&creator]).is_err());
    let before = env.token_balance(&pool.creator_quote());
    env.send(&[with_hooks(claim, &hooks)], &[&creator])
        .expect("claim");
    assert_eq!(
        env.token_balance(&pool.creator_quote()) - before,
        state.accrued_tax_quote
    );
    assert_eq!(env.hook_count(&pool.quote_mint), 7);

    // And so does the sweep to the treasury.
    let treasury = env.treasury;
    let treasury_quote = env.create_ata(&treasury, &pool.quote_mint, &pool.quote_program);
    let sweep = pool.sweep_ix(&treasury, &treasury_quote);
    assert!(env.send(std::slice::from_ref(&sweep), &[]).is_err());
    env.send(&[with_hooks(sweep, &hooks)], &[]).expect("sweep");
    assert_eq!(
        env.token_balance(&treasury_quote),
        state.accrued_protocol_quote
    );
    assert_eq!(env.hook_count(&pool.quote_mint), 8);
    env.assert_solvent(&pool);
}
