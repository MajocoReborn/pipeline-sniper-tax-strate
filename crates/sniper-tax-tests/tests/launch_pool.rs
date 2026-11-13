//! The sniper-tax program against the real Pipeline router (docs/PLAN.md section 9).

use sniper_tax_math::{Pricing, TaxSchedule};
use sniper_tax_tests::*;

fn schedule(state: &LaunchPool) -> TaxSchedule {
    TaxSchedule::new(
        state.tax_start_bps,
        state.tax_end_bps,
        state.tax_duration_secs,
    )
    .unwrap()
}

/// What the pool's books say a buy of `amount_in` pays and receives at `now`.
fn expected_buy(
    state: &LaunchPool,
    now: i64,
    amount_in: u64,
    is_creator: bool,
) -> sniper_tax_math::Quote {
    let tax_bps = if is_creator {
        0
    } else {
        schedule(state).tax_bps_at(state.launch_ts, now)
    };
    Pricing {
        reserve_in: state.reserve_quote,
        reserve_out: state.reserve_token,
        tax_bps,
        protocol_share_bps: state.protocol_tax_share_bps,
    }
    .quote_exact_in(amount_in)
    .unwrap()
}

// ------------------------------------------------------------------------- launch

#[test]
fn create_and_launch_fit_one_transaction() {
    let mut env = Env::new();
    let treasury_before = env.lamports(&env.treasury);
    let pool = env.launched_pool(PoolOptions::default());

    let state = env.pool_state(&pool);
    assert_eq!(state.status, PoolStatus::Live);
    assert_eq!(state.creator, pool.creator.pubkey());
    assert_eq!(state.launch_ts, LAUNCH_TS);
    assert_eq!(state.reserve_token, pool.options.token_amount);
    assert_eq!(state.reserve_quote, pool.options.quote_amount);
    assert_eq!(state.protocol_tax_share_bps, PROTOCOL_SHARE_BPS);
    assert_eq!(state.launch_fee_lamports, LAUNCH_FEE);
    assert_eq!(env.lamports(&env.treasury) - treasury_before, LAUNCH_FEE);

    // The router recorded the market with this pool's mints and vaults.
    let market = router::decode_market(&env.account_data(&pool.market)).expect("market");
    assert_eq!(market.mints(), &[pool.token_mint, pool.quote_mint]);
    assert_eq!(market.vault_for(&pool.quote_mint), Some(pool.quote_vault));
    env.assert_solvent(&pool);
}

#[test]
fn a_pending_pool_cannot_trade_and_only_its_creator_launches_it_once() {
    let mut env = Env::new();
    let (pool, created) = env.create_pool(PoolOptions::default());
    created.expect("create_market");
    assert_eq!(env.pool_state(&pool).status, PoolStatus::Pending);

    let buyer = env.buyer(&pool, 2 * SOL);
    assert_fails_with(
        env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0)),
        "NotLive",
    );

    // Someone else signs as creator, with the creator's token accounts.
    let intruder = env.funded_keypair();
    let mut ix = pool.launch_ix(&env.treasury, 1, 1);
    ix.accounts[0].pubkey = intruder.pubkey();
    assert_fails_with(env.send(&[ix], &[&intruder]), "NotCreator");

    env.launch(&pool).expect("launch");
    assert_fails_with(env.launch(&pool), "AlreadyLaunched");
}

#[test]
fn launch_requires_revoked_mint_and_freeze_authorities() {
    let mut env = Env::new();
    let (pool, created) = env.create_pool(PoolOptions {
        revoke_mint_authority: false,
        ..PoolOptions::default()
    });
    created.expect("create_market");
    assert_fails_with(env.launch(&pool), "MintAuthorityPresent");

    let (pool, created) = env.create_pool(PoolOptions {
        token: MintSpec::spl(6).with_freeze_authority(),
        ..PoolOptions::default()
    });
    created.expect("create_market");
    assert_fails_with(env.launch(&pool), "FreezeAuthorityPresent");
}

