import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, getAssociatedTokenAddress, ASSOCIATED_TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { assert } from "chai";
import * as web3 from "@solana/web3.js";
import path from "node:path";
import {fileURLToPath} from "node:url";
import fs from "node:fs";
import BN from "bn.js";
let admin = loadKeypair("keys/admin.json");

function loadKeypair(filePath: string): web3.Keypair {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const absolutePath = path.resolve(__dirname, filePath);
    const secretKey = Uint8Array.from(JSON.parse(fs.readFileSync(absolutePath, "utf8")));
    return web3.Keypair.fromSecretKey(secretKey);
}
describe("dg_solana_programs - Execute Instruction", () => {
    // Configure the client to use the mainnet cluster
    let connection = new web3.Connection("https://warmhearted-delicate-uranium.solana-mainnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be", 'confirmed')
    const provider = new anchor.AnchorProvider(connection, new anchor.Wallet(admin), {
        commitment: 'confirmed'
    })
    anchor.setProvider(provider);
    const program = anchor.workspace.DgSolanaPrograms as Program<DgSolanaPrograms>;

    // Constants for public keys
    const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"); // Circle official's mainnet pub key
    const USER_PUBKEY = new PublicKey("6vCoLQjHBsXCE8Y1K4krFhtGjjCD6bGHj7AYhu97SSUC"); // Test user pub key
    const RECIPIENT_PUBKEY = new PublicKey("DY9taff8ydRpFMfbC3woPFLwA6t1VtMGzjmpr5LjvwW4");

    // Generate a keypair for the authority (for initialization)
    const authority = admin;
    let transferDataPda: PublicKey;
    let bump: number;

    // Test data
    const transferId = "test-transfer-123";
    const amount = new BN(1_000_000); // 1 USDC (assuming 6 decimals)

    // Helper to find PDA
    const getTransferDataPda = async () => {
        const [pda, pdaBump] = await PublicKey.findProgramAddress(
            [Buffer.from("transfer_data")],
            program.programId
        );
        return { pda, bump: pdaBump };
    };

    before(async () => {

        // Find PDA
        const { pda, bump: pdaBump } = await getTransferDataPda();
        console.log("PDA: ", pda.toBase58())
        transferDataPda = pda;
        bump = pdaBump;

        // Initialize the PDA
        await program.methods
            .initialize()
            .accounts({
                transferData: transferDataPda,
                authority: authority.publicKey,
                systemProgram: SystemProgram.programId,
            })
            .signers([authority])
            .rpc();

        // Deposit transfer details
        await program.methods
            .deposit(transferId, amount, RECIPIENT_PUBKEY)
            .accounts({
                transferData: transferDataPda,
                authority: authority.publicKey,
            })
            .signers([admin])
            .rpc();
    });

    it("Executes a token transfer successfully", async () => {
        // Get associated token accounts
        const userTokenAccount = await getAssociatedTokenAddress(USDC_MINT, USER_PUBKEY);
        const recipientTokenAccount = await getAssociatedTokenAddress(USDC_MINT, RECIPIENT_PUBKEY);

        // Ensure token accounts exist (in a real mainnet test, these must be pre-created)
        // For testing, you may need to create these accounts if they don't exist
        const userTokenAccountInfo = await provider.connection.getAccountInfo(userTokenAccount);
        const recipientTokenAccountInfo = await provider.connection.getAccountInfo(recipientTokenAccount);
        console.log("recipientTokenAccount: ", recipientTokenAccountInfo);
        if (!userTokenAccountInfo || !recipientTokenAccountInfo) {
            throw new Error("User or recipient token account does not exist. Ensure accounts are created and funded.");
        }

        // Execute the transfer
        await program.methods
            .execute()
            .accounts({
                transferData: transferDataPda,
                user: USER_PUBKEY,
                userTokenAccount,
                recipientTokenAccount,
                usdcMint: USDC_MINT,
                tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([authority]) // USER_PUBKEY must sign, but in test env, provider wallet may differ
            .rpc();

        // Verify transfer data
        const transferDataAccount = await program.account.transferData.fetch(transferDataPda);
        assert.isTrue(transferDataAccount.executed, "Transfer should be marked as executed");
        assert.equal(transferDataAccount.transferId, transferId, "Transfer ID should match");
        assert.equal(transferDataAccount.amount.toNumber(), amount.toNumber(), "Amount should match");
        assert.equal(transferDataAccount.recipient.toBase58(), RECIPIENT_PUBKEY.toBase58(), "Recipient should match");

        // Verify token transfer (check recipient balance increased)
        const recipientBalance = await provider.connection.getTokenAccountBalance(recipientTokenAccount);
        assert.isTrue(recipientBalance.value.amount >= amount.toString(), "Recipient balance should increase by amount");
    });

    it("Fails to execute already executed transfer", async () => {
        // Try executing the same transfer again
        const userTokenAccount = await getAssociatedTokenAddress(USDC_MINT, USER_PUBKEY);
        const recipientTokenAccount = await getAssociatedTokenAddress(USDC_MINT, RECIPIENT_PUBKEY);

        try {
            await program.methods
                .execute()
                .accounts({
                    transferData: transferDataPda,
                    user: USER_PUBKEY,
                    userTokenAccount,
                    recipientTokenAccount,
                    usdcMint: USDC_MINT,
                    tokenProgram: TOKEN_PROGRAM_ID,
                })
                .signers([])
                .rpc();
            assert.fail("Should have failed with AlreadyExecuted error");
        } catch (err) {
            assert.include(err.toString(), "AlreadyExecuted", "Should fail with AlreadyExecuted error");
        }
    });

    it("Fails to execute with incorrect recipient token account", async () => {
        // Create a new PDA for a fresh transfer
        const newAuthority = Keypair.generate();
        const { pda: newTransferDataPda } = await getTransferDataPda();

        // Initialize and deposit for a new transfer
        await program.methods
            .initialize()
            .accounts({
                transferData: newTransferDataPda,
                authority: newAuthority.publicKey,
                systemProgram: SystemProgram.programId,
            })
            .signers([newAuthority])
            .rpc();

        await program.methods
            .deposit(transferId, amount, RECIPIENT_PUBKEY)
                .accounts({
                    transferData: newTransferDataPda,
                    authority: newAuthority.publicKey,
                })
                .signers([newAuthority])
                .rpc();

        // Use a different recipient pubkey to simulate incorrect recipient
        const wrongRecipient = Keypair.generate().publicKey;
        const wrongRecipientTokenAccount = await getAssociatedTokenAddress(USDC_MINT, wrongRecipient);
        const userTokenAccount = await getAssociatedTokenAddress(USDC_MINT, USER_PUBKEY);

        try {
            await program.methods
                .execute()
                .accounts({
                    transferData: newTransferDataPda,
                    user: USER_PUBKEY,
                    userTokenAccount,
                    recipientTokenAccount: wrongRecipientTokenAccount,
                    usdcMint: USDC_MINT,
                    tokenProgram: TOKEN_PROGRAM_ID,
                })
                .signers([])
                .rpc();
            assert.fail("Should have failed with Unauthorized error");
        } catch (err) {
            assert.include(err.toString(), "Unauthorized", "Should fail with Unauthorized error");
        }
    });
});