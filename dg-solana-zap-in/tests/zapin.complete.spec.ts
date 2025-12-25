import * as anchor from "@coral-xyz/anchor";
import { PublicKey, Keypair } from "@solana/web3.js";
import { Program } from "@coral-xyz/anchor";
import { ZapInClient, ZapInConfig, ZapInParams } from "./helpers/zapin";
import { createUserWithSol, createMintAndATA, getTokenAmount } from "./helpers/token";
import { operationDataPda } from "./helpers/pdas";
import { getOrCreateAssociatedTokenAccount } from "@solana/spl-token";
import * as fs from "fs";
import * as path from "path";

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "raydium.json"), "utf8")
);

describe("dg_solana_zapin :: Complete Zap-In Flow", () => {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin as Program<any>;
    let zapInClient: ZapInClient;
    let user: Keypair;

    before(async () => {
        // 创建测试用户
        user = await createUserWithSol(provider);
        console.log("Test user:", user.publicKey.toBase58());

        // 配置zap-in客户端
        const config: ZapInConfig = {
            program,
            provider,
            poolConfig: {
                clmmProgramId: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
                poolState: new PublicKey(raydiumConfig.POOL_STATE),
                ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
                observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
                tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
                tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
                tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
                tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
                tickSpacing: raydiumConfig.TICK_SPACING,
            }
        };

        zapInClient = new ZapInClient(config);
    });

    it("should execute complete zap-in flow", async () => {
        // 创建测试代币
        const { mint: testMint, ata: userAta } = await createMintAndATA(
            provider,
            user.publicKey,
            6, // decimals
            new anchor.BN(1000000) // 1M tokens
        );

        console.log("Created test mint:", testMint.toBase58());
        console.log("User ATA:", userAta.toBase58());

        // 定义zap-in参数
        const zapInParams: ZapInParams = {
            amountIn: new anchor.BN(100000), // 100k tokens
            tickLower: raydiumConfig.exampleTicks.tickLower,
            tickUpper: raydiumConfig.exampleTicks.tickUpper,
            slippageBps: 100, // 1% slippage
        };

        console.log("Zap-in parameters:", {
            amountIn: zapInParams.amountIn.toString(),
            tickLower: zapInParams.tickLower,
            tickUpper: zapInParams.tickUpper,
            slippageBps: zapInParams.slippageBps,
        });

        // 执行完整的zap-in流程
        const result = await zapInClient.executeZapIn(zapInParams, user);

        console.log("Zap-in completed successfully!");
        console.log("Transfer ID:", Buffer.from(result.transferId).toString('hex'));
        console.log("Transactions:", result.transactions);

        // 验证结果
        expect(result.transferId).toBeDefined();
        expect(result.transactions.length).toBeGreaterThan(0);
    }).timeout(300_000); // 5分钟超时

    it("should handle individual zap-in steps", async () => {
        const transferId = zapInClient.generateTransferId();
        console.log("Generated transfer ID:", Buffer.from(transferId).toString('hex'));

        // 创建测试代币
        const { mint: testMint, ata: userAta } = await createMintAndATA(
            provider,
            user.publicKey,
            6,
            new anchor.BN(1000000)
        );

        const zapInParams: ZapInParams = {
            amountIn: new anchor.BN(50000),
            tickLower: raydiumConfig.exampleTicks.tickLower,
            tickUpper: raydiumConfig.exampleTicks.tickUpper,
            slippageBps: 200, // 2% slippage
        };

        // 步骤1: 初始化
        await zapInClient.initialize();
        console.log("✓ Initialize completed");

        // 步骤2: 存入资金
        await zapInClient.deposit(transferId, zapInParams, user, userAta.address);
        console.log("✓ Deposit completed");

        // 步骤3: 准备执行
        const refundAta = await getOrCreateAssociatedTokenAccount(
            connection,
            user,
            testMint,
            user.publicKey
        );
        await zapInClient.prepareExecute(transferId, user, refundAta.address);
        console.log("✓ Prepare execute completed");

        // 步骤4: 执行swap
        await zapInClient.swapForBalance(transferId, user);
        console.log("✓ Swap for balance completed");

        // 步骤5: 打开仓位
        await zapInClient.openPosition(transferId, user);
        console.log("✓ Open position completed");

        // 步骤6: 增加流动性
        await zapInClient.increaseLiquidity(transferId, user);
        console.log("✓ Increase liquidity completed");

        // 步骤7: 完成执行
        await zapInClient.finalizeExecute(transferId, user);
        console.log("✓ Finalize execute completed");

        console.log("All individual steps completed successfully!");
    }).timeout(300_000);

    it("should handle cancellation", async () => {
        const transferId = zapInClient.generateTransferId();
        console.log("Generated transfer ID for cancellation:", Buffer.from(transferId).toString('hex'));

        // 创建测试代币
        const { mint: testMint, ata: userAta } = await createMintAndATA(
            provider,
            user.publicKey,
            6,
            new anchor.BN(1000000)
        );

        const zapInParams: ZapInParams = {
            amountIn: new anchor.BN(25000),
            tickLower: raydiumConfig.exampleTicks.tickLower,
            tickUpper: raydiumConfig.exampleTicks.tickUpper,
            slippageBps: 150, // 1.5% slippage
        };

        // 执行前几个步骤
        await zapInClient.initialize();
        await zapInClient.deposit(transferId, zapInParams, user, userAta.address);
        await zapInClient.prepareExecute(transferId, user, userAta.address);

        // 取消操作
        await zapInClient.cancel(transferId, user);
        console.log("✓ Cancel completed");

        console.log("Cancellation test completed successfully!");
    }).timeout(120_000);

    it("should validate operation data state", async () => {
        const transferId = zapInClient.generateTransferId();
        
        // 创建测试代币
        const { mint: testMint, ata: userAta } = await createMintAndATA(
            provider,
            user.publicKey,
            6,
            new anchor.BN(1000000)
        );

        const zapInParams: ZapInParams = {
            amountIn: new anchor.BN(75000),
            tickLower: raydiumConfig.exampleTicks.tickLower,
            tickUpper: raydiumConfig.exampleTicks.tickUpper,
            slippageBps: 50, // 0.5% slippage
        };

        // 执行deposit
        await zapInClient.deposit(transferId, zapInParams, user, userAta.address);

        // 验证操作数据状态
        const [operationDataPda] = operationDataPda(transferId, program.programId);
        const operationData = await program.account.operationData.fetch(operationDataPda);

        expect(operationData.initialized).toBe(true);
        expect(operationData.executed).toBe(false);
        expect(operationData.amount.toString()).toBe(zapInParams.amountIn.toString());
        expect(operationData.executor.equals(user.publicKey)).toBe(true);

        console.log("✓ Operation data validation passed");
        console.log("Operation data:", {
            initialized: operationData.initialized,
            executed: operationData.executed,
            amount: operationData.amount.toString(),
            executor: operationData.executor.toBase58(),
            stage: operationData.stage,
        });
    }).timeout(60_000);
});
