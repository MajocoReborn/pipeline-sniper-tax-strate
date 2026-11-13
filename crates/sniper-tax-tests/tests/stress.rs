//! Randomised stress test: a long random sequence of buys and sells (exact-in and
//! exact-out) by several traders and the creator, time jumps through the tax window,
//! claims, sweeps and donations, all through the real router. After every step:
//!
//! - the on-chain quote equals the swap, and both equal the maths crate's prediction,
//!   including the exact tax and its split;
//! - the vaults hold exactly the pool's books (reserves, unclaimed tax, donations);
//! - the constant product never falls;
//! - no token or lamport of the quote mint is created or lost across all accounts.
//!
//! Seeds are fixed, so a failure reproduces. Run longer with, for example:
//! `STRESS_SEEDS=40 STRESS_STEPS=1500 cargo test -p sniper-tax-tests --release --test stress`

use sniper_tax_math::{applicable_tax_bps, Pricing, TaxSchedule};
use sniper_tax_tests::*;

const TRADERS: usize = 5;
const TRADER_QUOTE: u64 = 50 * SOL;

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// xorshift64*: small, deterministic and good enough to pick actions.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// Roughly log-uniform in `[1, max]`, so dust and large trades both show up.
    fn amount(&mut self, max: u64) -> u64 {
        if max <= 1 {
            return 1;
        }
        let bits = 64 - max.leading_zeros() as u64;
        let width = 1 + self.below(bits);
        let cap = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        1 + self.below(cap.min(max))
    }
}

#[derive(Default, Debug)]
struct Counts {
    trades: u64,
    taxed_buys: u64,
    refused: u64,
    skipped: u64,
    claims: u64,
    sweeps: u64,
    donations: u64,
    time_jumps: u64,
}

struct Stress {
    env: Env,
    pool: Pool,
    traders: Vec<Keypair>,
    treasury_quote: Pubkey,
    rng: Rng,
    /// Total of the quote mint across every account that can hold it here.
    quote_total: u128,
    /// Total of the launch token across every account that can hold it here.
    token_total: u128,
    donated_to_quote_vault: u64,
    donated_to_token_vault: u64,
    k: u128,
    counts: Counts,
}

impl Stress {
    fn new(seed: u64, options: PoolOptions) -> Self {
        let mut env = Env::new();
        let pool = env.launched_pool(options);
        let traders = (0..TRADERS)
            .map(|_| env.buyer(&pool, TRADER_QUOTE))
            .collect();
        let treasury = env.treasury;
        let treasury_quote = env.create_ata(&treasury, &pool.quote_mint, &pool.quote_program);
        let state = env.pool_state(&pool);
        let mut stress = Stress {
            env,
            pool,
            traders,
            treasury_quote,
            rng: Rng::new(seed),
            quote_total: 0,
            token_total: 0,
            donated_to_quote_vault: 0,
            donated_to_token_vault: 0,
            k: u128::from(state.reserve_token) * u128::from(state.reserve_quote),
            counts: Counts::default(),
        };
        stress.quote_total = stress.sum(&stress.quote_accounts());
        stress.token_total = stress.sum(&stress.token_accounts());
        stress
    }

    fn quote_accounts(&self) -> Vec<Pubkey> {
        let pool = &self.pool;
        let mut accounts: Vec<Pubkey> = self
            .traders
            .iter()
            .map(|t| self.env.quote_account(pool, &t.pubkey()))
            .collect();
        accounts.extend([
            pool.creator_quote(),
            pool.quote_vault,
            self.treasury_quote,
            router_pda::fee_account(&pool.market, &pool.quote_mint, &pool.quote_program),
        ]);
        accounts
    }

    fn token_accounts(&self) -> Vec<Pubkey> {
        let pool = &self.pool;
        let mut accounts: Vec<Pubkey> = self
            .traders
            .iter()
            .map(|t| self.env.token_account(pool, &t.pubkey()))
            .collect();
        accounts.extend([
            pool.creator_token(),
            pool.token_vault,
            router_pda::fee_account(&pool.market, &pool.token_mint, &pool.token_program),
        ]);
        accounts
    }

    fn sum(&self, accounts: &[Pubkey]) -> u128 {
        accounts
            .iter()
            .map(|a| u128::from(self.env.token_balance(a)))
            .sum()
    }

    fn now(&self) -> i64 {
        self.env
            .svm
            .get_sysvar::<solana_clock::Clock>()
            .unix_timestamp
    }

    fn run(&mut self, steps: u64) {
        for step in 0..steps {
            let roll = self.rng.below(100);
            match roll {
                0..=29 => self.trade(None, Side::Buy, TradeMode::ExactIn),
                30..=39 => self.trade(None, Side::Buy, TradeMode::ExactOut),
                40..=59 => self.trade(None, Side::Sell, TradeMode::ExactIn),
                60..=66 => self.trade(None, Side::Sell, TradeMode::ExactOut),
                67..=71 => {
                    let side = if self.rng.below(2) == 0 {
                        Side::Buy
                    } else {
                        Side::Sell
                    };
                    self.trade(Some(()), side, TradeMode::ExactIn)
                }
                72..=81 => self.jump_time(),
                82..=86 => self.claim(),
                87..=91 => self.sweep(),
                _ => self.donate(),
            }
            self.check(step);
        }
    }