#[test]
fn a_permanent_delegate_mint_is_refused() {
    let mut env = Env::new();
    let (_, created) = env.create_pool(PoolOptions {
        token: MintSpec::token_2022(6).with_permanent_delegate(),
        ..PoolOptions::default()
    });
    assert_fails_with(created, "ForbiddenMintExtension");
}

#[test]
fn bad_schedules_are_refused() {
    let mut env = Env::new();
    for (start, end, duration) in [
        (9_901, 0, 90),
        (9_900, 501, 90),
        (9_900, 0, 59),
        (9_900, 0, 1_801),
        (100, 200, 90),
    ] {
        let (_, created) = env.create_pool(PoolOptions {
            tax_start_bps: start,
            tax_end_bps: end,
            tax_duration_secs: duration,
            ..PoolOptions::default()
        });
        assert_fails_with(created, "InvalidSchedule");
    }
}

// ------------------------------------------------------------------------- the tax

/// Buys 1 SOL at each `(seconds after launch, expected rate)` and checks the tax, its
/// split, the reserves and the output.
fn check_schedule(duration_secs: u32, points: &[(i64, u16)]) {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions {
        tax_duration_secs: duration_secs,
        ..PoolOptions::default()
    });

    for &(elapsed, rate_bps) in points {
        let now = LAUNCH_TS + elapsed;
        env.set_time(now);
        let before = env.pool_state(&pool);
        let buyer = env.buyer(&pool, 2 * SOL);
        let expected = expected_buy(&before, now, SOL, false);

        env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0))
            .expect("buy");

        let after = env.pool_state(&pool);
        let creator_tax = after.accrued_tax_quote - before.accrued_tax_quote;
        let protocol_fee = after.accrued_protocol_quote - before.accrued_protocol_quote;
        assert_eq!(
            creator_tax + protocol_fee,
            ceil_bps(SOL, rate_bps),
            "tax after {elapsed} s"
        );
        assert_eq!(
            protocol_fee,
            (creator_tax + protocol_fee) / 10,
            "protocol share after {elapsed} s"
        );
        assert_eq!(
            after.reserve_quote,
            before.reserve_quote + SOL - creator_tax - protocol_fee
        );
        assert_eq!(
            env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
            expected.amount_out
        );
        env.assert_solvent(&pool);
    }
}

#[test]
fn the_tax_follows_a_ninety_second_schedule() {
    check_schedule(
        90,
        &[
            (0, 9_900),
            (30, 6_600),
            (45, 4_950),
            (89, 110),
            (90, 0),
            (600, 0),
        ],
    );
}

#[test]
fn the_tax_follows_a_ten_minute_schedule() {
    check_schedule(
        600,
        &[
            (0, 9_900),
            (60, 8_910),
            (300, 4_950),
            (540, 990),
            (599, 16),
            (600, 0),
            (601, 0),
        ],
    );
}

#[test]
fn a_sniper_at_launch_pays_99_percent() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let sniper = env.buyer(&pool, 20 * SOL);
    env.swap(&pool, &sniper, Side::Buy, exact_in(10 * SOL, 0))
        .expect("buy");

    // 10 SOL in: 9.9 SOL tax, of which 0.99 SOL to the protocol and 8.91 SOL to the
    // creator; 0.1 SOL trades. Pipeline's 0.25% comes on top.
    let state = env.pool_state(&pool);
    assert_eq!(state.accrued_protocol_quote, 990_000_000);
    assert_eq!(state.accrued_tax_quote, 8_910_000_000);
    assert_eq!(state.reserve_quote, pool.options.quote_amount + SOL / 10);
    let spent = 20 * SOL - env.token_balance(&env.quote_account(&pool, &sniper.pubkey()));
    assert_eq!(spent, 10 * SOL + 25_000_000);
}

#[test]
fn the_creator_trades_tax_free() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let before = env.pool_state(&pool);
    let expected = expected_buy(&before, LAUNCH_TS, SOL, true);
    let creator = pool.creator.insecure_clone();

    env.swap(&pool, &creator, Side::Buy, exact_in(SOL, 0))
        .expect("creator buy");

    let after = env.pool_state(&pool);
    assert_eq!(after.accrued_tax_quote + after.accrued_protocol_quote, 0);
    assert_eq!(after.reserve_quote, before.reserve_quote + SOL);
    assert_eq!(
        after.reserve_token,
        before.reserve_token - expected.amount_out
    );
}

