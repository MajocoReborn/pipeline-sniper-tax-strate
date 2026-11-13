// The pool's trade maths, as in `crates/sniper-tax-math`: the tax split, constant-product
// pricing and full quotes, with the same integer rounding. `test/vectors.json` checks every
// function against the Rust crate.

import type { Side } from './pda.js';
import type { LaunchPool } from './codecs.js';
import { applicableTaxBps, poolSchedule } from './tax.js';

const BPS = 10_000n;
const U64_MAX = (1n << 64n) - 1n;

const ceilDiv = (a: bigint, b: bigint) => (a + b - 1n) / b;
const minBps = (bps: number) => (BigInt(bps) < BPS ? BigInt(bps) : BPS);

export type TaxSplit = { effective: bigint; tax: bigint };

/** `tax = ceil(gross × bps / 10,000)`; rates above 100% count as 100%. */
export function splitGross(gross: bigint, taxBps: number): TaxSplit {
    const tax = ceilDiv(gross * minBps(taxBps), BPS);
    return { effective: gross - tax, tax };
}

/** The least gross whose effective part is at least `effective`; `null` at 100% or on overflow. */
export function grossForEffective(effective: bigint, taxBps: number): bigint | null {
    if (effective === 0n) {
        return 0n;
    }
    const kept = BPS - BigInt(taxBps);
    if (kept <= 0n) {
        return null;
    }
    const gross = ceilDiv(effective * BPS, kept);
    return gross > U64_MAX ? null : gross;
}

/** The protocol takes `floor(tax × share / 10,000)`, the creator the rest. */
export function splitTax(tax: bigint, protocolShareBps: number): { creator: bigint; protocol: bigint } {
    const protocol = (tax * minBps(protocolShareBps)) / BPS;
    return { creator: tax - protocol, protocol };
}

/** `floor(r_out × in / (r_in + in))`. */
export function outGivenIn(reserveIn: bigint, reserveOut: bigint, amountIn: bigint): bigint {
    const denominator = reserveIn + amountIn;
    return denominator === 0n ? 0n : (reserveOut * amountIn) / denominator;
}

/** `ceil(r_in × out / (r_out − out))`; `null` if the pool can't give that much. */
export function inGivenOut(reserveIn: bigint, reserveOut: bigint, amountOut: bigint): bigint | null {
    if (amountOut === 0n) {
        return 0n;
    }
    if (amountOut >= reserveOut) {
        return null;
    }
    const needed = ceilDiv(reserveIn * amountOut, reserveOut - amountOut);
    return needed > U64_MAX ? null : needed;
}

export type Pricing = { reserveIn: bigint; reserveOut: bigint; taxBps: number; protocolShareBps: number };

export type Quote = {
    /** Input the strategy keeps: effective input plus both taxes. */
    amountIn: bigint;
    amountOut: bigint;
    /** The part of the input that enters the reserves. */
    effectiveIn: bigint;
    creatorTax: bigint;
    protocolFee: bigint;
    newReserveIn: bigint;
    newReserveOut: bigint;
};

export type QuoteResult = { ok: true; quote: Quote } | { ok: false; error: 'InsufficientLiquidity' | 'Overflow' };

function settle(pricing: Pricing, amountIn: bigint, split: TaxSplit, amountOut: bigint): QuoteResult {
    const shares = splitTax(split.tax, pricing.protocolShareBps);
    const newReserveIn = pricing.reserveIn + split.effective;
    if (newReserveIn > U64_MAX) {
        return { ok: false, error: 'Overflow' };
    }
    const newReserveOut = pricing.reserveOut - amountOut;
    if (newReserveOut <= 0n) {
        return { ok: false, error: 'InsufficientLiquidity' };
    }
    return {
        ok: true,
        quote: {
            amountIn,
            amountOut,
            effectiveIn: split.effective,
            creatorTax: shares.creator,
            protocolFee: shares.protocol,
            newReserveIn,
            newReserveOut,
        },
    };
}

/** Prices `amountIn` of input; all of it is kept. */
export function quoteExactIn(pricing: Pricing, amountIn: bigint): QuoteResult {
    if (pricing.reserveIn === 0n || pricing.reserveOut === 0n) {
        return { ok: false, error: 'InsufficientLiquidity' };
    }
    const split = splitGross(amountIn, pricing.taxBps);
    return settle(pricing, amountIn, split, outGivenIn(pricing.reserveIn, pricing.reserveOut, split.effective));
}

/** Prices exactly `amountOut`: the least input, tax included, that buys it. */
export function quoteExactOut(pricing: Pricing, amountOut: bigint): QuoteResult {
    if (pricing.reserveIn === 0n || pricing.reserveOut === 0n) {
        return { ok: false, error: 'InsufficientLiquidity' };
    }
    const needed = inGivenOut(pricing.reserveIn, pricing.reserveOut, amountOut);
    if (needed === null) {
        return { ok: false, error: 'InsufficientLiquidity' };
    }
    const amountIn = grossForEffective(needed, pricing.taxBps);
    if (amountIn === null) {
        return { ok: false, error: 'Overflow' };
    }
    return settle(pricing, amountIn, splitGross(amountIn, pricing.taxBps), amountOut);
}

/** What a trade on `pool` sees at `now`: reserves in trade direction and the rate it pays. */
export function poolPricing(pool: LaunchPool, side: Side, trader: string | null, now: bigint): Pricing {
    const buy = side === 'buy';
    return {
        reserveIn: buy ? pool.reserveQuote : pool.reserveToken,
        reserveOut: buy ? pool.reserveToken : pool.reserveQuote,
        taxBps: applicableTaxBps(poolSchedule(pool), pool.launchTs, now, side, trader === pool.creator),
        protocolShareBps: pool.protocolTaxShareBps,
    };
}
