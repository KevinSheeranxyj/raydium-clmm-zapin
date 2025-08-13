import { getOrCreateAssociatedTokenAccount} from "@solana/spl-token";
import {Keypair, PublicKey} from "@solana/web3.js";
import * as web3 from "@solana/web3.js";
import * as anchor from "@coral-xyz/anchor";
import {Program, Wallet} from "@coral-xyz/anchor";
import path from "node:path";
import { fileURLToPath } from 'node:url';
import fs from "node:fs";

const program = anchor.workspace.dgSolanaPrograms as Program<DgSolanaPrograms>;

const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

let mint: PublicKey;
let userTokenAccount: PublicKey;
let recipientTokenAccount: PublicKey;
let transferDataPda: PublicKey;
let admin : Keypair;
let recipient: Keypair;
let authority: anchor.Wallet;
let bump: number;

const transferId = "t1219282211";
const amount = 1_000_000; // 1 token with 6 decimals


function loadKeypair(filePath: string): web3.Keypair {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const absolutePath = path.resolve(__dirname, filePath);
    const secretKey = Uint8Array.from(JSON.parse(fs.readFileSync(absolutePath, "utf8")));
    return web3.Keypair.fromSecretKey(secretKey);
}

async function prepare() {
    // Load keypairs from local files
    admin = loadKeypair("keys/admin.json");
    console.log("Admin PubKey:", admin.publicKey.toBase58());

    recipient = loadKeypair("keys/recipient.json");
    console.log("Recipient Pubkey:", recipient.publicKey.toBase58());
}


let connection;
describe("dg-solana-programs", () => {
    before(async () => {
        await prepare();

        // https://falling-wiser-moon.solana-mainnet.quiknode.pro/653e836d3a2a94fb452fdc2a3796b420cb809b10
        // https://warmhearted-delicate-uranium.solana-mainnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be
        // https://mainnet.helius-rpc.com/?api-key=76647368-1e0d-405b-b0aa-b5e2d006b58d
        connection = new web3.Connection("https://warmhearted-delicate-uranium.solana-mainnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be", 'confirmed')
        try {
            console.log("Checking RPC health...");
            const blockhash = await connection.getLatestBlockhash();
            console.log("Latest Blockhash:", blockhash);
        } catch (error) {
            console.error("RPC health check failed:", error);
            throw new Error("Cannot connect to RPC endpoint");
        }

        const provider = new anchor.AnchorProvider(connection, new anchor.Wallet(admin), {
            commitment: 'confirmed'
        })
        anchor.setProvider(provider);
        authority = provider.wallet;

        console.log("ProgramID: ", program.programId.toBase58());

        // Check admin balance
        const balance = await provider.connection.getBalance(admin.publicKey);
        console.log(`Admin balance: ${balance / web3.LAMPORTS_PER_SOL} SOL`);
        if (balance < web3.LAMPORTS_PER_SOL * 0.01) {
            throw new Error("Insufficient balance in admin account. Please fund it with SOL.");
        }
        console.log("authority: ", authority.publicKey.toBase58());
        // Use existing USDC mint
        mint = USDC_MINT;
        console.log("Using USDC mint:", mint.toBase58());
        // Create/fetch ATAs for admin & recipient
        const adminAta = await getOrCreateAssociatedTokenAccount(
            provider.connection,
            admin,                 // payer
            mint,                  // USDC mint
            admin.publicKey        // owner
        );
        userTokenAccount = adminAta.address;
        console.log("Admin USDC ATA:", userTokenAccount.toBase58());

        const recipientAta = await getOrCreateAssociatedTokenAccount(
            provider.connection,
            admin,                 // payer for creation fees
            mint,
            recipient.publicKey
        );
        recipientTokenAccount = recipientAta.address;
        console.log("Recipient USDC ATA:", recipientTokenAccount.toBase58());

        // Find PDA
        [transferDataPda, bump] = await PublicKey.findProgramAddress(
            [Buffer.from("transfer_data")],
            program.programId
        );
        console.log("transferDataPDA: ", transferDataPda.toBase58());
    });


    it("Initializes the PDA", async () => {
        const tx = await program.methods
            .initialize()
            .accounts({
                transferData: transferDataPda,
                authority: authority.publicKey,
            })
            .rpc()
            .catch((e) => {console.log(e)});

        await connection.confirmTransaction(tx, 'confirmed')

        const transferData = await program.account.transferData.fetch(transferDataPda);
        console.log("transferData: ", transferData);
    });


});