    /// A trade by a random trader, or by the creator when `creator` is set.
    fn trade(&mut self, creator: Option<()>, side: Side, mode: TradeMode) {
        let trader = match creator {
            Some(()) => self.pool.creator.insecure_clone(),
            None => {
                let i = self.rng.below(TRADERS as u64) as usize;
                self.traders[i].insecure_clone()
            }
        };
        let is_creator = creator.is_some();
        let legs = self.pool.legs(side);
        let input =
            router_pda::associated_token(&trader.pubkey(), &legs.input_mint, &legs.input_program);
        let output =
            router_pda::associated_token(&trader.pubkey(), &legs.output_mint, &legs.output_program);
        let input_balance = self.env.token_balance(&input);
        let state = self.env.pool_state(&self.pool);
        let (reserve_in, reserve_out) = match side {
            Side::Buy => (state.reserve_quote, state.reserve_token),
            Side::Sell => (state.reserve_token, state.reserve_quote),
        };
        let amount = match mode {
            // Leave room for Pipeline's fee, which comes out of the same account.
            TradeMode::ExactIn => self.rng.amount(input_balance / 100 * 99),
            TradeMode::ExactOut => self.rng.amount(reserve_out / 20),
        };
        if input_balance == 0 {
            self.counts.skipped += 1;
            return;
        }

        let now = self.now();
        let schedule = TaxSchedule::new(
            state.tax_start_bps,
            state.tax_end_bps,
            state.tax_duration_secs,
        )
        .unwrap();
        let pricing = Pricing {
            reserve_in,
            reserve_out,
            tax_bps: applicable_tax_bps(&schedule, state.launch_ts, now, side, is_creator),
            protocol_share_bps: state.protocol_tax_share_bps,
        };
        let expected = match mode {
            TradeMode::ExactIn => pricing.quote_exact_in(amount),
            TradeMode::ExactOut => pricing.quote_exact_out(amount),
        };
        let quoted = self
            .env
            .quote(&self.pool, side, mode, amount, Some(trader.pubkey()));

        let expected = match (expected, quoted) {
            (Ok(expected), Ok(quoted)) => {
                assert_eq!(
                    (quoted.amount_in, quoted.amount_out),
                    (expected.amount_in, expected.amount_out),
                    "on-chain quote differs from the maths"
                );
                expected
            }
            (Err(_), Err(_)) => {
                // Neither prices it, so the swap must fail too.
                let args = match mode {
                    TradeMode::ExactIn => exact_in(amount, 0),
                    TradeMode::ExactOut => exact_out(input_balance / 2, amount),
                };
                assert!(self.env.swap(&self.pool, &trader, side, args).is_err());
                self.counts.refused += 1;
                return;
            }
            (expected, quoted) => {
                panic!("maths and chain disagree: {expected:?} vs {quoted:?}")
            }
        };

        let sent = match mode {
            TradeMode::ExactIn => amount,
            // Offer up to 5% more than needed, to exercise the refund.
            TradeMode::ExactOut => expected.amount_in + self.rng.below(expected.amount_in / 20 + 1),
        };
        if u128::from(sent) + u128::from(ceil_bps(sent, 25)) > u128::from(input_balance) {
            self.counts.skipped += 1;
            return;
        }
        let args = match mode {
            TradeMode::ExactIn => exact_in(sent, expected.amount_out),
            TradeMode::ExactOut => exact_out(sent, expected.amount_out),
        };
        let output_before = self.env.token_balance(&output);
        self.env
            .swap(&self.pool, &trader, side, args)
            .unwrap_or_else(|e| panic!("swap failed: {:?}\n{}", e.err, e.meta.logs.join("\n")));

        // The trader got exactly the quote and paid exactly the trade plus Pipeline's fee.
        assert_eq!(
            self.env.token_balance(&output) - output_before,
            expected.amount_out
        );
        assert_eq!(
            input_balance - self.env.token_balance(&input),
            expected.amount_in + ceil_bps(expected.amount_in, 25)
        );
        // The pool's books moved exactly as the maths says.
        let after = self.env.pool_state(&self.pool);
        let (new_in, new_out) = match side {
            Side::Buy => (after.reserve_quote, after.reserve_token),
            Side::Sell => (after.reserve_token, after.reserve_quote),
        };
        assert_eq!(
            (new_in, new_out),
            (expected.new_reserve_in, expected.new_reserve_out)
        );
        assert_eq!(
            after.accrued_tax_quote - state.accrued_tax_quote,
            expected.creator_tax
        );
        assert_eq!(
            after.accrued_protocol_quote - state.accrued_protocol_quote,
            expected.protocol_fee
        );
        self.counts.trades += 1;
        if expected.creator_tax + expected.protocol_fee > 0 {
            self.counts.taxed_buys += 1;
        }
    }

