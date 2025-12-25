import * as anchor from "@coral-xyz/anchor";
import { PublicKey, Keypair } from "@solana/web3.js";
import { Program } from "@coral-xyz/anchor";
import { ZapInClient, ZapInConfig, ZapInParams } from "./helpers/zapin";
import { createUserWithSol, createMintAndATA } from "./helpers/token";
import { operationDataPda } from "./helpers/pdas";
import { getOrCreateAssociatedTokenAccount } from "@solana/spl-token";
import * as fs from "fs";
import * as path from "path";

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "raydium.json"), "utf8")
);

describe("dg_solana_zapin :: Simple Zap-In Tests", () => {
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

    it("should initialize operation data", async () => {
        const operationDataPda = await zapInClient.initialize();
        console.log("Operation data PDA:", operationDataPda.toBase58());
        
        expect(operationDataPda).toBeDefined();
    }).timeout(30_000);

    it("should generate transfer ID", async () => {
        const transferId = zapInClient.generateTransferId();
        console.log("Generated transfer ID:", Buffer.from(transferId).toString('hex'));
        
        expect(transferId).toBeDefined();
        expect(transferId.length).toBe(32);
    });

    it("should create test tokens and user ATA", async () => {
        const { mint: testMint, ata: userAta } = await createMintAndATA(
            provider,
            user.publicKey,
            6, // decimals
            new anchor.BN(1000000) // 1M tokens
        );

        console.log("Created test mint:", testMint.toBase58());
        console.log("User ATA:", userAta.toBase58());

        expect(testMint).toBeDefined();
        expect(userAta).toBeDefined();
    }).timeout(30_000);

    it("should validate zap-in parameters", async () => {
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

        expect(zapInParams.amountIn.gt(new anchor.BN(0))).toBe(true);
        expect(zapInParams.tickLower).toBeLessThan(zapInParams.tickUpper);
        expect(zapInParams.slippageBps).toBeGreaterThan(0);
        expect(zapInParams.slippageBps).toBeLessThan(10000);
    });

    it("should test deposit step", async () => {
        const transferId = zapInClient.generateTransferId();
        console.log("Testing deposit with transfer ID:", Buffer.from(transferId).toString('hex'));

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

        try {
            // 先初始化
            await zapInClient.initialize();
            console.log("✓ Initialize completed");

            // 执行deposit
            const depositTx = await zapInClient.deposit(transferId, zapInParams, user, userAta.address);
            console.log("✓ Deposit completed:", depositTx);

            // 验证操作数据
            const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), program.programId);
            const operationData = await program.account.operationData.fetch(operationDataPda);

            expect(operationData.initialized).toBe(true);
            expect(operationData.executed).toBe(false);
            expect(operationData.amount.toString()).toBe(zapInParams.amountIn.toString());

            console.log("✓ Deposit validation passed");

        } catch (error) {
            console.error("Deposit test failed:", error);
            throw error;
        }
    }).timeout(60_000);
});