#[test]
fn sells_are_never_taxed() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let buyer = env.buyer(&pool, 2 * SOL);
    env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0))
        .expect("buy");
    // Half, so the trader can still pay Pipeline's fee in tokens.
    let tokens = env.token_balance(&env.token_account(&pool, &buyer.pubkey())) / 2;
    let before = env.pool_state(&pool);

    // Still inside the window, where a buy would pay 99%.
    env.swap(&pool, &buyer, Side::Sell, exact_in(tokens, 1))
        .expect("sell");

    let after = env.pool_state(&pool);
    assert_eq!(after.accrued_tax_quote, before.accrued_tax_quote);
    assert_eq!(after.accrued_protocol_quote, before.accrued_protocol_quote);
    assert_eq!(after.reserve_token, before.reserve_token + tokens);
    env.assert_solvent(&pool);
}

#[test]
fn quotes_match_swaps() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    env.set_time(LAUNCH_TS + 20);
    let buyer = env.buyer(&pool, 5 * SOL);

    let named = env
        .quote(
            &pool,
            Side::Buy,
            TradeMode::ExactIn,
            SOL,
            Some(buyer.pubkey()),
        )
        .unwrap();
    let anonymous = env
        .quote(&pool, Side::Buy, TradeMode::ExactIn, SOL, None)
        .unwrap();
    let as_creator = env
        .quote(
            &pool,
            Side::Buy,
            TradeMode::ExactIn,
            SOL,
            Some(pool.creator.pubkey()),
        )
        .unwrap();
    assert_eq!(named, anonymous);
    assert!(as_creator.amount_out > named.amount_out * 3);

    env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, named.amount_out))
        .expect("buy");
    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        named.amount_out
    );

    // A minimum above the quote fails.
    let quote = env
        .quote(&pool, Side::Buy, TradeMode::ExactIn, SOL, None)
        .unwrap();
    let other = env.buyer(&pool, 5 * SOL);
    assert!(env
        .swap(
            &pool,
            &other,
            Side::Buy,
            exact_in(SOL, quote.amount_out + 1)
        )
        .is_err());
}

#[test]
fn exact_out_buys_pay_the_tax_and_refund_the_rest() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    env.set_time(LAUNCH_TS + 30);
    let buyer = env.buyer(&pool, 10 * SOL);
    let want = 1_000_000_000_000;
    let quote = env
        .quote(&pool, Side::Buy, TradeMode::ExactOut, want, None)
        .unwrap();
    assert_eq!(quote.amount_out, want);

    env.swap(
        &pool,
        &buyer,
        Side::Buy,
        exact_out(quote.amount_in * 2, want),
    )
    .expect("buy");

    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        want
    );
    let spent = 10 * SOL - env.token_balance(&env.quote_account(&pool, &buyer.pubkey()));
    // Only what the trade needed, plus Pipeline's fee on it.
    assert_eq!(spent, quote.amount_in + ceil_bps(quote.amount_in, 25));
    env.assert_solvent(&pool);
}

#[test]
fn donations_do_not_move_the_price() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    env.set_time(LAUNCH_TS + 100);
    let quote_vault = pool.quote_vault;
    env.mint_to(&pool.quote_mint, &pool.quote_program, &quote_vault, 3 * SOL);

    let before = env.pool_state(&pool);
    let expected = expected_buy(&before, LAUNCH_TS + 100, SOL, false);
    let buyer = env.buyer(&pool, 2 * SOL);
    env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0))
        .expect("buy");
    assert_eq!(
        env.token_balance(&env.token_account(&pool, &buyer.pubkey())),
        expected.amount_out
    );
}

