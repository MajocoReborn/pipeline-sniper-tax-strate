// Instruction builders for the sniper-tax program and the router calls around it. Each
// mirrors the builder of the same name in `crates/sniper-tax-client`: same accounts, order,
// signer and writable flags, and data bytes. `test/vectors.json` checks this.

import { AccountRole, getU64Codec, type AccountMeta, type Address, type ReadonlyUint8Array } from '@solana/kit';

import { INSTRUCTION_DISCRIMINATORS, MAX_SETUP_LAMPORTS, PROGRAM_ID } from './constants.js';
import { encodeInitParams } from './codecs.js';
import { configAddress, creatorQuoteAccount, creatorTokenAccount, legs, type PoolKeys, type Side } from './pda.js';
import {
    ASSOCIATED_TOKEN_PROGRAM_ADDRESS,
    DEFAULT_ADDRESS,
    SYSTEM_PROGRAM_ADDRESS,
    TradeMode,
    associatedTokenAddress,
    createMarket,
    getTradeRequestCodec,
    initFeeAccount,
    swap,
    type PipelineInstruction,
    type SwapInstructionArgs,
} from '@pipeline-sol/sdk';

export type { PipelineInstruction };

function readonly(address: Address): AccountMeta {
    return { address, role: AccountRole.READONLY };
}

function writable(address: Address): AccountMeta {
    return { address, role: AccountRole.WRITABLE };
}

function readonlySigner(address: Address): AccountMeta {
    return { address, role: AccountRole.READONLY_SIGNER };
}

function writableSigner(address: Address): AccountMeta {
    return { address, role: AccountRole.WRITABLE_SIGNER };
}

function concat(...parts: ReadonlyUint8Array[]): Uint8Array {
    const data = new Uint8Array(parts.reduce((length, part) => length + part.length, 0));
    let offset = 0;
    for (const part of parts) {
        data.set(part, offset);
        offset += part.length;
    }
    return data;
}

function programInstruction(accounts: AccountMeta[], data: Uint8Array): PipelineInstruction {
    return { programAddress: PROGRAM_ID, accounts, data };
}

/** The router's `create_market` for this pool. Anyone may pay for it. */
export async function createMarketIx(keys: PoolKeys, payer: Address): Promise<PipelineInstruction> {
    return createMarket({
        creator: payer,
        strategyProgram: PROGRAM_ID,
        marketState: keys.address,
        marketStateSigns: false,
        initData: encodeInitParams(keys.params),
        maxCreatorLamports: MAX_SETUP_LAMPORTS,
        strategyAccounts: [
            readonly(keys.tokenMint),
            readonly(keys.quoteMint),
            writable(keys.tokenVault),
            writable(keys.quoteVault),
            readonly(keys.tokenProgram),
            readonly(keys.quoteProgram),
            readonly(ASSOCIATED_TOKEN_PROGRAM_ADDRESS),
            readonly(await configAddress()),
        ],
    });
}

/** The router's fee accounts for both mints. Anyone may pay for them. */
export async function feeAccountIxs(
    keys: PoolKeys,
    payer: Address,
): Promise<[PipelineInstruction, PipelineInstruction]> {
    return [
        await initFeeAccount({ payer, market: keys.market, mint: keys.tokenMint, tokenProgram: keys.tokenProgram }),
        await initFeeAccount({ payer, market: keys.market, mint: keys.quoteMint, tokenProgram: keys.quoteProgram }),
    ];
}

/**
 * `launch`, signed by the creator, from their associated token accounts. `hookAccounts`
 * are the quote mint's transfer-hook accounts, if it has a hook.
 */
export async function launchIx(
    keys: PoolKeys,
    treasury: Address,
    amountToken: bigint,
    amountQuote: bigint,
    hookAccounts: readonly AccountMeta[] = [],
): Promise<PipelineInstruction> {
    const u64 = getU64Codec();
    return programInstruction(
        [
            writableSigner(keys.params.creator),
            writable(keys.address),
            readonly(keys.tokenMint),
            readonly(keys.quoteMint),
            writable(await creatorTokenAccount(keys)),
            writable(await creatorQuoteAccount(keys)),
            writable(keys.tokenVault),
            writable(keys.quoteVault),
            readonly(keys.tokenProgram),
            readonly(keys.quoteProgram),
            readonly(await configAddress()),
            writable(treasury),
            readonly(SYSTEM_PROGRAM_ADDRESS),
            ...hookAccounts,
        ],
        concat(INSTRUCTION_DISCRIMINATORS.launch, u64.encode(amountToken), u64.encode(amountQuote)),
    );
}

/** `claim_tax`, signed by the creator, to any account in the quote mint. */
export async function claimTaxIx(
    keys: PoolKeys,
    destination: Address,
    hookAccounts: readonly AccountMeta[] = [],
): Promise<PipelineInstruction> {
    return programInstruction(
        [
            readonlySigner(keys.params.creator),
            writable(keys.address),
            readonly(keys.quoteMint),
            writable(keys.quoteVault),
            writable(destination),
            readonly(keys.quoteProgram),
            ...hookAccounts,
        ],
        new Uint8Array(INSTRUCTION_DISCRIMINATORS.claimTax),
    );
}

