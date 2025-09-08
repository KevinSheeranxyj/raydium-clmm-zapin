import * as anchor from "@coral-xyz/anchor";
import { PublicKey } from "@solana/web3.js";

/**
 * 使用 Anchor coder 把 Rust 的 AnchorSerialize/Deserialize 类型编码为 bytes
 * 需要你的 IDL 中有对应 `ZapInParams` / `TransferParams` / `OperationType` 类型定义
 */
export function encodeZapInParams(
    program: anchor.Program,
    params: { amountIn: anchor.BN, pool: PublicKey, tickLower: number, tickUpper: number, slippageBps: number }
) {
    return program.coder.types.encode("ZapInParams", {
        amountIn: params.amountIn,
        pool: params.pool,
        tickLower: params.tickLower,
        tickUpper: params.tickUpper,
        slippageBps: params.slippageBps
    });
}

export function encodeTransferParams(
    program: anchor.Program,
    params: { amount: anchor.BN, recipient: PublicKey }
) {
    return program.coder.types.encode("TransferParams", {
        amount: params.amount,
        recipient: params.recipient
    });
}

// OperationType 枚举（Anchor 的 enum 为 Rust tagged union）
export const OperationType = {
    Transfer: { transfer: {} },
    ZapIn: { zapIn: {} }
} as const;