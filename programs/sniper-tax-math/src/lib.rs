//! Pure maths for the Pipeline sniper-tax strategy.
//!
//! Everything the on-chain program prices with lives here, so `pipeline_quote` and
//! `pipeline_swap` share one implementation and every invariant can be property-tested on
//! the host. Amounts are vault-level, as in the Pipeline strategy interface: input is what
//! arrived in the strategy's input vault, output is what leaves its output vault.
//!
//! A buy (quote mint in, launch token out) pays a sniper tax on its input. The rate starts
//! at `start_bps` when the pool launches and falls linearly to `end_bps` over
//! `duration_secs`. The tax is split between the pool's creator and the protocol treasury,
//! except in the launch block (the launch's slot and the next), whose tax all goes to the
//! protocol. Neither part enters the reserves, so the tax never moves the price. Sells, and
//! trades by the creator, pay no tax.
//!
//! Rounding always favours the pool: the tax rounds up, output rounds down and the input
//! an exact-out trade needs rounds up. The protocol's share of the tax rounds down, in
//! the creator's favour.

#![cfg_attr(not(test), no_std)]

/// Basis-point denominator: 10,000 bps = 100%.
pub const BPS: u64 = 10_000;

/// Highest rate a schedule may start at: 99%.
pub const MAX_START_BPS: u16 = 9_900;
/// Highest rate a schedule may settle at after its window: 5%. Stops a pool from taxing
/// every buy at 99% forever.
pub const MAX_END_BPS: u16 = 500;
/// Shortest tax window: one minute.
pub const MIN_DURATION_SECS: u32 = 60;
/// Longest tax window: thirty minutes.
pub const MAX_DURATION_SECS: u32 = 1_800;

/// Highest share of each tax the protocol treasury may take: 30%. The launch block is the
/// exception (see [`LAUNCH_BLOCK_SLOTS`]).
pub const MAX_PROTOCOL_TAX_SHARE_BPS: u16 = 3_000;
/// Share of each tax the protocol treasury takes by default: 30%.
pub const DEFAULT_PROTOCOL_TAX_SHARE_BPS: u16 = 3_000;
/// The launch block: the slot a pool launched in and the next. The tax on buys landing in it
/// (bundled and same-block snipes) goes to the protocol in full.
pub const LAUNCH_BLOCK_SLOTS: u64 = 2;

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
/// Highest launch fee: 0.05 SOL.
pub const MAX_LAUNCH_FEE_LAMPORTS: u64 = 50_000_000;
/// Default launch fee: 0.01 SOL.
pub const DEFAULT_LAUNCH_FEE_LAMPORTS: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    /// A tax schedule outside the allowed bounds.
    InvalidSchedule,
    /// A protocol fee setting above its cap.
    FeeAboveCap,
    /// An empty side, or more output than the pool can give.
    InsufficientLiquidity,
    /// A result that doesn't fit in a u64.
    Overflow,
}

/// How a pool's sniper tax falls after launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxSchedule {
    /// Rate at the moment of launch.
    pub start_bps: u16,
    /// Rate from the end of the window onwards.
    pub end_bps: u16,
    /// Length of the window in seconds.
    pub duration_secs: u32,
}

