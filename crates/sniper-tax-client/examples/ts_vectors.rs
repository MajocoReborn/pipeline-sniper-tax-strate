//! Writes the test vectors for the TypeScript SDK (`sdk/test/vectors.json`): addresses,
//! instructions, encoded accounts and tax rates, all computed by this crate. The TypeScript
//! tests rebuild each one and must match byte for byte.
//!
//! ```sh
//! cargo run -q -p sniper-tax-client --example ts_vectors > sdk/test/vectors.json
//! ```
//!
//! Numbers above 2^53 don't survive JSON in JavaScript, so every u64 and i64 is a string.

use serde_json::{json, Value};
use sniper_tax_client::*;
use sniper_tax_math::{
    applicable_tax_bps, gross_for_effective, in_given_out, out_given_in, protocol_share_at,
    split_gross, split_tax, Pricing, TaxSchedule, LAUNCH_BLOCK_SLOTS, MAX_DURATION_SECS,
    MAX_END_BPS, MAX_LAUNCH_FEE_LAMPORTS, MAX_PROTOCOL_TAX_SHARE_BPS, MAX_START_BPS,
    MIN_DURATION_SECS,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn key(seed: u8) -> Pubkey {
    Pubkey::new_from_array([seed; 32])
}

/// An address that starts with zero bytes, so its base58 form starts with `1`s.
fn leading_zero_key() -> Pubkey {
    let mut bytes = [7u8; 32];
    bytes[..3].fill(0);
    Pubkey::new_from_array(bytes)
}

fn instruction(ix: &Instruction) -> Value {
    json!({
        "program_id": ix.program_id.to_string(),
        "accounts": ix.accounts.iter().map(|a| json!({
            "pubkey": a.pubkey.to_string(),
            "is_signer": a.is_signer,
            "is_writable": a.is_writable,
        })).collect::<Vec<_>>(),
        "data": hex(&ix.data),
    })
}

fn hooks() -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(key(0x31), false),
        AccountMeta::new(key(0x32), false),
    ]
}

fn pool_case(
    name: &str,
    params: InitParams,
    token: (Pubkey, Pubkey),
    quote: (Pubkey, Pubkey),
) -> Value {
    let keys = PoolKeys::new(params, token.0, token.1, quote.0, quote.1);
    let payer = key(9);
    let treasury = key(10);
    let trader = key(11);
    let treasury_quote =
        router_pda::associated_token(&treasury, &keys.quote_mint, &keys.quote_program);
    let launch = keys.launch_ix(&treasury, 800_000_000_000_000, 5_000_000_000);
    let mut launch_with_hooks = launch.clone();
    launch_with_hooks.accounts.extend(hooks());
    let fee_accounts = keys.fee_account_ixs(&payer);
    json!({
        "name": name,
        "params": {
            "creator": params.creator.to_string(),
            "tax_start_bps": params.tax_start_bps,
            "tax_end_bps": params.tax_end_bps,
            "tax_duration_secs": params.tax_duration_secs,
        },
        "init_data": hex(&borsh::to_vec(&params).unwrap()),
        "token_mint": keys.token_mint.to_string(),
        "token_program": keys.token_program.to_string(),
        "quote_mint": keys.quote_mint.to_string(),
        "quote_program": keys.quote_program.to_string(),
        "keys": {
            "address": keys.address.to_string(),
            "market": keys.market.to_string(),
            "token_vault": keys.token_vault.to_string(),
            "quote_vault": keys.quote_vault.to_string(),
            "creator_token": keys.creator_token().to_string(),
            "creator_quote": keys.creator_quote().to_string(),
        },
        "inputs": {
            "payer": payer.to_string(),
            "treasury": treasury.to_string(),
            "treasury_quote": treasury_quote.to_string(),
            "trader": trader.to_string(),
            "hooks": hooks().iter().map(|a| json!({
                "pubkey": a.pubkey.to_string(),
                "is_signer": a.is_signer,
                "is_writable": a.is_writable,
            })).collect::<Vec<_>>(),
        },
        "instructions": {
            "create_market": instruction(&keys.create_market_ix(&payer)),
            "fee_accounts": fee_accounts.iter().map(instruction).collect::<Vec<_>>(),
            "launch": instruction(&launch),
            "launch_with_hooks": instruction(&launch_with_hooks),
            "claim_tax": instruction(&keys.claim_tax_ix(&keys.creator_quote())),
            "sweep": instruction(&keys.sweep_ix(&treasury, &treasury_quote)),
            "quote_buy_exact_in": instruction(&keys.quote_ix(Side::Buy, TradeMode::ExactIn, 1_000_000_000, None)),
            "quote_sell_exact_out_creator": instruction(&keys.quote_ix(
                Side::Sell,
                TradeMode::ExactOut,
                12_345,
                Some(params.creator),
            )),
            "swap_buy_exact_in": instruction(&keys.swap_ix(&trader, Side::Buy, &exact_in(1_000_000_000, 123))),
            "swap_sell_exact_out_hooks": instruction(&keys.swap_ix_with_hooks(
                &trader,
                Side::Sell,
                &exact_out(u64::MAX, 777),
                hooks(),
                hooks(),
            )),
        },
    })
}

