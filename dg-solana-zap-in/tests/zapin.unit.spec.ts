import * as anchor from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { Program } from "@coral-xyz/anchor";
import { ZapInClient, ZapInConfig, ZapInParams } from "./helpers/zapin";
import { createUserWithSol, createMintAndATA } from "./helpers/token";
import { operationDataPda, registryPda } from "./helpers/pdas";
import { getOrCreateAssociatedTokenAccount } from "@solana/spl-token";
import * as fs from "fs";
import * as path from "path";

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "raydium.json"), "utf8")
);

describe("dg_solana_zapin :: Unit Tests", () => {
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

    describe("Initialize", () => {
        it("should initialize operation data PDA", async () => {
            const [operationDataPda] = operationDataPda("", program.programId);
            console.log("Operation data PDA:", operationDataPda.toBase58());

            const tx = await program.methods
                .initialize()
                .accounts({
                    operationData: operationDataPda,
                    authority: provider.wallet.publicKey,
                    setSolver: provider.wallet.publicKey, // 使用provider wallet作为setSolver
                    systemProgram: SystemProgram.programId,
                })
                .rpc();

            console.log("Initialize transaction:", tx);

            const od = await program.account.operationData.fetch(operationDataPda);
            expect(od.initialized).toBe(true);
            expect(od.authority.equals(provider.wallet.publicKey)).toBe(true);
        }).timeout(30_000);
    });

    describe("Deposit", () => {
        it("should deposit funds and parameters", async () => {
            const transferId = zapInClient.generateTransferId();
            console.log("Transfer ID:", Buffer.from(transferId).toString('hex'));

            // 创建测试代币
            const { mint: testMint, ata: userAta } = await createMintAndATA(
                provider,
                user.publicKey,
                6,
                new anchor.BN(1000000)
            );

            // 创建程序代币账户
            const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), program.programId);
            const programTokenAccount = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                testMint,
                operationDataPda,
                true
            );

            const zapInParams: ZapInParams = {
                amountIn: new anchor.BN(50000),
                tickLower: raydiumConfig.exampleTicks.tickLower,
                tickUpper: raydiumConfig.exampleTicks.tickUpper,
                slippageBps: 200,
            };

            const tx = await program.methods
                .deposit(
                    transferId,
                    { zapIn: {} }, // OperationType.ZapIn
                    Buffer.from([]), // action - 空的action，因为参数直接在deposit中传递
                    zapInParams.amountIn,
                    new PublicKey(raydiumConfig.POOL_STATE), // ca
                    user.publicKey // authorized_executor
                )
                .accounts({
                    registry: (await registryPda(program.programId))[0],
                    operationData: operationDataPda,
                    authority: user.publicKey,
                    authorityAta: userAta.address,
                    programTokenAccount: programTokenAccount.address,
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    systemProgram: SystemProgram.programId,
                })
                .signers([user])
                .rpc();

            console.log("Deposit transaction:", tx);

            // 验证操作数据
            const operationData = await program.account.operationData.fetch(operationDataPda);
            expect(operationData.initialized).toBe(true);
            expect(operationData.executed).toBe(false);
            expect(operationData.amount.toString()).toBe(zapInParams.amountIn.toString());
            expect(operationData.executor.equals(user.publicKey)).toBe(true);

        }).timeout(60_000);
    });

    describe("Prepare Execute", () => {
        it("should prepare execution step", async () => {
            const transferId = zapInClient.generateTransferId();
            
            // 先执行deposit
            const { mint: testMint, ata: userAta } = await createMintAndATA(
                provider,
                user.publicKey,
                6,
                new anchor.BN(1000000)
            );

            const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), program.programId);
            const programTokenAccount = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                testMint,
                operationDataPda,
                true
            );

            // Deposit
            await program.methods
                .deposit(
                    transferId,
                    { zapIn: {} },
                    Buffer.from([]),
                    new anchor.BN(50000),
                    new PublicKey(raydiumConfig.POOL_STATE),
                    user.publicKey
                )
                .accounts({
                    registry: (await registryPda(program.programId))[0],
                    operationData: operationDataPda,
                    authority: user.publicKey,
                    authorityAta: userAta.address,
                    programTokenAccount: programTokenAccount.address,
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    systemProgram: SystemProgram.programId,
                })
                .signers([user])
                .rpc();

            // 创建PDA代币账户
            const pdaToken0 = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                new PublicKey(raydiumConfig.TOKEN_MINT_0),
                operationDataPda,
                true
            );

            const pdaToken1 = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                new PublicKey(raydiumConfig.TOKEN_MINT_1),
                operationDataPda,
                true
            );

            // Prepare Execute
            const tx = await program.methods
                .prepareExecute(transferId)
                .accounts({
                    operationData: operationDataPda,
                    caller: user.publicKey,
                    programTokenAccount: programTokenAccount.address,
                    refundAta: userAta.address,
                    clmmProgram: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
                    poolState: new PublicKey(raydiumConfig.POOL_STATE),
                    ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
                    observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
                    tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
                    tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
                    tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
                    tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
                    pdaToken0: pdaToken0.address,
                    pdaToken1: pdaToken1.address,
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    systemProgram: SystemProgram.programId,
                    rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                })
                .signers([user])
                .rpc();

            console.log("Prepare execute transaction:", tx);

            // 验证操作数据状态
            const operationData = await program.account.operationData.fetch(operationDataPda);
            expect(operationData.stage).toBeDefined();

        }).timeout(60_000);
    });

    describe("Swap For Balance", () => {
        it("should execute swap for balance", async () => {
            const transferId = zapInClient.generateTransferId();
            
            // 先执行前面的步骤
            const { mint: testMint, ata: userAta } = await createMintAndATA(
                provider,
                user.publicKey,
                6,
                new anchor.BN(1000000)
            );

            const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), program.programId);
            const programTokenAccount = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                testMint,
                operationDataPda,
                true
            );

            // Deposit
            await program.methods
                .deposit(
                    transferId,
                    { zapIn: {} },
                    Buffer.from([]),
                    new anchor.BN(50000),
                    new PublicKey(raydiumConfig.POOL_STATE),
                    user.publicKey
                )
                .accounts({
                    registry: (await registryPda(program.programId))[0],
                    operationData: operationDataPda,
                    authority: user.publicKey,
                    authorityAta: userAta.address,
                    programTokenAccount: programTokenAccount.address,
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    systemProgram: SystemProgram.programId,
                })
                .signers([user])
                .rpc();

            // Prepare Execute
            const pdaToken0 = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                new PublicKey(raydiumConfig.TOKEN_MINT_0),
                operationDataPda,
                true
            );

            const pdaToken1 = await getOrCreateAssociatedTokenAccount(
                provider.connection,
                user,
                new PublicKey(raydiumConfig.TOKEN_MINT_1),
                operationDataPda,
                true
            );

            await program.methods
                .prepareExecute(transferId)
                .accounts({
                    operationData: operationDataPda,
                    caller: user.publicKey,
                    programTokenAccount: programTokenAccount.address,
                    refundAta: userAta.address,
                    clmmProgram: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
                    poolState: new PublicKey(raydiumConfig.POOL_STATE),
                    ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
                    observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
                    tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
                    tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
                    tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
                    tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
                    pdaToken0: pdaToken0.address,
                    pdaToken1: pdaToken1.address,
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    systemProgram: SystemProgram.programId,
                    rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                })
                .signers([user])
                .rpc();

            // Swap For Balance
            const tx = await program.methods
                .swapForBalance(transferId)
                .accounts({
                    operationData: operationDataPda,
                    user: user.publicKey,
                    clmmProgram: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
                    poolState: new PublicKey(raydiumConfig.POOL_STATE),
                    ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
                    observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
                    tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
                    tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
                    pdaToken0: pdaToken0.address,
                    pdaToken1: pdaToken1.address,
                    tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
                    tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    tokenProgram2022: anchor.utils.token.TOKEN_2022_PROGRAM_ID,
                    memoProgram: new PublicKey("MemoSq4gqABAX3b7cqRUpy4U5M1ZcvTd5Z73s3J6"),
                })
                .signers([user])
                .rpc();

            console.log("Swap for balance transaction:", tx);

        }).timeout(60_000);
    });

    describe("Error Handling", () => {
        it("should handle invalid transfer ID", async () => {
            const invalidTransferId = new Array(32).fill(0) as [number, 32];
            
            try {
                await program.methods
                    .prepareExecute(invalidTransferId)
                    .accounts({
                        operationData: (await operationDataPda(Buffer.from(invalidTransferId).toString('hex'), program.programId))[0],
                        caller: user.publicKey,
                        programTokenAccount: Keypair.generate().publicKey,
                        refundAta: Keypair.generate().publicKey,
                        clmmProgram: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
                        poolState: new PublicKey(raydiumConfig.POOL_STATE),
                        ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
                        observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
                        tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
                        tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
                        tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
                        tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
                        pdaToken0: Keypair.generate().publicKey,
                        pdaToken1: Keypair.generate().publicKey,
                        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                        systemProgram: SystemProgram.programId,
                        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                    })
                    .signers([user])
                    .rpc();

                expect.fail("Should have thrown an error");
            } catch (error) {
                expect(error.message).toContain("NotInitialized");
            }
        }).timeout(30_000);

        it("should handle unauthorized caller", async () => {
            const transferId = zapInClient.generateTransferId();
            const unauthorizedUser = Keypair.generate();
            
            try {
                await program.methods
                    .prepareExecute(transferId)
                    .accounts({
                        operationData: (await operationDataPda(Buffer.from(transferId).toString('hex'), program.programId))[0],
                        caller: unauthorizedUser.publicKey,
                        programTokenAccount: Keypair.generate().publicKey,
                        refundAta: Keypair.generate().publicKey,
                        clmmProgram: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
                        poolState: new PublicKey(raydiumConfig.POOL_STATE),
                        ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
                        observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
                        tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
                        tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
                        tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
                        tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
                        pdaToken0: Keypair.generate().publicKey,
                        pdaToken1: Keypair.generate().publicKey,
                        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                        systemProgram: SystemProgram.programId,
                        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                    })
                    .signers([unauthorizedUser])
                    .rpc();

                expect.fail("Should have thrown an error");
            } catch (error) {
                expect(error.message).toContain("Unauthorized");
            }
        }).timeout(30_000);
    });
});
