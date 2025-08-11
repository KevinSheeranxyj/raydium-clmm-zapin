import * as anchor from "@coral-xyz/anchor";
import {Program, Wallet} from "@coral-xyz/anchor";
import * as web3 from "@solana/web3.js";
import {
    createAccount,
    createMint, createTransferInstruction,
    getOrCreateAssociatedTokenAccount,
    mintTo,
    TOKEN_PROGRAM_ID
} from "@solana/spl-token";
import {Keypair, PublicKey} from "@solana/web3.js";
import * as fs from "node:fs";
import * as path from "node:path";
import {fileURLToPath} from "node:url";


const program = anchor.workspace.dgSolanaPrograms as Program<DgSolanaPrograms>;

const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");


let mint: PublicKey;
let userTokenAccount: PublicKey;
let recipientTokenAccount: PublicKey;
let adminTokenAccount: PublicKey;
let transferDataPda: PublicKey;
let authority: Wallet;
let bump: number;
let admin = loadKeypair("keys/admin.json");;



const isoTimestamp = new Date().toISOString();
const transferId = "t" + isoTimestamp;
const amount = 1_000_000; // 1 token with 6 decimals
let recipient: Keypair;

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


describe("dg-solana-programs", () => {
    before(async() => {

        await prepare();

        let connection = new web3.Connection("https://solana-devnet.api.syndica.io/api-key/3BrTAJSHwjMSUC3WxMHx72VSqUKrBJFciFbEd2RfabjZJ6F9LNhBdqQq3PkJxd2C9rKf5zBG1UNjf7NywRw1utuQwzMktZt1bSd", 'confirmed')
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
        console.log("ProgramID: ", program.programId.toBase58());
        console.log("Authority: ", authority.publicKey.toBase58());
        // ATAs for USDC
        const adminAta = await getOrCreateAssociatedTokenAccount(
            provider.connection,
            authority.payer,
            USDC_MINT,
            authority.publicKey,
        );
        adminTokenAccount = adminAta.address;
        console.log("Admin USDC ATA:", adminTokenAccount.toBase58());

        const recipientAta = await getOrCreateAssociatedTokenAccount(
            provider.connection,
            authority.payer,
            USDC_MINT,
            recipient.publicKey
        );
        recipientTokenAccount = recipientAta.address;
        console.log("Recipient USDC ATA:", recipientTokenAccount.toBase58());

        const tx = new web3.Transaction().add(
            createTransferInstruction(
                adminTokenAccount,
                recipientTokenAccount,
                authority.publicKey, // admin is the ATA owner
                amount,
                [],
                TOKEN_PROGRAM_ID
            )
        );
        const sig = await provider.sendAndConfirm(tx, []);
        console.log("USDC transfer sig:", sig);

        // Find PDA
        [transferDataPda, bump] = await PublicKey.findProgramAddress(
            [Buffer.from("transfer_data")],
            program.programId
        );
        console.log("transferDataPDA: {}", transferDataPda.toBase58());
    });


    it("Deposit transfer details", async () => {

        await program.methods
            .deposit(transferId, new anchor.BN(amount), recipient.publicKey)
            .accounts({
                transferData: transferDataPda,
                authority: authority.publicKey,
            })
            .rpc();

    });
})