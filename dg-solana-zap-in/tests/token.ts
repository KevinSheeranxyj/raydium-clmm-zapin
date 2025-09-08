import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
    PublicKey, Keypair, SystemProgram, SYSVAR_RENT_PUBKEY
} from "@solana/web3.js";
import {
    airdrop, createMintAndATA, createRawTokenAccountOwnedBy, getTokenAmount
} from "./helpers/token";
import {
    operationDataPda, registryPda,
    positionNftMintPda, tickArrayStartIndex, tickArrayPda, protocolPositionPda
} from "./helpers/pdas";
import { encodeZapInParams, OperationType } from "./helpers/params";
import { getAssociatedTokenAddressSync } from "@solana/spl-token";

// 载入 Raydium 池 fixture
const RAYDIUM = require("../fixtures/raydium.json");

describe("dg_solana_zapin :: execute (refund path)", () => {
    const provider = anchor.AnchorProvider.env();
    anchor.setProvider(provider);

    const program = anchor.workspace.DgSolanaZapin as Program;

    // 用户（execute 的 payer & position NFT owner）
    const user = Keypair.generate();
    const transferId = `t-${Date.now()}`;

    // 我们用“本地自定义 mint”来进行 deposit → execute 的资金流，以触发退款
    let localMint: PublicKey;
    let userLocalATA: PublicKey;
    let programTokenAccount: PublicKey;

    before("fund user & prepare tokens", async () => {
        await airdrop(provider.connection, user.publicKey);

        // 1) 本地铸造 mint 给 user
        const { mint, ata } = await createMintAndATA(
            provider,
            user.publicKey,
            6,
            800_000n // 0.8
        );
        localMint = mint;
        userLocalATA = ata.address;

        // 2) 创建由 OperationData PDA 拥有的 program_token_account（用本地 mint）
        const [opPda] = operationDataPda(transferId, program.programId);
        programTokenAccount = await createRawTokenAccountOwnedBy(
            provider,
            localMint,
            opPda
        );

        // 3) 先调用 deposit，把 0.6 打到 program_token_account
        const [regPda] = registryPda(program.programId);
        const zapParamsBytes = encodeZapInParams(program, {
            amountIn: new anchor.BN(1_000_000), // 1.0 —— 故意 > 实际存入的 0.6，以触发退款
            pool: new PublicKey(RAYDIUM.POOL_STATE),
            tickLower: RAYDIUM.exampleTicks.tickLower,
            tickUpper: RAYDIUM.exampleTicks.tickUpper,
            slippageBps: 100
        });

        await program.methods
            .deposit(
                transferId,
                OperationType.ZapIn,
                zapParamsBytes as Buffer,
                new anchor.BN(600_000), // 0.6
                new PublicKey(RAYDIUM.TOKEN_MINT_0) // ca 指向池中的 mint 之一（仅作校验用）
            )
            .accounts({
                registry: regPda,
                operationData: opPda,
                authority: user.publicKey,
                authorityAta: userLocalATA,
                programTokenAccount,
                clmmProgram: new PublicKey(RAYDIUM.CLMM_PROGRAM_ID),

                poolState: new PublicKey(RAYDIUM.POOL_STATE),
                ammConfig: new PublicKey(RAYDIUM.AMM_CONFIG),
                observationState: new PublicKey(RAYDIUM.OBSERVATION_STATE),

                tokenVault0: new PublicKey(RAYDIUM.TOKEN_VAULT_0),
                tokenVault1: new PublicKey(RAYDIUM.TOKEN_VAULT_1),
                tokenMint0: new PublicKey(RAYDIUM.TOKEN_MINT_0),
                tokenMint1: new PublicKey(RAYDIUM.TOKEN_MINT_1),

                tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                systemProgram: SystemProgram.programId
            })
            .signers([user])
            .rpc();
    });

    it("should early-return and refund when deposited < params.amount_in", async () => {
        const [opPda] = operationDataPda(transferId, program.programId);
        const [regPda] = registryPda(program.programId);

        // ======== 组装 execute 所需 remaining_accounts ========
        const clmmProgramId = new PublicKey(RAYDIUM.CLMM_PROGRAM_ID);
        const poolStatePk   = new PublicKey(RAYDIUM.POOL_STATE);
        const ammCfgPk      = new PublicKey(RAYDIUM.AMM_CONFIG);
        const obsPk         = new PublicKey(RAYDIUM.OBSERVATION_STATE);
        const vault0Pk      = new PublicKey(RAYDIUM.TOKEN_VAULT_0);
        const vault1Pk      = new PublicKey(RAYDIUM.TOKEN_VAULT_1);
        const mint0Pk       = new PublicKey(RAYDIUM.TOKEN_MINT_0);
        const mint1Pk       = new PublicKey(RAYDIUM.TOKEN_MINT_1);

        const tickLower = RAYDIUM.exampleTicks.tickLower;
        const tickUpper = RAYDIUM.exampleTicks.tickUpper;
        const lowerStart = tickArrayStartIndex(tickLower, RAYDIUM.TICK_SPACING);
        const upperStart = tickArrayStartIndex(tickUpper, RAYDIUM.TICK_SPACING);

        // Raydium 派生 PDA（仅作地址一致性）
        const [taLower] = tickArrayPda(poolStatePk, lowerStart, clmmProgramId);
        const [taUpper] = tickArrayPda(poolStatePk, upperStart, clmmProgramId);
        const [protocolPos] = protocolPositionPda(poolStatePk, lowerStart, upperStart, clmmProgramId);

        // 本程序派生的 position NFT mint PDA & 用户 ATA（execute 会尝试创建/使用）
        const [posMintPda] = positionNftMintPda(user.publicKey, poolStatePk, program.programId);
        const posNftUserAta = getAssociatedTokenAddressSync(
            posMintPda, user.publicKey, false, anchor.utils.token.TOKEN_PROGRAM_ID
        );

        // 两个“PDA 持有”的输入/输出 token 账户（mint 必须是 mint0/mint1；这里只是提供空壳即可，因为我们走退款路径）
        const pdaInputTokenAcc  = await createRawTokenAccountOwnedBy(provider, mint0Pk, opPda);
        const pdaOutputTokenAcc = await createRawTokenAccountOwnedBy(provider, mint1Pk, opPda);

        // refund 的接收者（用户同 mint 的 ATA）—— 由于 program_token_account 使用 localMint，
        // 合约会按该 mint 找到用户 ATA 进行退款
        const userRefundAta = getAssociatedTokenAddressSync(localMint, user.publicKey);

        // ======== 执行前余额 ========
        const beforeUserRefundBal = await getTokenAmount(provider, userRefundAta);

        // ======== 调用 execute（把 0.6 原路退回） ========
        // 注意：execute 的大部分账户通过 remainingAccounts 传入，顺序不强制，但你的程序会按 key 查找/解包。
        const remainingAccounts = [
            // programs/sysvars/identities
            { pubkey: clmmProgramId, isWritable: false, isSigner: false },
            { pubkey: anchor.utils.token.TOKEN_PROGRAM_ID, isWritable: false, isSigner: false },
            { pubkey: new PublicKey("TokenzQd...11111111111111111111111111"), isWritable: false, isSigner: false }, // Token-2022（如无可填占位）
            { pubkey: new PublicKey("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"), isWritable: false, isSigner: false },
            { pubkey: SystemProgram.programId, isWritable: false, isSigner: false },
            { pubkey: SYSVAR_RENT_PUBKEY, isWritable: false, isSigner: false },
            { pubkey: user.publicKey, isWritable: true, isSigner: true },
            { pubkey: opPda, isWritable: true, isSigner: false },

            // pool/config/observation + vaults + mints
            { pubkey: poolStatePk, isWritable: true, isSigner: false },
            { pubkey: ammCfgPk, isWritable: false, isSigner: false },
            { pubkey: obsPk, isWritable: true, isSigner: false },
            { pubkey: vault0Pk, isWritable: true, isSigner: false },
            { pubkey: vault1Pk, isWritable: true, isSigner: false },
            { pubkey: mint0Pk, isWritable: false, isSigner: false },
            { pubkey: mint1Pk, isWritable: false, isSigner: false },

            // tick arrays & protocol position
            { pubkey: taLower, isWritable: true, isSigner: false },
            { pubkey: taUpper, isWritable: true, isSigner: false },
            { pubkey: protocolPos, isWritable: true, isSigner: false },

            // PDA input/output token accounts
            { pubkey: pdaInputTokenAcc, isWritable: true, isSigner: false },
            { pubkey: pdaOutputTokenAcc, isWritable: true, isSigner: false },

            // program_token_account（由 deposit 已创建并持有 0.6）
            { pubkey: programTokenAccount, isWritable: true, isSigner: false },

            // refund recipient user ATA（与 program_token_account 的 mint 相同）
            { pubkey: userRefundAta, isWritable: true, isSigner: false },

            // position NFT mint (PDA) & user ATA
            { pubkey: posMintPda, isWritable: true, isSigner: false },
            { pubkey: posNftUserAta, isWritable: true, isSigner: false },

            // associated_token_program（open_position_v2 会用到；即便我们走退款，前置索引查找仍会发生）
            { pubkey: new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"), isWritable: false, isSigner: false },
        ];

        await program.methods
            .execute(transferId)
            .accounts({
                operationData: opPda,
                registry: regPda,
                user: user.publicKey,

                memoProgram: new PublicKey("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
                clmmProgram: clmmProgramId,
                associatedTokenProgram: new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
                tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                tokenProgram2022: new PublicKey("TokenzQd...11111111111111111111111111"),
                systemProgram: SystemProgram.programId,
                rent: SYSVAR_RENT_PUBKEY
            })
            .remainingAccounts(remainingAccounts)
            .signers([user])
            .rpc();

        // ======== 断言：退款到账 ========
        const afterUserRefundBal = await getTokenAmount(provider, userRefundAta);
        if (afterUserRefundBal - beforeUserRefundBal !== 600_000) {
            throw new Error(`refund mismatch: got ${afterUserRefundBal - beforeUserRefundBal}, expect 600_000`);
        }

        // 执行状态位更新
        const od = await program.account.operationData.fetch(opPda);
        if (!od.executed) throw new Error("od.executed should be true");
    }).timeout(180_000);
});