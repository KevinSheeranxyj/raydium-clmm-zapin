import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { assert } from "chai";
import * as web3 from "@solana/web3.js";
import path from "node:path";
import {fileURLToPath} from "node:url";
import fs from "node:fs";
import BN from "bn.js";

function loadKeypair(filePath: string): web3.Keypair {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const absolutePath = path.resolve(__dirname, filePath);
    const secretKey = Uint8Array.from(JSON.parse(fs.readFileSync(absolutePath, "utf8")));
    return web3.Keypair.fromSecretKey(secretKey);
}
let admin = loadKeypair("keys/admin.json");

describe("dg_solana_programs - Deposit Instruction", () => {
    // Configure the client to use the mainnet cluster
    let connection = new web3.Connection("https://warmhearted-delicate-uranium.solana-mainnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be", 'confirmed')
    const provider = new anchor.AnchorProvider(connection, new anchor.Wallet(admin), {
        commitment: 'confirmed'
    })
    anchor.setProvider(provider);
    const program = anchor.workspace.DgSolanaPrograms as Program<DgSolanaPrograms>;

    // Constants for public keys
    const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    const RECIPIENT_PUBKEY = new PublicKey("DY9taff8ydRpFMfbC3woPFLwA6t1VtMGzjmpr5LjvwW4");

    // Generate a keypair for the authority
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
        // Set provider with authority wallet
        const customProvider = new anchor.AnchorProvider(
            provider.connection,
            new anchor.Wallet(authority),
            { commitment: "confirmed" }
        );
        anchor.setProvider(customProvider);

        // Airdrop SOL to authority (for devnet/testnet; for mainnet, ensure account is funded)
        try {
            const signature = await provider.connection.requestAirdrop(
                authority.publicKey,
                2 * anchor.web3.LAMPORTS_PER_SOL
            );
            await provider.connection.confirmTransaction(signature);
        } catch (err) {
            console.warn("Airdrop failed; ensure authority account is funded on mainnet:", err);
        }

        // Find PDA
        const { pda, bump: pdaBump } = await getTransferDataPda();
        transferDataPda = pda;
        bump = pdaBump;
        console.log("Transfer Data PDA:", transferDataPda.toBase58());

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

        // Verify initialization
        const transferDataAccount = await program.account.transferData.fetch(transferDataPda);
        assert.equal(
            transferDataAccount.authority.toBase58(),
            authority.publicKey.toBase58(),
            "Authority mismatch after initialization"
        );
        assert.isTrue(transferDataAccount.initialized, "PDA should be initialized");
    });

    it("Deposits transfer details successfully", async () => {
        // Execute deposit
        await program.methods
            .deposit(transferId, amount, RECIPIENT_PUBKEY)
            .accounts({
                transferData: transferDataPda,
                authority: authority.publicKey,
            })
            .signers([authority])
            .rpc();

        // Verify transfer data
        const transferDataAccount = await program.account.transferData.fetch(transferDataPda);
        assert.equal(transferDataAccount.transferId, transferId, "Transfer ID should match");
        assert.equal(transferDataAccount.amount.toNumber(), amount.toNumber(), "Amount should match");
        assert.equal(
            transferDataAccount.recipient.toBase58(),
            RECIPIENT_PUBKEY.toBase58(),
            "Recipient should match"
        );
        assert.isFalse(transferDataAccount.executed, "Transfer should not be executed");
        assert.equal(
            transferDataAccount.authority.toBase58(),
            authority.publicKey.toBase58(),
            "Authority should remain unchanged"
        );
    });

    it("Fails to deposit with unauthorized authority", async () => {
        // Create a new keypair to simulate an unauthorized authority
        const unauthorizedAuthority = Keypair.generate();

        try {
            await program.methods
                .deposit(transferId + "-2", amount, RECIPIENT_PUBKEY)
                .accounts({
                    transferData: transferDataPda,
                    authority: unauthorizedAuthority.publicKey,
                })
                .signers([unauthorizedAuthority])
                .rpc();
            assert.fail("Should have failed with Unauthorized error");
        } catch (err) {
            assert.include(err.toString(), "Unauthorized", "Should fail with Unauthorized error");
        }
    });

    it("Fails to deposit with invalid transfer ID (empty)", async () => {
        try {
            await program.methods
                .deposit("", amount, RECIPIENT_PUBKEY)
                .accounts({
                    transferData: transferDataPda,
                    authority: authority.publicKey,
                })
                .signers([authority])
                .rpc();
            assert.fail("Should have failed with InvalidTransferId error");
        } catch (err) {
            assert.include(err.toString(), "InvalidTransferId", "Should fail with InvalidTransferId error");
        }
    });

    it("Fails to deposit with invalid amount (zero)", async () => {
        try {
            await program.methods
                .deposit(transferId + "-3", new BN(0), RECIPIENT_PUBKEY)
                .accounts({
                    transferData: transferDataPda,
                    authority: authority.publicKey,
                })
                .signers([authority])
                .rpc();
            assert.fail("Should have failed with InvalidAmount error");
        } catch (err) {
            assert.include(err.toString(), "InvalidAmount", "Should fail with InvalidAmount error");
        }
    });

    it("Fails to deposit with oversized transfer ID", async () => {
        // Create a transfer ID longer than MAX_TRANSFER_ID_LEN (32 bytes)
        const oversizedTransferId = "a".repeat(33);

        try {
            await program.methods
                .deposit(oversizedTransferId, amount, RECIPIENT_PUBKEY)
                .accounts({
                    transferData: transferDataPda,
                    authority: authority.publicKey,
                })
                .signers([authority])
                .rpc();
            assert.fail("Should have failed with InvalidTransferId error");
        } catch (err) {
            assert.include(err.toString(), "InvalidTransferId", "Should fail with InvalidTransferId egressor");
        }
    });
});