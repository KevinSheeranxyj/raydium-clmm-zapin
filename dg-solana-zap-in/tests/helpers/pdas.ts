import { PublicKey } from "@solana/web3.js";

export function operationDataPda(transferId: string, programId: PublicKey) {
    return PublicKey.findProgramAddressSync(
        [Buffer.from("operation_data"), Buffer.from(transferId)],
        programId
    );
}

export function registryPda(programId: PublicKey) {
    return PublicKey.findProgramAddressSync([Buffer.from("registry")], programId);
}

export function positionNftMintPda(
    user: PublicKey,
    poolState: PublicKey,
    programId: PublicKey
) {
    return PublicKey.findProgramAddressSync(
        [Buffer.from("pos_nft_mint"), user.toBuffer(), poolState.toBuffer()],
        programId
    );
}

// Raydium/UniV3 每个TickArray覆盖 88 * tickSpacing
const TICK_ARRAY_SIZE = 88;

export function tickArrayStartIndex(tick: number, tickSpacing: number) {
    const span = tickSpacing * TICK_ARRAY_SIZE;
    const q = tick >= 0 ? Math.trunc(tick / span) : Math.trunc((tick - (span - 1)) / span);
    return q * span;
}

export function tickArrayPda(
    poolState: PublicKey,
    startIndex: number,
    clmmProgramId: PublicKey
) {
    return PublicKey.findProgramAddressSync(
        [
            Buffer.from("tick_array"),
            poolState.toBuffer(),
            Buffer.from(new Uint8Array(new BigInt64Array([BigInt(startIndex)]).buffer))
        ],
        clmmProgramId
    );
}

export function protocolPositionPda(
    poolState: PublicKey,
    lowerStart: number,
    upperStart: number,
    clmmProgramId: PublicKey
) {
    return PublicKey.findProgramAddressSync(
        [
            Buffer.from("position"),
            poolState.toBuffer(),
            Buffer.from(new Uint8Array(new BigInt64Array([BigInt(lowerStart)]).buffer)),
            Buffer.from(new Uint8Array(new BigInt64Array([BigInt(upperStart)]).buffer))
        ],
        clmmProgramId
    );
}