fn encoded_account<T: borsh::BorshSerialize>(name: &str, value: &T) -> String {
    let mut data = router::account_discriminator(name).to_vec();
    data.extend(borsh::to_vec(value).unwrap());
    hex(&data)
}

fn accounts() -> Value {
    let pool = LaunchPool {
        version: 1,
        status: PoolStatus::Live,
        bump: 254,
        creator: key(1),
        token_mint: key(2),
        quote_mint: router::wallet::NATIVE_MINT,
        token_vault: key(4),
        quote_vault: key(5),
        market_signer: leading_zero_key(),
        reserve_token: u64::MAX - 1,
        reserve_quote: 5_000_000_000,
        accrued_tax_quote: 891_000_000,
        accrued_protocol_quote: 99_000_000,
        tax_start_bps: 9_900,
        tax_end_bps: 300,
        tax_duration_secs: 600,
        protocol_tax_share_bps: 1_000,
        launch_fee_lamports: 10_000_000,
        launch_ts: 1_800_000_000,
        // Above 2^53, so the TypeScript side must read it as a bigint.
        launch_slot: (1 << 60) + 7,
        reserved: [0; 56],
    };
    let pending = LaunchPool {
        status: PoolStatus::Pending,
        reserve_token: 0,
        reserve_quote: 0,
        accrued_tax_quote: 0,
        accrued_protocol_quote: 0,
        launch_ts: 0,
        launch_slot: 0,
        ..pool.clone()
    };
    let config = ProtocolConfig {
        admin: key(12),
        treasury: key(13),
        protocol_tax_share_bps: 1_000,
        launch_fee_lamports: 10_000_000,
        bump: 255,
        pending_admin: Pubkey::default(),
        reserved: [0; 32],
    };
    let proposed = ProtocolConfig {
        pending_admin: key(14),
        ..config.clone()
    };
    let pool_json = |p: &LaunchPool| {
        json!({
            "data": encoded_account("LaunchPool", p),
            "decoded": {
                "version": p.version,
                "status": if p.status == PoolStatus::Live { "live" } else { "pending" },
                "bump": p.bump,
                "creator": p.creator.to_string(),
                "token_mint": p.token_mint.to_string(),
                "quote_mint": p.quote_mint.to_string(),
                "token_vault": p.token_vault.to_string(),
                "quote_vault": p.quote_vault.to_string(),
                "market_signer": p.market_signer.to_string(),
                "reserve_token": p.reserve_token.to_string(),
                "reserve_quote": p.reserve_quote.to_string(),
                "accrued_tax_quote": p.accrued_tax_quote.to_string(),
                "accrued_protocol_quote": p.accrued_protocol_quote.to_string(),
                "tax_start_bps": p.tax_start_bps,
                "tax_end_bps": p.tax_end_bps,
                "tax_duration_secs": p.tax_duration_secs,
                "protocol_tax_share_bps": p.protocol_tax_share_bps,
                "launch_fee_lamports": p.launch_fee_lamports.to_string(),
                "launch_ts": p.launch_ts.to_string(),
                "launch_slot": p.launch_slot.to_string(),
            },
        })
    };
    let config_json = |c: &ProtocolConfig| {
        json!({
            "data": encoded_account("ProtocolConfig", c),
            "decoded": {
                "admin": c.admin.to_string(),
                "treasury": c.treasury.to_string(),
                "protocol_tax_share_bps": c.protocol_tax_share_bps,
                "launch_fee_lamports": c.launch_fee_lamports.to_string(),
                "bump": c.bump,
                "pending_admin": (c.pending_admin != Pubkey::default()).then(|| c.pending_admin.to_string()),
            },
        })
    };
    json!({
        "launch_pools": [pool_json(&pool), pool_json(&pending)],
        "protocol_configs": [config_json(&config), config_json(&proposed)],
        // The config's bytes under the pool's discriminator: not a pool.
        "not_a_pool": encoded_account("ProtocolConfig", &config),
    })
}