impl TaxSchedule {
    pub fn new(start_bps: u16, end_bps: u16, duration_secs: u32) -> Result<Self, MathError> {
        let schedule = Self {
            start_bps,
            end_bps,
            duration_secs,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    /// Checks the bounds every pool's schedule must meet.
    pub fn validate(&self) -> Result<(), MathError> {
        let valid = self.start_bps <= MAX_START_BPS
            && self.end_bps <= MAX_END_BPS
            && self.end_bps <= self.start_bps
            && (MIN_DURATION_SECS..=MAX_DURATION_SECS).contains(&self.duration_secs);
        if valid {
            Ok(())
        } else {
            Err(MathError::InvalidSchedule)
        }
    }

    /// The rate at `now` for a pool launched at `launch_ts`, both Unix timestamps in
    /// seconds:
    ///
    /// `end + (start − end) × (duration − elapsed) / duration`, rounded down, while
    /// `elapsed < duration`; `end` afterwards. A clock behind the launch time counts as
    /// no time elapsed, so the rate never exceeds `start`.
    pub fn tax_bps_at(&self, launch_ts: i64, now: i64) -> u16 {
        let elapsed = now.saturating_sub(launch_ts);
        if elapsed <= 0 {
            return self.start_bps;
        }
        let duration = u64::from(self.duration_secs);
        let elapsed = elapsed as u64;
        if elapsed >= duration {
            return self.end_bps;
        }
        let span = u64::from(self.start_bps.saturating_sub(self.end_bps));
        // Less than `span`, so the sum stays at most `start_bps`.
        let decayed = span * (duration - elapsed) / duration;
        self.end_bps + decayed as u16
    }
}

/// Which way a trade goes, from the launch token's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Quote mint in, launch token out. Taxed.
    Buy,
    /// Launch token in, quote mint out. Never taxed.
    Sell,
}

/// The rate a trade pays: the schedule's rate for a buy by anyone but the creator, and
/// nothing for a sell or for the creator.
pub fn applicable_tax_bps(
    schedule: &TaxSchedule,
    launch_ts: i64,
    now: i64,
    side: Side,
    trader_is_creator: bool,
) -> u16 {
    if side == Side::Sell || trader_is_creator {
        0
    } else {
        schedule.tax_bps_at(launch_ts, now)
    }
}

/// A buy's gross input, split into the part that trades and the tax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxSplit {
    pub effective: u64,
    pub tax: u64,
}

/// Splits `gross` input at `tax_bps`: `tax = ceil(gross × bps / 10,000)`. Rounding the tax
/// up means splitting one buy into several never pays less tax in total. Rates above
/// 100% count as 100%.
pub fn split_gross(gross: u64, tax_bps: u16) -> TaxSplit {
    let bps = u128::from(tax_bps).min(u128::from(BPS));
    // At most `gross`, so it fits in a u64.
    let tax = (u128::from(gross) * bps).div_ceil(u128::from(BPS)) as u64;
    TaxSplit {
        effective: gross - tax,
        tax,
    }
}

/// The least gross input whose effective part, after [`split_gross`], is at least
/// `effective`: `ceil(effective × 10,000 / (10,000 − bps))`. `None` if that doesn't fit in
/// a u64, or if the rate is 100% or more.
pub fn gross_for_effective(effective: u64, tax_bps: u16) -> Option<u64> {
    if effective == 0 {
        return Some(0);
    }
    let kept = u128::from(BPS)
        .checked_sub(u128::from(tax_bps))
        .filter(|kept| *kept > 0)?;
    u64::try_from((u128::from(effective) * u128::from(BPS)).div_ceil(kept)).ok()
}

/// How a tax is shared between the creator and the protocol treasury.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxShares {
    pub creator: u64,
    pub protocol: u64,
}

/// Splits `tax` between the creator and the protocol: the protocol takes
/// `floor(tax × share / 10,000)` and the creator the rest. Shares above 100% count as 100%.
pub fn split_tax(tax: u64, protocol_share_bps: u16) -> TaxShares {
    let share = u128::from(protocol_share_bps).min(u128::from(BPS));
    // At most `tax`, so it fits in a u64.
    let protocol = (u128::from(tax) * share / u128::from(BPS)) as u64;
    TaxShares {
        creator: tax - protocol,
        protocol,
    }
}

/// Constant-product output for `amount_in`: `floor(r_out × in / (r_in + in))`. It is
/// below `reserve_out` whenever `reserve_in > 0`.
pub fn out_given_in(reserve_in: u64, reserve_out: u64, amount_in: u64) -> u64 {
    let denominator = u128::from(reserve_in) + u128::from(amount_in);
    if denominator == 0 {
        return 0;
    }
    // At most `reserve_out`, so it fits in a u64. Two u64s multiply within a u128.
    (u128::from(reserve_out) * u128::from(amount_in) / denominator) as u64
}