/**
 * `sweep_protocol_fees` to `destination`, which must be the treasury's associated token
 * account for the quote mint. Anyone may send it.
 */
export async function sweepIx(
    keys: PoolKeys,
    treasury: Address,
    destination: Address,
    hookAccounts: readonly AccountMeta[] = [],
): Promise<PipelineInstruction> {
    return programInstruction(
        [
            writable(keys.address),
            readonly(await configAddress()),
            readonly(keys.quoteMint),
            writable(keys.quoteVault),
            readonly(treasury),
            writable(destination),
            readonly(keys.quoteProgram),
            ...hookAccounts,
        ],
        new Uint8Array(INSTRUCTION_DISCRIMINATORS.sweepProtocolFees),
    );
}

/**
 * `pipeline_quote`, for simulation. Only the market signer and the pool are read; the
 * other fixed accounts are placeholders. A `null` trader pays the tax.
 */
export async function quoteIx(
    keys: PoolKeys,
    side: Side,
    mode: TradeMode,
    amount: bigint,
    trader: Address | null,
): Promise<PipelineInstruction> {
    const leg = legs(keys, side);
    const request = getTradeRequestCodec().encode({
        mode,
        inputIndex: leg.inputIndex,
        outputIndex: leg.outputIndex,
        amount,
        trader,
        extra: new Uint8Array(),
    });
    return programInstruction(
        [readonly(keys.market), readonly(keys.address), ...Array.from({ length: 8 }, () => readonly(DEFAULT_ADDRESS))],
        concat(INSTRUCTION_DISCRIMINATORS.pipelineQuote, request),
    );
}

function checkFeeBps(integratorFeeBps: number): number {
    if (!Number.isInteger(integratorFeeBps) || integratorFeeBps < 0 || integratorFeeBps > 0xffff) {
        throw new RangeError(`integratorFeeBps must be a u16, got ${integratorFeeBps}`);
    }
    return integratorFeeBps;
}

/**
 * Router swap arguments: exact in, at least `minAmountOut` out. `integratorFeeBps` is the
 * front end's own fee (Pipeline's integrator fee), charged by the router on top of the input
 * and paid to the account passed to `swapIx`. The trader signs the rate; the router has no cap.
 */
export function exactIn(amountIn: bigint, minAmountOut: bigint, integratorFeeBps = 0): SwapInstructionArgs {
    return {
        mode: TradeMode.ExactIn,
        amountIn,
        amountOut: 0n,
        minAmountOut,
        integratorFeeBps: checkFeeBps(integratorFeeBps),
        extra: new Uint8Array(),
    };
}

/** Router swap arguments: exactly `amountOut` out, at most `maxAmountIn` in. See `exactIn`. */
export function exactOut(maxAmountIn: bigint, amountOut: bigint, integratorFeeBps = 0): SwapInstructionArgs {
    return {
        mode: TradeMode.ExactOut,
        amountIn: maxAmountIn,
        amountOut,
        minAmountOut: amountOut,
        integratorFeeBps: checkFeeBps(integratorFeeBps),
        extra: new Uint8Array(),
    };
}

/**
 * A swap through the router, from and to `trader`'s associated token accounts. The output
 * mint's hook accounts go after the strategy's own accounts; the input mint's are the
 * router's `inputHookAccounts`.
 *
 * `integratorFeeAccount` receives the integrator fee set in `args`: a token account in the
 * trade's input mint (wrapped SOL on a buy, the launch token on a sell), which must exist. It
 * is required when the fee is above zero and ignored by the router when it is zero.
 */
export async function swapIx(
    keys: PoolKeys,
    trader: Address,
    side: Side,
    args: SwapInstructionArgs,
    outputHooks: readonly AccountMeta[] = [],
    inputHooks: readonly AccountMeta[] = [],
    integratorFeeAccount: Address | null = null,
): Promise<PipelineInstruction> {
    if (args.integratorFeeBps > 0 && integratorFeeAccount === null) {
        throw new Error('an integrator fee needs an integratorFeeAccount');
    }
    const leg = legs(keys, side);
    return swap(
        {
            trader,
            strategyProgram: PROGRAM_ID,
            marketState: keys.address,
            traderInput: await associatedTokenAddress(trader, leg.inputMint, leg.inputProgram),
            traderOutput: await associatedTokenAddress(trader, leg.outputMint, leg.outputProgram),
            inputVault: leg.inputVault,
            inputMint: leg.inputMint,
            outputMint: leg.outputMint,
            inputTokenProgram: leg.inputProgram,
            outputTokenProgram: leg.outputProgram,
            integratorFeeAccount,
            strategyAccounts: [writable(leg.outputVault), ...outputHooks],
            inputHookAccounts: inputHooks,
        },
        args,
    );
}