fn tax() -> Value {
    let launch_ts: i64 = 1_800_000_000;
    let schedules = [
        (9_900, 0, 90),
        (9_900, 0, 600),
        (5_000, 0, 60),
        (9_900, 300, 600),
        (9_900, 500, 1_800),
        (0, 0, 60),
    ];
    let elapsed = [
        i64::MIN / 2,
        -5,
        0,
        1,
        30,
        45,
        59,
        60,
        89,
        90,
        91,
        300,
        540,
        599,
        600,
        1_799,
        1_800,
        1_000_000_000,
    ];
    let mut rates = vec![];
    for (start, end, duration) in schedules {
        let schedule = TaxSchedule::new(start, end, duration).unwrap();
        for e in elapsed {
            let now = launch_ts.saturating_add(e);
            for (side, creator) in [(Side::Buy, false), (Side::Buy, true), (Side::Sell, false)] {
                rates.push(json!({
                    "start_bps": start,
                    "end_bps": end,
                    "duration_secs": duration,
                    "launch_ts": launch_ts.to_string(),
                    "now": now.to_string(),
                    "side": if side == Side::Buy { "buy" } else { "sell" },
                    "trader_is_creator": creator,
                    "bps": applicable_tax_bps(&schedule, launch_ts, now, side, creator),
                }));
            }
        }
    }
    let candidates = [
        (9_900, 0, 90),
        (9_901, 0, 90),
        (9_900, 500, 90),
        (9_900, 501, 90),
        (100, 200, 90),
        (9_900, 0, 59),
        (9_900, 0, 60),
        (9_900, 0, 1_800),
        (9_900, 0, 1_801),
        (0, 0, 60),
    ];
    let validity = candidates
        .iter()
        .map(|&(start_bps, end_bps, duration_secs)| {
            json!({
                "start_bps": start_bps,
                "end_bps": end_bps,
                "duration_secs": duration_secs,
                "valid": TaxSchedule { start_bps, end_bps, duration_secs }.validate().is_ok(),
            })
        })
        .collect::<Vec<_>>();
    json!({ "rates": rates, "validity": validity })
}

/// The pool's trade maths: the tax split, constant-product pricing and full quotes, over
/// amounts and rates that include the edges (zero, one, u64::MAX, rates of 100% and above).
fn pricing() -> Value {
    let amounts = [
        0u64,
        1,
        7,
        9_999,
        10_000,
        1_000_000_000,
        123_456_789_012,
        u64::MAX / 2,
        u64::MAX,
    ];
    let rates = [0u16, 1, 300, 5_000, 9_570, 9_900, 9_999, 10_000, 12_000];
    let opt = |v: Option<u64>| v.map_or(Value::Null, |v| json!(v.to_string()));

    let mut splits = Vec::new();
    let mut grosses = Vec::new();
    let mut shares = Vec::new();
    for &amount in &amounts {
        for &bps in &rates {
            let split = split_gross(amount, bps);
            splits.push(json!({
                "gross": amount.to_string(), "bps": bps,
                "effective": split.effective.to_string(), "tax": split.tax.to_string(),
            }));
            grosses.push(json!({
                "effective": amount.to_string(), "bps": bps,
                "gross": opt(gross_for_effective(amount, bps)),
            }));
            let s = split_tax(amount, bps);
            shares.push(json!({
                "tax": amount.to_string(), "share_bps": bps,
                "creator": s.creator.to_string(), "protocol": s.protocol.to_string(),
            }));
        }
    }

    let reserves = [
        (100_000_000u64, 900_000_000_000_000u64),
        (5_000_000_000, 1_000_000_000_000),
        (1, 1),
        (u64::MAX - 1, 5_000_000_000),
        (0, 1_000_000),
    ];
    let mut swaps = Vec::new();
    for &(reserve_in, reserve_out) in &reserves {
        for &amount in &[
            0u64,
            1,
            10_000_000,
            1_000_000_000,
            1_000_000_000_000_000,
            u64::MAX,
        ] {
            swaps.push(json!({
                "reserve_in": reserve_in.to_string(), "reserve_out": reserve_out.to_string(),
                "amount": amount.to_string(),
                "out_given_in": out_given_in(reserve_in, reserve_out, amount).to_string(),
                "in_given_out": opt(in_given_out(reserve_in, reserve_out, amount)),
            }));
        }
    }

    let mut quotes = Vec::new();
    for &(reserve_in, reserve_out) in &reserves {
        for &amount in &[1u64, 10_000_000, 1_000_000_000, 1_000_000_000_000_000] {
            for &tax_bps in &[0u16, 9_680, 9_900] {
                for &protocol_share_bps in &[1_000u16, 3_000, 10_000] {
                    let pricing = Pricing {
                        reserve_in,
                        reserve_out,
                        tax_bps,
                        protocol_share_bps,
                    };
                    for (mode, result) in [
                        ("exact_in", pricing.quote_exact_in(amount)),
                        ("exact_out", pricing.quote_exact_out(amount)),
                    ] {
                        let result = match result {
                            Ok(q) => json!({
                                "amount_in": q.amount_in.to_string(),
                                "amount_out": q.amount_out.to_string(),
                                "effective_in": q.effective_in.to_string(),
                                "creator_tax": q.creator_tax.to_string(),
                                "protocol_fee": q.protocol_fee.to_string(),
                                "new_reserve_in": q.new_reserve_in.to_string(),
                                "new_reserve_out": q.new_reserve_out.to_string(),
                            }),
                            Err(e) => json!({ "error": format!("{e:?}") }),
                        };
                        quotes.push(json!({
                            "reserve_in": reserve_in.to_string(), "reserve_out": reserve_out.to_string(),
                            "tax_bps": tax_bps, "protocol_share_bps": protocol_share_bps,
                            "mode": mode, "amount": amount.to_string(), "result": result,
                        }));
                    }
                }
            }
        }
    }
    // The protocol's share by slot: the launch block (the launch slot and the next) takes
    // all of the tax; an unknown launch slot (0) never does.
    let mut launch_block = Vec::new();
    for &recorded in &[0u16, 1_000, 3_000] {
        for &(launch_slot, slot) in &[
            (0u64, 0u64),
            (0, 5),
            (100, 99),
            (100, 100),
            (100, 101),
            (100, 102),
            (100, u64::MAX),
            ((1 << 60) + 7, (1 << 60) + 8),
            (u64::MAX, u64::MAX),
        ] {
            launch_block.push(json!({
                "recorded_share_bps": recorded,
                "launch_slot": launch_slot.to_string(),
                "slot": slot.to_string(),
                "share_bps": protocol_share_at(recorded, launch_slot, slot),
            }));
        }
    }
    json!({
        "launch_block": launch_block,
        "split_gross": splits,
        "gross_for_effective": grosses,
        "split_tax": shares,
        "swaps": swaps,
        "quotes": quotes,
    })
}

