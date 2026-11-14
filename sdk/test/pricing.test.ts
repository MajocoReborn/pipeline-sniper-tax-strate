// The pool's trade maths against vectors from the Rust crate `sniper-tax-math`.

import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import {
    grossForEffective,
    inGivenOut,
    outGivenIn,
    quoteExactIn,
    quoteExactOut,
    splitGross,
    poolPricing,
    protocolShareAt,
    splitTax,
    type LaunchPool,
} from '../src/index.js';

type Opt = string | null;
const vectors = JSON.parse(readFileSync(new URL('./vectors.json', import.meta.url), 'utf8')).pricing as {
    split_gross: { gross: string; bps: number; effective: string; tax: string }[];
    gross_for_effective: { effective: string; bps: number; gross: Opt }[];
    split_tax: { tax: string; share_bps: number; creator: string; protocol: string }[];
    launch_block: { recorded_share_bps: number; launch_slot: string; slot: string; share_bps: number }[];
    swaps: { reserve_in: string; reserve_out: string; amount: string; out_given_in: string; in_given_out: Opt }[];
    quotes: {
        reserve_in: string;
        reserve_out: string;
        tax_bps: number;
        protocol_share_bps: number;
        mode: 'exact_in' | 'exact_out';
        amount: string;
        result: Record<string, string>;
    }[];
};

const big = (v: Opt) => (v === null ? null : BigInt(v));

describe('the pool maths matches the Rust crate', () => {
    it('splits a buy into tax and effective input', () => {
        for (const v of vectors.split_gross) {
            expect(splitGross(BigInt(v.gross), v.bps), JSON.stringify(v)).toEqual({
                effective: BigInt(v.effective),
                tax: BigInt(v.tax),
            });
        }
    });

    it('finds the gross for an effective input', () => {
        for (const v of vectors.gross_for_effective) {
            expect(grossForEffective(BigInt(v.effective), v.bps), JSON.stringify(v)).toBe(big(v.gross));
        }
    });

    it('gives the launch block tax to the protocol', () => {
        expect(vectors.launch_block.length).toBeGreaterThan(0);
        for (const v of vectors.launch_block) {
            expect(
                protocolShareAt(v.recorded_share_bps, BigInt(v.launch_slot), BigInt(v.slot)),
                JSON.stringify(v),
            ).toBe(v.share_bps);
        }
    });

    it('prices with the launch block share only when given a slot', () => {
        const pool = {
            status: 'live',
            creator: 'Creator1111111111111111111111111111111111111',
            reserveQuote: 5_000_000_000n,
            reserveToken: 1_000_000_000_000n,
            taxStartBps: 9_900,
            taxEndBps: 0,
            taxDurationSecs: 90,
            protocolTaxShareBps: 3_000,
            launchTs: 1_000n,
            launchSlot: 500n,
        } as unknown as LaunchPool;
        expect(poolPricing(pool, 'buy', null, 1_000n).protocolShareBps).toBe(3_000);
        expect(poolPricing(pool, 'buy', null, 1_000n, 500n).protocolShareBps).toBe(10_000);
        expect(poolPricing(pool, 'buy', null, 1_000n, 501n).protocolShareBps).toBe(10_000);
        expect(poolPricing(pool, 'buy', null, 1_000n, 502n).protocolShareBps).toBe(3_000);
        // The trader's side is the same in and out of the launch block.
        const inBlock = poolPricing(pool, 'buy', null, 1_000n, 500n);
        const after = poolPricing(pool, 'buy', null, 1_000n, 502n);
        expect({ ...inBlock, protocolShareBps: 0 }).toEqual({ ...after, protocolShareBps: 0 });
    });

    it('shares tax between creator and protocol', () => {
        for (const v of vectors.split_tax) {
            expect(splitTax(BigInt(v.tax), v.share_bps), JSON.stringify(v)).toEqual({
                creator: BigInt(v.creator),
                protocol: BigInt(v.protocol),
            });
        }
    });

    it('prices constant-product swaps both ways', () => {
        for (const v of vectors.swaps) {
            const [rin, rout, amount] = [BigInt(v.reserve_in), BigInt(v.reserve_out), BigInt(v.amount)];
            expect(outGivenIn(rin, rout, amount), JSON.stringify(v)).toBe(BigInt(v.out_given_in));
            expect(inGivenOut(rin, rout, amount), JSON.stringify(v)).toBe(big(v.in_given_out));
        }
    });

    it('quotes whole trades, errors included', () => {
        expect(vectors.quotes.some((q) => 'error' in q.result)).toBe(true);
        for (const v of vectors.quotes) {
            const pricing = {
                reserveIn: BigInt(v.reserve_in),
                reserveOut: BigInt(v.reserve_out),
                taxBps: v.tax_bps,
                protocolShareBps: v.protocol_share_bps,
            };
            const result = (v.mode === 'exact_in' ? quoteExactIn : quoteExactOut)(pricing, BigInt(v.amount));
            if ('error' in v.result) {
                expect(result, JSON.stringify(v)).toEqual({ ok: false, error: v.result.error });
            } else {
                expect(result, JSON.stringify(v)).toEqual({
                    ok: true,
                    quote: {
                        amountIn: BigInt(v.result.amount_in!),
                        amountOut: BigInt(v.result.amount_out!),
                        effectiveIn: BigInt(v.result.effective_in!),
                        creatorTax: BigInt(v.result.creator_tax!),
                        protocolFee: BigInt(v.result.protocol_fee!),
                        newReserveIn: BigInt(v.result.new_reserve_in!),
                        newReserveOut: BigInt(v.result.new_reserve_out!),
                    },
                });
            }
        }
    });
});
