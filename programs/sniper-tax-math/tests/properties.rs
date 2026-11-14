//! Property tests for the sniper-tax maths: the invariants from docs/PLAN.md section 9.

use proptest::prelude::*;
use sniper_tax_math::*;

/// Any schedule the program accepts.
fn schedule() -> impl Strategy<Value = TaxSchedule> {
    (0..=MAX_START_BPS, MIN_DURATION_SECS..=MAX_DURATION_SECS)
        .prop_flat_map(|(start, duration)| {
            (Just(start), 0..=start.min(MAX_END_BPS), Just(duration))
        })
        .prop_map(|(start, end, duration)| TaxSchedule::new(start, end, duration).unwrap())
}

/// Any tax rate a valid schedule can produce.
fn tax_rate() -> impl Strategy<Value = u16> {
    0..=MAX_START_BPS
}

/// Any protocol share a trade can see: up to the cap, or all of it in the launch block.
fn protocol_share() -> impl Strategy<Value = u16> {
    prop_oneof![0..=MAX_PROTOCOL_TAX_SHARE_BPS, Just(BPS as u16)]
}

/// Pricing with non-empty reserves below 2^63, so a reserve product fits in a u128 even
/// after a trade.
fn pricing() -> impl Strategy<Value = Pricing> {
    let reserve = 1..(1u64 << 63);
    (reserve.clone(), reserve, tax_rate(), protocol_share()).prop_map(
        |(reserve_in, reserve_out, tax_bps, protocol_share_bps)| Pricing {
            reserve_in,
            reserve_out,
            tax_bps,
            protocol_share_bps,
        },
    )
}

fn product(a: u64, b: u64) -> u128 {
    u128::from(a) * u128::from(b)
}

