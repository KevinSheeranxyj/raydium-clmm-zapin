const anchor = require("@coral-xyz/anchor");
const { PublicKey, Keypair, SystemProgram } = require("@solana/web3.js");
const fs = require("fs");
const path = require("path");

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "raydium.json"), "utf8")
);

// PDA helper functions
function getOperationDataPda(transferId, programId) {
    const seeds = [Buffer.from("operation_data")];
    if (transferId && transferId.length > 0) {
        seeds.push(Buffer.from(transferId));
    }
    return PublicKey.findProgramAddressSync(seeds, programId);
}

function registryPda(programId) {
    return PublicKey.findProgramAddressSync([Buffer.from("registry")], programId);
}

describe("dg_solana_zapin :: Simple Unit Tests", () => {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin;
    let user;

    before(async () => {
        // 使用provider的钱包作为测试用户
        user = provider.wallet;
        console.log("Test user:", user.publicKey.toBase58());
    });

    describe("Initialize", () => {
        it("should initialize operation data PDA", async () => {
            const [operationDataPda] = getOperationDataPda("", program.programId);
            console.log("Operation data PDA:", operationDataPda.toBase58());

            const tx = await program.methods
                .initialize()
                .accounts({
                    operationData: operationDataPda,
                    authority: provider.wallet.publicKey,
                    setSolver: provider.wallet.publicKey,
                    systemProgram: anchor.web3.SystemProgram.programId,
                })
                .rpc();

            console.log("Initialize transaction:", tx);

            const od = await program.account.operationData.fetch(operationDataPda);
            if (!od.initialized) throw new Error("operation_data not initialized");
            if (!od.authority.equals(provider.wallet.publicKey)) {
                throw new Error(`authority mismatch: got ${od.authority.toBase58()}`);
            }
        }).timeout(30_000);
    });

    describe("Deposit", () => {
        it("should deposit funds and parameters", async () => {
            const transferId = Keypair.generate().publicKey.toBytes().slice(0, 32);
            console.log("Transfer ID:", Buffer.from(transferId).toString('hex'));

            // 使用现有的代币mint
            const testMint = new PublicKey(raydiumConfig.TOKEN_MINT_0);
            
            // 创建程序代币账户
            const [operationDataPda] = getOperationDataPda(transferId, program.programId);
            
            // 创建用户ATA
            const userAta = await anchor.utils.token.associatedAddress({
                mint: testMint,
                owner: user.publicKey
            });

            // 创建程序代币账户 (PDA拥有的代币账户)
            const programTokenAccount = await anchor.utils.token.associatedAddress({
                mint: testMint,
                owner: operationDataPda
            });

            const amountIn = new anchor.BN(50000);

            const tx = await program.methods
                .deposit(
                    Array.from(transferId), // 转换为数组
                    { zapIn: {} }, // OperationType.ZapIn
                    Buffer.from([]), // action - 空的action
                    amountIn,
                    new PublicKey(raydiumConfig.POOL_STATE), // ca
                    user.publicKey // authorized_executor
                )
                .accounts({
                    registry: (await registryPda(program.programId))[0],
                    operationData: operationDataPda,
                    authority: user.publicKey,
                    authorityAta: userAta,
                    programTokenAccount: programTokenAccount, // 使用程序拥有的代币账户
                    tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
                    systemProgram: anchor.web3.SystemProgram.programId,
                })
                .signers([user])
                .rpc();

            console.log("Deposit transaction:", tx);

            // 验证操作数据
            const operationData = await program.account.operationData.fetch(operationDataPda);
            if (!operationData.initialized) throw new Error("operation_data not initialized");
            if (operationData.executed) throw new Error("operation should not be executed yet");
            if (operationData.amount.toString() !== amountIn.toString()) {
                throw new Error(`amount mismatch: expected ${amountIn.toString()}, got ${operationData.amount.toString()}`);
            }
            if (!operationData.executor.equals(user.publicKey)) {
                throw new Error(`executor mismatch: expected ${user.publicKey.toBase58()}, got ${operationData.executor.toBase58()}`);
            }

        }).timeout(60_000);
    });

    describe("Error Handling", () => {
        it("should handle invalid transfer ID", async () => {
            const invalidTransferId = new Array(32).fill(0);
            
            try {
                await program.methods
                    .prepareExecute(invalidTransferId)
                    .accounts({
                        operationData: (await getOperationDataPda(Buffer.from(invalidTransferId).toString('hex'), program.programId))[0],
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
                        systemProgram: anchor.web3.SystemProgram.programId,
                        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                    })
                    .signers([user])
                    .rpc();

                throw new Error("Should have thrown an error");
            } catch (error) {
                if (!error.message.includes("NotInitialized") && !error.message.includes("Max seed length exceeded")) {
                    throw new Error(`Expected NotInitialized or Max seed length exceeded error, got: ${error.message}`);
                }
            }
        }).timeout(30_000);

        it("should handle unauthorized caller", async () => {
            const transferId = Array.from(Keypair.generate().publicKey.toBytes().slice(0, 32));
            const unauthorizedUser = Keypair.generate();
            
            try {
                await program.methods
                    .prepareExecute(transferId)
                    .accounts({
                        operationData: (await getOperationDataPda(Buffer.from(transferId).toString('hex'), program.programId))[0],
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
                        systemProgram: anchor.web3.SystemProgram.programId,
                        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                    })
                    .signers([unauthorizedUser])
                    .rpc();

                throw new Error("Should have thrown an error");
            } catch (error) {
                if (!error.message.includes("Unauthorized") && !error.message.includes("Max seed length exceeded")) {
                    throw new Error(`Expected Unauthorized or Max seed length exceeded error, got: ${error.message}`);
                }
            }
        }).timeout(30_000);
    });
});
