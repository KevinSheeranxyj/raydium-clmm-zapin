import * as anchor from "@coral-xyz/anchor";
import {Program, Wallet} from "@coral-xyz/anchor";
import * as web3 from "@solana/web3.js";
import {
    createAccount,
    createMint,
    getOrCreateAssociatedTokenAccount,
    mintTo,
    TOKEN_PROGRAM_ID
} from "@solana/spl-token";
import * as assert from "node:assert";
import {Keypair, PublicKey} from "@solana/web3.js";
import * as fs from "node:fs";
import * as path from "node:path";
import {fileURLToPath} from "node:url";



const program = anchor.workspace.dgSolanaPrograms as Program<DgSolanaPrograms>;

const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");


function loadKeypair(filePath: string): web3.Keypair {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const absolutePath = path.resolve(__dirname, filePath);
    const secretKey = Uint8Array.from(JSON.parse(fs.readFileSync(absolutePath, "utf8")));
    return web3.Keypair.fromSecretKey(secretKey);
}

let mint: PublicKey;
let userTokenAccount: PublicKey;
let recipientTokenAccount: PublicKey;
let transferDataPda: PublicKey;
let bump: number;
let admin = loadKeypair("keys/admin.json");;
let authority: Wallet;

async function prepare() {
    // Load keypairs from local files
    admin = loadKeypair("keys/admin.json");
    console.log("Admin PubKey:", admin.publicKey.toBase58());

    recipient = loadKeypair("keys/recipient.json");
    console.log("Recipient Pubkey:", recipient.publicKey.toBase58());
}

let recipient: Keypair;
const isoTimestamp = new Date().toISOString();
const transferId = "t" + isoTimestamp;
const amount = 1_000_000; // 1 token with 6 decimals

describe("dg-solana-programs", () => {

    before(async()=> {

        await prepare();

        let connection = new web3.Connection("https://falling-wiser-moon.solana-mainnet.quiknode.pro/653e836d3a2a94fb452fdc2a3796b420cb809b10", 'confirmed')
        const provider = new anchor.AnchorProvider(connection, new anchor.Wallet(admin), {
            commitment: 'confirmed'
        })
        anchor.setProvider(provider);
        authority = provider.wallet; // admin account

        // Check admin balance
        const balance = await provider.connection.getBalance(admin.publicKey);
        console.log(`Admin balance: ${balance / web3.LAMPORTS_PER_SOL} SOL`);
        if (balance < web3.LAMPORTS_PER_SOL * 0.01) {
            throw new Error("Insufficient balance in admin account. Please fund it with SOL.");
        }

        console.log("authority: ", authority.publicKey.toBase58());

        const adminAta = await getOrCreateAssociatedTokenAccount(
            provider.connection,
            authority.payer,      // payer for ATA creation if missing
            USDC_MINT,            // fixed USDC mint
            authority.publicKey   // owner (admin)
        );
        userTokenAccount = adminAta.address;
        console.log("Admin USDC ATA:", userTokenAccount.toBase58());

        const recipAta = await getOrCreateAssociatedTokenAccount(
            provider.connection,
            authority.payer,
            USDC_MINT,
            recipient.publicKey
        );
        recipientTokenAccount = recipAta.address;
        console.log("Recipient USDC ATA:", recipientTokenAccount.toBase58());
        // Find PDA
        [transferDataPda, bump] = await PublicKey.findProgramAddress(
            [Buffer.from("transfer_data")],
            program.programId
        );
        console.log("transferDataPDA: {}", transferDataPda.toBase58());

    });

    it("Executes the transfer", async () => {
        await program.methods
            .execute()
            .accounts({
                transferData: transferDataPda,
                user: authority.publicKey,
                userTokenAccount,
                recipientTokenAccount,
                tokenProgram: TOKEN_PROGRAM_ID,
            })
            .rpc();
    });

});