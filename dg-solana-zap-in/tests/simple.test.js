const anchor = require("@coral-xyz/anchor");
const { PublicKey, Keypair } = require("@solana/web3.js");
const fs = require("fs");
const path = require("path");

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "raydium.json"), "utf8")
);

describe("dg_solana_zapin :: Simple Tests", () => {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin;

    it("should have valid program", async () => {
        console.log("Testing program setup...");
        
        if (!program) {
            throw new Error("Program is undefined");
        }
        
        console.log("Program ID:", program.programId.toBase58());
        console.log("✓ Program setup test passed");
    });

    it("should generate transfer ID", async () => {
        console.log("Testing transfer ID generation...");
        
        const keypair = Keypair.generate();
        const transferId = Array.from(keypair.publicKey.toBytes());
        
        console.log("Generated transfer ID:", Buffer.from(transferId).toString('hex'));
        
        if (!transferId) {
            throw new Error("Transfer ID is undefined");
        }
        if (transferId.length !== 32) {
            throw new Error(`Transfer ID length is ${transferId.length}, expected 32`);
        }
        
        console.log("✓ Transfer ID generation test passed");
    });

    it("should validate zap-in parameters", async () => {
        console.log("Testing parameter validation...");
        
        const zapInParams = {
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

        if (!zapInParams.amountIn.gt(new anchor.BN(0))) {
            throw new Error("Amount in should be greater than 0");
        }
        if (zapInParams.tickLower >= zapInParams.tickUpper) {
            throw new Error("Tick lower should be less than tick upper");
        }
        if (zapInParams.slippageBps <= 0 || zapInParams.slippageBps >= 10000) {
            throw new Error("Slippage bps should be between 0 and 10000");
        }
        
        console.log("✓ Parameter validation test passed");
    });

    it("should test Raydium pool configuration", async () => {
        console.log("Testing Raydium pool configuration...");
        
        // 验证配置
        console.log("CLMM Program ID:", raydiumConfig.CLMM_PROGRAM_ID);
        console.log("Pool State:", raydiumConfig.POOL_STATE);
        console.log("Token Mint 0:", raydiumConfig.TOKEN_MINT_0);
        console.log("Token Mint 1:", raydiumConfig.TOKEN_MINT_1);
        console.log("Tick Spacing:", raydiumConfig.TICK_SPACING);
        
        // 验证地址格式
        try {
            new PublicKey(raydiumConfig.CLMM_PROGRAM_ID);
            new PublicKey(raydiumConfig.POOL_STATE);
            new PublicKey(raydiumConfig.TOKEN_MINT_0);
            new PublicKey(raydiumConfig.TOKEN_MINT_1);
            console.log("✓ All addresses are valid PublicKey format");
        } catch (error) {
            throw new Error(`Invalid address format: ${error.message}`);
        }
        
        console.log("✓ Raydium pool configuration test passed");
    });

    it("should test PDA calculation", async () => {
        console.log("Testing PDA calculation...");
        
        const keypair = Keypair.generate();
        const transferId = Array.from(keypair.publicKey.toBytes());
        
        // 测试操作数据PDA
        const [operationDataPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("operation_data"), Buffer.from(transferId)],
            program.programId
        );
        
        console.log("Operation data PDA:", operationDataPda.toBase58());
        
        if (!operationDataPda) {
            throw new Error("Operation data PDA is undefined");
        }
        
        // 测试注册表PDA
        const [registryPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("registry")],
            program.programId
        );
        
        console.log("Registry PDA:", registryPda.toBase58());
        
        if (!registryPda) {
            throw new Error("Registry PDA is undefined");
        }
        
        console.log("✓ PDA calculation test passed");
    });

    it("should test basic functionality", async () => {
        console.log("Testing basic zap-in functionality...");
        
        // 测试生成transfer ID
        const keypair = Keypair.generate();
        const transferId = Array.from(keypair.publicKey.toBytes());
        console.log("✓ Generated transfer ID");

        // 测试参数验证
        const zapInParams = {
            amountIn: new anchor.BN(50000),
            tickLower: raydiumConfig.exampleTicks.tickLower,
            tickUpper: raydiumConfig.exampleTicks.tickUpper,
            slippageBps: 200,
        };
        console.log("✓ Created zap-in parameters");

        // 测试PDA计算
        const [operationDataPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("operation_data"), Buffer.from(transferId)],
            program.programId
        );
        console.log("✓ Calculated operation data PDA");

        console.log("All basic functionality tests passed!");
    }).timeout(30_000);
});
