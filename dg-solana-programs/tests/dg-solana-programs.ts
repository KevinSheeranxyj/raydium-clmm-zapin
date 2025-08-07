import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {PublicKey, } from "@solana/web3.js";
import { DgSolanaPrograms } from "../target/types/dg_solana_programs";
import {createAccount, createMint, mintTo} from "@solana/spl-token";
import {min} from "bn.js";

describe("dg-solana-programs", () => {
  // Configure the client to use the local cluster.
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.dgSolanaPrograms as Program<DgSolanaPrograms>;

  let mint: PublicKey;
  let userTokenAccount: PublicKey;
  let recipientTokenAccount: PublicKey;
  let transferDataPda: PublicKey;
  let bump: number;

  const authority = provider.wallet;
  const recipient = anchor.web3.Keypair.generate();
  const transferId = "t1219282211";
  const amount = 1_000_000; // 1 token with 6 decimals


  before(async () => {
    // Create a mint
    mint = await createMint(
        provider.connection,
        authority.payer,
        authority.publicKey,
        null,
        6
    );

    // Create token accounts
    userTokenAccount = await createAccount(
        provider.connection,
        authority.payer,
        mint,
        authority.publicKey,
    );
    recipientTokenAccount = await createAccount(
        provider.connection,
        authority.payer,
        mint,
        recipient.publicKey,
    );
    // Mint tokens to user
    await mintTo(
        provider.connection,
        authority.payer,
        mint,
        userTokenAccount,
        authority.publicKey,
        amount
    );
    // Find PDA
    [transferDataPda, bump] = await PublicKey.findProgramAddress(
        [Buffer.from("transfer_data")],
        program.programId
    )

  })

  it("Is initialized!", async () => {
    // Add your test here.
    const tx = await program.methods.initialize().rpc();
    console.log("Your transaction signature", tx);
  });
});
