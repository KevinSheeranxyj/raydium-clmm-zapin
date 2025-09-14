const anchor = require("@coral-xyz/anchor");
const { PublicKey, Keypair, SystemProgram } = require("@solana/web3.js");
const fs = require("fs");
const path = require("path");

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "raydium.json"), "utf8")
);

describe("dg_solana_zapin :: Initialize Test", () => {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin;

    describe("Initialize", () => {
        it("should initialize operation data PDA", async () => {
            // 使用简单的PDA生成
            const [operationDataPda] = PublicKey.findProgramAddressSync(
                [Buffer.from("operation_data")],
                program.programId
            );
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
            
            console.log("✓ Initialize test passed");
        }).timeout(30_000);
    });
});
