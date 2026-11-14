// The TypeScript SDK against vectors computed by the Rust client (`npm run vectors`). Every
// address, instruction byte, account field and tax rate must match exactly.

import { readFileSync } from 'node:fs';

import { PIPELINE_PROGRAM_ADDRESS, TradeMode } from '@pipeline-sol/sdk';
import { address, isSignerRole, isWritableRole, type Address, type ReadonlyUint8Array } from '@solana/kit';
import { describe, expect, it } from 'vitest';

import {
    ACCOUNT_DISCRIMINATORS,
    INSTRUCTION_DISCRIMINATORS,
    LAUNCH_BLOCK_SLOTS,
    MAX_DURATION_SECS,
    MAX_END_BPS,
    MAX_LAUNCH_FEE_LAMPORTS,
    MAX_PROTOCOL_TAX_SHARE_BPS,
    MAX_SETUP_LAMPORTS,
    MAX_START_BPS,
    MIN_DURATION_SECS,
    NATIVE_MINT,
    PROGRAM_ID,
    applicableTaxBps,
    claimTaxIx,
    configAddress,
    createMarketIx,
    creatorQuoteAccount,
    creatorTokenAccount,
    currentTaxBps,
    decodeLaunchPool,
    decodeProtocolConfig,
    encodeInitParams,
    exactIn,
    exactOut,
    feeAccountIxs,
    getLaunchPoolCodec,
    isValidSchedule,
    launchIx,
    poolKeys,
    quoteIx,
    swapIx,
    sweepIx,
    poolParams,
    verifyPoolKeys,
    type InitParams,
    type LaunchPool,
    type PipelineInstruction,
} from '../src/index.js';

type Meta = { pubkey: string; is_signer: boolean; is_writable: boolean };
type Ix = { program_id: string; accounts: Meta[]; data: string };

const vectors = JSON.parse(readFileSync(new URL('./vectors.json', import.meta.url), 'utf8'));

