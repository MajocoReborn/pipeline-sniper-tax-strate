//! Findings of the security review (docs/SECURITY-REVIEW.md). Each test first pinned the
//! behaviour the review found; now that the finding is fixed, it asserts the fix.

use sniper_tax_tests::*;
use spl_token_2022_interface::extension::transfer_fee::instruction::set_transfer_fee;
use spl_token_2022_interface::instruction::{set_authority, AuthorityType};

/// The admin, who created the mint, schedules a new transfer fee. Token-2022 applies it
/// two epochs later.
#[allow(clippy::result_large_err)]
fn schedule_fee(env: &mut Env, mint: &Pubkey, bps: u16) -> TxResult {
    let admin = env.admin.pubkey();
    let ix = set_transfer_fee(
        &router::TOKEN_2022_PROGRAM_ID,
        mint,
        &admin,
        &[],
        bps,
        u64::MAX,
    )
    .unwrap();
    env.send(&[ix], &[])
}

/// The admin gives up the transfer fee's config authority: the fee is fixed for good.
fn revoke_fee_authority(env: &mut Env, mint: &Pubkey) {
    let admin = env.admin.pubkey();
    let ix = set_authority(
        &router::TOKEN_2022_PROGRAM_ID,
        mint,
        None,
        AuthorityType::TransferFeeConfig,
        &admin,
        &[],
    )
    .unwrap();
    env.send(&[ix], &[]).expect("revoke the fee authority");
}

/// Pool options with `spec` as the launch token, or as the quote mint.
fn with_mint(spec: MintSpec, as_quote: bool) -> PoolOptions {
    if as_quote {
        PoolOptions {
            quote: QuoteMint::New(spec),
            ..PoolOptions::default()
        }
    } else {
        PoolOptions {
            token: spec,
            ..PoolOptions::default()
        }
    }
}

/// The mint a pool built with [`with_mint`] got from its spec.
fn mint_under_test(pool: &Pool, as_quote: bool) -> Pubkey {
    if as_quote {
        pool.quote_mint
    } else {
        pool.token_mint
    }
}

fn advance_epochs(env: &mut Env, epochs: u64) {
    let mut clock = env.svm.get_sysvar::<solana_clock::Clock>();
    clock.epoch += epochs;
    env.svm.set_sysvar::<solana_clock::Clock>(&clock);
}

/// A Token-2022 spec with the decimals the tests use for that side.
fn token_2022(as_quote: bool) -> MintSpec {
    MintSpec::token_2022(if as_quote { 9 } else { 6 })
}

/// Finding H1: a transfer fee whose authority is kept could be raised to 100% after
/// launch, after which every sell delivers nothing (a honeypot). A pool with such a mint,
/// on either side, can't launch. Once the authority is revoked the fee is fixed, the pool
/// launches, and nobody can raise the fee any more.
#[test]
fn finding_h1_a_mint_with_a_fee_authority_is_refused() {
    let mut env = Env::new();
    for as_quote in [false, true] {
        let spec = token_2022(as_quote)
            .with_transfer_fee(0, u64::MAX)
            .with_transfer_fee_authority();
        let (pool, created) = env.create_pool(with_mint(spec, as_quote));
        created.expect("create_market");
        assert_fails_with(env.launch(&pool), "TransferFeeAuthorityPresent");

        let mint = mint_under_test(&pool, as_quote);
        revoke_fee_authority(&mut env, &mint);
        env.launch(&pool).expect("launch with the fee fixed");
        assert!(schedule_fee(&mut env, &mint, 10_000).is_err());
    }
}

/// Finding H1: the fee in force may take at most 5%, on either mint.
#[test]
fn finding_h1_a_transfer_fee_above_5_percent_is_refused() {
    let mut env = Env::new();
    for as_quote in [false, true] {
        let spec = token_2022(as_quote).with_transfer_fee(501, u64::MAX);
        let (pool, created) = env.create_pool(with_mint(spec, as_quote));
        created.expect("create_market");
        assert_fails_with(env.launch(&pool), "TransferFeeAboveCap");

        let spec = token_2022(as_quote).with_transfer_fee(500, u64::MAX);
        let (pool, created) = env.create_pool(with_mint(spec, as_quote));
        created.expect("create_market");
        env.launch(&pool).expect("a 5% fee launches");
    }
}

/// Finding H1: both the fee in force and one scheduled for a later epoch count.
/// Otherwise a creator could schedule 100%, give up the authority and launch while the fee
/// in force is still low. A fee that no longer applies is ignored.
#[test]
fn finding_h1_the_fee_in_force_and_a_scheduled_fee_both_count() {
    let mut env = Env::new();
    for as_quote in [false, true] {
        // 1% now, 100% from two epochs on.
        let spec = token_2022(as_quote)
            .with_transfer_fee(100, u64::MAX)
            .with_transfer_fee_authority();
        let (pool, created) = env.create_pool(with_mint(spec, as_quote));
        created.expect("create_market");
        let mint = mint_under_test(&pool, as_quote);
        schedule_fee(&mut env, &mint, 10_000).expect("schedule a 100% fee");
        revoke_fee_authority(&mut env, &mint);
        assert_fails_with(env.launch(&pool), "TransferFeeAboveCap");

        // 10% now, 1% from two epochs on: refused until the lower fee applies.
        let spec = token_2022(as_quote)
            .with_transfer_fee(1_000, u64::MAX)
            .with_transfer_fee_authority();
        let (pool, created) = env.create_pool(with_mint(spec, as_quote));
        created.expect("create_market");
        let mint = mint_under_test(&pool, as_quote);
        schedule_fee(&mut env, &mint, 100).expect("schedule a 1% fee");
        revoke_fee_authority(&mut env, &mint);
        assert_fails_with(env.launch(&pool), "TransferFeeAboveCap");
        advance_epochs(&mut env, 2);
        env.launch(&pool).expect("launch once the 1% fee applies");
    }
}