#[test]
fn swaps_and_quotes_stay_well_within_compute_limits() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let buyer = env.buyer(&pool, 5 * SOL);
    let (_, quote_units) = env
        .simulate_quote(&pool, Side::Buy, TradeMode::ExactIn, SOL, None)
        .unwrap();
    // Pipeline's quoter caps quotes at 200,000 units.
    assert!(quote_units < 50_000, "quote used {quote_units} units");
    let meta = env
        .swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0))
        .expect("buy");
    assert!(
        meta.compute_units_consumed < 80_000,
        "swap used {} units",
        meta.compute_units_consumed
    );
}

// ------------------------------------------------------------------------- claims

#[test]
fn the_creator_claims_their_tax_once() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let sniper = env.buyer(&pool, 5 * SOL);
    env.swap(&pool, &sniper, Side::Buy, exact_in(2 * SOL, 0))
        .expect("buy");
    let owed = env.pool_state(&pool).accrued_tax_quote;
    assert!(owed > 0);

    // Someone else can't claim it, not even to the creator's account.
    let intruder = env.funded_keypair();
    let mut ix = pool.claim_tax_ix(&pool.creator_quote());
    ix.accounts[0].pubkey = intruder.pubkey();
    assert_fails_with(env.send(&[ix], &[&intruder]), "NotCreator");

    let before = env.token_balance(&pool.creator_quote());
    let creator = pool.creator.insecure_clone();
    let ix = pool.claim_tax_ix(&pool.creator_quote());
    env.send(&[ix], &[&creator]).expect("claim");
    assert_eq!(env.token_balance(&pool.creator_quote()) - before, owed);
    assert_eq!(env.pool_state(&pool).accrued_tax_quote, 0);
    env.assert_solvent(&pool);

    let ix = pool.claim_tax_ix(&pool.creator_quote());
    assert_fails_with(env.send(&[ix], &[&creator]), "NothingToClaim");

    // The creator can't "claim" into the vault itself (Anchor already refuses the same
    // writable account twice).
    env.swap(&pool, &sniper, Side::Buy, exact_in(SOL, 0))
        .expect("buy");
    let ix = pool.claim_tax_ix(&pool.quote_vault);
    assert_fails_with(
        env.send(&[ix], &[&creator]),
        "ConstraintDuplicateMutableAccount",
    );
}

#[test]
fn protocol_tax_is_swept_only_to_the_treasury() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let sniper = env.buyer(&pool, 5 * SOL);
    env.swap(&pool, &sniper, Side::Buy, exact_in(2 * SOL, 0))
        .expect("buy");
    let owed = env.pool_state(&pool).accrued_protocol_quote;
    let treasury = env.treasury;
    let treasury_quote = env.create_ata(&treasury, &pool.quote_mint, &pool.quote_program);

    // Pointing the sweep at another account, or another treasury, fails.
    let ix = pool.sweep_ix(&treasury, &pool.creator_quote());
    assert!(env.send(&[ix], &[]).is_err());
    let impostor = Pubkey::new_unique();
    let impostor_quote = env.create_ata(&impostor, &pool.quote_mint, &pool.quote_program);
    let ix = pool.sweep_ix(&impostor, &impostor_quote);
    assert_fails_with(env.send(&[ix], &[]), "WrongTreasury");

    // Anyone may sweep to the treasury.
    let ix = pool.sweep_ix(&treasury, &treasury_quote);
    env.send(&[ix], &[]).expect("sweep");
    assert_eq!(env.token_balance(&treasury_quote), owed);
    assert_eq!(env.pool_state(&pool).accrued_protocol_quote, 0);
    env.assert_solvent(&pool);
}

// ------------------------------------------------------------------------- attacks

/// The accounts the router passes to `pipeline_swap` for a buy, with `market_signer`
/// signing in its place, and the output vault.
fn strategy_swap_accounts(
    pool: &PoolKeys,
    market_signer: Pubkey,
    trader: &Pubkey,
) -> Vec<AccountMeta> {
    let legs = pool.legs(Side::Buy);
    vec![
        AccountMeta::new_readonly(market_signer, true),
        AccountMeta::new(pool.address, false),
        AccountMeta::new_readonly(*trader, false),
        AccountMeta::new(
            router_pda::associated_token(trader, &legs.input_mint, &legs.input_program),
            false,
        ),
        AccountMeta::new(
            router_pda::associated_token(trader, &legs.output_mint, &legs.output_program),
            false,
        ),
        AccountMeta::new(legs.input_vault, false),
        AccountMeta::new_readonly(legs.input_mint, false),
        AccountMeta::new_readonly(legs.output_mint, false),
        AccountMeta::new_readonly(legs.input_program, false),
        AccountMeta::new_readonly(legs.output_program, false),
        AccountMeta::new(legs.output_vault, false),
    ]
}

