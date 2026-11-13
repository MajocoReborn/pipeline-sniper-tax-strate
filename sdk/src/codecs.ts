// Borsh layouts of the sniper-tax program: `InitParams` and its two accounts. Field for
// field the same as `programs/sniper-tax/src/state.rs` and `crates/sniper-tax-client`.

import {
    fixCodecSize,
    getAddressCodec,
    getBytesCodec,
    getI64Codec,
    getStructCodec,
    getU16Codec,
    getU32Codec,
    getU64Codec,
    getU8Codec,
    transformCodec,
    type Address,
    type Codec,
    type ReadonlyUint8Array,
} from '@solana/kit';

import { ACCOUNT_DISCRIMINATORS } from './constants.js';
import { DEFAULT_ADDRESS } from '@pipeline-sol/sdk';

/** Setup data for `create_market`: who launches the pool, and its tax schedule. */
export type InitParams = {
    /** Launches the pool, trades tax-free and receives the creator's part of the tax. */
    creator: Address;
    taxStartBps: number;
    taxEndBps: number;
    taxDurationSecs: number;
};

export function getInitParamsCodec(): Codec<InitParams> {
    return getStructCodec([
        ['creator', getAddressCodec()],
        ['taxStartBps', getU16Codec()],
        ['taxEndBps', getU16Codec()],
        ['taxDurationSecs', getU32Codec()],
    ]);
}

/** `InitParams` as the router's `init_data`: always 40 bytes. */
export function encodeInitParams(params: InitParams): Uint8Array {
    return new Uint8Array(getInitParamsCodec().encode(params));
}

export type PoolStatus = 'pending' | 'live';

function getPoolStatusCodec(): Codec<PoolStatus> {
    return transformCodec(
        getU8Codec(),
        (status: PoolStatus) => (status === 'pending' ? 0 : 1),
        (byte: number): PoolStatus => {
            if (byte === 0) return 'pending';
            if (byte === 1) return 'live';
            throw new RangeError(`unknown pool status ${byte}`);
        },
    );
}

/** A launch pool, as stored on chain. */
export type LaunchPool = {
    version: number;
    status: PoolStatus;
    bump: number;
    creator: Address;
    tokenMint: Address;
    quoteMint: Address;
    tokenVault: Address;
    quoteVault: Address;
    marketSigner: Address;
    reserveToken: bigint;
    reserveQuote: bigint;
    /** The creator's unclaimed tax, held in the quote vault outside the reserves. */
    accruedTaxQuote: bigint;
    /** The protocol's unclaimed tax, held in the quote vault outside the reserves. */
    accruedProtocolQuote: bigint;
    taxStartBps: number;
    taxEndBps: number;
    taxDurationSecs: number;
    protocolTaxShareBps: number;
    launchFeeLamports: bigint;
    /** Unix seconds; 0 while pending. */
    launchTs: bigint;
    reserved: ReadonlyUint8Array;
};

export function getLaunchPoolCodec(): Codec<LaunchPool> {
    return getStructCodec([
        ['version', getU8Codec()],
        ['status', getPoolStatusCodec()],
        ['bump', getU8Codec()],
        ['creator', getAddressCodec()],
        ['tokenMint', getAddressCodec()],
        ['quoteMint', getAddressCodec()],
        ['tokenVault', getAddressCodec()],
        ['quoteVault', getAddressCodec()],
        ['marketSigner', getAddressCodec()],
        ['reserveToken', getU64Codec()],
        ['reserveQuote', getU64Codec()],
        ['accruedTaxQuote', getU64Codec()],
        ['accruedProtocolQuote', getU64Codec()],
        ['taxStartBps', getU16Codec()],
        ['taxEndBps', getU16Codec()],
        ['taxDurationSecs', getU32Codec()],
        ['protocolTaxShareBps', getU16Codec()],
        ['launchFeeLamports', getU64Codec()],
        ['launchTs', getI64Codec()],
        ['reserved', fixCodecSize(getBytesCodec(), 64)],
    ]);
}

/** The protocol config, as stored on chain. */
export type ProtocolConfig = {
    admin: Address;
    treasury: Address;
    protocolTaxShareBps: number;
    launchFeeLamports: bigint;
    bump: number;
    /** The proposed next admin, or `null` when nothing is proposed. */
    pendingAdmin: Address | null;
    reserved: ReadonlyUint8Array;
};

export function getProtocolConfigCodec(): Codec<ProtocolConfig> {
    return getStructCodec([
        ['admin', getAddressCodec()],
        ['treasury', getAddressCodec()],
        ['protocolTaxShareBps', getU16Codec()],
        ['launchFeeLamports', getU64Codec()],
        ['bump', getU8Codec()],
        [
            'pendingAdmin',
            transformCodec(
                getAddressCodec(),
                (admin: Address | null) => admin ?? DEFAULT_ADDRESS,
                (admin: Address) => (admin === DEFAULT_ADDRESS ? null : admin),
            ),
        ],
        ['reserved', fixCodecSize(getBytesCodec(), 32)],
    ]);
}

function startsWith(data: ReadonlyUint8Array, prefix: ReadonlyUint8Array): boolean {
    return data.length >= prefix.length && prefix.every((byte, i) => data[i] === byte);
}

function decodeAccount<T>(discriminator: ReadonlyUint8Array, codec: Codec<T>, data: ReadonlyUint8Array): T | null {
    if (!startsWith(data, discriminator)) {
        return null;
    }
    try {
        return codec.read(data, discriminator.length)[0];
    } catch {
        return null;
    }
}

/** Decodes a pool account, checking its Anchor discriminator. `null` if it isn't one. */
export function decodeLaunchPool(data: ReadonlyUint8Array): LaunchPool | null {
    return decodeAccount(ACCOUNT_DISCRIMINATORS.LaunchPool, getLaunchPoolCodec(), data);
}

/** Decodes the config account, checking its Anchor discriminator. `null` if it isn't one. */
export function decodeProtocolConfig(data: ReadonlyUint8Array): ProtocolConfig | null {
    return decodeAccount(ACCOUNT_DISCRIMINATORS.ProtocolConfig, getProtocolConfigCodec(), data);
}