function hex(bytes: ReadonlyUint8Array): string {
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

function fromHex(text: string): Uint8Array {
    return new Uint8Array((text.match(/../g) ?? []).map((byte) => parseInt(byte, 16)));
}

function asMetas(metas: Meta[]) {
    return metas.map((m) => ({
        address: address(m.pubkey),
        role: (m.is_signer ? 2 : 0) | (m.is_writable ? 1 : 0),
    }));
}

function expectInstruction(actual: PipelineInstruction, expected: Ix) {
    expect(actual.programAddress).toBe(expected.program_id);
    expect(
        actual.accounts.map((a) => ({
            pubkey: a.address,
            is_signer: isSignerRole(a.role),
            is_writable: isWritableRole(a.role),
        })),
    ).toEqual(expected.accounts);
    expect(hex(actual.data)).toBe(expected.data);
}

describe('constants', () => {
    const c = vectors.constants;
    it('match the Rust client', async () => {
        expect(PROGRAM_ID).toBe(c.program_id);
        expect(await configAddress()).toBe(c.config);
        expect(NATIVE_MINT).toBe(c.native_mint);
        expect(MAX_SETUP_LAMPORTS).toBe(BigInt(c.max_setup_lamports));
        expect(MAX_START_BPS).toBe(c.max_start_bps);
        expect(MAX_END_BPS).toBe(c.max_end_bps);
        expect(MIN_DURATION_SECS).toBe(c.min_duration_secs);
        expect(MAX_DURATION_SECS).toBe(c.max_duration_secs);
        expect(MAX_PROTOCOL_TAX_SHARE_BPS).toBe(c.max_protocol_tax_share_bps);
        expect(MAX_LAUNCH_FEE_LAMPORTS).toBe(BigInt(c.max_launch_fee_lamports));
        expect(LAUNCH_BLOCK_SLOTS).toBe(BigInt(c.launch_block_slots));
        expect(hex(INSTRUCTION_DISCRIMINATORS.launch)).toBe(c.instruction_discriminators.launch);
        expect(hex(INSTRUCTION_DISCRIMINATORS.claimTax)).toBe(c.instruction_discriminators.claim_tax);
        expect(hex(INSTRUCTION_DISCRIMINATORS.sweepProtocolFees)).toBe(
            c.instruction_discriminators.sweep_protocol_fees,
        );
        expect(hex(INSTRUCTION_DISCRIMINATORS.pipelineQuote)).toBe(c.instruction_discriminators.pipeline_quote);
        expect(hex(ACCOUNT_DISCRIMINATORS.LaunchPool)).toBe(c.account_discriminators.LaunchPool);
        expect(hex(ACCOUNT_DISCRIMINATORS.ProtocolConfig)).toBe(c.account_discriminators.ProtocolConfig);
    });
});

for (const pool of vectors.pools) {
    describe(`pool: ${pool.name}`, () => {
        const params: InitParams = {
            creator: address(pool.params.creator),
            taxStartBps: pool.params.tax_start_bps,
            taxEndBps: pool.params.tax_end_bps,
            taxDurationSecs: pool.params.tax_duration_secs,
        };
        const keys = () =>
            poolKeys(
                params,
                address(pool.token_mint),
                address(pool.token_program),
                address(pool.quote_mint),
                address(pool.quote_program),
            );
        const inputs = pool.inputs;
        const payer: Address = address(inputs.payer);
        const treasury: Address = address(inputs.treasury);
        const trader: Address = address(inputs.trader);
        const hooks = asMetas(inputs.hooks);
        const ixs = pool.instructions;

        it('encodes its init data', () => {
            expect(hex(encodeInitParams(params))).toBe(pool.init_data);
        });

        it('derives every address', async () => {
            const k = await keys();
            expect(k.address).toBe(pool.keys.address);
            expect(k.market).toBe(pool.keys.market);
            expect(k.tokenVault).toBe(pool.keys.token_vault);
            expect(k.quoteVault).toBe(pool.keys.quote_vault);
            expect(await creatorTokenAccount(k)).toBe(pool.keys.creator_token);
            expect(await creatorQuoteAccount(k)).toBe(pool.keys.creator_quote);
        });

        it('builds create_market and the fee accounts', async () => {
            const k = await keys();
            expectInstruction(await createMarketIx(k, payer), ixs.create_market);
            const [token, quote] = await feeAccountIxs(k, payer);
            expectInstruction(token, ixs.fee_accounts[0]);
            expectInstruction(quote, ixs.fee_accounts[1]);
        });

        it('builds launch, with and without hook accounts', async () => {
            const k = await keys();
            expectInstruction(await launchIx(k, treasury, 800_000_000_000_000n, 5_000_000_000n), ixs.launch);
            expectInstruction(
                await launchIx(k, treasury, 800_000_000_000_000n, 5_000_000_000n, hooks),
                ixs.launch_with_hooks,
            );
        });

        it('builds claim_tax and sweep_protocol_fees', async () => {
            const k = await keys();
            expectInstruction(await claimTaxIx(k, await creatorQuoteAccount(k)), ixs.claim_tax);
            expectInstruction(await sweepIx(k, treasury, address(inputs.treasury_quote)), ixs.sweep);
        });

        it('builds quotes', async () => {
            const k = await keys();
            expectInstruction(await quoteIx(k, 'buy', TradeMode.ExactIn, 1_000_000_000n, null), ixs.quote_buy_exact_in);
            expectInstruction(
                await quoteIx(k, 'sell', TradeMode.ExactOut, 12_345n, params.creator),
                ixs.quote_sell_exact_out_creator,
            );
        });

        it('builds swaps through the router', async () => {
            const k = await keys();
            expectInstruction(await swapIx(k, trader, 'buy', exactIn(1_000_000_000n, 123n)), ixs.swap_buy_exact_in);
            expectInstruction(
                await swapIx(k, trader, 'sell', exactOut(2n ** 64n - 1n, 777n), hooks, hooks),
                ixs.swap_sell_exact_out_hooks,
            );
        });

        it('adds an integrator fee: the account in its slot, the rate in the data', async () => {
            const k = await keys();
            const feeAccount = address('Fee1111111111111111111111111111111111111111');
            const plain = ixs.swap_buy_exact_in as Ix;
            const withFee = await swapIx(k, trader, 'buy', exactIn(1_000_000_000n, 123n, 75), [], [], feeAccount);
            // Without a fee the router's slot holds its own program id, read-only.
            const slot = plain.accounts.findIndex((m) => m.pubkey === PIPELINE_PROGRAM_ADDRESS && !m.is_writable);
            expect(slot).toBeGreaterThan(0);
            const accounts = plain.accounts.map((m, i) =>
                i === slot ? { pubkey: feeAccount, is_signer: false, is_writable: true } : m,
            );
            // Swap data: 8-byte discriminator, mode (1), amounts (3 × 8), then the fee rate as a u16.
            const data = fromHex(plain.data);
            data.set([75, 0], 8 + 1 + 3 * 8);
            expectInstruction(withFee, { program_id: plain.program_id, accounts, data: hex(data) });
        });

        it('refuses an integrator fee without an account to pay it to', async () => {
            const k = await keys();
            await expect(swapIx(k, trader, 'buy', exactIn(1n, 0n, 1))).rejects.toThrow(/integratorFeeAccount/);
        });
    });
}

describe('integrator fee rate', () => {
    it('is a u16, zero by default', () => {
        expect(exactIn(1n, 0n).integratorFeeBps).toBe(0);
        expect(exactOut(1n, 1n, 65_535).integratorFeeBps).toBe(65_535);
        for (const bad of [-1, 65_536, 1.5, Number.NaN]) {
            expect(() => exactIn(1n, 0n, bad)).toThrow(RangeError);
            expect(() => exactOut(1n, 1n, bad)).toThrow(RangeError);
        }
    });
});

describe('accounts', () => {
    const accounts = vectors.accounts;

    it('decodes launch pools field for field, and re-encodes them', () => {
        for (const vector of accounts.launch_pools) {
            const data = fromHex(vector.data);
            const pool = decodeLaunchPool(data);
            expect(pool).not.toBeNull();
            const d = vector.decoded;
            expect({
                version: pool!.version,
                status: pool!.status,
                bump: pool!.bump,
                creator: pool!.creator,
                token_mint: pool!.tokenMint,
                quote_mint: pool!.quoteMint,
                token_vault: pool!.tokenVault,
                quote_vault: pool!.quoteVault,
                market_signer: pool!.marketSigner,
                reserve_token: pool!.reserveToken.toString(),
                reserve_quote: pool!.reserveQuote.toString(),
                accrued_tax_quote: pool!.accruedTaxQuote.toString(),
                accrued_protocol_quote: pool!.accruedProtocolQuote.toString(),
                tax_start_bps: pool!.taxStartBps,
                tax_end_bps: pool!.taxEndBps,
                tax_duration_secs: pool!.taxDurationSecs,
                protocol_tax_share_bps: pool!.protocolTaxShareBps,
                launch_fee_lamports: pool!.launchFeeLamports.toString(),
                launch_ts: pool!.launchTs.toString(),
                launch_slot: pool!.launchSlot.toString(),
            }).toEqual(d);
            expect(hex(getLaunchPoolCodec().encode(pool!))).toBe(vector.data.slice(16));
        }
    });

    it('decodes protocol configs, with and without a proposed admin', () => {
        for (const vector of accounts.protocol_configs) {
            const config = decodeProtocolConfig(fromHex(vector.data));
            expect(config).not.toBeNull();
            expect({
                admin: config!.admin,
                treasury: config!.treasury,
                protocol_tax_share_bps: config!.protocolTaxShareBps,
                launch_fee_lamports: config!.launchFeeLamports.toString(),
                bump: config!.bump,
                pending_admin: config!.pendingAdmin,
            }).toEqual(vector.decoded);
        }
    });

    it('refuses accounts that are not pools', () => {
        expect(decodeLaunchPool(fromHex(accounts.not_a_pool))).toBeNull();
        expect(decodeLaunchPool(new Uint8Array())).toBeNull();
        // A pool's discriminator on too few bytes.
        expect(decodeLaunchPool(fromHex(accounts.launch_pools[0].data.slice(0, 40)))).toBeNull();
    });
});

describe('tax', () => {
    it('matches the program at every point of every schedule', () => {
        for (const v of vectors.tax.rates) {
            const schedule = { startBps: v.start_bps, endBps: v.end_bps, durationSecs: v.duration_secs };
            expect(
                applicableTaxBps(schedule, BigInt(v.launch_ts), BigInt(v.now), v.side, v.trader_is_creator),
                JSON.stringify(v),
            ).toBe(v.bps);
        }
    });

    it('accepts exactly the schedules the program accepts', () => {
        for (const v of vectors.tax.validity) {
            expect(
                isValidSchedule({ startBps: v.start_bps, endBps: v.end_bps, durationSecs: v.duration_secs }),
                JSON.stringify(v),
            ).toBe(v.valid);
        }
    });

    it('is 0 on a pending pool and the schedule rate on a live one', () => {
        const [live, pending] = vectors.accounts.launch_pools.map((v: { data: string }) =>
            decodeLaunchPool(fromHex(v.data)),
        );
        expect(currentTaxBps(pending!, 1_800_000_000n)).toBe(0);
        expect(currentTaxBps(live!, 1_800_000_000n)).toBe(9_900);
        expect(currentTaxBps(live!, 1_800_000_300n)).toBe(300 + Math.floor((9_600 * 300) / 600));
        expect(currentTaxBps(live!, 1_800_000_600n)).toBe(300);
    });
});

describe('pool keys from a pool account', () => {
    function poolAt(v: (typeof vectors.pools)[number]): { at: Address; pool: LaunchPool } {
        return {
            at: address(v.keys.address),
            pool: {
                version: 1,
                status: 'live',
                bump: 255,
                creator: address(v.params.creator),
                tokenMint: address(v.token_mint),
                quoteMint: address(v.quote_mint),
                tokenVault: address(v.keys.token_vault),
                quoteVault: address(v.keys.quote_vault),
                marketSigner: address(v.params.creator),
                reserveToken: 1n,
                reserveQuote: 1n,
                accruedTaxQuote: 0n,
                accruedProtocolQuote: 0n,
                taxStartBps: v.params.tax_start_bps,
                taxEndBps: v.params.tax_end_bps,
                taxDurationSecs: v.params.tax_duration_secs,
                protocolTaxShareBps: 1_000,
                launchFeeLamports: 0n,
                launchTs: 0n,
                launchSlot: 0n,
                reserved: new Uint8Array(56),
            },
        };
    }

    it('accepts a pool at its own address, with either token program', async () => {
        for (const v of vectors.pools) {
            const { at, pool } = poolAt(v);
            const keys = await verifyPoolKeys(at, pool);
            expect(keys?.market, v.name).toBe(v.keys.market);
            expect(keys?.tokenProgram, v.name).toBe(v.token_program);
            expect(keys?.quoteProgram, v.name).toBe(v.quote_program);
            expect(poolParams(pool)).toEqual(keys?.params);
        }
    });

    it('refuses a pool whose fields or vaults do not match its address', async () => {
        const { at, pool } = poolAt(vectors.pools[0]);
        const other = address(vectors.pools[1].keys.address);
        expect(await verifyPoolKeys(other, pool)).toBeNull();
        expect(await verifyPoolKeys(at, { ...pool, taxStartBps: pool.taxStartBps - 1 })).toBeNull();
        expect(await verifyPoolKeys(at, { ...pool, creator: other })).toBeNull();
        expect(await verifyPoolKeys(at, { ...pool, tokenVault: other })).toBeNull();
        expect(await verifyPoolKeys(at, { ...pool, quoteVault: pool.tokenVault })).toBeNull();
    });
});