#[test]
fn swaps_only_come_through_the_router() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions::default());
    let attacker = env.buyer(&pool, 5 * SOL);
    let request = psi::SwapRequest {
        trade: psi::TradeRequest {
            mode: TradeMode::ExactIn,
            input_index: 1,
            output_index: 0,
            amount: SOL,
            // Claiming to be the creator, to dodge the tax.
            trader: Some(pool.creator.pubkey().to_bytes()),
            extra: vec![],
        },
        amount_sent: SOL,
        min_amount_out: 0,
    };
    // The attacker signs as the market signer.
    let accounts = strategy_swap_accounts(&pool, attacker.pubkey(), &pool.creator.pubkey());
    let ix = program_ix("pipeline_swap", &request, accounts);
    assert_fails_with(env.send(&[ix], &[&attacker]), "NotPipeline");
}

#[test]
fn vaults_created_in_advance_do_not_block_a_pool() {
    let mut env = Env::new();
    let pool = env.prepare_pool(PoolOptions::default());
    let address = pool.address;
    env.create_ata(&address, &pool.token_mint, &pool.token_program);
    env.create_ata(&address, &pool.quote_mint, &pool.quote_program);

    let admin = env.admin.pubkey();
    let mut ixs = vec![pool.create_market_ix(&admin)];
    ixs.extend(pool.fee_account_ixs(&admin));
    env.send(&ixs, &[]).expect("create_market");
    env.launch(&pool).expect("launch");
    env.assert_solvent(&pool);
}

#[test]
fn creating_the_creators_pool_first_gains_nothing() {
    // `create_pool` has the admin, not the creator, call `create_market`.
    let mut env = Env::new();
    let (pool, created) = env.create_pool(PoolOptions::default());
    created.expect("a stranger creates the creator's pool");

    // The creator's own create_market now finds the pool taken, but the pool is exactly
    // theirs: same creator, same schedule, and only they can launch it.
    let creator = pool.creator.insecure_clone();
    let ix = pool.create_market_ix(&creator.pubkey());
    assert!(env.send(&[ix], &[&creator]).is_err());
    let state = env.pool_state(&pool);
    assert_eq!(state.creator, creator.pubkey());
    assert_eq!(
        (
            state.tax_start_bps,
            state.tax_end_bps,
            state.tax_duration_secs
        ),
        (9_900, 0, 90)
    );
    env.launch(&pool).expect("the creator launches");
}

#[test]
fn register_existing_market_rejects_accounts_that_are_not_pools() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let ix = router::register_existing_market(&admin, &PROGRAM_ID, &config_address(), vec![]);
    assert!(env.send(&[ix], &[]).is_err());
}

#[test]
fn a_transfer_fee_quote_mint_never_spends_unclaimed_tax() {
    // With a transfer fee less arrives than the router sends. Input must be credited from
    // what arrived above the books, which include unclaimed tax.
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions {
        quote: QuoteMint::New(MintSpec::token_2022(9).with_transfer_fee(100, u64::MAX)),
        ..PoolOptions::default()
    });
    let sniper = env.buyer(&pool, 20 * SOL);
    env.swap(&pool, &sniper, Side::Buy, exact_in(5 * SOL, 0))
        .expect("taxed buy");
    let unclaimed = env.pool_state(&pool);
    assert!(unclaimed.accrued_tax_quote > 0);

    env.set_time(LAUNCH_TS + 1_000);
    let before = env.pool_state(&pool);
    let vault_before = env.token_balance(&pool.quote_vault);
    let buyer = env.buyer(&pool, 5 * SOL);
    env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0))
        .expect("untaxed buy");
    let arrived = env.token_balance(&pool.quote_vault) - vault_before;

    let after = env.pool_state(&pool);
    assert_eq!(arrived, SOL - SOL / 100);
    assert_eq!(after.reserve_quote - before.reserve_quote, arrived);
    assert_eq!(after.accrued_tax_quote, before.accrued_tax_quote);
    env.assert_solvent(&pool);
}

