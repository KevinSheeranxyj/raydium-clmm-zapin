import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import {
    airdrop, createMintAndATA, createRawTokenAccountOwnedBy, getTokenAmount
} from "./helpers/token";
import {
    operationDataPda, registryPda
} from "./helpers/pdas";
import { encodeZapInParams, OperationType } from "./helpers/params";

const RAYDIUM = require("./fixtures/raydium.json");

describe("dg_solana_zapin :: deposit", () => {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.DgSolanaZapin as Program;

    // 测试用户
    const authority = Keypair.generate();

    // 本地自定义 mint（仅用于从用户转入到 program_token_account）
    let userMint: PublicKey;
    let userATA: PublicKey;
    let programTokenAccount: PublicKey;

    const transferId = `t-${Date.now()}`;

    before("fund authority & prepare tokens", async () => {
        await airdrop(provider.connection, authority.publicKey);

        // 1) 本地铸造一个mint并给用户一些余额
        const { mint, ata } = await createMintAndATA(
            provider,
            authority.publicKey,
            6,
            1_000_000_000n // 1000.000000
        );
        userMint = mint;
        userATA = ata.address;

        // 2) 创建一个由 OperationData PDA(尚未生成) 作为 owner 的 token account
        const [opPda] = operationDataPda(transferId, program.programId);
        programTokenAccount = await createRawTokenAccountOwnedBy(
            provider,
            userMint,
            opPda
        );
    });

    it("should deposit tokens and persist operation_data (ZapIn)", async () => {
        // 预推导两个全局 PDA
        const [regPda] = registryPda(program.programId);
        const [opPda] = operationDataPda(transferId, program.programId);

        // 组装 ZapInParams（注意：这里仅存参数，不会在 deposit 时调用 Raydium）
        const zapParamsBytes = encodeZapInParams(program, {
            amountIn: new anchor.BN(1_000_000), // 1.0
            pool: new PublicKey(RAYDIUM.POOL_STATE),
            tickLower: RAYDIUM.exampleTicks.tickLower,
            tickUpper: RAYDIUM.exampleTicks.tickUpper,
            slippageBps: 100 // 1%
        });

        // 执行 deposit（把用户 token 从 userATA -> program_token_account）
        await program.methods
            .deposit(
                transferId,
                OperationType.ZapIn,      // or OperationType.Transfer
                zapParamsBytes as Buffer, // action
                new anchor.BN(500_000),   // amount: 0.5
                new PublicKey(RAYDIUM.TOKEN_MINT_0) // ca = 池里某个 mint
            )
            .accounts({
                registry: regPda,
                operationData: opPda,
                authority: authority.publicKey,
                authorityAta: userATA,
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
            .signers([authority])
            .rpc();

        // 校验 program_token_account 收到 0.5
        const ptaAmount = await getTokenAmount(provider, programTokenAccount);
        if (ptaAmount !== 500_000) {
            throw new Error(`program_token_account amount=${ptaAmount} != 500_000`);
        }

        // 读取 operation_data 账户并断言核心字段
        const od = await program.account.operationData.fetch(opPda);
        if (!od.initialized) throw new Error("operation_data not initialized");
        if (!od.transferId || od.transferId !== transferId) throw new Error("transfer_id mismatch");
        if (!od.amount.eq(new anchor.BN(500_000))) throw new Error("amount mismatch");
        if (od.executed) throw new Error("should not be executed yet");
        if (!od.operationType || Object.keys(od.operationType)[0] !== "zapIn") {
            throw new Error("operation_type should be ZapIn");
        }
    }).timeout(120_000);
});