/// Finding H2: a transfer hook on the launch token could refuse transfers into the vault,
/// so that nobody can sell. Such a token is refused when its pool is created. A hook on the
/// quote mint stays allowed (`transfer_hook.rs`).
#[test]
fn finding_h2_a_hooked_launch_token_is_refused() {
    let mut env = Env::new();
    let hook = env.load_test_hook();
    let (_, created) = env.create_pool(PoolOptions {
        token: MintSpec::token_2022(6).with_transfer_hook(hook),
        ..PoolOptions::default()
    });
    assert_fails_with(created, "ForbiddenMintExtension");
}

/// Finding L1: a launch fee below the rent-exempt minimum, paid into an empty treasury,
/// would make every launch fail. The config refuses such a fee; 0 and the minimum itself
/// are fine, and the minimum reaches an empty treasury.
#[test]
fn finding_l1_a_launch_fee_below_rent_is_refused() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let rent = env.svm.minimum_balance_for_rent_exemption(0);
    for fee in [1, rent - 1] {
        let args = ConfigUpdate {
            launch_fee_lamports: fee,
            ..env.default_update()
        };
        assert_fails_with(
            env.send(&[update_config_ix(&admin, &args)], &[]),
            "LaunchFeeBelowRent",
        );
    }
    let free = ConfigUpdate {
        launch_fee_lamports: 0,
        ..env.default_update()
    };
    env.send(&[update_config_ix(&admin, &free)], &[])
        .expect("no launch fee");

    env.treasury = Pubkey::new_unique();
    let minimum = ConfigUpdate {
        launch_fee_lamports: rent,
        ..env.default_update()
    };
    env.send(&[update_config_ix(&admin, &minimum)], &[])
        .expect("the minimum fee");
    env.launched_pool(PoolOptions::default());
    assert_eq!(env.lamports(&env.treasury), rent);

    // `initialize_config` checks the same.
    let mut env = Env::without_protocol_config();
    let args = ConfigArgs {
        launch_fee_lamports: rent - 1,
        ..env.default_config()
    };
    let ix = initialize_config_ix(&env.admin.pubkey(), &args);
    assert_fails_with(env.send(&[ix], &[]), "LaunchFeeBelowRent");
}

/// Finding L2: the admin moves in two steps. Until the proposed admin signs
/// `accept_admin`, the old admin stays in charge, so a mistyped address, or one nobody can
/// sign for, takes nothing.
#[test]
fn finding_l2_the_admin_moves_only_when_the_successor_accepts() {
    let mut env = Env::new();
    let admin = env.admin.pubkey();
    let successor = env.funded_keypair();
    let stranger = env.funded_keypair();

    // Only the admin proposes.
    let ix = propose_admin_ix(&stranger.pubkey(), &stranger.pubkey());
    assert_fails_with(env.send(&[ix], &[&stranger]), "NotAdmin");

    // A proposal nobody can accept changes nothing, and a new one replaces it.
    let unreachable = Pubkey::new_unique();
    env.send(&[propose_admin_ix(&admin, &unreachable)], &[])
        .expect("propose");
    let config = env.config_state();
    assert_eq!((config.admin, config.pending_admin), (admin, unreachable));
    env.send(&[propose_admin_ix(&admin, &successor.pubkey())], &[])
        .expect("propose again");
    assert_eq!(env.config_state().pending_admin, successor.pubkey());

    // Only the proposed admin accepts, and until then the admin stays in charge.
    let ix = accept_admin_ix(&stranger.pubkey());
    assert_fails_with(env.send(&[ix], &[&stranger]), "NotPendingAdmin");
    assert_fails_with(env.send(&[accept_admin_ix(&admin)], &[]), "NotPendingAdmin");
    let update = env.default_update();
    env.send(&[update_config_ix(&admin, &update)], &[])
        .expect("still the admin");

    // A withdrawn proposal can't be accepted.
    env.send(&[propose_admin_ix(&admin, &Pubkey::default())], &[])
        .expect("withdraw");
    let accept = accept_admin_ix(&successor.pubkey());
    assert_fails_with(
        env.send(std::slice::from_ref(&accept), &[&successor]),
        "NotPendingAdmin",
    );

    // Proposed and accepted: the successor is in, the old admin out, and the proposal
    // is used up.
    env.send(&[propose_admin_ix(&admin, &successor.pubkey())], &[])
        .expect("propose");
    env.send(std::slice::from_ref(&accept), &[&successor])
        .expect("accept");
    let config = env.config_state();
    assert_eq!(
        (config.admin, config.pending_admin),
        (successor.pubkey(), Pubkey::default())
    );
    assert_fails_with(
        env.send(&[update_config_ix(&admin, &update)], &[]),
        "NotAdmin",
    );
    assert_fails_with(
        env.send(&[propose_admin_ix(&admin, &admin)], &[]),
        "NotAdmin",
    );
    env.send(
        &[update_config_ix(&successor.pubkey(), &update)],
        &[&successor],
    )
    .expect("the new admin updates the config");
    assert_fails_with(env.send(&[accept], &[&successor]), "NotPendingAdmin");
}