    fn jump_time(&mut self) {
        // Mostly small steps through the window, sometimes far past it.
        let jump = if self.rng.below(5) == 0 {
            self.rng.below(2_000)
        } else {
            self.rng.below(6)
        };
        let now = self.now();
        self.env.set_time(now + jump as i64);
        self.counts.time_jumps += 1;
    }

    fn claim(&mut self) {
        let owed = self.env.pool_state(&self.pool).accrued_tax_quote;
        let creator = self.pool.creator.insecure_clone();
        let before = self.env.token_balance(&self.pool.creator_quote());
        let ix = self.pool.claim_tax_ix(&self.pool.creator_quote());
        let result = self.env.send(&[ix], &[&creator]);
        if owed == 0 {
            assert_fails_with(result, "NothingToClaim");
            return;
        }
        result.expect("claim");
        assert_eq!(
            self.env.token_balance(&self.pool.creator_quote()) - before,
            owed
        );
        assert_eq!(self.env.pool_state(&self.pool).accrued_tax_quote, 0);
        self.counts.claims += 1;
    }

    fn sweep(&mut self) {
        let owed = self.env.pool_state(&self.pool).accrued_protocol_quote;
        let before = self.env.token_balance(&self.treasury_quote);
        let treasury = self.env.treasury;
        let ix = self.pool.sweep_ix(&treasury, &self.treasury_quote);
        let result = self.env.send(&[ix], &[]);
        if owed == 0 {
            assert_fails_with(result, "NothingToClaim");
            return;
        }
        result.expect("sweep");
        assert_eq!(self.env.token_balance(&self.treasury_quote) - before, owed);
        self.counts.sweeps += 1;
    }

    /// Someone sends tokens straight to a vault. They must never enter the price.
    fn donate(&mut self) {
        if self.rng.below(2) == 0 {
            let amount = self.rng.amount(SOL);
            let (mint, program, vault) = (
                self.pool.quote_mint,
                self.pool.quote_program,
                self.pool.quote_vault,
            );
            self.env.mint_to(&mint, &program, &vault, amount);
            self.donated_to_quote_vault += amount;
            self.quote_total += u128::from(amount);
        } else {
            let creator = self.pool.creator.insecure_clone();
            let amount = self
                .rng
                .amount(self.env.token_balance(&self.pool.creator_token()) / 1_000);
            let ix = spl_token_2022_interface::instruction::transfer_checked(
                &self.pool.token_program,
                &self.pool.creator_token(),
                &self.pool.token_mint,
                &self.pool.token_vault,
                &creator.pubkey(),
                &[],
                amount,
                6,
            )
            .unwrap();
            self.env.send(&[ix], &[&creator]).expect("donate tokens");
            self.donated_to_token_vault += amount;
        }
        self.counts.donations += 1;
    }

    fn check(&mut self, step: u64) {
        let state = self.env.pool_state(&self.pool);
        assert_eq!(state.status, PoolStatus::Live, "step {step}");
        assert!(
            state.reserve_token > 0 && state.reserve_quote > 0,
            "step {step}"
        );

        // The vaults hold exactly the books plus donations.
        assert_eq!(
            u128::from(self.env.token_balance(&self.pool.quote_vault)),
            u128::from(state.reserve_quote)
                + u128::from(state.accrued_tax_quote)
                + u128::from(state.accrued_protocol_quote)
                + u128::from(self.donated_to_quote_vault),
            "quote vault books at step {step}"
        );
        assert_eq!(
            u128::from(self.env.token_balance(&self.pool.token_vault)),
            u128::from(state.reserve_token) + u128::from(self.donated_to_token_vault),
            "token vault books at step {step}"
        );

        // The constant product never falls.
        let k = u128::from(state.reserve_token) * u128::from(state.reserve_quote);
        assert!(k >= self.k, "constant product fell at step {step}");
        self.k = k;

        // Nothing is created or lost.
        assert_eq!(
            self.sum(&self.quote_accounts()),
            self.quote_total,
            "quote conservation at step {step}"
        );
        assert_eq!(
            self.sum(&self.token_accounts()),
            self.token_total,
            "token conservation at step {step}"
        );
    }
}

fn stress(options: PoolOptions) {
    let seeds = env_number("STRESS_SEEDS", 3);
    let steps = env_number("STRESS_STEPS", 250);
    for seed in 1..=seeds {
        let mut stress = Stress::new(seed, options.clone());
        stress.run(steps);
        println!("seed {seed}: {:?}", stress.counts);
        assert!(
            stress.counts.trades > steps / 4,
            "too few trades went through"
        );
    }
}

#[test]
fn stress_a_ninety_second_window() {
    stress(PoolOptions::default());
}

#[test]
fn stress_a_ten_minute_window_with_a_permanent_fee() {
    stress(PoolOptions {
        tax_duration_secs: 600,
        tax_end_bps: 300,
        ..PoolOptions::default()
    });
}