/// The least input for which [`out_given_in`] gives at least `amount_out`:
/// `ceil(r_in × out / (r_out − out))`. `None` if the pool can't give that much, or the
/// input doesn't fit in a u64.
pub fn in_given_out(reserve_in: u64, reserve_out: u64, amount_out: u64) -> Option<u64> {
    if amount_out == 0 {
        return Some(0);
    }
    if amount_out >= reserve_out {
        return None;
    }
    let needed = (u128::from(reserve_in) * u128::from(amount_out))
        .div_ceil(u128::from(reserve_out - amount_out));
    u64::try_from(needed).ok()
}

/// What a trade sees of a pool: its reserves in trade direction, the tax rate that
/// applies to this trade (from [`applicable_tax_bps`]) and the protocol's share of tax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pricing {
    pub reserve_in: u64,
    pub reserve_out: u64,
    pub tax_bps: u16,
    pub protocol_share_bps: u16,
}

/// The full result of a trade, shared by `pipeline_quote` and `pipeline_swap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    /// Input the strategy keeps: `effective_in + creator_tax + protocol_fee`.
    pub amount_in: u64,
    /// Output the strategy sends.
    pub amount_out: u64,
    /// The part of the input that enters the reserves and sets the price.
    pub effective_in: u64,
    /// Tax owed to the creator. Stays in the input vault until claimed.
    pub creator_tax: u64,
    /// Tax owed to the protocol treasury. Stays in the input vault until swept.
    pub protocol_fee: u64,
    /// The input reserve after the trade.
    pub new_reserve_in: u64,
    /// The output reserve after the trade. Always at least 1.
    pub new_reserve_out: u64,
}

impl Pricing {
    /// Prices `amount_in` of input. All of it is kept: the strategy refunds nothing.
    pub fn quote_exact_in(&self, amount_in: u64) -> Result<Quote, MathError> {
        self.check_liquidity()?;
        let split = split_gross(amount_in, self.tax_bps);
        let amount_out = out_given_in(self.reserve_in, self.reserve_out, split.effective);
        self.settle(amount_in, split, amount_out)
    }

    /// Prices exactly `amount_out` of output: the least input, tax included, that buys it.
    pub fn quote_exact_out(&self, amount_out: u64) -> Result<Quote, MathError> {
        self.check_liquidity()?;
        let needed = in_given_out(self.reserve_in, self.reserve_out, amount_out)
            .ok_or(MathError::InsufficientLiquidity)?;
        let amount_in = gross_for_effective(needed, self.tax_bps).ok_or(MathError::Overflow)?;
        // The effective part is at least `needed`; anything above it is the pool's gain.
        let split = split_gross(amount_in, self.tax_bps);
        self.settle(amount_in, split, amount_out)
    }

    /// A pool with an empty side has no meaningful price: either side could be drained.
    fn check_liquidity(&self) -> Result<(), MathError> {
        if self.reserve_in == 0 || self.reserve_out == 0 {
            return Err(MathError::InsufficientLiquidity);
        }
        Ok(())
    }

    fn settle(&self, amount_in: u64, split: TaxSplit, amount_out: u64) -> Result<Quote, MathError> {
        let shares = split_tax(split.tax, self.protocol_share_bps);
        let new_reserve_in = self
            .reserve_in
            .checked_add(split.effective)
            .ok_or(MathError::Overflow)?;
        let new_reserve_out = self
            .reserve_out
            .checked_sub(amount_out)
            .filter(|reserve| *reserve > 0)
            .ok_or(MathError::InsufficientLiquidity)?;
        Ok(Quote {
            amount_in,
            amount_out,
            effective_in: split.effective,
            creator_tax: shares.creator,
            protocol_fee: shares.protocol,
            new_reserve_in,
            new_reserve_out,
        })
    }
}

/// The part of a vault's balance the pool has on its books: its reserve plus tax not yet
/// claimed or swept. Input for a swap is credited only above this (with Pipeline's
/// `credited_input`), so unclaimed tax can never be traded as free input. Saturates,
/// which can only credit less.
pub fn accounted_balance(reserve: u64, creator_tax: u64, protocol_fee: u64) -> u64 {
    reserve
        .saturating_add(creator_tax)
        .saturating_add(protocol_fee)
}