/// Checks everything that must hold for any successful quote.
fn check_quote(pricing: &Pricing, quote: &Quote) -> Result<(), TestCaseError> {
    // Every unit of input is accounted for.
    prop_assert_eq!(
        u128::from(quote.effective_in)
            + u128::from(quote.creator_tax)
            + u128::from(quote.protocol_fee),
        u128::from(quote.amount_in)
    );
    // The tax is exactly what the rate says, rounded up.
    let split = split_gross(quote.amount_in, pricing.tax_bps);
    prop_assert_eq!(split.effective, quote.effective_in);
    prop_assert_eq!(split.tax, quote.creator_tax + quote.protocol_fee);
    // Reserves move by the trade and never empty.
    prop_assert_eq!(
        quote.new_reserve_in,
        pricing.reserve_in + quote.effective_in
    );
    prop_assert_eq!(
        quote.new_reserve_out,
        pricing.reserve_out - quote.amount_out
    );
    prop_assert!(quote.new_reserve_out > 0);
    // The constant product never falls.
    prop_assert!(
        product(quote.new_reserve_in, quote.new_reserve_out)
            >= product(pricing.reserve_in, pricing.reserve_out)
    );
    // The curve gives at least the output for the input that entered it.
    prop_assert!(
        out_given_in(pricing.reserve_in, pricing.reserve_out, quote.effective_in)
            >= quote.amount_out
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn tax_rate_never_rises(
        schedule in schedule(),
        launch in any::<i32>(),
        a in -5_000i64..5_000,
        b in -5_000i64..5_000,
    ) {
        let launch = i64::from(launch);
        let (early, late) = (launch + a.min(b), launch + a.max(b));
        prop_assert!(schedule.tax_bps_at(launch, early) >= schedule.tax_bps_at(launch, late));
    }

    #[test]
    fn tax_rate_stays_within_its_bounds(
        schedule in schedule(),
        launch in any::<i64>(),
        now in any::<i64>(),
        k in 0i64..1_000_000,
    ) {
        let rate = schedule.tax_bps_at(launch, now);
        prop_assert!(schedule.end_bps <= rate && rate <= schedule.start_bps);
        prop_assert!(rate <= MAX_START_BPS);

        let launch = launch.clamp(-(1 << 40), 1 << 40);
        let duration = i64::from(schedule.duration_secs);
        prop_assert_eq!(schedule.tax_bps_at(launch, launch), schedule.start_bps);
        prop_assert_eq!(schedule.tax_bps_at(launch, launch - k), schedule.start_bps);
        prop_assert_eq!(schedule.tax_bps_at(launch, launch + duration + k), schedule.end_bps);
    }

    #[test]
    fn split_is_exact_and_rounds_tax_up(gross in any::<u64>(), bps in 0..=(BPS as u16)) {
        let split = split_gross(gross, bps);
        prop_assert_eq!(u128::from(split.effective) + u128::from(split.tax), u128::from(gross));
        let exact = u128::from(gross) * u128::from(bps);
        let tax = u128::from(split.tax) * u128::from(BPS);
        prop_assert!(tax >= exact);
        prop_assert!(tax < exact + u128::from(BPS));
    }

    #[test]
    fn splitting_a_buy_never_saves_tax(a in 0..u64::MAX / 2, b in 0..u64::MAX / 2, bps in tax_rate()) {
        let separate = split_gross(a, bps).tax + split_gross(b, bps).tax;
        prop_assert!(separate >= split_gross(a + b, bps).tax);
    }

    #[test]
    fn gross_for_effective_is_the_least_that_works(effective in any::<u64>(), bps in tax_rate()) {
        match gross_for_effective(effective, bps) {
            Some(gross) => {
                prop_assert!(split_gross(gross, bps).effective >= effective);
                if gross > 0 {
                    prop_assert!(split_gross(gross - 1, bps).effective < effective);
                }
            }
            None => {
                // Only when the least gross input doesn't fit in a u64.
                let kept = u128::from(BPS - u64::from(bps));
                prop_assert!(u128::from(effective) * u128::from(BPS) > u128::from(u64::MAX) * kept);
            }
        }
    }

    #[test]
    fn tax_shares_add_up(tax in any::<u64>(), share in 0..=MAX_PROTOCOL_TAX_SHARE_BPS) {
        let shares = split_tax(tax, share);
        prop_assert_eq!(u128::from(shares.creator) + u128::from(shares.protocol), u128::from(tax));
        prop_assert!(u128::from(shares.protocol) * u128::from(BPS) <= u128::from(tax) * u128::from(share));
        // Outside the launch block the protocol never takes more than 30%, so the creator
        // always keeps the most.
        prop_assert!(shares.creator >= shares.protocol);
    }

    #[test]
    fn the_launch_block_tax_goes_to_the_protocol_in_full(
        tax in any::<u64>(),
        recorded in 0..=MAX_PROTOCOL_TAX_SHARE_BPS,
        launch_slot in 1..u64::MAX,
        offset in 0..LAUNCH_BLOCK_SLOTS,
    ) {
        let slot = launch_slot.saturating_add(offset);
        let shares = split_tax(tax, protocol_share_at(recorded, launch_slot, slot));
        prop_assert_eq!(shares.protocol, tax);
        prop_assert_eq!(shares.creator, 0);
    }

    #[test]
    fn after_the_launch_block_the_recorded_share_applies(
        recorded in 0..=MAX_PROTOCOL_TAX_SHARE_BPS,
        launch_slot in 0..(u64::MAX / 2),
        after in LAUNCH_BLOCK_SLOTS..(u64::MAX / 2),
    ) {
        prop_assert_eq!(protocol_share_at(recorded, launch_slot, launch_slot + after), recorded);
        // An unknown launch slot never triggers the launch block.
        prop_assert_eq!(protocol_share_at(recorded, 0, after), recorded);
    }

    #[test]
    fn in_given_out_is_the_least_that_works(
        reserve_in in 1..u64::MAX,
        reserve_out in 2..u64::MAX,
        out_fraction in 0.0f64..1.0,
    ) {
        let amount_out = ((reserve_out - 1) as f64 * out_fraction) as u64;
        if let Some(input) = in_given_out(reserve_in, reserve_out, amount_out) {
            prop_assert!(out_given_in(reserve_in, reserve_out, input) >= amount_out);
            if input > 0 {
                prop_assert!(out_given_in(reserve_in, reserve_out, input - 1) < amount_out);
            }
        }
    }

    #[test]
    fn exact_in_quotes_hold_every_invariant(pricing in pricing(), amount_in in 0..(1u64 << 63)) {
        let quote = pricing.quote_exact_in(amount_in).unwrap();
        prop_assert_eq!(quote.amount_in, amount_in);
        check_quote(&pricing, &quote)?;
    }

    #[test]
    fn exact_out_quotes_hold_every_invariant(pricing in pricing(), out_fraction in 0.0f64..1.0) {
        let amount_out = ((pricing.reserve_out - 1) as f64 * out_fraction) as u64;
        if let Ok(quote) = pricing.quote_exact_out(amount_out) {
            prop_assert_eq!(quote.amount_out, amount_out);
            check_quote(&pricing, &quote)?;
            // It is the least input that buys `amount_out`.
            if quote.amount_in > 0 {
                let less = pricing.quote_exact_in(quote.amount_in - 1).unwrap();
                prop_assert!(less.amount_out < amount_out);
            }
        }
    }

    #[test]
    fn exact_out_never_costs_more_than_the_exact_in_that_buys_it(
        pricing in pricing(),
        amount_in in 1..(1u64 << 63),
    ) {
        let bought = pricing.quote_exact_in(amount_in).unwrap();
        if bought.amount_out > 0 {
            let exact_out = pricing.quote_exact_out(bought.amount_out).unwrap();
            prop_assert!(exact_out.amount_in <= amount_in);
        }
    }

    #[test]
    fn a_lower_rate_never_gives_less(pricing in pricing(), amount_in in 0..(1u64 << 63), lower in tax_rate()) {
        // Tax only falls over time, so a swap that lands after its quote is never worse
        // for the buyer, and a stale clock only makes a quote conservative.
        let lower = Pricing { tax_bps: lower.min(pricing.tax_bps), ..pricing };
        let now = pricing.quote_exact_in(amount_in).unwrap();
        let later = lower.quote_exact_in(amount_in).unwrap();
        prop_assert!(later.amount_out >= now.amount_out);
    }

    #[test]
    fn quotes_never_panic(
        reserve_in in any::<u64>(),
        reserve_out in any::<u64>(),
        tax_bps in any::<u16>(),
        protocol_share_bps in any::<u16>(),
        amount in any::<u64>(),
    ) {
        let pricing = Pricing { reserve_in, reserve_out, tax_bps, protocol_share_bps };
        let _ = pricing.quote_exact_in(amount);
        let _ = pricing.quote_exact_out(amount);
    }

}
