// The tax schedule, for showing the tax a buy pays right now. Integer maths identical to
// `TaxSchedule::tax_bps_at` and `applicable_tax_bps` in `crates/sniper-tax-math`, so a
// front end shows exactly the rate the program charges.

import { MAX_DURATION_SECS, MAX_END_BPS, MAX_START_BPS, MIN_DURATION_SECS } from './constants.js';
import type { LaunchPool } from './codecs.js';
import type { Side } from './pda.js';

export type TaxSchedule = {
    startBps: number;
    endBps: number;
    durationSecs: number;
};

/** Whether a schedule meets the bounds the program enforces at `create_market`. */
export function isValidSchedule(schedule: TaxSchedule): boolean {
    return (
        Number.isInteger(schedule.startBps) &&
        Number.isInteger(schedule.endBps) &&
        Number.isInteger(schedule.durationSecs) &&
        schedule.endBps >= 0 &&
        schedule.startBps <= MAX_START_BPS &&
        schedule.endBps <= MAX_END_BPS &&
        schedule.endBps <= schedule.startBps &&
        schedule.durationSecs >= MIN_DURATION_SECS &&
        schedule.durationSecs <= MAX_DURATION_SECS
    );
}

/** The schedule's rate at `now`, for a pool launched at `launchTs` (both Unix seconds). */
export function taxBpsAt(schedule: TaxSchedule, launchTs: bigint, now: bigint): number {
    const elapsed = now - launchTs;
    if (elapsed <= 0n) {
        return schedule.startBps;
    }
    const duration = BigInt(schedule.durationSecs);
    if (elapsed >= duration) {
        return schedule.endBps;
    }
    const span = BigInt(Math.max(schedule.startBps - schedule.endBps, 0));
    return schedule.endBps + Number((span * (duration - elapsed)) / duration);
}

/** The rate a trade pays: the schedule's rate for a buy by anyone but the creator. */
export function applicableTaxBps(
    schedule: TaxSchedule,
    launchTs: bigint,
    now: bigint,
    side: Side,
    traderIsCreator: boolean,
): number {
    return side === 'sell' || traderIsCreator ? 0 : taxBpsAt(schedule, launchTs, now);
}

export function poolSchedule(pool: LaunchPool): TaxSchedule {
    return { startBps: pool.taxStartBps, endBps: pool.taxEndBps, durationSecs: pool.taxDurationSecs };
}

/** The tax an anonymous buy on `pool` pays at `now`. 0 while the pool is pending. */
export function currentTaxBps(pool: LaunchPool, now: bigint): number {
    return pool.status === 'live' ? taxBpsAt(poolSchedule(pool), pool.launchTs, now) : 0;
}

/** Seconds until the window ends and only the permanent rate is left; 0 once it has. */
export function secondsUntilWindowEnds(pool: LaunchPool, now: bigint): bigint {
    if (pool.status !== 'live') {
        return 0n;
    }
    const end = pool.launchTs + BigInt(pool.taxDurationSecs);
    return end > now ? end - now : 0n;
}