#[test]
fn a_transfer_fee_launch_token_is_priced_on_what_arrives() {
    let mut env = Env::new();
    let pool = env.launched_pool(PoolOptions {
        token: MintSpec::token_2022(6).with_transfer_fee(200, u64::MAX),
        ..PoolOptions::default()
    });
    let state = env.pool_state(&pool);
    let deposit = pool.options.token_amount;
    assert_eq!(state.reserve_token, deposit - ceil_bps(deposit, 200));

    env.set_time(LAUNCH_TS + 1_000);
    let buyer = env.buyer(&pool, 5 * SOL);
    env.swap(&pool, &buyer, Side::Buy, exact_in(SOL, 0))
        .expect("buy");
    let tokens = env.token_balance(&env.token_account(&pool, &buyer.pubkey()));
    let sent = state.reserve_token - env.pool_state(&pool).reserve_token;
    assert_eq!(tokens, sent - ceil_bps(sent, 200));

    // Half, so the trader can still pay Pipeline's fee in tokens.
    let selling = tokens / 2;
    let before = env.pool_state(&pool);
    env.swap(&pool, &buyer, Side::Sell, exact_in(selling, 1))
        .expect("sell");
    let after = env.pool_state(&pool);
    assert_eq!(
        after.reserve_token - before.reserve_token,
        selling - ceil_bps(selling, 200)
    );
    env.assert_solvent(&pool);
}

// ------------------------------------------------------------------------- config

#[test]
fn only_the_upgrade_authority_creates_the_config() {
    let mut env = Env::without_protocol_config();
    let stranger = env.funded_keypair();
    let args = env.default_config();
    let ix = initialize_config_ix(&stranger.pubkey(), &args);
    assert_fails_with(env.send(&[ix], &[&stranger]), "NotUpgradeAuthority");

    let ix = initialize_config_ix(&env.admin.pubkey(), &args);
    env.send(&[ix], &[]).expect("initialize_config");
    assert_eq!(
        env.config_state().protocol_tax_share_bps,
        PROTOCOL_SHARE_BPS
    );
}

#[test]
fn the_config_is_capped_and_changes_reach_new_pools_only() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();

    let stranger = env.funded_keypair();
    let ix = update_config_ix(&stranger.pubkey(), &env.default_update());
    assert_fails_with(env.send(&[ix], &[&stranger]), "NotAdmin");
    for args in [
        ConfigUpdate {
            protocol_tax_share_bps: 2_001,
            ..env.default_update()
        },
        ConfigUpdate {
            launch_fee_lamports: 50_000_001,
            ..env.default_update()
        },
    ] {
        assert_fails_with(
            env.send(&[update_config_ix(&admin, &args)], &[]),
            "FeeAboveCap",
        );
    }

    // A pool created before the change keeps its share, and pays the lower launch fee.
    let (old, created) = env.create_pool(PoolOptions::default());
    created.expect("create_market");
    let raised = ConfigUpdate {
        protocol_tax_share_bps: 2_000,
        launch_fee_lamports: 0,
        ..env.default_update()
    };
    env.send(&[update_config_ix(&admin, &raised)], &[])
        .expect("update_config");

    let treasury_before = env.lamports(&env.treasury);
    env.set_time(LAUNCH_TS);
    env.launch(&old).expect("launch");
    assert_eq!(env.lamports(&env.treasury), treasury_before);
    assert_eq!(
        env.pool_state(&old).protocol_tax_share_bps,
        PROTOCOL_SHARE_BPS
    );

    let new = env.launched_pool(PoolOptions::default());
    assert_eq!(env.pool_state(&new).protocol_tax_share_bps, 2_000);
}
