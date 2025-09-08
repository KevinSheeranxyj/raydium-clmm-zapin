import {
    Keypair, PublicKey, SystemProgram, Connection, LAMPORTS_PER_SOL
} from "@solana/web3.js";
import {
    createMint, getOrCreateAssociatedTokenAccount,
    mintTo, getAccount, createAccount, TOKEN_PROGRAM_ID
} from "@solana/spl-token";
import * as anchor from "@coral-xyz/anchor";

export async function airdrop(
    connection: Connection,
    pubkey: PublicKey,
    sol = 2 * LAMPORTS_PER_SOL
) {
    const sig = await connection.requestAirdrop(pubkey, sol);
    await connection.confirmTransaction(sig, "confirmed");
}

export async function createUserWithSol(
    provider: anchor.AnchorProvider
) {
    const kp = Keypair.generate();
    await airdrop(provider.connection, kp.publicKey);
    return kp;
}

export async function createMintAndATA(
    provider: anchor.AnchorProvider,
    owner: PublicKey,
    decimals = 6,
    initialAmount = 0n
) {
    const mint = await createMint(
        provider.connection,
        (provider.wallet as anchor.Wallet).payer,
        owner, // mint authority
        owner, // freeze authority
        decimals
    );
    const ata = await getOrCreateAssociatedTokenAccount(
        provider.connection,
        (provider.wallet as anchor.Wallet).payer,
        mint,
        owner
    );
    if (initialAmount > 0n) {
        await mintTo(
            provider.connection,
            (provider.wallet as anchor.Wallet).payer,
            mint,
            ata.address,
            owner,
            Number(initialAmount)
        );
    }
    return { mint, ata };
}

export async function createRawTokenAccountOwnedBy(
    provider: anchor.AnchorProvider,
    mint: PublicKey,
    owner: PublicKey // 可以是 PDA
) {
    const acc = await createAccount(
        provider.connection,
        (provider.wallet as anchor.Wallet).payer,
        mint,
        owner
    );
    return acc;
}

export async function getTokenAmount(
    provider: anchor.AnchorProvider,
    ata: PublicKey
) {
    const info = await getAccount(provider.connection, ata);
    return Number(info.amount);
}