fn main() {
    let creator_wsol = InitParams {
        creator: key(1),
        tax_start_bps: 9_900,
        tax_end_bps: 0,
        tax_duration_secs: 90,
    };
    let creator_other = InitParams {
        creator: leading_zero_key(),
        tax_start_bps: 5_000,
        tax_end_bps: 300,
        tax_duration_secs: 600,
    };
    let vectors = json!({
        "constants": {
            "program_id": PROGRAM_ID.to_string(),
            "config": config_address().to_string(),
            "native_mint": router::wallet::NATIVE_MINT.to_string(),
            "max_setup_lamports": MAX_SETUP_LAMPORTS.to_string(),
            "max_start_bps": MAX_START_BPS,
            "max_end_bps": MAX_END_BPS,
            "min_duration_secs": MIN_DURATION_SECS,
            "max_duration_secs": MAX_DURATION_SECS,
            "max_protocol_tax_share_bps": MAX_PROTOCOL_TAX_SHARE_BPS,
            "max_launch_fee_lamports": MAX_LAUNCH_FEE_LAMPORTS.to_string(),
            "launch_block_slots": LAUNCH_BLOCK_SLOTS.to_string(),
            "instruction_discriminators": {
                "launch": hex(&router::instruction_discriminator("launch")),
                "claim_tax": hex(&router::instruction_discriminator("claim_tax")),
                "sweep_protocol_fees": hex(&router::instruction_discriminator("sweep_protocol_fees")),
                "pipeline_quote": hex(&router::instruction_discriminator("pipeline_quote")),
            },
            "account_discriminators": {
                "LaunchPool": hex(&router::account_discriminator("LaunchPool")),
                "ProtocolConfig": hex(&router::account_discriminator("ProtocolConfig")),
            },
        },
        "pools": [
            pool_case(
                "token-2022 launch token against wrapped SOL",
                creator_wsol,
                (key(2), router::TOKEN_2022_PROGRAM_ID),
                (router::wallet::NATIVE_MINT, router::TOKEN_PROGRAM_ID),
            ),
            pool_case(
                "SPL launch token against a Token-2022 quote mint",
                creator_other,
                (key(3), router::TOKEN_PROGRAM_ID),
                (key(6), router::TOKEN_2022_PROGRAM_ID),
            ),
        ],
        "accounts": accounts(),
        "tax": tax(),
        "pricing": pricing(),
    });
    println!("{}", serde_json::to_string_pretty(&vectors).unwrap());
}
