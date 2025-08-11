import * as anchor from "@coral-xyz/anchor";
import {Program, Wallet} from "@coral-xyz/anchor";
import * as web3 from "@solana/web3.js";
import { DgSolanaPrograms } from "../target/types/dg_solana_programs";
import {createAccount, createMint, mintTo, TOKEN_PROGRAM_ID} from "@solana/spl-token";
import * as assert from "node:assert";
import {Keypair, PublicKey} from "@solana/web3.js";
import * as fs from "node:fs";
import * as path from "node:path";


const program = anchor.workspace.dgSolanaPrograms as Program<DgSolanaPrograms>;

function loadKeypair(filePath: string): web3.Keypair {
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
const isoTimestamp = new Date().toISOString();
const transferId = "t" + isoTimestamp;
const amount = 1_000_000; // 1 token with 6 decimals

let recipient: Keypair;

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

        // https://falling-wiser-moon.solana-devnet.quiknode.pro/653e836d3a2a94fb452fdc2a3796b420cb809b10
        let connection = new web3.Connection("https://solana-devnet.api.syndica.io/api-key/3BrTAJSHwjMSUC3WxMHx72VSqUKrBJFciFbEd2RfabjZJ6F9LNhBdqQq3PkJxd2C9rKf5zBG1UNjf7NywRw1utuQwzMktZt1bSd", 'confirmed')
        const provider = new anchor.AnchorProvider(connection, new anchor.Wallet(admin), {
            commitment: 'confirmed'
        })
        anchor.setProvider(provider);
        authority = provider.wallet;

        // Check admin balance
        const balance = await provider.connection.getBalance(admin.publicKey);
        console.log(`Admin balance: ${balance / web3.LAMPORTS_PER_SOL} SOL`);
        if (balance < web3.LAMPORTS_PER_SOL * 0.01) {
            throw new Error("Insufficient balance in admin account. Please fund it with SOL.");
        }

        console.log("authority: ", authority.publicKey.toBase58());
        // Create a mint
        mint = await createMint(
            provider.connection,
            authority.payer,
            authority.publicKey,
            null,
            6
        );
        console.log("Mint created:", mint.toBase58())
        // Create token accounts
        userTokenAccount = await createAccount(
            provider.connection,
            authority.payer,
            mint,
            authority.publicKey,
        );
        console.log("userTokenAccount: ", userTokenAccount.toBase58());
        recipientTokenAccount = await createAccount(
            provider.connection,
            authority.payer,
            mint,
            recipient.publicKey,
        );
        console.log("recipientTokenAccount: ", recipientTokenAccount.toBase58());
        // Mint tokens to user
        await mintTo(
            provider.connection,
            authority.payer,
            mint,
            userTokenAccount,
            authority.publicKey,
            amount
        );
        console.log("Mint token successfully");
        console.log("ProgramID:  {}", program.programId.toBase58());

        // Find PDA
        [transferDataPda, bump] = await PublicKey.findProgramAddress(
            [Buffer.from("transfer_data")],
            program.programId
        );
        console.log("transferDataPDA: {}", transferDataPda.toBase58());
    });


    it("Modifies PDA authority", async () => {
        const newAuthority = anchor.web3.Keypair.generate();
        await program.methods
            .modifyPdaAuthority(newAuthority.publicKey)
            .accounts({
                transferData: transferDataPda,
                currentAuthority: authority.publicKey,
            })
            .rpc();
    });

});