/// The protocol's share of a tax paid at `slot`: all of it in the launch block, the pool's
/// recorded share otherwise. `launch_slot` 0 means unknown (pools launched before the launch
/// block existed), and then the recorded share always applies.
pub fn protocol_share_at(recorded_share_bps: u16, launch_slot: u64, slot: u64) -> u16 {
    let in_launch_block =
        launch_slot != 0 && slot >= launch_slot && slot - launch_slot < LAUNCH_BLOCK_SLOTS;
    if in_launch_block {
        BPS as u16
    } else {
        recorded_share_bps
    }
}

/// Checks protocol fee settings against their caps.
pub fn validate_protocol_fees(
    protocol_share_bps: u16,
    launch_fee_lamports: u64,
) -> Result<(), MathError> {
    if protocol_share_bps > MAX_PROTOCOL_TAX_SHARE_BPS
        || launch_fee_lamports > MAX_LAUNCH_FEE_LAMPORTS
    {
        return Err(MathError::FeeAboveCap);
    }
    Ok(())
}

/// The launch fee a creator pays: the lower of the fee recorded when their pool was
/// created and the current one, so a creator never pays more than they were shown and
/// still benefits from a later cut.
pub fn launch_fee_due(recorded_lamports: u64, current_lamports: u64) -> u64 {
    recorded_lamports.min(current_lamports)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: u64 = LAMPORTS_PER_SOL;

    #[test]
    fn schedule_matches_the_plan_table() {
        let short = TaxSchedule::new(9_900, 0, 90).unwrap();
        let long = TaxSchedule::new(9_900, 0, 600).unwrap();
        let launch = 1_700_000_000;
        for (elapsed, short_bps, long_bps) in [
            (0, 9_900, 9_900),
            (30, 6_600, 9_405),
            (45, 4_950, 9_157),
            (60, 3_300, 8_910),
            (89, 110, 8_431),
            (90, 0, 8_415),
            (300, 0, 4_950),
            (540, 0, 990),
            (600, 0, 0),
            (10_000, 0, 0),
        ] {
            assert_eq!(
                short.tax_bps_at(launch, launch + elapsed),
                short_bps,
                "90 s at {elapsed}"
            );
            assert_eq!(
                long.tax_bps_at(launch, launch + elapsed),
                long_bps,
                "600 s at {elapsed}"
            );
        }
    }

    #[test]
    fn schedule_settles_at_its_end_rate() {
        let schedule = TaxSchedule::new(9_900, 100, 60).unwrap();
        assert_eq!(schedule.tax_bps_at(0, 60), 100);
        assert_eq!(schedule.tax_bps_at(0, 59), 100 + 9_800 / 60);
    }

    #[test]
    fn clock_extremes_do_not_panic() {
        let schedule = TaxSchedule::new(9_900, 0, 90).unwrap();
        assert_eq!(schedule.tax_bps_at(i64::MAX, i64::MIN), 9_900);
        assert_eq!(schedule.tax_bps_at(i64::MIN, i64::MAX), 0);
        assert_eq!(schedule.tax_bps_at(100, 50), 9_900);
    }

    #[test]
    fn schedule_bounds_are_enforced() {
        assert!(TaxSchedule::new(MAX_START_BPS, MAX_END_BPS, MIN_DURATION_SECS).is_ok());
        assert!(TaxSchedule::new(MAX_START_BPS, 0, MAX_DURATION_SECS).is_ok());
        assert_eq!(
            TaxSchedule::new(MAX_START_BPS + 1, 0, 90),
            Err(MathError::InvalidSchedule)
        );
        assert_eq!(
            TaxSchedule::new(9_900, MAX_END_BPS + 1, 90),
            Err(MathError::InvalidSchedule)
        );
        assert_eq!(
            TaxSchedule::new(100, 200, 90),
            Err(MathError::InvalidSchedule)
        );
        assert_eq!(
            TaxSchedule::new(9_900, 0, MIN_DURATION_SECS - 1),
            Err(MathError::InvalidSchedule)
        );
        assert_eq!(
            TaxSchedule::new(9_900, 0, MAX_DURATION_SECS + 1),
            Err(MathError::InvalidSchedule)
        );
    }

    #[test]
    fn creator_and_sells_pay_nothing() {
        let schedule = TaxSchedule::new(9_900, 0, 90).unwrap();
        assert_eq!(applicable_tax_bps(&schedule, 0, 0, Side::Buy, false), 9_900);
        assert_eq!(applicable_tax_bps(&schedule, 0, 0, Side::Buy, true), 0);
        assert_eq!(applicable_tax_bps(&schedule, 0, 0, Side::Sell, false), 0);
        assert_eq!(applicable_tax_bps(&schedule, 0, 0, Side::Sell, true), 0);
    }

    #[test]
    fn plan_example_ten_sol_sniper_buy() {
        // A sniper buys for 10 SOL at launch: 9.9 SOL tax, of which 30% (2.97 SOL) goes
        // to the protocol and 6.93 SOL to the creator; 0.1 SOL trades.
        let pricing = Pricing {
            reserve_in: 5 * SOL,
            reserve_out: 1_000_000_000_000,
            tax_bps: 9_900,
            protocol_share_bps: DEFAULT_PROTOCOL_TAX_SHARE_BPS,
        };
        let quote = pricing.quote_exact_in(10 * SOL).unwrap();
        assert_eq!(quote.amount_in, 10 * SOL);
        assert_eq!(quote.effective_in, SOL / 10);
        assert_eq!(quote.protocol_fee, 2_970_000_000);
        assert_eq!(quote.creator_tax, 6_930_000_000);
        assert_eq!(quote.new_reserve_in, 5 * SOL + SOL / 10);
        assert_eq!(
            quote.amount_out,
            out_given_in(5 * SOL, 1_000_000_000_000, SOL / 10)
        );
    }

    #[test]
    fn untaxed_trade_is_plain_constant_product() {
        let pricing = Pricing {
            reserve_in: 1_000,
            reserve_out: 1_000,
            tax_bps: 0,
            protocol_share_bps: DEFAULT_PROTOCOL_TAX_SHARE_BPS,
        };
        let quote = pricing.quote_exact_in(1_000).unwrap();
        assert_eq!(quote.amount_out, 500);
        assert_eq!(quote.creator_tax + quote.protocol_fee, 0);
        let quote = pricing.quote_exact_out(500).unwrap();
        assert_eq!(quote.amount_in, 1_000);
    }

    #[test]
    fn empty_or_drained_pools_refuse_to_price() {
        let mut pricing = Pricing {
            reserve_in: 0,
            reserve_out: 1_000,
            tax_bps: 0,
            protocol_share_bps: 0,
        };
        assert_eq!(
            pricing.quote_exact_in(1),
            Err(MathError::InsufficientLiquidity)
        );
        pricing.reserve_in = 1_000;
        pricing.reserve_out = 0;
        assert_eq!(
            pricing.quote_exact_out(1),
            Err(MathError::InsufficientLiquidity)
        );
        pricing.reserve_out = 1_000;
        assert_eq!(
            pricing.quote_exact_out(1_000),
            Err(MathError::InsufficientLiquidity)
        );
    }

    #[test]
    fn small_taxed_buys_can_trade_nothing() {
        // At 99% a 1-unit buy is all tax. The trader's minimum output protects them; the
        // router still requires the strategy to keep some input, which it does.
        let pricing = Pricing {
            reserve_in: 1_000,
            reserve_out: 1_000,
            tax_bps: 9_900,
            protocol_share_bps: 0,
        };
        let quote = pricing.quote_exact_in(1).unwrap();
        assert_eq!(
            (quote.effective_in, quote.creator_tax, quote.amount_out),
            (0, 1, 0)
        );
    }

    #[test]
    fn the_launch_block_tax_goes_to_the_protocol() {
        let share = DEFAULT_PROTOCOL_TAX_SHARE_BPS;
        // The launch slot and the next.
        assert_eq!(protocol_share_at(share, 1_000, 1_000), 10_000);
        assert_eq!(protocol_share_at(share, 1_000, 1_001), 10_000);
        // From the second slot after, the recorded share.
        assert_eq!(protocol_share_at(share, 1_000, 1_002), share);
        assert_eq!(protocol_share_at(share, 1_000, u64::MAX), share);
        // A slot before the launch can't happen, but never counts as the launch block.
        assert_eq!(protocol_share_at(share, 1_000, 999), share);
        // Unknown launch slot (pools launched before this rule): always the recorded share.
        assert_eq!(protocol_share_at(share, 0, 0), share);
        assert_eq!(protocol_share_at(share, 0, 1), share);
        // At the end of the slot range there is no overflow.
        assert_eq!(protocol_share_at(share, u64::MAX, u64::MAX), 10_000);
        // A launch-block buy: all of the tax to the protocol, none to the creator.
        let pricing = Pricing {
            reserve_in: 5 * SOL,
            reserve_out: 1_000_000_000_000,
            tax_bps: 9_900,
            protocol_share_bps: protocol_share_at(share, 7, 8),
        };
        let quote = pricing.quote_exact_in(10 * SOL).unwrap();
        assert_eq!(quote.protocol_fee, 9_900_000_000);
        assert_eq!(quote.creator_tax, 0);
    }

    #[test]
    fn fee_caps_and_launch_fee() {
        assert!(validate_protocol_fees(
            DEFAULT_PROTOCOL_TAX_SHARE_BPS,
            DEFAULT_LAUNCH_FEE_LAMPORTS
        )
        .is_ok());
        assert!(
            validate_protocol_fees(MAX_PROTOCOL_TAX_SHARE_BPS, MAX_LAUNCH_FEE_LAMPORTS).is_ok()
        );
        assert_eq!(
            validate_protocol_fees(MAX_PROTOCOL_TAX_SHARE_BPS + 1, 0),
            Err(MathError::FeeAboveCap)
        );
        assert_eq!(
            validate_protocol_fees(0, MAX_LAUNCH_FEE_LAMPORTS + 1),
            Err(MathError::FeeAboveCap)
        );
        assert_eq!(launch_fee_due(DEFAULT_LAUNCH_FEE_LAMPORTS, 0), 0);
        assert_eq!(
            launch_fee_due(DEFAULT_LAUNCH_FEE_LAMPORTS, MAX_LAUNCH_FEE_LAMPORTS),
            DEFAULT_LAUNCH_FEE_LAMPORTS
        );
    }

    #[test]
    fn accounted_balance_saturates() {
        assert_eq!(accounted_balance(1, 2, 3), 6);
        assert_eq!(accounted_balance(u64::MAX, 1, 1), u64::MAX);
    }

    #[test]
    fn a_full_tax_has_no_gross_input() {
        // At 100% or more nothing trades, so no input is enough (and nothing divides by 0).
        assert_eq!(gross_for_effective(1, 10_000), None);
        assert_eq!(gross_for_effective(1, u16::MAX), None);
        assert_eq!(gross_for_effective(0, 10_000), Some(0));
        assert_eq!(gross_for_effective(1, 9_999), Some(10_000));
    }

    #[test]
    fn an_empty_side_refuses_even_zero_and_tiny_trades() {
        // Without the explicit check, a zero-input or zero-output trade on an empty side
        // would slip through the other arithmetic.
        for (reserve_in, reserve_out) in [(0, 1_000), (1_000, 0), (0, 0)] {
            let pricing = Pricing {
                reserve_in,
                reserve_out,
                tax_bps: 0,
                protocol_share_bps: 0,
            };
            assert_eq!(
                pricing.quote_exact_in(0),
                Err(MathError::InsufficientLiquidity),
                "exact-in on ({reserve_in}, {reserve_out})"
            );
            assert_eq!(
                pricing.quote_exact_out(0),
                Err(MathError::InsufficientLiquidity),
                "exact-out on ({reserve_in}, {reserve_out})"
            );
        }
    }
}
